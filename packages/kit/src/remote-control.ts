import type { E2eDevice } from "./api";
import { CLOCK_SKEW_MS, controlNow, recordClockOffset } from "./code-control-clock";
import type { CodeAction, CodeCommandCancel, CodeCommandEnvelope, CodeControl, CodeControlLease } from "./code-control-types";
import { b64u, type DeviceKey, fromB64u, lockContentKey, sealEvent, sign } from "./e2e";
import { DeviceTrustError, deviceTrustStore, type DeviceTrustMode, type DeviceTrustStore, trustedDevices } from "./device-trust";
import { holderKind, thisDevice } from "./pairing";
import type { SessionEvent } from "./session-share";

export const CODE_COMMAND_TTL_MS = 60_000;
/** How long a message may wait for a computer that is asleep. */
export const CODE_COMMAND_WAIT_MS = 24 * 60 * 60 * 1000;
export const MAX_CODE_COMMAND_BYTES = 64 * 1024;
export { CLOCK_SKEW_MS, controlNow, recordClockOffset };

export function codeControlMessage(threadId: string, lease: Omit<CodeControlLease, "signature">): string {
  return ["nexus-code-control-v1", threadId, lease.runnerDeviceId, String(lease.enabled), String(lease.expiresAt)].join("\n");
}

/** v1 for a computer that is online; v2 binds the choice to wait for a sleeping one. */
export function codeCommandMessage(threadId: string, command: Omit<CodeCommandEnvelope, "signature">): string {
  const routing = [threadId, command.id, command.senderDeviceId, command.runnerDeviceId, String(command.expiresAt)];
  if (command.waits) return ["nexus-code-command-v2", ...routing, "true", command.lockedKey, command.sealed].join("\n");
  return ["nexus-code-command-v1", ...routing, command.lockedKey, command.sealed].join("\n");
}

export function codeCommandCancelMessage(threadId: string, id: string, senderDeviceId: string): string {
  return ["nexus-code-command-cancel-v1", threadId, id, senderDeviceId].join("\n");
}

/** The signed body that takes back a command this phone sent, before a computer claims it. */
export async function signCodeCommandCancel(threadId: string, id: string, reader: { deviceId: string; holder: DeviceKey }): Promise<CodeCommandCancel> {
  return { senderDeviceId: reader.deviceId, signature: b64u(await sign(reader.holder, codeCommandCancelMessage(threadId, id, reader.deviceId))) };
}

export function codeCommandClaimMessage(threadId: string, id: string, runnerDeviceId: string): string {
  return ["nexus-code-command-claim-v1", threadId, id, runnerDeviceId].join("\n");
}

export function codeCommandResultMessage(threadId: string, id: string, runnerDeviceId: string, state: "completed" | "rejected"): string {
  return ["nexus-code-command-result-v1", threadId, id, runnerDeviceId, state].join("\n");
}

export function controlAvailability(control: CodeControl | undefined, now = controlNow(control)): "loading" | "disabled" | "offline" | "ready" {
  if (!control) return "loading";
  if (!control.runnerDeviceId) return "disabled";
  if (control.expiresAt === null || !Number.isSafeInteger(control.expiresAt) || control.expiresAt <= now) return "offline";
  return control.enabled ? "ready" : "disabled";
}

/**
 * A message may wait for this computer. Its lease lapsed while Phone control was on: the computer sleeps or
 * is offline. A computer with Phone control off is not asleep, so nothing waits for it.
 */
export function controlWaitable(control: CodeControl | undefined, now = controlNow(control)): boolean {
  return !!control?.runnerDeviceId && control.enabled && controlAvailability(control, now) === "offline";
}

/** This device as it reads the devices list: its id, its key, and the list's mode (`deviceTrustMode`). */
export type DeviceReader = { deviceId: string; holder: DeviceKey; trust?: DeviceTrustMode };

/**
 * The runner, when this phone trusts it (`device-trust.ts`): a chain of approvals from this phone or from a device it
 * verified before, followed from approver to approved only. A root Gateway made up, or a device it approved, is refused.
 * With `reader.trust` `gateway`, a runner Gateway lists as a trusted device, and this phone has not seen revoked, is it.
 */
export async function trustedRunner(
  devices: E2eDevice[],
  reader: DeviceReader,
  runnerDeviceId: string,
  store: DeviceTrustStore = deviceTrustStore(),
): Promise<E2eDevice> {
  const self = thisDevice(devices, reader.holder);
  if (!self || self.id !== reader.deviceId || self.state !== "trusted" || holderKind(self) !== "device") throw new Error("Pair this phone with a trusted computer before controlling sessions.");
  let trusted: Set<string>;
  try {
    ({ trusted } = await trustedDevices(devices, { deviceId: reader.deviceId, encryptionKey: reader.holder.encryptionKey, signingKey: reader.holder.signingKey, holder: reader.holder, ...(reader.trust ? { trust: reader.trust } : {}) }, store));
  } catch (error) {
    if (error instanceof DeviceTrustError) throw new Error("This phone's approval does not verify. Pair it again with a new device key.");
    throw error;
  }
  const runner = devices.find((device) => device.id === runnerDeviceId);
  if (!runner || runner.state !== "trusted" || holderKind(runner) !== "device" || !runner.signingKey || !trusted.has(runner.id)) throw new Error("This computer is not one of this phone's trusted devices. Pair them again.");
  return runner;
}

/** A UUID from the same cryptographic random source on Hermes and in a browser. */
function commandId(): string {
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6]! & 15) | 64;
  bytes[8] = (bytes[8]! & 63) | 128;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * Encrypts for the runner alone, then signs the URL's thread, recipient, expiry and exact ciphertext. The expiry is
 * by Gateway's clock (`controlNow`) when `control` came from `CoreClient.codeControl`.
 *
 * `waits`: a message that may wait up to 24 hours for a computer that is asleep (`controlWaitable`). Its body
 * is version 2, with `sentAt`, and its signature is v2. Stop and approval answers never wait: a late one is wrong.
 */
export async function sealCodeCommand({ threadId, action, reader, devices, control, now = controlNow(control), waits = false }: {
  threadId: string;
  action: CodeAction;
  reader: DeviceReader;
  devices: E2eDevice[];
  control: CodeControl;
  now?: number;
  waits?: boolean;
}): Promise<CodeCommandEnvelope> {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(threadId)) throw new Error("This session's ID is invalid.");
  if (waits && action.kind !== "send") throw new Error("Only a message can wait for the computer.");
  const usable = controlAvailability(control, now) === "ready" || waits && controlWaitable(control, now);
  if (!usable || !control.runnerDeviceId) throw new Error("This computer is offline or phone control is turned off.");
  if (action.kind === "send" && !action.text.trim()) throw new Error("Write a message first.");
  if (action.kind === "approve" && !action.requestId.trim()) throw new Error("This approval is no longer available.");
  const plaintext = new TextEncoder().encode(JSON.stringify(waits ? { version: 2, threadId, sentAt: now, action } : { version: 1, threadId, action }));
  if (plaintext.length > MAX_CODE_COMMAND_BYTES) throw new Error("This message is too long. Shorten it and try again.");
  const runner = await trustedRunner(devices, reader, control.runnerDeviceId);
  const publicKey = fromB64u(runner.encryptionKey);
  if (publicKey?.length !== 32) throw new Error("This computer's encryption key is invalid.");
  const id = commandId();
  const raw = globalThis.crypto.getRandomValues(new Uint8Array(32));
  try {
    const [lockedKey, sealed] = await Promise.all([
      lockContentKey(raw, id, 0, runner.id, publicKey),
      sealEvent(raw, 0, id, 0, plaintext),
    ]);
    const expiresAt = now + (waits ? CODE_COMMAND_WAIT_MS : CODE_COMMAND_TTL_MS);
    const envelope = { id, senderDeviceId: reader.deviceId, runnerDeviceId: runner.id, expiresAt, lockedKey: b64u(lockedKey), sealed: b64u(sealed), ...(waits ? { waits: true } : {}) };
    return { ...envelope, signature: b64u(await sign(reader.holder, codeCommandMessage(threadId, envelope))) };
  } finally {
    raw.fill(0);
    plaintext.fill(0);
  }
}

/** Only unresolved requests from the current turn can be answered. Older pages never revive ended approvals. */
export function pendingCodeApprovals(events: readonly SessionEvent[]): Extract<SessionEvent, { kind: "approval" }>[] {
  const pending = new Map<string, Extract<SessionEvent, { kind: "approval" }>>();
  for (const event of events) {
    if (event.kind === "approval") pending.set(event.requestId, event);
    else if (event.kind === "approvalResolved") pending.delete(event.requestId);
    else if (event.kind === "turnEnd" || event.kind === "turnStart") pending.clear();
  }
  return [...pending.values()];
}
