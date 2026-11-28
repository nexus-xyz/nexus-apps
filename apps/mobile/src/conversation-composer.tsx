import { useEffect } from "react";
import { StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useReanimatedKeyboardAnimation } from "react-native-keyboard-controller";
import Reanimated, { cancelAnimation, interpolate, interpolateColor, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import { waitingLook, workingLook, type Halo } from "@/composer-halo";
import { Glass } from "@/glass";
import { BREATH, DURATION, timing } from "@/motion";
import { useScreenOwnsKeyboard } from "@/screen-keyboard";
import { EdgeFade } from "@/top-bar";
import type { Theme } from "@/theme";

/**
 * The floating composer: Liquid Glass on iOS 26 (`glass.tsx` has the stand-ins), over a fade of the screen, in its halo.
 * `footer` sits under the pill, above the home indicator (the Assistant's line).
 */
export function Composer({ theme, halo, focused, box, footer, children }: { theme: Theme; halo: Halo; focused: boolean; box: boolean; footer?: React.ReactNode; children: React.ReactNode }) {
  const insets = useSafeAreaInsets();
  // The home indicator's inset below the pill while the keyboard is down, 8 pt above it while up, eased with the
  // keyboard's own progress on the UI thread rather than switched when it starts to move. The sidebar's keyboard leaves it.
  const { progress } = useReanimatedKeyboardAnimation();
  const owns = useScreenOwnsKeyboard();
  const closed = Math.max(insets.bottom, 12);
  const bottom = useAnimatedStyle(() => ({ paddingBottom: owns ? interpolate(progress.value, [0, 1], [closed, 8]) : closed }));
  return (
    <Reanimated.View style={[styles.composer, bottom]}>
      <EdgeFade color={theme.screen} edge="bottom" style={[StyleSheet.absoluteFill, { top: -20 }]} />
      <View>
        <Glass style={[styles.pill, box && styles.box]}>{children}</Glass>
        {halo ? <ComposerHalo halo={halo} focused={focused} box={box} theme={theme} /> : null}
      </View>
      {footer}
    </Reanimated.View>
  );
}

/**
 * The composer's halo (`composer-halo.ts`), a layer over the pill's edge that never takes a touch. It is the pill's
 * sibling, so the working glow's breath fades the layer alone: glass under an opacity below 1 does not render. The
 * waiting halo's edge brightens over 180 ms when the composer takes focus.
 */
function ComposerHalo({ halo, focused, box, theme }: { halo: NonNullable<Halo>; focused: boolean; box: boolean; theme: Theme }) {
  const reduce = useReducedMotion();
  const breath = useSharedValue(1);
  const focus = useSharedValue(focused ? 1 : 0);
  useEffect(() => {
    focus.value = withTiming(focused ? 1 : 0, timing(DURATION.fast));
  }, [focused, focus]);
  useEffect(() => {
    if (halo !== "working" || reduce) {
      cancelAnimation(breath);
      breath.value = 1;
      return;
    }
    breath.value = BREATH.low;
    breath.value = withRepeat(withTiming(1, { duration: BREATH.half, easing: BREATH.easing }), -1, true);
    return () => cancelAnimation(breath);
  }, [halo, reduce, breath]);
  const look = halo === "waiting" ? waitingLook(theme.ice, theme.border, focused) : workingLook(theme.foreground);
  const edge = halo === "waiting" ? [false, true].map((on) => waitingLook(theme.ice, theme.border, on).borderColor ?? theme.border) : null;
  const fade = useAnimatedStyle(() => ({
    opacity: breath.value,
    ...(edge && { borderColor: interpolateColor(focus.value, [0, 1], edge) }),
  }));
  return <Reanimated.View pointerEvents="none" style={[styles.halo, box && styles.box, edge ? { borderWidth: 1 } : null, { boxShadow: look.boxShadow }, fade]} />;
}

const styles = StyleSheet.create({
  composer: { paddingHorizontal: 12, paddingTop: 6 },
  pill: { borderRadius: 28, borderCurve: "continuous", paddingHorizontal: 6, paddingTop: 4, paddingBottom: 6 },
  halo: { position: "absolute", top: 0, right: 0, bottom: 0, left: 0, borderRadius: 28, borderCurve: "continuous" },
  box: { borderRadius: 20 },
});
