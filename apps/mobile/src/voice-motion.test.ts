import { describe, expect, it } from "vitest";
import { fade, follow, layerOpacity, LAYERS, light, markOpacity, restAngle, ringOpacity, ringPath, TICKS } from "./voice-motion";

// The voice mark's motion (A25·E), as plain functions: the numbers of the spec, at chosen times and levels.

describe("the voice mark's motion (A25·E)", () => {
  it("follows the level fast up and slower down; under reduced motion over a second", () => {
    expect(follow(0, 1, 40, false)).toBeCloseTo(1 - Math.exp(-1));
    expect(follow(1, 0, 180, false)).toBeCloseTo(Math.exp(-1));
    expect(follow(0, 1, 40, true)).toBeLessThan(0.05);
    expect(fade(1, 240)).toBeCloseTo(Math.exp(-1));
  });

  it("lights the eye first, from 82 % to full", () => {
    const eye = LAYERS - 1;
    expect(layerOpacity("listening", eye, 1, 0, 0, false)).toBeCloseTo(1);
    expect(layerOpacity("listening", 0, 0, 0, 0, false)).toBeCloseTo(0.82);
    expect(layerOpacity("speaking", eye, 0.5, 0, 0, false)).toBeGreaterThan(layerOpacity("speaking", 0, 0.5, 0, 0, false));
  });

  it("breathes at rest every 5.2 s, still under reduced motion; an interruption drops the light in 600 ms", () => {
    expect(light("ready", 0, 0, 0, false)).toBeCloseTo(light("ready", 0, 0, 5200, false));
    expect(light("ready", 0, 0, 2600, false)).toBeCloseTo(0.5);
    expect(light("ready", 0, 0, 1000, true)).toBe(light("ready", 0, 0, 2000, true));
    expect(light("interrupted", 0, 0, 0, false)).toBeCloseTo(0.5);
    expect(light("interrupted", 0, 600, 0, false)).toBeCloseTo(0.22);
  });

  it("thinks as A13 does: the eye dims first, to 35 %; under reduced motion it holds at 60 %", () => {
    expect(layerOpacity("thinking", LAYERS - 1, 0, 0, 1200, false)).toBeCloseTo(0.35);
    expect(layerOpacity("thinking", 0, 0, 0, 1200 + 700, false)).toBeCloseTo(0.35);
    expect(layerOpacity("thinking", 3, 0, 0, 1234, true)).toBe(0.6);
  });

  it("draws 64 ticks, 3 to 12 units, and a still ring under reduced motion", () => {
    expect(ringPath(85, 0.5, 0, false).match(/M/g)).toHaveLength(TICKS);
    expect(ringPath(85, 0, 0, false)).toContain("M85.0 0.0L82.0 0.0");
    expect(ringPath(85, 1, 123, true)).toBe(ringPath(85, 1, 999, true));
  });

  it("shows the ring only while it speaks or listens, and dims the mark when it can't hear", () => {
    expect([ringOpacity("speaking"), ringOpacity("listening"), ringOpacity("thinking")]).toEqual([0.45, 0.28, 0]);
    expect([markOpacity("cant-hear"), markOpacity("ready")]).toEqual([0.38, 1]);
  });

  it("rests forward at the next sixth of a turn", () => {
    expect([restAngle(0), restAngle(1), restAngle(61), restAngle(359)]).toEqual([0, 60, 120, 360]);
  });
});
