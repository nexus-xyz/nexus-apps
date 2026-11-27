import { useNavigation } from "expo-router";
import type { ComponentProps, ReactNode } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

type IconName = ComponentProps<typeof Icon>["name"];

type BarProps = {
  title: string;
  line?: string;
  onShare?: () => void;
  onMore?: () => void;
  moreLabel?: string;
  /** Opens the comments; `comments` is how many threads are open. */
  onComments?: () => void;
  comments?: number;
  children: ReactNode;
  onContents?: () => void;
};

/**
 * A site read full-bleed (brand v5's P20 "Page"): the page owns the screen, and this bar at its foot is the only chrome.
 * Back, the page's title with who sees it and its version under it, then Comments, Share and More. The status bar's
 * height above the page stays the screen's colour, so the page never runs under the clock.
 */
export function PageScreen({ title, line, onShare, onMore, moreLabel = "More", onComments, comments, children }: BarProps) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const navigation = useNavigation();
  return (
    <View style={[styles.screen, { paddingTop: insets.top, backgroundColor: theme.screen }]}>
      <View style={styles.page}>{children}</View>
      <View style={[styles.bar, { paddingBottom: Math.max(insets.bottom, 8), borderTopColor: theme.border, backgroundColor: theme.screen }]}>
        <BarButton icon="back" label="Back" onPress={() => navigation.goBack()} color={theme.foreground} />
        <View style={styles.text} accessible accessibilityRole="header" accessibilityLabel={[title, line].filter(Boolean).join(", ")}>
          <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {title}
          </Text>
          {line ? (
            <View style={styles.lineRow}>
              <View style={[styles.dot, { backgroundColor: theme.tertiary }]} />
              <Text style={[styles.line, { color: theme.tertiary }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                {line}
              </Text>
            </View>
          ) : null}
        </View>
        {onComments ? <CommentsButton count={comments ?? 0} onPress={onComments} color={theme.foreground} /> : null}
        {onShare ? <BarButton icon="share" label="Share" onPress={onShare} color={theme.foreground} /> : null}
        {onMore ? <BarButton icon="more" label={moreLabel} onPress={onMore} color={theme.foreground} /> : null}
      </View>
    </View>
  );
}

/**
 * A doc read as brand v5's P20 Doc draws it: the bar on top (Back; Comments, Share and More at the right), then the
 * doc's provenance under it ("Doc · v7 · written in a chat"), then the doc. A doc is read top down, so its controls are
 * where the reading starts; a site keeps its bar at the foot, out of the page's way.
 */
export function DocScreen({ title, line, onShare, onMore, moreLabel = "More", onComments, comments, onContents, children }: BarProps) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const navigation = useNavigation();
  return (
    // The doc runs to the screen's foot, under the home indicator, as a page in Safari does.
    <View style={[styles.screen, { paddingTop: insets.top, backgroundColor: theme.screen }]}>
      <View style={styles.topBar}>
        <BarButton icon="back" label="Back" onPress={() => navigation.goBack()} color={theme.foreground} />
        {onContents ? (
          <Pressable style={({ pressed }) => [styles.contentsTarget, pressed && styles.pressed]}
            accessibilityRole="button" accessibilityLabel={`${title}, Contents`} onPress={() => { haptic("tap"); onContents(); }}>
            <Text style={[styles.topTitle, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>{title}</Text>
            <Icon name="chevron" size={12} color={theme.muted} />
          </Pressable>
        ) : (
          <Text style={[styles.topTitle, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE} accessibilityRole="header">
            {title}
          </Text>
        )}
        {onComments ? <CommentsButton count={comments ?? 0} onPress={onComments} color={theme.foreground} /> : null}
        {onShare ? <BarButton icon="share" label="Share" onPress={onShare} color={theme.foreground} /> : null}
        {onMore ? <BarButton icon="more" label={moreLabel} onPress={onMore} color={theme.foreground} /> : null}
      </View>
      {line ? (
        <Text style={[styles.provenance, { color: theme.tertiary }]} numberOfLines={2} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
          {line}
        </Text>
      ) : null}
      <View style={styles.page}>{children}</View>
    </View>
  );
}

/** Comments, with how many threads are open beside the bubble; nothing beside it when none are. */
function CommentsButton({ count, onPress, color }: { count: number; onPress: () => void; color: string }) {
  return (
    <Pressable
      style={({ pressed }) => [styles.button, count ? styles.counted : null, pressed && styles.pressed]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={count ? `Comments, ${count} open` : "Comments"}
    >
      <Icon name="comment" size={19} color={color} />
      {count ? (
        <Text style={[styles.count, { color }]} allowFontScaling={false}>
          {count}
        </Text>
      ) : null}
    </Pressable>
  );
}

/** A 44 pt control in the bar, its symbol optically centred in it. */
function BarButton({ icon, label, onPress, color }: { icon: IconName; label: string; onPress: () => void; color: string }) {
  return (
    <Pressable style={({ pressed }) => [styles.button, pressed && styles.pressed]} onPress={onPress} accessibilityRole="button" accessibilityLabel={label}>
      <Icon name={icon} size={19} color={color} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  page: { flex: 1 },
  bar: { flexDirection: "row", alignItems: "center", gap: 4, paddingTop: 6, paddingHorizontal: 6, borderTopWidth: StyleSheet.hairlineWidth },
  text: { flex: 1, paddingHorizontal: 4, gap: 1 },
  title: { ...text.secondary, fontFamily: font.medium },
  lineRow: { flexDirection: "row", alignItems: "center", gap: 5 },
  dot: { width: 5, height: 5, borderRadius: 2.5 },
  line: { fontFamily: font.regular, fontSize: 12, lineHeight: 15 },
  button: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  // The count widens the target rather than crowding the bubble: 4 pt between them, tabular so 9 → 10 holds its width.
  counted: { width: undefined, minWidth: 44, flexDirection: "row", gap: 4, paddingHorizontal: 8 },
  count: { fontFamily: font.medium, fontSize: 13, fontVariant: ["tabular-nums"] },
  topBar: { flexDirection: "row", alignItems: "center", gap: 2, paddingHorizontal: 6, minHeight: 52 },
  contentsTarget: { flex: 1, minHeight: 44, flexDirection: "row", alignItems: "center", paddingRight: 6 },
  topTitle: { ...text.secondary, fontFamily: font.medium, flex: 1, paddingHorizontal: 4 },
  // The boards' provenance: 13 pt in text-3, on the reading column's 20 pt inset.
  provenance: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 20, paddingBottom: 6 },
  pressed: { opacity: 0.5 },
});
