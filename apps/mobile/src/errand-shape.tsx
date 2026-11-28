import type { AgentErrandStatus } from "@nexus/kit";
import { useEffect } from "react";
import { StyleSheet, View } from "react-native";
import Reanimated, { cancelAnimation, Easing, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";

/**
 * An errand's state by shape (A1·C, A2·A, A2·B), 8 pt: a filled dot needs you, an arc that turns once a second runs.
 * Under Reduce Motion the arc holds still. Hidden from VoiceOver: the row or pill says the state in words.
 */
export function ErrandShape({ status, color }: { status: AgentErrandStatus; color: string }) {
  if (status === "needs-you") return <View style={[styles.dot, { backgroundColor: color }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" />;
  return <TurningArc color={color} />;
}

function TurningArc({ color }: { color: string }) {
  const reduce = useReducedMotion();
  const turn = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    turn.value = withRepeat(withTiming(1, { duration: 1000, easing: Easing.linear }), -1, false);
    return () => cancelAnimation(turn);
  }, [turn, reduce]);
  const style = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.value * 360}deg` }] }));
  return (
    <Reanimated.View
      style={[styles.arc, { borderColor: color, borderTopColor: "transparent" }, style]}
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
    />
  );
}

const styles = StyleSheet.create({
  dot: { width: 8, height: 8, borderRadius: 4 },
  arc: { width: 8, height: 8, borderRadius: 4, borderWidth: 1.5 },
});
