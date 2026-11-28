import type { ConnectorSummary } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { briefLabel, briefTime, whyActions } from "./brief-words";
import { connectRows, keepsLine, newsTarget, seenIds } from "./discover-words";

// Discover (A15) and the morning brief (A1, A10·b) on the phone.

const connector = (name: string, more: Partial<ConnectorSummary> = {}): ConnectorSummary =>
  ({ name, description: `Read your ${name}. More words.`, operations: [], connects: ["user"], offered: true, ...more }) as ConnectorSummary;

describe("Connect more", () => {
  it("lists what each account unlocks, the unconnected first, and leaves out what is off for the person", () => {
    const rows = connectRows(
      [connector("linear"), connector("gmail"), connector("acme-crm"), connector("slack", { usable: false }), connector("jira", { connects: ["org"] })],
      [{ connector: "linear", scope: "user" }],
    );
    expect(rows.map((row) => [row.connector, row.connected])).toEqual([
      ["gmail", false],
      ["acme-crm", false],
      ["linear", true],
    ]);
    expect(rows[0]!.line).toBe("Unlocks: Summarize your mail and draft replies");
    expect(rows[1]!.line).toBe("Unlocks: Read your acme-crm");
    expect(rows[2]!.line).toBe("Already connected: list and track your open issues");
  });

  it("lists six at most", () => {
    expect(connectRows(Array.from({ length: 9 }, (_, i) => connector(`c${i}`)), [])).toHaveLength(6);
  });
});

describe("What's new and Teach it about you", () => {
  it("takes each line to its place on the phone", () => {
    expect(newsTarget({ kind: "view", view: "memory" })).toEqual({ href: "/assistant-memory" });
    expect(newsTarget({ kind: "view", view: "chat" })).toEqual({ href: "/agent" });
    expect(newsTarget({ kind: "ask", line: "Hi" })).toEqual({ href: { pathname: "/agent", params: { ask: "Hi" } } });
    expect(newsTarget({ kind: "view", view: "approvals" }, (path) => `https://nexus.inc/app/acme${path}`)).toEqual({ url: "https://nexus.inc/app/acme/agent?view=approvals" });
    expect(newsTarget({ kind: "view", view: "approvals" })).toBeUndefined();
  });

  it("keeps only the seen ids that are text, and says how many notes it keeps", () => {
    expect(seenIds(["a", 2, "b"])).toEqual(["a", "b"]);
    expect(seenIds("a")).toEqual([]);
    expect(keepsLine("Nexus", 1)).toBe("Nexus keeps 1 note about you.");
    expect(keepsLine("Ava", 38)).toBe("Ava keeps 38 notes about you.");
  });
});

describe("the morning brief", () => {
  const brief = { day: "2026-11-18", at: "2026-11-18T08:02:00", head: "Morning, Maya. Three things:", lines: ["Ana’s birthday is Saturday", "AT&T due Friday: $84.20"], errands: "Dinner Friday needs you" };

  it("says its time and reads as one message", () => {
    expect(briefTime(brief)).toBe("08:02");
    expect(briefTime({ at: "soon" })).toBeUndefined();
    expect(briefLabel(brief)).toBe("Morning, Maya. Three things: Ana’s birthday is Saturday AT&T due Friday: $84.20 Dinner Friday needs you");
  });

  it("offers Fewer like this, a step down the dial, and Turn off its kind", () => {
    const [fewer, off] = whyActions("brief");
    const view = { level: "balanced" as const, muted: [], batchMinutes: 15 } as never;
    expect(fewer!.label).toBe("Fewer like this");
    expect(fewer!.change(view)).toEqual({ level: "quiet" });
    expect(fewer!.done({ level: "quiet", muted: [] } as never)).toBe("Done: Proactive is set to Quiet.");
    expect(off!.label).toBe("Turn off the morning brief");
    expect(off!.change(view)).toEqual({ muted: ["brief"] });
    expect(off!.done(view)).toBe("Done: the morning brief off.");
  });
});
