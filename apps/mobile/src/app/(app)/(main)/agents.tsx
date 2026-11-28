import {
  offlineAgentAccess,
  payerLabel,
  routeOf,
  setupSteps,
  type AgentAccessEntry,
  type AgentAccessResponse,
  type AgentRoute,
  type AgentSessionSummary,
} from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { ActivityIndicator, Alert, Share, StyleSheet, Text, View } from "react-native";
import { runsLine } from "@/agent-runs";
import { agentSetupAvailable, DESKTOP_AGENT_SETUP_LINK, phoneSteps } from "@/agent-steps";
import { useCore } from "@/auth";
import { haptic } from "@/haptics";
import { Footnote, Inset, Page, Row, Section, Segmented } from "@/list";
import { font, text, useTheme, type Theme } from "@/theme";
import { CustomizeTabs, useCustomize } from "@/customize-tabs";

/**
 * Coding agents, as desktop and Nexus UI show them (`agent-access.tsx`, `agent-sessions.tsx`): for Claude Code and
 * Codex, the route each takes (through Gateway, or on your own plan), who pays on it, and what an owner or admin still
 * has to set up; then a row to your agent sessions (Agents). The route is saved for every client. The phone shows no
 * terminal commands: an agent is installed and signed in on the computer. Running an agent, and steering one from
 * here, are out of this screen's scope.
 */
export default function Agents() {
  const customize = useCustomize();
  const core = useCore();
  const theme = useTheme();
  const request = useRef(0);
  const [data, setData] = useState<{
    core: typeof core;
    access: AgentAccessResponse;
    verified: boolean;
    sessions?: AgentSessionSummary[];
    error?: string;
  }>();
  const [loading, setLoading] = useState(true);
  const visible = data?.core === core ? data : undefined;
  const access = visible?.access;
  const accessVerified = !loading && visible?.verified === true;
  const load = useCallback(() => {
    const id = ++request.current;
    setLoading(true);
    void Promise.allSettled([core.agentAccess(), core.agentSessions()]).then(([accessResult, sessionsResult]) => {
      if (request.current !== id) return;
      const verified = accessResult.status === "fulfilled" && accessResult.value != null;
      setData({
        core,
        access: accessResult.status === "fulfilled" ? accessResult.value ?? offlineAgentAccess() : offlineAgentAccess(),
        verified,
        sessions: sessionsResult.status === "fulfilled" ? sessionsResult.value.sessions : undefined,
        error: sessionsResult.status === "rejected" && sessionsResult.reason instanceof Error ? sessionsResult.reason.message : undefined,
      });
      setLoading(false);
    });
  }, [core]);
  useFocusEffect(useCallback(() => {
    load();
    return () => { request.current++; };
  }, [load]));

  const choose = (entry: AgentAccessEntry, route: AgentRoute) => {
    if (route === entry.route || !accessVerified || !visible || !agentSetupAvailable(entry, route, visible.access.agentSubscriptions)) return;
    const id = ++request.current;
    setLoading(true);
    void core.setAgentAccess({ [entry.agent]: route }).then(
      (response) => {
        if (request.current !== id) return;
        setData({ ...visible, access: response, error: undefined });
        setLoading(false);
      },
      (error: unknown) => {
        if (request.current !== id) return;
        setData({ ...visible, error: error instanceof Error ? error.message : "Couldn’t update the billing route." });
        setLoading(false);
      },
    );
  };

  return (
    <Page onRefresh={load} refreshing={loading && !!access}>
      {customize ? <CustomizeTabs active="/agents" /> : null}
      {visible?.error ? <Footnote tone="danger">{visible.error}</Footnote> : null}
      {loading && !access ? <ActivityIndicator color={theme.muted} accessibilityLabel="Checking agent access" /> : null}
      {!loading && !accessVerified ? <Footnote>Agent access is unavailable. Pull to refresh.</Footnote> : null}
      {access?.agents.map((entry) => (
        <AgentCard
          key={entry.agent}
          entry={entry}
          subscriptions={access.agentSubscriptions}
          verified={accessVerified}
          onChoose={(route) => choose(entry, route)}
          theme={theme}
        />
      ))}
      {visible?.sessions ? <Sessions sessions={visible.sessions} /> : null}
    </Page>
  );
}

/** An agent: its route, who pays, and what an owner or admin still has to do. */
function AgentCard({ entry, subscriptions, verified, onChoose, theme }: { entry: AgentAccessEntry; subscriptions: boolean; verified: boolean; onChoose: (route: AgentRoute) => void; theme: Theme }) {
  const steps = verified ? phoneSteps(setupSteps(entry, entry.route, { agentSubscriptions: subscriptions })) : [];
  const plan = routeOf(entry, "subscription")?.plan ?? "own";
  const canSetUp = verified && agentSetupAvailable(entry, entry.route, subscriptions);
  const routes: { id: AgentRoute; label: string }[] = [
    { id: "gateway", label: "Through Gateway" },
    { id: "subscription", label: `Your ${plan} plan` },
  ];
  const offered = routes.filter(({ id }) => id === entry.route || agentSetupAvailable(entry, id, subscriptions));
  const shareSetup = () => {
    haptic("tap");
    void Share.share({ message: DESKTOP_AGENT_SETUP_LINK }).catch(() => {
      Alert.alert("Couldn’t share the link", "On your computer, open Nexus → Settings → Coding agents.");
    });
  };
  return (
    <Section
      title={entry.name}
      footer={verified ? `Pays: ${payerLabel(routeOf(entry, entry.route))}${entry.from === "organization" ? " · set by your organization" : entry.from === "default" ? " · the default" : ""}.` : undefined}
    >
      {verified && offered.length > 1 ? (
        <Inset><Segmented label={`${entry.name} route`} options={offered} value={entry.route} onChange={onChoose} /></Inset>
      ) : verified ? <Row label="Billing route" value={offered[0]?.label} /> : null}
      {steps.map((step) => (
        <Row
          key={step.id}
          leading={<View style={[styles.dot, { borderColor: theme.border }]} accessibilityLabel="To do" />}
          label={<Text style={[styles.step, { color: theme.foreground }]}>{step.text}</Text>}
        />
      ))}
      {canSetUp ? <Row label="Share desktop setup link" detail="On your computer, open Nexus → Settings → Coding agents to check installation and sign-in." onPress={shareSetup} tone="primary" /> : null}
    </Section>
  );
}

/** The sessions, one row to Agents, where they run live: how many there are, and how many run now. */
function Sessions({ sessions }: { sessions: AgentSessionSummary[] }) {
  return (
    <Section title="Sessions">
      <Row icon="agents" label="Agents" value={runsLine(sessions)} href="/agent-sessions" />
    </Section>
  );
}

const styles = StyleSheet.create({
  dot: { width: 18, height: 18, borderRadius: 9, borderWidth: 1.5 },
  step: { ...text.base, fontFamily: font.regular },
});
