import type { RunReport, RunSummary } from "@nexus/kit";
import { useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { font, text, type Theme } from "@/theme";

/** How often a screen asks for news while a run is in progress, as Nexus UI's panels do. */
export const POLL_MS = 1500;

type Core = ReturnType<typeof useCore>;
/** Finished runs don't change: kept per signed-in client, so the run's screen and its pages share one load. */
const finished = new WeakMap<Core, Map<string, RunReport>>();

/** A run's report (its pages), polled while it runs. */
export function useRunReport(product: string, capability: string, id: string) {
  const core = useCore();
  const key = `${product}/${capability}/${id}`;
  const [report, setReport] = useState<RunReport | null>(() => finished.get(core)?.get(key) ?? null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const kept = finished.get(core)?.get(key);
    if (kept) return setReport(kept);
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = () =>
      core.moduleReport<RunReport>(product, capability, id).then(
        (r) => {
          if (!live) return;
          setReport(r);
          setError(null);
          if (r.status === "running") timer = setTimeout(load, POLL_MS);
          else finished.set(core, (finished.get(core) ?? new Map()).set(key, r));
        },
        (e: Error) => live && setError(e.message),
      );
    void load();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [core, key, product, capability, id, attempt]);

  return { report, error, reload: () => setAttempt((n) => n + 1) };
}

/** A run's status as a dot and its word, as Nexus UI's status pill. */
export function Status({ status, theme }: { status: RunSummary["status"]; theme: Theme }) {
  const color = status === "failed" ? theme.danger : status === "running" ? theme.primary : theme.muted;
  return (
    <View style={styles.status}>
      <View style={[styles.dot, { backgroundColor: color }]} />
      <Text style={[styles.statusText, { color }]}>{status === "succeeded" ? "Done" : status === "running" ? "Running" : "Failed"}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  status: { flexDirection: "row", alignItems: "center", gap: 6 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  statusText: { ...text.xs, fontFamily: font.medium },
});
