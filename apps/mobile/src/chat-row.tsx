import type { ChatSessionSummary } from "@nexus/kit";
import { Pressable, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { Icon } from "@/icon";
import type { Model } from "@/models";
import { nameOf } from "@/models";
import { pressedFill, ripple } from "@/press";
import { SwipeActions, type SwipeAction } from "@/swipe-actions";
import { isAccessibilitySize } from "@/text-size";
import { font, text, useTheme } from "@/theme";

const startOfToday = (now = new Date()) => new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();

/** The time for today's chats, the date for any other: the same calendar day as the shelves. */
const when = (iso: string) => {
  const at = new Date(iso);
  return at.getTime() >= startOfToday() ? at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : at.toLocaleDateString([], { month: "short", day: "numeric" });
};

/**
 * A chat in a grouped list, as brand v5's P18 draws a session: a row of its day's card, 64 pt high, with its title, the
 * model on the line under it, and when on the right. `first` and `last` round the card's corners; a hairline parts the
 * rows between. Long press opens its menu. With `onArchive`, a swipe to the left shows More (the same menu) and Archive.
 */
export function ChatRow({
  chat,
  models,
  onPress,
  onLongPress,
  onArchive,
  onUnarchive,
  pin,
  first,
  last,
}: {
  chat: ChatSessionSummary;
  models: Model[];
  onPress: () => void;
  onLongPress?: () => void;
  onArchive?: () => void;
  onUnarchive?: () => void;
  pin?: { label: string; onPress: () => void };
  first?: boolean;
  last?: boolean;
}) {
  const theme = useTheme();
  const archive = chat.archived
    ? { label: "Unarchive", action: onUnarchive }
    : { label: "Archive", action: onArchive };
  const menuLabel = chat.archived ? "Restore or delete" : pin ? "Pin, rename, archive or delete" : "Rename, archive or delete";
  const actions: SwipeAction[] = [];
  if (pin) actions.push({ key: "pin", icon: "pin", color: theme.swipeMore, ...pin });
  else if (archive.action && onLongPress) actions.push({ key: "more", label: "More", icon: "more", color: theme.swipeMore, onPress: onLongPress });
  if (archive.action && (pin || onLongPress)) actions.push({ key: "archive", label: archive.label, icon: "archive", color: theme.swipeArchive, onPress: archive.action });
  const model = nameOf(models, chat.lastModel ?? chat.model);
  const detail = `Chat · ${model}`;
  // At the accessibility text sizes the time goes under the model, so the title keeps the row's width.
  const stacked = isAccessibilitySize(useWindowDimensions().fontScale);
  const time = <Text style={[styles.when, { color: theme.tertiary }]}>{when(chat.updatedAt)}</Text>;
  const row = (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`${chat.title || "Untitled"}, ${detail}, ${when(chat.updatedAt)}`}
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, { backgroundColor: theme.card }, pressed && pressedFill(theme)]}
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityHint={onLongPress ? `Long press to ${menuLabel.toLowerCase()}` : undefined}
      accessibilityActions={[
        ...(onLongPress ? [{ name: "longpress", label: menuLabel }] : []),
        ...(archive.action ? [{ name: "archive", label: archive.label }] : []),
        ...(pin ? [{ name: "pin", label: pin.label }] : []),
      ]}
      onAccessibilityAction={({ nativeEvent }) => {
        if (nativeEvent.actionName === "longpress") onLongPress?.();
        if (nativeEvent.actionName === "archive") archive.action?.();
        if (nativeEvent.actionName === "pin") pin?.onPress();
      }}
    >
      <View style={styles.text}>
        <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={stacked ? 3 : 1}>
          {chat.title || "Untitled"}
        </Text>
        <Text style={[styles.meta, { color: theme.muted }]} numberOfLines={1}>
          {detail}
        </Text>
        {stacked ? time : null}
      </View>
      {stacked ? null : time}
      <Icon name="forward" size={13} color={theme.tertiary} />
      {last ? null : <View style={[styles.separator, { backgroundColor: theme.separator }]} />}
    </Pressable>
  );
  return (
    <View style={[styles.card, { backgroundColor: theme.card }, first && styles.first, last && styles.last]}>
      <SwipeActions actions={actions}>
        {row}
      </SwipeActions>
    </View>
  );
}

const styles = StyleSheet.create({
  card: { marginHorizontal: 16, overflow: "hidden" },
  first: { borderTopLeftRadius: 12, borderTopRightRadius: 12, borderCurve: "continuous" },
  last: { borderBottomLeftRadius: 12, borderBottomRightRadius: 12, borderCurve: "continuous" },
  row: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 64, paddingHorizontal: 16 },
  text: { flex: 1, gap: 2, paddingVertical: 10 },
  title: { ...text.body, fontFamily: font.regular },
  meta: { ...text.secondary, fontFamily: font.regular },
  when: { ...text.secondary, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  separator: { position: "absolute", left: 16, right: 0, bottom: 0, height: StyleSheet.hairlineWidth },
});
