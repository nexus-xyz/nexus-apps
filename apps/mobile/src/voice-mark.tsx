import { useEffect } from "react";
import { StyleSheet, View } from "react-native";
import Reanimated, {
  cancelAnimation,
  Easing,
  useAnimatedProps,
  useAnimatedStyle,
  useFrameCallback,
  useReducedMotion,
  useSharedValue,
  withRepeat,
  withTiming,
  type SharedValue,
} from "react-native-reanimated";
import Svg, { Path } from "react-native-svg";
import { DURATION, EASE, timing } from "@/motion";
import { MARK_LAYERS, MARK_VIEWBOX } from "@/nexus-mark-paths";
import type { Theme } from "@/theme";
import { fade, follow, FRAME_MS, layerOpacity, markOpacity, ringOpacity, ringPath, restAngle, THINK, type VoiceState } from "@/voice-motion";

const AnimatedPath = Reanimated.createAnimatedComponent(Path);
/** The ring's room: 1.7 times the mark, reserved, so nothing moves when the ring fades in (A25·E). */
const RING = 1.7;
/** The ring's units: the mark is 100 across, so the ring's radius is 85. */
const RADIUS = 85;

interface Clock {
  /** Milliseconds since the mark appeared, a frame at most every 33 ms. */
  now: SharedValue<number>;
  /** When the state began, on that clock. */
  since: SharedValue<number>;
  state: SharedValue<VoiceState>;
  /** The level, smoothed. */
  level: SharedValue<number>;
}

/**
 * The mark is the voice (A25): the F5 mark in one ink, the light carrying the state, and a quiet ring of level ticks
 * while it speaks and listens. `level` is the engine's, 0 to 1. Decorative: the word under it carries the state.
 */
export function VoiceMark({ state, level, size, theme }: { state: VoiceState; level: SharedValue<number>; size: number; theme: Theme }) {
  const reduce = useReducedMotion();
  const clock: Clock = {
    now: useSharedValue(0),
    since: useSharedValue(0),
    state: useSharedValue<VoiceState>(state),
    level: useSharedValue(0),
  };
  const pending = useSharedValue(0);
  const ring = useSharedValue(0);
  const fill = useSharedValue(1);
  const turn = useSharedValue(0);

  useFrameCallback((frame) => {
    pending.value += frame.timeSincePreviousFrame ?? 0;
    if (pending.value < FRAME_MS) return;
    const dt = pending.value;
    pending.value = 0;
    clock.now.value += dt;
    clock.level.value = follow(clock.level.value, level.value, dt, reduce);
    if (clock.state.value === "speaking") level.value = fade(level.value, dt);
  });

  useEffect(() => {
    clock.state.value = state;
    clock.since.value = clock.now.value;
    ring.value = withTiming(ringOpacity(state), timing(DURATION.fast));
    fill.value = withTiming(markOpacity(state), timing(DURATION.fast));
    // Only Thinking turns it. After, it eases forward to rest over 600 ms, from the same angle within one turn.
    cancelAnimation(turn);
    const at = turn.value % 360;
    turn.value = at;
    if (reduce) return;
    turn.value =
      state === "thinking"
        ? withRepeat(withTiming(at + 360, { duration: THINK.turn, easing: Easing.linear }), -1, false)
        : withTiming(restAngle(at), { duration: 600, easing: EASE });
  }, [state, reduce]);

  const turning = useAnimatedStyle(() => ({ opacity: fill.value, transform: [{ rotate: `${turn.value}deg` }] }));
  const ticks = useAnimatedProps(() => ({
    d: ring.value > 0.001 ? ringPath(RADIUS, clock.level.value, clock.now.value, reduce) : "",
    strokeOpacity: ring.value,
  }));
  const box = size * RING;

  return (
    <View style={{ width: box, height: box }} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Svg width={box} height={box} viewBox={`${-RADIUS} ${-RADIUS} ${RADIUS * 2} ${RADIUS * 2}`} style={StyleSheet.absoluteFill}>
        <AnimatedPath animatedProps={ticks} stroke={theme.tertiary} strokeWidth={1.2} strokeLinecap="round" fill="none" />
      </Svg>
      <Reanimated.View style={[styles.mark, { width: size, height: size, top: (box - size) / 2, left: (box - size) / 2 }, turning]}>
        <Svg width={size} height={size} viewBox={MARK_VIEWBOX}>
          {MARK_LAYERS.map((layer, i) => (
            <Layer key={i} index={i} d={layer.d} width={layer.width} ink={theme.foreground} clock={clock} reduce={reduce} />
          ))}
        </Svg>
      </Reanimated.View>
      {state === "needs-you" ? <View style={[styles.dot, { backgroundColor: theme.foreground, top: (box + size) / 2 + 6, left: box / 2 - 3 }]} /> : null}
    </View>
  );
}

/** One layer, its light on its own. */
function Layer({ index, d, width, ink, clock, reduce }: { index: number; d: string; width: number; ink: string; clock: Clock; reduce: boolean }) {
  const props = useAnimatedProps(() => ({
    strokeOpacity: layerOpacity(clock.state.value, index, clock.level.value, clock.now.value - clock.since.value, clock.now.value, reduce),
  }));
  return <AnimatedPath d={d} animatedProps={props} fill="none" stroke={ink} strokeWidth={width} strokeLinejoin="round" />;
}

const styles = StyleSheet.create({
  mark: { position: "absolute" },
  dot: { position: "absolute", width: 6, height: 6, borderRadius: 3 },
});
