import type { ChatSessionSummary, SyncedStatus, SyncedThread } from "@nexus/kit";
import type { UnifiedSession } from "@nexus/kit/session-history";
import type { OpenedEvent } from "@nexus/kit/synced-sessions";
import { describe, expect, it } from "vitest";
import { EMPTY_DRAWER_ORGANIZATION, moveDrawerSession, organizeDrawerSessions } from "./drawer-organization";
import { acceptPin, migrateLocalPins, queuePin, visiblePinKeys } from "./session-pin-state";
import { sessionKey } from "@nexus/kit/session-history";
import { waitsOnYou, type TitleState } from "./code-sessions";
import { drawerAgents, drawerGroups, filterDrawerSessions, groupDrawerSessions, loudest, projectComputer, rowState, sessionSections, stateWord } from "./drawer-groups";

const at = (minutesAgo: number) => new Date(Date.UTC(2026, 10, 13, 12, 0) - minutesAgo * 60_000).toISOString();

const chat = (id: string, minutesAgo = 0): UnifiedSession => ({
  source: "chat",
  id,
  updatedAt: at(minutesAgo),
  chat: { id, title: id, updatedAt: at(minutesAgo) } as ChatSessionSummary,
  origin: { kind: "chat" },
});

const code = (id: string, status: SyncedStatus | null = "completed", minutesAgo = 0): UnifiedSession => ({
  source: "synced",
  id,
  updatedAt: at(minutesAgo),
  thread: { threadId: id, status, lastSeq: 3, keyVersion: 1 } as SyncedThread,
  origin: { kind: "unknown" },
});

const open = (projectName: string, waiting = false): TitleState => ({ state: "open", title: { title: "t", projectName }, ...(waiting ? { waiting } : {}) });

it("puts older active sessions ahead of today's completed sessions", () => {
  const now = new Date(2026, 10, 13, 12);
  const day = (offset: number) => new Date(2026, 10, 13 + offset, 9).toISOString();
  const sessions = [
    { ...chat("chat"), updatedAt: day(0) },
    { ...code("failed", "error"), updatedAt: day(0) },
    { ...code("run", "running"), updatedAt: day(-1) },
    { ...code("wait", "running"), updatedAt: day(-2) },
    { ...code("finished", "completed"), updatedAt: day(-2) },
  ];
  const sections = sessionSections(sessions, { wait: open("docs", true) }, now);
  expect(sections.map(({ title, data }) => [title, data.map((session) => session.id)])).toEqual([
    ["Needs you", ["wait"]], ["Running", ["run"]], ["Done today", ["chat", "failed"]], ["Earlier", ["finished"]],
  ]);
});

describe("a session's mark, as the desktop's sidebar draws it", () => {
  it("works while its agent runs, and needs you while it runs on an open request", () => {
    expect(rowState(code("a", "running"), {})).toBe("working");
    expect(rowState(code("a", "running"), { a: open("nexus") })).toBe("working");
    expect(rowState(code("a", "running"), { a: open("nexus", true) })).toBe("needsYou");
  });

  it("fails on an error, and rests otherwise, a chat included", () => {
    expect(rowState(code("a", "error"), { a: open("nexus", true) })).toBe("failed");
    expect(rowState(code("a", "completed"), { a: open("nexus", true) })).toBe("idle");
    expect(rowState(code("a", "interrupted"), {})).toBe("idle");
    expect(rowState(code("a", null), {})).toBe("idle");
    expect(rowState(chat("c"), {})).toBe("idle");
  });

  it("says Working or Needs you in place of the age, and nothing at rest or failed", () => {
    expect(stateWord("working")).toBe("Working");
    expect(stateWord("needsYou")).toBe("Needs you");
    expect(stateWord("failed")).toBeNull();
    expect(stateWord("idle")).toBeNull();
  });

  it("lets a folded project show its loudest state, Needs you over Working", () => {
    expect(loudest(["idle", "working", "needsYou", "failed"])).toBe("needsYou");
    expect(loudest(["failed", "working", "idle"])).toBe("working");
    expect(loudest(["idle", "failed"])).toBeNull();
    expect(loudest([])).toBeNull();
  });
});

describe("the latest event asks you something", () => {
  const event = (kind: string): OpenedEvent => ({ state: "open", seq: 1, at: 0, thread: { title: "t", projectName: "p" } as never, event: { kind } });
  it("when it is an unanswered approval or question", () => {
    expect(waitsOnYou(event("approval"))).toBe(true);
    expect(waitsOnYou(event("approvalResolved"))).toBe(false);
    expect(waitsOnYou(event("tool"))).toBe(false);
    expect(waitsOnYou({ state: "no-key", seq: 1, at: 0 })).toBe(false);
    expect(waitsOnYou(undefined)).toBe(false);
  });
});

describe("the drawer's groups: Projects, then Recent", () => {
  it("puts an opened Code session under its project, and the rest in Recent, each once", () => {
    const sessions = [code("a", "running", 1), chat("c1", 2), code("b", "completed", 3), code("locked", "completed", 4), code("c", "completed", 5)];
    const titles = { a: open("nexus"), b: open("site"), c: open("nexus"), locked: { state: "no-key" } as const };
    const groups = drawerGroups(sessions, titles);
    expect(groups.projects.map((group) => [group.name, group.sessions.map((s) => s.id)])).toEqual([
      ["nexus", ["a", "c"]],
      ["site", ["b"]],
    ]);
    expect(groups.recent.map((s) => s.id)).toEqual(["c1", "locked"]);
    expect(groups.more).toBe(false);
  });

  it("keeps a session whose title is still opening, or has no project name, in Recent", () => {
    const groups = drawerGroups([code("a"), code("b")], { b: open("   ") });
    expect(groups.projects).toEqual([]);
    expect(groups.recent.map((s) => s.id)).toEqual(["a", "b"]);
  });

  it("shows the first rows of Recent and says it held more", () => {
    const sessions = Array.from({ length: 10 }, (_, i) => chat(`c${i}`, i));
    const groups = drawerGroups(sessions, {});
    expect(groups.recent).toHaveLength(8);
    expect(groups.more).toBe(true);
  });
});

it("can pivot sessions without losing a row, including a locked session", () => {
  const sessions = [chat("a"), code("b", "running"), code("c", "running"), code("d")];
  const titles = { b: open("project", true), c: open("project") };
  const state = groupDrawerSessions(sessions, titles, "status");
  expect(state.projects.map(({ name, sessions }) => [name, sessions.map(({ id }) => id)])).toEqual([
    ["Waiting on you", ["b"]],
    ["Working", ["c"]],
    ["Idle and done", ["a", "d"]],
  ]);
  const machine = groupDrawerSessions(sessions, titles, "machine");
  expect(machine.projects.map(({ name, sessions }) => [name, sessions.map(({ id }) => id)])).toEqual([
    ["Chat", ["a"]],
    ["Code", ["b", "c", "d"]],
  ]);
  expect(groupDrawerSessions(sessions, titles, "none").recent.map(({ id }) => id)).toEqual(["a", "b", "c", "d"]);
});

it("filters by a displayed agent name or its stored id without reading sealed content", () => {
  const coding = code("b");
  if (coding.source !== "synced") throw new Error("Expected a synced session");
  const sessions = [chat("a"), { ...coding, thread: { ...coding.thread, agent: "claude" } }, code("c")];
  expect(drawerAgents(sessions)).toEqual([{ id: "claude", label: "Claude Code" }, { id: "nexus", label: "Nexus" }]);
  expect(filterDrawerSessions(sessions, "agent: CLAUDE CODE").map(({ id }) => id)).toEqual(["b"]);
  expect(filterDrawerSessions(sessions, "agent: claude").map(({ id }) => id)).toEqual(["b"]);
  expect(filterDrawerSessions(sessions, "agent: Codex")).toEqual([]);
});

it("keeps a pinned session in one place and restores its section when unpinned", () => {
  const a = chat("shared-id");
  const b = code("shared-id");
  const key = sessionKey(a);
  const moved = moveDrawerSession(EMPTY_DRAWER_ORGANIZATION, key, "Release");
  const pinned = organizeDrawerSessions([b, a], { ...moved, pinned: [key] });
  expect(pinned.pinned).toEqual([a]);
  expect(pinned.sections).toEqual([{ name: "Release", sessions: [] }]);
  expect(pinned.remaining).toEqual([b]);
  expect(organizeDrawerSessions([b, a], moved).sections).toEqual([{ name: "Release", sessions: [a] }]);
});

it("moves a session between sections and can restore its default group", () => {
  const a = chat("a");
  const key = sessionKey(a);
  const moved = moveDrawerSession(moveDrawerSession(EMPTY_DRAWER_ORGANIZATION, key, "First"), key, "Second");
  expect(organizeDrawerSessions([a], moved).sections).toEqual([
    { name: "First", sessions: [] }, { name: "Second", sessions: [a] },
  ]);
  expect(organizeDrawerSessions([a], moveDrawerSession(moved, key, null)).remaining).toEqual([a]);
});

it("uses a project header's computer only when its known sessions point to the same computer", () => {
  const group = { name: "Gateway", sessions: [code("first"), code("second")] };
  const title = (computerId: string): TitleState => ({
    state: "open", title: { title: "Test", projectName: group.name, origin: { kind: "computer", computerId, name: "Mac" } },
  });
  expect(projectComputer(group, { first: title("a") })).toBe("a");
  expect(projectComputer(group, { first: title("a"), second: title("a") })).toBe("a");
  expect(projectComputer(group, { first: title("a"), second: title("b") })).toBeUndefined();
  expect(projectComputer(group, {})).toBeUndefined();
});

it("migrates local pins without replacing another device’s unpin", () => {
  const updatedAt = at(0);
  const previous = { source: "chat" as const, sessionId: "old", pinned: false, version: 2, updatedAt };
  const snapshot = migrateLocalPins([previous], ["chat:old", "synced:new"]);
  expect(visiblePinKeys(snapshot)).toEqual(["synced:new"]);
  expect(snapshot.pending).toEqual([{ source: "synced", sessionId: "new", pinned: true, expectedVersion: 0 }]);
});

it("keeps an offline unpin when an earlier pin receives its server revision", () => {
  const draft = queuePin({ pins: [], pending: [] }, "chat", "own", true);
  const pending = draft.pending[0]!;
  const latest = queuePin(draft, "chat", "own", false);
  const acknowledged = acceptPin(latest, pending, { source: "chat", sessionId: "own", pinned: true, version: 1, updatedAt: at(0) });
  expect(visiblePinKeys(acknowledged)).toEqual([]);
  expect(acknowledged.pending).toEqual([{ source: "chat", sessionId: "own", pinned: false, expectedVersion: 1 }]);
});

it("keeps different computers with matching names in separate machine groups", () => {
  const sessions = [code("a"), code("b")];
  const title = (computerId: string): TitleState => ({
    state: "open", title: { title: "Test", projectName: "Project", origin: { kind: "computer", computerId, name: "Mac" } },
  });
  const groups = groupDrawerSessions(sessions, { a: title("first"), b: title("second") }, "machine");
  expect(groups.projects.map((group) => [group.key, group.sessions.map((row) => row.id)]))
    .toEqual([["computer:first", ["a"]], ["computer:second", ["b"]]]);
});
