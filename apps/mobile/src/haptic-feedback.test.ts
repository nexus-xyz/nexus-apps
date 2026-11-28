import { describe, expect, it, vi } from "vitest";
import { createHapticFeedback, type HapticEffect } from "./haptic-feedback";

describe("haptic feedback", () => {
  it("honors preference and activity changes before every action", () => {
    let enabled = true;
    let active = true;
    let at = 0;
    const play = vi.fn(async (_effect: HapticEffect) => undefined);
    const haptic = createHapticFeedback({ enabled: () => enabled, active: () => active, now: () => at, play });
    haptic("selection");
    enabled = false;
    at += 100;
    haptic("send");
    enabled = true;
    active = false;
    at += 100;
    haptic("success");
    active = true;
    haptic("error");
    expect(play.mock.calls.map(([effect]) => effect)).toEqual(["selection", "error"]);
  });

  it("coalesces overlapping controls but preserves a task outcome", () => {
    let at = 0;
    const play = vi.fn(async (_effect: HapticEffect) => undefined);
    const haptic = createHapticFeedback({ enabled: () => true, active: () => true, now: () => at, play });
    haptic("tap");
    haptic("selection");
    haptic("success");
    haptic("tap");
    haptic("success");
    at = 80;
    haptic("send");
    expect(play.mock.calls.map(([effect]) => effect)).toEqual(["tap", "success", "send"]);
  });

  it("keeps actions usable when native haptics are unavailable", async () => {
    const play = vi.fn(async () => { throw new Error("Unsupported hardware"); });
    const haptic = createHapticFeedback({ enabled: () => true, active: () => true, now: () => 0, play });
    expect(() => haptic("tap")).not.toThrow();
    await Promise.resolve();
    expect(play).toHaveBeenCalledOnce();
  });
});
