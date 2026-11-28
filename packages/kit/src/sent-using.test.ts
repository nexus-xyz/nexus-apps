import { describe, expect, it } from "vitest";
import { MESSAGE_FOOTERS, NEXUS_URL, postWeight } from "./sent-using";

describe("Sent using Nexus", () => {
  it("links Nexus on every messaging connector, and limits only X", () => {
    expect(MESSAGE_FOOTERS.slack?.line).toBe(`_Sent using <${NEXUS_URL}|Nexus>_`);
    for (const [connector, footer] of Object.entries(MESSAGE_FOOTERS)) {
      expect(footer.line).toContain("Sent using");
      expect(footer.limit !== undefined).toBe(connector === "x");
    }
  });

  it("weighs a post as X does: Latin 1, other scripts 2, a link at least 23", () => {
    expect(postWeight("On my way")).toBe(9);
    expect(postWeight("日本")).toBe(4);
    expect(postWeight("see https://x.co")).toBe(4 + 23);
    expect(postWeight(`see https://example.com/${"a".repeat(40)}`)).toBe(4 + 20 + 40);
  });
});
