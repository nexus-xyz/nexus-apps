import { describe, expect, it } from "vitest";
import { fromError } from "./diagnostics";
import { TEST_ERROR, testCrashKind } from "./test-crash";

const device = { appVersion: "0.1.0", appBuild: "42", platform: "ios", osVersion: "27.0" } as const;

describe("the forced test crash", () => {
  it("reads the link's kind, and defaults to a JavaScript error", () => {
    expect(testCrashKind("native")).toBe("native");
    expect(testCrashKind("js")).toBe("js");
    expect(testCrashKind(undefined)).toBe("js");
    expect(testCrashKind(["native", "js"])).toBe("native");
  });

  it("refuses a kind it does not know", () => {
    expect(testCrashKind("anr")).toBeNull();
  });

  it("keeps the test error's whole message through redaction, so the log query can find it", () => {
    const report = fromError(new Error(TEST_ERROR), true, device);
    expect(report.message).toBe(TEST_ERROR);
    expect(report.fatal).toBe(true);
  });
});
