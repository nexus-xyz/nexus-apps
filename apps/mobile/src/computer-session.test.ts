import { createCoreClient, type ComputerAgentChoices, type ComputerOperation, type ComputerPresence, type ComputerSessionSettings, type ComputerRequest, type ComputerRequestEnvelope, type ComputerResult } from "@nexus/kit";
import { computerPresenceMessage, computerReplyMessage, computerRequestHash } from "@nexus/kit/computer-sessions";
import { b64u, sign } from "@nexus/kit/e2e";
import { fixtureDevice, fixtureDeviceKey, FIXTURE } from "@nexus/kit/synced-fixture";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openObject, sealObject } from "../../../packages/kit/src/computer-protocol";
import { computerSession, ComputerSession, requestedSettings, type ComputerSaved } from "./computer-session";

const scope = { gateway: "https://gateway.test", tenantId: "tenant", personId: "person" };
const computerId = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f507";
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
const missing = () => json({ error: { code: "not_found", message: "Not found" } }, 404);
async function setup({ asleep = false, lost = false, failStorage = false, delayed = false, refuseRetry = false, replies = [], settings = false, threadId, loseAt, controlOff = false, corrupt = false, live = false }: { asleep?: boolean; settings?: boolean; threadId?: string; loseAt?: number; lost?: boolean; failStorage?: boolean; delayed?: boolean; refuseRetry?: boolean; replies?: ComputerResult[]; controlOff?: boolean; corrupt?: boolean; live?: boolean } = {}) {
  const holder = await fixtureDeviceKey();
  let saved: ComputerSaved | undefined;
  let receipt: ComputerRequest | undefined;
  // Gateway's live stream of request states, when `live`; else an older Gateway's 404.
  let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
  let receiptReads = 0;
  const posts: ComputerRequestEnvelope[] = [];
  const operations: ComputerOperation[] = [];
  const order: string[] = [];
  let release: (() => void) | undefined;
  let awake = !asleep;
  let network = true;
  let synced = false;
  let syncReads = 0;
  const core = createCoreClient({ baseUrl: scope.gateway, apiKey: "test", fetch: async (input, init) => {
    if (!network) throw new TypeError("Gateway connection lost");
    const path = new URL(String(input)).pathname;
    if (path === "/v1/devices") return json({ devices: [fixtureDevice()] });
    if (path === "/v1/computers") {
      const unsigned: Omit<ComputerPresence, "signature"> = { version: 1, scope, computerId, runnerDeviceId: FIXTURE.deviceId, name: "Laptop", generation: 1, issuedAt: Date.now() - (awake ? 0 : 60_000), expiresAt: Date.now() + (awake ? 45_000 : -15_000), enabled: !controlOff, capabilities: settings ? ["projects", "start", "settings"] : ["projects", "start"] };
      return json({ computers: [{ ...unsigned, signature: b64u(await sign(holder, computerPresenceMessage(unsigned))) }], serverTime: Date.now() });
    }
    if (path.endsWith("/requests") && init?.method === "POST") {
      const envelope: ComputerRequestEnvelope = JSON.parse(String(init.body));
      posts.push(envelope); order.push("post");
      const plain = await openObject(envelope.id, FIXTURE.deviceId, holder, envelope.lockedKey, envelope.sealed, 64 * 1024);
      operations.push(plain.operation as ComputerOperation);
      if (refuseRetry && posts.length > 1) return json({ error: { code: "denied", message: "Not allowed" } }, 403);
      receipt = { envelope, requestHash: "hash", state: "queued", reply: null, serverTime: Date.now() };
      const result = replies.shift();
      if (result) {
        const route = { version: 1 as const, scope, computerId, requestId: envelope.id, requestHash: await computerRequestHash(envelope), runnerDeviceId: FIXTURE.deviceId, recipientDeviceId: envelope.senderDeviceId };
        const box = await sealObject(envelope.id, envelope.senderDeviceId, holder.encryptionKey, { ...route, kind: "computerReply", result }, 256 * 1024);
        const unsigned = { ...route, ...box };
        receipt = { ...receipt, state: "replied", reply: { ...unsigned, signature: b64u(await sign(holder, computerReplyMessage(unsigned))) } };
      }
      if (lost || posts.length === loseAt) { receipt = undefined; throw new TypeError("Connection lost"); }
      if (delayed) return new Promise<Response>((resolve) => { const queued = receipt; release = () => resolve(json(queued)); });
      return json(receipt);
    }
    if (path === "/v1/computers/stream") {
      if (!live) return missing();
      return new Response(new ReadableStream<Uint8Array>({ start: (controller) => { stream = controller; } }), { headers: { "content-type": "text/event-stream" } });
    }
    if (path.includes("/requests/")) { receiptReads++; return receipt ? json(receipt) : missing(); }
    if (path === `/v1/threads/${FIXTURE.threadId}`) {
      syncReads++;
      return synced ? json({ threadId: FIXTURE.threadId, agent: "claude", model: null, status: "running", totals: { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: "0" }, lastSeq: 0, keyVersion: 1, lastEventAt: new Date().toISOString(), createdAt: new Date().toISOString(), updatedAt: new Date().toISOString() }) : missing();
    }
    throw new Error(`Unexpected ${path}`);
  } });
  let unreadable = corrupt;
  const journal = {
    load: async () => { if (unreadable) throw new Error("Saved computer request is invalid."); return saved; },
    save: async (value: ComputerSaved) => { order.push("save"); if (failStorage) throw new Error("Storage full"); saved = structuredClone(value); },
    remove: async () => { order.push("remove"); unreadable = false; saved = undefined; },
  };
  const options = { core, scope, computerId, threadId, sender: { deviceId: FIXTURE.deviceId, holder }, journal };
  const session = new ComputerSession(options);
  await session.restore();
  return { session, options, posts, operations, order, disconnect: () => { network = false; }, reconnect: () => { network = true; }, sleep: () => { awake = false; }, wake: () => { awake = true; }, saved: () => saved, publish: () => { synced = true; }, syncReads: () => syncReads, reopen: async () => { const reopened = new ComputerSession(options); await reopened.restore(); return reopened; }, delayedReady: () => release !== undefined, claim: () => { if (receipt) receipt = { ...receipt, state: "claimed" }; }, release: () => release?.(),
    streaming: () => stream !== undefined, receiptReads: () => receiptReads,
    // Several states arrive in one chunk, as close announcements can.
    announce: (...states: ComputerRequest["state"][]) => stream?.enqueue(new TextEncoder().encode(states.map((state) => `event: request\ndata: ${JSON.stringify({ computerId, id: receipt?.envelope.id, state })}\n\n`).join(""))) };
}
beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());
describe("computer request delivery", () => {
  it("stores the encrypted request before sending and restores its exact id without posting on focus", async () => {
    const { session, posts, order, reopen } = await setup({ lost: true });
    session.setDraft("Keep this native draft");
    await session.describe();
    expect(order.at(-2)).toBe("save");
    expect(order.at(-1)).toBe("post");
    expect(session.snapshot().pending?.state).toBe("unknown");
    const restored = await reopen();
    restored.setActive(true);
    try {
      await vi.waitFor(() => expect(restored.snapshot().pending?.state).toBe("unknown"));
      expect(restored.snapshot().draft).toBe("Keep this native draft");
      expect(restored.snapshot().pending?.envelope.id).toBe(posts[0]?.id);
      expect(posts).toHaveLength(1);
      await restored.describe();
      expect(posts).toHaveLength(1);
    } finally { restored.setActive(false); }
  });
  it("reads the receipt the moment the stream says it changed, and polls slowly while the stream is live", async () => {
    const { session, streaming, claim, announce, receiptReads } = await setup({ live: true });
    session.setActive(true);
    try {
      // Focus describes the computer's projects: one request waits.
      await vi.waitFor(() => expect(session.snapshot().pending?.state).toBe("queued"));
      await vi.waitFor(() => expect(streaming()).toBe(true));
      await vi.advanceTimersByTimeAsync(100);
      const reads = receiptReads();
      await vi.advanceTimersByTimeAsync(5000);
      expect(receiptReads()).toBe(reads);
      claim();
      announce("claimed");
      await vi.advanceTimersByTimeAsync(100);
      expect(session.snapshot().pending?.state).toBe("claimed");
    } finally { session.setActive(false); }
  });
  it("reads the receipt once more when a state is announced during a read", async () => {
    const { session, streaming, claim, announce, receiptReads } = await setup({ live: true });
    session.setActive(true);
    try {
      await vi.waitFor(() => expect(session.snapshot().pending?.state).toBe("queued"));
      await vi.waitFor(() => expect(streaming()).toBe(true));
      await vi.advanceTimersByTimeAsync(100);
      const reads = receiptReads();
      claim();
      // The second announcement comes while the first one's read is in flight: it may hold the older state.
      announce("claimed", "claimed");
      await vi.advanceTimersByTimeAsync(100);
      expect(receiptReads()).toBe(reads + 2);
      expect(session.snapshot().pending?.state).toBe("claimed");
    } finally { session.setActive(false); }
  });
  it("does not send if retaining its request fails", async () => {
    const { session, posts } = await setup({ failStorage: true });
    await session.describe();
    expect(posts).toHaveLength(0);
    expect(session.snapshot().error).toContain("Storage full");
  });
  it("keeps a delayed queued receipt pending after the computer claimed it near expiry", async () => {
    const { session, posts, claim, release, delayedReady } = await setup({ delayed: true });
    const sending = session.describe();
    await vi.waitFor(() => expect(posts).toHaveLength(1));
    await vi.waitFor(() => expect(delayedReady()).toBe(true));
    claim();
    vi.setSystemTime(Date.now() + 31_000);
    release();
    await sending;
    expect(session.snapshot().pending?.state).toBe("queued");
    session.setActive(true);
    try { await vi.waitFor(() => expect(session.snapshot().pending?.state).toBe("claimed")); expect(posts).toHaveLength(1); }
    finally { session.setActive(false); }
  });
  it("retains the original request after an uncertain retry is refused and receipt is still missing", async () => {
    const { session, posts } = await setup({ lost: true, refuseRetry: true });
    await session.describe();
    await session.retry();
    expect(posts).toHaveLength(2);
    expect(posts[1]).toEqual(posts[0]);
    expect(session.snapshot().pending?.state).toBe("unknown");
    expect(session.snapshot().pending?.envelope.id).toBe(posts[0]?.id);
  });
});

const project = { id: "project", name: "Nexus", locationHint: "nexus", revision: "r1", settings: { agent: "claude", model: "claude-sonnet", via: "subscription", permissionMode: "default", effort: null, workspace: "folder" as const } };
describe("the paired worktree branch", () => {
  const listed: ComputerResult = { kind: "projects", projects: [project] };
  const branches: ComputerResult = { kind: "branches", projectId: project.id, revision: project.revision, branches: ["main", "origin/topic"] };

  it("restores the selected branch and sends it with a start for the same project", async () => {
    const { session, operations, reopen } = await setup({ replies: [listed, branches, listed] });
    await session.describe();
    await session.describeBranches();
    session.selectBaseBranch("origin/topic");
    session.setDraft("Start work from topic");

    const reopened = await reopen();
    await reopened.describe();
    await reopened.start();

    expect(operations[1]).toEqual({ kind: "describeBranches", projectId: project.id, revision: project.revision });
    expect(operations.at(-1)).toMatchObject({ kind: "startSession", projectId: project.id, baseBranch: "origin/topic" });
  });

  it("holds the draft when the saved branch list belongs to an older project revision", async () => {
    const updated: ComputerResult = { kind: "projects", projects: [{ ...project, revision: "r2" }] };
    const { session, operations, reopen } = await setup({ replies: [listed, branches, updated] });
    await session.describe();
    await session.describeBranches();
    session.selectBaseBranch("origin/topic");
    session.setDraft("Keep my starting branch");

    const reopened = await reopen();
    await reopened.describe();
    await reopened.start();

    expect(operations.some((operation) => operation.kind === "startSession")).toBe(false);
    expect(reopened.snapshot()).toMatchObject({ draft: "Keep my starting branch", baseBranch: "origin/topic" });
  });
});


describe("starting a computer session", () => {
  it("keeps a confirmed creation through restart and a transient missing history without starting twice", async () => {
    const { session, posts, saved, reopen, publish, syncReads } = await setup({ replies: [{ kind: "projects", projects: [project] }, { kind: "created", threadId: FIXTURE.threadId, origin: { kind: "computer", computerId, name: "Laptop" }, launch: "starting" }] });
    await session.describe();
    session.setDraft("Implement the change");
    await session.start();
    expect(posts).toHaveLength(2);
    expect(session.snapshot().created?.threadId).toBe(FIXTURE.threadId);
    expect(session.snapshot().pending).toBeUndefined();
    expect(saved()?.draft).toBe("Implement the change");
    expect(saved()?.created?.threadId).toBe(FIXTURE.threadId);
    const restored = await reopen();
    restored.setActive(true);
    try {
      await vi.waitFor(() => expect(syncReads()).toBeGreaterThan(0));
      expect(restored.snapshot().created?.ready).toBe(false);
      await restored.start();
      await restored.describe();
      await restored.retry();
      expect(posts).toHaveLength(2);
      publish();
      await vi.advanceTimersByTimeAsync(2000);
      await vi.waitFor(() => expect(restored.snapshot().created?.ready).toBe(true));
      restored.opened();
      expect(restored.snapshot().created).toBeUndefined();
      await vi.waitFor(() => expect(saved()?.draft).toBe(""));
      expect(saved()?.created).toBeUndefined();
      expect(posts).toHaveLength(2);
    } finally { restored.setActive(false); }
  });
  it("keeps a start that the computer cannot confirm pending after reopening, without resubmission", async () => {
    const { session, posts, reopen } = await setup({ replies: [{ kind: "projects", projects: [project] }, { kind: "uncertain", threadId: null }] });
    await session.describe();
    session.setDraft("Run once");
    await session.start();
    expect(session.snapshot().pending?.state).toBe("uncertain");
    const restored = await reopen();
    restored.setActive(true);
    try {
      await vi.waitFor(() => expect(restored.snapshot().pending?.state).toBe("uncertain"));
      await restored.start();
      await restored.retry();
      expect(posts).toHaveLength(2);
      expect(restored.snapshot().draft).toBe("Run once");
    } finally { restored.setActive(false); }
  });
  it("requires a new deliberate start when the project's defaults change", async () => {
    const changed = { ...project, revision: "r2", settings: { ...project.settings, permissionMode: "acceptEdits" } };
    const { session, posts } = await setup({ replies: [{ kind: "projects", projects: [project] }, { kind: "choicesChanged", projects: [changed] }] });
    await session.describe();
    session.setDraft("Keep this request");
    await session.start();
    expect(session.snapshot().pending).toBeUndefined();
    expect(session.snapshot().projects[0]?.revision).toBe("r2");
    expect(session.snapshot().draft).toBe("Keep this request");
    expect(posts).toHaveLength(2);
  });
});

describe("a start whose history never syncs", () => {
  const created: ComputerResult = { kind: "created", threadId: FIXTURE.threadId, origin: { kind: "computer", computerId, name: "Laptop" }, launch: "starting" };
  async function stalled() {
    const run = await setup({ replies: [{ kind: "projects", projects: [project] }, created] });
    await run.session.describe();
    run.session.setDraft("Implement the change");
    await run.session.start();
    run.session.setActive(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(run.session.snapshot().created?.stalled).toBeFalsy();
    // Before the wait ends, nothing unlocks the composer.
    run.session.startAnother();
    expect(run.session.openAnyway()).toBeUndefined();
    expect(run.session.snapshot().created?.threadId).toBe(FIXTURE.threadId);
    await vi.advanceTimersByTimeAsync(25_000);
    await vi.waitFor(() => expect(run.session.snapshot().created?.stalled).toBe(true));
    expect(run.session.snapshot().notice).toContain("Sync may be off");
    run.session.setDraft("Locked while the start is tracked");
    expect(run.session.snapshot().draft).toBe("Implement the change");
    return run;
  }

  it("explains the wait after 30 s, and Start another unlocks the composer without sending again", async () => {
    const { session, posts, saved } = await stalled();
    try {
      session.startAnother();
      expect(session.snapshot().created).toBeUndefined();
      expect(session.snapshot().draft).toBe("");
      session.setDraft("A new session");
      expect(session.snapshot().draft).toBe("A new session");
      await vi.waitFor(() => expect(saved()?.created).toBeUndefined());
      expect(posts).toHaveLength(2);
    } finally { session.setActive(false); }
  });

  it("opens the started session anyway when the person asks", async () => {
    const { session, posts, saved } = await stalled();
    try {
      expect(session.openAnyway()).toBe(FIXTURE.threadId);
      expect(session.snapshot().created).toBeUndefined();
      await vi.waitFor(() => expect(saved()?.created).toBeUndefined());
      expect(posts).toHaveLength(2);
    } finally { session.setActive(false); }
  });
});

it("unlocks a composer whose saved draft does not open only when the person discards it", async () => {
  const { session, order, posts } = await setup({ corrupt: true });
  expect(session.snapshot()).toMatchObject({ loaded: false, unreadable: true });
  session.setDraft("Cannot type yet");
  expect(session.snapshot().draft).toBe("");
  await session.discardSaved();
  expect(order).toContain("remove");
  expect(session.snapshot()).toMatchObject({ loaded: true, unreadable: false });
  session.setDraft("Now I can type");
  expect(session.snapshot().draft).toBe("Now I can type");
  expect(posts).toHaveLength(0);
});

it("names Remote control off apart from an offline computer, and sends nothing", async () => {
  const { session, posts } = await setup({ controlOff: true });
  await session.describe();
  expect(posts).toHaveLength(0);
  expect(session.snapshot().error).toBe("Turn on Remote control on Laptop.");
  const computer = session.snapshot().computers[0]!;
  expect(session.reach(computer)).toBe("off");
  expect(session.reach({ ...computer, expiresAt: 0 })).toBe("offline");
  expect(session.reach({ ...computer, enabled: true })).toBe("awake");
});

it("reacquires the production navigation owner without a second POST and isolates another account and session settings", async () => {
  const { options, posts } = await setup({ lost: true });
  const first = await computerSession(options);
  first.setDraft("Keep this between screens");
  await first.describe();
  first.setActive(false);
  const reopened = await computerSession({ ...options, sender: { ...options.sender, holder: await fixtureDeviceKey() } });
  expect(reopened.snapshot().pending?.envelope.id).toBe(posts[0]?.id);
  expect(reopened.snapshot().draft).toBe("Keep this between screens");
  await reopened.start();
  expect(posts).toHaveLength(1);
  const empty = { load: async () => undefined, save: async () => undefined, remove: async () => undefined };
  const separate = await computerSession({ ...options, scope: { ...scope, personId: "another-person" }, journal: empty });
  expect(separate.snapshot().draft).toBe("");
  expect(separate.snapshot().pending).toBeUndefined();
  const settings = await computerSession({ ...options, threadId: FIXTURE.threadId, journal: empty });
  expect(settings.snapshot().draft).toBe("");
  expect(settings.snapshot().pending).toBeUndefined();
  expect(posts).toHaveLength(1);
});

const agents: ComputerAgentChoices[] = [{ agent: "claude", name: "Claude Code", routes: [
  { via: "subscription", payer: "plan", label: "Claude subscription", detail: "On this computer", state: "usable", models: [{ id: "sonnet", name: "Sonnet", efforts: ["low", "high"] }], defaultModel: "sonnet" },
  { via: "nexus", payer: "gateway", label: "Nexus account", detail: null, state: "usable", models: [{ id: "opus", name: "Opus", efforts: ["medium", "high"] }], defaultModel: "opus" },
] }];
const configured = { ...project, settings: { ...project.settings, model: "sonnet", effort: "low" }, payer: { kind: "plan" as const, label: "Claude subscription", detail: null } };
const sessionSettings: ComputerSessionSettings = { threadId: FIXTURE.threadId, revision: "settings-1", agent: "claude", via: "subscription", model: "sonnet", effort: "low", payer: { kind: "plan", label: "Claude subscription", detail: "On this computer" }, pending: null, models: [{ id: "sonnet", name: "Sonnet", efforts: ["low", "high"] }, { id: "opus", name: "Opus", efforts: ["high"] }], editable: true, reason: null };

describe("remote model and payment choices", () => {
  it("encrypts explicit selections for the configured project agent and keeps them through a stale-choice response", async () => {
    const changed: ComputerAgentChoices[] = [{ ...agents[0]!, routes: agents[0]!.routes.filter((route) => route.payer === "plan") }];
    const { session, posts, operations, saved, reopen } = await setup({ settings: true, replies: [{ kind: "projects", projects: [configured], agents }, { kind: "choicesChanged", projects: [configured], agents: changed }] });
    await session.describe();
    expect(session.snapshot().selection).toEqual({ via: "subscription", payer: "plan", model: "sonnet", effort: "low" });
    const selected = { via: "nexus" as const, payer: "gateway" as const, model: "opus", effort: "high" as const };
    session.select(selected);
    session.setDraft("Keep my prompt and choices");
    await session.start();
    expect(operations[1]).toEqual({ kind: "startSession", projectId: "project", revision: "r1", text: "Keep my prompt and choices", selection: selected });
    expect(JSON.stringify(posts[1])).not.toContain("Keep my prompt");
    expect(session.snapshot().selection).toEqual(selected);
    expect(saved()?.selection).toEqual(selected);
    expect((await reopen()).snapshot().selection).toEqual(selected);
    await session.start();
    expect(posts).toHaveLength(2);
    expect(session.snapshot().error).toContain("choices changed");
    expect(session.snapshot().draft).toBe("Keep my prompt and choices");
  });

  it("does not send settings to an old runner", async () => {
    const { session, posts } = await setup({ threadId: FIXTURE.threadId });
    await session.describe();
    expect(posts).toHaveLength(0);
    expect(session.snapshot().error).toContain("Update Nexus on this computer");
  });

  it("restores an uncertain model update GET-only with its exact request id and chosen values", async () => {
    const { session, posts, operations, reopen } = await setup({ settings: true, threadId: FIXTURE.threadId, loseAt: 2, replies: [{ kind: "sessionSettings", session: sessionSettings }] });
    await session.describe();
    session.edit({ model: "opus", effort: "high" });
    await session.apply();
    expect(operations[1]).toEqual({ kind: "updateSession", threadId: FIXTURE.threadId, revision: "settings-1", settings: { model: "opus", effort: "high" } });
    const restored = await reopen();
    restored.setActive(true);
    try {
      await vi.waitFor(() => expect(restored.snapshot().computers).toHaveLength(1));
      expect(restored.snapshot().pending?.envelope.id).toBe(posts[1]?.id);
      expect(restored.snapshot().edit).toEqual({ model: "opus", effort: "high" });
      await restored.apply();
      await restored.describe();
      await vi.advanceTimersByTimeAsync(35_000);
      expect(posts).toHaveLength(2);
    } finally { restored.setActive(false); }
  });

  it("shows saved settings as pending until the agent acknowledges, then stops read polling", async () => {
    const pending = { ...sessionSettings, revision: "settings-2", pending: { model: "opus", effort: "high" as const } };
    const effective = { ...pending, model: "opus", effort: "high" as const, pending: null };
    const { session, posts, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: sessionSettings }, { kind: "sessionUpdated", session: pending }, { kind: "sessionSettings", session: pending }, { kind: "sessionSettings", session: effective }] });
    await session.describe();
    session.edit({ model: "opus", effort: "high" });
    await session.apply();
    expect(session.snapshot().settings?.model).toBe("sonnet");
    expect(session.snapshot().settings?.pending?.model).toBe("opus");
    session.setActive(true);
    try {
      await vi.waitFor(() => expect(posts).toHaveLength(3));
      // The fake POST records its envelope before the encrypted reply finishes.
      await vi.waitFor(() => expect(session.snapshot().preparing).toBe(false));
      await vi.advanceTimersByTimeAsync(5000);
      await vi.waitFor(() => expect(session.snapshot().settings?.pending).toBeNull());
      expect(session.snapshot().settings?.model).toBe("opus");
      await vi.advanceTimersByTimeAsync(45_000);
      expect(operations.map((operation) => operation.kind)).toEqual(["describeSession", "updateSession", "describeSession", "describeSession"]);
    } finally { session.setActive(false); }
  });

  it.each([false, true])("refreshes cached model A after synced B, preserving unsaved edits when present (%s)", async (edited) => {
    const changed = { ...sessionSettings, revision: "settings-2", model: "opus", effort: "high" as const, models: [{ id: "opus", name: "Opus from computer", efforts: ["high" as const] }] };
    const { session, posts, operations, saved } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: sessionSettings }, { kind: "sessionSettings", session: changed }] });
    await session.describe();
    session.observeModel("sonnet");
    if (edited) session.edit({ model: "sonnet", effort: "high" });
    session.observeModel("opus");
    // The sync hint is never used as the descriptor or the advertised model choices.
    expect(session.snapshot().settings?.model).toBe("sonnet");
    session.setActive(true);
    try {
      await vi.waitFor(() => expect(session.snapshot().settings?.model).toBe("opus"));
      expect(session.snapshot().settings?.models[0]?.name).toBe("Opus from computer");
      const kept = edited ? { model: "sonnet", effort: "high" } : { model: "opus", effort: "high" };
      expect(session.snapshot().edit).toEqual(kept);
      expect(saved()?.edit).toEqual(kept);
      for (let n = 0; n < 20; n++) session.observeModel("opus");
      await vi.advanceTimersByTimeAsync(60_000);
      expect(posts).toHaveLength(2);
      expect(operations.map((operation) => operation.kind)).toEqual(["describeSession", "describeSession"]);
    } finally { session.setActive(false); }
  });

  it("initializes first-described requested settings from pending fields including explicit null", async () => {
    const pending = { ...sessionSettings, pending: { model: "opus", effort: null } };
    const { session } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: pending }] });
    await session.describe();
    expect(session.snapshot().settings?.model).toBe("sonnet");
    expect(session.snapshot().settings?.effort).toBe("low");
    expect(session.snapshot().edit).toEqual({ model: "opus", effort: null });
    expect(requestedSettings(session.snapshot().settings!)).toEqual({ model: "opus", effort: null });
  });

  it("keeps an unsaved model apart from the computer's actual pending model and preserves omitted fields", async () => {
    const pending = { ...sessionSettings, revision: "settings-2", pending: { model: "opus" } };
    const { session, reopen } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: sessionSettings }, { kind: "sessionSettings", session: pending }] });
    await session.describe();
    session.edit({ model: null, effort: null });
    await session.describe();
    expect(session.snapshot().edit).toEqual({ model: null, effort: null });
    expect(requestedSettings(session.snapshot().settings!)).toEqual({ model: "opus", effort: "low" });
    expect((await reopen()).snapshot().edit).toEqual({ model: null, effort: null });
  });

  it("bounds acknowledgement reads when the desktop agent never confirms the saved model", async () => {
    const pending = { ...sessionSettings, revision: "settings-2", pending: { model: "opus", effort: "high" as const } };
    const { session, posts, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: sessionSettings }, { kind: "sessionUpdated", session: pending }, ...Array.from({ length: 6 }, () => ({ kind: "sessionSettings" as const, session: pending }))] });
    await session.describe();
    session.edit({ model: "opus", effort: "high" });
    await session.apply();
    session.setActive(true);
    try {
      await vi.waitFor(() => expect(posts).toHaveLength(3));
      for (let n = 0; n < 8; n++) {
        await vi.waitFor(() => {
          expect(session.snapshot().preparing).toBe(false);
          expect(session.snapshot().pending).toBeUndefined();
        });
        await vi.advanceTimersByTimeAsync(5000);
      }
      await vi.waitFor(() => expect(session.snapshot().preparing).toBe(false));
      const bounded = posts.length;
      // The deadline may end polling before the count limit when crypto delivery is slow.
      expect(bounded).toBeGreaterThanOrEqual(3);
      expect(bounded).toBeLessThanOrEqual(7);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(posts).toHaveLength(bounded);
      expect(operations.filter((operation) => operation.kind === "updateSession")).toHaveLength(1);
      expect(session.snapshot().settings?.model).toBe("sonnet");
      expect(session.snapshot().settings?.pending?.model).toBe("opus");
    } finally { session.setActive(false); }
  });

  it("retains a rejected stale edit and uses the refreshed revision only after another deliberate apply", async () => {
    const changed = { ...sessionSettings, revision: "settings-2", effort: "high" as const };
    const { session, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: sessionSettings }, { kind: "sessionChanged", session: changed }, { kind: "sessionUpdated", session: { ...changed, model: "opus" } }] });
    await session.describe();
    session.edit({ model: "opus", effort: "high" });
    await session.apply();
    expect(session.snapshot().edit).toEqual({ model: "opus", effort: "high" });
    expect(session.snapshot().notice).toContain("Your selection is kept");
    expect(operations).toHaveLength(2);
    await session.apply();
    expect(operations[2]).toEqual({ kind: "updateSession", threadId: FIXTURE.threadId, revision: "settings-2", settings: { model: "opus", effort: "high" } });
  });

  it("shows session of agent from newer computer without sending an edit", async () => {
    // The fake computer seals what it is given, so the reply can hold an agent this phone's types do not name.
    const future = { ...sessionSettings, agent: "future-agent", effort: "future-level" } as unknown as ComputerSessionSettings;
    const { session, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: future }] });
    await session.describe();
    expect(session.snapshot().error).toBeUndefined();
    expect(session.snapshot().settings).toMatchObject({ agent: "unknown", effort: null, editable: false });
    session.edit({ model: "opus", effort: "high" });
    await session.apply();
    expect(operations.map((operation) => operation.kind)).toEqual(["describeSession"]);
  });

  it("starts project with level from newer computer without selection, so computer keeps its level", async () => {
    const future = { ...configured, settings: { ...configured.settings, effort: "future-level" } };
    const { session, operations } = await setup({ settings: true, replies: [{ kind: "projects", projects: [future], agents }] });
    await session.describe();
    expect(session.snapshot().selection).toBeUndefined();
    session.setDraft("Use the project's level");
    await session.start();
    expect(operations[1]).toEqual({ kind: "startSession", projectId: "project", revision: "r1", text: "Use the project's level" });
  });

  it("drops selection when refreshed project reports level from newer computer", async () => {
    const future = { ...configured, settings: { ...configured.settings, effort: "future-level" } };
    const { session, operations } = await setup({ settings: true, replies: [{ kind: "projects", projects: [configured], agents }, { kind: "choicesChanged", projects: [future], agents }] });
    await session.describe();
    expect(session.snapshot().selection).toEqual({ via: "subscription", payer: "plan", model: "sonnet", effort: "low" });
    session.setDraft("Keep the computer's level");
    await session.start();
    expect(session.snapshot().selection).toBeUndefined();
    await session.start();
    expect(operations[2]).toEqual({ kind: "startSession", projectId: "project", revision: "r1", text: "Keep the computer's level" });
  });

  it("keeps project level from newer computer until person picks level", async () => {
    const future = { ...configured, settings: { ...configured.settings, effort: "future-level" } };
    const { session, operations } = await setup({ settings: true, replies: [{ kind: "projects", projects: [future], agents }] });
    await session.describe();
    session.setDraft("Keep the computer's level");
    // A payer pick carries no level of the person's own, so it would replace the project's level.
    session.select({ via: "subscription", payer: "plan", model: "sonnet", effort: null });
    await session.start();
    expect(operations.map((operation) => operation.kind)).toEqual(["describeProjects"]);
    session.select({ via: "subscription", payer: "plan", model: "sonnet", effort: null }, true);
    await session.start();
    expect(operations[1]).toEqual({ kind: "startSession", projectId: "project", revision: "r1", text: "Keep the computer's level", selection: { via: "subscription", payer: "plan", model: "sonnet", effort: null } });
  });

  it("clears picked level when model change drops it", async () => {
    const future = { ...sessionSettings, effort: "future-level", models: [{ id: "sonnet", name: "Sonnet", efforts: ["low", "high"] }, { id: "haiku", name: "Haiku", efforts: ["low"] }] } as unknown as ComputerSessionSettings;
    const { session, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: future }] });
    await session.describe();
    session.edit({ model: "sonnet", effort: "high" }, true);
    session.edit({ model: "haiku", effort: null });
    expect(session.snapshot().effortPicked).toBe(false);
    await session.apply();
    expect(operations.map((operation) => operation.kind)).toEqual(["describeSession"]);
  });

  it("keeps level set on newer computer until person picks level", async () => {
    const future = { ...sessionSettings, effort: "future-level" } as unknown as ComputerSessionSettings;
    const { session, operations } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: future }, { kind: "sessionUpdated", session: { ...sessionSettings, model: "opus", effort: null } }] });
    await session.describe();
    expect(session.snapshot().settings).toMatchObject({ effort: null, effortUnknown: true, editable: true });
    expect(session.snapshot().edit).toEqual({ model: "sonnet", effort: null });
    // A model change carries no level of the person's own, so it would replace the computer's level.
    session.edit({ model: "opus", effort: null });
    await session.apply();
    expect(operations.map((operation) => operation.kind)).toEqual(["describeSession"]);
    session.edit({ model: "opus", effort: null }, true);
    await session.apply();
    expect(operations[1]).toEqual({ kind: "updateSession", threadId: FIXTURE.threadId, revision: "settings-1", settings: { model: "opus", effort: null } });
    expect(session.snapshot().effortPicked).toBe(false);
  });

  it("keeps tracking a settings reply for the wrong thread instead of presenting it", async () => {
    const { session } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [{ kind: "sessionSettings", session: { ...sessionSettings, threadId: "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f509" } }] });
    await session.describe();
    expect(session.snapshot().settings).toBeUndefined();
    expect(session.snapshot().pending).toBeDefined();
    expect(session.snapshot().error).toContain("different session");
  });
});


describe("automatic computer reconnection", () => {
  it.each([false, true])("recovers after the paired computer wakes without a scan or a session mutation (existing session: %s)", async (existing) => {
    const reply: ComputerResult = existing ? { kind: "sessionSettings", session: sessionSettings } : { kind: "projects", projects: [configured], agents };
    const { session, operations, posts, sleep, wake } = await setup({ asleep: true, settings: true, threadId: existing ? FIXTURE.threadId : undefined, replies: [reply, reply] });
    session.setDraft("Keep my unsent message");
    await session.describe();
    expect(posts).toHaveLength(0);
    session.setActive(true);
    try {
      await vi.waitFor(() => expect(session.snapshot().computers).toHaveLength(1));
      await vi.waitFor(() => expect(session.snapshot().error).toBeUndefined());
      wake();
      await vi.advanceTimersByTimeAsync(3500);
      await vi.waitFor(() => expect(existing ? session.snapshot().settings?.model : session.snapshot().projects[0]?.id).toBe(existing ? "sonnet" : configured.id));
      if (existing) session.edit({ model: "opus", effort: "high" });
      sleep();
      await vi.advanceTimersByTimeAsync(15000);
      await vi.waitFor(() => { const computer = session.snapshot().computers[0]; expect(computer && session.awake(computer)).toBe(false); });
      wake();
      await vi.advanceTimersByTimeAsync(3500);
      await vi.waitFor(() => expect(operations).toHaveLength(2));
      expect(operations.map((operation) => operation.kind)).toEqual([existing ? "describeSession" : "describeProjects", existing ? "describeSession" : "describeProjects"]);
      expect(session.snapshot().draft).toBe("Keep my unsent message");
      if (existing) expect(session.snapshot().edit).toEqual({ model: "opus", effort: "high" });
    } finally { session.setActive(false); }
  });
});


it("recovers the same paired session after Gateway connectivity returns without applying unsaved edits", async () => {
  const reply: ComputerResult = { kind: "sessionSettings", session: sessionSettings };
  const { session, operations, disconnect, reconnect } = await setup({ settings: true, threadId: FIXTURE.threadId, replies: [reply, reply] });
  await session.describe();
  session.setDraft("Keep this while the Mac sleeps");
  session.edit({ model: "opus", effort: "high" });
  disconnect();
  session.setActive(true);
  try {
    await vi.waitFor(() => expect(session.snapshot().error).toContain("Gateway connection lost"));
    reconnect();
    await vi.advanceTimersByTimeAsync(3500);
    await vi.waitFor(() => expect(operations).toHaveLength(2));
    expect(operations.map((operation) => operation.kind)).toEqual(["describeSession", "describeSession"]);
    expect(session.snapshot().draft).toBe("Keep this while the Mac sleeps");
    expect(session.snapshot().edit).toEqual({ model: "opus", effort: "high" });
    expect(session.snapshot().error).toBeUndefined();
  } finally { session.setActive(false); }
});

describe("manual compaction on a paired session", () => {
  it("sends the owner-scoped action through the journal and handles it without changing the draft", async () => {
    const threadId = FIXTURE.threadId;
    const run = await setup({ settings: true, threadId, replies: [
      { kind: "sessionSettings", session: { ...sessionSettings, threadId } },
      { kind: "compacted", threadId },
    ] });
    await run.session.describe();
    run.session.setDraft("Keep my next prompt");
    await run.session.compact();
    expect(run.operations.at(-1)).toEqual({ kind: "compactSession", threadId });
    expect(run.session.snapshot()).toMatchObject({ draft: "Keep my next prompt", notice: "The computer accepted compaction." });
  });
});

describe("handoff to another paired agent", () => {
  it("loads authorized options and sends only the edited text with the selected project and payer", async () => {
    const threadId = FIXTURE.threadId;
    const route = { via: "nexus", payer: "gateway", label: "Nexus", detail: null, state: "usable", models: [], defaultModel: null } as const;
    const run = await setup({
      settings: true, threadId, replies: [{
        kind: "handoffChoices", threadId, project,
        agents: [{ agent: "codex", name: "Codex", routes: [{ ...route, models: [] }] }],
      }, {
        kind: "created", threadId: "0192f3a4-0000-7000-8000-000000000004",
        origin: { kind: "computer", computerId, name: "Laptop" }, launch: "starting",
      }],
    });
    const chosen = { via: "nexus", payer: "gateway", model: null, effort: null } as const;
    run.session.setDraft("The source draft remains private");
    await run.session.startHandoff("Unreviewed", "codex", chosen);
    expect(run.posts).toHaveLength(0);
    await run.session.describeHandoff();
    expect(run.operations).toEqual([{ kind: "describeHandoff", threadId }]);
    await run.session.startHandoff("An unavailable choice", "claude", chosen);
    await run.session.startHandoff("An unavailable payer", "codex", { via: "subscription", payer: "plan", model: null, effort: null });
    expect(run.posts).toHaveLength(1);
    await run.session.startHandoff("Redacted and approved handoff", "codex", chosen);
    expect(run.operations[1]).toEqual({
      kind: "startSession", projectId: project.id, revision: project.revision, text: "Redacted and approved handoff",
      handoff: { sourceThreadId: threadId, agent: "codex" }, selection: chosen,
    });
    expect(run.session.snapshot().draft).toBe("The source draft remains private");
    expect(run.session.snapshot().created?.threadId).toBe("0192f3a4-0000-7000-8000-000000000004");
  });
});
