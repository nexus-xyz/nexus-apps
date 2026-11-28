import { CoreError, type CoreClient, type SessionPinList, type SessionPinSource } from "@nexus/kit";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AppState } from "react-native";
import { useStored } from "./auth";
import { acceptPin, migrateLocalPins, pinKey, queuePin, visiblePinKeys, type PinSnapshot } from "./session-pin-state";
import type { UnifiedSession } from "./session-history";
import { useToast } from "./toast";

type PinView = { snapshot?: PinSnapshot; rows: UnifiedSession[] };
type PinOwner = {
  view: PinView;
  request?: AbortController;
  openViews: number;
  listeners: Set<() => void>;
};
const owners = new WeakMap<CoreClient, Map<string, PinOwner>>();
const emptyOwner = (): PinOwner => ({ view: { rows: [] }, openViews: 0, listeners: new Set() });

function pinOwner(core: CoreClient, userId: string, load: () => PinSnapshot | undefined): PinOwner {
  const accounts = owners.get(core) ?? new Map<string, PinOwner>();
  owners.set(core, accounts);
  const existing = accounts.get(userId);
  if (existing) return existing;
  const owner = emptyOwner();
  owner.view.snapshot = load();
  accounts.set(userId, owner);
  return owner;
}

function pinnedRows({ chats, threads }: SessionPinList): UnifiedSession[] {
  return [
    ...chats.map((chat): UnifiedSession => ({ source: "chat", id: chat.id, updatedAt: chat.updatedAt, chat, origin: { kind: "chat" } })),
    ...threads.map((thread): UnifiedSession => ({ source: "synced", id: thread.threadId, updatedAt: thread.updatedAt, thread, origin: { kind: "unknown" } })),
  ];
}

export function useSessionPins(core: CoreClient, opened: boolean, local: readonly string[]) {
  const [account, setAccount] = useState<{ core: CoreClient; userId: string }>();
  const userId = account?.core === core ? account.userId : undefined;
  const storage = useStored<PinSnapshot>("choice", `synced-session-pins:${userId ?? ""}`);
  useEffect(() => {
    if (!opened || userId) return;
    let active = true;
    void core.me().then(({ user }) => { if (active && user) setAccount({ core, userId: user.id }); }, () => undefined);
    return () => { active = false; };
  }, [core, opened, userId]);
  const toast = useToast();
  const localPins = useRef(local);
  localPins.current = local;
  const owner = useMemo(() => userId ? pinOwner(core, userId, storage.load) : emptyOwner(), [core, storage, userId]);
  const current = useRef(owner);
  current.current = owner;
  const subscribe = useCallback((listener: () => void) => {
    owner.listeners.add(listener);
    return () => { owner.listeners.delete(listener); };
  }, [owner]);
  const getSnapshot = useCallback(() => owner.view, [owner]);
  const { snapshot, rows } = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const persist = useCallback((next: PinSnapshot, rows = owner.view.rows) => {
    if (current.current !== owner) return;
    owner.view = { snapshot: next, rows };
    storage.save(next);
    for (const listener of owner.listeners) listener();
  }, [owner, storage]);
  const sync = useCallback(async () => {
    if (!userId || current.current !== owner || owner.request || AppState.currentState !== "active") return;
    const controller = new AbortController();
    owner.request = controller;
    try {
      const listing = await core.sessionPins(controller.signal);
      const { pins } = listing;
      if (current.current !== owner || controller.signal.aborted) return;
      persist(owner.view.snapshot ? { ...owner.view.snapshot, pins } : migrateLocalPins(pins, localPins.current), pinnedRows(listing));
      while (current.current === owner && !controller.signal.aborted) {
        const change = owner.view.snapshot?.pending[0];
        if (!change) return;
        try {
          const { source, sessionId, pinned, expectedVersion } = change;
          const pin = await core.setSessionPin(source, sessionId, { pinned, expectedVersion }, controller.signal);
          if (current.current !== owner || controller.signal.aborted || !owner.view.snapshot) return;
          persist(acceptPin(owner.view.snapshot, change, pin));
        } catch (error) {
          if (current.current !== owner || controller.signal.aborted || !owner.view.snapshot) return;
          if (!(error instanceof CoreError)
            || (error.status !== 400 && !["session_pin_changed", "pin_session_not_found"].includes(error.code ?? ""))) return;
          const pending = owner.view.snapshot.pending.filter((item) => item !== change);
          const pins = owner.view.snapshot.pins.filter((pin) => pinKey(pin) !== pinKey(change));
          persist({ pins, pending });
          const fresh = await core.sessionPins(controller.signal);
          if (current.current !== owner || controller.signal.aborted || !owner.view.snapshot) return;
          persist({ ...owner.view.snapshot, pins: fresh.pins }, pinnedRows(fresh));
          toast({ text: error.code === "session_pin_changed" ? "The pin changed on another device." : "This session is no longer available.", haptic: "error" });
        }
      }
    } catch {
      // The selected local layout also works on Gateways without the pin route.
    } finally {
      if (owner.request === controller) owner.request = undefined;
    }
  }, [core, owner, persist, toast, userId]);
  useEffect(() => {
    if (!opened || !userId) return;
    owner.openViews += 1;
    void sync();
    const timer = setInterval(() => void sync(), 10_000);
    const foreground = AppState.addEventListener("change", (state) => { if (state === "active") void sync(); });
    return () => {
      owner.openViews -= 1;
      clearInterval(timer);
      foreground.remove();
      if (!owner.openViews) { owner.request?.abort(); owner.request = undefined; }
    };
  }, [opened, owner, sync, userId]);

  const set = (source: SessionPinSource, id: string, pinned: boolean): boolean => {
    if (!owner.view.snapshot) return false;
    persist(queuePin(owner.view.snapshot, source, id, pinned));
    void sync();
    return true;
  };
  return { userId, available: !!snapshot, keys: snapshot ? visiblePinKeys(snapshot) : local, set, rows };
}
