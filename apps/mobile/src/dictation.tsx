import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Keyboard, Linking, Pressable, StyleSheet, Text, View } from "react-native";
import { haptic } from "@/haptics";
import { requestRecordingPermissionsAsync, setAudioModeAsync, useAudioStream, type AudioStreamBuffer } from "expo-audio";
import Reanimated, {
  cancelAnimation,
  FadeIn,
  ReduceMotion,
  useAnimatedStyle,
  useReducedMotion,
  useSharedValue,
  withRepeat,
  withTiming,
  type EntryAnimationsValues,
  type ExitAnimationsValues,
} from "react-native-reanimated";
import {
  clock,
  compose,
  createDictation,
  NOTICE_TEXT,
  PROBLEM_TEXT,
  resample,
  spoken,
  TARGET_RATE,
  type Dictation,
  type DictationProblem,
  type DictationState,
} from "@nexus/kit/dictation";
import { useCore } from "@/auth";
import { Icon } from "@/icon";
import { DURATION, EASE_PRESS, FOLLOW, PULSE, SPIN } from "@/motion";
import { load, save } from "@/store";
import { font, mono, text, type Theme } from "@/theme";

/**
 * Dictation in the composer (canvas M38, Nexus UI's P15·B): the mic before Send types what you say. `@nexus/kit/dictation`
 * runs it (the Gateway's token, AssemblyAI's socket, the words); this file gives it the phone's microphone, through
 * expo-audio's PCM stream, and draws the button. Idle, a quiet mic. Listening, a capsule in place of the chips: a red
 * dot, the voice as bars, the time and a stop circle. Finishing, a turning ring. Refused, a slashed mic and one line.
 */

/** The device showed the notice to a dictation that began (a `pref:` key: it stays on sign-out). */
const NOTICE_KEY = "pref:dictation-notice";
/** How long the notice stays after dictation ends. */
const NOTICE_LINGER_MS = 6_000;
/** The mic's circle, which the capsule grows from and shrinks back to. */
const MIC = 34;
/** How many bars the capsule shows, and how often a new one comes in: about a tenth of a second each. */
const BARS = 36;
const BAR_EVERY_MS = 90;

export interface DictationView {
  state: DictationState;
  /** Whole seconds since it began listening. */
  seconds: number;
  /** The last levels, 0 to 1, oldest first. */
  bars: number[];
  problem: DictationProblem | undefined;
  /** The one-line notice of the first press on this phone. */
  notice: boolean;
  /** Starts with the box's text and selection, or stops when it listens. */
  toggle: (value: string, selection: { start: number; end: number }) => void;
  /** Stops now and gives the box's text with every word said so far, or `undefined` when it was not dictating. */
  finish: () => string | undefined;
}

/** The phone's refusal, named as the browser names it, so `problemOf` reads both. */
const refused = () => Object.assign(new Error("microphone refused"), { name: "NotAllowedError" });

/**
 * The mic's state for one composer. `onText` gets the box's whole text with the dictated words in it, and where the caret
 * goes: the end of the words, so the next dictation (or typing) continues there.
 */
export function useDictation(onText: (text: string, caret: number) => void): DictationView {
  const core = useCore();
  const [state, setState] = useState<DictationState>("idle");
  const [seconds, setSeconds] = useState(0);
  const [bars, setBars] = useState<number[]>(() => Array<number>(BARS).fill(0));
  const [problem, setProblem] = useState<DictationProblem>();
  const [notice, setNotice] = useState(false);
  const latest = useRef({ core, onText });
  useEffect(() => {
    latest.current = { core, onText };
  });
  // The text around the caret as dictation began: the words go between them. `last` is the box with the words in it.
  const around = useRef({ before: "", after: "" });
  const last = useRef<string | undefined>(undefined);
  const lastBar = useRef(0);
  // Where a chunk from the microphone goes while it listens.
  const chunk = useRef<((pcm: Int16Array) => void) | undefined>(undefined);
  const options = useMemo(
    () => ({
      sampleRate: TARGET_RATE,
      channels: 1,
      encoding: "int16" as const,
      onBuffer: (buffer: AudioStreamBuffer) => chunk.current?.(resample(new Int16Array(buffer.data), buffer.sampleRate)),
    }),
    [],
  );
  const { stream } = useAudioStream(options);
  const engine = useRef<Dictation>(undefined);

  const make = useCallback(() => {
    engine.current ??= createDictation(
      {
        token: () => latest.current.core?.createVoiceToken() ?? Promise.reject(Object.assign(new Error("signed out"), { status: 503 })),
        microphone: async (next) => {
          const { granted } = await requestRecordingPermissionsAsync();
          if (!granted) throw refused();
          await setAudioModeAsync({ allowsRecording: true, playsInSilentMode: true });
          chunk.current = next;
          await stream.start();
          return () => {
            chunk.current = undefined;
            stream.stop();
            // Recording ends, so other apps' audio and the speaker come back.
            void setAudioModeAsync({ allowsRecording: false }).catch(() => undefined);
          };
        },
        socket: (url) => new WebSocket(url),
      },
      {
        state: (next) => {
          setState(next);
          if (next === "listening") {
            setSeconds(0);
            haptic("send");
          }
          if (next === "idle") setBars(Array<number>(BARS).fill(0));
        },
        text: (words) => {
          const said = spoken(words);
          if (!said) return;
          const { before, after } = around.current;
          const next = compose(before, said, after);
          last.current = next.text;
          latest.current.onText(next.text, next.caret);
        },
        level: (value) => {
          const now = Date.now();
          if (now - lastBar.current < BAR_EVERY_MS) return;
          lastBar.current = now;
          setBars((old) => [...old.slice(1), value]);
        },
        problem: setProblem,
      },
    );
    return engine.current;
  }, [stream]);

  const toggle = useCallback(
    (value: string, selection: { start: number; end: number }) => {
      const dictation = make();
      if (state === "listening" || state === "starting") {
        haptic("send");
        void dictation.stop();
        return;
      }
      if (state === "finishing") return;
      const from = Math.min(selection.start, value.length);
      const to = Math.min(Math.max(selection.end, from), value.length);
      around.current = { before: value.slice(0, from), after: value.slice(to) };
      last.current = undefined;
      setProblem(undefined);
      if (!load<boolean>(NOTICE_KEY)) setNotice(true);
      // The words are the input now: the keyboard steps aside so the chat shows.
      Keyboard.dismiss();
      void dictation.start();
    },
    [make, state],
  );

  const finish = useCallback(() => {
    if (state === "idle") return undefined;
    engine.current?.cancel();
    return last.current;
  }, [state]);

  // The phone remembers the notice only once a dictation has really begun.
  useEffect(() => {
    if (notice && state === "listening") save(NOTICE_KEY, true);
  }, [notice, state]);
  // The notice stays while it dictates, and a few seconds after.
  useEffect(() => {
    if (!notice || state !== "idle") return;
    const gone = setTimeout(() => setNotice(false), NOTICE_LINGER_MS);
    return () => clearTimeout(gone);
  }, [notice, state]);
  // One tick a second while it listens, for the "0:07" in the capsule.
  useEffect(() => {
    if (state !== "listening") return;
    const tick = setInterval(() => setSeconds((value) => value + 1), 1000);
    return () => clearInterval(tick);
  }, [state]);
  // Leaving the chat stops the microphone.
  useEffect(() => () => engine.current?.cancel(), []);

  return { state, seconds, bars, problem, notice, toggle, finish };
}

/** Whether the capsule takes the chips' place. */
export const dictating = (view: DictationView) => view.state === "listening" || view.state === "starting";

/** The capsule grows to the left out of the mic's circle (scaleX from the right edge), or fades in under Reduce Motion. */
function grow(reduce: boolean) {
  return (values: EntryAnimationsValues) => {
    "worklet";
    const from = Math.min(1, MIC / Math.max(values.targetWidth, 1));
    return {
      initialValues: { opacity: reduce ? 0 : 1, transform: [{ scaleX: reduce ? 1 : from }] },
      animations: {
        opacity: withTiming(1, { duration: DURATION.fast }),
        transform: [{ scaleX: withTiming(1, { duration: DURATION.base, easing: EASE_PRESS }) }],
      },
    };
  };
}

/** The capsule shrinks back into the mic, faster than it grew. */
function shrink(reduce: boolean) {
  return (values: ExitAnimationsValues) => {
    "worklet";
    const to = Math.min(1, MIC / Math.max(values.currentWidth, 1));
    return {
      initialValues: { opacity: 1, transform: [{ scaleX: 1 }] },
      animations: {
        opacity: withTiming(0, { duration: reduce ? DURATION.exit : DURATION.fast }),
        transform: [{ scaleX: withTiming(reduce ? 1 : to, { duration: DURATION.fast, easing: EASE_PRESS }) }],
      },
    };
  };
}

/** While it listens: the capsule in place of the chips. A tap anywhere on it stops. */
export function MicMeter({ view, theme, onPress }: { view: DictationView; theme: Theme; onPress: () => void }) {
  const reduce = useReducedMotion();
  const live = view.state === "listening";
  const entering = useMemo(() => grow(reduce), [reduce]);
  const exiting = useMemo(() => shrink(reduce), [reduce]);
  return (
    <Pressable
      style={styles.meter}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel="Stop dictating"
      accessibilityValue={{ text: live ? clock(view.seconds) : "Starting" }}
      accessibilityState={{ selected: true, busy: !live }}
    >
      <Reanimated.View style={[styles.capsule, { backgroundColor: theme.fill }]} entering={entering} exiting={exiting} />
      <Reanimated.View style={styles.meterBody} entering={FadeIn.delay(FOLLOW.after).duration(FOLLOW.in).reduceMotion(ReduceMotion.System)}>
        <Dot theme={theme} />
        <Wave bars={view.bars} live={live} theme={theme} />
        <Text style={[styles.time, { color: theme.muted }]}>{clock(live ? view.seconds : 0)}</Text>
        <View style={[styles.stop, { backgroundColor: theme.thumb }]}>
          <View style={[styles.stopMark, { backgroundColor: theme.foreground }]} />
        </View>
      </Reanimated.View>
    </Pressable>
  );
}

/** The red dot: it pulses while it listens, and holds still under Reduce Motion. */
function Dot({ theme }: { theme: Theme }) {
  const reduce = useReducedMotion();
  const opacity = useSharedValue(1);
  useEffect(() => {
    if (reduce) return;
    opacity.value = withRepeat(withTiming(PULSE.low, { duration: PULSE.half, easing: PULSE.easing }), -1, true);
    return () => cancelAnimation(opacity);
  }, [opacity, reduce]);
  const style = useAnimatedStyle(() => ({ opacity: opacity.value }));
  return <Reanimated.View style={[styles.dot, { backgroundColor: theme.danger }, style]} />;
}

/** The voice as bars that move left. Until the socket opens they are flat and breathe, so the press answers at once. */
function Wave({ bars, live, theme }: { bars: number[]; live: boolean; theme: Theme }) {
  const reduce = useReducedMotion();
  const breath = useSharedValue(1);
  useEffect(() => {
    if (live || reduce) {
      breath.value = 1;
      return;
    }
    breath.value = withRepeat(withTiming(PULSE.low, { duration: PULSE.half, easing: PULSE.easing }), -1, true);
    return () => cancelAnimation(breath);
  }, [breath, live, reduce]);
  const style = useAnimatedStyle(() => ({ opacity: breath.value }));
  return (
    <Reanimated.View style={[styles.wave, style]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {bars.map((value, i) => (
        <View key={i} style={[styles.bar, { backgroundColor: theme.foreground, height: live ? 4 + Math.min(1, value * 1.8) * 16 : 4 }]} />
      ))}
    </Reanimated.View>
  );
}

/** The mic when it does not listen: quiet, a turning ring while the last words come in, slashed in red when refused. */
export function MicButton({ view, theme, onPress, onHold, disabled }: {
  view: DictationView;
  theme: Theme;
  onPress: () => void;
  onHold?: () => void;
  disabled?: boolean;
}) {
  const finishing = view.state === "finishing";
  const blocked = view.problem === "denied" || view.problem === "no-microphone";
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={finishing ? "Finishing" : "Dictate"}
      accessibilityHint={onHold ? "Tap to dictate. Touch and hold to start voice chat." : "Speak, and Nexus types it into your message"}
      accessibilityActions={onHold ? [{ name: "voiceChat", label: "Start voice chat" }] : undefined}
      onAccessibilityAction={(event) => { if (event.nativeEvent.actionName === "voiceChat") onHold?.(); }}
      accessibilityState={{ disabled: disabled || finishing, busy: finishing }}
      style={({ pressed }) => [styles.micTarget, pressed && styles.pressed]}
      onPress={onPress}
      onLongPress={onHold}
      disabled={disabled || finishing}
    >
      <View style={[styles.mic, finishing && { backgroundColor: theme.fill }]}>
        {finishing ? <Ring theme={theme} /> : <Icon name={blocked ? "micOff" : "mic"} size={18} color={blocked ? theme.danger : theme.foreground} />}
      </View>
    </Pressable>
  );
}

/** The ring while the last words come in: it turns once a second, and holds still under Reduce Motion. */
function Ring({ theme }: { theme: Theme }) {
  const reduce = useReducedMotion();
  const turn = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    turn.value = withRepeat(withTiming(360, { duration: SPIN.turn, easing: SPIN.easing }), -1, false);
    return () => cancelAnimation(turn);
  }, [turn, reduce]);
  const style = useAnimatedStyle(() => ({ transform: [{ rotate: `${turn.value}deg` }] }));
  return <Reanimated.View style={[styles.ring, { borderColor: theme.border, borderTopColor: theme.foreground }, style]} />;
}

/** The one line above the chips: why it cannot dictate, or the first-press notice. */
export function MicNote({ view, theme }: { view: DictationView; theme: Theme }) {
  const line = view.problem ? PROBLEM_TEXT[view.problem] : view.notice ? NOTICE_TEXT : undefined;
  if (!line) return null;
  return (
    <Text style={[styles.note, { color: theme.muted }]} accessibilityRole="text" accessibilityLiveRegion="polite">
      {line}
      {view.problem === "denied" ? (
        <>
          {" "}
          <Text style={[styles.link, { color: theme.foreground }]} onPress={() => void Linking.openSettings()} accessibilityRole="link">
            Open Settings
          </Text>
        </>
      ) : null}
    </Text>
  );
}

const styles = StyleSheet.create({
  // The circle in a 44 pt target for VoiceOver, TalkBack and touch alike. The target reaches 5 pt under the send
  // button's, so the circle keeps its place: 5 pt from what is before it, touching the send button's target.
  micTarget: { width: 44, height: 44, alignItems: "center", justifyContent: "center", marginRight: -5 },
  mic: { width: MIC, height: MIC, borderRadius: MIC / 2, alignItems: "center", justifyContent: "center" },
  pressed: { opacity: 0.5 },
  meter: { flex: 1, height: MIC, marginVertical: 5, justifyContent: "center" },
  capsule: { ...StyleSheet.absoluteFill, borderRadius: MIC / 2, transformOrigin: "right center" },
  meterBody: { flex: 1, flexDirection: "row", alignItems: "center", gap: 10, paddingLeft: 13, paddingRight: 4 },
  dot: { width: 7, height: 7, borderRadius: 3.5 },
  wave: { flex: 1, height: 22, flexDirection: "row", alignItems: "center", justifyContent: "flex-end", gap: 2.5, overflow: "hidden" },
  bar: { width: 2.5, borderRadius: 1.25, opacity: 0.85 },
  time: { fontFamily: mono, fontSize: 12, fontVariant: ["tabular-nums"] },
  stop: { width: 26, height: 26, borderRadius: 13, alignItems: "center", justifyContent: "center" },
  stopMark: { width: 9, height: 9, borderRadius: 2 },
  ring: { width: 13, height: 13, borderRadius: 6.5, borderWidth: 1.5 },
  note: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 10, paddingBottom: 4 },
  link: { fontFamily: font.medium, textDecorationLine: "underline" },
});
