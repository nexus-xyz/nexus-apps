export type HapticEffect = "tap" | "selection" | "send" | "receive" | "longPress" | "success" | "warning" | "error";

type Environment = {
  enabled: () => boolean;
  active: () => boolean;
  now: () => number;
  play: (effect: HapticEffect) => Promise<void>;
};

const priority = (effect: HapticEffect) => (effect === "error" || effect === "warning" || effect === "success" ? 1 : 0);

/** Coalesces feedback from controls in the same press, while letting an outcome supersede its initial tap. */
export function createHapticFeedback(environment: Environment) {
  let last: { at: number; effect: HapticEffect } | undefined;
  return (effect: HapticEffect) => {
    if (!environment.enabled() || !environment.active()) return;
    const at = environment.now();
    if (last && at - last.at < 80 && priority(effect) <= priority(last.effect)) return;
    last = { at, effect };
    // Feedback is optional on unsupported hardware and must never interrupt the action it confirms.
    void environment.play(effect).catch(() => undefined);
  };
}
