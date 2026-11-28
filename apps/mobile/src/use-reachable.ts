import { useEffect, useRef } from "react";
import { AppState } from "react-native";
import { PROBE_MS } from "@/assistant-thread";

/**
 * While `offline`, tries `probe` every few seconds and each time the app comes back to the front, and calls `onBack`
 * once it answers. The app has no network-state module, so offline means a send failed at the network, and back means
 * the agent API answered again (any answer, even a refusal). `probe` throws when the network fails.
 */
export function useReachable(offline: boolean, probe: (signal: AbortSignal) => Promise<unknown>, onBack: () => void) {
  const latest = useRef({ probe, onBack });
  latest.current = { probe, onBack };

  useEffect(() => {
    if (!offline) return;
    let controller: AbortController | undefined;
    const attempt = () => {
      if (controller) return;
      const mine = new AbortController();
      controller = mine;
      latest.current.probe(mine.signal).then(
        () => {
          if (!mine.signal.aborted) latest.current.onBack();
        },
        () => undefined,
      ).finally(() => {
        if (controller === mine) controller = undefined;
      });
    };
    const timer = setInterval(attempt, PROBE_MS);
    const listener = AppState.addEventListener("change", (state) => state === "active" && attempt());
    return () => {
      clearInterval(timer);
      listener.remove();
      controller?.abort();
    };
  }, [offline]);
}
