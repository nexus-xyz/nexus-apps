import { useDrawerProgress } from "expo-router/drawer";
import type { ReactNode } from "react";
import { Dimensions, Platform, StyleSheet, useColorScheme, View } from "react-native";
import Reanimated, { interpolate, useAnimatedStyle, useReducedMotion } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { displayCornerRadius } from "@/display-corner";
import { DRAWER } from "@/motion";
import { useTheme } from "@/theme";

const IOS = Platform.OS === "ios";

/**
 * The chat as a card over the sidebar on iOS (`DRAWER` in `motion.ts`). As the sidebar opens, the card takes the
 * screen's own corners, draws an outline and fades its content under a scrim of its own colour. A tap on the card still closes the
 * sidebar: the drawer's overlay stays over it, transparent. Android keeps Material's drawer, which slides over the chat.
 */
export function DrawerCard({ children }: { children: ReactNode }) {
  return IOS ? <Card>{children}</Card> : <>{children}</>;
}

/** The sidebar behind the card on iOS: small and shaded while the card covers it, full size as the card slides off. */
export function SidebarDepth({ children }: { children: ReactNode }) {
  return IOS ? <Depth>{children}</Depth> : <>{children}</>;
}

function Card({ children }: { children: ReactNode }) {
  const progress = useDrawerProgress();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const screen = Dimensions.get("screen");
  // The screen's own corners: the card is the screen, sliding away. A Home button's phone has square ones.
  const radius = displayCornerRadius(screen.width, screen.height, insets.bottom > 0);

  const shape = useAnimatedStyle(() => ({
    borderRadius: interpolate(progress.value, [0, DRAWER.roundBy], [0, radius], "clamp"),
  }));
  const edge = useAnimatedStyle(() => ({
    borderRadius: interpolate(progress.value, [0, DRAWER.roundBy], [0, radius], "clamp"),
    opacity: interpolate(progress.value, [0, DRAWER.roundBy], [0, 1], "clamp"),
  }));
  const scrim = useAnimatedStyle(() => ({ opacity: progress.value * DRAWER.fade }));

  // The scrim and the outline sit outside the clipped card, over it: inside, the native screen container can be
  // stacked above them, and the chat's opaque top fade and composer then hide the outline along the corners.
  return (
    <View style={styles.fill}>
      <Reanimated.View style={[styles.card, shape]}>{children}</Reanimated.View>
      <Reanimated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.round, { backgroundColor: theme.screen }, shape, scrim]} />
      {/* The outline, a step brighter than a row's hairline, so the corners read against the sidebar. */}
      <Reanimated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.edge, { borderColor: theme.outline }, edge]} />
    </View>
  );
}

function Depth({ children }: { children: ReactNode }) {
  const progress = useDrawerProgress();
  const reduceMotion = useReducedMotion();
  const shade = useColorScheme() === "dark" ? DRAWER.shade.dark : DRAWER.shade.light;

  // Under Reduce Motion the sidebar does not scale; its shade still lifts.
  const grow = useAnimatedStyle(() => ({
    transform: [{ scale: reduceMotion ? 1 : interpolate(progress.value, [0, 1], [DRAWER.recede, 1], "clamp") }],
  }));
  const dim = useAnimatedStyle(() => ({ opacity: (1 - progress.value) * shade }));

  return (
    <View style={styles.fill}>
      <Reanimated.View style={[styles.fill, grow]}>{children}</Reanimated.View>
      <Reanimated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.shade, dim]} />
    </View>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  card: { flex: 1, overflow: "hidden", borderCurve: "continuous" },
  round: { borderCurve: "continuous" },
  edge: { borderWidth: StyleSheet.hairlineWidth, borderCurve: "continuous" },
  shade: { backgroundColor: "#000" },
});
