/**
 * A25's motion spec for the voice mark, as worklets: `voice-mark.tsx` runs them on the UI thread at up to 30 frames a
 * second, and the tests run them as plain functions. This slice moves the light only, as A25's reduced motion does;
 * the eye drawing in and breathing is a follow-up. Only Thinking turns the mark.
 */

/** A25's states that the phone draws. Muted waits for the hands-free call, where the mic stays open. */
export type VoiceState = "ready" | "listening" | "thinking" | "speaking" | "interrupted" | "needs-you" | "cant-hear";

/** The mark's eleven layers, rim (0) to eye (10). */
export const LAYERS = 11;
/** The voice ring: 64 ticks on a ring 1.7 times the mark. */
export const TICKS = 64;
/** A frame at most every 33 ms. */
export const FRAME_MS = 33;
/** Thinking: A13's breath, 2.4 s, each layer 70 ms after the one inside it; it turns once in 14 s. */
export const THINK = { breath: 2400, stagger: 70, low: 0.35, turn: 14_000 } as const;

const clamp = (value: number) => {
  "worklet";
  return Math.min(1, Math.max(0, value));
};

/**
 * The level smoothed toward `target` over `dt` ms: attack 40 ms, release 180 ms. Under reduced motion, over 1 s.
 */
export function follow(level: number, target: number, dt: number, reduce: boolean): number {
  "worklet";
  const tau = reduce ? 1000 : target > level ? 40 : 180;
  return level + (target - level) * (1 - Math.exp(-dt / tau));
}

/** A spoken word's pulse fades, so the level follows the words and falls between them. */
export function fade(target: number, dt: number): number {
  "worklet";
  return target * Math.exp(-dt / 240);
}

/** The mark's light, 0 to 1, for a state: `level` is the smoothed voice, `since` the ms in the state, `clock` the ms. */
export function light(state: VoiceState, level: number, since: number, clock: number, reduce: boolean): number {
  "worklet";
  if (state === "listening") return 0.22 + 0.68 * level;
  if (state === "speaking") return 0.3 + 0.6 * level;
  // You spoke over it: the light drops from 0.5 to 0.22 over 600 ms.
  if (state === "interrupted") return 0.5 - 0.28 * Math.min(1, since / 600);
  // At rest: a slow 5.2 s breath. Still under reduced motion.
  if (state === "ready") return reduce ? 0.35 : 0.35 + 0.15 * (1 - Math.cos((2 * Math.PI * clock) / 5200)) / 2;
  // Needs you and can't hear you: still, the light held.
  return 0.5;
}

/** Layer `i`'s opacity: the light reaches the eye first (A25's "light first"); Thinking breathes eye to rim (A13). */
export function layerOpacity(state: VoiceState, i: number, level: number, since: number, clock: number, reduce: boolean): number {
  "worklet";
  if (state === "thinking") {
    if (reduce) return 0.6;
    const phase = (clock - (LAYERS - 1 - i) * THINK.stagger) / THINK.breath;
    return 1 - ((1 - THINK.low) * (1 - Math.cos(2 * Math.PI * phase))) / 2;
  }
  const u = i / (LAYERS - 1);
  return 0.82 + 0.18 * clamp((light(state, level, since, clock, reduce) - 0.35 * (1 - u)) / 0.65);
}

/** The whole mark's opacity: can't hear you is still at 38 %. */
export const markOpacity = (state: VoiceState) => (state === "cant-hear" ? 0.38 : 1);

/** The voice ring's opacity: 45 % while Nexus speaks, 28 % while it hears you, none otherwise. */
export const ringOpacity = (state: VoiceState) => (state === "speaking" ? 0.45 : state === "listening" ? 0.28 : 0);

/**
 * The voice ring as one path, centred on 0,0: 64 ticks that grow inward from `radius`, each 3 to 12 units with the
 * level, on its own slow oscillator, with no rotation and no sweep. Under reduced motion, a still, faint ring.
 */
export function ringPath(radius: number, level: number, clock: number, reduce: boolean): string {
  "worklet";
  let d = "";
  for (let k = 0; k < TICKS; k += 1) {
    const angle = (2 * Math.PI * k) / TICKS;
    const wobble = reduce ? 0 : 0.55 + 0.45 * Math.sin(clock * (0.002 + 0.0007 * (k % 5)) + k * 2.4);
    const inner = radius - (3 + 9 * clamp(level) * wobble);
    const [x, y] = [Math.cos(angle), Math.sin(angle)];
    d += `M${(x * radius).toFixed(1)} ${(y * radius).toFixed(1)}L${(x * inner).toFixed(1)} ${(y * inner).toFixed(1)}`;
  }
  return d;
}

/** Where the turning mark comes to rest: forward to the next sixth of a turn, where it looks as drawn (A13). */
export const restAngle = (angle: number) => Math.ceil(angle / 60) * 60;
