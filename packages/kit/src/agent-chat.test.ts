import { describe, expect, it } from "vitest";
import { AGENT_TEXT_CHARS, AgentEventReader, agentProblem, agentWide, answerConnects, answerSuggestions, answerTexts, approvalsViewOf, connectLine, conversationListOf, createAgentClient, learningOf, memoryNoteOf, memoryViewOf, noteSourceText, readAgentStream, noticeErrand, safeUrl, type AgentEvent, type AgentMessage } from "./agent-chat";

// The apps' side of the agent API (S2): its event stream, read in any pieces, the words for each failure, and the
// client the phone app calls it with (S2.5), against a fake fetch.

const STREAM = [
  'event: started\ndata: {"conversation":"c1"}\n\n',
  ": keepalive\n\n",
  'event: model\ndata: {"model":"anthropic/claude-sonnet-5"}\n\n',
  'event: text\ndata: {"delta":"Hello "}\n\n',
  'event: text\ndata: {"delta":"**Ada**"}\n\n',
  'event: done\ndata: {"status":200,"conversation":"c1","platform":{"name":"Nexus","markup":"markdown"},"messages":[{"kind":"answer","text":"Hello **Ada**","model":"anthropic/claude-sonnet-5","costUsd":0.0123}]}\n\n',
].join("");

function chunked(text: string, size: number): ReadableStream<Uint8Array> {
  const bytes = new TextEncoder().encode(text);
  let at = 0;
  return new ReadableStream({
    pull(controller) {
      if (at >= bytes.length) return controller.close();
      controller.enqueue(bytes.slice(at, at + size));
      at += size;
    },
  });
}

describe("the agent API's event stream", () => {
  it("reads every event, whatever the pieces, and skips keepalives", async () => {
    for (const size of [1, 3, 17, 4096]) {
      const events: AgentEvent[] = [];
      await readAgentStream(chunked(STREAM, size), (event) => events.push(event));
      expect(events.map((event) => event.type)).toEqual(["started", "model", "text", "text", "done"]);
      expect(events.filter((e) => e.type === "text").map((e) => (e.type === "text" ? e.delta : "")).join("")).toBe("Hello **Ada**");
      const done = events.at(-1);
      expect(done?.type === "done" && done.reply).toMatchObject({ status: 200, conversation: "c1", messages: [{ kind: "answer", costUsd: 0.0123 }] });
    }
  });

  it("reads CRLF line ends and a last event without its blank line", async () => {
    const events: AgentEvent[] = [];
    await readAgentStream(chunked('event: text\r\ndata: {"delta":"a"}\r\n\r\nevent: error\ndata: {"status":504,"error":"timeout","detail":"Too long."}', 5), (e) => events.push(e));
    expect(events).toEqual([
      { type: "text", delta: "a" },
      { type: "error", status: 504, error: "timeout", detail: "Too long." },
    ]);
  });

  it("reads a step line strictly: one line of plain text, short (A1·C b)", () => {
    const reader = new AgentEventReader();
    const long = "x".repeat(81);
    const events = reader.push(
      `event: step\ndata: {"line":" Searching Linear "}\n\nevent: step\ndata: {"line":"a\\nb"}\n\nevent: step\ndata: {"line":"${long}"}\n\nevent: step\ndata: {"line":3}\n\n`,
    );
    expect(events).toEqual([{ type: "step", line: "Searching Linear" }]);
  });

  it("drops unknown events and data that is not JSON", () => {
    const reader = new AgentEventReader();
    expect(reader.push("event: tool\ndata: {}\n\nevent: text\ndata: not json\n\nevent: text\ndata: {\"delta\":1}\n\n")).toEqual([]);
  });
});

describe("what the page says when an ask fails", () => {
  it("asks to sign in again on 401", () => {
    expect(agentProblem(401, { error: "credential_refused", detail: "Sign in again with `nexus auth login`." })).toMatchObject({ title: "Sign in again", signIn: true });
  });

  it("shows the API's own hint on 403", () => {
    const hint = "This agent link was not granted models.use, so Nexus cannot answer through it.";
    expect(agentProblem(403, { error: "models_use_not_granted", detail: hint })).toEqual({ title: "Nexus cannot answer with this sign-in", detail: hint });
  });

  it("says plain words on 429 and 504, not the API's", () => {
    expect(agentProblem(429, { error: "rate_limited", detail: "You asked more than 10 questions in a minute." })).toEqual({ title: "Too many questions at once", detail: "Wait a moment, then ask again." });
    expect(agentProblem(504, { error: "timeout", detail: "x" }).title).toBe("The answer took too long");
  });

  it("says Nexus is not available here when the UI is not connected", () => {
    expect(agentProblem(503, { error: "agents_api_unconfigured" }).title).toBe("Nexus is not available here");
  });

  it("says Nexus is not available here, with no sign-in, when its service refused the UI's token", () => {
    const problem = agentProblem(503, { error: "agents_api_refused_token", detail: "x" });
    expect(problem.title).toBe("Nexus is not available here");
    expect(problem.detail).toMatch(/sign-in works/);
    expect(problem.signIn).toBeUndefined();
  });

  it("names the step after a new sign-in that the view asks for", () => {
    expect(agentProblem(401).detail).toBe("Your sign-in ended. Sign in again, then ask your question again.");
    expect(agentProblem(401, undefined, "open What it knows again").detail).toBe("Your sign-in ended. Sign in again, then open What it knows again.");
  });

  it("reads a sign-in, a refused credential and an agent API not available here the same in every view", () => {
    for (const [status, error] of [[401, ""], [403, "not_a_person"], [503, "agents_api_unconfigured"], [503, "agents_api_refused_token"]] as const) {
      expect(agentWide(status, { error }), `${status} ${error}`).toBe(true);
    }
    for (const [status, error] of [[503, "memory_unavailable"], [503, ""], [404, ""], [502, ""]] as const) {
      expect(agentWide(status, { error }), `${status} ${error}`).toBe(false);
    }
  });

  it("has a plain answer for anything else", () => {
    expect(agentProblem(502).title).toBe("Nexus did not answer");
  });
});

it("opens only http(s) links", () => {
  expect(safeUrl("https://nexus.test/app/acme/approve/1")).toBe("https://nexus.test/app/acme/approve/1");
  expect(safeUrl("javascript:alert(1)")).toBeUndefined();
  expect(safeUrl(42)).toBeUndefined();
});

describe("the agent client, against a fake API", () => {
  type Seen = { url: string; method: string; headers: Record<string, string>; body?: string };
  const fake = (seen: Seen[], answer: () => Response) =>
    createAgentClient({
      baseUrl: "https://agents.invalid/",
      apiKey: "nxs_token",
      client: "mobile/0.1.0",
      fetch: async (url, init) => {
        seen.push({ url: String(url), method: init?.method ?? "GET", headers: init?.headers as Record<string, string>, body: init?.body as string | undefined });
        return answer();
      },
    });
  const sse = (text: string) => new Response(chunked(text, 7), { status: 200, headers: { "content-type": "text/event-stream" } });
  const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

  it("asks with the person's own token, streamed, in the conversation, and hands on each event", async () => {
    const seen: Seen[] = [];
    const events: string[] = [];
    const asked = await fake(seen, () => sse(STREAM)).ask("Who am I?", { conversation: "c1", onEvent: (e) => events.push(e.type) });
    expect(seen).toEqual([
      {
        url: "https://agents.invalid/agent/ask",
        method: "POST",
        headers: { "content-type": "application/json", accept: "text/event-stream", authorization: "Bearer nxs_token", "x-nexus-client": "mobile/0.1.0" },
        body: JSON.stringify({ message: "Who am I?", stream: true, conversation: "c1" }),
      },
    ]);
    expect(events).toEqual(["started", "model", "text", "text", "done"]);
    expect(asked).toMatchObject({ kind: "reply", reply: { conversation: "c1", messages: [{ kind: "answer", text: "Hello **Ada**" }] } });
  });

  it("through Gateway: calls /v1/agent/*, keys every write, streams, and reads Gateway's own refusal", async () => {
    const seen: Seen[] = [];
    const via = (answer: () => Response) =>
      createAgentClient({
        baseUrl: "https://gateway.invalid/",
        apiKey: "nxs_token",
        client: "mobile/0.1.0",
        viaGateway: true,
        fetch: async (url, init) => {
          seen.push({ url: String(url), method: init?.method ?? "GET", headers: init?.headers as Record<string, string>, body: init?.body as string | undefined });
          return answer();
        },
      });
    const events: string[] = [];
    expect(await via(() => sse(STREAM)).ask("Who am I?", { onEvent: (e) => events.push(e.type) })).toMatchObject({ kind: "reply" });
    expect(events).toEqual(["started", "model", "text", "text", "done"]);
    expect(seen[0]).toMatchObject({ url: "https://gateway.invalid/v1/agent/ask", method: "POST", headers: { authorization: "Bearer nxs_token" } });
    expect(seen[0]!.headers["idempotency-key"]).toMatch(/\S{8,}/);
    await via(() => json(200, { level: "balanced" })).proactivity();
    expect(seen[1]).toMatchObject({ url: "https://gateway.invalid/v1/agent/proactivity", method: "GET" });
    expect(seen[1]!.headers["idempotency-key"]).toBeUndefined();
    const unreachable = { error: { message: "The agent service did not answer.", type: "api_error", code: "agents_api_unreachable" } };
    expect(await via(() => json(502, unreachable)).ask("x")).toEqual({
      kind: "failed",
      failure: { status: 502, error: "agents_api_unreachable", detail: "The agent service did not answer." },
    });
    // Off (the default), nothing changes: the agent API's URL, and no key.
    const direct: Seen[] = [];
    await fake(direct, () => sse(STREAM)).ask("Hi");
    expect(direct[0]!.url).toBe("https://agents.invalid/agent/ask");
    expect(direct[0]!.headers["idempotency-key"]).toBeUndefined();
  });

  it("sends no conversation on a first question", async () => {
    const seen: Seen[] = [];
    await fake(seen, () => sse(STREAM)).ask("Hi");
    expect(JSON.parse(seen[0]!.body!)).toEqual({ message: "Hi", stream: true });
  });

  it("returns the API's failure, before the stream and inside it, and a stream that stops early", async () => {
    expect(await fake([], () => json(403, { error: "not_a_person", detail: "Use your own key." })).ask("x")).toEqual({
      kind: "failed",
      failure: { status: 403, error: "not_a_person", detail: "Use your own key." },
    });
    expect(await fake([], () => sse('event: error\ndata: {"status":504,"error":"timeout","detail":"Too long."}\n\n')).ask("x")).toEqual({
      kind: "failed",
      failure: { status: 504, error: "timeout", detail: "Too long." },
    });
    expect(await fake([], () => sse('event: started\ndata: {"conversation":"c1"}\n\nevent: text\ndata: {"delta":"Hel"}\n\n')).ask("x")).toEqual({ kind: "cut" });
  });

  it("reads the proactivity and sets it, and says when the API has none (A10)", async () => {
    const seen: Seen[] = [];
    const view = { level: "balanced", ownLevel: false, agentProactivity: "ask_first", muted: [], quietHours: null, batchMinutes: 15 };
    expect(await fake(seen, () => json(200, view)).proactivity()).toEqual({ kind: "view", view });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/proactivity", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    const hours = { start: "22:00", end: "07:00", timezone: "Europe/Madrid" };
    expect(await fake(seen, () => json(200, { ...view, level: "quiet", quietHours: hours })).setProactivity({ level: "quiet", quietHours: hours })).toMatchObject({ kind: "view", view: { level: "quiet", quietHours: hours } });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/proactivity", method: "PUT", body: JSON.stringify({ level: "quiet", quietHours: hours }) });
    expect(await fake([], () => json(503, { error: "proactivity_off" })).proactivity()).toEqual({ kind: "off" });
    expect(await fake([], () => json(200, { level: "loud" })).proactivity()).toEqual({ kind: "failed", status: 502 });
  });

  it("takes a reply in one piece, as JSON", async () => {
    expect(await fake([], () => json(200, { status: 200, conversation: "c2", messages: [{ kind: "notice", topic: "t", state: "on", text: "On." }] })).ask("x")).toMatchObject({
      kind: "reply",
      reply: { conversation: "c2", messages: [{ kind: "notice" }] },
    });
  });

  it("reads a conversation back by its id, encoded, and says when it is gone", async () => {
    const seen: Seen[] = [];
    const turns = [{ role: "user", text: "Hi", at: "2026-11-04T10:00:00.000Z" }];
    expect(await fake(seen, () => json(200, { conversation: "c/1", turns })).conversation("c/1")).toEqual({ kind: "turns", turns });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/conversations/c%2F1", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    const displayTurns = [...turns, { role: "assistant", text: "Tools are unavailable.", at: "2026-11-04T10:00:01.000Z", messages: [{ kind: "notice", topic: "tools", state: "unavailable", text: "Tools are unavailable." }] }];
    expect(await fake([], () => json(200, { turns, displayTurns })).conversation("c1")).toEqual({ kind: "turns", turns: displayTurns });
    expect(await fake([], () => json(404, { error: "not_found" })).conversation("c1")).toEqual({ kind: "gone" });
    expect(await fake([], () => json(401, { error: "credential_refused" })).conversation("c1")).toEqual({ kind: "failed", failure: { status: 401, error: "credential_refused" } });
  });

  it("reads the privacy view, pauses and resumes, and revokes all access with the confirm (A11)", async () => {
    const seen: Seen[] = [];
    const view = { pause: { paused: false }, links: [], accounts: [] };
    expect(await fake(seen, () => json(200, view)).privacy()).toEqual({ kind: "ok", value: view });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/privacy", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    const paused = { paused: true, pausedAt: "2026-11-17T14:02:00.000Z" };
    expect(await fake(seen, () => json(200, paused)).setPaused(true)).toEqual({ kind: "ok", value: paused });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/pause", method: "PUT", body: '{"paused":true}' });
    expect(await fake(seen, () => json(200, paused)).pause()).toEqual({ kind: "ok", value: paused });
    const result = { links: { ended: [], failed: [] }, accounts: { disconnected: [], failed: [] } };
    expect(await fake(seen, () => json(200, result)).revokeAll(true)).toEqual({ kind: "ok", value: result });
    expect(seen[3]).toMatchObject({ url: "https://agents.invalid/agent/revoke", method: "POST", body: '{"confirm":true,"accounts":true}' });
    expect(await fake([], () => json(404, {})).privacy()).toEqual({ kind: "off" });
    expect(await fake([], () => json(502, { error: "gateway", detail: "Down." })).revokeAll(false)).toEqual({ kind: "failed", failure: { status: 502, error: "gateway", detail: "Down." } });
    expect(await fake([], () => json(200, { pause: { paused: "yes" } })).privacy()).toEqual({ kind: "failed", failure: { status: 502 } });
  });

  it("reads the memory, and forgets, adds, edits and pauses learning with the person's token (A16)", async () => {
    const seen: Seen[] = [];
    const note = { id: "n1", text: "Ana is my manager.", createdAt: "2026-11-01T09:00:00.000Z", audience: "dm" };
    expect(await fake(seen, () => json(200, { notes: [note], savedModel: "auto", controls: true })).memory()).toEqual({ kind: "view", view: { notes: [note], savedModel: "auto", controls: true } });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/memory", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    expect(await fake(seen, () => json(200, { forgotten: 1 })).changeMemory({ forget: "n 1" })).toEqual({ kind: "forgotten", count: 1 });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/memory/notes/n%201", method: "DELETE" });
    expect(seen[1]!.body).toBeUndefined();
    expect(await fake(seen, () => json(200, { forgotten: 3 })).changeMemory({ forgetAll: true })).toEqual({ kind: "forgotten", count: 3 });
    expect(seen[2]).toMatchObject({ url: "https://agents.invalid/agent/memory", method: "DELETE" });
    expect(await fake(seen, () => json(200, { note })).changeMemory({ add: "Ana is my manager." })).toEqual({ kind: "note", note });
    expect(seen[3]).toMatchObject({ url: "https://agents.invalid/agent/memory/notes", method: "POST", body: '{"text":"Ana is my manager."}' });
    expect(await fake(seen, () => json(200, { note })).changeMemory({ edit: "n1", text: "Ana is my manager." })).toEqual({ kind: "note", note });
    expect(seen[4]).toMatchObject({ url: "https://agents.invalid/agent/memory/notes/n1", method: "PATCH" });
    expect(await fake(seen, () => json(200, { learning: { paused: true } })).changeMemory({ paused: true })).toEqual({ kind: "learning", learning: { paused: true } });
    expect(seen[5]).toMatchObject({ url: "https://agents.invalid/agent/memory/learning", method: "PUT", body: '{"paused":true}' });
    expect(await fake([], () => json(404, { error: "not_found" })).changeMemory({ add: "x" })).toEqual({ kind: "failed", failure: { status: 404, error: "not_found" } });
    expect(await fake([], () => json(200, { notes: "no" })).memory()).toEqual({ kind: "failed", failure: { status: 502 } });
  });

  it("reads the approval settings (A9·g, h)", async () => {
    const seen: Seen[] = [];
    const view = { tiers: "on", spendLimitUsd: "50.00", ownLine: false, defaultSpendLimitUsd: "50.00", maxSpendLimitUsd: "500.00", rules: [] };
    const read = await fake(seen, () => json(200, view)).approvals();
    expect(read).toEqual({ kind: "view", view });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/approvals/settings", method: "GET" });
    expect(await fake([], () => json(503, { error: "agents_api_unconfigured" })).approvals()).toEqual({ kind: "failed", failure: { status: 503, error: "agents_api_unconfigured" } });
  });

  it("names the app's surface on an ask, and none when the app gives none", async () => {
    const seen: Seen[] = [];
    await fake(seen, () => sse(STREAM)).ask("Hi", { conversation: "c_slack", surface: "mobile" });
    expect(JSON.parse(seen[0]!.body!)).toEqual({ message: "Hi", stream: true, conversation: "c_slack", surface: "mobile" });
  });

  it("lists the person's conversations a page at a time, with the cursor, and says when the API keeps none", async () => {
    const seen: Seen[] = [];
    const row = { id: "c_1", title: "Deploy plan", surface: "slack", updatedAt: "2026-11-09T10:00:00.000Z", turns: 4, preview: "Ship it Friday." };
    expect(await fake(seen, () => json(200, { conversations: [row], next: "cur/2" })).conversations({ limit: 20, before: "cur/1" })).toEqual({ kind: "page", conversations: [row], next: "cur/2" });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/conversations?limit=20&before=cur%2F1", method: "GET", headers: { accept: "application/json", authorization: "Bearer nxs_token" } });
    await fake(seen, () => json(200, { conversations: [], next: null })).conversations();
    expect(seen[1]!.url).toBe("https://agents.invalid/agent/conversations");
    expect(await fake([], () => json(404, { error: "not_found" })).conversations()).toEqual({ kind: "off" });
    expect(await fake([], () => json(503, { error: "conversations_unavailable", detail: "The store did not answer." })).conversations()).toEqual({
      kind: "failed",
      failure: { status: 503, error: "conversations_unavailable", detail: "The store did not answer." },
    });
    expect(await fake([], () => json(200, { rows: [] })).conversations()).toEqual({ kind: "failed", failure: { status: 502 } });
  });

  it("lists the errands, reads one, and sends its action, and says when the API has none (A12)", async () => {
    const seen: Seen[] = [];
    const row = { id: "i3", goal: "Dinner Friday", status: "needs-you", need: "confirm", steps: 0, at: "2026-11-10T08:02:00.000Z" };
    const view = { errand: row, constraints: [], surface: "slack", createdAt: row.at, steps: [], actions: ["confirm", "cancel"], thread: [] };
    expect(await fake(seen, () => json(200, { errands: [row] })).errands()).toEqual({ kind: "list", errands: [row] });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/errands", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    const closed = { ...row, id: "i1", status: "done", archived: true };
    expect(await fake([], () => json(200, { errands: [row], history: [closed, { id: "bad" }] })).errands()).toEqual({ kind: "list", errands: [row], history: [closed] });
    expect(await fake(seen, () => json(200, view)).errand("i3")).toEqual({ kind: "errand", view });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3", method: "GET" });
    expect(await fake(seen, () => json(200, { state: "confirmed", errand: view })).errandAction("i3", "confirm")).toEqual({ kind: "acted", acted: { state: "confirmed", errand: view } });
    expect(seen[2]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3", method: "POST", body: '{"action":"confirm"}' });
    expect(await fake([], () => json(503, { error: "errands_off", detail: "Off." })).errands()).toEqual({ kind: "off" });
    expect(await fake([], () => json(404, { error: "not_found" })).errands()).toEqual({ kind: "off" });
    expect(await fake([], () => json(503, { error: "errands_unavailable", detail: "Later." })).errands()).toEqual({ kind: "failed", failure: { status: 503, error: "errands_unavailable", detail: "Later." } });
    expect(await fake([], () => json(404, { error: "not_found" })).errand("i9")).toEqual({ kind: "gone" });
    expect(await fake([], () => json(409, { error: "closed", detail: "Closed." })).errandAction("i3", "done")).toEqual({ kind: "failed", failure: { status: 409, error: "closed", detail: "Closed." } });
    expect(await fake([], () => json(200, { state: "archived" })).errandAction("i3", "done")).toEqual({ kind: "failed", failure: { status: 502 } });
  });

  it("edits an errand and replies about it", async () => {
    const seen: Seen[] = [];
    const row = { id: "i3", goal: "Dinner Friday", status: "running", steps: 0, at: "2026-11-10T08:02:00.000Z", pinned: true };
    const view = { errand: row, constraints: [], surface: "api", createdAt: row.at, steps: [], actions: ["done", "cancel"], thread: [] };
    expect(await fake(seen, () => json(200, view)).errandEdit("i3", { pinned: true })).toEqual({ kind: "errand", view });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3", method: "PATCH", body: '{"pinned":true}' });
    const answer = { kind: "answer", text: "Booked.", model: "m", costUsd: 0 };
    expect(await fake(seen, () => json(200, { conversation: "errand-i3", messages: [answer], errand: view })).errandReply("i3", "Book it")).toEqual({ kind: "reply", reply: { messages: [answer], errand: view } });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3/messages", method: "POST", body: '{"message":"Book it"}' });
    expect(await fake([], () => json(409, { error: "closed", detail: "Closed." })).errandReply("i3", "hi")).toEqual({ kind: "failed", failure: { status: 409, error: "closed", detail: "Closed." } });
    expect(await fake([], () => json(404, { error: "not_found" })).errandEdit("i9", { done: true })).toEqual({ kind: "gone" });
  });

  it("reads an errand's hand-off and records it (A12·e)", async () => {
    const seen: Seen[] = [];
    const row = { id: "i3", goal: "Fix the decoder", status: "running", steps: 1, at: "2026-11-10T08:02:00.000Z" };
    const view = { errand: row, constraints: [], surface: "api", createdAt: row.at, steps: [{ id: "s1", kind: "code", text: "Handed off to a session", urls: [], at: row.at }], actions: ["done", "cancel"], thread: [] };
    expect(await fake(seen, () => json(200, { prompt: "Continue my errand.", repo: "nexus-xyz/cortex" })).errandHandoff("i3")).toEqual({ kind: "handoff", handoff: { prompt: "Continue my errand.", repo: "nexus-xyz/cortex" } });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3/handoff", method: "GET" });
    expect(await fake(seen, () => json(200, view)).errandHandedOff("i3", "https://nexus.test/app/acme/sessions/s1")).toEqual({ kind: "errand", view });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/errands/i3/handoff", method: "POST", body: '{"url":"https://nexus.test/app/acme/sessions/s1"}' });
    expect(await fake(seen, () => json(200, view)).errandHandedOff("i3")).toEqual({ kind: "errand", view });
    expect(seen[2]?.body).toBe("{}");
    expect(await fake([], () => json(404, { error: "not_found" })).errandHandoff("i9")).toEqual({ kind: "gone" });
    expect(await fake([], () => json(409, { error: "closed", detail: "Closed." })).errandHandedOff("i3")).toEqual({ kind: "failed", failure: { status: 409, error: "closed", detail: "Closed." } });
    expect(await fake([], () => json(200, { prompt: "" })).errandHandoff("i3")).toEqual({ kind: "failed", failure: { status: 502 } });
  });

  it("reads the vault, changes a grant, the card and an ask, and never shows a body shaped like a secret (A8)", async () => {
    const seen: Seen[] = [];
    const vault = { manager: null, logins: [], card: { last4: "4021", state: "active", spent: "142", limit: "500", period: "month" }, merchants: [], applePay: true, asks: [] };
    expect(await fake(seen, () => json(200, vault)).vault()).toEqual({ kind: "view", view: vault });
    expect(seen[0]).toMatchObject({ url: "https://agents.invalid/agent/vault", method: "GET", headers: { authorization: "Bearer nxs_token" } });
    expect(await fake([], () => json(200, { ...vault, password: "hunter2" })).vault()).toEqual({ kind: "failed", status: 502 });
    expect(await fake([], () => json(503, { error: "vault_off" })).vault()).toEqual({ kind: "off" });
    expect(await fake(seen, () => json(200, { vault, stopped: 1 })).vaultWrite({ login: "united", change: { grant: "none" } })).toEqual({ kind: "changed", changed: { vault, stopped: 1 } });
    expect(seen[1]).toMatchObject({ url: "https://agents.invalid/agent/vault/logins/united", method: "PUT", body: '{"grant":"none"}' });
    await fake(seen, () => json(200, { vault })).vaultWrite({ frozen: true });
    expect(seen[2]).toMatchObject({ url: "https://agents.invalid/agent/vault/card", method: "PUT", body: '{"frozen":true}' });
    await fake(seen, () => json(200, { vault, decided: "approved" })).vaultWrite({ ask: "a1", decision: { for: "once" } });
    expect(seen[3]).toMatchObject({ url: "https://agents.invalid/agent/vault/asks/a1/approve", method: "POST", body: '{"for":"once"}' });
    await fake(seen, () => json(200, { vault, decided: "declined" })).vaultWrite({ ask: "a1", decision: "decline" });
    expect(seen[4]).toMatchObject({ url: "https://agents.invalid/agent/vault/asks/a1/decline", method: "POST", body: "{}" });
    expect(await fake([], () => json(410, { error: "gone" })).vaultWrite({ ask: "a1", decision: "decline" })).toEqual({ kind: "failed", status: 410, reason: "gone" });
    expect(await fake([], () => json(200, {})).vaultWrite({ login: "../x", change: { grant: "none" } })).toEqual({ kind: "failed", status: 400 });
    const entry = { at: "2026-11-10T07:41:00.000Z", what: "Checked in UA 1532", with: "United login · 1Password", approved: "Face ID · iPhone", result: "Done", outcome: "done" };
    expect(await fake(seen, () => json(200, { entries: [entry] })).vaultActivity()).toEqual({ kind: "entries", entries: [entry] });
    expect(seen[5]).toMatchObject({ url: "https://agents.invalid/agent/vault/activity", method: "GET" });
    expect(await fake([], () => json(404, {})).vaultActivity()).toEqual({ kind: "off" });
  });
});

describe("the list of conversations, read strictly", () => {
  const at = "2026-11-09T10:00:00.000Z";
  it("keeps each row, reads an unknown surface as api, and leaves out a row that is not one", () => {
    expect(
      conversationListOf({
        conversations: [
          { id: "c_1", title: "From the terminal", surface: "cli", updatedAt: at, turns: 2, preview: "Done." },
          { id: "c_2", title: "Somewhere new", surface: "teams", updatedAt: at },
          { id: "", title: "No id", updatedAt: at },
          { id: "c_3", title: 3, updatedAt: at },
          null,
        ],
      }),
    ).toEqual({
      conversations: [
        { id: "c_1", title: "From the terminal", surface: "cli", updatedAt: at, turns: 2, preview: "Done." },
        { id: "c_2", title: "Somewhere new", surface: "api", updatedAt: at, turns: 0, preview: "" },
      ],
      next: null,
    });
  });

  it("is no list without its rows, or with a cursor that is not text", () => {
    expect(conversationListOf({ next: "x" })).toBeUndefined();
    expect(conversationListOf({ conversations: [], next: 2 })).toBeUndefined();
    expect(conversationListOf("nope")).toBeUndefined();
    expect(conversationListOf({ conversations: [], next: "" })).toEqual({ conversations: [], next: null });
  });
});

describe("the memory view (A16)", () => {
  it("reads the notes with their audience and the saved model, and leaves out what is not a note", () => {
    const at = "2026-11-01T12:00:00.000Z";
    expect(
      memoryViewOf({
        notes: [
          { id: "n1", text: "Said in #design.", createdAt: at, audience: "channel", platform: "slack" },
          { id: "n2", text: "Ships on Friday.", createdAt: at, audience: "dm", extra: "dropped" },
          { id: "n3", text: "No audience.", createdAt: at },
          { id: 4, text: "Not an id.", createdAt: at, audience: "dm" },
          null,
        ],
        savedModel: "openai/gpt-5",
      }),
    ).toEqual({
      notes: [
        { id: "n1", text: "Said in #design.", createdAt: at, audience: "channel", platform: "slack" },
        { id: "n2", text: "Ships on Friday.", createdAt: at, audience: "dm" },
      ],
      savedModel: "openai/gpt-5",
    });
    expect(memoryViewOf({ notes: [] })).toBeUndefined();
    expect(memoryViewOf({ savedModel: "auto" })).toBeUndefined();
    expect(memoryViewOf("nope")).toBeUndefined();
  });

  it("reads the memory controls (A16·a, b): each note's source and edit time, the learning switch, and nothing unknown", () => {
    const at = "2026-11-01T12:00:00.000Z";
    expect(
      memoryViewOf({
        notes: [
          { id: "n1", text: "Told it.", createdAt: at, audience: "dm", source: { kind: "told", surface: "slack" }, editedAt: at },
          { id: "n2", text: "Unknown source.", createdAt: at, audience: "dm", source: { kind: "guessed" } },
        ],
        savedModel: "auto",
        learning: { paused: true, pausedAt: at, extra: 1 },
        controls: true,
      }),
    ).toEqual({
      notes: [
        { id: "n1", text: "Told it.", createdAt: at, audience: "dm", source: { kind: "told", surface: "slack" }, editedAt: at },
        { id: "n2", text: "Unknown source.", createdAt: at, audience: "dm" },
      ],
      savedModel: "auto",
      learning: { paused: true, pausedAt: at },
      controls: true,
    });
    expect(memoryViewOf({ notes: [], savedModel: "auto", learning: { paused: "yes" }, controls: "yes" })).toEqual({ notes: [], savedModel: "auto" });
    expect(memoryNoteOf({ id: "n3", text: "Added.", createdAt: at, audience: "dm", source: { kind: "added" } })).toEqual({ id: "n3", text: "Added.", createdAt: at, audience: "dm", source: { kind: "added" } });
    expect(memoryNoteOf({ id: "n3" })).toBeUndefined();
    expect(learningOf({ paused: false })).toEqual({ paused: false });
    expect(learningOf(undefined)).toBeUndefined();
  });

  it("reads a note's kind and its errand strictly, and names the errand as its source", () => {
    const at = "2026-11-04T09:02:00.000Z";
    const errand = { id: "int_1", goal: "Northwind Labs SOW" };
    const note = memoryNoteOf({ id: "n1", text: "Priya N.", createdAt: at, audience: "dm", kind: "people", source: { kind: "told", surface: "slack", errand } });
    expect(note).toEqual({ id: "n1", text: "Priya N.", createdAt: at, audience: "dm", kind: "people", source: { kind: "told", surface: "slack", errand } });
    expect(noteSourceText(note!)).toBe("Errand: Northwind Labs SOW");
    // A kind it does not know and an errand without a goal are left out; the note is kept.
    expect(memoryNoteOf({ id: "n2", text: "x", createdAt: at, audience: "dm", kind: "pets", source: { kind: "told", errand: { id: "int_2" } } })).toEqual({
      id: "n2",
      text: "x",
      createdAt: at,
      audience: "dm",
      source: { kind: "told" },
    });
  });

  it("reads the approval settings strictly (A9·g, h): money as dollars, and a rule that is not one left out", () => {
    const at = "2026-11-03T09:00:00.000Z";
    const view = { tiers: "on", spendLimitUsd: "40.00", ownLine: true, defaultSpendLimitUsd: "50.00", maxSpendLimitUsd: "100.00" };
    expect(approvalsViewOf({ ...view, rules: [{ id: "allow_1", kind: "send", to: "team-1", since: at }, { id: "allow_2", kind: "rsvp", since: at, extra: 1 }, { kind: "send" }, null] })).toEqual({
      ...view,
      rules: [{ id: "allow_1", kind: "send", to: "team-1", since: at }, { id: "allow_2", kind: "rsvp", since: at }],
    });
    expect(approvalsViewOf({ ...view, rules: [], spendLimitUsd: 40 })).toBeUndefined();
    expect(approvalsViewOf({ ...view, rules: [], maxSpendLimitUsd: "100" })).toBeUndefined();
    expect(approvalsViewOf({ ...view, tiers: "maybe", rules: [] })).toBeUndefined();
    expect(approvalsViewOf({ ...view })).toBeUndefined();
  });

  it("reads a card's expiry in whole minutes, and leaves out one that is not (A9·h)", () => {
    const view = { tiers: "on", spendLimitUsd: "40.00", ownLine: true, defaultSpendLimitUsd: "50.00", maxSpendLimitUsd: "100.00", rules: [] };
    expect(approvalsViewOf({ ...view, expiresAfterMinutes: 30 })?.expiresAfterMinutes).toBe(30);
    for (const minutes of [0, -5, 2.5, "30", 24 * 60 + 1, null]) expect(approvalsViewOf({ ...view, expiresAfterMinutes: minutes })).not.toHaveProperty("expiresAfterMinutes");
    expect(approvalsViewOf(view)).toEqual(view);
  });

  it("reads the Sent using Nexus switch, and leaves out one that is not a boolean", () => {
    const view = { tiers: "on", spendLimitUsd: "40.00", ownLine: true, defaultSpendLimitUsd: "50.00", maxSpendLimitUsd: "100.00", rules: [] };
    expect(approvalsViewOf({ ...view, signMessages: false })?.signMessages).toBe(false);
    expect(approvalsViewOf({ ...view, signMessages: true })?.signMessages).toBe(true);
    for (const sign of ["false", 0, null]) expect(approvalsViewOf({ ...view, signMessages: sign })).not.toHaveProperty("signMessages");
  });
});

describe("an answer's Connect cards (A6·e)", () => {
  const linear = { connector: "linear", label: "Linear", url: "https://ui.test/app?next=%2Fsettings%2Fconnectors%3Fopen%3Dlinear#linear", summary: "One team's issues" };

  it("reads each connector to connect, as the API sends it, through the stream's done event", () => {
    const answer = { kind: "answer", text: "I can look once Linear is connected.", model: "m", costUsd: 0, connect: [linear] };
    const [event] = new AgentEventReader().push(`event: done\ndata: ${JSON.stringify({ status: 200, conversation: "c1", messages: [answer] })}\n\n`);
    const message = event?.type === "done" ? event.reply.messages[0] : undefined;
    expect(message && answerConnects(message)).toEqual([linear]);
  });

  it("leaves out an entry that is not one: no id, an id Gateway never names, a link that is not http(s), or a repeat", () => {
    const connect = [
      linear,
      { ...linear, summary: "again" },
      { label: "No id", url: linear.url },
      { connector: "../evil", label: "Evil", url: linear.url },
      { connector: "notion", label: "Notion", url: "javascript:alert(1)" },
      "linear",
      null,
    ];
    expect(answerConnects({ connect })).toEqual([linear]);
    expect(answerConnects({ connect: "linear" })).toEqual([]);
    expect(answerConnects({})).toEqual([]);
  });

  it("names a connector with no label by the kit's name, and keeps the summary to one plain line", () => {
    const [drive] = answerConnects({ connect: [{ connector: "google-drive", url: "https://ui.test/x", summary: "  Search\n files  " }] });
    expect(drive).toEqual({ connector: "google-drive", label: "Google Drive", url: "https://ui.test/x", summary: "Search files" });
  });

  it("says the agent's plain words, else the kit's line for the connector", () => {
    expect(connectLine(linear)).toBe("One team's issues.");
    expect(connectLine({ ...linear, summary: undefined })).toBe("It reads your issues and projects; each change asks first.");
    expect(connectLine({ connector: "resy", label: "Resy", url: "https://ui.test/x" }, "Ada")).toBe("Ada can read it, and each write asks you first.");
    expect(connectLine({ connector: "constructor", label: "Constructor", url: "https://ui.test/x" })).toBe("Nexus can read it, and each write asks you first.");
  });
});

describe("an answer's quick replies (A1·D)", () => {
  it("keeps up to three short one-line replies, each once", () => {
    const message = { kind: "answer", suggestions: ["Book it", " book it ", "Other times", 7, "a\nb", "x".repeat(41), "Not now", "Later"] };
    expect(answerSuggestions(message)).toEqual(["Book it", "Other times", "Not now"]);
  });

  it("reads none from an answer without them", () => {
    expect(answerSuggestions({ kind: "answer" })).toEqual([]);
    expect(answerSuggestions({ kind: "answer", suggestions: "Yes" })).toEqual([]);
  });
});

describe("an answer's text cards", () => {
  it("reads each card, with invite only when true", () => {
    expect(answerTexts({ texts: [{ to: " Ana  Lima ", text: "Running late\nSorry", invite: "yes" }, { to: "Bo", text: "Join me", invite: true }] })).toEqual([
      { to: "Ana Lima", text: "Running late\nSorry" },
      { to: "Bo", text: "Join me", invite: true },
    ]);
  });

  it("leaves out a card that is not one", () => {
    const bad = [null, "x", { to: "Ana" }, { to: "", text: "hi" }, { to: "Ana", text: "x".repeat(AGENT_TEXT_CHARS + 1) }, { to: "Ana", text: "bell\u0007" }];
    expect(answerTexts({ texts: bad })).toEqual([]);
    expect(answerTexts({ texts: "nope" })).toEqual([]);
    expect(answerTexts({})).toEqual([]);
  });

  it("shows each recipient and text once, at most three", () => {
    const texts = [{ to: "Ana", text: "a" }, { to: "ana", text: "a" }, { to: "Bo", text: "b" }, { to: "Cy", text: "c" }, { to: "Di", text: "d" }];
    expect(answerTexts({ texts }).map((card) => card.to)).toEqual(["Ana", "Bo", "Cy"]);
  });
});

describe("the errand an opened notice carries (A1)", () => {
  const errand = { id: "i4", goal: "Dinner Friday", status: "running", steps: 2, at: "2026-11-18T08:02:00.000Z" };
  const notice = (extra: Partial<Extract<AgentMessage, { kind: "notice" }>>): AgentMessage => ({ kind: "notice", topic: "errands", state: "opened", text: "I opened an errand for this.", ...extra });

  it("reads it strictly, only on an opened errands notice", () => {
    expect(noticeErrand(notice({ errand: errand as never }))).toEqual(errand);
    expect(noticeErrand(notice({ state: "list", errand: errand as never }))).toBeUndefined();
    expect(noticeErrand(notice({ errand: { ...errand, id: "x1" } as never }))).toBeUndefined();
    expect(noticeErrand(notice({}))).toBeUndefined();
  });
});
