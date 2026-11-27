import { agentLabel, CoreError, type AgentSessionSummary, type SpendCall } from "@nexus/kit";
import { Stack, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { elapsed, folder, launcherList, STATUS } from "@/agent-runs";
import { rangeOf } from "@/analytics";
import { useCore } from "@/auth";
import { Empty, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { nameOf, useModels } from "@/models";
import { adoptionCounts, dayBefore, lastHour, latestRuns, people, perMinuteText, reachesIntoHour } from "@/observability";
import { useNow } from "@/use-now";

/** At most this many pages of 500 calls for the last hour; past it the count says "or more". */
const PAGES = 4;

/**
 * Observability, as brand v5's P27·D draws it on the phone: the title in the list, the last hour of the organization's
 * model calls (how many, a minute on average, and how many were refused or failed: owners and admins), then the coding
 * agents' latest runs, each opening its session (`agent-session/[id]`), then who ran them over the last 7 days, through
 * Nexus Gateway or on their own plans (adoption, which the boards moved here). The board's p50 and p95 are left out:
 * Gateway records no latency per call. Analytics keeps spend by team, person, model and product.
 */
export default function Observability() {
  const core = useCore();
  const models = useModels(core);
  const [hour, setHour] = useState<{ calls: SpendCall[]; more: boolean; at: number } | null>(null);
  // A member is refused the organization's calls: that section then says who sees them.
  const [hourError, setHourError] = useState<{ text: string; refused: boolean } | null>(null);
  const [runs, setRuns] = useState<AgentSessionSummary[] | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);
  const [adoption, setAdoption] = useState<{ nexus: number; own: number } | null>(null);
  const running = !!runs?.some((r) => r.status === "running");
  const now = useNow(running);

  const load = useCallback(() => {
    const at = Date.now();
    // Newest first, page by page, until a page ends before the hour did.
    (async () => {
      const calls: SpendCall[] = [];
      let cursor: string | undefined;
      for (let n = 0; n < PAGES; n++) {
        const page = await core.spendCalls({ from: dayBefore(at), limit: 500, ...(cursor ? { cursor } : {}) });
        calls.push(...page.calls);
        cursor = page.nextCursor ?? undefined;
        if (!cursor || !reachesIntoHour(page.calls, at)) return { calls, more: false, at };
      }
      return { calls, more: true, at };
    })().then(
      (r) => {
        setHour(r);
        setHourError(null);
      },
      (e: Error) => setHourError({ text: e.message, refused: e instanceof CoreError && e.status === 403 }),
    );
    core.agentSessions().then(
      (r) => {
        setRuns(r.sessions);
        setRunsError(null);
      },
      (e: Error) => setRunsError(e.message),
    );
    // Rust gateway only: an older one leaves adoption out.
    core.getAgentAdoption(rangeOf(7)).then((r) => setAdoption(adoptionCounts(r.days)), () => setAdoption(null));
  }, [core]);
  useFocusEffect(load);

  if (!hour && !hourError && !runs && !runsError) return <Loading alone />;
  const figures = hour ? lastHour(hour.calls, hour.at) : null;
  const shown = runs ? latestRuns(runs) : [];
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>Observability</LargeTitle>
      {figures ? (
        <Section title="Last hour" footer={hour?.more ? "More calls than the phone reads at once: the counts are the latest ones." : undefined}>
          <Row label="Calls" value={`${figures.calls}${hour?.more ? " or more" : ""}`} monoValue valueTone="foreground" />
          <Row label="Calls a minute" value={perMinuteText(figures.perMinute)} monoValue />
          <Row label="Refused" value={String(figures.refused)} monoValue valueTone={figures.refused ? "foreground" : undefined} />
          <Row label="Failed" value={String(figures.failed)} monoValue valueTone={figures.failed ? "danger" : undefined} />
        </Section>
      ) : hourError ? (
        <Section title="Last hour">
          {hourError.refused ? <Empty>Only owners and admins see the organization's calls.</Empty> : <Empty tone="danger">{hourError.text}</Empty>}
        </Section>
      ) : null}
      <Section title="Runs" footer="Coding agents' sessions, the running ones first.">
        {runsError ? <Empty tone="danger">{runsError}</Empty> : null}
        {runs && !shown.length ? <Empty>No coding agent has run yet. Start one on a computer with {launcherList()}.</Empty> : null}
        {shown.map((s) => (
          <Row
            key={s.id}
            label={folder(s.cwd) ?? agentLabel(s.agent)}
            detail={[`${agentLabel(s.agent)} · ${nameOf(models, s.model)}`, elapsed(s, now)].filter(Boolean).join(" · ")}
            value={STATUS[s.status]}
            valueTone={s.status === "failed" ? "danger" : s.status === "running" ? "foreground" : undefined}
            href={{ pathname: "/agent-session/[id]", params: { id: s.id } }}
          />
        ))}
      </Section>
      {adoption ? (
        <Section title="Coding agents · 7 days" footer="People who ran a coding agent, counted once each.">
          <Row label="Through Nexus Gateway" value={people(adoption.nexus)} />
          <Row label="On their own plans" value={people(adoption.own)} />
        </Section>
      ) : null}
    </Page>
  );
}
