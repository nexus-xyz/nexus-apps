import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

it("carries caller-owned keys across explicit chat startup retries", async () => {
  const seen: { path: string; key: string | null }[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (url, init) => {
    seen.push({ path: String(url), key: new Headers(init?.headers).get("idempotency-key") });
    return new Response(JSON.stringify({ error: { message: "offline", code: "unavailable" } }), { status: 503 });
  } });
  for (let attempt = 0; attempt < 2; attempt++) {
    await expect(core.createChatSession({ mode: "chat" }, "first:create")).rejects.toThrow("offline");
    await expect(core.streamChatMessage("session", { content: "Hello" }, () => {}, undefined, "first:message")).rejects.toThrow("offline");
  }
  expect(seen.map(({ key }) => key)).toEqual(["first:create", "first:message", "first:create", "first:message"]);
});

it("retries a model call with the caller's key, never a fresh one", async () => {
  const keys: (string | null)[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", idempotency: true, fetch: async (_url, init) => {
    keys.push(new Headers(init?.headers).get("idempotency-key"));
    return keys.length < 3
      ? new Response(JSON.stringify({ error: { message: "busy", code: "idempotency_in_progress" } }), { status: 409 })
      : new Response(JSON.stringify({ id: "c1", model: "m" }), { status: 200 });
  } });
  await expect(core.chatCompletion({ model: "nexus/auto", messages: [{ role: "user", content: "Hi" }] }, undefined, "event:1")).resolves.toMatchObject({ id: "c1" });
  expect(keys).toEqual(["event:1", "event:1", "event:1"]);
});

it("retries a write whose answer was lost with the same key, and succeeds", async () => {
  const keys: (string | null)[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", idempotency: true, fetch: async (_url, init) => {
    keys.push(new Headers(init?.headers).get("idempotency-key"));
    // The first answer is lost after Gateway applied the write: the connection is reset under it.
    if (keys.length === 1) throw new TypeError("fetch failed", { cause: new Error("read ECONNRESET") });
    return new Response(JSON.stringify({ slug: "web", name: "web" }), { status: 201 });
  } });
  await expect(core.createTeam({ slug: "web" })).resolves.toMatchObject({ slug: "web" });
  expect(keys).toHaveLength(2);
  expect(keys[0]).toMatch(/\S/);
  expect(keys[1]).toBe(keys[0]);
});

it("sends a fresh key, never an empty one, when an idempotent client gets a blank request ID", async () => {
  const keys: (string | null)[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", idempotency: true, fetch: async (_url, init) => {
    keys.push(new Headers(init?.headers).get("idempotency-key"));
    return new Response(JSON.stringify({ error: { message: "bad", code: "invalid" } }), { status: 400 });
  } });
  for (const blank of ["", " ", undefined]) {
    await expect(core.createChatSession({ mode: "chat" }, blank)).rejects.toThrow("bad");
    await expect(core.streamChatMessage("session", { content: "Hello" }, () => {}, undefined, blank)).rejects.toThrow("bad");
  }
  expect(keys).toHaveLength(6);
  for (const key of keys) expect(key).toMatch(/\S/);
});

it("sends tools and tool turns as they are, and reads the tool calls back", async () => {
  const bodies: unknown[] = [];
  const call = { id: "call_1", type: "function" as const, function: { name: "linear__list_issues", arguments: "{}" } };
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (_url, init) => {
    bodies.push(JSON.parse(String(init?.body)));
    return new Response(JSON.stringify({ id: "c1", model: "m", choices: [{ index: 0, message: { role: "assistant", content: "", tool_calls: [call] }, finish_reason: "tool_calls" }] }), { status: 200 });
  } });
  const tools = [{ type: "function" as const, function: { name: "linear__list_issues", parameters: { type: "object" } } }];
  const messages = [{ role: "user" as const, content: "Hi" }, { role: "assistant" as const, content: "", tool_calls: [call] }, { role: "tool" as const, tool_call_id: "call_1", content: "[]" }];
  const response = await core.chatCompletionWithTools({ model: "nexus/auto", messages, tools });
  expect(bodies).toEqual([{ model: "nexus/auto", messages, tools }]);
  expect(response.choices[0]?.message.tool_calls).toEqual([call]);
});

it("asks the code with stream off and the caller's key, and throws Gateway's refusal", async () => {
  const seen: { method?: string; path: string; key: string | null; body: unknown }[] = [];
  let status = 200;
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (url, init) => {
    seen.push({ method: init?.method, path: new URL(String(url)).pathname, key: new Headers(init?.headers).get("idempotency-key"), body: JSON.parse(String(init?.body)) });
    return status === 200
      ? new Response(JSON.stringify({ session: { id: "chs_1" }, messages: [] }), { status })
      : new Response(JSON.stringify({ error: { message: "not granted", code: "role_required" } }), { status });
  } });
  await expect(core.askCode({ content: "How does billing work?", session: "chs_1" }, "event:1")).resolves.toMatchObject({ session: { id: "chs_1" } });
  expect(seen).toEqual([{ method: "POST", path: "/v1/chat/ask", key: "event:1", body: { content: "How does billing work?", session: "chs_1", stream: false } }]);
  status = 403;
  await expect(core.askCode({ content: "q" }, "event:2")).rejects.toMatchObject({ status: 403, code: "role_required" });
});
