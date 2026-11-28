import { CoreError, type CodeAction, type CodeCommand, type CodeCommandEnvelope, type CodeControl, type CoreClient } from "@nexus/kit";
import { deviceTrustMode } from "@nexus/kit/device-trust";
import { b64u, type DeviceKey } from "@nexus/kit/e2e";
import { controlAvailability, controlNow, controlWaitable, sealCodeCommand, signCodeCommandCancel } from "@nexus/kit/remote-control";
import { gatewayKeyCopies, ThreadKeys } from "@nexus/kit/synced-sessions";
import type { CodeWait, CodeWaitStore } from "./code-draft";
import { CommandFeed, commandFeed, type FeedChange } from "./command-feed";
import { transientError } from "./connection-errors";

/**
 * `offline` with `waits`: the computer sleeps while Phone control is on, so a message can wait for it.
 * `computer` is its name. Without `waits`, nothing can be sent.
 */
export type ControlState =
  | { kind: "loading" | "disabled" | "unsupported" | "locked" | "reconnecting" }
  | { kind: "offline"; waits?: { computer: string } }
  | { kind: "ready"; control: CodeControl }
  | { kind: "error"; message: string };

/** `waitsFor`: the name of the sleeping computer a message waits for. */
type Submission = { id: string; action: CodeAction; waitsFor?: string };
export type Delivery =
  | { kind: "idle" }
  | { kind: "sending"; action: CodeAction }
  | ({ kind: "queued" | "claimed" | "completed" | "rejected" | "expired" | "unconfirmed" | "cancelled" } & Submission)
  | ({ kind: "unknown" | "retry" | "recheck"; message: string } & Submission)
  | { kind: "failed"; message: string };

/** `recheck`: the phone tried to cancel, and Gateway's answer is unknown. It only reads the receipt again, never resends. */
const CANCEL_UNCONFIRMED = "Couldn't confirm the cancel. Check again.";
const WAITING_CONNECTION = "Connection lost. Checking your message when Nexus is reachable.";

/**
 * `delivery` follows a message or an approval; `stop` follows Stop. Each has its own command, so the person can stop
 * the session while a message still waits for the computer.
 */
export interface ControlSnapshot {
  control: ControlState;
  delivery: Delivery;
  stop: Delivery;
}

/** Which in-flight command an action uses. */
export type Slot = "delivery" | "stop";
export const slotOf = (action: CodeAction): Slot => action.kind === "stop" ? "stop" : "delivery";

type Core = Pick<CoreClient, "codeControl" | "codeCommand" | "sendCodeCommand" | "cancelCodeCommand" | "e2eDevices" | "syncedThread" | "threadKey">;
type Reader = { deviceId: string; holder: DeviceKey };
/**
 * `cancelling`: the person tapped Cancel. It stays set, and the journal keeps it, so this command is never sent again,
 * even when its receipt is missing.
 */
type Pending = { envelope: CodeCommandEnvelope; action: CodeAction; claimed: boolean; attempts: number; waitsFor?: string; cancelling?: boolean };

function submissionOf(pending: Pending): Submission {
  return { id: pending.envelope.id, action: pending.action, ...(pending.waitsFor ? { waitsFor: pending.waitsFor } : {}) };
}

/** What the phone calls a computer whose name it cannot read. */
const SOME_COMPUTER = "your computer";

// Keep uncertain commands through navigation, without writing plaintext or device keys to disk. The account's
// CoreClient owns the map weakly, and both public keys distinguish a fresh key from a fresh import of the same key.
const owners = new WeakMap<Core, Map<string, CodeControlSession>>();

export function codeControlSession(core: Core, threadId: string, reader: Reader): CodeControlSession {
  let sessions = owners.get(core);
  if (!sessions) {
    sessions = new Map();
    owners.set(core, sessions);
  }
  const key = JSON.stringify([threadId, reader.deviceId, b64u(reader.holder.encryptionKey), b64u(reader.holder.signingKey)]);
  let session = sessions.get(key);
  if (!session) {
    session = new CodeControlSession(core, threadId, reader, commandFeed(core, threadId));
    sessions.set(key, session);
  }
  return session;
}

/** The poll while no stream announces this thread's commands: the stream is down, or Gateway is older. */
export const POLL_MS = 2000;
/** The backstop poll while the stream announces them: an announcement can be lost. */
export const SLOW_POLL_MS = 15_000;
let nextViewKey = 0;

/**
 * A screen's commands and receipts. Encrypted commands keep their id across uncertain sends and explicit retries. A
 * message or an approval and a Stop are separate slots: each slot holds at most one command in flight. A receipt is
 * read again at once when the thread's stream announces a new state (`feed`), and on a poll.
 */
export class CodeControlSession {
  readonly viewKey = ++nextViewKey;
  private value: ControlSnapshot = { control: { kind: "loading" }, delivery: { kind: "idle" }, stop: { kind: "idle" } };
  private readonly listeners = new Set<() => void>();
  private readonly pending: Record<Slot, Pending | undefined> = { delivery: undefined, stop: undefined };
  private readonly submitting: Record<Slot, boolean> = { delivery: false, stop: false };
  private active: AbortController | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private unfeed: (() => void) | undefined;
  /** Gateway's last control answer: it holds Gateway's clock offset (`controlNow`). */
  private clock: CodeControl | undefined;
  /** The runner's name, by its device id: read once from the device directory. */
  private readonly names = new Map<string, string>();
  /** This account's journal of waiting messages, once the screen knows the person. */
  private waitStore: CodeWaitStore | undefined;

  constructor(private readonly core: Core, private readonly threadId: string, private readonly reader: Reader, private readonly feed: CommandFeed = new CommandFeed()) {}

  readonly snapshot = (): ControlSnapshot => this.value;
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  start(): void {
    if (this.active) return;
    const controller = new AbortController();
    this.active = controller;
    for (const slot of SLOTS) if (this.value[slot].kind === "sending" && !this.pending[slot]) this.set(slot, { kind: "idle" });
    this.unfeed = this.feed.subscribe((change) => this.announced(change, controller.signal));
    void this.poll(controller.signal);
  }

  pause(): void {
    this.active?.abort();
    this.active = undefined;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.unfeed?.();
    this.unfeed = undefined;
  }

  /** Gateway's clock now, as far as this phone knows it. */
  private now(): number {
    return controlNow(this.clock);
  }

  private async poll(signal: AbortSignal): Promise<void> {
    this.timer = undefined;
    await this.refresh(signal);
    if (!signal.aborted) this.timer = setTimeout(() => void this.poll(signal), this.pollDelay());
  }

  private pollDelay(): number {
    return this.feed.live ? SLOW_POLL_MS : POLL_MS;
  }

  /** The stream announced a command's new state, or opened or closed. */
  private announced(change: FeedChange, signal: AbortSignal): void {
    if (signal.aborted) return;
    if (change.kind === "command") {
      if (change.command.threadId !== this.threadId) return;
      // Even while its send is in flight: an announced command is stored, and `show` keeps the newest state.
      for (const slot of SLOTS) {
        const pending = this.pending[slot];
        if (pending && pending.envelope.id === change.command.id) void this.readReceipt(slot, pending, signal);
      }
      return;
    }
    // A state announced while the stream was down is lost: read the receipts again once it is back.
    if (change.live) for (const slot of SLOTS) {
      const pending = this.pending[slot];
      if (pending && !this.submitting[slot]) void this.readReceipt(slot, pending, signal);
    }
    // A poll in flight sets its own timer with the new pace when it ends.
    if (this.timer !== undefined) {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => void this.poll(signal), this.pollDelay());
    }
  }

  private update(patch: Partial<ControlSnapshot>): void {
    this.value = { ...this.value, ...patch };
    this.listeners.forEach((listener) => listener());
  }

  private set(slot: Slot, delivery: Delivery): void {
    this.update(slot === "stop" ? { stop: delivery } : { delivery });
  }

  private show(slot: Slot, command: CodeCommand): void {
    const pending = this.pending[slot];
    if (!pending || command.id !== pending.envelope.id || command.threadId !== this.threadId) return;
    if (pending.claimed && (command.state === "queued" || command.state === "expired")) return;
    if (command.state === "claimed") pending.claimed = true;
    const kind = command.state;
    this.set(slot, { kind, ...submissionOf(pending) });
    if (kind === "completed" || kind === "rejected" || kind === "expired" || kind === "cancelled") this.keep(slot, undefined);
  }

  /**
   * Holds `pending` in its slot. A message that waits for a sleeping computer is also written to this account's journal,
   * so it survives an app restart; any other change to the slot removes the saved copy.
   */
  private keep(slot: Slot, pending: Pending | undefined): void {
    const previous = this.pending[slot];
    this.pending[slot] = pending;
    const store = this.waitStore;
    if (slot !== "delivery" || !store) return;
    if (pending?.waitsFor && pending.action.kind === "send") {
      const saved: CodeWait = { envelope: pending.envelope, text: pending.action.text, waitsFor: pending.waitsFor, ...(pending.cancelling ? { cancelling: true } : {}) };
      void store.save(saved).catch(() => undefined);
    } else if (previous?.waitsFor) void store.remove().catch(() => undefined);
  }

  /**
   * Attaches this account's journal of waiting messages. A message saved before an app restart comes back
   * as this session's delivery, and its receipt is read from Gateway by id. A stable arrow: a screen passes it to an
   * effect.
   */
  readonly attachWaits = (store: CodeWaitStore | undefined): void => {
    if (this.waitStore === store) return;
    this.waitStore = store;
    if (store) void this.restore(store);
  };

  private async restore(store: CodeWaitStore): Promise<void> {
    let saved: CodeWait | undefined;
    try {
      saved = await store.load();
    } catch {
      // A saved copy that no longer opens is dropped: Gateway still holds the command, and it expires there.
      void store.remove().catch(() => undefined);
      return;
    }
    if (!saved || this.waitStore !== store || this.pending.delivery || this.submitting.delivery) return;
    const pending: Pending = { envelope: saved.envelope, action: { kind: "send", text: saved.text }, claimed: false, attempts: 1, waitsFor: saved.waitsFor, ...(saved.cancelling ? { cancelling: true } : {}) };
    this.pending.delivery = pending;
    const submission = submissionOf(pending);
    // A cancel that was in flight when the app closed: no Cancel and no Retry until the receipt says what happened.
    this.set("delivery", pending.cancelling ? { kind: "unknown", ...submission, message: "Checking whether the cancel went through…" } : { kind: "queued", ...submission });
    const signal = this.active?.signal;
    if (signal && !signal.aborted) await this.readReceipt("delivery", pending, signal);
  }

  /** The runner's name from the device directory, read once. */
  private async computerName(runnerDeviceId: string, devices?: readonly { id: string; name: string }[]): Promise<string> {
    const known = this.names.get(runnerDeviceId);
    if (known) return known;
    try {
      const listed = devices ?? (await this.core.e2eDevices()).devices;
      const name = listed.find((device) => device.id === runnerDeviceId)?.name.trim();
      if (!name) return SOME_COMPUTER;
      this.names.set(runnerDeviceId, name);
      return name;
    } catch {
      return SOME_COMPUTER;
    }
  }

  private async refresh(signal: AbortSignal): Promise<void> {
    try {
      const control = await this.core.codeControl(this.threadId, signal);
      if (signal.aborted) return;
      this.clock = control;
      const kind = controlAvailability(control);
      if (kind === "ready") this.update({ control: { kind, control } });
      else if (kind === "offline" && controlWaitable(control) && control.runnerDeviceId) {
        const computer = await this.computerName(control.runnerDeviceId);
        if (!signal.aborted) this.update({ control: { kind, waits: { computer } } });
      } else this.update({ control: { kind } });
    } catch (error) {
      if (signal.aborted) return;
      if (error instanceof CoreError && error.status === 404) this.update({ control: { kind: "unsupported" } });
      else if (transientError(error)) this.update({ control: { kind: "reconnecting" } });
      else this.update({ control: { kind: "error", message: failure(error) } });
    }
    await Promise.all(SLOTS.map((slot) => {
      const pending = this.pending[slot];
      if (!pending || signal.aborted || this.submitting[slot]) return;
      return this.readReceipt(slot, pending, signal);
    }));
  }

  private async readReceipt(slot: Slot, pending: Pending, signal: AbortSignal): Promise<void> {
    try {
      const command = await this.core.codeCommand(this.threadId, pending.envelope.id, signal);
      if (!signal.aborted && this.pending[slot] === pending) this.show(slot, command);
    } catch (error) {
      if (signal.aborted || this.pending[slot] !== pending) return;
      const submission = submissionOf(pending);
      if (pending.cancelling) {
        // Gateway keeps a cancelled command, so it reads `cancelled`. A missing one is never sent again: the person
        // asked for it not to run.
        this.set(slot, { kind: "recheck", ...submission, message: CANCEL_UNCONFIRMED });
      } else if (error instanceof CoreError && error.status === 404) {
        if (pending.envelope.expiresAt > this.now() && !pending.claimed) {
          this.set(slot, { kind: "retry", ...submission, message: "Delivery was not confirmed. Retry sends this same command once." });
        } else {
          this.set(slot, { kind: "unknown", ...submission, message: "The command's confirmation is unavailable. The action may already have run. Check the session before sending it again." });
        }
      } else {
        const message = pending.waitsFor && transientError(error)
          ? WAITING_CONNECTION
          : "Couldn't check delivery. Keep this session open to check again.";
        this.set(slot, { kind: "unknown", ...submission, message });
      }
    }
  }

  /**
   * Sends `action` in its slot. A slot that still holds a command refuses, so nothing is sent twice. `waits`:
   * a message may wait up to 24 hours for a computer that is asleep. Stop and answers never wait.
   */
  async submit(action: CodeAction, options: { waits?: boolean } = {}): Promise<void> {
    const waits = options.waits === true && action.kind === "send";
    const slot = slotOf(action);
    const signal = this.active?.signal;
    if (!signal || signal.aborted || this.submitting[slot] || this.pending[slot]) return;
    this.submitting[slot] = true;
    this.set(slot, { kind: "sending", action });
    try {
      const [control, directory, thread] = await Promise.all([
        this.core.codeControl(this.threadId, signal),
        this.core.e2eDevices(),
        this.core.syncedThread(this.threadId),
      ]);
      if (signal.aborted) return;
      this.clock = control;
      if (thread.keyVersion === null) {
        this.update({ control: { kind: "locked" } });
        this.set(slot, { kind: "failed", message: "This phone needs the session's current key before it can send commands." });
        return;
      }
      // The mode of the list just read: with `gateway`, a computer Gateway lists as trusted is trusted.
      const reader = { deviceId: this.reader.deviceId, holder: this.reader.holder, trust: deviceTrustMode(directory) };
      const keys = new ThreadKeys(this.threadId, reader, directory.devices, gatewayKeyCopies(this.core, this.threadId, this.reader.deviceId));
      await keys.at(thread.keyVersion);
      if (signal.aborted) return;
      const envelope = await sealCodeCommand({ threadId: this.threadId, action, reader, devices: directory.devices, control, waits });
      if (signal.aborted) return;
      const waitsFor = waits && control.runnerDeviceId ? await this.computerName(control.runnerDeviceId, directory.devices) : undefined;
      const pending: Pending = { envelope, action, claimed: false, attempts: 0, ...(waitsFor ? { waitsFor } : {}) };
      // Saved before the send, so a restart mid-send still checks this command by id instead of sending a new one.
      this.keep(slot, pending);
      await this.deliver(slot, pending, signal);
    } catch (error) {
      if (!signal.aborted) this.set(slot, { kind: "failed", message: failure(error) });
    } finally {
      this.submitting[slot] = false;
    }
  }

  async retry(slot: Slot = "delivery"): Promise<void> {
    const pending = this.pending[slot];
    const signal = this.active?.signal;
    if (!pending || !signal || signal.aborted || this.submitting[slot] || pending.claimed) return;
    if (pending.cancelling) return this.recheck(slot);
    this.submitting[slot] = true;
    try {
      if (pending.envelope.expiresAt <= this.now()) {
        this.set(slot, { kind: "unknown", ...submissionOf(pending), message: "The delivery deadline passed. Checking the original command before sending anything again." });
        await this.readReceipt(slot, pending, signal);
        return;
      }
      await this.deliver(slot, pending, signal);
    } finally {
      this.submitting[slot] = false;
    }
  }

  /** A message that waits for a sleeping computer can be taken back until the computer receives it. */
  canCancel(slot: Slot = "delivery"): boolean {
    const pending = this.pending[slot];
    return !!pending?.waitsFor && !pending.claimed && !this.submitting[slot] && this.value[slot].kind === "queued";
  }

  async cancel(slot: Slot = "delivery"): Promise<void> {
    const pending = this.pending[slot];
    const signal = this.active?.signal;
    if (!pending || !signal || signal.aborted || !this.canCancel(slot)) return;
    this.submitting[slot] = true;
    const submission = submissionOf(pending);
    // Journaled before the request, so neither a lost answer nor an app restart can turn this command into a resend.
    pending.cancelling = true;
    this.keep(slot, pending);
    try {
      const body = await signCodeCommandCancel(this.threadId, pending.envelope.id, this.reader);
      const command = await this.core.cancelCodeCommand(this.threadId, pending.envelope.id, body, signal);
      if (!signal.aborted && this.pending[slot] === pending) this.show(slot, command);
    } catch {
      if (signal.aborted || this.pending[slot] !== pending) return;
      // Claimed first, or no answer: the receipt says what happened. Gateway answers `cancelled` for a cancel that landed.
      this.set(slot, { kind: "unknown", ...submission, message: "Couldn't cancel. Checking whether the computer received it…" });
      await this.readReceipt(slot, pending, signal);
    } finally {
      this.submitting[slot] = false;
    }
  }

  /** After a cancel whose answer is unknown: reads the receipt again. It never sends the command. */
  async recheck(slot: Slot = "delivery"): Promise<void> {
    const pending = this.pending[slot];
    const signal = this.active?.signal;
    if (!pending || !signal || signal.aborted || this.submitting[slot]) return;
    this.submitting[slot] = true;
    try {
      await this.readReceipt(slot, pending, signal);
    } finally {
      this.submitting[slot] = false;
    }
  }

  /**
   * Past the deadline, or after a cancel Gateway cannot confirm: the person may stop waiting. The phone then forgets
   * the command and never sends it again.
   */
  canStopChecking(slot: Slot = "delivery"): boolean {
    const pending = this.pending[slot];
    return !!pending && !this.submitting[slot] && (pending.envelope.expiresAt <= this.now() || this.value[slot].kind === "recheck");
  }

  stopChecking(slot: Slot = "delivery"): void {
    const pending = this.pending[slot];
    if (!pending || !this.canStopChecking(slot)) return;
    this.keep(slot, undefined);
    this.set(slot, { kind: "unconfirmed", id: pending.envelope.id, action: pending.action });
  }

  private async deliver(slot: Slot, pending: Pending, signal: AbortSignal): Promise<void> {
    pending.attempts += 1;
    this.set(slot, { kind: "sending", action: pending.action });
    try {
      const command = await this.core.sendCodeCommand(this.threadId, pending.envelope, signal);
      if (!signal.aborted) this.show(slot, command);
    } catch (error) {
      if (signal.aborted) return;
      if (pending.attempts === 1 && error instanceof CoreError && error.status >= 400 && error.status < 500 && error.status !== 408 && error.status !== 429) {
        this.keep(slot, undefined);
        this.set(slot, { kind: "failed", message: failure(error) });
      } else {
        const message = pending.waitsFor && transientError(error)
          ? WAITING_CONNECTION
          : "Delivery is uncertain. Checking before sending anything again.";
        this.set(slot, { kind: "unknown", ...submissionOf(pending), message });
        if (pending.attempts > 1) await this.readReceipt(slot, pending, signal);
      }
    }
  }
}

const SLOTS: readonly Slot[] = ["delivery", "stop"];

export function deliveryPending(delivery: Delivery): boolean {
  return delivery.kind === "sending" || delivery.kind === "queued" || delivery.kind === "claimed" || delivery.kind === "unknown" || delivery.kind === "retry" || delivery.kind === "recheck";
}

export function deliveryText(delivery: Delivery): string | null {
  switch (delivery.kind) {
    case "idle": return null;
    case "sending": return delivery.action.kind === "stop" ? "Sending Stop to the computer…" : "Sending to the computer…";
    case "queued": return delivery.waitsFor ? `Waits for ${delivery.waitsFor}. Runs when it's back, within 24 hours.` : "Waiting for the computer to receive it…";
    case "cancelled": return "Cancelled. Your message is still here.";
    case "claimed": return "The computer received it. Waiting for confirmation…";
    case "completed": return delivery.action.kind === "send" ? "Message delivered to the session." : delivery.action.kind === "stop" ? "The session stopped." : "Your answer reached the session.";
    case "rejected": return "The computer couldn't apply that action. Check the session and try again.";
    case "expired": return expiredText(delivery.action);
    case "unconfirmed": return "Stopped checking delivery. The action may already have run. Check the session before sending it again.";
    case "unknown":
    case "retry":
    case "recheck":
    case "failed": return delivery.message;
    default: {
      const exhaustive: never = delivery;
      return exhaustive;
    }
  }
}

/**
 * Gateway also reports a claimed command as expired once its deadline is well past, and still takes a late result. So
 * `expired` does not prove the computer never received it: the text never says so.
 */
function expiredText(action: CodeAction): string {
  switch (action.kind) {
    case "send": return "The computer did not confirm this message in time. It may still have run. Check the session before sending it again. Your draft is still here.";
    case "stop": return "The computer did not confirm Stop in time. The session may have stopped. Check the session before you tap Stop again.";
    case "approve": return "The computer did not confirm your answer in time. It may still have reached the session. Check the session before answering again.";
    default: return "The computer did not confirm this command in time. It may still have run. Check the session before sending it again.";
  }
}

function failure(error: unknown): string {
  return error instanceof Error ? error.message : "Couldn't reach the computer. Try again.";
}
