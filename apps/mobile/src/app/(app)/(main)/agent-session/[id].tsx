import { agentLabel, CoreError, resumeCommand, sessionPayer, type AgentSessionSummary } from "@nexus/kit";
import { useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { elapsed, STATUS } from "@/agent-runs";
import { useCore, useStored } from "@/auth";
import { useCopy } from "@/command";
import { Blank, Footnote, Page, Row, Section } from "@/list";
import { usd } from "@/meta";
import { nameOf, useModels } from "@/models";
import { StatusMark } from "@/status-mark";
import { font, mono, text, useTheme } from "@/theme";
import { useNow } from "@/use-now";

const when = (iso: string) => new Date(iso).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
const count = (n: number) => n.toLocaleString();

/**
 * One coding agent's session, pushed from Agents (brand v5's P4 "One agent"): its mark and name, its model and where it
 * works under them, then what the Gateway knows of it (its status and clock, who pays, its team, the calls an agent on
 * a person's own plan reported) and the command that resumes it on the computer, a tap to copy. The boards' log and
 * composer need the agent's own events, which the Gateway does not keep; the phone does not steer an agent.
 */
export default function AgentSession() {
  const core = useCore();
  const theme = useTheme();
  const models = useModels(core);
  const { id } = useLocalSearchParams<{ id: string }>();
  const cache = useStored<AgentSessionSummary[]>("cache", "agent-sessions");
  const [session, setSession] = useState<AgentSessionSummary | undefined>(() => cache.load()?.find((s) => s.id === id));
  const [error, setError] = useState<string | null>(null);
  const [copied, copy] = useCopy();
  const now = useNow(session?.status === "running");

  useFocusEffect(
    useCallback(() => {
      const controller = new AbortController();
      setSession(cache.load()?.find((s) => s.id === id));
      setError(null);
      core.agentSession(id, controller.signal).then(
        (run) => {
          if (controller.signal.aborted) return;
          setSession(run);
          setError(null);
        },
        (e: Error) => {
          if (controller.signal.aborted) return;
          if (e instanceof CoreError && [401, 403, 404].includes(e.status)) {
            cache.save((cache.load() ?? []).filter((run) => run.id !== id));
            setSession(undefined);
          }
          setError(e.message);
        },
      );
      return () => controller.abort();
    }, [core, cache, id]),
  );

  if (!session || session.id !== id) return error ? <Blank title="Couldn't load this session" body={error} tone="danger" /> : <Blank title="Loading…" />;
  const resume = resumeCommand(session);
  const usage = session.via === "subscription" ? session.usage : undefined;
  return (
    <Page>
      <View style={styles.head} accessible accessibilityRole="header" accessibilityLabel={`${agentLabel(session.agent)}, ${STATUS[session.status]}`}>
        <View style={styles.titleRow}>
          <StatusMark status={session.status} size={14} />
          <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1}>
            {agentLabel(session.agent)}
          </Text>
        </View>
        <Text style={[styles.sub, { color: theme.tertiary }]} numberOfLines={2}>
          {[nameOf(models, session.model), session.cwd].filter(Boolean).join(" · ")}
        </Text>
      </View>
      {error ? <Footnote tone="danger">{error}</Footnote> : null}

      <Section>
        <Row label="Started by" detail={session.person} />
        <Row label="Status" value={STATUS[session.status]} valueTone={session.status === "failed" ? "danger" : undefined} />
        <Row label="Started" value={when(session.startedAt)} />
        {elapsed(session, now) ? <Row label={session.status === "running" ? "Running for" : "Took"} value={elapsed(session, now)} monoValue /> : null}
        {session.exitCode !== undefined ? <Row label="Exit code" value={String(session.exitCode)} monoValue /> : null}
        <Row label="Who pays" value={sessionPayer(session).label} detail={sessionPayer(session).title} />
        {session.team ? <Row label="Team" value={session.team} /> : null}
      </Section>

      {usage ? (
        <Section title="What it reported" footer="The agent's own count, on the person's plan. Gateway did not meter or charge it.">
          <Row label="Calls" value={count(usage.calls)} monoValue />
          <Row label="Tokens in" value={count(usage.inputTokens)} monoValue />
          <Row label="Tokens out" value={count(usage.outputTokens)} monoValue />
          <Row label="At list prices" value={`about ${usd(usage.estimatedCostUsd)}`} />
        </Section>
      ) : null}

      {resume ? (
        <Section title="Resume on your computer" footer="Tap to copy, then run it in a terminal.">
          <Row label={resume} monoLabel value={copied === resume ? "Copied" : undefined} valueTone="primary" onPress={() => copy(resume)} accessibilityHint="Copies the command" />
        </Section>
      ) : null}
    </Page>
  );
}

const styles = StyleSheet.create({
  head: { paddingHorizontal: 4, gap: 4 },
  titleRow: { flexDirection: "row", alignItems: "center", gap: 6, marginLeft: -4 },
  title: { fontFamily: font.medium, fontSize: 26, lineHeight: 32, letterSpacing: -0.4, flexShrink: 1 },
  sub: { ...text.secondary, fontFamily: mono, fontSize: 13 },
});
