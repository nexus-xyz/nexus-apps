/** `streamChatCompletion`, with and without tools, against Gateway's `chat.completion.chunk` frames (`nexus-routing/.../chunks.rs`). */
import { expect, it } from "vitest";
import { CoreError, createCoreClient } from "./core-client";
import type { ChatCompletionChunk } from "./api";

const REQUEST = { model: "nexus/auto", messages: [{ role: "user" as const, content: "Hi" }] };
const NEXUS = { cost_usd: 0.0123, product: "chat", provider: "anthropic", requested: "nexus/auto", resolved_from: "nexus/auto" as const };

const chunk = (delta: Record<string, unknown>, finish: string | null = null) =>
  `data: ${JSON.stringify({ id: "chatcmpl_1", object: "chat.completion.chunk", created: 1, model: "anthropic/claude-sonnet-4-5", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
const last = `data: ${JSON.stringify({ id: "chatcmpl_1", object: "chat.completion.chunk", created: 1, model: "anthropic/claude-sonnet-4-5", choices: [], usage: { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 }, nexus: NEXUS })}\n\n`;

/** A stream whose frames arrive in pieces cut anywhere, as a network delivers them. */
function sse(frames: string[], cut = 7): Response {
  const bytes = new TextEncoder().encode(frames.join(""));
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (let at = 0; at < bytes.length; at += cut) controller.enqueue(bytes.slice(at, at + cut));
      controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream; charset=utf-8" } });
}

function client(answer: () => Response) {
  const sent: { body: Record<string, unknown>; key: string | null }[] = [];
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    idempotency: true,
    fetch: async (_url, init) => {
      sent.push({ body: JSON.parse(String(init?.body)), key: new Headers(init?.headers).get("idempotency-key") });
      return answer();
    },
  });
  return { core, sent };
}

it("streams the answer's pieces and resolves with the whole answer, as chatCompletion gives it", async () => {
  const { core, sent } = client(() => sse([chunk({ role: "assistant", content: "" }), chunk({ content: "Hel" }), chunk({ content: "lo" }), chunk({}, "stop"), last, "data: [DONE]\n\n"]));
  const pieces: string[] = [];
  const answer = await core.streamChatCompletion(REQUEST, (part: ChatCompletionChunk) => pieces.push(part.choices[0]?.delta.content ?? "·"), undefined, "event:1");
  expect(sent).toEqual([{ body: { ...REQUEST, stream: true }, key: "event:1" }]);
  expect(pieces).toEqual(["", "Hel", "lo", "·", "·"]);
  expect(answer).toMatchObject({
    model: "anthropic/claude-sonnet-4-5",
    choices: [{ message: { role: "assistant", content: "Hello" }, finish_reason: "stop" }],
    usage: { total_tokens: 5 },
    nexus: NEXUS,
  });
});

it("rejects on an error event mid-stream, with Gateway's code", async () => {
  const { core } = client(() => sse([chunk({ content: "Half an ans" }), `data: ${JSON.stringify({ error: { message: "the provider broke off", type: "api_error", code: "upstream_error" } })}\n\n`, "data: [DONE]\n\n"]));
  const failure = await core.streamChatCompletion(REQUEST, () => {}).catch((error: unknown) => error);
  expect(failure).toBeInstanceOf(CoreError);
  expect(failure).toMatchObject({ status: 502, code: "upstream_error", message: "the provider broke off" });
});

it("rejects a stream that ends before its last chunk", async () => {
  const { core } = client(() => sse([chunk({ content: "Half" })]));
  await expect(core.streamChatCompletion(REQUEST, () => {})).rejects.toMatchObject({ status: 502 });
});

it("rejects a refusal before the stream starts as chatCompletion does, and is not retried", async () => {
  const { core, sent } = client(() => Response.json({ error: { message: "model not allowed", type: "invalid_request_error", code: "model_not_allowed" } }, { status: 403 }));
  await expect(core.streamChatCompletion(REQUEST, () => {}, undefined, "event:2")).rejects.toMatchObject({ status: 403, code: "model_not_allowed" });
  expect(sent).toHaveLength(1);
});

it("retries with the same key while the first try still runs, before any chunk", async () => {
  let tries = 0;
  const { core, sent } = client(() =>
    ++tries < 2 ? Response.json({ error: { message: "busy", code: "idempotency_in_progress" } }, { status: 409 }) : sse([chunk({ content: "Hi" }), last]),
  );
  await expect(core.streamChatCompletion(REQUEST, () => {}, undefined, "event:3")).resolves.toMatchObject({ choices: [{ message: { content: "Hi" } }] });
  expect(sent.map(({ key }) => key)).toEqual(["event:3", "event:3"]);
});

it("reads a whole JSON answer, such as a replay, with no chunks", async () => {
  const whole = { id: "c1", object: "chat.completion", created: 1, model: "m", choices: [{ index: 0, message: { role: "assistant", content: "Kept" }, finish_reason: "stop" }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }, nexus: NEXUS };
  const { core } = client(() => Response.json(whole, { headers: { "idempotent-replayed": "true" } }));
  const chunks: unknown[] = [];
  await expect(core.streamChatCompletion(REQUEST, (part) => chunks.push(part))).resolves.toEqual(whole);
  expect(chunks).toEqual([]);
});

it("joins a streamed answer's tool calls by index, as chatCompletionWithTools gives them", async () => {
  const tools = [{ type: "function" as const, function: { name: "web_search", parameters: { type: "object" } } }];
  const { core, sent } = client(() =>
    sse([
      chunk({ role: "assistant", content: "" }),
      chunk({ content: "Looking." }),
      chunk({ tool_calls: [{ index: 0, id: "call_a", type: "function", function: { name: "web_search", arguments: "" } }] }),
      chunk({ tool_calls: [{ index: 0, function: { arguments: '{"query":' } }] }),
      chunk({ tool_calls: [{ index: 1, id: "call_b", type: "function", function: { name: "web_search", arguments: '{"query":"b"}' } }] }),
      chunk({ tool_calls: [{ index: 0, function: { arguments: '"a"}' } }] }),
      chunk({}, "tool_calls"),
      last,
      "data: [DONE]\n\n",
    ]),
  );
  const answer = await core.streamChatCompletion({ ...REQUEST, tools }, () => {}, undefined, "event:t");
  expect(sent[0]?.body).toEqual({ ...REQUEST, tools, stream: true });
  expect(answer.choices[0]).toEqual({
    index: 0,
    message: {
      role: "assistant",
      content: "Looking.",
      tool_calls: [
        { id: "call_a", type: "function", function: { name: "web_search", arguments: '{"query":"a"}' } },
        { id: "call_b", type: "function", function: { name: "web_search", arguments: '{"query":"b"}' } },
      ],
    },
    finish_reason: "tool_calls",
  });
});

/**
 * `streamChatMessage` and the `nexus.skill` event: the kit passes a loaded skill's scope and team through to the
 * callback and to the stored answer as Gateway sent them, and adds none to a block that had none. The kit reads no field of a
 * block, so this pins the pass-through. The cards that read `scope` are tested in `chat-skills.test.ts` and `skill-render.test.ts`.
 */
const event = (nexus: Record<string, unknown>) =>
  `data: ${JSON.stringify({ id: "chatcmpl_1", object: "chat.completion.chunk", created: 1, model: "anthropic/claude-sonnet-4-5", choices: [], nexus })}\n\n`;

it("passes a loaded skill's scope and team through untouched, and adds none to a block that had none", async () => {
  const session = { id: "chs_1", title: "New session", mode: "chat", model: "nexus/auto" };
  const question = { id: "chm_1", role: "user", content: "Load triage", status: "complete" };
  const team = { skillId: "skl_lib", name: "triage", version: 3, scope: "team", teamId: "team_web" };
  const old = { skillId: "skl_old", name: "notes", version: 1 };
  const answer = { id: "chm_2", role: "assistant", content: "Done", status: "complete", skills: [team, old] };
  const frames = [event({ session, question }), event({ skill: team }), event({ skill: old }), event({ message: answer, session }), "data: [DONE]\n\n"];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async () => sse(frames, 11) });
  const seen: unknown[] = [];
  const done = await core.streamChatMessage("chs_1", { content: "Load triage" }, (part) => seen.push(part.nexus?.skill));
  expect(seen.filter(Boolean)).toEqual([team, old]);
  expect(done.message.skills).toEqual([team, old]);
  expect(done.message.skills?.[1]).not.toHaveProperty("scope");
});

/**
 * `streamGuestChatMessage`: a guest round reads the chat sessions' frames through the same parser, with the
 * guest id in its header and the rounds from the answer's headers. A refusal before the stream keeps its code, and a stream
 * cut before its last chunk rejects.
 */
it("streams a guest round with the guest id, and reads the rounds from the headers", async () => {
  const usage = `data: ${JSON.stringify({ id: "chatcmpl_1", object: "chat.completion.chunk", created: 1, model: "anthropic/claude-sonnet-4-5", choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 } })}\n\n`;
  const first = chunk({ role: "assistant" });
  const frames = [first, chunk({ content: "Hel" }), chunk({ content: "lo" }), usage, "data: [DONE]\n\n"];
  const sent: { url: string; guest: string | null; body: unknown }[] = [];
  const guestId = "6f1c2a52-3a8e-4bde-9a51-0d3c1e9f7b10";
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      sent.push({ url: String(url), guest: new Headers(init?.headers).get("x-nexus-guest"), body: JSON.parse(String(init?.body)) });
      const headers = { "content-type": "text/event-stream; charset=utf-8", "x-nexus-guest-rounds-used": "3", "x-nexus-guest-rounds-limit": "10" };
      return new Response(sse(frames, 5).body, { status: 200, headers });
    },
  });
  const body = { messages: [{ role: "user" as const, content: "Hi" }], turnstile: "tok" };
  const pieces: string[] = [];
  const done = await core.streamGuestChatMessage(guestId, body, (part) => pieces.push(part.choices[0]?.delta.content ?? "·"));
  expect(sent).toEqual([{ url: "http://gateway.invalid/v1/guest/chat/messages", guest: guestId, body }]);
  expect(pieces.join("")).toBe("·Hello·");
  expect(done).toEqual({ content: "Hello", model: "anthropic/claude-sonnet-4-5", roundsUsed: 3, roundsLimit: 10 });

  const refused = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async () => new Response(JSON.stringify({ error: { message: "used", type: "rate_limit_error", code: "guest_rounds_used" } }), { status: 429 }),
  });
  await expect(refused.streamGuestChatMessage(guestId, body, () => {})).rejects.toMatchObject({ status: 429, code: "guest_rounds_used" });
  const cut = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async () => sse([first, chunk({ content: "Hel" })]) });
  await expect(cut.streamGuestChatMessage(guestId, body, () => {})).rejects.toBeInstanceOf(CoreError);
});

/** `guestChatStatus`: the status is read under the guest id, and it names the model that answers. */
it("reads a guest's status with the guest id, and the model's display name", async () => {
  const guestId = "6f1c2a52-3a8e-4bde-9a51-0d3c1e9f7b10";
  const status = { enabled: true, rounds_used: 1, rounds_limit: 5, model: "DeepSeek V4 Flash 0731" };
  const asked: { url: string; guest: string | null }[] = [];
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      asked.push({ url: String(url), guest: new Headers(init?.headers).get("x-nexus-guest") });
      return new Response(JSON.stringify(status), { status: 200, headers: { "content-type": "application/json" } });
    },
  });
  expect(await core.guestChatStatus(guestId)).toEqual(status);
  expect(asked).toEqual([{ url: "http://gateway.invalid/v1/guest/chat", guest: guestId }]);
});
