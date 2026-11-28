import { pausedLine, type AgentClient, type AgentPauseView } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { haptic } from "@/haptics";
import { clock } from "@/privacy-words";
import { font, text, type Theme } from "@/theme";

/**
 * The paused bar in the Assistant (A11·b): while the person paused it, "Paused at 14:02 · nothing runs until
 * you resume" and Resume. It reads the pause each time `revision` moves. An agent API without the pause, or a failed
 * read, draws nothing, so the chat never waits on it.
 */
export function PausedBar({ agent, revision, theme }: { agent: AgentClient; revision: number; theme: Theme }) {
  const [pause, setPause] = useState<AgentPauseView | undefined>();
  const [working, setWorking] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    agent.pause(controller.signal).then(
      (call) => !controller.signal.aborted && setPause(call.kind === "ok" ? call.value : undefined),
      () => undefined,
    );
    return () => controller.abort();
  }, [agent, revision]);

  if (!pause?.paused) return null;
  const resume = () => {
    haptic("tap");
    setWorking(true);
    agent.setPaused(false).then(
      (call) => {
        setWorking(false);
        if (call.kind === "ok") setPause(call.value);
      },
      () => setWorking(false),
    );
  };
  return (
    <View style={[styles.bar, { backgroundColor: theme.fill }]} accessibilityRole="alert">
      <Text style={[styles.line, { color: theme.foreground }]}>{pausedLine(pause, clock)}</Text>
      <Pressable
        onPress={resume}
        disabled={working}
        style={[styles.resume, { backgroundColor: theme.primary }, working && styles.idle]}
        accessibilityRole="button"
        accessibilityState={{ disabled: working }}
      >
        <Text style={[styles.resumeText, { color: theme.onPrimary }]}>Resume</Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create({
  bar: { flexDirection: "row", alignItems: "center", gap: 12, marginBottom: 12, paddingVertical: 10, paddingHorizontal: 14, borderRadius: 16, borderCurve: "continuous" },
  line: { ...text.sm, fontFamily: font.regular, flex: 1 },
  resume: { minHeight: 36, paddingHorizontal: 14, borderRadius: 18, alignItems: "center", justifyContent: "center" },
  resumeText: { ...text.sm, fontFamily: font.medium },
  idle: { opacity: 0.4 },
});
