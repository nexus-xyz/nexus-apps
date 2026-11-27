import type { OrgWaitingCounts } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { useCore, useStored } from "@/auth";
import { transientError } from "@/connection-errors";
import { useReachable } from "@/use-reachable";

export const orgWaitingOn = process.env.EXPO_PUBLIC_NEXUS_ORG_WAITING === "on";

type WaitingCounts =
  | { kind: "idle" }
  | { kind: "checking"; snapshot?: OrgWaitingCounts }
  | { kind: "ready"; snapshot: OrgWaitingCounts }
  | { kind: "offline"; snapshot?: OrgWaitingCounts }
  | { kind: "failed" };

export function useOrgWaitingCounts(visible: boolean): WaitingCounts {
  const core = useCore();
  const cache = useStored<OrgWaitingCounts>("cache", "org-waiting-counts");
  const [read, setRead] = useState<{ core: typeof core; counts: WaitingCounts }>();
  const [revision, setRevision] = useState(0);
  const counts: WaitingCounts = !orgWaitingOn ? { kind: "idle" } :
    read?.core === core ? read.counts : { kind: "checking", snapshot: cache.load() };
  const refresh = useCallback(() => setRevision((current) => current + 1), []);
  useReachable(visible && counts.kind === "offline", (signal) => core.me(signal), refresh);
  useEffect(() => {
    if (!orgWaitingOn || !visible) return;
    const controller = new AbortController();
    setRead({ core, counts: { kind: "checking", snapshot: cache.load() } });
    void core.orgWaitingCounts(controller.signal).then((snapshot) => {
      if (controller.signal.aborted) return;
      cache.save(snapshot);
      setRead({ core, counts: { kind: "ready", snapshot } });
    }, (error: unknown) => {
      if (controller.signal.aborted) return;
      if (transientError(error)) {
        setRead({ core, counts: { kind: "offline", snapshot: cache.load() } });
      } else {
        cache.remove();
        setRead({ core, counts: { kind: "failed" } });
      }
    });
    return () => controller.abort();
  }, [cache, core, revision, visible]);
  return counts;
}

export function waitingLine(waiting: WaitingCounts, id: string): { detail?: string; dot: boolean } {
  if (waiting.kind === "idle") return { dot: false };
  if (waiting.kind === "failed") return { detail: "Couldn’t check what waits", dot: false };
  const count = waiting.snapshot?.counts[id] ?? 0;
  if (!count) {
    return {
      detail: waiting.kind === "checking" && !waiting.snapshot ? "Checking…" :
        waiting.kind === "offline" && !waiting.snapshot ? "Needs a connection" : undefined,
      dot: false,
    };
  }
  const total = count === 1 ? "1 item needs you" : `${count} items need you`;
  if (waiting.kind === "ready") return { detail: total, dot: true };
  const time = waiting.snapshot ? new Date(waiting.snapshot.checkedAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : undefined;
  return { detail: time ? `${total} as of ${time}` : total, dot: true };
}
