import { describe, expect, it } from "vitest";
import { accountHead, clock, neverLine, pauseText, privacyFailed, shortList } from "./privacy-words";

// The Assistant's Privacy on the phone (A11): an account's row, and what each change says.

describe("Privacy on the phone", () => {
  it("names an account and whose it is, and keeps its lists to one line", () => {
    expect(accountHead({ connector: "gmail", account: "ann@gmail.test", scope: "user" })).toEqual({ name: "Gmail · ann@gmail.test", whose: "Yours" });
    expect(accountHead({ connector: "linear", account: "Acme", scope: "org" }).whose).toBe("Your organization's");
    expect(shortList(["a", "b", "c", "d", "e"])).toBe("a; b; c; 2 more");
    expect(neverLine({ writes: [], never: ["Delete a message"] })).toBe("Delete a message");
    expect(neverLine({ writes: ["Send"], never: [] })).toBe("Any write without your approval");
    expect(neverLine({ writes: [], never: [] })).toBe("Any write");
  });

  it("says what a pause did, by the Assistant's name, and why a call failed", () => {
    expect(pauseText({ paused: true })).toBe("Nexus is paused. Nothing runs until you resume.");
    expect(pauseText({ paused: false }, "Juno")).toBe("Juno runs again.");
    expect(clock(new Date(2026, 10, 17, 9, 5))).toBe("09:05");
    expect(privacyFailed({ kind: "off" })).toBe("This agent service has no Privacy settings yet.");
    expect(privacyFailed({ kind: "failed", failure: { status: 502 } })).toBe("Nexus Gateway did not answer. Nothing changed. Try again in a moment.");
    expect(privacyFailed({ kind: "failed", failure: { status: 400, detail: "Send the confirm." } })).toBe("Send the confirm.");
  });
});
