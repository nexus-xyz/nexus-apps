import { GlassView, isGlassEffectAPIAvailable, isLiquidGlassAvailable } from "expo-glass-effect";
import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { AccessibilityInfo, Platform, Pressable, StyleSheet, View, type ColorValue, type ViewProps } from "react-native";
import { DURATION, SCALE } from "@/motion";
import { haptic } from "@/haptics";
import { useTheme } from "@/theme";

/**
 * Liquid Glass (iOS 26 and later) for what floats over the content: the chat's bar and composer, the sidebar's search
 * and foot, the menus. Where iOS draws none (before 26, a 26 beta without the API, Reduce Transparency on) and on
 * Android, the same shape in the theme's stand-in surface. Apple warns that glass under an opacity below 1 does not
 * render, so nothing here fades a glass view or its parents.
 */
const NATIVE = Platform.OS === "ios" && isLiquidGlassAvailable() && isGlassEffectAPIAvailable();

// Reduce Transparency, followed by one system listener for every glass view on screen: added with the first, read
// then, and removed with the last.
let reduced = false;
const listeners = new Set<() => void>();
let source: { remove(): void } | undefined;
const setReduced = (value: boolean) => {
  reduced = value;
  listeners.forEach((listener) => listener());
};
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  if (NATIVE && !source) {
    source = AccessibilityInfo.addEventListener("reduceTransparencyChanged", setReduced);
    // A failed read draws the stand-in surface: when the setting is unknown, the accessible choice.
    AccessibilityInfo.isReduceTransparencyEnabled().then(setReduced, () => setReduced(true));
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size) return;
    source?.remove();
    source = undefined;
  };
};

/** Whether glass is drawn now. */
export const useGlass = () => useSyncExternalStore(subscribe, () => NATIVE && !reduced);

/**
 * A glass surface; `interactive` (set once, at mount) makes it react to a touch, for controls only. `appear` makes it
 * materialize when it mounts, as iOS 26's own menus do.
 */
export function Glass({
  interactive,
  appear,
  tint,
  style,
  children,
  ...props
}: ViewProps & { interactive?: boolean; appear?: boolean; tint?: ColorValue }) {
  const theme = useTheme();
  const [on, setOn] = useState(!appear);
  useEffect(() => setOn(true), []);
  if (useGlass()) {
    return (
      <GlassView
        {...props}
        glassEffectStyle={on ? (appear ? { style: "regular", animate: true, animationDuration: DURATION.fast / 1000 } : "regular") : "none"}
        isInteractive={interactive}
        tintColor={tint}
        style={style}
      >
        {children}
      </GlassView>
    );
  }
  const surface =
    Platform.OS === "android"
      ? [styles.raised, { backgroundColor: theme.glassSolid }]
      : [styles.frosted, { backgroundColor: theme.glass, borderColor: theme.glassBorder }];
  return (
    <View {...props} style={[surface, tint ? { backgroundColor: tint } : null, style]}>
      {children}
    </View>
  );
}

/** A round glass button with one icon, 44 pt across (the touch target Apple's guidelines ask for). */
export function GlassButton({ label, hint, onPress, children }: { label: string; hint?: string; onPress: () => void; children: ReactNode }) {
  const theme = useTheme();
  const glass = useGlass();
  return (
    <Pressable
      onPress={() => {
        haptic("tap");
        onPress();
      }}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityHint={hint}
      hitSlop={6}
      // Glass answers a touch itself; iOS's stand-in surface shrinks a little instead, and Android's ripples.
      android_ripple={Platform.OS === "android" ? { color: theme.ripple, borderless: true, radius: 22 } : undefined}
      style={({ pressed }) => pressed && !glass && Platform.OS !== "android" && styles.pressed}
    >
      <Glass interactive style={styles.round}>
        {children}
      </Glass>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  // No shadow: iOS cannot draw one cheaply under a translucent view, and the border already sets it apart.
  frosted: { borderWidth: StyleSheet.hairlineWidth },
  raised: { elevation: 3 },
  round: { width: 44, height: 44, borderRadius: 22, alignItems: "center", justifyContent: "center" },
  pressed: { transform: [{ scale: SCALE.press }] },
});
