import { describe, expect, it } from "vitest";
import { assistantFooter, assistantSections, NOT_YET } from "./assistant-settings";

// Account › Assistant on the phone (A4·B): every section the board draws, in the desktop's order.

const uiLink = (path: string) => `https://nexus.inc/app/acme${path}`;

describe("Account › Assistant", () => {
  it("lists the board's eight sections, with Memory on the phone and Approvals on Nexus UI's Assistant page", () => {
    const sections = assistantSections(uiLink);
    expect(sections.map((s) => [s.label, s.href ?? s.url ?? NOT_YET])).toEqual([
      ["Personality", NOT_YET],
      ["Integrations", "/integrations"],
      ["Channels", NOT_YET],
      ["Proactivity", "/proactivity"],
      ["Approvals", "https://nexus.inc/app/acme/agent?view=approvals"],
      ["Passwords and payments", "/vault"],
      ["Memory", "/assistant-memory"],
      ["Privacy", "/assistant-privacy"],
    ]);
    expect(sections.filter((s) => s.soon).every((s) => s.detail === NOT_YET)).toBe(true);
    expect(assistantFooter(sections)).toBe("Approvals opens in Nexus on the web.");
  });

  it("keeps the web sections in place, waiting, without a link to Nexus UI", () => {
    const sections = assistantSections(undefined);
    expect(sections.some((s) => s.url)).toBe(false);
    expect(sections).toHaveLength(8);
    expect(assistantFooter(sections)).toBeUndefined();
  });
});
