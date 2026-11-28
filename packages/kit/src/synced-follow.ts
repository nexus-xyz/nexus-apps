/**
 * Following a synced thread live (eng-architecture.md §4.2): Gateway's stream says only that the
 * thread now holds events up to N; the viewer then reads "after the last one I hold" and appends. On a dropped stream
 * it waits, reads after its last again and reopens the stream from there, so it misses nothing and repeats nothing:
 * it hands on only the events that continue its last number without a gap, each once.
 *
 * Phone control's receipts ride the same stream for the thread's person (`onCommand`), and
 * `followComputerEvents` follows a person's computer requests and commands on `GET /v1/computers/stream`.
 */
import type { SyncedEvent, SyncedEventPage, SyncedThreadAdvanced } from "./api";
import type { CodeCommandChanged } from "./code-control-types";
import type { ComputerRequestChanged } from "./computer-types";
import { CoreError } from "./core-client";

export interface FollowOptions {
  /** The last event number this viewer holds; -1 for none. */
  after: number;
  /**
   * Gateway's stream from `after` (`CoreClient.streamSyncedThread`): calls `onOpen` once it is open, resolves
   * `"deleted"` after its last event and `"closed"` when it just ends. `commands` gets phone control's events;
   * a stream that does not pass them on still follows the thread.
   */
  stream: (
    after: number,
    onAdvance: (event: SyncedThreadAdvanced) => void,
    signal: AbortSignal,
    onOpen: () => void,
    commands?: { live?: () => void; command?: (event: CodeCommandChanged) => void },
  ) => Promise<"deleted" | "closed">;
  /**
   * Phone control: this stream carries the thread's command receipts. Only the thread's person gets it, and
   * only from a Gateway that announces them. It holds until `onState` says `reconnecting`.
   */
  onCommandsLive?: () => void;
  /** Phone control: one of the thread's commands has a new state. Ids and state only: read the receipt. */
  onCommand?: (event: CodeCommandChanged) => void;
  /** A page of events numbered after `after`. */
  read: (after: number) => Promise<SyncedEventPage>;
  /** New events, in order, each once, continuing the last number handed on. */
  onEvents: (events: SyncedEvent[]) => void | Promise<void>;
  /** The thread was deleted: the follow ends. */
  onDeleted: () => void;
  /** `live` once the stream is open again, `reconnecting` while waiting to reopen it. */
  onState?: (state: "live" | "reconnecting") => void;
  signal: AbortSignal;
  /** How long to wait before reconnect `attempt` (1 and up), in milliseconds. */
  backoffMs?: (attempt: number) => number;
}

/** 1 s, 2 s, 4 s … up to 30 s. */
export const defaultBackoff = (attempt: number) => Math.min(30_000, 1000 * 2 ** Math.max(0, attempt - 1));

/** Follows until the thread is deleted or `signal` aborts. A thread that is gone or not yours (404) rejects. */
export async function followThread(options: FollowOptions): Promise<void> {
  const { signal } = options;
  const backoff = options.backoffMs ?? defaultBackoff;
  let last = options.after;
  let attempt = 0;
  const commands = options.onCommandsLive || options.onCommand ? { live: options.onCommandsLive, command: options.onCommand } : undefined;

  // Reads every event after `last`, a page at a time, and hands on the ones that continue it.
  const catchUp = async () => {
    for (;;) {
      const page = await options.read(last);
      const fresh = contiguous(page.events, last);
      if (fresh.length) {
        // Reads are one at a time, so `last` moves only once the viewer took the batch: a batch it failed on is read
        // again after the reconnect.
        await options.onEvents(fresh);
        last = fresh.at(-1)!.seq;
      }
      if (!page.more || !fresh.length || signal.aborted) return;
    }
  };

  while (!signal.aborted) {
    let reading = Promise.resolve();
    let failed: unknown;
    // This stream's own stop: the follow's signal, or a read that failed while the stream stays open.
    const stream = new AbortController();
    const stop = () => stream.abort();
    signal.addEventListener("abort", stop, { once: true });
    try {
      // Before reopening: whatever arrived while the stream was down.
      if (attempt > 0) await catchUp();
      const ended = await options.stream(
        last,
        (event) => {
          if (event.lastSeq <= last) return;
          // One read at a time, in the order the stream said so. A failed read ends the stream, so the follow
          // reconnects and reads the same batch again.
          reading = reading.then(catchUp).catch((error: unknown) => {
            failed ??= error;
            stream.abort();
          });
        },
        stream.signal,
        () => {
          attempt = 0;
          options.onState?.("live");
        },
        commands,
      );
      await reading;
      if (failed) throw failed;
      if (ended === "deleted") return options.onDeleted();
      // A stream that just ends (a keep-alive check, a draining Gateway copy) is reopened after a short wait.
      attempt += 1;
    } catch (error) {
      // A stream that dropped mid-read finishes that read first, so reads never overlap.
      await reading;
      if (signal.aborted) return;
      const cause = failed ?? error;
      if (cause instanceof CoreError && cause.status === 410) return options.onDeleted();
      if (cause instanceof CoreError && cause.status === 404) throw cause;
      attempt += 1;
    } finally {
      signal.removeEventListener("abort", stop);
    }
    options.onState?.("reconnecting");
    await sleep(backoff(attempt), signal);
  }
}

/**
 * Following your phone control receipts live: `CoreClient.streamComputerEvents`, reopened after a drop.
 * `onState` says `live` at each open: read your pending receipts then, since an event sent while the stream was down is
 * lost. It says `down` while the stream is closed. The follow ends when `signal` aborts, and with no retry when Gateway
 * refuses the stream for good (403, or 404 from an older Gateway): poll then.
 */
export async function followComputerEvents(options: {
  stream: (
    on: { request?: (event: ComputerRequestChanged) => void; command?: (event: CodeCommandChanged) => void },
    signal: AbortSignal,
    onOpen: () => void,
  ) => Promise<unknown>;
  onRequest?: (event: ComputerRequestChanged) => void;
  onCommand?: (event: CodeCommandChanged) => void;
  onState?: (state: "live" | "down") => void;
  signal: AbortSignal;
  /** How long to wait before reconnect `attempt` (1 and up), in milliseconds. */
  backoffMs?: (attempt: number) => number;
}): Promise<void> {
  const { signal } = options;
  const backoff = options.backoffMs ?? defaultBackoff;
  let attempt = 0;
  while (!signal.aborted) {
    try {
      const open = () => {
        attempt = 0;
        options.onState?.("live");
      };
      await options.stream({ request: options.onRequest, command: options.onCommand }, signal, open);
    } catch (error) {
      if (signal.aborted) return;
      if (error instanceof CoreError && (error.status === 403 || error.status === 404)) {
        options.onState?.("down");
        return;
      }
    }
    if (signal.aborted) return;
    attempt += 1;
    options.onState?.("down");
    await sleep(backoff(attempt), signal);
  }
}

/** The events of `events` that continue `last` without a gap, each number once. */
export function contiguous(events: readonly SyncedEvent[], last: number): SyncedEvent[] {
  const out: SyncedEvent[] = [];
  let next = last + 1;
  for (const event of [...events].sort((a, b) => a.seq - b.seq)) {
    if (event.seq < next) continue;
    if (event.seq > next) break;
    out.push(event);
    next += 1;
  }
  return out;
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(done, ms);
    signal.addEventListener("abort", done, { once: true });
    function done() {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
  });
}
