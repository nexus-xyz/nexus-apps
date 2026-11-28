import type { SyncedThread } from "@nexus/kit";
import { PAIRING_TTL_MS } from "@nexus/kit/pairing";
import { fixtureDevice, fixtureDeviceKey } from "@nexus/kit/synced-fixture";
import { describe, expect, it } from "vitest";
import { accessOf, costText, groupThreads, readerState, rowDetail, titleText, waitText } from "./code-sessions";

const thread = (overrides: Partial<SyncedThread> = {}): SyncedThread => ({
  threadId: "t1",
  personId: "u1",
  agent: "claude",
  model: "anthropic/claude-sonnet-5-5",
  status: "running",
  totals: { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: "0.184" },
  lastSeq: 4,
  keyVersion: 1,
  lastEventAt: "2026-11-05T10:00:00Z",
  createdAt: "2026-11-05T09:00:00Z",
  updatedAt: "2026-11-05T10:00:00Z",
  ...overrides,
});

describe("where this phone stands as a reader", () => {
  it("waits for the Keychain and the directory, then reads its own row", async () => {
    const key = await fixtureDeviceKey();
    expect(readerState(null, [])).toEqual({ kind: "loading" });
    expect(readerState(key, undefined)).toEqual({ kind: "loading" });
    expect(readerState(undefined, [])).toEqual({ kind: "unpaired" });
    // A key Gateway does not list is unpaired too: the person pairs it again.
    expect(readerState(key, [fixtureDevice({ signingKey: "other" })])).toEqual({ kind: "unpaired" });
    expect(readerState(key, [fixtureDevice()])).toMatchObject({ kind: "ready", reader: { deviceId: fixtureDevice().id } });
  });

  it("is pending for 15 minutes, then expired; revoked stays revoked", async () => {
    const key = await fixtureDeviceKey();
    const pending = fixtureDevice({ state: "pending", createdAt: "2026-11-05T10:00:00Z" });
    const at = Date.parse(pending.createdAt);
    expect(readerState(key, [pending], at + 60_000).kind).toBe("pending");
    expect(readerState(key, [pending], at + PAIRING_TTL_MS).kind).toBe("expired");
    expect(readerState(key, [fixtureDevice({ state: "revoked" })]).kind).toBe("revoked");
  });

  it("never waits for an approval when Gateway vouches: it registers, then reads with gateway trust", async () => {
    const key = await fixtureDeviceKey();
    const now = Date.parse("2026-11-05T10:01:00Z");
    // Unpaired, and a pending row from before, register again: the screen loads meanwhile, with no code to show.
    expect(readerState(undefined, [], now, "gateway")).toEqual({ kind: "loading" });
    expect(readerState(key, [fixtureDevice({ state: "pending", createdAt: "2026-11-05T10:00:00Z" })], now, "gateway")).toEqual({ kind: "loading" });
    expect(readerState(key, [fixtureDevice({ state: "revoked" })], now, "gateway").kind).toBe("revoked");
    expect(readerState(key, [fixtureDevice()], now, "gateway")).toMatchObject({ kind: "ready", reader: { trust: "gateway" } });
    expect(readerState(key, [fixtureDevice()], now)).not.toMatchObject({ reader: { trust: "gateway" } });
  });
});

describe("a session's row", () => {
  it("says what this phone can read of it", () => {
    expect(accessOf(thread({ lastSeq: null }), new Set())).toBe("empty");
    expect(accessOf(thread({ keyVersion: null }), new Set())).toBe("local");
    expect(accessOf(thread(), new Set())).toBe("no-key");
    expect(accessOf(thread(), new Set(["t1"]))).toBe("sealed");
  });

  it("shows the opened title, or what stands for it", () => {
    expect(titleText("sealed", { state: "open", title: { title: "Decoder for session 14", projectName: "cortex-sim" } })).toBe("Decoder for session 14");
    expect(titleText("sealed", { state: "open", title: { title: " ", projectName: "" } })).toBe("Untitled session");
    expect(titleText("no-key", undefined)).toBe("Locked session");
    expect(titleText("local", undefined)).toBe("Kept on its computer");
  });

  it("says the repository, state, agent, model and cost in one line", () => {
    const name = (id: string) => (id.endsWith("sonnet-5-5") ? "Sonnet 5.5" : id);
    const open = { state: "open" as const, title: { title: "Decoder", projectName: "cortex-sim" } };
    expect(rowDetail(thread(), open, name)).toBe("cortex-sim · Running · Claude Code · Sonnet 5.5 · $0.1840");
    expect(rowDetail(thread({ status: "error", agent: "codex", model: null, totals: { ...thread().totals, costUsd: "0" } }), undefined, name)).toBe("Failed · Codex");
  });

  it("writes money as the rest of the app does, and nothing for none", () => {
    expect(costText("1.5")).toBe("$1.50");
    expect(costText("0")).toBeNull();
    expect(costText("nope")).toBeNull();
  });
});

describe("the list", () => {
  it("groups Today and Earlier, newest first, and leaves an empty group out", () => {
    const now = new Date(2026, 10, 5, 12).getTime();
    const morning = new Date(2026, 10, 5, 8).toISOString();
    const noon = new Date(2026, 10, 5, 11).toISOString();
    const monday = new Date(2026, 10, 2, 9).toISOString();
    const groups = groupThreads([thread({ threadId: "a", lastEventAt: morning }), thread({ threadId: "b", lastEventAt: monday }), thread({ threadId: "c", lastEventAt: noon })], now);
    expect(groups.map((group) => [group.title, group.threads.map((t) => t.threadId)])).toEqual([
      ["Today", ["c", "a"]],
      ["Earlier", ["b"]],
    ]);
    expect(groupThreads([thread({ lastEventAt: monday })], now).map((group) => group.title)).toEqual(["Earlier"]);
  });

  it("says how long a pairing code still works", () => {
    const created = "2026-11-05T10:00:00Z";
    expect(waitText(created, Date.parse(created), PAIRING_TTL_MS)).toBe("15 minutes left");
    expect(waitText(created, Date.parse(created) + 14.5 * 60_000, PAIRING_TTL_MS)).toBe("Under a minute left");
  });
});
