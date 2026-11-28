import { useEffect, useRef, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import Reanimated, { useAnimatedStyle, useSharedValue, withSpring } from "react-native-reanimated";
import { SPRING } from "@/motion";
import { haptic } from "@/haptics";
import { ripple } from "@/press";
import { font, text, useTheme } from "@/theme";

/**
 * Two to four choices side by side on Android: a 40 dp pill track with the current choice on a thumb that slides to the
 * next on the mode switch's spring (Reanimated jumps it under Remove animations). The app's own control, as on iOS
 * (`segmented.tsx`), not Material's outlined buttons with a check.
 */
export function Segmented<T extends string>({ label, options, value, onChange }: { label: string; options: { id: T; label: string }[]; value: T; onChange: (v: T) => void }) {
  const theme = useTheme();
  const index = Math.max(0, options.findIndex((o) => o.id === value));
  const [width, setWidth] = useState(0);
  const segment = width ? (width - PAD * 2) / options.length : 0;
  const position = useSharedValue(index);
  const placed = useRef(false);
  useEffect(() => {
    if (!segment) return;
    // Placed where it is on the first layout; slid after.
    position.value = placed.current ? withSpring(index, SPRING) : index;
    placed.current = true;
  }, [index, segment, position]);
  const thumb = useAnimatedStyle(() => ({ width: segment, transform: [{ translateX: position.value * segment }] }));

  return (
    <View style={[styles.track, { backgroundColor: theme.fill }]} onLayout={(e) => setWidth(e.nativeEvent.layout.width)} accessibilityLabel={label}>
      {segment ? <Reanimated.View style={[styles.thumb, { backgroundColor: theme.thumb }, thumb]} /> : null}
      {options.map((o) => {
        const selected = o.id === value;
        return (
          <Pressable
            key={o.id}
            style={styles.segment}
            android_ripple={ripple(theme)}
            onPress={() => {
              if (selected) return;
              haptic("selection");
              onChange(o.id);
            }}
            accessibilityRole="button"
            accessibilityState={{ selected }}
            accessibilityLabel={`${label}: ${o.label}`}
          >
            <Text style={[styles.label, { color: selected ? theme.foreground : theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={1.3}>
              {o.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const PAD = 4;
const styles = StyleSheet.create({
  track: { flexDirection: "row", height: 40, borderRadius: 20, padding: PAD },
  thumb: { position: "absolute", top: PAD, bottom: PAD, left: PAD, borderRadius: 16, elevation: 1 },
  // Each segment clips its ripple to the thumb's shape.
  segment: { flex: 1, borderRadius: 16, overflow: "hidden", alignItems: "center", justifyContent: "center", paddingHorizontal: 8 },
  label: { ...text.sm, fontFamily: font.medium },
});
