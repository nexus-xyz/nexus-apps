import { useEffect, type ComponentProps, type ReactNode } from "react";
import { Platform, Pressable, StyleSheet, Text, View, useWindowDimensions, type AccessibilityActionInfo, type AccessibilityActionEvent } from "react-native";
import Reanimated, { cancelAnimation, Easing, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import type { RowState } from "@/drawer-groups";
import { Icon } from "@/icon";
import { SPIN } from "@/motion";
import { NexusMark } from "@/nexus-mark";
import { ripple } from "@/press";
import { ACCESSIBILITY_FONT_SCALE, CHROME_FONT_SCALE, isAccessibilitySize } from "@/text-size";
import { font, mono, text, useTheme, type Theme } from "@/theme";

/**
 * A row of the drawer and of Search, as brand v5's P94·A draws it: a 22 pt column for the icon or the session's mark,
 * the name, then on the right a count in its pill or the age in mono. Rows are 44 pt, 12 pt in from the drawer's
 * edge, the one on screen filled. Android keeps Material 3's navigation drawer: a 56 dp pill that clips its ripple.
 *
 * At the accessibility text sizes (P94·E) the row grows, the age moves under the name, and the name wraps rather than
 * being cut before the age.
 */
export function NavRow({
  leading,
  label,
  detail,
  meta,
  stateWord,
  badge,
  trailing,
  selected,
  onPress,
  onLongPress,
  accessibilityLabel,
  accessibilityHint,
  accessibilityState,
  accessibilityActions,
  onAccessibilityAction,
}: {
  leading: ReactNode;
  label: ReactNode;
  /** A second, muted line: where a search matched. */
  detail?: ReactNode;
  /** On the right, in mono: the age, a version, where a page lives. */
  meta?: string;
  /** In place of `meta`, in the label's face: what a session does now ("Working", "Needs you"), as the desktop says it. */
  stateWord?: string | null;
  /** On the right, in a pill: a count (the inbox's unread). */
  badge?: string;
  /** Last on the right: a folded project's mark and its chevron. */
  trailing?: ReactNode;
  selected?: boolean;
  onPress: () => void;
  onLongPress?: () => void;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  /** Merged over `selected`: a project row says whether it is expanded. */
  accessibilityState?: { expanded?: boolean };
  accessibilityActions?: AccessibilityActionInfo[];
  onAccessibilityAction?: (event: AccessibilityActionEvent) => void;
}) {
  const theme = useTheme();
  const stacked = isAccessibilitySize(useWindowDimensions().fontScale);
  const metaText = stateWord ? (
    <Text style={[styles.stateWord, { color: theme.muted }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
      {stateWord}
    </Text>
  ) : meta ? (
    <Text style={[styles.meta, { color: theme.tertiary }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
      {meta}
    </Text>
  ) : null;
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, selected && { backgroundColor: theme.sidebarRow }, pressed && pressedRow(theme)]}
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole="menuitem"
      // The row's words, never its symbol's name ("compose, New session").
      accessibilityLabel={accessibilityLabel ?? (typeof label === "string" ? [label, meta].filter(Boolean).join(", ") : undefined)}
      accessibilityHint={accessibilityHint}
      accessibilityState={{ selected: !!selected, ...accessibilityState }}
      accessibilityActions={accessibilityActions}
      onAccessibilityAction={onAccessibilityAction}
    >
      <View style={styles.lead}>{leading}</View>
      <View style={styles.body}>
        {typeof label === "string" ? (
          <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={stacked ? undefined : 1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {label}
          </Text>
        ) : (
          label
        )}
        {detail ? (
          <Text style={[styles.detail, { color: theme.muted }]} numberOfLines={2} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {detail}
          </Text>
        ) : null}
        {stacked ? metaText : null}
      </View>
      {stacked ? null : metaText}
      {badge ? (
        <View style={[styles.badge, { backgroundColor: theme.fill }]}>
          <Text style={[styles.badgeText, { color: theme.foreground }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {badge}
          </Text>
        </View>
      ) : null}
      {trailing}
    </Pressable>
  );
}

/** The row's name in the label's style, for a label made of parts (a search's matched words). */
export function NavLabel({ children }: { children: ReactNode }) {
  const theme = useTheme();
  const stacked = isAccessibilitySize(useWindowDimensions().fontScale);
  return (
    <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={stacked ? undefined : 1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
      {children}
    </Text>
  );
}

/**
 * A destination's or a page's icon, in the row's 22 pt column: 20 pt, the text's colour. "mark" is the Assistant's
 * Nexus mark, in the same ink at 18 pt: the phone board (AIPDAsstPhone) draws it at 16 beside 18 px symbols, since the
 * spiral fills its square and a symbol does not.
 */
export function NavIcon({ name }: { name: ComponentProps<typeof Icon>["name"] | "mark" }) {
  const theme = useTheme();
  if (name === "mark") return <NexusMark size={18} color={theme.foreground} />;
  return <Icon name={name} size={20} color={theme.foreground} />;
}

/**
 * A session's mark (P94·A), as the desktop's sidebar draws it (GPUI `sidebar.rs` `status_mark`): a ring turning on a
 * faint track while its agent works, an amber dot breathing a soft ring while it waits on you, a red cross when it
 * failed, and at rest a 10 pt ring, faint, or in the text's colour when it is the one on screen. Reduce Motion holds the
 * ring and the breath still. It only decorates: the row says the state in words.
 */
export function SessionMark({ state = "idle", current }: { state?: RowState; current?: boolean }) {
  const theme = useTheme();
  return (
    <View style={styles.markBox} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" pointerEvents="none">
      {state === "working" ? (
        <WorkingMark color={theme.working} />
      ) : state === "needsYou" ? (
        <NeedsYouMark color={theme.needsYou} />
      ) : state === "failed" ? (
        <Icon name="close" size={13} color={theme.danger} />
      ) : (
        <View style={[styles.ring, { borderColor: current ? theme.foreground : theme.tertiary }]} />
      )}
    </View>
  );
}

function WorkingMark({ color }: { color: string }) {
  const reduce = useReducedMotion();
  const turn = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    turn.value = withRepeat(withTiming(1, { duration: SPIN.turn, easing: SPIN.easing }), -1, false);
    return () => cancelAnimation(turn);
  }, [reduce, turn]);
  const spin = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.value * 360}deg` }] }));
  return (
    <>
      <View style={[styles.spinner, { borderColor: color, opacity: 0.25 }]} />
      <Reanimated.View style={[styles.spinner, { position: "absolute", borderColor: color, borderTopColor: "transparent" }, spin]} />
    </>
  );
}

function NeedsYouMark({ color }: { color: string }) {
  const reduce = useReducedMotion();
  const breath = useSharedValue(reduce ? 0.5 : 0);
  useEffect(() => {
    if (reduce) return;
    breath.value = withRepeat(withTiming(1, { duration: NEEDS_YOU_BREATH, easing: Easing.inOut(Easing.ease) }), -1, true);
    return () => cancelAnimation(breath);
  }, [reduce, breath]);
  const halo = useAnimatedStyle(() => ({ opacity: 0.3 * breath.value }));
  return (
    <>
      <Reanimated.View style={[styles.halo, { backgroundColor: color }, halo]} />
      <View style={[styles.dot, { backgroundColor: color }]} />
    </>
  );
}

/** Half of the desktop's 2.4 s pulse (`motion::PULSE`): the ring fades in, then out. */
const NEEDS_YOU_BREATH = 1200;

/** A group's label in the drawer and in Search: "Sessions", "Settings", "Artifacts". */
export function NavSection({ children }: { children: string }) {
  const theme = useTheme();
  return (
    <Text style={[styles.section, { color: theme.tertiary }]} accessibilityRole="header" maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
      {children}
    </Text>
  );
}

/** A row pressed: filled on iOS; Android ripples instead (`press.ts`). */
const pressedRow = (theme: Theme) => (ANDROID ? null : { backgroundColor: theme.sidebarRow });

const ANDROID = Platform.OS === "android";
const PILL = ANDROID ? { borderRadius: 28, overflow: "hidden" as const } : null;
// The boards' metrics (P94·A): rows 44 pt high, 12 pt padding and a 14 pt gap, 17 pt text; the age and the count in
// 13 pt mono; a group's label 13 pt, 20 pt in, 14 pt over its rows and 4 pt above them.
const styles = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "center", gap: 14, minHeight: ANDROID ? 56 : 44, paddingHorizontal: 12, paddingVertical: 6, borderRadius: 12, borderCurve: "continuous", ...PILL },
  lead: { width: 22, alignItems: "center", justifyContent: "center" },
  body: { flex: 1, gap: 2 },
  label: { ...text.body, fontFamily: font.regular },
  detail: { ...text.caption, fontFamily: font.regular },
  meta: { fontFamily: mono, fontSize: 13, lineHeight: 18, fontVariant: ["tabular-nums"] },
  stateWord: { ...text.caption, fontFamily: font.medium },
  badge: { minWidth: 24, paddingHorizontal: 7, height: 20, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  badgeText: { fontFamily: mono, fontSize: 13, fontVariant: ["tabular-nums"] },
  markBox: { width: 22, height: 22, alignItems: "center", justifyContent: "center" },
  ring: { width: 10, height: 10, borderRadius: 5, borderWidth: 1.5 },
  spinner: { width: 14, height: 14, borderRadius: 7, borderWidth: 1.6 },
  halo: { position: "absolute", width: 18, height: 18, borderRadius: 9 },
  dot: { width: 10, height: 10, borderRadius: 5 },
  section: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 20, paddingTop: 14, paddingBottom: 4 },
});
