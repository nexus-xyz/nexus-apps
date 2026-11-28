import { useEffect, useState } from "react";
import { StyleSheet, Text, View, type DimensionValue } from "react-native";
import Reanimated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import Svg, { Defs, LinearGradient, Rect, Stop } from "react-native-svg";
import { SKELETON } from "@/motion";
import { font, text, useTheme } from "@/theme";

/**
 * Loading, as brand v5's P44 shows it: nothing for the first 300 ms (most answers come by then, and a flash of grey
 * reads as a flicker); then the content's shape in the fill, a sheen sweeping across it; past 3 s, words under it.
 * The shape is the loaded layout's, so nothing moves when the content comes. Reduce Motion holds the sheen still.
 */

/** Whether `ms` has passed since the first render. */
function useAfter(ms: number) {
  const [past, setPast] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setPast(true), ms);
    return () => clearTimeout(timer);
  }, [ms]);
  return past;
}

/** One bar of the shape, its sheen sweeping left to right. */
export function Bone({ width = "100%", height = 12, radius = 6 }: { width?: DimensionValue; height?: number; radius?: number }) {
  const theme = useTheme();
  const reduce = useReducedMotion();
  const [room, setRoom] = useState(0);
  const sweep = useSharedValue(0);
  useEffect(() => {
    if (reduce || !room) return;
    sweep.value = withRepeat(withTiming(1, { duration: SKELETON.sweep, easing: SKELETON.easing }), -1, false);
    return () => cancelAnimation(sweep);
  }, [reduce, room, sweep]);
  // A sheen 40 % of the bar wide, from wholly left of it to wholly right.
  const moving = useAnimatedStyle(() => ({ transform: [{ translateX: -room * 0.4 + sweep.value * room * 1.4 }] }));
  return (
    <View style={[styles.bone, { width, height, borderRadius: radius, backgroundColor: theme.fill }]} onLayout={(e) => setRoom(e.nativeEvent.layout.width)}>
      {room && !reduce ? (
        <Reanimated.View style={[styles.sheen, { width: room * 0.4 }, moving]}>
          <Svg width="100%" height="100%">
            <Defs>
              <LinearGradient id="sheen" x1="0" y1="0" x2="1" y2="0">
                <Stop offset="0" stopColor={theme.card} stopOpacity={0} />
                <Stop offset="0.5" stopColor={theme.card} stopOpacity={0.55} />
                <Stop offset="1" stopColor={theme.card} stopOpacity={0} />
              </LinearGradient>
            </Defs>
            <Rect width="100%" height="100%" fill="url(#sheen)" />
          </Svg>
        </Reanimated.View>
      ) : null}
    </View>
  );
}

// The rows' bars, at widths that read as text rather than a grid.
const WIDTHS: DimensionValue[] = ["62%", "48%", "70%", "55%"];

/**
 * A card of rows in the shape of a settings list's (`list.tsx`): a tile, a line and a shorter one under it, 60 pt a
 * row. `rows` is how many the loaded list is likely to have; `bare` draws the rows alone, inside a section's own card.
 */
export function SkeletonRows({ rows = 4, bare }: { rows?: number; bare?: boolean }) {
  const theme = useTheme();
  const shown = useAfter(SKELETON.after);
  const slow = useAfter(SKELETON.words);
  return (
    <View accessible accessibilityLabel="Loading" accessibilityState={{ busy: true }}>
      {/* The card keeps its room from the start, so the page does not move when it shows. */}
      <View style={[styles.card, { backgroundColor: shown && !bare ? theme.card : "transparent" }]}>
        {shown
          ? Array.from({ length: rows }, (_, n) => (
              <View key={n} style={styles.row}>
                <Bone width={30} height={30} radius={8} />
                <View style={styles.lines}>
                  <Bone width={WIDTHS[n % WIDTHS.length]} />
                  <Bone width="34%" height={10} />
                </View>
              </View>
            ))
          : Array.from({ length: rows }, (_, n) => <View key={n} style={styles.row} />)}
      </View>
      {slow ? <Text style={[styles.words, { color: theme.tertiary }]}>Still loading…</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  bone: { overflow: "hidden" },
  sheen: { position: "absolute", top: 0, bottom: 0, left: 0 },
  card: { borderRadius: 12, borderCurve: "continuous", overflow: "hidden" },
  row: { flexDirection: "row", alignItems: "center", gap: 12, height: 60, paddingHorizontal: 16 },
  lines: { flex: 1, gap: 8 },
  words: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 16, paddingTop: 8 },
});
