import { describe, expect, it, vi } from "vitest";
import type { SyncedEvent, SyncedEventPage, SyncedThreadAdvanced } from "./api";
import type { CodeCommandChanged } from "./code-control-types";
import type { ComputerRequestChanged } from "./computer-types";
import { CoreError } from "./core-client";
import { contiguous, followComputerEvents, followThread, type FollowOptions } from "./synced-follow";

const THREAD = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506";
const event = (seq: number): SyncedEvent => ({ seq, at: seq, agent: "claude" });

interface FakeStream {
  after: number;
  drop(): void;
  close(): void;
  delete(): void;
}

/**
 * A fake session store and its stream, as Gateway behaves: a stream opened after N says `thread` at once
 * when the thread holds more, a read gives at most `pageSize` events after N, and appends tell only the open stream.
 */
function fakeGateway(initial: number, pageSize = 100) {
  const events = Array.from({ length: initial }, (_, seq) => event(seq));
  const streams: FakeStream[] = [];
  let live: ((event: SyncedThreadAdvanced) => void) | undefined;
  const reads: number[] = [];
  const read = async (after: number): Promise<SyncedEventPage> => {
    reads.push(after);
    const rest = events.filter((stored) => stored.seq > after);
    return { threadId: THREAD, lastSeq: events.length - 1, events: rest.slice(0, pageSize), more: rest.length > pageSize };
  };
  const stream: FollowOptions["stream"] = (after, onAdvance, signal, onOpen) =>
    new Promise((resolve, reject) => {
      const end = () => (live = undefined);
      streams.push({
        after,
        drop: () => (end(), reject(new TypeError("network error"))),
        close: () => (end(), resolve("closed")),
        delete: () => (end(), resolve("deleted")),
      });
      signal.addEventListener("abort", () => (end(), reject(new DOMException("aborted", "AbortError"))));
      live = onAdvance;
      onOpen();
      if (events.length - 1 > after) onAdvance({ threadId: THREAD, lastSeq: events.length - 1 });
    });
  return {
    events,
    streams,
    reads,
    read,
    stream,
    append(count: number, tell = true) {
      for (let i = 0; i < count; i++) events.push(event(events.length));
      if (tell) live?.({ threadId: THREAD, lastSeq: events.length - 1 });
    },
  };
}

function follow(gateway: ReturnType<typeof fakeGateway>, after: number) {
  const received: number[] = [];
  const states: string[] = [];
  const controller = new AbortController();
  let deleted = false;
  const done = followThread({
    after,
    stream: gateway.stream,
    read: gateway.read,
    onEvents: (events) => void received.push(...events.map((event) => event.seq)),
    onDeleted: () => (deleted = true),
    onState: (state) => void states.push(state),
    signal: controller.signal,
    backoffMs: () => 0,
  });
  return { received, states, controller, done, deleted: () => deleted };
}

describe("synced sessions: following live", () => {
  it("reconnects after a dropped stream with no gap and no duplicate", async () => {
    const gateway = fakeGateway(3);
    const viewer = follow(gateway, 2);
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(1));
    gateway.append(2);
    await vi.waitFor(() => expect(viewer.received).toEqual([3, 4]));

    // The connection drops; events arrive while the viewer is away.
    gateway.streams[0]!.drop();
    gateway.append(2, false);
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(2));
    // It read after its last before reopening, and reopened from its new last.
    expect(gateway.streams.map((stream) => stream.after)).toEqual([2, 6]);
    expect(viewer.received).toEqual([3, 4, 5, 6]);

    gateway.append(1);
    await vi.waitFor(() => expect(viewer.received).toEqual([3, 4, 5, 6, 7]));
    expect(viewer.states).toEqual(["live", "reconnecting", "live"]);

    gateway.streams[1]!.delete();
    await viewer.done;
    expect(viewer.deleted()).toBe(true);
    expect(viewer.received).toEqual([3, 4, 5, 6, 7]);
  });

  it("reopens a stream that just ends, and reads past a page at a time", async () => {
    const gateway = fakeGateway(1, 2);
    const viewer = follow(gateway, 0);
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(1));
    gateway.append(5);
    await vi.waitFor(() => expect(viewer.received).toEqual([1, 2, 3, 4, 5]));
    // Three pages of at most two after 0, 2 and 4.
    expect(gateway.reads).toEqual([0, 2, 4]);
    gateway.streams[0]!.close();
    gateway.append(1, false);
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(2));
    expect(viewer.received).toEqual([1, 2, 3, 4, 5, 6]);
    viewer.controller.abort();
    await viewer.done;
    expect(viewer.deleted()).toBe(false);
  });

  it("reads a batch again after the viewer failed on it, so it is shown once and not lost", async () => {
    const gateway = fakeGateway(3);
    const received: number[] = [];
    let failures = 1;
    const controller = new AbortController();
    const done = followThread({
      after: 2,
      stream: gateway.stream,
      read: gateway.read,
      // The first batch fails as opening it would on a dropped connection.
      onEvents: (events) => {
        if (failures-- > 0) throw new TypeError("network error");
        received.push(...events.map((one) => one.seq));
      },
      onDeleted: () => undefined,
      signal: controller.signal,
      backoffMs: () => 0,
    });
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(1));
    gateway.append(2);
    await vi.waitFor(() => expect(received).toEqual([3, 4]));
    // The reconnect read after 2, the last batch the viewer took, and reopened from 4.
    expect(gateway.streams.map((stream) => stream.after)).toEqual([2, 4]);
    controller.abort();
    await done;
    expect(received).toEqual([3, 4]);
  });

  it("hands on nothing twice when the stream repeats itself or reports a number it already has", async () => {
    const gateway = fakeGateway(5);
    const viewer = follow(gateway, 4);
    await vi.waitFor(() => expect(gateway.streams).toHaveLength(1));
    gateway.append(1);
    gateway.append(0);
    gateway.append(0);
    await vi.waitFor(() => expect(viewer.received).toEqual([5]));
    viewer.controller.abort();
    await viewer.done;
    expect(viewer.received).toEqual([5]);
  });

  it("ends on a deleted thread (410), and rejects on one that is not yours (404)", async () => {
    const gone = followThread({
      after: -1,
      stream: async () => Promise.reject(new CoreError(410, "deleted", "thread_deleted")),
      read: async () => ({ threadId: THREAD, lastSeq: null, events: [], more: false }),
      onEvents: () => undefined,
      onDeleted: vi.fn(),
      signal: new AbortController().signal,
      backoffMs: () => 0,
    });
    await expect(gone).resolves.toBeUndefined();
    const missing = followThread({
      after: -1,
      stream: async () => Promise.reject(new CoreError(404, "not found", "thread_not_found")),
      read: async () => ({ threadId: THREAD, lastSeq: null, events: [], more: false }),
      onEvents: () => undefined,
      onDeleted: () => undefined,
      signal: new AbortController().signal,
      backoffMs: () => 0,
    });
    await expect(missing).rejects.toMatchObject({ status: 404 });
  });

  it("passes phone control's events on to the follower", async () => {
    const commands: CodeCommandChanged[] = [];
    let live = 0;
    const controller = new AbortController();
    const done = followThread({
      after: -1,
      stream: (_after, _onAdvance, signal, onOpen, on) =>
        new Promise((resolve) => {
          onOpen();
          on?.live?.();
          on?.command?.({ threadId: THREAD, id: "c1", runnerDeviceId: "dev_r", state: "claimed" });
          signal.addEventListener("abort", () => resolve("closed"));
        }),
      read: async () => ({ threadId: THREAD, lastSeq: null, events: [], more: false }),
      onEvents: () => undefined,
      onDeleted: () => undefined,
      onCommandsLive: () => void (live += 1),
      onCommand: (event) => void commands.push(event),
      signal: controller.signal,
      backoffMs: () => 0,
    });
    await vi.waitFor(() => expect(commands.map((one) => one.state)).toEqual(["claimed"]));
    expect(live).toBe(1);
    controller.abort();
    await done;
  });

  it("follows the computers' stream, reopens it, and stops for good on a 404", async () => {
    const requests: ComputerRequestChanged[] = [];
    const states: string[] = [];
    let opened = 0;
    const done = followComputerEvents({
      stream: async (on, _signal, onOpen) => {
        opened += 1;
        if (opened === 3) throw new CoreError(404, "not found");
        onOpen();
        on.request?.({ computerId: "k1", id: `r${opened}`, state: "replied" });
      },
      onRequest: (event) => void requests.push(event),
      onState: (state) => void states.push(state),
      signal: new AbortController().signal,
      backoffMs: () => 0,
    });
    await expect(done).resolves.toBeUndefined();
    expect(requests.map((one) => one.id)).toEqual(["r1", "r2"]);
    expect(states).toEqual(["live", "down", "live", "down", "down"]);
  });

  it("keeps only the events that continue the last number, in order, once", () => {
    const seqs = (events: SyncedEvent[]) => events.map((one) => one.seq);
    expect(seqs(contiguous([event(4), event(3), event(5)], 2))).toEqual([3, 4, 5]);
    expect(seqs(contiguous([event(1), event(2), event(3), event(3)], 2))).toEqual([3]);
    // A gap stops it: 6 waits until 5 is read.
    expect(seqs(contiguous([event(3), event(4), event(6)], 2))).toEqual([3, 4]);
    expect(seqs(contiguous([event(5)], 2))).toEqual([]);
  });
});
