import { describe, expect, it, vi } from "vitest";
import type { ChatSessionSummary, ChatSessionsResponse, SyncedThread } from "./api";
import { createCoreClient, type CoreClient } from "./core-client";
import { compareSessions, createUnifiedHistory, originLabel, sessionKey } from "./session-history";

const at = (second: number) => new Date(Date.UTC(2026, 10, 9, 12, 0, second)).toISOString();
const chat = (id: string, second: number): ChatSessionSummary => ({ id, title: id, mode: "chat", model: "nexus/auto", archived: false, messageCount: 1, costUsd: 0, createdAt: at(0), updatedAt: at(second) });
const thread = (id: string, second: number): SyncedThread => ({ threadId: id, personId: "u1", agent: "claude", model: null, status: "completed",
  totals: { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: "0" }, lastSeq: 0, keyVersion: 1, lastEventAt: at(second), createdAt: at(0), updatedAt: at(second) });
const activity = (threads: SyncedThread[], nextCursor: string | null = null): Awaited<ReturnType<CoreClient["activeSyncedThreads"]>> => ({ order: "activity", threads, nextBefore: null, nextCursor });

describe("one activity-ordered session history", () => {
  it("fills mixed pages without dropping unused rows from either source", async () => {
    const chatSessions = vi.fn<CoreClient["chatSessions"]>()
      .mockResolvedValueOnce({ sessions: [chat("chat6", 6), chat("chat4", 4)], nextCursor: "chat-page2" })
      .mockResolvedValueOnce({ sessions: [chat("chat2", 2)] });
    const activeSyncedThreads = vi.fn<CoreClient["activeSyncedThreads"]>()
      .mockResolvedValueOnce(activity([thread("old-created-native5", 5), thread("native3", 3)], "native-page2"))
      .mockResolvedValueOnce(activity([thread("native1", 1)]));
    const pager = createUnifiedHistory({ chatSessions, activeSyncedThreads });
    const first = await pager.next({ limit: 3 });
    const second = await pager.next({ limit: 3 });
    expect(first.sessions.map(sessionKey)).toEqual(["chat:chat6", "synced:old-created-native5", "chat:chat4"]);
    expect(second.sessions.map(sessionKey)).toEqual(["synced:native3", "chat:chat2", "synced:native1"]);
    expect(second).toMatchObject({ more: false, errors: [] });
  });

  it("keeps chat and execution IDs distinct even when their strings coincide", async () => {
    const pager = createUnifiedHistory({ chatSessions: async () => ({ sessions: [chat("same", 4)] }), activeSyncedThreads: async () => activity([thread("same", 4)]) });
    expect(new Set((await pager.next()).sessions.map(sessionKey))).toEqual(new Set(["chat:same", "synced:same"]));
    expect(originLabel({ kind: "computer", computerId: "one", name: "Eduardo’s MacBook" }, "one")).toBe("This computer");
    expect(originLabel({ kind: "computer", computerId: "one", name: "Eduardo’s MacBook" })).toBe("Eduardo’s MacBook");
    expect(originLabel({ kind: "cloud", machineId: "vm_one" })).toBe("Cloud");
  });

  it("retains the page position after one source fails, then includes its newer rows on retry", async () => {
    const activeSyncedThreads = vi.fn<CoreClient["activeSyncedThreads"]>()
      .mockRejectedValueOnce(new Error("Computer history unavailable"))
      .mockResolvedValue(activity([thread("native5", 5)]));
    const pager = createUnifiedHistory({ chatSessions: async () => ({ sessions: [chat("chat6", 6), chat("chat4", 4)] }), activeSyncedThreads });
    const partial = await pager.next({ limit: 2 });
    expect(partial).toMatchObject({ more: true, errors: [{ source: "synced" }] });
    const complete = await pager.next({ limit: 2 });
    const tail = await pager.next({ limit: 2 });
    expect(complete.sessions.map(sessionKey)).toEqual(["chat:chat6", "synced:native5"]);
    expect(tail.sessions.map(sessionKey)).toEqual(["chat:chat4"]);
    const merged = new Map([...partial.sessions, ...complete.sessions, ...tail.sessions].map((row) => [sessionKey(row), row]));
    expect([...merged.values()].sort(compareSessions).map(sessionKey)).toEqual(["chat:chat6", "synced:native5", "chat:chat4"]);
  });

  it("does not silently treat a creation-ordered legacy Gateway response as recent history", async () => {
    const core = createCoreClient({ baseUrl: "https://gateway.test", fetch: async (input) => Response.json(String(input).includes("/v1/threads")
      ? { threads: [thread("creation-order-only", 5)], nextBefore: null }
      : { sessions: [chat("chat6", 6)] }) });
    const page = await createUnifiedHistory(core).next();
    expect(page.sessions.map(sessionKey)).toEqual(["chat:chat6"]);
    expect(page.errors).toEqual([{ source: "synced", message: "Update Gateway to include computer sessions in history." }]);
  });

  it("merges team and private history without repeating a shared owned session", async () => {
    const activeSyncedThreads = vi.fn<CoreClient["activeSyncedThreads"]>().mockImplementation(async (query) => query?.scope === "team"
      ? activity([thread("team", 6), thread("shared", 5)])
      : activity([thread("shared", 5), thread("private", 2)]));
    const core = { activeSyncedThreads, chatSessions: async () => ({ sessions: [chat("chat", 4)] }) };
    const all = createUnifiedHistory(core, true);
    expect((await all.next({ limit: 2 })).sessions.map(sessionKey)).toEqual(["synced:team", "synced:shared"]);
    expect((await all.next({ limit: 2 })).sessions.map(sessionKey)).toEqual(["chat:chat", "synced:private"]);
    expect((await createUnifiedHistory(core).next()).sessions.map(sessionKey)).toEqual(["synced:shared", "chat:chat", "synced:private"]);
  });

  it("does not let an old refresh clear or consume the new in-flight page", async () => {
    let oldResolve: ((page: ChatSessionsResponse) => void) | undefined;
    let freshResolve: ((page: ChatSessionsResponse) => void) | undefined;
    const oldChat = new Promise<ChatSessionsResponse>((resolve) => { oldResolve = resolve; });
    const freshChat = new Promise<ChatSessionsResponse>((resolve) => { freshResolve = resolve; });
    const chatSessions = vi.fn<CoreClient["chatSessions"]>().mockReturnValueOnce(oldChat).mockReturnValueOnce(freshChat);
    const pager = createUnifiedHistory({ chatSessions, activeSyncedThreads: async () => activity([]) });
    const old = pager.next().catch((error: unknown) => error);
    pager.reset();
    const fresh = pager.next();
    oldResolve?.({ sessions: [chat("old", 1)] });
    expect(await old).toMatchObject({ name: "AbortError" });
    const joining = pager.next();
    freshResolve?.({ sessions: [chat("fresh", 9)] });
    expect((await fresh).sessions.map(sessionKey)).toEqual(["chat:fresh"]);
    expect((await joining).sessions.map(sessionKey)).toEqual(["chat:fresh"]);
  });
});
