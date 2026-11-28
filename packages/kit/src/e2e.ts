/**
 * End-to-end encryption of synced sessions on WebCrypto (eng-architecture.md §4.3, the key directory ADR's I1 to I3).
 * Nexus UI's session viewer uses it. It writes and reads the same bytes as nexus-client's e2e module
 * (`rs/crates/nexus-client/src/e2e.rs`), and the tests check both against `tests/e2e-vectors.json` in that crate.
 *
 * - Device keys: an X25519 pair receives locked content keys and an Ed25519 pair signs. The private keys are
 *   `CryptoKey`s that cannot be exported; `e2e-store.ts` keeps them in IndexedDB.
 * - Locked copies: HPKE (RFC 9180) base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM. The `info` is
 *   {@link LOCK_INFO_LABEL}, the thread id, the epoch and the holder id, length-prefixed; the associated data is empty.
 *   Bytes are `enc (32) || ciphertext (32) || tag (16)`.
 * - Events: AES-256-GCM. The associated data is {@link EVENT_AAD_LABEL}, the thread id, the epoch and the sequence
 *   number, length-prefixed. Bytes are `nonce (12) || ciphertext || tag (16)`.
 * - Signatures: Ed25519 over Gateway's colon-separated texts. Fingerprints: ADR I3, with a 16-character short code.
 * - Length prefixes: each field's byte length as 2 big-endian bytes, then its bytes. Epochs and sequence numbers are
 *   8 big-endian bytes.
 *
 * - Recovery keys (ADR I6): a passkey's PRF output or a printable recovery code derives an X25519 holder with HKDF-SHA256
 *   ({@link recoveryCodeSecret}, {@link passkeySecret}). A new device unlocks the holder's copies with it and locks
 *   them again for itself ({@link lockContentKey}).
 *
 * - Handovers: an approver signs {@link handoverMessage} over its trusted list and seals the signature and
 *   the list to the device it approves ({@link sealHandover}, {@link openHandover}). HPKE as a lock, with the `info`
 *   {@link HANDOVER_LABEL}, the device id and the approver id. The plaintext is the signature, then the list.
 *
 * This side opens what a runner seals; it does not seal events. It locks a content key only to copy history to a new
 * device after a recovery. Nothing here imports a browser API at the
 * top level: every call reaches `globalThis.crypto.subtle` when it runs, so the phone can reuse the format code.
 */

import type { HolderKind } from "./api";

export const EVENT_AAD_LABEL = "nexus-event-v1";
export const LOCK_INFO_LABEL = "nexus-thread-key-v1";
/** The first field of HPKE's `info` when a team key's private half is locked to a holder (ADR 0011, M5). */
export const TEAM_KEY_LOCK_LABEL = "nexus-team-key-v1";
export const FINGERPRINT_LABEL = "nexus-holder-fingerprint-v1";
/** The first field of HPKE's `info` when Gateway seals a recovery challenge to a recovery key (ADR I4). */
export const RECOVER_CHALLENGE_LABEL = "nexus-recover-challenge-v1";
/** The HKDF salt that turns a recovery code's 16 bytes into its holder's X25519 private key (ADR I6). */
export const RECOVERY_CODE_SALT = "nexus-recovery-code-v1";
/** The HKDF salt that turns a passkey's PRF output into its holder's X25519 private key (ADR I6). */
export const PASSKEY_SALT = "nexus-passkey-v1";
/** The text whose SHA-256 is the passkey's PRF input, WebAuthn's `prf.eval.first`. */
export const PASSKEY_PRF_LABEL = "nexus-passkey-prf-v1";
/** The first field of HPKE's `info` when an approver seals its trusted list to the device it approves. */
export const HANDOVER_LABEL = "nexus-device-handover-v1";
/** The longest handover payload a device seals: its sealed form fits Gateway's 65,536 base64url characters. */
export const MAX_HANDOVER_PAYLOAD = 48_000;
/** A recovery code's random bytes: 128 bits. */
export const RECOVERY_CODE_BYTES = 16;
const MAX_PERSON_ID_LEN = 128;
const MAX_CREDENTIAL_ID_LEN = 1023;
/** Crockford's base32 digits, then the five extra symbols of its mod-37 check symbol. */
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ*~$=U";
const RECOVERY_CODE_DIGITS = 26;
const RECOVERY_CODE_GROUPS = [5, 5, 5, 4, 4, 4];

const KEY_LEN = 32;
const NONCE_LEN = 12;
const TAG_LEN = 16;
/** A device's stored secret: its X25519 private key, then its Ed25519 seed. */
export const DEVICE_SECRET_LEN = 2 * KEY_LEN;
/** A locked content key: HPKE's encapsulated key, then the sealed content key and its tag. */
export const LOCKED_LEN = KEY_LEN + KEY_LEN + TAG_LEN;
export const SIGNATURE_LEN = 64;
const SHORT_CODE_BYTES = 10;
const MAX_HOLDER_ID_LEN = 128;
/** Gateway's key version is a 32-bit signed integer. */
const MAX_KEY_VERSION = 2 ** 31 - 1;

/** Bytes WebCrypto accepts: backed by a plain `ArrayBuffer`. */
export type Bytes = Uint8Array<ArrayBuffer>;

/**
 * `open`: the bytes did not open (the wrong key, thread, epoch, event number or holder, or changed since they were
 * sealed), deliberately one answer for every cause. `field`: a thread id, holder id, epoch, sequence number, person id
 * or credential id is not in the form the format binds. `code`: the text is not a recovery code (its length, a
 * character, or its check symbol). `key`: a holder's X25519 key cannot be locked to.
 */
export class E2eError extends Error {
  constructor(readonly kind: "open" | "field" | "code" | "key") {
    super(E2E_MESSAGES[kind]);
    this.name = "E2eError";
  }
}

const E2E_MESSAGES: Record<E2eError["kind"], string> = {
  open: "the encrypted data could not be opened",
  field: "a thread id, holder id, epoch or sequence number is not in the form the format binds",
  code: "That is not a recovery code. Check each character against the printed code.",
  key: "the holder's public key is not a usable X25519 key",
};

/** A holder that receives locked content keys, such as a recovery key (ADR I6): an X25519 pair. */
export interface Holder {
  /** The X25519 public key, 32 bytes. */
  readonly encryptionKey: Bytes;
  /** The X25519 private key: `deriveBits` only, not exportable. */
  readonly x25519: CryptoKey;
}

/** This device's two key pairs (ADR I1). */
export interface DeviceKey extends Holder {
  /** The Ed25519 public key, 32 bytes. */
  readonly signingKey: Bytes;
  /** The Ed25519 private key: `sign` only, not exportable. */
  readonly ed25519: CryptoKey;
}

/** A thread's content key at one epoch: `decrypt` only, not exportable. */
export interface ContentKey {
  readonly key: CryptoKey;
  readonly epoch: number;
}

function subtle(): SubtleCrypto {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) throw new Error("WebCrypto (crypto.subtle) is not available here");
  return subtle;
}

/** Whether this browser's WebCrypto has X25519 and Ed25519. Current Chrome, Firefox and Safari do; check before use. */
export async function e2eSupported(): Promise<boolean> {
  try {
    await subtle().generateKey({ name: "X25519" }, false, ["deriveBits"]);
    await subtle().generateKey({ name: "Ed25519" }, false, ["sign"]);
    return true;
  } catch {
    return false;
  }
}

/**
 * A new device, made on this device. Its private keys cannot leave WebCrypto. With `wrappable`, the X25519 key can, so
 * that a store wraps it (`e2e-store.ts`); the store gives back a copy that cannot.
 */
export async function generateDeviceKey(wrappable = false): Promise<DeviceKey> {
  const x25519 = (await subtle().generateKey({ name: "X25519" }, wrappable, ["deriveBits"])) as CryptoKeyPair;
  const ed25519 = (await subtle().generateKey({ name: "Ed25519" }, false, ["sign"])) as CryptoKeyPair;
  return {
    encryptionKey: new Uint8Array(await subtle().exportKey("raw", x25519.publicKey)),
    x25519: x25519.privateKey,
    signingKey: new Uint8Array(await subtle().exportKey("raw", ed25519.publicKey)),
    ed25519: ed25519.privateKey,
  };
}

// PKCS #8 wrappers of a raw 32-byte private key (RFC 8410): X25519 is OID 1.3.101.110, Ed25519 1.3.101.112.
const X25519_PKCS8 = hex("302e020100300506032b656e04220420");
const ED25519_PKCS8 = hex("302e020100300506032b657004220420");

/** A device from its stored secret: the X25519 private key, then the Ed25519 seed (nexus-client's `secret_bytes`). */
export async function importDeviceSecret(secret: Uint8Array): Promise<DeviceKey> {
  if (secret.length !== DEVICE_SECRET_LEN) throw new RangeError(`a device secret is ${DEVICE_SECRET_LEN} bytes`);
  const holder = await importHolderSecret(secret.subarray(0, KEY_LEN));
  const [ed25519, signingKey] = await importPrivate("Ed25519", ED25519_PKCS8, secret.subarray(KEY_LEN), ["sign"]);
  return { ...holder, signingKey, ed25519 };
}

/** A holder from its 32-byte X25519 private key, such as a recovery key the person types in. */
export async function importHolderSecret(secret: Uint8Array): Promise<Holder> {
  if (secret.length !== KEY_LEN) throw new RangeError(`an X25519 private key is ${KEY_LEN} bytes`);
  const [x25519, encryptionKey] = await importPrivate("X25519", X25519_PKCS8, secret, ["deriveBits"]);
  return { encryptionKey, x25519 };
}

async function importPrivate(
  name: "X25519" | "Ed25519",
  prefix: Bytes,
  secret: Uint8Array,
  usages: KeyUsage[],
): Promise<[CryptoKey, Bytes]> {
  const pkcs8 = concat(prefix, secret);
  try {
    // WebCrypto gives a private key's public half only through its JWK, so import once to export that, then again
    // without export.
    const open = await subtle().importKey("pkcs8", pkcs8, { name }, true, usages);
    const publicKey = fromB64u((await subtle().exportKey("jwk", open)).x ?? "");
    if (publicKey?.length !== KEY_LEN) throw new Error(`WebCrypto gave no ${name} public key`);
    return [await subtle().importKey("pkcs8", pkcs8, { name }, false, usages), publicKey];
  } finally {
    pkcs8.fill(0);
  }
}

/** A content key from its raw 32 bytes, for `epoch`. Keys from Gateway come through {@link unlockContentKey}. */
export async function importContentKey(raw: Uint8Array, epoch: number): Promise<ContentKey> {
  if (raw.length !== KEY_LEN) throw new RangeError(`a content key is ${KEY_LEN} bytes`);
  u64(epoch);
  return { key: await subtle().importKey("raw", copy(raw), "AES-GCM", false, ["decrypt"]), epoch };
}

/**
 * Opens a content key a runner locked to `holder` as `holderId`, for `threadId` at `epoch` (HPKE base mode). The key
 * it returns opens that epoch's events.
 */
export async function unlockContentKey(
  holder: Holder,
  threadId: string,
  epoch: number,
  holderId: string,
  locked: Uint8Array,
): Promise<ContentKey> {
  const raw = await unlockContentKeyBytes(holder, threadId, epoch, holderId, locked);
  try {
    return await importContentKey(raw, epoch);
  } finally {
    raw.fill(0);
  }
}

/**
 * {@link unlockContentKey}'s raw 32 bytes, to lock again for another holder with {@link lockContentKey} after a recovery.
 * The caller wipes them (`fill(0)`) when done.
 */
export async function unlockContentKeyBytes(
  holder: Holder,
  threadId: string,
  epoch: number,
  holderId: string,
  locked: Uint8Array,
): Promise<Bytes> {
  const info = lockInfo(threadId, epoch, holderId);
  if (locked.length !== LOCKED_LEN) throw new E2eError("open");
  try {
    return await hpkeOpen(holder, locked.slice(0, KEY_LEN), locked.slice(KEY_LEN), info);
  } catch {
    throw new E2eError("open");
  }
}

/**
 * Opens the team key's private half locked to `holder` as `holderId`, for organization `tenantId` at `epoch` (ADR 0011,
 * M7): nexus-client's `DeviceKey::unlock_team_key`. The team key then opens a thread key locked to it, with
 * {@link unlockContentKey} and the team key's id as the holder id.
 */
export async function unlockTeamKey(holder: Holder, tenantId: string, epoch: number, holderId: string, locked: Uint8Array): Promise<Holder> {
  const info = teamLockInfo(tenantId, epoch, holderId);
  if (locked.length !== LOCKED_LEN) throw new E2eError("open");
  let secret: Bytes;
  try {
    secret = await hpkeOpen(holder, locked.slice(0, KEY_LEN), locked.slice(KEY_LEN), info);
  } catch {
    throw new E2eError("open");
  }
  try {
    return await importHolderSecret(secret);
  } finally {
    secret.fill(0);
  }
}

/** Locks a team key's 32-byte private half to `holderKey` as `holderId` (HPKE base mode): `HolderKey::lock_team_key_for`. */
export async function lockTeamKey(secret: Uint8Array, tenantId: string, epoch: number, holderId: string, holderKey: Uint8Array): Promise<Bytes> {
  const ephemeral = (await subtle().generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  const enc = new Uint8Array(await subtle().exportKey("raw", ephemeral.publicKey));
  return hpkeSeal(ephemeral.privateKey, enc, secret, teamLockInfo(tenantId, epoch, holderId), holderKey);
}

/** The HPKE `info` for a team key's copy: the label, the organization id, the epoch (from 1) and the holder id. */
export function teamLockInfo(tenantId: string, epoch: number, holderId: string): Bytes {
  checkHolderId(tenantId);
  checkHolderId(holderId);
  if (epoch < 1) throw new E2eError("field");
  return lengthPrefixed([utf8(TEAM_KEY_LOCK_LABEL), utf8(tenantId), u64(epoch), utf8(holderId)]);
}

/**
 * Opens the recovery challenge Gateway sealed to `holder` as `holderId`, for `personId` (ADR I4): the proof that this
 * device holds the recovery key. Its 32 bytes go back to Gateway as base64url.
 */
export async function openRecoveryChallenge(holder: Holder, personId: string, holderId: string, sealed: Uint8Array): Promise<Bytes> {
  const info = recoverChallengeInfo(personId, holderId);
  if (sealed.length !== LOCKED_LEN) throw new E2eError("open");
  try {
    return await hpkeOpen(holder, sealed.slice(0, KEY_LEN), sealed.slice(KEY_LEN), info);
  } catch {
    throw new E2eError("open");
  }
}

/** ADR I4's HPKE `info` for a recovery challenge: the label, the person id and the holder id, length-prefixed. */
export function recoverChallengeInfo(personId: string, holderId: string): Bytes {
  checkHolderId(holderId);
  return lengthPrefixed([utf8(RECOVER_CHALLENGE_LABEL), personIdBytes(personId), utf8(holderId)]);
}

/**
 * Locks a content key's raw bytes to `holderKey` as holder `holderId`, for `threadId` at `epoch` (HPKE base mode, ADR I1's
 * `info`): nexus-client's `ContentKey::lock_for`, byte for byte. The ephemeral key is new each call.
 */
export async function lockContentKey(raw: Uint8Array, threadId: string, epoch: number, holderId: string, holderKey: Uint8Array): Promise<Bytes> {
  const ephemeral = (await subtle().generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  const enc = new Uint8Array(await subtle().exportKey("raw", ephemeral.publicKey));
  return hpkeSeal(ephemeral.privateKey, enc, raw, lockInfo(threadId, epoch, holderId), holderKey);
}

/** {@link lockContentKey} with the ephemeral key from RFC 9180's DeriveKeyPair(`ikmE`): for the shared vectors only. */
export async function lockContentKeyWithIkm(
  raw: Uint8Array,
  threadId: string,
  epoch: number,
  holderId: string,
  holderKey: Uint8Array,
  ikmE: Uint8Array,
): Promise<Bytes> {
  const prk = await labeledExtract(KEM_SUITE, new Uint8Array(), "dkp_prk", copy(ikmE));
  const secret = await labeledExpand(KEM_SUITE, prk, "sk", new Uint8Array(), KEY_LEN);
  const [privateKey, enc] = await importPrivate("X25519", X25519_PKCS8, secret, ["deriveBits"]);
  secret.fill(0);
  return hpkeSeal(privateKey, enc, raw, lockInfo(threadId, epoch, holderId), holderKey);
}

/**
 * Seals an approver's `signature` over {@link handoverMessage} and the `payload` to the new device's X25519 key
 * (`deviceKey`), as `deviceId` approved by `approverId`: nexus-client's `seal_handover`. The ephemeral key is new each
 * call.
 */
export async function sealHandover(deviceId: string, approverId: string, deviceKey: Uint8Array, signature: Uint8Array, payload: Uint8Array): Promise<Bytes> {
  const info = handoverInfo(deviceId, approverId);
  if (signature.length !== SIGNATURE_LEN || payload.length > MAX_HANDOVER_PAYLOAD) throw new RangeError("a handover is a signature and at most 48,000 bytes");
  const ephemeral = (await subtle().generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  const enc = new Uint8Array(await subtle().exportKey("raw", ephemeral.publicKey));
  return hpkeSealBytes(ephemeral.privateKey, enc, concat(signature, payload), info, deviceKey);
}

/** A handover this device opened: the approver's signature and the payload. Not trusted until the signature verifies. */
export interface OpenedHandover {
  signature: Bytes;
  payload: Bytes;
}

/**
 * Opens a handover sealed to `holder` as `deviceId` by `approverId`. It does not check the signature: anyone who knows
 * this device's public key can seal one, Gateway included. Check it with {@link handoverMessage} and {@link verify}
 * against the approver this device trusts.
 */
export async function openHandover(holder: Holder, deviceId: string, approverId: string, sealed: Uint8Array): Promise<OpenedHandover> {
  const info = handoverInfo(deviceId, approverId);
  if (sealed.length < KEY_LEN + SIGNATURE_LEN + TAG_LEN || sealed.length > KEY_LEN + SIGNATURE_LEN + MAX_HANDOVER_PAYLOAD + TAG_LEN) throw new E2eError("open");
  let opened: Bytes;
  try {
    opened = await hpkeOpen(holder, sealed.slice(0, KEY_LEN), sealed.slice(KEY_LEN), info);
  } catch {
    throw new E2eError("open");
  }
  return { signature: opened.slice(0, SIGNATURE_LEN), payload: opened.slice(SIGNATURE_LEN) };
}

/** What an approver signs over its handover: `nexus-device-handover:v1:{deviceId}:{approverId}:{payload}`, base64url. */
export function handoverMessage(deviceId: string, approverId: string, payload: Uint8Array): string {
  checkHolderId(deviceId);
  checkHolderId(approverId);
  return `nexus-device-handover:v1:${deviceId}:${approverId}:${b64u(payload)}`;
}

/** The HPKE `info` of a handover: {@link HANDOVER_LABEL}, the device id and the approver id, length-prefixed. */
export function handoverInfo(deviceId: string, approverId: string): Bytes {
  checkHolderId(deviceId);
  checkHolderId(approverId);
  return lengthPrefixed([utf8(HANDOVER_LABEL), utf8(deviceId), utf8(approverId)]);
}

/** Seals an event with a fresh nonce in the runner's wire format. The caller owns the raw key's lifetime. */
export async function sealEvent(raw: Uint8Array, epoch: number, threadId: string, seq: number, plaintext: Uint8Array): Promise<Bytes> {
  if (raw.length !== KEY_LEN) throw new RangeError(`a content key is ${KEY_LEN} bytes`);
  const aad = eventAad(threadId, epoch, seq);
  const iv = globalThis.crypto.getRandomValues(new Uint8Array(NONCE_LEN));
  const key = await subtle().importKey("raw", copy(raw), "AES-GCM", false, ["encrypt"]);
  const ciphertext = new Uint8Array(await subtle().encrypt({ name: "AES-GCM", iv, additionalData: aad, tagLength: 128 }, key, copy(plaintext)));
  const sealed = new Uint8Array(iv.length + ciphertext.length);
  sealed.set(iv);
  sealed.set(ciphertext, iv.length);
  return sealed;
}

/** Opens event `seq` of `threadId`. Fails unless it was sealed with this key, at its epoch, for that thread and number. */
export async function openEvent(content: ContentKey, threadId: string, seq: number, sealed: Uint8Array): Promise<Bytes> {
  const aad = eventAad(threadId, content.epoch, seq);
  if (sealed.length < NONCE_LEN + TAG_LEN) throw new E2eError("open");
  try {
    const iv = sealed.slice(0, NONCE_LEN);
    return new Uint8Array(
      await subtle().decrypt({ name: "AES-GCM", iv, additionalData: aad, tagLength: 128 }, content.key, sealed.slice(NONCE_LEN)),
    );
  } catch {
    throw new E2eError("open");
  }
}

/** The Ed25519 signature over `message`: one of {@link approvalMessage}, {@link threadKeyMessage} or {@link revocationMessage}. */
export async function sign(device: DeviceKey, message: string): Promise<Bytes> {
  return new Uint8Array(await subtle().sign("Ed25519", device.ed25519, utf8(message)));
}

/** Whether `signature` is `signingKey`'s Ed25519 signature over `message`, as Gateway checks it. */
export async function verify(signingKey: Uint8Array, message: string, signature: Uint8Array): Promise<boolean> {
  if (signingKey.length !== KEY_LEN || signature.length !== SIGNATURE_LEN) return false;
  try {
    const key = await subtle().importKey("raw", copy(signingKey), "Ed25519", false, ["verify"]);
    return await subtle().verify("Ed25519", key, copy(signature), utf8(message));
  } catch {
    return false;
  }
}

/**
 * A holder's fingerprint (ADR I3): the SHA-256 of {@link FINGERPRINT_LABEL}, its X25519 key and its Ed25519 key, each
 * length-prefixed. A recovery key has no Ed25519 key: leave it out, and that field is empty.
 */
export async function fingerprint(encryptionKey: Uint8Array, signingKey?: Uint8Array): Promise<Bytes> {
  if (encryptionKey.length !== KEY_LEN || (signingKey && signingKey.length !== KEY_LEN)) {
    throw new RangeError(`a public key is ${KEY_LEN} bytes`);
  }
  const bytes = lengthPrefixed([utf8(FINGERPRINT_LABEL), encryptionKey, signingKey ?? new Uint8Array()]);
  return new Uint8Array(await subtle().digest("SHA-256", bytes));
}

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** The code a terminal shows instead of a QR code (ADR I3): the fingerprint's first 10 bytes as 16 base32 characters. */
export function shortCode(fingerprint: Uint8Array): string {
  if (fingerprint.length !== KEY_LEN) throw new RangeError(`a fingerprint is ${KEY_LEN} bytes`);
  let code = "";
  let bits = 0;
  let value = 0;
  for (const byte of fingerprint.subarray(0, SHORT_CODE_BYTES)) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      code += BASE32[(value >>> bits) & 31];
    }
    value &= (1 << bits) - 1;
  }
  return code;
}

/** What an approver signs for a new device: `nexus-device-approval:v1:{deviceId}:{encryptionKey}:{signingKey}`. */
export function approvalMessage(deviceId: string, encryptionKey: Uint8Array, signingKey: Uint8Array): string {
  checkHolderId(deviceId);
  if (encryptionKey.length !== KEY_LEN || signingKey.length !== KEY_LEN) throw new RangeError(`a public key is ${KEY_LEN} bytes`);
  return `nexus-device-approval:v1:${deviceId}:${b64u(encryptionKey)}:${b64u(signingKey)}`;
}

/**
 * What an approver signs for a recovery key (ADR I2, I6): `nexus-device-approval:v2:{holderId}:{kind}:{encryptionKey}:`,
 * the Ed25519 field empty. A device's approval stays {@link approvalMessage} (v1), so neither stands for the other.
 */
export function holderApprovalMessage(holderId: string, kind: Exclude<HolderKind, "device">, encryptionKey: Uint8Array): string {
  checkHolderId(holderId);
  if (encryptionKey.length !== KEY_LEN) throw new RangeError(`a public key is ${KEY_LEN} bytes`);
  if (kind !== "passkey" && kind !== "recovery_code") throw new E2eError("field");
  return `nexus-device-approval:v2:${holderId}:${kind}:${b64u(encryptionKey)}:`;
}

/** A new recovery code: 128 bits from the platform's random source. */
export function generateRecoveryCode(): Bytes {
  return globalThis.crypto.getRandomValues(new Uint8Array(RECOVERY_CODE_BYTES));
}

/**
 * The code as printed: 26 Crockford base32 digits of its 128 bits, most significant first, then Crockford's mod-37
 * check symbol, in groups of 5, 5, 5, 4, 4 and 4 joined by hyphens.
 */
export function formatRecoveryCode(code: Uint8Array): string {
  if (code.length !== RECOVERY_CODE_BYTES) throw new RangeError(`a recovery code is ${RECOVERY_CODE_BYTES} bytes`);
  const value = code.reduce((sum, byte) => (sum << BigInt(8)) | BigInt(byte), BigInt(0));
  let symbols = "";
  for (let place = RECOVERY_CODE_DIGITS - 1; place >= 0; place--) symbols += CROCKFORD[Number((value >> BigInt(5 * place)) & BigInt(31))];
  symbols += CROCKFORD[Number(value % BigInt(37))];
  const groups: string[] = [];
  let at = 0;
  for (const size of RECOVERY_CODE_GROUPS) {
    groups.push(symbols.slice(at, at + size));
    at += size;
  }
  return groups.join("-");
}

/**
 * A recovery code as typed: any case, hyphens and spaces ignored, `O` read as `0` and `I` and `L` as `1`. Throws
 * {@link E2eError} `code` for anything else, a wrong check symbol included, before any network call.
 */
export function parseRecoveryCode(text: string): Bytes {
  const symbols = text
    .replace(/[\s-]/g, "")
    .toUpperCase()
    .replace(/O/g, "0")
    .replace(/[IL]/g, "1");
  if (symbols.length !== RECOVERY_CODE_DIGITS + 1) throw new E2eError("code");
  let value = BigInt(0);
  for (let place = 0; place < RECOVERY_CODE_DIGITS; place++) {
    const digit = CROCKFORD.slice(0, 32).indexOf(symbols[place]!);
    // 26 digits carry 130 bits: the first is at most 7, or the value passes 128 bits.
    if (digit < 0 || (place === 0 && digit > 7)) throw new E2eError("code");
    value = (value << BigInt(5)) | BigInt(digit);
  }
  if (CROCKFORD[Number(value % BigInt(37))] !== symbols[RECOVERY_CODE_DIGITS]) throw new E2eError("code");
  const bytes = new Uint8Array(RECOVERY_CODE_BYTES);
  for (let i = RECOVERY_CODE_BYTES - 1; i >= 0; i--) {
    bytes[i] = Number(value & BigInt(0xff));
    value >>= BigInt(8);
  }
  return bytes;
}

/**
 * The recovery code's holder secret for `personId` (`users.id`): HKDF-SHA256 with the code's 16 bytes as input keying
 * material, {@link RECOVERY_CODE_SALT} as salt and the length-prefixed person id as info. The code has full entropy, so
 * no slow KDF. {@link importHolderSecret} makes the holder; the caller wipes the bytes.
 */
export async function recoveryCodeSecret(code: Uint8Array, personId: string): Promise<Bytes> {
  if (code.length !== RECOVERY_CODE_BYTES) throw new E2eError("code");
  return hkdf(code, RECOVERY_CODE_SALT, lengthPrefixed([personIdBytes(personId)]));
}

/** The passkey's WebAuthn PRF input, `prf.eval.first`: the SHA-256 of {@link PASSKEY_PRF_LABEL}. */
export async function passkeyPrfInput(): Promise<Bytes> {
  return new Uint8Array(await subtle().digest("SHA-256", utf8(PASSKEY_PRF_LABEL)));
}

/**
 * The passkey's holder secret for `personId`: HKDF-SHA256 with the 32-byte PRF output as input keying material,
 * {@link PASSKEY_SALT} as salt, and the length-prefixed person id and credential id (`rawId`) as info.
 */
export async function passkeySecret(prfOutput: Uint8Array, personId: string, credentialId: Uint8Array): Promise<Bytes> {
  if (prfOutput.length !== KEY_LEN) throw new RangeError(`a PRF output is ${KEY_LEN} bytes`);
  if (credentialId.length < 1 || credentialId.length > MAX_CREDENTIAL_ID_LEN) throw new E2eError("field");
  return hkdf(prfOutput, PASSKEY_SALT, lengthPrefixed([personIdBytes(personId), credentialId]));
}

/** The length-prefixed info a recovery key's derivation binds: exported for the vectors. */
export function derivationInfo(personId: string, credentialId?: Uint8Array): Bytes {
  return lengthPrefixed(credentialId ? [personIdBytes(personId), credentialId] : [personIdBytes(personId)]);
}

/**
 * HKDF-SHA256 (RFC 5869) to 32 bytes, on WebCrypto's HMAC: extract, then one block of expand. The phone's WebCrypto
 * subset has HMAC and no HKDF, so this runs on both.
 */
async function hkdf(ikm: Uint8Array, salt: string, info: Bytes): Promise<Bytes> {
  const prk = await hmac(utf8(salt), copy(ikm));
  try {
    return await hmac(prk, concat(info, Uint8Array.of(1)));
  } finally {
    prk.fill(0);
  }
}

function personIdBytes(personId: string): Bytes {
  const bytes = utf8(personId);
  if (bytes.length < 1 || bytes.length > MAX_PERSON_ID_LEN) throw new E2eError("field");
  return bytes;
}

/** What the device that stores a locked copy signs: `nexus-thread-key:v1:{threadId}:{keyVersion}:{deviceId}:{locked}`. */
export function threadKeyMessage(threadId: string, epoch: number, deviceId: string, locked: Uint8Array): string {
  checkThreadId(threadId);
  checkHolderId(deviceId);
  return `nexus-thread-key:v1:${threadId}:${keyVersion(epoch)}:${deviceId}:${b64u(locked)}`;
}

/** What the device that locks a thread key to the team key signs: `nexus-team-thread-key:v1:{threadId}:{keyVersion}:{teamKeyId}:{locked}`. */
export function teamThreadKeyMessage(threadId: string, epoch: number, teamKeyId: string, locked: Uint8Array): string {
  checkThreadId(threadId);
  checkHolderId(teamKeyId);
  return `nexus-team-thread-key:v1:${threadId}:${keyVersion(epoch)}:${teamKeyId}:${b64u(locked)}`;
}

/** What a revoker signs: `nexus-device-revoke:v1:{deviceId}`. */
export function revocationMessage(deviceId: string): string {
  checkHolderId(deviceId);
  return `nexus-device-revoke:v1:${deviceId}`;
}

/** Gateway's `keyVersion` for `epoch`: Gateway counts from 1, the epoch from 0. */
export function keyVersion(epoch: number): number {
  u64(epoch);
  if (epoch + 1 > MAX_KEY_VERSION) throw new E2eError("field");
  return epoch + 1;
}

/** The epoch of Gateway's `keyVersion`, or `undefined` for a version that is not one. */
export function epochOf(keyVersion: number): number | undefined {
  return Number.isInteger(keyVersion) && keyVersion >= 1 && keyVersion <= MAX_KEY_VERSION ? keyVersion - 1 : undefined;
}

/** ADR I1's event associated data. */
export function eventAad(threadId: string, epoch: number, seq: number): Bytes {
  checkThreadId(threadId);
  return lengthPrefixed([utf8(EVENT_AAD_LABEL), utf8(threadId), u64(epoch), u64(seq)]);
}

/** ADR I1's HPKE `info` for a locked copy. */
export function lockInfo(threadId: string, epoch: number, holderId: string): Bytes {
  checkThreadId(threadId);
  checkHolderId(holderId);
  return lengthPrefixed([utf8(LOCK_INFO_LABEL), utf8(threadId), u64(epoch), utf8(holderId)]);
}

const B64U = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/** Bytes as Gateway reads and stores them: base64url without padding. */
export function b64u(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const left = Math.min(3, bytes.length - i);
    const n = (bytes[i]! << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    for (let j = 0; j <= left; j++) text += B64U[(n >>> (18 - 6 * j)) & 63];
  }
  return text;
}

/** The bytes of canonical base64url without padding, or `undefined` for any other text. */
export function fromB64u(text: string): Bytes | undefined {
  if (text.length % 4 === 1) return undefined;
  const bytes = new Uint8Array(Math.floor((text.length * 3) / 4));
  let bits = 0;
  let value = 0;
  let at = 0;
  for (const char of text) {
    const digit = B64U.indexOf(char);
    if (digit < 0) return undefined;
    value = (value << 6) | digit;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes[at++] = value >>> bits;
    }
    value &= (1 << bits) - 1;
  }
  // Canonical: the bits after the last whole byte are zero, so one byte string has one text.
  return value === 0 ? bytes : undefined;
}

// HPKE (RFC 9180), base mode, single shot: the receiver's half of DHKEM(X25519, HKDF-SHA256) (§4.1), the key schedule
// (§5.1) and one AES-256-GCM open at sequence number 0 (§5.2). WebCrypto's HKDF runs extract and expand together,
// and RFC 9180 labels each step apart, so the two steps of HKDF (RFC 5869 §2.2, §2.3) run on WebCrypto's HMAC-SHA256.
const HPKE_V1 = utf8("HPKE-v1");
const KEM_SUITE = concat(utf8("KEM"), u16(0x0020));
const HPKE_SUITE = concat(utf8("HPKE"), u16(0x0020), u16(0x0001), u16(0x0002));

async function hpkeOpen(holder: Holder, enc: Bytes, ciphertext: Bytes, info: Bytes): Promise<Bytes> {
  const ephemeral = await subtle().importKey("raw", enc, { name: "X25519" }, true, []);
  // WebCrypto refuses an all-zero shared secret, so a low-order `enc` fails here (RFC 9180 §7.1.4).
  const dh = new Uint8Array(await subtle().deriveBits({ name: "X25519", public: ephemeral }, holder.x25519, 256));
  const eaePrk = await labeledExtract(KEM_SUITE, new Uint8Array(), "eae_prk", dh);
  const shared = await labeledExpand(KEM_SUITE, eaePrk, "shared_secret", concat(enc, holder.encryptionKey), KEY_LEN);
  const pskIdHash = await labeledExtract(HPKE_SUITE, new Uint8Array(), "psk_id_hash", new Uint8Array());
  const infoHash = await labeledExtract(HPKE_SUITE, new Uint8Array(), "info_hash", info);
  const context = concat(Uint8Array.of(0), pskIdHash, infoHash);
  const secret = await labeledExtract(HPKE_SUITE, shared, "secret", new Uint8Array());
  const key = await labeledExpand(HPKE_SUITE, secret, "key", context, KEY_LEN);
  const iv = await labeledExpand(HPKE_SUITE, secret, "base_nonce", context, NONCE_LEN);
  try {
    const aead = await subtle().importKey("raw", key, "AES-GCM", false, ["decrypt"]);
    return new Uint8Array(await subtle().decrypt({ name: "AES-GCM", iv, tagLength: 128 }, aead, ciphertext));
  } finally {
    for (const bytes of [dh, eaePrk, shared, secret, key]) bytes.fill(0);
  }
}

async function hpkeSeal(ephemeral: CryptoKey, enc: Bytes, raw: Uint8Array, info: Bytes, holderKey: Uint8Array): Promise<Bytes> {
  if (raw.length !== KEY_LEN) throw new RangeError(`a content key is ${KEY_LEN} bytes`);
  return hpkeSealBytes(ephemeral, enc, raw, info, holderKey);
}

async function hpkeSealBytes(ephemeral: CryptoKey, enc: Bytes, raw: Uint8Array, info: Bytes, holderKey: Uint8Array): Promise<Bytes> {
  if (holderKey.length !== KEY_LEN) throw new E2eError("key");
  let dh: Bytes;
  try {
    const recipient = await subtle().importKey("raw", copy(holderKey), { name: "X25519" }, true, []);
    // WebCrypto refuses an all-zero shared secret, so a low-order holder key fails here (RFC 9180 §7.1.4).
    dh = new Uint8Array(await subtle().deriveBits({ name: "X25519", public: recipient }, ephemeral, 256));
  } catch {
    throw new E2eError("key");
  }
  const eaePrk = await labeledExtract(KEM_SUITE, new Uint8Array(), "eae_prk", dh);
  const shared = await labeledExpand(KEM_SUITE, eaePrk, "shared_secret", concat(enc, holderKey), KEY_LEN);
  const pskIdHash = await labeledExtract(HPKE_SUITE, new Uint8Array(), "psk_id_hash", new Uint8Array());
  const infoHash = await labeledExtract(HPKE_SUITE, new Uint8Array(), "info_hash", info);
  const context = concat(Uint8Array.of(0), pskIdHash, infoHash);
  const secret = await labeledExtract(HPKE_SUITE, shared, "secret", new Uint8Array());
  const key = await labeledExpand(HPKE_SUITE, secret, "key", context, KEY_LEN);
  const iv = await labeledExpand(HPKE_SUITE, secret, "base_nonce", context, NONCE_LEN);
  try {
    const aead = await subtle().importKey("raw", key, "AES-GCM", false, ["encrypt"]);
    const sealed = new Uint8Array(await subtle().encrypt({ name: "AES-GCM", iv, tagLength: 128 }, aead, copy(raw)));
    return concat(enc, sealed);
  } finally {
    for (const bytes of [dh, eaePrk, shared, secret, key]) bytes.fill(0);
  }
}

function labeledExtract(suite: Bytes, salt: Bytes, label: string, ikm: Bytes): Promise<Bytes> {
  // RFC 5869: an absent salt is 32 zero bytes. WebCrypto refuses an empty HMAC key; HMAC pads a short key with zeros,
  // so both give the same key.
  return hmac(salt.length ? salt : new Uint8Array(32), concat(HPKE_V1, suite, utf8(label), ikm));
}

function labeledExpand(suite: Bytes, prk: Bytes, label: string, info: Bytes, length: number): Promise<Bytes> {
  // Every output here fits in one SHA-256 block, so expand is T(1) = HMAC(prk, info || 0x01), cut to length.
  const labeled = concat(u16(length), HPKE_V1, suite, utf8(label), info);
  return hmac(prk, concat(labeled, Uint8Array.of(1))).then((block) => block.slice(0, length));
}

async function hmac(key: Bytes, data: Bytes): Promise<Bytes> {
  const hmacKey = await subtle().importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await subtle().sign("HMAC", hmacKey, data));
}

/** A thread id in its one form: a lowercase hyphenated UUID (ADR I1, I8). */
function checkThreadId(id: string): void {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw new E2eError("field");
}

/** A holder id as Gateway mints it (`dev_…`): 1 to 128 of `A-Z a-z 0-9 _ -`, so it holds no `:`. */
function checkHolderId(id: string): void {
  if (!new RegExp(`^[A-Za-z0-9_-]{1,${MAX_HOLDER_ID_LEN}}$`).test(id)) throw new E2eError("field");
}

/** 8 big-endian bytes. A JavaScript number reaches 2^53 exactly, far past any epoch or event number. */
function u64(n: number): Bytes {
  if (!Number.isSafeInteger(n) || n < 0) throw new E2eError("field");
  const bytes = new Uint8Array(8);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, Math.floor(n / 2 ** 32));
  view.setUint32(4, n >>> 0);
  return bytes;
}

function u16(n: number): Bytes {
  return Uint8Array.of(n >>> 8, n & 0xff);
}

/** Each field as its length in 2 big-endian bytes, then its bytes. */
function lengthPrefixed(fields: Uint8Array[]): Bytes {
  return concat(...fields.flatMap((field) => [u16(field.length), field]));
}

function concat(...parts: Uint8Array[]): Bytes {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

function copy(bytes: Uint8Array): Bytes {
  return new Uint8Array(bytes);
}

function utf8(text: string): Bytes {
  return new TextEncoder().encode(text) as Bytes;
}

function hex(text: string): Bytes {
  return Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));
}
