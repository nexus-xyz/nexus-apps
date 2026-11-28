import type { AgentErrand, AgentMessage } from "@nexus/kit";
import type { ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Reanimated, { useReducedMotion, withSpring, type EntryAnimationsValues } from "react-native-reanimated";
import { AgentReply } from "@/agent-reply";
import type { DecideWrite, EditWrite } from "@/approval-card";
import { OFFLINE_LINE } from "@/assistant-thread";
import { Icon } from "@/icon";
import { Markdown } from "@/markdown";
import { BUBBLE, spring } from "@/motion";
import { NexusMark } from "@/nexus-mark";
import { font, mono, text, type Theme } from "@/theme";
import { QuickReplies, StepLine } from "@/thread-cards";
import { useAssistantName } from "@/use-assistant-name";

/**
 * The Assistant's thread as the Session canon's transcript (ASSISTANT.md, "How the thread looks"; the Assistant
 * canvas's phone boards). Nexus writes plain, full-width text; your turns are the right-side bubble, as in Chat. No
 * avatar on each message and no Read receipts. A message Nexus starts on its own (the brief) carries a small
 * "Nexus · 08:00" header. The profile header opens the thread, and a day marker starts each day. Texting is how Nexus
 * behaves, not how the thread looks.
 *
 * A message you send rises from the composer, and Nexus's words fade in. While Nexus works, a pulsing line says what
 * runs ("Thinking", "Searching Linear"). Reduce Motion keeps all of it in place.
 */

const RISE = spring(BUBBLE.rise);
const ARRIVE = spring(BUBBLE.arrive);

/** Yours: up from the composer, with no bounce. */
function riseIn(_values: EntryAnimationsValues) {
  "worklet";
  return {
    initialValues: { opacity: 0, transform: [{ translateY: BUBBLE.rise.from }] },
    animations: { opacity: withSpring(1, RISE), transform: [{ translateY: withSpring(0, RISE) }] },
  };
}

/** Nexus's: its words fade in where they stand. */
function arriveIn(_values: EntryAnimationsValues) {
  "worklet";
  return { initialValues: { opacity: 0 }, animations: { opacity: withSpring(1, ARRIVE) } };
}

/**
 * The profile header (the canvas's "Empty state"): Nexus's mark, large and unframed, its name, and where it lives, as
 * Messages shows a contact. It opens every thread and stays as the start of its history.
 */
export function ProfileHeader({ org, theme }: { org?: string; theme: Theme }) {
  const assistant = useAssistantName();
  const where = org ? `Your assistant in ${org}` : "Your assistant";
  return (
    <View style={styles.profile} accessible accessibilityRole="header" accessibilityLabel={`${assistant}. ${where}`}>
      <NexusMark size={56} />
      <Text style={[styles.profileName, { color: theme.foreground }]}>{assistant}</Text>
      <Text style={[styles.profileWhere, { color: theme.muted }]}>{where}</Text>
    </View>
  );
}

/** A day marker: the day's name between two hairlines. */
export function DayMarker({ label, theme }: { label: string; theme: Theme }) {
  return (
    <View style={styles.day} accessible accessibilityRole="header" accessibilityLabel={label}>
      <View style={[styles.rule, { backgroundColor: theme.border }]} />
      <Text style={[styles.dayText, { color: theme.tertiary }]}>{label}</Text>
      <View style={[styles.rule, { backgroundColor: theme.border }]} />
    </View>
  );
}

/** The small header on a message Nexus starts on its own: its mark, its name and, when known, the time. */
export function NexusHeader({ when, theme }: { when?: string; theme: Theme }) {
  const assistant = useAssistantName();
  return (
    <View style={styles.header}>
      <NexusMark size={12} />
      <Text style={[styles.headerText, { color: theme.tertiary }]}>{when ? `${assistant} · ${when}` : assistant}</Text>
    </View>
  );
}

/**
 * One of your messages, and under it Not sent · Retry when it failed. `last`: the last of its group, with the room
 * before the next speaker. `fresh`: just sent here, so it rises in.
 */
export function PersonBubble({
  text: words,
  line,
  onRetry,
  theme,
  last = true,
  fresh = false,
}: {
  text: string;
  line?: string;
  onRetry?: () => void;
  theme: Theme;
  last?: boolean;
  fresh?: boolean;
}) {
  const reduce = useReducedMotion();
  return (
    <Reanimated.View entering={fresh && !reduce ? riseIn : undefined} style={[styles.personRow, last || line ? styles.groupEnd : styles.grouped]}>
      <View style={[styles.bubble, { backgroundColor: theme.userBubble }]}>
        <Text selectable style={[styles.words, { color: theme.foreground }]}>
          {words}
        </Text>
      </View>
      {line && onRetry ? (
        <Pressable onPress={onRetry} accessibilityRole="button" accessibilityLabel="Retry" accessibilityHint="Sends the message again" hitSlop={12}>
          <Text style={[styles.line, { color: theme.danger }]}>{line}</Text>
        </Pressable>
      ) : line ? (
        <Text style={[styles.line, { color: theme.tertiary }]} accessibilityLiveRegion="polite">
          {line}
        </Text>
      ) : null}
    </Reanimated.View>
  );
}

/** Nexus's side: full width, with no avatar. */
export function NexusSide({ fresh = false, children }: { fresh?: boolean; children: ReactNode }) {
  const reduce = useReducedMotion();
  return (
    <Reanimated.View entering={fresh && !reduce ? arriveIn : undefined} style={styles.nexus}>
      {children}
    </Reanimated.View>
  );
}

/**
 * A reply of Nexus's: each answer as prose with the line under it (the model and who paid), each card as a card.
 * `line: false` for a reply read back. `decide`: how an approval card approves or declines its write (A9).
 */
export function NexusTurn({
  messages,
  name,
  line,
  theme,
  decide,
  edit,
  fresh = false,
  replies,
  onReply,
  errandNow,
}: {
  messages: AgentMessage[];
  name: (model: string) => string;
  line: boolean;
  theme: Theme;
  decide?: DecideWrite;
  edit?: EditWrite;
  fresh?: boolean;
  /** Quick replies under the turn (A1·D), with what a tap does. */
  replies?: readonly string[];
  onReply?: (reply: string) => void;
  errandNow?: (id: string) => AgentErrand | undefined;
}) {
  return (
    <NexusSide fresh={fresh}>
      {messages.map((message, i) => (
        <AgentReply key={i} message={message} name={name} theme={theme} line={line} decide={decide} edit={edit} errandNow={errandNow} />
      ))}
      {replies?.length && onReply ? (
        <View style={styles.replies}>
          <QuickReplies replies={replies} theme={theme} onPick={onReply} />
        </View>
      ) : null}
    </NexusSide>
  );
}

/**
 * The answer being written: the live step line while a tool runs ("Searching Linear"), or "Thinking" until the words
 * start. Then the words, as prose.
 */
export function NexusDraft({ draft, name, theme }: { draft: { text: string; model?: string; step?: string }; name: (model: string) => string; theme: Theme }) {
  return (
    <NexusSide fresh>
      {draft.text ? (
        <View style={styles.answer} accessibilityLabel="The answer, as it is written">
          <Markdown source={draft.text} linkHosts />
          {draft.model ? <Text style={[styles.meta, { color: theme.tertiary }]}>{name(draft.model)}</Text> : null}
        </View>
      ) : (
        <View style={styles.answer}>
          <StepLine line={draft.step} theme={theme} />
        </View>
      )}
    </NexusSide>
  );
}

/** Nexus's first message in an empty thread, with its header: it writes first, so the thread is never blank. */
export function NexusGreeting({ words, theme, children }: { words: string; theme: Theme; children?: ReactNode }) {
  return (
    <NexusSide>
      <View style={styles.answer}>
        <NexusHeader theme={theme} />
        <Text style={[styles.words, { color: theme.foreground }]}>{words}</Text>
      </View>
      {children}
    </NexusSide>
  );
}

/** Under the thread while the phone is offline (A1·C i). */
export function OfflineLine({ theme }: { theme: Theme }) {
  return (
    <View style={styles.offline} accessibilityRole="alert" accessibilityLabel={OFFLINE_LINE}>
      <Icon name="offline" size={16} color={theme.tertiary} />
      <Text style={[styles.offlineText, { color: theme.tertiary }]}>{OFFLINE_LINE}</Text>
    </View>
  );
}

// The transcript's metrics, as Chat's (`messages.tsx`): 17/24 reading text, a 20 pt bubble with 14 by 10 pt padding at
// most 85 % wide, 20 pt before the next speaker and 6 pt inside a group. The profile header is the canvas's: the mark
// at 56 pt, the name at 18 pt over the place at 13 pt. A day marker is 12 pt between hairlines.
const styles = StyleSheet.create({
  profile: { alignItems: "center", gap: 4, paddingTop: 12, paddingBottom: 20 },
  profileName: { fontSize: 18, lineHeight: 24, fontFamily: font.medium, marginTop: 6 },
  profileWhere: { fontSize: 13, lineHeight: 17, fontFamily: font.regular },
  day: { flexDirection: "row", alignItems: "center", gap: 10, marginTop: 4, marginBottom: 16 },
  rule: { flex: 1, height: StyleSheet.hairlineWidth },
  dayText: { fontSize: 12, lineHeight: 16, fontFamily: font.medium },
  header: { flexDirection: "row", alignItems: "center", gap: 6, marginBottom: 4 },
  headerText: { fontSize: 12, lineHeight: 16, fontFamily: font.regular },
  personRow: { alignItems: "flex-end", gap: 4 },
  bubble: { maxWidth: "85%", paddingHorizontal: 14, paddingVertical: 10, borderRadius: 20, borderCurve: "continuous" },
  words: { ...text.reading, fontFamily: font.regular },
  groupEnd: { marginBottom: 20 },
  grouped: { marginBottom: 6 },
  nexus: { alignSelf: "stretch" },
  answer: { marginBottom: 20, paddingHorizontal: 4 },
  replies: { marginTop: -12, marginBottom: 20, paddingHorizontal: 4 },
  line: { fontSize: 12, lineHeight: 16, fontFamily: font.regular, paddingRight: 4 },
  meta: { ...text.xs, fontFamily: mono, marginTop: 6 },
  offline: { flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 8, marginBottom: 14, paddingHorizontal: 16 },
  offlineText: { fontSize: 12, lineHeight: 16, fontFamily: font.regular, flexShrink: 1 },
});
