import { CoreError, type CoreClient } from "@nexus/kit";
import { useEffect, useRef, useState } from "react";
import { AccessibilityInfo, ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import { requestNextStep } from "@/chat-next-step";
import { haptic } from "@/haptics";
import { pressedFill, ripple } from "@/press";
import { font, text, useTheme } from "@/theme";

type Step =
  | { kind: "idle" | "loading" | "absent" | "error" | "unavailable" | "dismissed" }
  | { kind: "ready"; text: string };

export function ChatNextStep({ core, sessionId, consent, onUse }: {
  core: CoreClient;
  sessionId: string;
  consent: (signal: AbortSignal) => Promise<boolean>;
  onUse: (suggestion: string) => void;
}) {
  const theme = useTheme();
  const [step, setStep] = useState<Step>({ kind: "idle" });
  const running = useRef<AbortController | null>(null);
  const close = () => {
    haptic("tap");
    running.current?.abort();
    setStep({ kind: "dismissed" });
  };
  useEffect(() => () => running.current?.abort(), []);

  const ask = async () => {
    if (running.current) return;
    const controller = new AbortController();
    running.current = controller;
    haptic("tap");
    setStep({ kind: "loading" });
    try {
      const result = await requestNextStep(
        () => consent(controller.signal),
        () => core.suggestChatMessage(sessionId, AbortSignal.any([controller.signal, AbortSignal.timeout(30_000)])),
        controller.signal,
      );
      if (controller.signal.aborted) return;
      setStep(result.kind === "cancelled" ? { kind: "idle" } : result);
      if (result.kind === "ready") {
        haptic("success");
        AccessibilityInfo.announceForAccessibility("Suggested next step ready.");
      }
    } catch (error) {
      if (controller.signal.aborted) return;
      const unavailable = error instanceof CoreError && (error.status === 404 || error.status === 501);
      setStep({ kind: unavailable ? "unavailable" : "error" });
    } finally {
      running.current = null;
    }
  };

  if (step.kind === "dismissed") return null;
  const message = step.kind === "ready" ? step.text
    : step.kind === "idle" ? "Generating a suggestion can incur a Chat charge."
      : step.kind === "error" ? "Couldn’t get a suggestion. Trying again can incur a Chat charge."
        : step.kind === "absent" ? "No suggestion was available."
          : step.kind === "unavailable" ? "Suggestions are unavailable for this chat." : undefined;
  return (
    <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]}>
      {step.kind === "ready" ? <Text style={[styles.heading, { color: theme.foreground }]} accessibilityRole="header">Suggested next step</Text> : null}
      {message ? <Text style={[styles.body, { color: step.kind === "ready" ? theme.foreground : theme.muted }]} accessibilityLiveRegion="polite">{message}</Text> : null}
      {step.kind === "loading" ? <View style={styles.progress}>
        <ActivityIndicator color={theme.muted} accessible={false} />
        <Text style={[styles.body, { color: theme.muted }]} accessibilityLiveRegion="polite">Generating suggestion…</Text>
      </View> : null}
      <View style={styles.actions}>
        {step.kind === "idle" || step.kind === "error" ? (
          <Pressable accessibilityRole="button" android_ripple={ripple(theme)}
            style={({ pressed }) => [styles.action, { backgroundColor: theme.fill }, pressed && pressedFill(theme)]} onPress={() => void ask()}>
            <Text style={[styles.label, { color: theme.foreground }]}>{step.kind === "error" ? "Try again" : "Suggest a next step"}</Text>
          </Pressable>
        ) : null}
        {step.kind === "ready" ? <Pressable accessibilityRole="button" android_ripple={ripple(theme)}
          style={({ pressed }) => [styles.action, { backgroundColor: theme.fill }, pressed && pressedFill(theme)]} onPress={() => {
            close();
            onUse(step.text);
          }}>
          <Text style={[styles.label, { color: theme.foreground }]}>Add to draft</Text>
        </Pressable> : null}
        <Pressable accessibilityRole="button" android_ripple={ripple(theme)} style={({ pressed }) => [styles.action, pressed && pressedFill(theme)]} onPress={close}>
          <Text style={[styles.label, { color: theme.muted }]}>{step.kind === "loading" ? "Cancel" : "Not now"}</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: { borderWidth: StyleSheet.hairlineWidth, borderRadius: 16, marginTop: 8, padding: 12, gap: 8 },
  heading: { ...text.secondary, fontFamily: font.medium },
  body: { ...text.secondary, fontFamily: font.regular },
  progress: { flexDirection: "row", alignItems: "center", gap: 8 },
  actions: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 6 },
  action: { minHeight: 48, borderRadius: 12, paddingHorizontal: 12, justifyContent: "center" },
  label: { ...text.secondary, fontFamily: font.medium, flexShrink: 1 },
});
