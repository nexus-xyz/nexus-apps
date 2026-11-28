import type { ReactNode } from "react";
import { Platform, StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import Svg, { Defs, LinearGradient, Path, Rect, Stop } from "react-native-svg";
import { GlassButton } from "@/glass";
import { useTheme } from "@/theme";

/** The bar's height under the status bar: a 44 pt control and its margins. */
export const BAR_HEIGHT = 56;

/**
 * The chat's bar, as ChatGPT's: no navigation bar, the controls float in glass over the messages, with a fade behind
 * them so what scrolls up under the status bar stays out of the way. `center` is centered on the screen, kept
 * `centerInset` from each edge so it never runs under the sides. On Android it starts after the left control, as
 * Material 3's top app bar sets its title, and runs to the right control, or to the edge when there is none. A control
 * in the middle (the mode switch) sets `centered`, and is centered on Android too.
 */
export function TopBar({
  left,
  center,
  right,
  centerInset = 64,
  centered,
}: {
  left?: ReactNode;
  center?: ReactNode;
  right?: ReactNode;
  centerInset?: number;
  centered?: boolean;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  return (
    <View pointerEvents="box-none" style={[styles.bar, { paddingTop: insets.top }]}>
      <EdgeFade color={theme.screen} edge="top" style={[StyleSheet.absoluteFill, { bottom: -20 }]} />
      <View pointerEvents="box-none" style={styles.row}>
        {left}
        <View pointerEvents="box-none" style={styles.spacer} />
        {right}
        <View pointerEvents="box-none" style={[styles.center, ANDROID && !centered ? [styles.start, { right: right ? centerInset : 16 }] : { left: centerInset, right: centerInset }]}>
          {center}
        </View>
      </View>
    </View>
  );
}

/** The screen's colour fading out from one edge: the stand-in for iOS 26's scroll edge effect, on both platforms. */
export function EdgeFade({ color, edge, style }: { color: string; edge: "top" | "bottom"; style?: StyleProp<ViewStyle> }) {
  const id = `fade-${edge}`;
  const [from, to] = edge === "top" ? ["0", "1"] : ["1", "0"];
  return (
    <View pointerEvents="none" style={style}>
      <Svg width="100%" height="100%" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <Defs>
          <LinearGradient id={id} x1="0" y1={from} x2="0" y2={to}>
            <Stop offset="0" stopColor={color} stopOpacity={1} />
            <Stop offset="0.6" stopColor={color} stopOpacity={0.94} />
            <Stop offset="1" stopColor={color} stopOpacity={0} />
          </LinearGradient>
        </Defs>
        <Rect width="100%" height="100%" fill={`url(#${id})`} />
      </Svg>
    </View>
  );
}

/** Opens the sidebar: ChatGPT's glyph, two lines with the lower one shorter. */
export function SidebarButton({ onPress }: { onPress: () => void }) {
  const theme = useTheme();
  return (
    <GlassButton label="Open the sidebar" onPress={onPress}>
      <Svg width={22} height={22} viewBox="0 0 24 24">
        <Path d="M3.5 8.5h17M3.5 15.5h10" stroke={theme.foreground} strokeWidth={2.2} strokeLinecap="round" />
      </Svg>
    </GlassButton>
  );
}

// The left control's 44 pt and the bar's 16 pt margin, then 12 pt to the title.
const ANDROID = Platform.OS === "android";
const styles = StyleSheet.create({
  bar: { position: "absolute", top: 0, left: 0, right: 0, zIndex: 5 },
  row: { height: BAR_HEIGHT, flexDirection: "row", alignItems: "center", paddingHorizontal: 16 },
  spacer: { flex: 1 },
  center: { position: "absolute", top: 0, bottom: 0, alignItems: "center", justifyContent: "center" },
  start: { left: 72, alignItems: "flex-start" },
});
