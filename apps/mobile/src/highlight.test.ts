import { describe, expect, it, vi } from "vitest";
import { highlight } from "./highlight";

describe("highlight", () => {
  it("colours code the way Nexus UI does, in light and dark", () => {
    const light = highlight("const x = 1;", "ts", false)!;
    expect(light[0]?.map((t) => t.content).join("")).toBe("const x = 1;");
    // `const` in github-light's keyword red, after Nexus UI's AA replacement.
    expect(light[0]?.find((t) => t.content === "const")?.color?.toLowerCase()).toBe("#cf222e");
    expect(highlight("const x = 1;", "typescript", true)![0]?.find((t) => t.content === "const")?.color).not.toBe("#cf222e");
  });

  it("gives a language's first call the time to compile its grammar, and keeps the time limit after it", () => {
    // A clock that jumps a second at each read, so every time-limited line runs out of time. No other test uses Python,
    // so this is its first call.
    let now = 0;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => (now += 1000));
    try {
      expect(highlight("def f(): pass", "python", false)![0]?.find((t) => t.content === "def")?.color?.toLowerCase()).toBe("#cf222e");
      expect(highlight("def f(): pass", "python", false)![0]?.map((t) => t.content)).toEqual(["def f(): pass"]);
    } finally {
      clock.mockRestore();
    }
  });

  it("leaves unknown or missing languages plain", () => {
    expect(highlight("x", "brainfuck", false)).toBeUndefined();
    expect(highlight("x", undefined, false)).toBeUndefined();
  });
});
