/**
 * The approver's trusted list for the device it approves (S1 of the sync threat model; the key directory
 * ADR's I2). When a trusted device approves a new one, it hands over the holders it trusts and the revoked ids it
 * remembers. It signs them and seals them to the new device (`e2e.ts`, {@link sealHandover}). Gateway stores the bytes
 * on the new device's row and cannot open them. The new device opens them and checks the signature against its own
 * approver. Without the list, a new device trusts only its approver and the devices approved below it.
 *
 * The payload is UTF-8 JSON: `{"version":2,"holders":[{"id","encryptionKey","signingKey"}],"revoked":[id]}`, keys in
 * base64url. Version 2 requires corrected local trust; legacy handovers are ignored during mixed-version pairing.
 * A recovery key's `signingKey` is null. nexus-workbench's `session_sync/trust.rs` reads the same payload.
 */
import type { E2eDevice } from "./api";
import { b64u, type Bytes, fromB64u, handoverMessage, type Holder, MAX_HANDOVER_PAYLOAD, openHandover, sealHandover, verify } from "./e2e";

/** A holder in a handover: its id and public keys, base64url. A recovery key has no signing key. */
export interface HandoverHolder {
  id: string;
  encryptionKey: string;
  signingKey: string | null;
}

/** What an approver hands over: the holders it trusts, and every revoked id it remembers. */
export interface Handover {
  holders: HandoverHolder[];
  revoked: string[];
}

/** A handover this device opened and whose signature verified with `approver`'s listed signing key. */
export interface ReceivedHandover extends Handover {
  approverId: string;
  approverKeys: { encryptionKey: string; signingKey: string };
}

/** Signs a handover as the approver: the `PairingSigner`'s optional `signHandover`. */
export interface HandoverSigner {
  signHandover(deviceId: string, approverId: string, payload: Uint8Array): Promise<Bytes>;
}

const HOLDER_ID = /^[A-Za-z0-9_-]{1,128}$/;

/** The payload's bytes, as the approver signs and seals them. */
export function handoverPayload(handover: Handover): Bytes {
  const json = JSON.stringify({ version: 2, holders: handover.holders.map(({ id, encryptionKey, signingKey }) => ({ id, encryptionKey, signingKey })), revoked: handover.revoked });
  return new TextEncoder().encode(json) as Bytes;
}

/** The payload read back, or `undefined` when it is not one: a wrong version, a malformed id or key, or bad JSON. */
export function readHandoverPayload(payload: Uint8Array): Handover | undefined {
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payload));
  } catch {
    return undefined;
  }
  if (!value || typeof value !== "object") return undefined;
  const { version, holders, revoked } = value as Record<string, unknown>;
  if (version !== 2 || !Array.isArray(holders) || !Array.isArray(revoked)) return undefined;
  const read: HandoverHolder[] = [];
  for (const holder of holders) {
    if (!holder || typeof holder !== "object") return undefined;
    const { id, encryptionKey, signingKey } = holder as Record<string, unknown>;
    if (typeof id !== "string" || !HOLDER_ID.test(id) || !isKey(encryptionKey) || (signingKey !== null && !isKey(signingKey))) return undefined;
    read.push({ id, encryptionKey, signingKey });
  }
  if (!revoked.every((id) => typeof id === "string" && HOLDER_ID.test(id))) return undefined;
  return { holders: read, revoked: revoked as string[] };
}

/** `device`'s handover from `approverId`, signed by `signer` and sealed to the device: base64url for `approveDevice`. */
export async function sealHandoverFor(signer: HandoverSigner, approverId: string, device: E2eDevice, handover: Handover): Promise<string | undefined> {
  const deviceKey = fromB64u(device.encryptionKey);
  const payload = handoverPayload(handover);
  // A list too long to seal is left out: the new device then trusts only its approver and the devices below it.
  if (deviceKey?.length !== 32 || payload.length > MAX_HANDOVER_PAYLOAD) return undefined;
  const signature = await signer.signHandover(device.id, approverId, payload);
  return b64u(await sealHandover(device.id, approverId, deviceKey, signature, payload));
}

/**
 * The handover listed on `self`, opened with `holder` (this device's X25519 key), when `self`'s listed approver signed
 * it. `undefined` when there is none or it does not open or verify. The caller still checks that it trusts that
 * approver (`device-trust.ts`).
 */
export async function receivedHandover(devices: E2eDevice[], self: E2eDevice, holder: Holder): Promise<ReceivedHandover | undefined> {
  const approver = self.approvedBy ? devices.find((device) => device.id === self.approvedBy) : undefined;
  const sealed = self.handover ? fromB64u(self.handover) : undefined;
  const signingKey = approver?.signingKey ? fromB64u(approver.signingKey) : undefined;
  const encryptionKey = approver ? fromB64u(approver.encryptionKey) : undefined;
  if (!approver || !sealed || signingKey?.length !== 32 || encryptionKey?.length !== 32) return undefined;
  try {
    const opened = await openHandover(holder, self.id, approver.id, sealed);
    if (!(await verify(signingKey, handoverMessage(self.id, approver.id, opened.payload), opened.signature))) return undefined;
    const handover = readHandoverPayload(opened.payload);
    return handover && { ...handover, approverId: approver.id, approverKeys: { encryptionKey: b64u(encryptionKey), signingKey: b64u(signingKey) } };
  } catch {
    return undefined;
  }
}

function isKey(value: unknown): value is string {
  return typeof value === "string" && fromB64u(value)?.length === 32;
}
