import type { AgentConversationSummary } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { historyAfter, offersHistory, rowLabel, rowLine, surfaceLabel, type History } from "./agent-conversations";

// Nexus's conversations from every surface: the labels, a row's line, and what each read makes of the list.

const NOW = Date.parse("2026-11-09T12:00:00.000Z");
const row = (id: string, minutesAgo: number, more: Partial<AgentConversationSummary> = {}): AgentConversationSummary => ({
  id,
  title: `Title ${id}`,
  surface: "slack",
  updatedAt: new Date(NOW - minutesAgo * 60_000).toISOString(),
  turns: 2,
  preview: `Preview ${id}`,
  ...more,
});

describe("a row", () => {
  it("names each surface as the apps do, and none for an ask that named none", () => {
    expect(["slack", "web", "desktop", "mobile", "cli", "api"].map((s) => surfaceLabel(s as AgentConversationSummary["surface"]))).toEqual(["Slack", "Web", "Desktop", "Phone", "Terminal", undefined]);
  });

  it("says where it started and when, and reads it out in words", () => {
    expect(rowLine(row("a", 120), NOW)).toBe("Slack · 2h");
    expect(rowLine(row("b", 5, { surface: "api" }), NOW)).toBe("5m");
    expect(rowLabel(row("a", 120), NOW)).toBe("Title a, from Slack, 2 hours ago, Preview a");
    expect(rowLabel(row("c", 0, { surface: "cli", title: "", preview: "" }), NOW)).toBe("Untitled, from Terminal, just now");
  });
});

describe("the list on screen", () => {
  const unknown: History = { kind: "unknown" };

  it("shows once the API answers, and hides on a 404 so the screen works as before", () => {
    expect(offersHistory(unknown)).toBe(false);
    const off = historyAfter(unknown, { kind: "off" }, false);
    expect(off).toEqual({ kind: "off" });
    expect(offersHistory(off)).toBe(false);
    expect(offersHistory(historyAfter(unknown, { kind: "page", conversations: [], next: null }, false))).toBe(true);
  });

  it("says the history could not be read on a 503 or the network, and still offers the list", () => {
    const failed = historyAfter(unknown, { kind: "failed", failure: { status: 503, error: "conversations_unavailable" } }, false);
    expect(failed).toEqual({ kind: "unavailable" });
    expect(offersHistory(failed)).toBe(true);
    expect(historyAfter(unknown, "unreachable", false)).toEqual({ kind: "unavailable" });
  });

  it("adds the next page newest first, each id once, and a first page replaces the list", () => {
    const first = historyAfter(unknown, { kind: "page", conversations: [row("a", 1), row("b", 10)], next: "cur" }, false);
    const both = historyAfter(first, { kind: "page", conversations: [row("b", 10), row("c", 30)], next: null }, true);
    expect(both).toEqual({ kind: "on", rows: [row("a", 1), row("b", 10), row("c", 30)], next: null });
    // An older conversation continued since moves up.
    expect(historyAfter(both, { kind: "page", conversations: [row("c", 0), row("a", 1)], next: "cur2" }, false)).toEqual({ kind: "on", rows: [row("c", 0), row("a", 1)], next: "cur2" });
  });

  it("keeps the rows and their cursor when a read fails, with the line, so Load more tries again", () => {
    const first = historyAfter(unknown, { kind: "page", conversations: [row("a", 1)], next: "cur" }, false);
    expect(historyAfter(first, { kind: "failed", failure: { status: 503 } }, true)).toEqual({ kind: "on", rows: [row("a", 1)], next: "cur", failed: true });
    expect(historyAfter(first, "unreachable", false)).toEqual({ kind: "on", rows: [row("a", 1)], next: "cur", failed: true });
  });
});
