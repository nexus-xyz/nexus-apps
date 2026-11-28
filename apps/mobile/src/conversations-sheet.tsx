import type { AgentClient, AgentConversationSummary } from "@nexus/kit";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Platform, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { HISTORY_UNAVAILABLE, historyAfter, PAGE, rowLabel, rowLine, type History } from "@/agent-conversations";
import { Icon } from "@/icon";
import { pressedFill, ripple } from "@/press";
import { Sheet } from "@/sheet";
import { font, text, useTheme, type Theme } from "@/theme";

/**
 * Nexus's list of conversations: the first page as the screen opens, so a 404 hides the list before it shows,
 * the first page again each time the sheet opens, and the next page on Load more. Leaving the screen stops a read.
 */
export function useHistory(agent: AgentClient) {
  const [history, setHistory] = useState<History>({ kind: "unknown" });
  const [reading, setReading] = useState(false);
  const shown = useRef(history);
  shown.current = history;
  const pending = useRef<AbortController | null>(null);

  const read = useCallback(
    (more: boolean) => {
      const before = shown.current.kind === "on" ? shown.current.next : null;
      if (more && !before) return;
      pending.current?.abort();
      const controller = new AbortController();
      pending.current = controller;
      setReading(true);
      void agent
        .conversations({ limit: PAGE, ...(more && before ? { before } : {}), signal: controller.signal })
        .catch(() => "unreachable" as const)
        .then((result) => {
          if (controller.signal.aborted) return;
          setHistory((now) => historyAfter(now, result, more));
          setReading(false);
        });
    },
    [agent],
  );

  useEffect(() => {
    read(false);
    return () => pending.current?.abort();
  }, [read]);

  return { history, reading, refresh: useCallback(() => read(false), [read]), more: useCallback(() => read(true), [read]) };
}

/**
 * The sheet of conversations, as the model sheet is drawn: its title and Done, then a row for each conversation, newest
 * first, with its title, the start of its last turn, and where it started and when. The one on screen is checked.
 * When the list cannot be read, one line says so; the chat still works.
 */
export function ConversationsSheet({
  visible,
  onClose,
  history,
  reading,
  current,
  onOpen,
  onMore,
}: {
  visible: boolean;
  onClose: () => void;
  history: History;
  reading: boolean;
  /** The conversation on screen. */
  current?: string;
  onOpen: (row: AgentConversationSummary) => void;
  onMore: () => void;
}) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const now = Date.now();
  const rows = history.kind === "on" ? history.rows : [];
  const failed = history.kind === "unavailable" || (history.kind === "on" && history.failed);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.head}>
        <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
          Conversations
        </Text>
        <Pressable onPress={onClose} hitSlop={12} accessibilityRole="button" style={styles.done}>
          <Text style={[styles.doneText, { color: theme.muted }]}>Done</Text>
        </Pressable>
      </View>
      <ScrollView style={{ maxHeight: height * 0.62 }} accessibilityLabel="Your conversations">
        {rows.map((row) => (
          <ConversationRow key={row.id} row={row} now={now} selected={row.id === current} theme={theme} onPress={() => onOpen(row)} />
        ))}
        {history.kind === "on" && !rows.length && !failed ? (
          <Text style={[styles.line, { color: theme.muted }]}>No conversations yet. What you ask Nexus in Slack, on the web, on the desktop or here appears in this list.</Text>
        ) : null}
        {failed ? (
          <Text style={[styles.line, { color: theme.muted }]} accessibilityRole="alert">
            {HISTORY_UNAVAILABLE}
          </Text>
        ) : null}
        {reading ? (
          <ActivityIndicator style={styles.more} color={theme.muted} accessibilityLabel="Loading your conversations" />
        ) : history.kind === "on" && history.next ? (
          <Pressable onPress={onMore} style={styles.more} accessibilityRole="button" accessibilityLabel="Load more conversations" hitSlop={8}>
            <Text style={[styles.doneText, { color: theme.muted }]}>Load more</Text>
          </Pressable>
        ) : null}
      </ScrollView>
    </Sheet>
  );
}

function ConversationRow({ row, now, selected, theme, onPress }: { row: AgentConversationSummary; now: number; selected: boolean; theme: Theme; onPress: () => void }) {
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, selected && !ANDROID && { backgroundColor: theme.rowPressed }, pressed && pressedFill(theme)]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      accessibilityLabel={rowLabel(row, now)}
    >
      <View style={styles.grow}>
        <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={1}>
          {row.title || "Untitled"}
        </Text>
        {row.preview ? (
          <Text style={[styles.preview, { color: theme.muted }]} numberOfLines={2}>
            {row.preview}
          </Text>
        ) : null}
        <Text style={[styles.detail, { color: theme.tertiary }]} numberOfLines={1}>
          {rowLine(row, now)}
        </Text>
      </View>
      {selected ? <Icon name="check" size={16} color={theme.primary} /> : null}
    </Pressable>
  );
}

const ANDROID = Platform.OS === "android";
// The model sheet's measures (P32·F): the title and Done 20 pt in, rows set in 6 pt with a 12 pt radius. Android
// keeps Material's list items, 24 dp in.
const INSET = ANDROID ? 24 : 20;
const styles = StyleSheet.create({
  head: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: INSET, paddingTop: ANDROID ? 0 : 8, paddingBottom: 6 },
  title: { ...text.body, fontFamily: font.medium },
  done: { minHeight: 44, justifyContent: "center" },
  doneText: { ...text.body, fontFamily: font.regular },
  row: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 64, paddingVertical: 10, paddingHorizontal: ANDROID ? INSET : 14, marginHorizontal: ANDROID ? 0 : 6, borderRadius: 12, borderCurve: "continuous" },
  grow: { flex: 1, gap: 2 },
  label: { ...text.base, fontFamily: font.regular },
  preview: { ...text.sm, fontFamily: font.regular },
  detail: { ...text.caption, fontFamily: font.regular },
  line: { ...text.sm, fontFamily: font.regular, paddingHorizontal: INSET, paddingVertical: 16 },
  more: { alignSelf: "center", minHeight: 44, justifyContent: "center", paddingHorizontal: 24 },
});
