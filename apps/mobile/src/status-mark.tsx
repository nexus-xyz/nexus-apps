import type { AgentSessionSummary } from "@nexus/kit";
import { useEffect } from "react";
import { StyleSheet, View } from "react-native";
import Reanimated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import { Icon } from "@/icon";
import { SPIN } from "@/motion";
import { useTheme } from "@/theme";

/**
 * A session's live mark, first in its row (brand v5's P4 and P18): the turning ring while it runs, a check once done, a
 * cross when it failed, and a hollow ring when it stopped otherwise. Reduce Motion holds the ring still. It only
 * decorates: the row says the status in words.
 */
export function StatusMark({ status, size = 12 }: { status: AgentSessionSummary["status"]; size?: number }) {
  const theme = useTheme();
  return (
    <View style={[styles.box, { width: size + 8 }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" pointerEvents="none">
      {status === "running" ? (
        <Turning size={size} color={theme.foreground} />
      ) : status === "succeeded" ? (
        <Icon name="check" size={size} color={theme.tertiary} />
      ) : status === "failed" ? (
        <Icon name="close" size={size} color={theme.danger} />
      ) : (
        <View style={[styles.ring, { width: size - 2, height: size - 2, borderRadius: size, borderColor: theme.tertiary }]} />
      )}
    </View>
  );
}

/** The ring with a gap, turning once a second. */
function Turning({ size, color }: { size: number; color: string }) {
  const reduce = useReducedMotion();
  const turn = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    turn.value = withRepeat(withTiming(1, { duration: SPIN.turn, easing: SPIN.easing }), -1, false);
    return () => cancelAnimation(turn);
  }, [reduce, turn]);
  const spin = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.value * 360}deg` }] }));
  return <Reanimated.View style={[styles.ring, { width: size, height: size, borderRadius: size, borderColor: color, borderTopColor: "transparent" }, spin]} />;
}

const styles = StyleSheet.create({
  box: { alignItems: "center", justifyContent: "center" },
  ring: { borderWidth: 1.6 },
});
