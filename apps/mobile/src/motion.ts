import { LayoutAnimation } from "react-native";
import { Easing, ReduceMotion } from "react-native-reanimated";

/**
 * Brand v5's motion on the phone (Nexus Mobile M35–M37): every duration, curve, scale and spring in one place, so a
 * change here moves the whole app together. A screen never invents its own number.
 *
 * The rules. Frequent things (typing, streaming, search, repeated taps) do not move. Enter and leave on a strong
 * ease-out, move what is already on screen ease-in-out, never ease-in. Exits are quicker than enters and travel a
 * nudge, not the full height. Animate transform and opacity only. Never grow from scale 0. Grow from what was touched.
 * Anything a finger can reverse runs on a spring. System pieces (context menu, alerts, switches, push, Android's
 * sheet) keep their own motion. Under Reduce Motion nothing travels or scales: a short fade at most, and loops stop.
 */
export const DURATION = {
  /** A press going down. */
  press: 120,
  /** A press coming back, a little slower. */
  release: 160,
  /** A colour, a line fading in, a small reveal; a menu growing from its chip. */
  fast: 180,
  /** A card or a toast entering. UI motion stays under 300 ms. */
  base: 250,
  /** A row leaving a list: the gap closes. */
  collapse: 200,
  /** Anything leaving. */
  exit: 150,
} as const;

/** The brand's curve, cubic-bezier(.2,.7,.2,1): out fast, in soft. */
export const EASE = Easing.bezier(0.2, 0.7, 0.2, 1);
/** A press, and a menu growing: down at once, back softly. */
export const EASE_PRESS = Easing.bezier(0.23, 1, 0.32, 1);

/** How small a thing starts or presses to: never 0, never below 0.95. */
export const SCALE = { press: 0.96, enter: 0.95 } as const;

/** How far a toast rises as it comes and sinks as it goes: an exit is a nudge. */
export const TRAVEL = { rise: 12, sink: 8 } as const;

/** iOS's own sheet: in fast, out faster. Android draws Material's sheet with its own motion. */
export const SHEET = { in: 360, out: 220, easing: Easing.bezier(0.32, 0.72, 0, 1) } as const;

/**
 * Where a finger drives it (the mode switch's thumb): critically damped, so it stops with no bounce, and a second tap
 * mid-slide turns it around from where it is, at its speed.
 */
export const SPRING = { duration: 320, dampingRatio: 1 } as const;

/**
 * The sidebar on iOS, as Claude's app moves it. The chat is a card that slides off the sidebar, which stays in place.
 * The card takes the screen's own corners and a hairline, and its content fades under a scrim of its own colour.
 * Behind it, the sidebar comes forward: it grows from 0.97 as its shade lifts. The library's spring carries the card
 * (patched in `patches/react-native-drawer-layout+*.patch`: critically damped, ω ≈ 24.5). Under Reduce Motion nothing
 * scales.
 */
export const DRAWER = {
  /**
   * How far into the slide (6 pt of 320) the card has the screen's own corners (`display-corner.ts`) and its hairline.
   * At rest the screen's corners hide them; past this they read as the screen itself sliding away.
   */
  roundBy: 0.02,
  /** How much of the card's own colour covers its content when the sidebar is open. */
  fade: 0.5,
  /** The sidebar's scale while the card covers it. */
  recede: 0.97,
  /** The black shade on the sidebar while the card covers it, in dark and in light. */
  shade: { dark: 0.3, light: 0.1 },
} as const;

/** An icon that swaps for another in place (Send ↔ Stop): it grows from 0.3 on a spring with no bounce. */
export const SWAP = { from: 0.3, duration: 350, dampingRatio: 1 } as const;

/**
 * A new chat's greeting and suggestions as the mode changes: what goes fades out fast; what is new comes 4 pt up once
 * that is mostly gone, so the two never read over each other. The dictation capsule's contents follow it the same way.
 */
export const FOLLOW = { out: 120, in: 220, after: 90, rise: 4 } as const;

/** The working halo's breath: from 15 % to full and back, 3 s each way, on a sine. */
export const BREATH = { low: 0.15, half: 3000, easing: Easing.bezier(0.37, 0, 0.63, 1) } as const;
/** "Thinking" and the dictation's dot: from full to 35 % and back, 700 ms each way. Stops on the first word. */
export const PULSE = { low: 0.35, half: 700, easing: Easing.linear } as const;
/** A running mark or ring: one turn a second. */
export const SPIN = { turn: 1000, easing: Easing.linear } as const;
/** Loading: nothing for 300 ms, so a fast load never flashes; then a sheen every 1.8 s; words past 3 s. */
export const SKELETON = { after: 300, sweep: 1800, words: 3000, easing: Easing.bezier(0.4, 0, 0.2, 1) } as const;

/**
 * The Assistant's thread (`assistant-bubbles.tsx`). Your bubble rises from the composer on a spring with no bounce.
 * Nexus's words are plain text, so they only fade in where they stand. Under Reduce Motion both appear in place.
 */
export const BUBBLE = {
  rise: { from: 36, duration: 380, dampingRatio: 1 },
  arrive: { duration: 280, dampingRatio: 1 },
} as const;

/** Timing options for Reanimated, following the system's Reduce Motion. */
export const timing = (duration: number, easing = EASE) => ({ duration, easing, reduceMotion: ReduceMotion.System });

/** Spring options for Reanimated, following the system's Reduce Motion. */
export const spring = ({ duration, dampingRatio }: { duration: number; dampingRatio: number }) => ({
  duration,
  dampingRatio,
  reduceMotion: ReduceMotion.System,
});

/**
 * The next list change animates: a row that leaves fades while the rows under it close the gap, on ease-in-out (it is
 * already on screen). Under Reduce Motion (`reduce`), it just happens. Call it right before the state change.
 */
export function collapseNext(reduce: boolean) {
  if (!reduce) LayoutAnimation.configureNext(LayoutAnimation.create(DURATION.collapse, LayoutAnimation.Types.easeInEaseOut, LayoutAnimation.Properties.opacity));
}
