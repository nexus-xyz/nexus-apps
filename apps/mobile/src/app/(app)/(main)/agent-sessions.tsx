import { agentLabel, type AgentSessionSummary } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { StyleSheet, Text } from "react-native";
import { elapsed, launcherList, runLine, runsLine, splitRuns } from "@/agent-runs";
import { useCore, useStored } from "@/auth";
import { Empty, Footnote, LargeTitle, Page, Row, Section, Segmented } from "@/list";
import { nameOf, useModels } from "@/models";
import { StatusMark } from "@/status-mark";
import { font, text, useTheme } from "@/theme";
import { useNow } from "@/use-now";

/** How often the list asks again while it shows. */
const POLL_MS = 10_000;

/**
 * Agents, as brand v5's phone draws them (P18, P4): readable coding agent sessions, Running or Done, with each owner,
 * live mark, its model, what it is doing and where, and its clock. A row opens the session (`agent-session/[id]`).
 * While one runs, the list asks again every 10 s and the clocks tick. Starting or steering an agent stays on the
 * computer (`nexus claude`, `nexus codex`, `nexus agy`); Coding agents sets their routes.
 */
export default function AgentSessions() {
  const core = useCore();
  const theme = useTheme();
  const models = useModels(core);
  // As it was last time, until the Gateway answers; the session's own page reads it too.
  const cache = useStored<AgentSessionSummary[]>("cache", "agent-sessions");
  const [sessions, setSessions] = useState<AgentSessionSummary[] | null>(() => cache.load() ?? null);
  // Only a pull shows the spinner; the poll asks quietly.
  const [pulling, setPulling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { running, done } = splitRuns(sessions ?? []);
  // Running when any agent runs, else Done: from the cache, then from the first answer, until you pick one.
  const [tab, setTab] = useState<"running" | "done">(() => (splitRuns(cache.load() ?? []).running.length ? "running" : "done"));
  const picked = useRef(false);
  const focusVersion = useRef(0);
  const now = useNow(running.length > 0);

  const load = useCallback(() => {
    const version = focusVersion.current;
    return core
      .agentSessions()
      .then((r) => {
        if (version !== focusVersion.current) return;
        setSessions(r.sessions);
        cache.save(r.sessions);
        if (!picked.current) setTab(splitRuns(r.sessions).running.length ? "running" : "done");
        setError(null);
      })
      .catch((e: Error) => {
        if (version === focusVersion.current) setError(e.message);
      });
  }, [core, cache]);
  const pull = () => {
    setPulling(true);
    void load().finally(() => setPulling(false));
  };
  useFocusEffect(
    useCallback(() => {
      setSessions(cache.load() ?? null);
      void load();
      const timer = setInterval(() => void load(), POLL_MS);
      return () => {
        focusVersion.current += 1;
        clearInterval(timer);
      };
    }, [load, cache]),
  );

  const shown = tab === "running" ? running : done;
  return (
    <Page onRefresh={pull} refreshing={pulling}>
      <LargeTitle>Agents</LargeTitle>
      {sessions ? <Text style={[styles.line, { color: theme.muted }]}>{runsLine(sessions)}</Text> : null}
      <Footnote>Agents you can view in this organization.</Footnote>
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      <Segmented
        label="Sessions"
        options={[
          { id: "running", label: `Running · ${running.length}` },
          { id: "done", label: `Done · ${done.length}` },
        ]}
        value={tab}
        onChange={(next) => {
          picked.current = true;
          setTab(next);
        }}
      />
      <Section>
        {sessions && !shown.length ? (
          <Empty>{tab === "running" ? `No agent is running. Start one on your computer with ${launcherList()}.` : "No finished sessions yet."}</Empty>
        ) : null}
        {shown.map((s) => {
          const agent = agentLabel(s.agent);
          const model = nameOf(models, s.model);
          const detail = `Started by ${s.person} · ${runLine(s)}`;
          const duration = elapsed(s, now);
          return (
            <Row
              key={s.id}
              leading={<StatusMark status={s.status} />}
              label={
                <Text style={[styles.name, { color: theme.foreground }]} numberOfLines={1}>
                  {agent}
                  <Text style={[styles.model, { color: theme.tertiary }]}> · {model}</Text>
                </Text>
              }
              detail={detail}
              value={duration}
              monoValue
              href={{ pathname: "/agent-session/[id]", params: { id: s.id } }}
              accessibilityLabel={`${agent}, ${model}, ${detail}, ${duration}`}
            />
          );
        })}
      </Section>
      <Section>
        <Row icon="agents" label="Code" detail="Your synced sessions, read live on this phone" href="/code" />
        <Row icon="agents" label="Coding agents" detail="Routes and setup on your computer" href="/agents" />
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  line: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 16, marginTop: -12 },
  name: { ...text.body, fontFamily: font.medium },
  model: { ...text.secondary, fontFamily: font.regular },
});
