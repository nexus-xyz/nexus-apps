import { describe, expect, it } from "vitest";
import { haloOf, mix, waitingLook, withAlpha, workingLook } from "./composer-halo";

describe("the composer's halo", () => {
  it("waits in ice on a new chat until its first message, as Nexus UI's chat home", () => {
    expect(haloOf({ busy: false, chat: null, sent: 0 })).toBe("waiting");
    expect(haloOf({ busy: false, chat: null, sent: 1 })).toBeNull();
    expect(haloOf({ busy: false, chat: "chs_1", sent: 0 })).toBeNull();
  });

  it("breathes while an answer comes in, the first one included", () => {
    expect(haloOf({ busy: true, chat: null, sent: 1 })).toBe("working");
    expect(haloOf({ busy: true, chat: "chs_1", sent: 4 })).toBe("working");
  });

  it("mixes as CSS's color-mix in sRGB", () => {
    expect(withAlpha("#5ac8f0", 0.12)).toBe("rgba(90,200,240,0.12)");
    expect(mix("#ffffff", "#000000", 0.5)).toBe("#808080");
    expect(mix("#5ac8f0", "#e9ecea", 1)).toBe("#5ac8f0");
    expect(() => withAlpha("ice", 1)).toThrow();
  });

  it("draws Nexus UI's shadows: the ice edge brightens while you type", () => {
    const idle = waitingLook("#5ac8f0", "#e9ecea", false);
    expect(idle.boxShadow).toBe("0 0 0 3px rgba(90,200,240,0.12), 0 0 28px -6px rgba(90,200,240,0.45)");
    expect(idle.borderColor).toBe(mix("#5ac8f0", "#e9ecea", 0.55));
    expect(waitingLook("#5ac8f0", "#e9ecea", true).borderColor).toBe(mix("#5ac8f0", "#e9ecea", 0.85));
    expect(workingLook("#e9ecea")).toEqual({ boxShadow: "0 0 0 3px rgba(233,236,234,0.08), 0 0 30px -4px rgba(233,236,234,0.32)" });
  });
});
