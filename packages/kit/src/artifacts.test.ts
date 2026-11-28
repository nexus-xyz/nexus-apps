import { afterEach, expect, it, vi } from "vitest";
import { createArtifactsClient } from "./artifacts";

afterEach(() => vi.unstubAllGlobals());

// React Native (the phone app) has no global `crypto`: a write must still carry a key, not throw before it is sent.
it("sends an idempotency key on a write where there is no crypto.randomUUID", async () => {
  vi.stubGlobal("crypto", undefined);
  const keys: (string | null)[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (_url, init) => {
      keys.push(new Headers(init?.headers).get("idempotency-key"));
      return new Response(JSON.stringify({ revoked: true, id: "art_1", version: 1 }), { status: 200 });
    },
  });
  await client.revokeArtifact("art_1");
  await client.revokeArtifact("art_1");
  expect(keys).toHaveLength(2);
  for (const key of keys) expect(key).toMatch(/^ui_\S+$/);
  expect(keys[0]).not.toBe(keys[1]);
});

// A page's storage: the key is one path segment, encoded; the value goes as `{ value }`.
it("calls a page's storage with the key encoded and the value in the body", async () => {
  const calls: { url: string; method?: string; body?: string }[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      calls.push({ url: String(url), method: init?.method, body: init?.body as string | undefined });
      return new Response("{}", { status: 200 });
    },
  });
  await client.setPageStorageValue("pag_1", "shared", "notes:été", "hi");
  await client.pageStorageKeys("pag_1", "personal", { prefix: "notes:" });
  expect(calls).toEqual([
    { url: "http://gateway.invalid/v1/pages/pag_1/storage/shared/notes%3A%C3%A9t%C3%A9", method: "PUT", body: '{"value":"hi"}' },
    { url: "http://gateway.invalid/v1/pages/pag_1/storage/personal?prefix=notes%3A", method: "GET", body: undefined },
  ]);
});

// The draft's text goes as the page's live preview, a PUT without an idempotency key; the owner frames `live`.
it("puts a page's live preview and frames its live state", async () => {
  const calls: { url: string; method?: string; body?: string; key: string | null }[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      calls.push({ url: String(url), method: init?.method, body: init?.body as string | undefined, key: new Headers(init?.headers).get("idempotency-key") });
      return new Response('{"pageId":"pag_1","version":2,"contentSha256":"ab"}', { status: 200 });
    },
  });
  expect(await client.putPagePreview("pag_1", { baseVersion: 2, content: "<h1>Hi</h1>" })).toEqual({ pageId: "pag_1", version: 2, contentSha256: "ab" });
  await client.framePageVersion("pag_1", "live");
  expect(calls).toEqual([
    { url: "http://gateway.invalid/v1/pages/pag_1/preview", method: "PUT", body: '{"baseVersion":2,"content":"<h1>Hi</h1>"}', key: null },
    { url: "http://gateway.invalid/v1/pages/pag_1/versions/live/frame", method: "GET", body: undefined, key: null },
  ]);
});

// A page's live events, split anywhere across chunks, keep-alives skipped; a refusal rejects.
it("reads a page's events from its stream, and rejects a refusal", async () => {
  const chunks = [
    'event: version\ndata: {"pageId":"pag_1","version":2,"title":"Q3","format":"html"}\n\n: keep-',
    'alive\n\nevent: version\ndata: {"pageId":"pag_1",',
    '"version":3,"title":"Q3","format":"html"}\n\n',
  ];
  const paths: string[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url) => {
      paths.push(url);
      if (url.includes("pag_gone")) return new Response(JSON.stringify({ error: { message: "no page", code: "page_not_found" } }), { status: 404 });
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(new TextEncoder().encode(chunk));
          controller.close();
        },
      });
      return new Response(body, { status: 200, headers: { "content-type": "text/event-stream; charset=utf-8" } });
    },
  });
  const seen: unknown[] = [];
  await client.pageEvents("pag_1", (event) => seen.push(event));
  expect(paths).toEqual(["http://gateway.invalid/v1/pages/pag_1/events"]);
  expect(seen).toEqual([
    { type: "version", data: { pageId: "pag_1", version: 2, title: "Q3", format: "html" } },
    { type: "version", data: { pageId: "pag_1", version: 3, title: "Q3", format: "html" } },
  ]);
  await expect(client.pageEvents("pag_gone", () => undefined)).rejects.toMatchObject({ status: 404, code: "page_not_found" });
});

// An export is a file with its name from content-disposition, and the boards that did not render.
it("exports a page as a file, named by Gateway, with the boards that did not render", async () => {
  const urls: string[] = [];
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url) => {
      urls.push(String(url));
      return new Response("PNG", {
        status: 200,
        headers: { "content-type": "image/png", "content-disposition": 'attachment; filename="q3-m1.png"', "x-nexus-export-failed": encodeURIComponent(JSON.stringify([{ board: "Phone", error: "boom" }, { nope: 1 }])) },
      });
    },
  });
  const file = await client.exportPage("pag_1", "png", { board: "M1 Phone", scale: 2 });
  expect(urls).toEqual(["http://gateway.invalid/v1/pages/pag_1/export?format=png&board=M1+Phone&scale=2"]);
  expect([file.filename, await file.blob.text(), file.failed]).toEqual(["q3-m1.png", "PNG", [{ board: "Phone", error: "boom" }]]);
});

it("requests the selected Site version as ZIP and lets the caller cancel", async () => {
  const controller = new AbortController();
  const started = vi.fn();
  const client = createArtifactsClient({
    baseUrl: "http://gateway.invalid",
    fetch: (url, init) => {
      started(String(url));
      return new Promise<Response>((_, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new Error("Canceled")), { once: true });
      });
    },
  });
  const exportFile = client.exportPage("pag_1", "zip", { version: 7, signal: controller.signal });
  const canceled = expect(exportFile).rejects.toThrow("Canceled");
  await vi.waitFor(() => expect(started).toHaveBeenCalledWith("http://gateway.invalid/v1/pages/pag_1/export?format=zip&version=7"));
  controller.abort();
  await canceled;
});
