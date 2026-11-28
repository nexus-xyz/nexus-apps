import { describe, expect, it } from "vitest";
import { displayCornerRadius, FALLBACK } from "./display-corner";

describe("the screen's corner radius", () => {
  it("matches the phone's own corners, as the sidebar's card rounds to them", () => {
    expect(displayCornerRadius(402, 874, true)).toBe(62);
    expect(displayCornerRadius(393, 852, true)).toBe(55);
    expect(displayCornerRadius(390, 844, true)).toBe(47.33);
  });

  it("reads the size the same way in landscape", () => {
    expect(displayCornerRadius(874, 402, true)).toBe(62);
  });

  it("is square on a phone with a Home button, and a safe guess on a size it does not know", () => {
    expect(displayCornerRadius(375, 667, false)).toBe(0);
    expect(displayCornerRadius(1024, 1366, true)).toBe(FALLBACK);
  });
});
