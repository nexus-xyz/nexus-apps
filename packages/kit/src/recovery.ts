/**
 * Recovery keys (the key directory ADR's I4 and I6): a person's passkey and printable recovery code, each an
 * X25519 holder that receives locked copies and never signs. Settings › Devices uses this file on the web and on
 * desktop; `e2e.ts` holds the derivations and `pairing.ts` the signer.
 *
 * - Setup, on a trusted device: derive the holder, register it (`kind`), and approve it with this device's signature.
 *   The recovery code is shown once and never stored. The passkey's private key never leaves its authenticator; its
 *   PRF output is the derivation's input.
 * - The way back, on a new device: derive the holder, find it in the directory, and unlock its copies here
 *   ({@link unlockHistory}), before anything changes. Then open the challenge Gateway seals to that recovery key, which
 *   proves it, recover this device as the new root, approve the recovery key again, and lock each content key for this
 *   device ({@link recoverWith}).
 *
 * A passkey is offered only where WebAuthn reports the PRF extension ({@link passkeySupported}). There is no fallback:
 * without PRF, a passkey derives nothing.
 */
import type { CoreClient } from "./core-client";
import type { E2eDevice, ThreadKey } from "./api";
import { CoreError } from "./core-client";
import {
  b64u,
  type Bytes,
  epochOf,
  fromB64u,
  type Holder,
  importHolderSecret,
  lockContentKey,
  openRecoveryChallenge,
  parseRecoveryCode,
  passkeyPrfInput,
  passkeySecret,
  recoveryCodeSecret,
  threadKeyMessage,
  unlockContentKeyBytes,
  verify,
} from "./e2e";
import { type DeviceTrust, DeviceTrustError, signerTrusted, verifyDeviceTrust } from "./device-trust";
import { holderKind, type PairingSigner, type RecoveryKind } from "./pairing";

/**
 * Why a recovery stopped. `passkey`: the browser made no passkey, or its authenticator has no PRF. `cancelled`: the
 * person closed the passkey prompt. `unknown`: no trusted recovery key of the person has this key. `empty`: none of its
 * copies verified and opened here.
 */
export class RecoveryError extends Error {
  constructor(readonly kind: "passkey" | "cancelled" | "unknown" | "empty") {
    super(RECOVERY_MESSAGES[kind]);
    this.name = "RecoveryError";
  }
}

const RECOVERY_MESSAGES: Record<RecoveryError["kind"], string> = {
  passkey: "This passkey cannot unlock keys: its authenticator has no PRF support. Use a recovery code instead.",
  cancelled: "The passkey prompt was closed. Try again when you are ready.",
  unknown: "No recovery key of yours matches. Check the code, or use the passkey you set up.",
  empty: "This recovery key holds no history this device could open.",
};

/** WebAuthn's PRF extension, which the DOM types may not have yet. */
interface PrfOutputs {
  prf?: { enabled?: boolean; results?: { first?: BufferSource } };
}

/** Whether this browser makes passkeys with the PRF extension. Feature-detected, never assumed. */
export async function passkeySupported(): Promise<boolean> {
  const credential = (globalThis as { PublicKeyCredential?: { getClientCapabilities?: () => Promise<Record<string, boolean>> } }).PublicKeyCredential;
  if (!credential?.getClientCapabilities || !globalThis.navigator?.credentials) return false;
  try {
    return (await credential.getClientCapabilities())["extension:prf"] === true;
  } catch {
    return false;
  }
}

function bytesOf(source: BufferSource): Bytes {
  return source instanceof ArrayBuffer ? new Uint8Array(source.slice(0)) : new Uint8Array(source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength));
}

function random(length: number): Bytes {
  return globalThis.crypto.getRandomValues(new Uint8Array(length));
}

/** The holder from a PRF output and the credential that made it. */
async function prfHolder(prf: BufferSource | undefined, personId: string, rawId: ArrayBuffer): Promise<Holder> {
  if (!prf) throw new RecoveryError("passkey");
  const output = bytesOf(prf);
  if (output.length !== 32) throw new RecoveryError("passkey");
  const secret = await passkeySecret(output, personId, new Uint8Array(rawId));
  output.fill(0);
  try {
    return await importHolderSecret(secret);
  } finally {
    secret.fill(0);
  }
}

/** Runs a WebAuthn call; a closed prompt is {@link RecoveryError} `cancelled`. */
async function webauthn<T>(call: () => Promise<T | null>): Promise<T> {
  try {
    const result = await call();
    if (!result) throw new RecoveryError("cancelled");
    return result;
  } catch (error) {
    if (error instanceof RecoveryError) throw error;
    if (error instanceof DOMException && error.name === "NotAllowedError") throw new RecoveryError("cancelled");
    throw error;
  }
}

/**
 * Creates a passkey for this person and derives its holder. The credential is discoverable, so a new device finds it
 * with no stored id. Its user handle is the SHA-256 of the person id, so a second setup replaces the first on the same
 * authenticator. The challenge is random: Gateway never verifies this credential, it only stores the derived public key.
 */
export async function createPasskeyHolder(personId: string, userName: string): Promise<Holder> {
  const first = await passkeyPrfInput();
  const user = new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", new TextEncoder().encode(personId)));
  const created = await webauthn(
    () =>
      navigator.credentials.create({
        publicKey: {
          rp: { name: "Nexus" },
          user: { id: user, name: userName, displayName: userName },
          challenge: random(32),
          pubKeyCredParams: [
            { type: "public-key", alg: -7 },
            { type: "public-key", alg: -257 },
          ],
          authenticatorSelection: { residentKey: "required", userVerification: "required" },
          extensions: { prf: { eval: { first } } } as AuthenticationExtensionsClientInputs,
        },
      }) as Promise<PublicKeyCredential | null>,
  );
  const outputs = created.getClientExtensionResults() as PrfOutputs;
  if (outputs.prf?.results?.first) return prfHolder(outputs.prf.results.first, personId, created.rawId);
  if (!outputs.prf?.enabled) throw new RecoveryError("passkey");
  // Most authenticators evaluate PRF only at sign-in: ask once more for this credential.
  return passkeyHolder(personId, created.rawId);
}

/** Asks for the person's passkey (or `credentialId`'s) and derives its holder. */
export async function passkeyHolder(personId: string, credentialId?: ArrayBuffer): Promise<Holder> {
  const first = await passkeyPrfInput();
  const asserted = await webauthn(
    () =>
      navigator.credentials.get({
        publicKey: {
          challenge: random(32),
          userVerification: "required",
          allowCredentials: credentialId ? [{ type: "public-key", id: credentialId }] : [],
          extensions: { prf: { eval: { first } } } as AuthenticationExtensionsClientInputs,
        },
      }) as Promise<PublicKeyCredential | null>,
  );
  return prfHolder((asserted.getClientExtensionResults() as PrfOutputs).prf?.results?.first, personId, asserted.rawId);
}

/** The recovery code's holder, from the code as typed. Throws `E2eError` `code` on a typo, before any network call. */
export async function recoveryCodeHolder(text: string, personId: string): Promise<Holder> {
  const code = parseRecoveryCode(text);
  const secret = await recoveryCodeSecret(code, personId);
  code.fill(0);
  try {
    return await importHolderSecret(secret);
  } finally {
    secret.fill(0);
  }
}

/** The person's trusted recovery key whose X25519 key is `holder`'s. */
export function findRecoveryKey(devices: E2eDevice[], holder: Pick<Holder, "encryptionKey">): E2eDevice | undefined {
  const key = b64u(holder.encryptionKey);
  return devices.find((device) => holderKind(device) !== "device" && device.state === "trusted" && device.encryptionKey === key);
}

/** Registers `holder` as a recovery key of `kind` and approves it from `selfId`, this trusted device. */
export async function addRecoveryKey(
  core: Pick<CoreClient, "registerE2eDevice" | "approveE2eDevice">,
  selfId: string,
  signer: PairingSigner,
  kind: RecoveryKind,
  holder: Holder,
  name: string,
): Promise<E2eDevice> {
  const registered = await core.registerE2eDevice({ name, kind, encryptionKey: b64u(holder.encryptionKey) });
  const signature = await signer.signHolderApproval(registered.id, kind, holder.encryptionKey);
  return core.approveE2eDevice(registered.id, { approverDeviceId: selfId, signature: b64u(signature) });
}

/** One content key the recovery key unlocked: its raw bytes, wiped once locked for the new device. */
export interface UnlockedKey {
  threadId: string;
  keyVersion: number;
  raw: Bytes;
}

/**
 * Every copy locked for `recoveryKey`, each checked and opened with `holder`. Its sender must chain to the recovery
 * key: the key's own approvals up to the root, then the devices those approved (`device-trust.ts`, ADR I2). A device
 * Gateway lists with no approver does not. Copies that do not verify or open are counted in `skipped`, never used.
 * Changes nothing at Gateway.
 */
export async function unlockHistory(
  core: Pick<CoreClient, "deviceThreadKeys">,
  devices: E2eDevice[],
  recoveryKey: E2eDevice,
  holder: Holder,
  senderTrust?: DeviceTrust,
): Promise<{ keys: UnlockedKey[]; skipped: number }> {
  let trust = senderTrust;
  if (!trust) {
    // A recovery key anchors its own history when there is no verified device trust.
    try {
      ({ trust } = await verifyDeviceTrust(devices, { id: recoveryKey.id, encryptionKey: b64u(holder.encryptionKey), signingKey: null }, { revoked: [], verified: {} }));
    } catch (error) {
      if (!(error instanceof DeviceTrustError)) throw error;
    }
  }
  const byId = new Map(devices.map((device) => [device.id, device]));
  const keys: UnlockedKey[] = [];
  let skipped = 0;
  let after: string | undefined;
  do {
    const page = await core.deviceThreadKeys(recoveryKey.id, { after, limit: 500 });
    for (const copy of page.keys) {
      const raw = await openCopy(copy, recoveryKey.id, holder, byId, trust);
      if (raw) keys.push({ threadId: copy.threadId, keyVersion: copy.keyVersion, raw });
      else skipped += 1;
    }
    after = page.nextAfter ?? undefined;
  } while (after);
  if (keys.length === 0 && skipped > 0) throw new RecoveryError("empty");
  return { keys, skipped };
}

async function openCopy(
  copy: ThreadKey,
  holderId: string,
  holder: Holder,
  byId: Map<string, E2eDevice>,
  trust: DeviceTrust | undefined,
): Promise<Bytes | undefined> {
  const epoch = epochOf(copy.keyVersion);
  const sender = byId.get(copy.senderDeviceId);
  const signingKey = sender?.signingKey ? fromB64u(sender.signingKey) : undefined;
  const signature = fromB64u(copy.signature);
  const locked = fromB64u(copy.lockedKey);
  if (epoch === undefined || copy.deviceId !== holderId || !signingKey || !signature || !locked) return undefined;
  if (!trust || !sender || !signerTrusted(trust, sender, copy.createdAt)) return undefined;
  try {
    if (!(await verify(signingKey, threadKeyMessage(copy.threadId, epoch, holderId, locked), signature))) return undefined;
    return await unlockContentKeyBytes(holder, copy.threadId, epoch, holderId, locked);
  } catch {
    return undefined;
  }
}

/** A recovery key this device unlocked with, and its holder: approved again after the way back. */
export interface UsedRecoveryKey {
  device: E2eDevice;
  holder: Holder;
}

/**
 * The way back (ADR I4), after {@link unlockHistory}: opens the challenge Gateway sealed to the first recovery key in
 * `used`, which proves it, and with that proof registers this device as the new root (Gateway revokes the person's other
 * devices). Then approves each recovery key it unlocked with again, and locks each unlocked content key for itself,
 * signed. A thread deleted meanwhile is skipped. Wipes every raw key. Returns this device and the copies stored.
 */
export async function recoverWith(
  core: Pick<CoreClient, "recoverE2eChallenge" | "recoverE2eDevice" | "approveE2eDevice" | "putThreadKeys">,
  signer: PairingSigner,
  personId: string,
  name: string,
  used: UsedRecoveryKey[],
  keys: UnlockedKey[],
): Promise<{ device: E2eDevice; stored: number }> {
  try {
    const prover = used[0];
    if (!prover) throw new RecoveryError("unknown");
    const sealed = (await core.recoverE2eChallenge()).challenges.find((entry) => entry.holderId === prover.device.id);
    const opened = sealed && fromB64u(sealed.sealed);
    if (!opened) throw new RecoveryError("unknown");
    const challenge = await openRecoveryChallenge(prover.holder, personId, prover.device.id, opened);
    const device = await core.recoverE2eDevice({
      name,
      encryptionKey: b64u(signer.encryptionKey),
      signingKey: b64u(signer.signingKey),
      holderId: prover.device.id,
      challenge: b64u(challenge),
    });
    challenge.fill(0);
    for (const { device: recoveryKey, holder } of used) {
      const kind = holderKind(recoveryKey);
      if (kind === "device") continue;
      const signature = await signer.signHolderApproval(recoveryKey.id, kind, holder.encryptionKey);
      await core.approveE2eDevice(recoveryKey.id, { approverDeviceId: device.id, signature: b64u(signature) });
    }
    let stored = 0;
    for (const key of keys) {
      const epoch = key.keyVersion - 1;
      const locked = await lockContentKey(key.raw, key.threadId, epoch, device.id, signer.encryptionKey);
      const signature = await signer.signThreadKey(key.threadId, epoch, device.id, locked);
      try {
        await core.putThreadKeys(key.threadId, {
          keyVersion: key.keyVersion,
          senderDeviceId: device.id,
          keys: [{ deviceId: device.id, lockedKey: b64u(locked), signature: b64u(signature) }],
        });
        stored += 1;
      } catch (error) {
        // Deleted since (410), or its copies revoked (409): nothing to read there any more.
        if (!(error instanceof CoreError) || (error.status !== 410 && error.status !== 409)) throw error;
      }
    }
    return { device, stored };
  } finally {
    for (const key of keys) key.raw.fill(0);
  }
}
