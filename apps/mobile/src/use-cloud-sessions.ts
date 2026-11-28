import { CoreError, type CoreClient } from "@nexus/kit";
import { useCallback, useEffect, useRef, useState } from "react";
import { cloudError, type CloudSessions } from "./cloud-sessions";

/** GET /v1/sessions is the feature check. The disabled Gateway does not mount the route. */
export function useCloudSessions(core: CoreClient, enabled: boolean) {
  const [result, setResult] = useState<{ core: CoreClient; value: CloudSessions }>();
  const [loading, setLoading] = useState(false);
  const request = useRef<AbortController | undefined>(undefined);
  const refresh = useCallback(() => {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setLoading(true);
    void core.cloudSessions(controller.signal).then(
      ({ sessions }) => {
        if (!controller.signal.aborted) setResult({ core, value: { state: "ready", sessions } });
      },
      (error: unknown) => {
        if (controller.signal.aborted) return;
        const value: CloudSessions = error instanceof CoreError && error.status === 404
          ? { state: "disabled" }
          : { state: "error", error: cloudError(error) };
        setResult({ core, value });
      },
    ).finally(() => { if (!controller.signal.aborted) setLoading(false); });
  }, [core]);
  useEffect(() => {
    if (!enabled) return;
    refresh();
    return () => request.current?.abort();
  }, [enabled, refresh]);
  return { result: result?.core === core ? result.value : undefined, loading, refresh };
}
