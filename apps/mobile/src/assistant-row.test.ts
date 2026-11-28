import type { AgentConversation, AgentErrand, AgentTurn } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { activityOf, markSeen, setStreaming, subscribe } from "./assistant-activity";
import { errandNeedsYou, rowLabel, rowState, unreadPill, waitsOnYou } from "./assistant-row";

// The Assistant's row in the drawer (A21·c): unread, thinking, needs you, and how VoiceOver reads it.

const ask = (text: string): AgentTurn => ({ role: "user", text, at: "2026-11-16T08:00:00Z" });
const reply = (text: string, extra: Partial<AgentTurn> = {}): AgentTurn => ({ role: "assistant", text, at: "2026-11-16T08:00:05Z", ...extra });
const turns = (...list: AgentTurn[]): AgentConversation => ({ kind: "turns", turns: list });
const write = { kind: "pending_action" as const, card: "w1", write: { connector: "github", operation: "merge", summary: "Merge #482", effect: "reversible", arguments: {} }, detail: "Asks first", approveUrl: "https://nexus.inc/a/1" };
const errand = (status: AgentErrand["status"]): AgentErrand => ({ id: "i1", goal: "Dinner Friday", status, steps: 1, at: "2026-11-16T08:00:00Z" });

describe("unread", () => {
  it("counts Nexus's replies the screen has not shown in the kept conversation", () => {
    const read = turns(ask("a"), reply("1"), ask("b"), reply("2"), reply("3"));
    expect(rowState({ conversation: "c1", read, seen: { conversation: "c1", answers: 1 } })).toEqual({ unread: 2, needsYou: false });
    expect(rowState({ conversation: "c1", read, seen: { conversation: "c1", answers: 3 } }).unread).toBe(0);
  });

  it("counts every reply of a conversation the screen did not show, and never goes below zero", () => {
    const read = turns(ask("a"), reply("1"));
    expect(rowState({ conversation: "c2", read, seen: { conversation: "c1", answers: 4 } }).unread).toBe(1);
    expect(rowState({ conversation: "c1", read, seen: { conversation: "c1", answers: 4 } }).unread).toBe(0);
  });

  it("keeps a first baseline instead of counting old replies after an update", () => {
    expect(rowState({ conversation: "c1", read: turns(ask("a"), reply("1")) })).toEqual({ unread: 0, needsYou: false, baseline: { conversation: "c1", answers: 1 } });
  });

  it("counts nothing with no kept conversation, or when the read failed or it is gone", () => {
    expect(rowState({}).unread).toBe(0);
    expect(rowState({ conversation: "c1", read: { kind: "gone" }, seen: { conversation: "c1", answers: 0 } }).unread).toBe(0);
    expect(rowState({ conversation: "c1", read: { kind: "failed", failure: { status: 503 } } }).unread).toBe(0);
  });

  it("writes the pill as the inbox does: nothing at zero, 99+ past 99", () => {
    expect(unreadPill(0)).toBeUndefined();
    expect(unreadPill(2)).toBe("2");
    expect(unreadPill(140)).toBe("99+");
  });
});

describe("needs you", () => {
  it("is a write that waits in Nexus's last turn, until the person answers it", () => {
    expect(waitsOnYou(turns(ask("merge it"), reply("Merge?", { messages: [write] })))).toBe(true);
    expect(waitsOnYou(turns(ask("merge it"), reply("Merge?", { messages: [write] }), ask("not now")))).toBe(false);
    expect(waitsOnYou(turns(ask("hi"), reply("Hi")))).toBe(false);
    expect(waitsOnYou(undefined)).toBe(false);
  });

  it("is an errand that needs the person, from the errands list when it shows", () => {
    expect(errandNeedsYou({ kind: "list", errands: [errand("running"), errand("needs-you")] })).toBe(true);
    expect(errandNeedsYou({ kind: "list", errands: [errand("running")] })).toBe(false);
    expect(errandNeedsYou({ kind: "off" })).toBe(false);
    expect(errandNeedsYou(undefined)).toBe(false);
    expect(rowState({ errands: { kind: "list", errands: [errand("needs-you")] } }).needsYou).toBe(true);
  });
});

describe("the row's label", () => {
  it("reads its states in words", () => {
    expect(rowLabel({ unread: 0, needsYou: false, thinking: false })).toBe("Assistant");
    expect(rowLabel({ unread: 2, needsYou: false, thinking: false })).toBe("Assistant, 2 unread");
    expect(rowLabel({ unread: 0, needsYou: false, thinking: true })).toBe("Assistant, thinking");
    expect(rowLabel({ unread: 1, needsYou: true, thinking: false })).toBe("Assistant, needs you, 1 unread");
  });
});

describe("what the screen tells the drawer", () => {
  it("keeps streaming and seen per account, and tells listeners once per change", () => {
    const listener = vi.fn();
    const stop = subscribe(listener);
    const before = activityOf("acct-row-test");
    setStreaming("acct-row-test", true);
    setStreaming("acct-row-test", true);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(activityOf("acct-row-test")).toEqual({ streaming: true, seen: 0 });
    expect(activityOf("other-row-test").streaming).toBe(false);
    markSeen("acct-row-test");
    expect(activityOf("acct-row-test").seen).toBe(1);
    expect(activityOf("acct-row-test")).toBe(activityOf("acct-row-test"));
    expect(activityOf("acct-row-test")).not.toBe(before);
    setStreaming(undefined, true);
    expect(listener).toHaveBeenCalledTimes(2);
    stop();
  });
});
