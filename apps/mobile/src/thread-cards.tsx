import type { AgentClient, AgentErrand } from "@nexus/kit";
import { router } from "expo-router";
import { useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Reanimated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import type { ErrandCardView, SessionCardView } from "@/assistant-cards";
import { errandHref } from "@/drawer-places";
import { ErrandShape } from "@/errand-shape";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { open } from "@/markdown";
import { PULSE } from "@/motion";
import { font, type Theme } from "@/theme";

/**
 * The Assistant thread's live parts on the phone (A1·C b, A1·D, A1·5; web an internal pull request), drawn from the words in
 * `assistant-cards.ts`:
 *
 * - The step line: while Nexus thinks, the tool that runs now, such as "Searching Linear". It pulses, and holds still
 *   under Reduce Motion.
 * - Quick replies: at most three pills under the answer that asks. A tap sends the words as the person's message.
 * - The errand link card: under "I opened an errand for this.", the errand's state by shape, its name and one line.
 *   The whole card opens the errand's own thread.
 * - The session link card: the terminal tile, the title, the state by shape with a detail and the repository. A page of
 *   Nexus opens in the in-app browser; a desktop draft says to open it in Nexus for desktop.
 *
 * No action runs from a card, so each one is a link.
 */

/** The live step line while a tool runs (A1·C b), or "Thinking" without one. */
export function StepLine({ line, theme }: { line?: string; theme: Theme }) {
  const reduce = useReducedMotion();
  const opacity = useSharedValue(1);
  useEffect(() => {
    if (reduce) return;
    opacity.value = withRepeat(withTiming(PULSE.low, { duration: PULSE.half, easing: PULSE.easing }), -1, true);
    return () => cancelAnimation(opacity);
  }, [opacity, reduce]);
  const pulse = useAnimatedStyle(() => ({ opacity: opacity.value }));
  const words = line ?? "Thinking";
  return (
    <Reanimated.View style={[styles.step, pulse]} accessible accessibilityLabel={words} accessibilityLiveRegion="polite">
      <Text style={[styles.stepText, { color: theme.muted }]} numberOfLines={1}>
        {words}
      </Text>
    </Reanimated.View>
  );
}

/** Quick replies (A1·D): 32 pt pills with a hairline, in a row that wraps. */
export function QuickReplies({ replies, theme, onPick }: { replies: readonly string[]; theme: Theme; onPick: (reply: string) => void }) {
  return (
    <View style={styles.replies} accessibilityRole="toolbar" accessibilityLabel="Quick replies">
      {replies.map((reply) => (
        <Pressable
          key={reply}
          style={({ pressed }) => [styles.reply, { borderColor: theme.border, backgroundColor: pressed ? theme.fill : theme.card }]}
          onPress={() => {
            haptic("tap");
            onPick(reply);
          }}
          accessibilityRole="button"
          accessibilityHint="Sends it as your message"
          hitSlop={6}
        >
          <Text style={[styles.replyText, { color: theme.foreground }]}>{reply}</Text>
        </Pressable>
      ))}
    </View>
  );
}

/** The state of an errand in its tile: the dot or the turning arc, a check when done, a cross in red when it failed. */
function ErrandMark({ status, theme }: { status: AgentErrand["status"]; theme: Theme }) {
  if (status === "done") return <Icon name="check" size={12} color={theme.foreground} />;
  if (status === "failed") return <Icon name="close" size={12} color={theme.danger} />;
  return <ErrandShape status={status} color={theme.foreground} />;
}

/** The errand link card (A1·5): the whole card opens the errand's own thread. */
export function ErrandCard({ card, theme }: { card: ErrandCardView; theme: Theme }) {
  return (
    <Pressable
      style={({ pressed }) => [styles.card, { backgroundColor: pressed ? theme.fill : theme.card, borderColor: theme.border }]}
      onPress={() => {
        haptic("tap");
        router.push(errandHref(card.id));
      }}
      accessibilityRole="link"
      accessibilityLabel={card.label}
    >
      <View style={[styles.tile, { backgroundColor: theme.fill }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <ErrandMark status={card.status} theme={theme} />
      </View>
      <View style={styles.words}>
        <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1}>
          {card.name}
        </Text>
        <Text style={[styles.line, { color: card.status === "failed" ? theme.danger : theme.muted }]} numberOfLines={2}>
          {card.line}
        </Text>
      </View>
      <Icon name="forward" size={12} color={theme.tertiary} />
    </Pressable>
  );
}

/** A session's state by shape: a still ring while it waits to open, the arc while it runs, a check, a cross, a square. */
function SessionMark({ state, theme }: { state: SessionCardView["state"]; theme: Theme }) {
  if (state === "running") return <ErrandShape status="running" color={theme.muted} />;
  if (state === "done") return <Icon name="check" size={11} color={theme.muted} />;
  if (state === "failed") return <Icon name="close" size={11} color={theme.danger} />;
  if (state === "stopped") return <Icon name="stop" size={9} color={theme.muted} />;
  return <View style={[styles.ring, { borderColor: theme.muted }]} />;
}

/** The session link card (A1·5): Open when the phone can open it, else a card that says where to. */
export function SessionCard({ card, theme }: { card: SessionCardView; theme: Theme }) {
  const url = card.url;
  const body = (
    <>
      <View style={[styles.tile, { backgroundColor: theme.fill }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <Icon name="command" size={14} color={theme.foreground} />
      </View>
      <View style={styles.words}>
        <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1}>
          {card.title}
        </Text>
        <View style={styles.stateLine}>
          <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
            <SessionMark state={card.state} theme={theme} />
          </View>
          <Text style={[styles.line, styles.stateText, { color: theme.muted }]} numberOfLines={2}>
            {card.line}
            {card.repo ? <Text style={{ color: theme.tertiary }}> · {card.repo}</Text> : null}
          </Text>
        </View>
      </View>
      {url ? <Icon name="forward" size={12} color={theme.tertiary} /> : null}
    </>
  );
  if (!url) {
    return (
      <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]} accessible accessibilityLabel={card.label}>
        {body}
      </View>
    );
  }
  return (
    <Pressable
      style={({ pressed }) => [styles.card, { backgroundColor: pressed ? theme.fill : theme.card, borderColor: theme.border }]}
      onPress={() => {
        haptic("tap");
        open(url);
      }}
      accessibilityRole="link"
      accessibilityLabel={card.label}
      accessibilityHint="Opens it in Nexus"
    >
      {body}
    </Pressable>
  );
}

/**
 * The person's errands as they are now, by id, for the thread's errand cards. It reads only while `wanted` (the
 * thread has an errand card), and again when `revision` changes. A failure keeps the map as it was: each card then
 * shows the state its notice carried.
 */
export function useErrandStates(agent: AgentClient, wanted: boolean, revision: number): ReadonlyMap<string, AgentErrand> {
  const [states, setStates] = useState<ReadonlyMap<string, AgentErrand>>(new Map());
  useEffect(() => {
    if (!wanted) return;
    const controller = new AbortController();
    agent.errands(controller.signal).then(
      (read) => {
        if (controller.signal.aborted || read.kind !== "list") return;
        const rows = [...read.errands, ...(read.history ?? [])];
        if (rows.length) setStates(new Map(rows.map((row) => [row.id, row])));
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [agent, wanted, revision]);
  return states;
}

// The phone board's link card (A1·5, A2·A): 280 pt wide, 10 by 12 pt padding, 14 pt round, a hairline; a 30 pt tile
// 9 pt round, 10 pt from the words (15/20 over 12.5/17), and a chevron. A quick reply is a 32 pt pill with a hairline.
const styles = StyleSheet.create({
  step: { minHeight: 40, justifyContent: "center", paddingHorizontal: 4 },
  stepText: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  replies: { flexDirection: "row", flexWrap: "wrap", gap: 6, marginTop: 6 },
  reply: { minHeight: 32, justifyContent: "center", paddingHorizontal: 12, borderRadius: 16, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  replyText: { fontSize: 14, lineHeight: 18, fontFamily: font.regular },
  card: {
    width: 280,
    maxWidth: "100%",
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    paddingVertical: 10,
    paddingHorizontal: 12,
    borderRadius: 14,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
  },
  tile: { width: 30, height: 30, borderRadius: 9, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  words: { flex: 1, gap: 1 },
  title: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  line: { fontSize: 12.5, lineHeight: 17, fontFamily: font.regular },
  stateLine: { flexDirection: "row", alignItems: "center", gap: 5 },
  stateText: { flexShrink: 1 },
  ring: { width: 8, height: 8, borderRadius: 4, borderWidth: 1.5 },
});
