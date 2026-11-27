/**
 * Which of the person's listed holders this device trusts (S2 and S3 of the sync threat model; the key
 * directory ADR's I2 and I4). Gateway serves the list, and the list is not trusted. Nexus UI and the phone use this
 * check. It is the runner's check (`nexus-workbench/src/session_sync/trust.rs`, `verify` and `grow`):
 *
 * - The start: this device, found by its own keys, and the holders it verified before that Gateway still lists as
 *   trusted with the same keys.
 * - The first check trusts only this device. Its approver needs a code confirmed here. Ancestors come only from
 *   that approver's signed handover.
 * - Trust grows only through trusted, unrevoked approvers, from the approver to the device it approved. A device with
 *   no approver (a second root) is not trusted. A recovery key approves nothing.
 * - Every revoked id is remembered. A holder that Gateway lists again as trusted stays refused.
 * - The approver's handover (`handover.ts`): the holders it trusts and the revoked ids it remembers. It counts
 *   only when this device's approver signed it, and only while this device trusts that approver: the one the person
 *   confirmed here, with the same keys.
 *
 * The memory holds ids and public keys only. {@link deviceTrustStore} keeps it: IndexedDB in the browser, the app's
 * store on the phone ({@link setDeviceTrustStore}).
 *
 * When the list's `trust` is `gateway`, Gateway vouches for every device the person signs in on, and the
 * check above does not run: see {@link gatewayDeviceTrust}.
 */
import type { E2eDevice } from "./api";
import { b64u, fromB64u, type Holder, verify } from "./e2e";
import { indexedDbDeviceTrust, rememberDeviceTrust } from "./e2e-store";
import { type Handover, type ReceivedHandover, receivedHandover } from "./handover";
import { approvalOf, holderKind, matchesInput, readPairingInput } from "./pairing";

export { rememberDeviceTrust } from "./e2e-store";

/** A holder's public keys, base64url. A recovery key has no signing key. */
export interface HolderKeys {
  encryptionKey: string;
  signingKey: string | null;
}

/** What a device remembers between checks: every revoked id it saw, and every holder it trusted, with its keys. */
export interface DeviceTrustMemory {
  revoked: string[];
  verified: Record<string, HolderKeys>;
}

/** Keeps one device's memory. `remember` adds `seen` atomically and returns the whole memory. It never removes. */
export interface DeviceTrustStore {
  remember(scope: string, seen: DeviceTrustMemory): Promise<DeviceTrustMemory>;
}

/** `not-trusted`: Gateway does not list this device as trusted with its keys. `chain-broken`: its approvals do not verify. */
export class DeviceTrustError extends Error {
  constructor(readonly kind: "not-trusted" | "chain-broken") {
    super(TRUST_MESSAGES[kind]);
    this.name = "DeviceTrustError";
  }
}

const TRUST_MESSAGES: Record<DeviceTrustError["kind"], string> = {
  "not-trusted": "This device is not one of your trusted devices. Pair it from a trusted device.",
  "chain-broken": "This device's approval does not verify. Do not trust it until you pair it again.",
};

/**
 * The result of a check. `trusted`: the holders that are trusted now, this device included. `verified`: the holders
 * whose approval verifies from those, revoked ones included. A revoked holder's copies made before its revocation
 * stay readable; that time comes from Gateway (`synced-sessions.ts` says why).
 */
export interface DeviceTrust {
  me: E2eDevice;
  trusted: Set<string>;
  verified: Set<string>;
  /** Every revoked id this device knows: listed, remembered, or handed over by its approver. */
  revoked: Set<string>;
}

/**
 * How a client trusts the devices list (`GET /v1/devices`, its top-level `trust`). `approvals`: each device is trusted
 * through a chain of signed approvals that this device checks. `gateway`: Gateway vouches for every device the person
 * signs in on.
 */
export type DeviceTrustMode = "approvals" | "gateway";

/** The mode of a devices list (`DeviceList`). An absent `trust`, or any other value, is `approvals`. */
export function deviceTrustMode(list: object | null | undefined): DeviceTrustMode {
  return !!list && "trust" in list && list.trust === "gateway" ? "gateway" : "approvals";
}

let configured: DeviceTrustStore | undefined;
let browser: DeviceTrustStore | undefined;

/** Sets the store for a platform with no IndexedDB: the phone calls it once at start. */
export function setDeviceTrustStore(store: DeviceTrustStore): void {
  configured = store;
}

/**
 * The store {@link trustedDevices} uses by default: the one set with {@link setDeviceTrustStore}, else IndexedDB. With
 * neither (Node, tests), nothing is remembered: each check is a first check.
 */
export function deviceTrustStore(): DeviceTrustStore {
  if (configured) return configured;
  if (globalThis.indexedDB) return (browser ??= indexedDbDeviceTrust());
  return NO_MEMORY;
}

const NO_MEMORY: DeviceTrustStore = { remember: async (_scope, seen) => rememberDeviceTrust(undefined, seen) };

/** A store in memory, for tests and for a process that keeps its memory only while it runs. */
export function memoryDeviceTrust(): DeviceTrustStore {
  const held = new Map<string, DeviceTrustMemory>();
  return {
    async remember(scope, seen) {
      const next = rememberDeviceTrust(held.get(scope), seen);
      held.set(scope, next);
      return next;
    },
  };
}

/**
 * The holders this device trusts, with its memory in `store`. `self` is this device: its id and its own public keys.
 * With `self.holder`, its X25519 private key, it also reads the handover its approver sealed to it. `self.trust` is the
 * list's mode ({@link deviceTrustMode}); absent is `approvals`.
 * Rejects with {@link DeviceTrustError}, and with the store's error: a caller refuses on either.
 */
type Self = { deviceId: string; encryptionKey: Uint8Array; signingKey: Uint8Array; holder?: Holder; trust?: DeviceTrustMode };

function trustScope(self: Self): string {
  return JSON.stringify(["handover-v2", self.deviceId, b64u(self.signingKey)]);
}

export async function trustedDevices(
  devices: E2eDevice[],
  self: Self,
  store: DeviceTrustStore = deviceTrustStore(),
): Promise<DeviceTrust> {
  const me = { id: self.deviceId, encryptionKey: b64u(self.encryptionKey), signingKey: b64u(self.signingKey) };
  const scope = trustScope(self);
  const legacy = await store.remember(JSON.stringify([me.id, me.signingKey]), { revoked: [], verified: {} });
  const previous = await store.remember(JSON.stringify(["confirmed-approver-v2", me.id, me.signingKey]), { revoked: [], verified: {} });
  const memory = await store.remember(scope, { revoked: [...legacy.revoked, ...previous.revoked, ...devices.filter((device) => device.state === "revoked").map((device) => device.id)], verified: {} });
  // Only the revoked ids are remembered: a holder Gateway vouched for must not count as verified if the mode goes back.
  if (self.trust === "gateway") return gatewayDeviceTrust(devices, me, memory.revoked);
  const listed = devices.find((device) => device.id === me.id);
  const handover = self.holder && listed ? await receivedHandover(devices, listed, self.holder) : undefined;
  const { trust, verified } = await verifyDeviceTrust(devices, me, memory, handover);
  await store.remember(scope, { revoked: [...trust.revoked], verified });
  return trust;
}

/** What this device hands a device it approves: the holders it trusts now, and every revoked id it knows. */
export function handoverOf(devices: E2eDevice[], trust: DeviceTrust): Handover | undefined {
  if (!trust.me.signingKey) return undefined;
  if (trust.me.approvedBy && !trust.trusted.has(trust.me.approvedBy)) return undefined;
  const holders = devices.flatMap((device) => {
    const keys = trust.trusted.has(device.id) ? keysOf(device) : undefined;
    return keys ? [{ id: device.id, ...keys }] : [];
  });
  return { holders, revoked: [...trust.revoked].sort() };
}

/**
 * The check itself, with no store: `me` is the anchor, a listed holder with these keys (this device, or the recovery
 * key on the way back). `handover` is what this device's approver handed it, opened and checked against that
 * approver's listed key ({@link receivedHandover}). Returns the trust and the holders to remember as verified.
 */
export async function verifyDeviceTrust(
  devices: E2eDevice[],
  me: { id: string } & HolderKeys,
  memory: DeviceTrustMemory,
  handover?: ReceivedHandover,
): Promise<{ trust: DeviceTrust; verified: Record<string, HolderKeys> }> {
  const byId = new Map(devices.map((device) => [device.id, device]));
  if (byId.size !== devices.length) throw new DeviceTrustError("chain-broken");
  const revoked = new Set([...memory.revoked, ...devices.filter((device) => device.state === "revoked").map((device) => device.id)]);
  const usable = (device: E2eDevice) => device.state === "trusted" && !revoked.has(device.id);
  const self = byId.get(me.id);
  const selfKeys = self && keysOf(self);
  if (!self || !selfKeys || !sameKeys(selfKeys, me) || !usable(self)) throw new DeviceTrustError("not-trusted");
  const checks = new Map<string, Promise<boolean>>();
  const signed = (device: E2eDevice, approver: HolderKeys) => {
    let check = checks.get(device.id);
    if (!check) checks.set(device.id, (check = approvedBy(device, approver)));
    return check;
  };

  // Recovery is read-only: its secret opens old copies; it never hands trust to a runner.
  const recovery = me.signingKey === null ? await recoveryChain(byId, self, signed) : [];
  const remembered = Object.entries(memory.verified).filter(([id, keys]) => {
    const device = byId.get(id);
    const listed = device && keysOf(device);
    return !!listed && sameKeys(listed, keys);
  });
  const approverKeys = self.approvedBy ? memory.verified[self.approvedBy] : undefined;
  const handed = handover && byId.get(handover.approverId)?.state === "trusted" && !revoked.has(handover.approverId) && handover.approverId === self.approvedBy && approverKeys && sameKeys(approverKeys, handover.approverKeys) ? handover : undefined;
  for (const id of handed?.revoked ?? []) revoked.add(id);
  const handedHolders = (handed?.holders ?? []).flatMap((holder) => {
    const device = byId.get(holder.id);
    const keys = device && keysOf(device);
    return keys && sameKeys(keys, holder) ? [[holder.id, keys] as const] : [];
  });

  const trusted = new Map<string, HolderKeys>([[self.id, selfKeys]]);
  for (const device of recovery) if (usable(device)) trusted.set(device.id, keysOf(device)!);
  for (const [id, keys] of remembered) if (usable(byId.get(id)!)) trusted.set(id, keys);
  for (const [id, keys] of handedHolders) if (usable(byId.get(id)!)) trusted.set(id, keys);
  await grow(devices, trusted, usable, signed);

  // Who was approved by one of these, revoked or not: the signers of copies made before a revocation.
  const verified = new Map(trusted);
  for (const device of recovery) verified.set(device.id, keysOf(device)!);
  for (const [id, keys] of remembered) verified.set(id, keys);
  for (const [id, keys] of handedHolders) verified.set(id, keys);
  await grow(devices, verified, (device) => device.state === "trusted" || (device.state === "revoked" && !!device.approvedAt), signed, byId);

  return {
    trust: { me: self, trusted: new Set(trusted.keys()), verified: new Set(verified.keys()), revoked },
    verified: Object.fromEntries(trusted),
  };
}

/**
 * The trust when Gateway vouches for the person's devices (`trust` `gateway`). No approval signature is
 * checked and no approver is confirmed.
 *
 * - `me` must be listed as a trusted device with these keys, and not revoked.
 * - `trusted`: every listed device (absent kind is a device) that Gateway lists as trusted, minus every revoked id this
 *   device saw. A recovery key is never a holder.
 * - `verified`: those, and the devices revoked since they were trusted: the signers of copies made before a revocation.
 *
 * `remembered` holds the revoked ids this device saw before.
 */
export function gatewayDeviceTrust(devices: E2eDevice[], me: { id: string } & HolderKeys, remembered: readonly string[] = []): DeviceTrust {
  const byId = new Map(devices.map((device) => [device.id, device]));
  if (byId.size !== devices.length) throw new DeviceTrustError("chain-broken");
  const revoked = new Set([...remembered, ...devices.filter((device) => device.state === "revoked").map((device) => device.id)]);
  const holder = (device: E2eDevice) => holderKind(device) === "device" && !!keysOf(device);
  const self = byId.get(me.id);
  const selfKeys = self && holder(self) ? keysOf(self) : undefined;
  if (!self || !selfKeys || !sameKeys(selfKeys, me) || self.state !== "trusted" || revoked.has(self.id)) throw new DeviceTrustError("not-trusted");
  const trusted = new Set(devices.filter((device) => holder(device) && device.state === "trusted" && !revoked.has(device.id)).map((device) => device.id));
  const verified = new Set(trusted);
  for (const device of devices) if (holder(device) && device.state === "revoked" && device.approvedAt) verified.add(device.id);
  return { me: self, trusted, verified, revoked };
}

/**
 * Whether `sender` may have signed a key copy made at `createdAt`: a holder trusted now, or one revoked since whose
 * approval this device verified, for a copy older than its revocation. Both times come from Gateway, and the copy's
 * signature does not cover `createdAt`: Gateway can backdate a copy past this check.
 */
export function signerTrusted(trust: DeviceTrust, sender: E2eDevice, createdAt: string): boolean {
  if (trust.trusted.has(sender.id)) return true;
  return sender.state === "revoked" && trust.verified.has(sender.id) && !!sender.revokedAt && Date.parse(createdAt) < Date.parse(sender.revokedAt);
}

async function recoveryChain(
  byId: Map<string, E2eDevice>,
  self: E2eDevice,
  signed: (device: E2eDevice, approver: HolderKeys) => Promise<boolean> = approvedBy,
): Promise<E2eDevice[]> {
  const chain: E2eDevice[] = [];
  const seen = new Set([self.id]);
  for (let current = self; current.approvedBy; ) {
    const approver = byId.get(current.approvedBy);
    const keys = approver && keysOf(approver);
    if (!approver || !keys?.signingKey || seen.has(approver.id) || !(await signed(current, keys))) throw new DeviceTrustError("chain-broken");
    seen.add(approver.id);
    chain.push(approver);
    current = approver;
  }
  return chain;
}

/** Confirms only this device's direct approver, using the code read on that approver's own screen. */
export async function confirmDeviceApprover(
  devices: E2eDevice[],
  self: Self,
  code: string,
  store: DeviceTrustStore = deviceTrustStore(),
): Promise<DeviceTrust> {
  const trust = await trustedDevices(devices, self, store);
  const approver = devices.find((device) => device.id === trust.me.approvedBy);
  const keys = approver && keysOf(approver);
  if (!approver || !keys?.signingKey || approver.state !== "trusted" || trust.revoked.has(approver.id)
      || !(await matchesInput(approver, readPairingInput(code))) || !(await approvedBy(trust.me, keys))) {
    throw new DeviceTrustError("chain-broken");
  }
  const saved = await store.remember(trustScope(self), { revoked: [], verified: { [approver.id]: keys } });
  if (!sameKeys(saved.verified[approver.id]!, keys)) throw new DeviceTrustError("chain-broken");
  return trustedDevices(devices, self, store);
}

/**
 * Adds to `set` each listed holder that `admit` takes and whose approver in `set` signed it, until none is added. With
 * `byId`, a holder approved after its approver's revocation is refused.
 */
async function grow(
  devices: E2eDevice[],
  set: Map<string, HolderKeys>,
  admit: (device: E2eDevice) => boolean,
  signed: (device: E2eDevice, approver: HolderKeys) => Promise<boolean>,
  byId?: Map<string, E2eDevice>,
): Promise<void> {
  for (let grew = true; grew; ) {
    grew = false;
    for (const device of devices) {
      const keys = keysOf(device);
      const approver = device.approvedBy ? set.get(device.approvedBy) : undefined;
      if (set.has(device.id) || !admit(device) || !keys || !approver?.signingKey) continue;
      const revokedAt = byId?.get(device.approvedBy!)?.revokedAt;
      if (revokedAt && (!device.approvedAt || Date.parse(device.approvedAt) >= Date.parse(revokedAt))) continue;
      if (await signed(device, approver)) {
        set.set(device.id, keys);
        grew = true;
      }
    }
  }
}

/** Whether `approver`, a device, signed `device`'s id and keys as its kind's approval message spells them. */
async function approvedBy(device: E2eDevice, approver: HolderKeys): Promise<boolean> {
  const key = approver.signingKey ? fromB64u(approver.signingKey) : undefined;
  const signature = device.approvalSignature ? fromB64u(device.approvalSignature) : undefined;
  const message = approvalOf(device);
  return !!key && !!signature && !!message && verify(key, message, signature);
}

/** A holder's keys, base64url as `b64u` writes them, when they fit its kind: a device signs, a recovery key does not. */
function keysOf(device: E2eDevice): HolderKeys | undefined {
  const encryptionKey = fromB64u(device.encryptionKey);
  if (encryptionKey?.length !== 32) return undefined;
  if (holderKind(device) !== "device") return device.signingKey ? undefined : { encryptionKey: b64u(encryptionKey), signingKey: null };
  const signingKey = device.signingKey ? fromB64u(device.signingKey) : undefined;
  return signingKey?.length === 32 ? { encryptionKey: b64u(encryptionKey), signingKey: b64u(signingKey) } : undefined;
}

function sameKeys(a: HolderKeys, b: HolderKeys): boolean {
  return a.encryptionKey === b.encryptionKey && a.signingKey === b.signingKey;
}
