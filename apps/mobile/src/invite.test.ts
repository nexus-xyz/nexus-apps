import { describe, expect, it } from "vitest";
import { invite } from "./invite";

describe("invite", () => {
  it("offers nothing without a referral link (program paused, invitations used up)", () => {
    expect(invite(undefined, true)).toBeNull();
  });

  it("carries the link once, in the message alone", () => {
    expect(invite("https://nexus.inc/r/ab12", true)).toEqual({ message: "Join me on Nexus: https://nexus.inc/r/ab12", text: true });
  });

  it("offers a text only when the phone can send one", () => {
    expect(invite("https://nexus.inc/r/ab12", false)?.text).toBe(false);
  });
});
