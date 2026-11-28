/**
 * Reading synced coding sessions on the phone, the hooks: this phone as a reader, the list's titles, and
 * one session followed live. A port of Nexus UI's `synced/use-synced.ts` and `reader.ts`, with the phone's own key
 * (`device-key.ts`) and its pairing. Everything decrypted stays in memory: React state and a cache of titles that a
 * restart empties. Nothing decrypted is written to the device or sent anywhere.
 */
import { CoreError, type CoreClient, type createCoreClient, type E2eDevice, type SyncedThread } from "@nexus/kit";
import { deviceTrustMode, type DeviceTrustMode } from "@nexus/kit/device-trust";
import { b64u, type DeviceKey } from "@nexus/kit/e2e";
import { deviceStatus, thisDevice } from "@nexus/kit/pairing";
import { followThread } from "@nexus/kit/synced-follow";
import { gatewayKeyCopies, openSyncedEvent, type OpenedEvent, SyncedKeyError, type SyncedReader, ThreadKeys } from "@nexus/kit/synced-sessions";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppState, Platform } from "react-native";
import { commandFeed } from "./command-feed";
import { latestTitle, mergeEvents, readerState, waitsOnYou, type ReaderState, type TitleState } from "./code-sessions";
import { createDeviceKey, forgetDeviceKey, loadDeviceKey } from "./device-key";
import { ensureGatewayDevice, needsGatewayRegistration, type PhoneDevice } from "./gateway-device";
import { connectionSignal } from "./reconnect";

type Core = ReturnType<typeof createCoreClient>;

/** How often a pending phone asks whether it was approved. */
const PENDING_POLL_MS = 3000;
/** Events per read: a session opens on its last 100 and pages back 100 at a time. */
export const EVENTS_PAGE = 100;
/** Titles opened at once on the list. */
const TITLE_WORKERS = 4;

/** What the directory calls this phone until the person renames it. */
export const DEVICE_NAME = Platform.OS === "android" ? "Android phone" : Platform.OS === "ios" && Platform.isPad ? "iPad" : "iPhone";

const PHONE_KEYS = { load: loadDeviceKey, create: createDeviceKey, forget: forgetDeviceKey };

/** This phone's device, registered when Gateway vouches for the person's devices (`gateway-device.ts`). */
export function ensurePhoneDevice(core: Pick<CoreClient, "e2eDevices" | "registerE2eDevice">): Promise<PhoneDevice> {
  return ensureGatewayDevice(core, PHONE_KEYS, DEVICE_NAME, () => connectionSignal());
}

/**
 * Registers this phone right after sign-in, and after an organization switch, when the list's `trust` is `gateway`
 * . Nothing shows: a failure waits for a screen that needs the device, which tries again.
 */
export function useGatewayDevice(core: Pick<CoreClient, "e2eDevices" | "registerE2eDevice"> | null | undefined): void {
  useEffect(() => {
    if (core) ensurePhoneDevice(core).catch(() => undefined);
  }, [core]);
}

export interface Reader {
  state: ReaderState | { kind: "error"; message: string };
  /** The devices list's mode: `gateway` hides pairing, codes and approvals. `undefined` until read. */
  trust?: DeviceTrustMode;
  /** Makes a key if the phone has none and registers it: the phone then waits for approval (or is trusted at once). */
  pair(): Promise<void>;
  /** A fresh key and a fresh registration, for an expired or revoked phone. */
  pairAgain(): Promise<void>;
  refresh(): void;
  busy: boolean;
}

/** This phone as a reader. The directory is read on focus, and every 3 s while the phone waits for approval. */
export function useReader(core: Core): Reader {
  const [key, setKey] = useState<DeviceKey | undefined | null>(null);
  const [directory, setDirectory] = useState<{ core: Core; devices: E2eDevice[]; trust: DeviceTrustMode }>();
  const devices = directory?.core === core ? directory.devices : undefined;
  const trust = directory?.core === core ? directory.trust : undefined;
  const [foreground, setForeground] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [attempt, setAttempt] = useState(0);
  const reading = useRef<Core | undefined>(undefined);
  const current = useRef(core);
  current.current = core;

  const read = useCallback(() => {
    if (reading.current === core) return Promise.resolve();
    reading.current = core;
    setNow(Date.now());
    return core.e2eDevices(connectionSignal()).then(
      (list) => {
        if (current.current !== core) return;
        setDirectory({ core, devices: list.devices, trust: deviceTrustMode(list) });
        setFailure(undefined);
      },
      (error: unknown) => { if (current.current === core) setFailure(message(error)); },
    ).finally(() => { if (reading.current === core) reading.current = undefined; });
  }, [core]);

  useEffect(() => {
    let live = true;
    loadDeviceKey().then(
      (loaded) => live && setKey(loaded),
      (error: unknown) => live && setFailure(message(error)),
    );
    return () => {
      live = false;
    };
  }, []);
  useFocusEffect(
    useCallback(() => {
      const update = () => {
        const active = AppState.currentState === "active";
        setForeground(active);
        if (active) void read();
      };
      update();
      const listener = AppState.addEventListener("change", update);
      return () => { listener.remove(); setForeground(false); };
    }, [read]),
  );

  const self = key && devices ? thisDevice(devices, key) : undefined;
  const pending = !!self && deviceStatus(self, Date.now(), trust) === "pending";
  useEffect(() => {
    if ((!pending && !failure) || !foreground) return;
    const timer = setInterval(() => void read(), PENDING_POLL_MS);
    return () => clearInterval(timer);
  }, [pending, failure, foreground, read]);

  // Gateway vouches: a phone with no trusted row registers now, with no screen. Only Retry asks again:
  // a read does not clear a failed registration, so a refused one is not sent every few seconds.
  const register = key !== null && !!trust && needsGatewayRegistration(trust, self);
  const [refused, setRefused] = useState<string>();
  useEffect(() => {
    if (!register) return;
    let live = true;
    ensurePhoneDevice(core).then(
      (device) => {
        if (!live || current.current !== core) return;
        setKey(device.holder);
        setDirectory({ core, devices: device.list.devices, trust: device.trust });
        setRefused(undefined);
      },
      (error: unknown) => live && setRefused(message(error)),
    );
    return () => {
      live = false;
    };
  }, [core, register, attempt]);

  const pair = useCallback(
    async (fresh: boolean) => {
      setBusy(true);
      try {
        const made = fresh ? (await forgetDeviceKey(), await createDeviceKey()) : (key ?? (await createDeviceKey()));
        setKey(made);
        await core.registerE2eDevice({ name: DEVICE_NAME, encryptionKey: b64u(made.encryptionKey), signingKey: b64u(made.signingKey) });
        await read();
      } catch (error) {
        setFailure(message(error));
      } finally {
        setBusy(false);
      }
    },
    [core, key, read],
  );

  // One object while the key and the directory stay: the screens' effects key on it.
  const state = useMemo(() => readerState(key, devices, now, trust), [key, devices, now, trust]);
  const stopped = failure ?? (register ? refused : undefined);
  return {
    state: stopped && state.kind === "loading" ? { kind: "error", message: stopped } : state,
    trust,
    pair: () => pair(false),
    pairAgain: () => pair(true),
    refresh: () => {
      setRefused(undefined);
      setAttempt((n) => n + 1);
      void read();
    },
    busy,
  };
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type Ready = Extract<ReaderState, { kind: "ready" }>;

function threadKeys(core: Core, ready: Ready, threadId: string): ThreadKeys {
  return new ThreadKeys(threadId, ready.reader, ready.devices, gatewayKeyCopies(core, threadId, ready.reader.deviceId));
}

/**
 * The threads this phone holds a key copy for, every page of `GET /v1/devices/{id}/keys`. Read again when the list
 * shows a thread no read covered, so a session synced since does not read as locked.
 */
export function useHeldThreads(core: Core, reader: SyncedReader | undefined, threads: readonly SyncedThread[]): Set<string> | undefined {
  const [state, setState] = useState<{ core: Core; deviceId: string; held: Set<string>; asked: Set<string> }>();
  const deviceId = reader?.deviceId;
  const current = state?.core === core && state.deviceId === deviceId ? state : undefined;
  const unasked = threads.filter((thread) => thread.keyVersion !== null && !current?.held.has(thread.threadId) && !current?.asked.has(thread.threadId));
  const pending = current ? unasked.map((thread) => thread.threadId).join(",") : "first";
  useEffect(() => {
    if (!deviceId || !pending) return;
    let live = true;
    const asked = new Set(threads.map((thread) => thread.threadId));
    (async () => {
      const held = new Set<string>();
      let after: string | undefined;
      do {
        const page = await core.deviceThreadKeys(deviceId, { after, limit: 500 });
        for (const copy of page.keys) held.add(copy.threadId);
        after = page.nextAfter ?? undefined;
      } while (after && live);
      if (live) setState({ core, deviceId, held, asked });
    })().catch(() => live && setState({ core, deviceId, held: current?.held ?? new Set(), asked }));
    return () => {
      live = false;
    };
    // `threads` is read when a read starts; `pending` says when one starts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [core, deviceId, pending]);
  return current?.held;
}

/** Titles opened this launch, by thread and the last event they were read from: memory only. */
type TitleCache = Map<string, { lastSeq: number | null; value: TitleState }>;
const titleOwners = new WeakMap<Core, Map<string, TitleCache>>();
function titleCacheFor(core: Core, ready?: Ready) {
  let devices = titleOwners.get(core);
  if (!devices) { devices = new Map(); titleOwners.set(core, devices); }
  const identity = ready ? `${ready.reader.deviceId}:${b64u(ready.reader.holder.encryptionKey)}:${b64u(ready.reader.holder.signingKey)}` : "locked";
  let cache = devices.get(identity);
  if (!cache) { cache = new Map(); devices.set(identity, cache); }
  return cache;
}

/**
 * Opens the title of each thread `sealed` keeps, from its last event (every event carries it), a few at a time, newest
 * first. A title is read again only when its thread has a new last event.
 */
export function useTitles(core: Core, ready: Ready | undefined, threads: readonly SyncedThread[], sealed: (thread: SyncedThread) => boolean): Record<string, TitleState> {
  const titleCache = titleCacheFor(core, ready);
  const [opened, setOpened] = useState<{ owner: typeof titleCache; values: Record<string, TitleState> }>({ owner: titleCache, values: {} });
  const titles = opened.owner === titleCache ? opened.values : {};
  const wanted = threads.filter(sealed);
  const signature = wanted.map((thread) => `${thread.threadId}:${thread.lastSeq}`).join(",");
  useEffect(() => {
    if (!ready) return;
    let live = true;
    const queue = [...wanted];
    const work = async () => {
      for (let thread = queue.shift(); thread && live; thread = queue.shift()) {
        const cached = titleCache.get(thread.threadId);
        const value = cached?.lastSeq === thread.lastSeq ? cached.value : await openTitle(core, ready, thread);
        if (!live) return;
        if (value.state !== "failed") titleCache.set(thread.threadId, { lastSeq: thread.lastSeq, value });
        setOpened((current) => ({ owner: titleCache, values: { ...(current.owner === titleCache ? current.values : {}), [thread.threadId]: value } }));
      }
    };
    void Promise.all(Array.from({ length: TITLE_WORKERS }, work));
    return () => {
      live = false;
    };
    // `signature` names the threads and their last events; `wanted` comes from it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [core, ready, signature]);
  return titles;
}

async function openTitle(core: Core, ready: Ready, thread: SyncedThread): Promise<TitleState> {
  try {
    const page = await core.syncedEvents(thread.threadId, { before: (thread.lastSeq ?? 0) + 1, limit: 1 });
    const last = page.events.at(-1);
    if (!last) return { state: "unreadable" };
    const opened = await openSyncedEvent(threadKeys(core, ready, thread.threadId), last);
    const title = latestTitle([opened]);
    return title ? { state: "open", title, ...(waitsOnYou(opened) ? { waiting: true } : {}) } : { state: opened.state === "open" ? "unreadable" : opened.state };
  } catch {
    return { state: "failed" };
  }
}

/** Drops a deleted thread's title: nothing of it stays in memory. */
export function forgetTitle(core: Core, threadId: string): void {
  for (const cache of titleOwners.get(core)?.values() ?? []) cache.delete(threadId);
}

export interface ThreadView {
  thread?: SyncedThread;
  /** The events held, opened, in number order without a gap. */
  events: OpenedEvent[];
  /** Older events exist before the first held. */
  more: boolean;
  live: "connecting" | "live" | "reconnecting";
  deleted: boolean;
  missing: boolean;
  error?: string;
  loadingOlder: boolean;
  loadOlder: () => void;
  /**
   * This phone opens the thread's current key (its `keyVersion`). Commands need it; an unreadable last event does
   * not take it away. `undefined` while this phone checks.
   */
  keyHeld?: boolean;
  /** Loads again after a failed first load. */
  retry: () => void;
}

/**
 * One session: its last page, opened; earlier pages on request; then its live stream, reopened after a drop. The
 * follow restarts at once when the app returns to the foreground, so it never waits out the follow's backoff.
 */
export function useSyncedThread(core: Core, ready: Ready | undefined, threadId: string): ThreadView {
  const [thread, setThread] = useState<SyncedThread>();
  const [events, setEvents] = useState<OpenedEvent[]>([]);
  const [more, setMore] = useState(false);
  const [live, setLive] = useState<ThreadView["live"]>("connecting");
  const [deleted, setDeleted] = useState(false);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string>();
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [keyHeld, setKeyHeld] = useState<{ threadId: string; keyVersion: number; held: boolean }>();
  const [attempt, setAttempt] = useState(0);
  const keys = useRef<ThreadKeys | undefined>(undefined);
  const first = useRef<number | undefined>(undefined);

  // Back in the foreground: restart the follow now. The stream may have dropped while the app was away.
  useEffect(() => {
    let state = AppState.currentState;
    const listener = AppState.addEventListener("change", (next) => {
      if (next === "active" && state !== "active") setAttempt((n) => n + 1);
      state = next;
    });
    return () => listener.remove();
  }, []);

  useEffect(() => {
    if (!ready) return;
    const controller = new AbortController();
    const { signal } = controller;
    const these = threadKeys(core, ready, threadId);
    keys.current = these;
    setError(undefined);
    const feed = commandFeed(core, threadId);
    let release: (() => void) | undefined;
    const closeFeed = () => {
      release?.();
      release = undefined;
    };
    const open = (batch: Parameters<typeof openSyncedEvent>[1][]) => Promise.all(batch.map((event) => openSyncedEvent(these, event)));
    // Whether this phone holds the current key: the composer unlocks on it, not on the last event's state.
    const checkKey = (summary: SyncedThread) => {
      const version = summary.keyVersion;
      if (version === null || version <= 0) return setKeyHeld({ threadId, keyVersion: version ?? 0, held: false });
      these.at(version).then(
        () => !signal.aborted && setKeyHeld({ threadId, keyVersion: version, held: true }),
        (failure: unknown) => {
          // A failed read is not a missing key: keep what is known and ask again with the next summary.
          if (!signal.aborted && failure instanceof SyncedKeyError) setKeyHeld({ threadId, keyVersion: version, held: false });
        },
      );
    };
    const summarize = (summary: SyncedThread) => {
      setThread(summary);
      checkKey(summary);
    };
    const gone = () => {
      if (signal.aborted) return;
      forgetTitle(core, threadId);
      setEvents([]);
      setDeleted(true);
    };
    const fail = (failure: unknown) => {
      if (signal.aborted) return;
      if (failure instanceof CoreError && failure.status === 410) return gone();
      if (failure instanceof CoreError && failure.status === 404) return setMissing(true);
      setError(message(failure));
    };
    (async () => {
      const summary = await core.syncedThread(threadId);
      if (signal.aborted) return;
      summarize(summary);
      let last = -1;
      if (summary.lastSeq !== null) {
        const page = await core.syncedEvents(threadId, { before: summary.lastSeq + 1, limit: EVENTS_PAGE });
        const opened = await open(page.events);
        if (signal.aborted) return;
        first.current = page.events[0]?.seq;
        last = page.events.at(-1)?.seq ?? -1;
        setEvents(opened);
        setMore(page.more);
      }
      await followThread({
        after: last,
        stream: (after, onAdvance, streamSignal, onOpen, commands) => core.streamSyncedThread(threadId, after, onAdvance, streamSignal, onOpen, undefined, commands),
        // Phone control's receipts ride this stream: the session's control reads them from the feed.
        onCommandsLive: () => {
          if (!signal.aborted) release ??= feed.open();
        },
        onCommand: (command) => {
          if (!signal.aborted) feed.publish(command);
        },
        read: (after) => core.syncedEvents(threadId, { after, limit: EVENTS_PAGE }),
        onEvents: async (fresh) => {
          const opened = await open(fresh);
          if (signal.aborted) return;
          setEvents((held) => mergeEvents(held, opened));
          // Status, usage and cost are readable: the header's come from Gateway.
          core.syncedThread(threadId).then((next) => !signal.aborted && summarize(next), () => undefined);
        },
        onDeleted: gone,
        onState: (state) => {
          if (state === "reconnecting") closeFeed();
          if (!signal.aborted) setLive(state);
        },
        signal,
      });
    })().catch(fail).finally(closeFeed);
    return () => {
      controller.abort();
      closeFeed();
    };
  }, [core, ready, threadId, attempt]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  const loadOlder = useCallback(() => {
    const before = first.current;
    const these = keys.current;
    if (before === undefined || before <= 0 || !these || loadingOlder) return;
    setLoadingOlder(true);
    core
      .syncedEvents(threadId, { before, limit: EVENTS_PAGE })
      .then(async (page) => {
        const opened = await Promise.all(page.events.map((event) => openSyncedEvent(these, event)));
        if (keys.current !== these) return;
        first.current = page.events[0]?.seq ?? before;
        setEvents((held) => mergeEvents(opened, held));
        setMore(page.more);
      })
      .catch((failure: unknown) => setError(message(failure)))
      .finally(() => setLoadingOlder(false));
  }, [core, threadId, loadingOlder]);

  const known = keyHeld?.threadId === threadId && keyHeld.keyVersion === (thread?.keyVersion ?? 0) ? keyHeld.held : undefined;
  return { thread, events, more, live, deleted, missing, error, loadingOlder, loadOlder, keyHeld: known, retry };
}
