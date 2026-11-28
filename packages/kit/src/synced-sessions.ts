/**
 * Reading a synced session on this device (eng-architecture.md §4.3): find this device's locked copy of a
 * thread's content key, check who signed it, unlock it with HPKE, and open each event with the key at its
 * `keyVersion` (epoch = keyVersion − 1). Gateway cannot read any of it.
 *
 * What a runner seals is nexus-workbench's `Plaintext` (`session_sync.rs`): a version, the event's time, the thread's
 * private metadata (local id, title, project name and path) and the raw stored event as JSON. Every event carries
 * the metadata, so a page read from the end still names its thread.
 */
import type { E2eDevice, SyncedEvent, ThreadKey } from "./api";
import type { ExecutionOrigin } from "./computer-types";
import { readExecutionOrigin } from "./computer-sessions";
import { CoreError, type CoreClient } from "./core-client";
import { deviceTrustStore, type DeviceTrust, type DeviceTrustMode, type DeviceTrustStore, signerTrusted, trustedDevices } from "./device-trust";
import { type ContentKey, type DeviceKey, epochOf, fromB64u, openEvent, teamThreadKeyMessage, threadKeyMessage, unlockContentKey, unlockTeamKey, verify } from "./e2e";
import { browserTeamTrust, trustedTeamHolders, type TeamTrustStore } from "./team-trust";

/** The thread's private metadata, as the runner seals it with every event. */
export interface PrivateThread {
  localId: string;
  title: string;
  projectName: string;
  projectPath: string;
  origin?: ExecutionOrigin;
}

/**
 * One synced event as this device reads it. `open`: its body, decrypted. `local`: the organization's content never
 * leaves the machine, so only the usage synced. `no-key`: this device holds no copy of the key it was sealed with.
 * `unreadable`: it did not open, or opened to something this viewer does not read.
 */
export type OpenedEvent =
  | { state: "open"; seq: number; at: number; thread: PrivateThread; event: { kind: string } & Record<string, unknown> }
  | { state: "local" | "no-key" | "unreadable"; seq: number; at: number };

/**
 * Why a key copy was not used. `none`: this device has no copy at that version (Settings › Devices pairs it).
 * `untrusted`: the copy's signer is not a trusted device of yours, or its signature does not verify. `open`: the copy
 * did not unlock with this device's key.
 */
export class SyncedKeyError extends Error {
  constructor(readonly kind: "none" | "untrusted" | "open") {
    super(KEY_MESSAGES[kind]);
    this.name = "SyncedKeyError";
  }
}

const KEY_MESSAGES: Record<SyncedKeyError["kind"], string> = {
  none: "This device holds no key for this session. Pair it from a device that does, in Settings › Devices.",
  untrusted: "The key for this session was not signed by one of your trusted devices, so it was not used.",
  open: "The key for this session did not unlock on this device.",
};

/**
 * This device as a reader: its id in the directory, and its keys. Its copies are locked to its X25519 key, and its
 * public keys find it in the directory, where its trust starts.
 */
export interface SyncedReader {
  deviceId: string;
  holder: DeviceKey;
  /** The devices list's mode (`deviceTrustMode`). With `gateway`, a signer Gateway lists as trusted is trusted. */
  trust?: DeviceTrustMode;
}

/**
 * A thread's content keys on this device, unlocked once per version and kept in memory only. Each copy is checked
 * before use: its signer must be a device this device trusts through a chain of approvals from its own
 * (`device-trust.ts`, ADR I2, I4). A device that Gateway lists with no approver is not trusted, and a revocation this
 * device saw once is never forgotten. A signer revoked since, whose approval this device verified, passes only for a
 * copy older than the revocation.
 *
 * Without a copy of its own, it tries `team`: a team-visible thread's copy locked to the team key (ADR 0011, M6).
 *
 * The revocation check trusts Gateway. It compares the copy's `createdAt` with the signer's `revokedAt`, and both
 * come from Gateway. The copy's signature does not cover `createdAt`, so Gateway can backdate a copy past the check.
 * The check stops an honest Gateway from serving a copy a revoked device made later. It is no guarantee against
 * Gateway itself.
 */
export class ThreadKeys {
  private readonly unlocked = new Map<number, Promise<ContentKey>>();
  private readonly trust: Promise<DeviceTrust | undefined>;

  constructor(
    readonly threadId: string,
    private readonly reader: SyncedReader,
    private readonly devices: E2eDevice[],
    private readonly fetchCopy: (keyVersion: number) => Promise<ThreadKey | undefined>,
    private readonly team?: (keyVersion: number) => Promise<ContentKey | undefined>,
    store: DeviceTrustStore = deviceTrustStore(),
  ) {
    const { deviceId, holder, trust } = reader;
    // A device that is not trusted, or a store that fails, trusts no signer.
    this.trust = trustedDevices(devices, { deviceId, encryptionKey: holder.encryptionKey, signingKey: holder.signingKey, holder, ...(trust ? { trust } : {}) }, store).catch(() => undefined);
  }

  /** The content key at `keyVersion`. Rejects with {@link SyncedKeyError}. */
  at(keyVersion: number): Promise<ContentKey> {
    let key = this.unlocked.get(keyVersion);
    if (!key) {
      key = this.unlock(keyVersion);
      // A refused copy is asked again next time: the person may pair the device meanwhile.
      key.catch(() => this.unlocked.delete(keyVersion));
      this.unlocked.set(keyVersion, key);
    }
    return key;
  }

  private async unlock(keyVersion: number): Promise<ContentKey> {
    const epoch = epochOf(keyVersion);
    if (epoch === undefined) throw new SyncedKeyError("open");
    const copy = await this.fetchCopy(keyVersion);
    if (!copy && this.team) {
      const key = await this.team(keyVersion);
      if (key) return key;
    }
    if (!copy || copy.keyVersion !== keyVersion || copy.threadId !== this.threadId || copy.deviceId !== this.reader.deviceId) {
      throw new SyncedKeyError("none");
    }
    if (!(await this.signedByTrusted(copy))) throw new SyncedKeyError("untrusted");
    const locked = fromB64u(copy.lockedKey);
    if (!locked) throw new SyncedKeyError("open");
    try {
      return await unlockContentKey(this.reader.holder, this.threadId, epoch, this.reader.deviceId, locked);
    } catch {
      throw new SyncedKeyError("open");
    }
  }

  private async signedByTrusted(copy: ThreadKey): Promise<boolean> {
    const sender = this.devices.find((device) => device.id === copy.senderDeviceId);
    const trust = await this.trust;
    if (!sender || !trust || !signerTrusted(trust, sender, copy.createdAt)) return false;
    const signingKey = sender.signingKey ? fromB64u(sender.signingKey) : undefined;
    const signature = fromB64u(copy.signature);
    const locked = fromB64u(copy.lockedKey);
    if (!signingKey || !signature || !locked) return false;
    try {
      return await verify(signingKey, threadKeyMessage(this.threadId, copy.keyVersion - 1, copy.deviceId, locked), signature);
    } catch {
      return false;
    }
  }
}

/** Fetches this device's copy at a version from Gateway; `undefined` when it holds none. */
export function gatewayKeyCopies(core: Pick<CoreClient, "threadKey">, threadId: string, deviceId: string): (keyVersion: number) => Promise<ThreadKey | undefined> {
  return async (keyVersion) => {
    try {
      return await core.threadKey(threadId, deviceId, keyVersion);
    } catch (error) {
      if (error instanceof CoreError && error.status === 404) return undefined;
      throw error;
    }
  };
}

/**
 * A teammate's thread key through the team key (ADR 0011, M6): the thread's team copy at a version, signed
 * by a trusted holder of the thread's person, then this device's copy of that team-key epoch. `undefined` when either
 * is missing, does not verify or does not open.
 *
 * The thread signer must chain to the teammate's pinned root. Changed roots and remembered revocations fail closed.
 * Storage failures also refuse the copy. First use trusts the first root; Gateway can hide an unseen revocation.
 */
export function gatewayTeamKeys(
  core: Pick<CoreClient, "whoami" | "teamThreadKeys" | "teamKey" | "memberKeyHolders">,
  threadId: string,
  reader: SyncedReader,
  trustStore: TeamTrustStore = browserTeamTrust,
): (keyVersion: number) => Promise<ContentKey | undefined> {
  return async (keyVersion) => {
    const epoch = epochOf(keyVersion);
    if (epoch === undefined) return undefined;
    const notFound = (error: unknown) => {
      if (error instanceof CoreError && error.status === 404) return undefined;
      throw error;
    };
    const list = await core.teamThreadKeys(threadId).catch(notFound);
    const copy = list?.keys.find((key) => key.keyVersion === keyVersion && key.threadId === threadId);
    if (!copy) return undefined;
    const owner = await core.memberKeyHolders(copy.personId).catch(notFound);
    const holders = (owner?.holders ?? []).map((holder) => ({ ...holder, name: "", state: holder.state ?? "trusted" as const, createdAt: "", revokedAt: null }));
    const { tenantId } = await core.whoami();
    let trusted: Set<string>;
    try {
      trusted = await trustedTeamHolders(trustStore, JSON.stringify([tenantId, reader.deviceId]), copy.personId, holders);
    } catch {
      return undefined;
    }
    const signingKey = holders.find((holder) => holder.id === copy.senderDeviceId)?.signingKey;
    const locked = fromB64u(copy.lockedKey);
    const signature = fromB64u(copy.signature);
    const sender = signingKey ? fromB64u(signingKey) : undefined;
    if (!trusted.has(copy.senderDeviceId) || !sender || !locked || !signature) return undefined;
    if (!(await verify(sender, teamThreadKeyMessage(threadId, epoch, copy.teamKeyId, locked), signature))) return undefined;
    const state = await core.teamKey(reader.deviceId);
    const mine = state.copies.find((team) => team.teamKeyId === copy.teamKeyId && team.holderId === reader.deviceId);
    const teamLocked = mine ? fromB64u(mine.lockedKey) : undefined;
    if (!mine || !teamLocked) return undefined;
    try {
      const team = await unlockTeamKey(reader.holder, tenantId, mine.epoch, reader.deviceId, teamLocked);
      return await unlockContentKey(team, threadId, epoch, copy.teamKeyId, locked);
    } catch {
      return undefined;
    }
  };
}

/**
 * Opens one event. A missing key copy is `no-key`, not an error, so a thread whose older events predate this device
 * still shows what it can. Other failures to fetch a copy (the network) reject.
 */
export async function openSyncedEvent(keys: ThreadKeys, event: SyncedEvent): Promise<OpenedEvent> {
  const base = { seq: event.seq, at: event.at };
  // Only an event with no sealed body is the organization's policy; a sealed body without a key version is malformed.
  if (!event.sealed) return { ...base, state: "local" };
  if (!event.keyVersion) return { ...base, state: "unreadable" };
  let key: ContentKey;
  try {
    key = await keys.at(event.keyVersion);
  } catch (error) {
    if (error instanceof SyncedKeyError) return { ...base, state: error.kind === "none" ? "no-key" : "unreadable" };
    throw error;
  }
  const sealed = sealedBytes(event.sealed);
  if (!sealed) return { ...base, state: "unreadable" };
  try {
    const plaintext = readPlaintext(await openEvent(key, keys.threadId, event.seq, sealed));
    return plaintext ? { ...base, state: "open", ...plaintext } : { ...base, state: "unreadable" };
  } catch {
    return { ...base, state: "unreadable" };
  }
}

/**
 * The sealed bytes. Gateway's contract is base64url without padding; standard base64 is read too, since the runner's
 * adapter is not wired yet and its engine encodes that way today.
 */
export function sealedBytes(text: string): Uint8Array<ArrayBuffer> | undefined {
  let end = text.length;
  while (end > 0 && text[end - 1] === "=") end--;
  return fromB64u(text.slice(0, end).replace(/\+/g, "-").replace(/\//g, "_"));
}

/** nexus-workbench's `Plaintext`, version 1; `undefined` for anything else. */
export function readPlaintext(bytes: Uint8Array): Pick<Extract<OpenedEvent, { state: "open" }>, "at" | "thread" | "event"> | undefined {
  try {
    const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as Record<string, unknown>;
    const thread = value.thread as Record<string, unknown> | null;
    if (value.version !== 1 || typeof value.at !== "number" || !thread || typeof value.event_json !== "string") return undefined;
    const text = (name: string) => (typeof thread[name] === "string" ? (thread[name] as string) : "");
    const event = JSON.parse(value.event_json) as unknown;
    if (!event || typeof event !== "object" || typeof (event as { kind?: unknown }).kind !== "string") return undefined;
    let origin: ExecutionOrigin | undefined;
    if (thread.origin !== undefined && thread.origin !== null) {
      try { origin = readExecutionOrigin(thread.origin); }
      catch { origin = { kind: "unknown" }; }
    }
    return {
      at: value.at,
      thread: { localId: text("local_id"), title: text("title"), projectName: text("project_name"), projectPath: text("project_path"),
        ...(origin ? { origin } : {}) },
      event: event as { kind: string } & Record<string, unknown>,
    };
  } catch {
    return undefined;
  }
}
