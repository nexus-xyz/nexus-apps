import type { OrgSummary } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { AppState } from "react-native";
import { useCore, useStored } from "@/auth";
import { transientError } from "@/connection-errors";
import { orgWaitingOn } from "@/org-waiting-counts";
import { orderOrgs } from "@/orgs";
import { useReachable } from "@/use-reachable";

/** Your organizations, Personal first, with the last known list while Gateway is unreachable. */
export function useOrgs(load = true) {
  const core = useCore();
  const cache = useStored<OrgSummary[]>("cache", "orgs");
  const [snapshot, setSnapshot] = useState<{
    core: typeof core;
    orgs: OrgSummary[] | null;
    error: string | null;
    unreachable: boolean;
  }>();
  const [reload, setReload] = useState(0);
  const currentScope = snapshot?.core === core ? snapshot : undefined;
  const orgs = currentScope?.orgs ?? cache.load() ?? null;
  const error = currentScope?.error ?? null;
  const unreachable = currentScope?.unreachable ?? false;
  const refresh = useCallback(() => setReload((count) => count + 1), []);
  useReachable(load && unreachable, (signal) => core.me(signal), refresh);
  useEffect(() => {
    if (!orgWaitingOn || !load || unreachable) return;
    const listener = AppState.addEventListener("change", (state) => { if (state === "active") refresh(); });
    return () => listener.remove();
  }, [load, unreachable, refresh]);
  useEffect(() => {
    if (!load) return;
    const controller = new AbortController();
    void core.orgs(controller.signal).then(
      (response) => {
        if (controller.signal.aborted) return;
        const ordered = orderOrgs(response.orgs);
        setSnapshot({ core, orgs: ordered, error: null, unreachable: false });
        cache.save(ordered);
      },
      (error: unknown) => {
        if (controller.signal.aborted) return;
        const unreachable = transientError(error);
        if (!unreachable) cache.remove();
        setSnapshot({ core, orgs: unreachable ? cache.load() ?? null : null, error: error instanceof Error ? error.message : String(error), unreachable });
      },
    );
    return () => controller.abort();
  }, [core, cache, load, reload]);
  return { orgs, current: orgs?.find((org) => org.current), error, unreachable, refresh };
}
