import type { ReactNode } from "react";
import { Pressable, type PressableProps, type StyleProp, type ViewStyle } from "react-native";
import Reanimated, { useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { DURATION, EASE_PRESS, SCALE, timing } from "@/motion";

// Down at once, back softly (`motion.ts`). A release mid-press turns back from where the scale is. Under Reduce Motion
// the scale jumps rather than moves.
const DOWN = timing(DURATION.press, EASE_PRESS);
const UP = timing(DURATION.release, EASE_PRESS);

/**
 * A control that gives a little under the finger: a card, a chip, a sheet's button. It shrinks to 0.96, never further.
 * A row in a list highlights instead (`press.ts`), as iOS's cells do.
 */
export function PressScale({
  style,
  hostStyle,
  children,
  onPressIn,
  onPressOut,
  ...props
}: Omit<PressableProps, "style" | "children"> & {
  /** The shape that shrinks. */
  style?: StyleProp<ViewStyle>;
  /** Its place in the layout (`flex`, a width), which stays put while it shrinks. */
  hostStyle?: StyleProp<ViewStyle>;
  children: ReactNode;
}) {
  const scale = useSharedValue(1);
  const pressed = useAnimatedStyle(() => ({ transform: [{ scale: scale.value }] }));
  return (
    <Pressable
      {...props}
      style={hostStyle}
      onPressIn={(e) => {
        scale.value = withTiming(SCALE.press, DOWN);
        onPressIn?.(e);
      }}
      onPressOut={(e) => {
        scale.value = withTiming(1, UP);
        onPressOut?.(e);
      }}
    >
      <Reanimated.View style={[style, pressed]}>{children}</Reanimated.View>
    </Pressable>
  );
}
