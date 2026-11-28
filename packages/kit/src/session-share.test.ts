import { expect, it } from "vitest";
import { createArtifactsClient } from "./artifacts";
import { CUT_MARK, SESSION_SHARE_LIMITS, SessionTooLargeError, cutUtf8, fitSessionSnapshot, type SessionEvent, type SessionSnapshotInput } from "./session-share";

const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAoAAAAK";

const snapshot = (events: SessionEvent[]): SessionSnapshotInput => ({ source: "desktop", id: "thread-1", title: "Fix the login", events });

it("cuts text at a character boundary, never inside one", () => {
  expect(cutUtf8("abc", 10)).toBe("abc");
  expect(cutUtf8("aé", 2)).toBe("a");
  expect(cutUtf8("😀😀", 5)).toBe("😀");
  expect(new TextEncoder().encode(cutUtf8("ü".repeat(1000), 101)).length).toBeLessThanOrEqual(101);
});

it("leaves a small session as it is", () => {
  const session = snapshot([{ kind: "user", text: "hi", images: ["img_1"] }, { kind: "text", text: "hello" }]);
  const fitted = fitSessionSnapshot(session, [{ id: "img_1", data: PNG }, { id: "img_9", data: PNG }]);
  expect(fitted.session).toEqual(session);
  expect(fitted.images.map((image) => image.id)).toEqual(["img_1"]);
  expect(fitted.trimmed).toEqual({ outputs: 0, inputs: 0, images: 0, events: 0 });
});

it("cuts tool results step by step until the session fits under 4 MB", () => {
  const output = "x".repeat(SESSION_SHARE_LIMITS.toolOutputBytes);
  const events: SessionEvent[] = [{ kind: "user", text: "build it" }];
  for (let index = 0; index < 100; index += 1) events.push({ kind: "toolResult", id: `tu${index}`, output, isError: false, truncated: false });
  const fitted = fitSessionSnapshot(snapshot(events), []);
  expect(new TextEncoder().encode(JSON.stringify(fitted.session)).length).toBeLessThan(SESSION_SHARE_LIMITS.sessionBytes);
  const result = fitted.session.events[1];
  expect(result).toMatchObject({ kind: "toolResult", truncated: true });
  expect(result?.kind === "toolResult" && result.output.length).toBe(16 * 1024);
  expect(fitted.trimmed.outputs).toBe(100);
});

it("keeps the first 32 images the events name and drops the rest from their events", () => {
  const ids = Array.from({ length: 40 }, (_, index) => `img_${index}`);
  const fitted = fitSessionSnapshot(snapshot([{ kind: "user", text: "screens", images: ids }]), ids.map((id) => ({ id, data: PNG })));
  expect(fitted.images).toHaveLength(SESSION_SHARE_LIMITS.images);
  expect(fitted.session.events[0]).toMatchObject({ images: ids.slice(0, 32) });
  expect(fitted.trimmed.images).toBe(8);
});

it("keeps the newest 20,000 events, where the session ended", () => {
  const events: SessionEvent[] = Array.from({ length: SESSION_SHARE_LIMITS.events + 5 }, (_, index) => ({ kind: "notice", text: `n${index}` }));
  const fitted = fitSessionSnapshot(snapshot(events), []);
  expect(fitted.session.events).toHaveLength(SESSION_SHARE_LIMITS.events);
  expect(fitted.session.events[0]).toMatchObject({ text: "n5" });
  expect(fitted.session.events.at(-1)).toMatchObject({ text: `n${SESSION_SHARE_LIMITS.events + 4}` });
  expect(fitted.trimmed.events).toBe(5);
});

it("cuts strings in tool inputs once tool results alone do not fit, and counts them apart", () => {
  const big = "w".repeat(1024 * 1024);
  const events: SessionEvent[] = [{ kind: "user", text: "write them" }];
  for (let index = 0; index < 5; index += 1) events.push({ kind: "tool", id: `tu${index}`, name: "Write", input: { file_path: "a.ts", content: big } });
  const fitted = fitSessionSnapshot(snapshot(events), []);
  const tool = fitted.session.events[1];
  expect(tool?.kind === "tool" && (tool.input as { content: string; file_path: string })).toMatchObject({ file_path: "a.ts", content: "w".repeat(16 * 1024) + CUT_MARK });
  expect(fitted.trimmed).toMatchObject({ outputs: 0, inputs: 5 });
});

it("cuts tool results alone while that is enough, and leaves tool inputs whole", () => {
  const output = "x".repeat(SESSION_SHARE_LIMITS.toolOutputBytes);
  const content = "w".repeat(32 * 1024);
  const events: SessionEvent[] = [{ kind: "user", text: "build it" }, { kind: "tool", id: "tw", name: "Write", input: { content } }];
  for (let index = 0; index < 100; index += 1) events.push({ kind: "toolResult", id: `tu${index}`, output, isError: false, truncated: false });
  const fitted = fitSessionSnapshot(snapshot(events), []);
  expect(fitted.session.events[1]).toMatchObject({ input: { content } });
  expect(fitted.trimmed).toMatchObject({ outputs: 100, inputs: 0 });
});

it("drops an image over 5 MB and says so, instead of failing the share", () => {
  const huge = "A".repeat(Math.ceil((SESSION_SHARE_LIMITS.oneImageBytes + 3) / 3) * 4);
  const fitted = fitSessionSnapshot(snapshot([{ kind: "user", text: "two", images: ["big", "small"] }]), [
    { id: "big", data: huge },
    { id: "small", data: PNG },
  ]);
  expect(fitted.images.map((image) => image.id)).toEqual(["small"]);
  expect(fitted.session.events[0]).toMatchObject({ images: ["small"] });
  expect(fitted.trimmed.images).toBe(1);
});

it("refuses a session that does not fit even with tool results at their shortest", () => {
  const events: SessionEvent[] = [{ kind: "user", text: "y".repeat(SESSION_SHARE_LIMITS.sessionBytes) }];
  expect(() => fitSessionSnapshot(snapshot(events), [])).toThrow(SessionTooLargeError);
});

it("posts the snapshot to the sessions route with an idempotency key", async () => {
  const calls: { url: string; method?: string; key: string | null; body: unknown }[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      calls.push({ url, method: init?.method, key: new Headers(init?.headers).get("idempotency-key"), body: JSON.parse(String(init?.body)) });
      return new Response(JSON.stringify({ id: "art_1", kind: "session" }), { status: 201 });
    },
  });
  const shared = await client.shareAgentSession({ audience: "org", session: snapshot([{ kind: "user", text: "hi" }]) });
  expect(shared.kind).toBe("session");
  expect(calls[0]).toMatchObject({ url: "http://gateway.invalid/v1/artifacts/sessions", method: "POST", body: { audience: "org", session: { source: "desktop" } } });
  expect(calls[0]?.key).toMatch(/^ui_/);
});

it("stops sharing a session by its key", async () => {
  const calls: { url: string; method?: string }[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      calls.push({ url, method: init?.method });
      return new Response(JSON.stringify({ sessionId: "cds_a", visibility: "private", revoked: [{ id: "art_1", version: 1 }] }), { status: 200 });
    },
  });
  const stopped = await client.unshareAgentSession("cds_a");
  expect(stopped.revoked).toHaveLength(1);
  expect(calls).toEqual([{ url: "http://gateway.invalid/v1/artifacts/sessions/cds_a", method: "DELETE" }]);
});
