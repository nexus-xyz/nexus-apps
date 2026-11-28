/**
 * Device pairing (eng-architecture.md §4.3, the key directory ADR's I2 to I4): a new device registers and
 * shows its id and fingerprint as a QR code and a 16-character code; a trusted device finds it among the person's pending
 * devices, checks the fingerprint itself, and signs the approval. Gateway is not trusted for that check.
 *
 * The private key never reaches this file. A {@link PairingSigner} signs: in the browser it is {@link webCryptoSigner}
 * over a non-exportable WebCrypto key; on desktop it is the Rust core, which keeps the key in the OS keychain and checks
 * the fingerprint again before it signs.
 */
import type { CoreClient } from "./core-client";
import type { DeviceThreadKey, E2eDevice, HolderKind } from "./api";
import type { DeviceTrustMode } from "./device-trust";
import {
  approvalMessage,
  b64u,
  type Bytes,
  type DeviceKey,
  fingerprint,
  type Holder,
  fromB64u,
  handoverMessage,
  holderApprovalMessage,
  revocationMessage,
  shortCode,
  sign,
  threadKeyMessage,
  verify,
} from "./e2e";
import { type Handover, sealHandoverFor } from "./handover";

/** How long a pending device can be approved after it registers: Gateway's `PAIRING_TTL_MS`. */
export const PAIRING_TTL_MS = 15 * 60 * 1000;
/** What a pairing QR code starts with; then the device id and its fingerprint, base64url. */
export const PAIRING_QR_PREFIX = "nexus-pair:v1:";
/** The fingerprint's first 10 bytes in base32 (ADR I3). */
export const SHORT_CODE_LENGTH = 16;

/** Where a device is in its life. `expired`: still pending at Gateway, but past {@link PAIRING_TTL_MS}. */
export type DeviceStatus = "trusted" | "pending" | "expired" | "revoked";

/** How a holder came to be trusted, as {@link verifyApprovals} labels it. */
export type ApprovalLabel = "root" | "approved" | "unverified" | "gateway";

/** This device's public half and the means to sign with its private key. */
export interface PairingSigner {
  readonly encryptionKey: Bytes;
  readonly signingKey: Bytes;
  /** Signs `device`'s approval, after checking its fingerprint begins with `code`. Refuses with {@link PairingError}. */
  signApproval(device: E2eDevice, code: string): Promise<Bytes>;
  signRevocation(deviceId: string): Promise<Bytes>;
  /** Signs a recovery key's approval: its id, kind and X25519 key, which this device derived itself. */
  signHolderApproval(holderId: string, kind: RecoveryKind, encryptionKey: Uint8Array): Promise<Bytes>;
  /** Signs a locked copy this device stores: `nexus-thread-key:v1`, for a recovery's history copied to this device. */
  signThreadKey(threadId: string, epoch: number, deviceId: string, locked: Uint8Array): Promise<Bytes>;
  /**
   * Signs the trusted list this device hands a device it approves: `nexus-device-handover:v1`. A signer
   * without it approves with no handover.
   */
  signHandover?(deviceId: string, approverId: string, payload: Uint8Array): Promise<Bytes>;
  /**
   * This device's X25519 private key, where the signer holds it in this process: the browser's. With it, the trust
   * check reads the handover this device's own approver sealed to it, so it hands on that list's revoked ids too.
   */
  readonly holder?: Holder;
}

/** A recovery key's kind. */
export type RecoveryKind = Exclude<HolderKind, "device">;

/** A holder's kind: `device` from a Gateway that predates recovery keys. */
export function holderKind(device: Pick<E2eDevice, "kind">): HolderKind {
  return device.kind ?? "device";
}

/** What the person typed or scanned: a short code, or a QR code's device id and whole fingerprint. */
export interface PairingInput {
  code: string;
  deviceId?: string;
  fingerprint?: Bytes;
}

/**
 * Why a pairing stopped. `code`: the text is no code. `none`: no pending device has it. `expired`: the device with it
 * waited past 15 minutes. `ambiguous`: two different devices have it. `mismatch`: the device's keys do not have it.
 */
export class PairingError extends Error {
  constructor(readonly kind: "code" | "none" | "expired" | "ambiguous" | "mismatch") {
    super(PAIRING_MESSAGES[kind]);
    this.name = "PairingError";
  }
}

const PAIRING_MESSAGES: Record<PairingError["kind"], string> = {
  code: "That is not a pairing code. It has 16 letters and digits, shown on the new device.",
  none: "No device is waiting with that code. Check the code on the new device.",
  expired: "That device waited more than 15 minutes. Start the pairing again on it.",
  ambiguous: "Two devices have that code. Revoke the one you do not know, then pair again.",
  mismatch: "That device's keys do not match the code. Do not approve it.",
};

/** A holder's fingerprint (ADR I3) and its short code, from the keys Gateway lists. A recovery key has no signing key. */
export async function deviceFingerprint(device: Pick<E2eDevice, "encryptionKey" | "signingKey">): Promise<{ fingerprint: Bytes; code: string }> {
  const encryptionKey = fromB64u(device.encryptionKey);
  const signingKey = device.signingKey === null ? undefined : fromB64u(device.signingKey);
  if (encryptionKey?.length !== 32 || (device.signingKey !== null && signingKey?.length !== 32)) throw new PairingError("mismatch");
  const bytes = await fingerprint(encryptionKey, signingKey);
  return { fingerprint: bytes, code: shortCode(bytes) };
}

/** The code as people read it: four groups of four. */
export function formatCode(code: string): string {
  return code.replace(/(.{4})(?=.)/g, "$1 ");
}

/** A short code as typed: any case, with or without spaces and dashes. `undefined` when it is not 16 base32 characters. */
export function normalizeCode(text: string): string | undefined {
  const code = text.toUpperCase().replace(/[\s-]/g, "");
  return /^[A-Z2-7]{16}$/.test(code) ? code : undefined;
}

/** What a new device's QR code holds (ADR I3): its id and its whole fingerprint. */
export function pairingQrText(deviceId: string, deviceFingerprint: Uint8Array): string {
  return `${PAIRING_QR_PREFIX}${deviceId}:${b64u(deviceFingerprint)}`;
}

/** Reads a typed code or a scanned QR code's text. */
export function readPairingInput(text: string): PairingInput {
  const trimmed = text.trim();
  if (trimmed.startsWith(PAIRING_QR_PREFIX)) {
    const [deviceId, encoded] = trimmed.slice(PAIRING_QR_PREFIX.length).split(":");
    const bytes = encoded ? fromB64u(encoded) : undefined;
    if (!deviceId || !/^[A-Za-z0-9_-]{1,128}$/.test(deviceId) || bytes?.length !== 32) throw new PairingError("code");
    return { code: shortCode(bytes), deviceId, fingerprint: bytes };
  }
  const code = normalizeCode(trimmed);
  if (!code) throw new PairingError("code");
  return { code };
}

/**
 * Where `device` is in its life. With `trust` `gateway`, nothing approves a pending device: Gateway registers a new
 * device as trusted at once, so a row left pending from before is `expired`, and a new registration replaces it.
 */
export function deviceStatus(device: E2eDevice, now = Date.now(), trust: DeviceTrustMode = "approvals"): DeviceStatus {
  if (device.state !== "pending") return device.state;
  if (trust === "gateway") return "expired";
  return now - Date.parse(device.createdAt) >= PAIRING_TTL_MS ? "expired" : "pending";
}

/** Whether `device`'s keys have this input's fingerprint: the whole one from a QR code, or the code's 10 bytes. */
export async function matchesInput(device: E2eDevice, input: PairingInput): Promise<boolean> {
  if (input.deviceId !== undefined && input.deviceId !== device.id) return false;
  const { fingerprint: bytes, code } = await deviceFingerprint(device);
  if (input.fingerprint) return sameBytes(bytes, input.fingerprint);
  return code === input.code;
}

/**
 * The pending device the person means, checked here and not by Gateway. Rows with the same keys are one device: the
 * newest is taken (Gateway made such rows before it kept one per key). Two devices with different keys and one code
 * are refused.
 */
export async function findPending(devices: E2eDevice[], input: PairingInput, now = Date.now()): Promise<E2eDevice> {
  const live: E2eDevice[] = [];
  let expired = false;
  for (const device of devices) {
    const status = deviceStatus(device, now);
    if (holderKind(device) !== "device" || (status !== "pending" && status !== "expired")) continue;
    if (!(await matchesInput(device, input))) continue;
    if (status === "expired") expired = true;
    else live.push(device);
  }
  if (live.length === 0) throw new PairingError(expired ? "expired" : "none");
  const keys = new Set(live.map((device) => `${device.encryptionKey}:${device.signingKey}`));
  if (keys.size > 1) throw new PairingError("ambiguous");
  return live.reduce((newest, device) => (Date.parse(device.createdAt) > Date.parse(newest.createdAt) ? device : newest));
}

/** This device's newest row in the directory: the one with its keys. `undefined` until it registers. */
export function thisDevice(devices: E2eDevice[], signer: Pick<PairingSigner, "encryptionKey" | "signingKey">): E2eDevice | undefined {
  const encryptionKey = b64u(signer.encryptionKey);
  const signingKey = b64u(signer.signingKey);
  return devices
    .filter((device) => device.encryptionKey === encryptionKey && device.signingKey === signingKey)
    .reduce<E2eDevice | undefined>((newest, device) => (!newest || Date.parse(device.createdAt) >= Date.parse(newest.createdAt) ? device : newest), undefined);
}

/**
 * Approves `device` from `approverId`, this trusted device: checks its fingerprint against `input` again, signs, and
 * sends the copies of the thread keys this device holds. `copies` locks them for the new device; it is empty until the
 * runner's sync holds content keys. `handover` is what this device trusts (`handoverOf` in
 * `device-trust.ts`): signed and sealed to the new device, so it trusts the same holders and revocations.
 */
export async function approvePending(
  core: Pick<CoreClient, "approveE2eDevice">,
  approverId: string,
  signer: PairingSigner,
  device: E2eDevice,
  input: PairingInput,
  copies: (device: E2eDevice) => Promise<DeviceThreadKey[]> = async () => [],
  handover?: Handover,
): Promise<E2eDevice> {
  if (!(await matchesInput(device, input))) throw new PairingError("mismatch");
  const signature = await signer.signApproval(device, input.code);
  const signHandover = signer.signHandover?.bind(signer);
  const sealed = handover && signHandover ? await sealHandoverFor({ signHandover }, approverId, device, handover) : undefined;
  return core.approveE2eDevice(device.id, {
    approverDeviceId: approverId,
    signature: b64u(signature),
    keys: await copies(device),
    ...(sealed ? { handover: sealed } : {}),
  });
}

/** Revokes `deviceId` with the signature of `revokerId`, this trusted device. Its copies go, and it gets no new key. */
export async function revokeWith(
  core: Pick<CoreClient, "revokeE2eDevice">,
  revokerId: string,
  signer: PairingSigner,
  deviceId: string,
): Promise<E2eDevice> {
  const signature = await signer.signRevocation(deviceId);
  return core.revokeE2eDevice(deviceId, { revokerDeviceId: revokerId, signature: b64u(signature) });
}

/**
 * How each device and recovery key came to be trusted, checked on this device (ADR I2, I4, I6). `root`: a device
 * trusted with no approval, as a person's first device. `approved`: its approver, a device, signed its keys (v1 for a
 * device, v2 for a recovery key). `unverified`: the signature does not verify, its approver is not listed or is a
 * recovery key, or a recovery key claims to be a root. Pending holders are not in the map.
 *
 * These are labels, not trust: each approval is checked against its listed approver only, and any device with no
 * approver is a `root`. Whom this device trusts comes from `trustedDevices` (`device-trust.ts`).
 *
 * With `trust` `gateway`, a device is labelled `gateway`: Gateway vouches for it, and its approval, if any,
 * is not checked. A recovery key is labelled as above.
 */
export async function verifyApprovals(devices: E2eDevice[], trust: DeviceTrustMode = "approvals"): Promise<Map<string, ApprovalLabel>> {
  const byId = new Map(devices.map((device) => [device.id, device]));
  const result = new Map<string, ApprovalLabel>();
  for (const device of devices) {
    if (device.state === "pending" || (device.state === "revoked" && !device.approvedAt)) continue;
    const kind = holderKind(device);
    if (trust === "gateway" && kind === "device") {
      result.set(device.id, "gateway");
      continue;
    }
    if (!device.approvedBy || !device.approvalSignature) {
      result.set(device.id, kind === "device" ? "root" : "unverified");
      continue;
    }
    const approver = byId.get(device.approvedBy);
    const approverKey = approver?.signingKey ? fromB64u(approver.signingKey) : undefined;
    const message = approvalOf(device);
    const signature = fromB64u(device.approvalSignature);
    const ok = !!approverKey && !!message && !!signature && (await verify(approverKey, message, signature));
    result.set(device.id, ok ? "approved" : "unverified");
  }
  return result;
}

/** What `device`'s approver signed, by its kind; `undefined` when its keys do not fit its kind. */
export function approvalOf(device: E2eDevice): string | undefined {
  const kind = holderKind(device);
  const encryptionKey = fromB64u(device.encryptionKey);
  if (encryptionKey?.length !== 32) return undefined;
  try {
    if (kind === "device") {
      const signingKey = device.signingKey ? fromB64u(device.signingKey) : undefined;
      return signingKey?.length === 32 ? approvalMessage(device.id, encryptionKey, signingKey) : undefined;
    }
    return device.signingKey === null ? holderApprovalMessage(device.id, kind, encryptionKey) : undefined;
  } catch {
    return undefined;
  }
}

/** The browser's signer: a non-exportable WebCrypto key (`e2e-store.ts` keeps it in IndexedDB). */
export function webCryptoSigner(key: DeviceKey): PairingSigner {
  return {
    encryptionKey: key.encryptionKey,
    signingKey: key.signingKey,
    holder: key,
    async signApproval(device, code) {
      const encryptionKey = fromB64u(device.encryptionKey);
      const signingKey = device.signingKey ? fromB64u(device.signingKey) : undefined;
      if (encryptionKey?.length !== 32 || signingKey?.length !== 32) throw new PairingError("mismatch");
      if (shortCode(await fingerprint(encryptionKey, signingKey)) !== code) throw new PairingError("mismatch");
      return sign(key, approvalMessage(device.id, encryptionKey, signingKey));
    },
    signRevocation: (deviceId) => sign(key, revocationMessage(deviceId)),
    signHolderApproval: (holderId, kind, encryptionKey) => sign(key, holderApprovalMessage(holderId, kind, encryptionKey)),
    signThreadKey: (threadId, epoch, deviceId, locked) => sign(key, threadKeyMessage(threadId, epoch, deviceId, locked)),
    signHandover: (deviceId, approverId, payload) => sign(key, handoverMessage(deviceId, approverId, payload)),
  };
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}
