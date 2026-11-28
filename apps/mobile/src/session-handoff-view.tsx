import type { ComputerAgent, ComputerRouteChoices } from "@nexus/kit/computer-types";
import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import type { ComputerSession, ComputerState } from "@/computer-session";
import { RemoteChoice, SettingsAction, SettingsNote } from "@/remote-settings-ui";
import { HANDOFF_LIMIT } from "@/session-handoff";
import { font, text, useTheme } from "@/theme";

export function SessionHandoffView({ session, state, initialPrompt, awake, onOpen }: {
  session: ComputerSession;
  state: ComputerState;
  initialPrompt: string;
  awake: boolean;
  onOpen: (threadId: string) => void;
}) {
  const theme = useTheme();
  const [prompt, setPrompt] = useState(initialPrompt);
  const [agent, setAgent] = useState<ComputerAgent>();
  const [route, setRoute] = useState<Pick<ComputerRouteChoices, "via" | "payer">>();
  const options = state.handoff;
  const agents = options?.agents.filter((entry) => entry.routes.some((route) => route.state === "usable")) ?? [];
  const selected = agents.find((entry) => entry.agent === agent);
  const payment = selected?.routes.find((entry) => entry.state === "usable" && entry.via === route?.via && entry.payer === route.payer);
  const pending = state.pending;
  const busy = state.preparing || !!pending || !!state.created;
  const disabled = busy || !awake || !selected || !payment || !prompt.trim();

  return (
    <>
      {options ? <SettingsNote>{`The new session uses a separate worktree from the default branch in ${options.project.name}.`}</SettingsNote> : null}
      {agents.map((entry) => (
        <RemoteChoice key={entry.agent} label={entry.name} selected={agent === entry.agent} disabled={busy || !awake} onPress={() => {
          setAgent(entry.agent);
          setRoute(undefined);
        }} />
      ))}
      {selected ? (
        <View style={styles.group}>
          <Text style={[styles.label, { color: theme.foreground }]} accessibilityRole="header">Who pays</Text>
          {selected.routes.filter((entry) => entry.state === "usable").map((entry) => (
            <RemoteChoice key={`${entry.via}:${entry.payer}`} label={entry.label} detail={entry.detail}
              selected={payment === entry} disabled={busy || !awake} onPress={() => setRoute({ via: entry.via, payer: entry.payer })} />
          ))}
        </View>
      ) : null}
      {options && agents.length ? (
        <View style={styles.group}>
          <Text style={[styles.label, { color: theme.foreground }]} accessibilityRole="header">Handoff message</Text>
          <SettingsNote>Edit or remove any detail before you send. Recent messages are included as a starting point.</SettingsNote>
          <TextInput accessibilityLabel="Handoff message" multiline maxLength={HANDOFF_LIMIT} editable={!busy} value={prompt} onChangeText={setPrompt}
            placeholder="What should the next agent know?" placeholderTextColor={theme.muted}
            style={[styles.input, { backgroundColor: theme.fill, color: theme.foreground }]} />
          <Pressable disabled={disabled} accessibilityRole="button" accessibilityState={{ disabled }}
            style={[styles.send, { backgroundColor: theme.primary, opacity: disabled ? 0.4 : 1 }]} onPress={() => {
              if (!selected || !payment) return;
              haptic("send");
              void session.startHandoff(prompt, selected.agent, { via: payment.via, payer: payment.payer, model: payment.defaultModel, effort: null });
            }}>
            <Text style={[styles.sendText, { color: theme.onPrimary }]}>{pending?.kind === "startSession" || state.created ? "Starting session…" : "Send handoff"}</Text>
          </Pressable>
        </View>
      ) : null}
      {options && !agents.length ? <SettingsNote>No other agent on this computer has an available payment route.</SettingsNote> : null}
      {!awake ? <SettingsNote>Connect to the paired computer to continue.</SettingsNote> : null}
      {!options && !busy && awake ? <SettingsAction label="Load available agents" onPress={() => { haptic("tap"); void session.describeHandoff(); }} /> : null}
      {(pending && pending.state !== "uncertain") || state.created ? <SettingsAction label="Check request" onPress={() => { haptic("tap"); void session.retry(); }} /> : null}
      {state.created?.stalled ? <SettingsAction label="Open session" onPress={() => {
        const id = session.openAnyway();
        if (id) { haptic("tap"); onOpen(id); }
      }} /> : null}
    </>
  );
}

const styles = StyleSheet.create({
  group: { gap: 6, paddingTop: 8 },
  label: { ...text.secondary, fontFamily: font.medium, marginHorizontal: 16 },
  input: { ...text.base, fontFamily: font.regular, minHeight: 110, maxHeight: 164, borderRadius: 14, padding: 12, marginHorizontal: 16, textAlignVertical: "top" },
  send: { minHeight: 44, borderRadius: 16, paddingHorizontal: 16, justifyContent: "center", alignItems: "center", marginHorizontal: 16, marginVertical: 8 },
  sendText: { ...text.base, fontFamily: font.medium },
});
