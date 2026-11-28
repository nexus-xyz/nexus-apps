import type { CoreClient } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useMemo, useSyncExternalStore } from "react";
import { AppState } from "react-native";
import { codeControlSession, type CodeControlSession, type Slot } from "./code-control";
import type { ReaderState } from "./code-sessions";

export function useCodeControl(core: CoreClient, threadId: string, reader: Extract<ReaderState, { kind: "ready" }>["reader"]) {
  const { deviceId, holder } = reader;
  const session = useMemo(() => codeControlSession(core, threadId, { deviceId, holder }), [core, threadId, deviceId, holder]);
  const state = useSyncExternalStore(session.subscribe, session.snapshot, session.snapshot);
  useFocusEffect(useCallback(() => {
    const update = () => AppState.currentState === "active" ? session.start() : session.pause();
    update();
    const listener = AppState.addEventListener("change", update);
    return () => {
      listener.remove();
      session.pause();
    };
  }, [session]));
  return { ...state, viewKey: session.viewKey, canStopChecking: (slot: Slot) => session.canStopChecking(slot), stopChecking: (slot: Slot) => session.stopChecking(slot),
    submit: (...args: Parameters<CodeControlSession["submit"]>) => session.submit(...args), retry: (slot: Slot) => session.retry(slot),
    canCancel: (slot: Slot) => session.canCancel(slot), cancel: (slot: Slot) => session.cancel(slot), recheck: (slot: Slot) => session.recheck(slot),
    attachWaits: session.attachWaits };
}
