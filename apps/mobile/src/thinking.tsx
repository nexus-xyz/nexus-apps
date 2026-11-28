import { useEffect } from "react";
import { StyleSheet, Text } from "react-native";
import Reanimated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withSpring, withTiming, type EntryAnimationsValues } from "react-native-reanimated";
import { Icon } from "@/icon";
import { PULSE, spring, SWAP } from "@/motion";
import { font, text, type Theme } from "@/theme";

/** "Thinking" until the first words arrive: it pulses, and holds still under Reduce Motion. Chat uses it. */
export function Thinking({ theme, label }: { theme: Theme; label: string }) {
  const reduce = useReducedMotion();
  const opacity = useSharedValue(1);
  useEffect(() => {
    if (reduce) return;
    opacity.value = withRepeat(withTiming(PULSE.low, { duration: PULSE.half, easing: PULSE.easing }), -1, true);
    return () => cancelAnimation(opacity);
  }, [opacity, reduce]);
  const pulse = useAnimatedStyle(() => ({ opacity: opacity.value }));
  return (
    <Reanimated.View style={[styles.thinking, pulse]} accessibilityLabel={label} accessibilityLiveRegion="polite">
      <Icon name="brain" size={16} color={theme.muted} />
      <Text style={[styles.thinkingText, { color: theme.muted }]}>{label}</Text>
    </Reanimated.View>
  );
}

/**
 * Send and Stop take each other's place in the same circle: the new icon grows from 0.3 and fades in on a spring with
 * no bounce, so the swap reads as one control changing. Under Reduce Motion it just appears. The options are built
 * here, on the JS thread: `swapIn` runs on the UI thread, where it may only read them, not call `spring`.
 */
const SWAP_SPRING = spring(SWAP);
export function swapIn(_values: EntryAnimationsValues) {
  "worklet";
  return {
    initialValues: { opacity: 0, transform: [{ scale: SWAP.from }] },
    animations: { opacity: withSpring(1, SWAP_SPRING), transform: [{ scale: withSpring(1, SWAP_SPRING) }] },
  };
}

const styles = StyleSheet.create({
  thinking: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32 },
  thinkingText: { ...text.sm, fontFamily: font.regular },
});
