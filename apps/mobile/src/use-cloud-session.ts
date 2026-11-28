import { CoreError, type CloudHistoryEventPage, type CoreClient, type Session } from "@nexus/kit";
import { mergeCloudHistoryEvents } from "@nexus/kit/cloud-session-helpers";
import { useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { AppState } from "react-native";
import { cloudError } from "./cloud-sessions";

const PAGE_SIZE = 100;
const POLL_MS = 5_000;

export function useCloudSession(core: CoreClient, id: string) {
  const [session, setSession] = useState<Session>();
  const [error, setError] = useState("");
  const [page, setPage] = useState<CloudHistoryEventPage>();
  const [historyError, setHistoryError] = useState("");
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const request = useRef<AbortController | undefined>(undefined);
  const refreshNow = useRef<() => void>(() => undefined);

  const updateHistory = (next: CloudHistoryEventPage, earlier = false) => {
    setPage((held) => {
      const sameThread = held?.machineId === next.machineId && held.threadId === next.threadId;
      if (earlier && (!sameThread || !held.enabled)) return held;
      if (!next.enabled || !sameThread || !held.events.length) return next;
      if (next.firstSeq === null && !earlier) return next;
      const first = next.events[0]?.seq;
      const last = held.events.at(-1)?.seq;
      if (!earlier && first !== undefined && last !== undefined && first > last + 1) return next;
      return {
        ...(earlier ? held : next),
        events: mergeCloudHistoryEvents(held.events, next.events).filter((event) => next.firstSeq === null || event.seq >= next.firstSeq),
      };
    });
  };

  const read = useCallback(async (signal: AbortSignal) => {
    try {
      const session = await core.cloudSession(id, signal);
      if (signal.aborted) return;
      setSession(session);
      setError("");
      if (!session.machine?.id || !session.threadId) { setPage(undefined); return; }
      try {
        const latest = await core.cloudHistoryEvents(session.machine.id, session.threadId, { limit: PAGE_SIZE }, signal);
        if (!signal.aborted) { updateHistory(latest); setHistoryError(""); }
      } catch (error) {
        if (signal.aborted) return;
        if (error instanceof CoreError && (error.status === 403 || error.status === 404)) setPage(undefined);
        setHistoryError(cloudError(error));
      }
    } catch (error) {
      if (signal.aborted) return;
      if (error instanceof CoreError && (error.status === 403 || error.status === 404)) { setSession(undefined); setPage(undefined); }
      setError(cloudError(error));
    }
  }, [core, id]);

  const refresh = useCallback(() => refreshNow.current(), []);

  useFocusEffect(useCallback(() => {
    const controller = new AbortController();
    request.current = controller;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let running = false;
    const poll = async () => {
      if (running || controller.signal.aborted || AppState.currentState !== "active") return;
      clearTimeout(timer);
      running = true;
      try { await read(controller.signal); }
      finally {
        running = false;
        if (!controller.signal.aborted && AppState.currentState === "active") timer = setTimeout(() => void poll(), POLL_MS);
      }
    };
    refreshNow.current = () => { void poll(); };
    void poll();
    const listener = AppState.addEventListener("change", () => {
      if (AppState.currentState === "active") void poll();
      else clearTimeout(timer);
    });
    return () => { controller.abort(); clearTimeout(timer); listener.remove(); };
  }, [read]));

  const before = page?.events[0]?.seq;
  const more = before !== undefined && page?.firstSeq != null && before > page.firstSeq;
  const loadEarlier = async () => {
    const signal = request.current?.signal;
    if (!more || loadingEarlier || !page || !signal || signal.aborted) return;
    setLoadingEarlier(true);
    try {
      const older = await core.cloudHistoryEvents(page.machineId, page.threadId, { before, limit: PAGE_SIZE }, signal);
      if (!signal.aborted) { updateHistory(older, true); setHistoryError(""); }
    } catch (error) {
      if (!signal.aborted) {
        if (error instanceof CoreError && (error.status === 403 || error.status === 404)) setPage(undefined);
        setHistoryError(cloudError(error));
      }
    } finally { setLoadingEarlier(false); }
  };

  return { session, error, page, historyError, more, loadingEarlier, loadEarlier, refresh };
}
