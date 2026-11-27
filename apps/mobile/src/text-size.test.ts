import { describe, expect, it } from "vitest";
import { isAccessibilitySize } from "./text-size";

describe("isAccessibilitySize", () => {
  it("is false up to iOS's largest standard size, Extra Extra Extra Large", () => {
    for (const scale of [0.82, 1, 1.12, 1.24, 1.35]) expect(isAccessibilitySize(scale)).toBe(false);
  });

  it("is true from AX1, the first of the Larger Accessibility Sizes, to AX5", () => {
    for (const scale of [1.65, 1.94, 2.35, 2.76, 3.12]) expect(isAccessibilitySize(scale)).toBe(true);
  });

  it("is true for Android's two largest font sizes and false below them", () => {
    expect(isAccessibilitySize(1.5)).toBe(false);
    expect(isAccessibilitySize(1.8)).toBe(true);
    expect(isAccessibilitySize(2)).toBe(true);
  });
});
