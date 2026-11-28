import { describe, expect, it } from "vitest";
import type { ChatCompletionRequest, ChatCompletionResponse } from "./api";
import { CoreError } from "./core-client";
import { createBridge, MAX_INPUT_CHARS, MAX_IN_FLIGHT, MAX_KEY_CHARS, MAX_OUTPUT_TOKENS, PAGE_MODEL, parseComplete, parseQuery, parseReadFile, parseStorage, type BridgeDeps, type BridgeReply, type StorageClient } from "./page-bridge";
import type { PageEvent } from "./pages";

/** A frame's window that records the replies it gets. */
function frameWindow() {
  const replies: BridgeReply[] = [];
  return {
    replies,
    postMessage: (reply: BridgeReply, target: string) => {
      expect(target).toBe("*");
      replies.push(reply);
    },
  };
}

function answerWith(content: string): ChatCompletionResponse {
  return { choices: [{ index: 0, message: { role: "assistant", content }, finish_reason: "stop" }] } as ChatCompletionResponse;
}

/** A page's files: `region,sales\nnorth,12\n` with a byte-order mark, and a spreadsheet's first bytes. */
const FILES: Record<string, { mediaType: "text/csv" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"; bytes: number; data: string }> = {
  "sales.csv": { mediaType: "text/csv", bytes: 25, data: btoa("\xef\xbb\xbfregion,sales\nnorth,12\n") },
  "book.xlsx": { mediaType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", bytes: 6, data: btoa("PK\x03\x04\xff\x00") },
};

/** A storage client over a map, recording each call as `[method, pageId, scope, key or prefix]`. */
function fakeStorage(fail?: CoreError) {
  const values = new Map<string, string>();
  const calls: string[][] = [];
  const at = (pageId: string, scope: string, key: string) => `${pageId}/${scope}/${key}`;
  const client: StorageClient = {
    pageStorageValue: async (pageId, scope, key) => {
      calls.push(["get", pageId, scope, key]);
      if (fail) throw fail;
      const value = values.get(at(pageId, scope, key));
      if (value === undefined) throw new CoreError(404, "no personal value k on this page", "page_storage_key_not_found");
      return { key, scope, value, updatedAt: "2026-10-27T00:00:00Z" };
    },
    setPageStorageValue: async (pageId, scope, key, value) => {
      calls.push(["set", pageId, scope, key]);
      if (fail) throw fail;
      values.set(at(pageId, scope, key), value);
      return { key, scope, bytes: key.length + value.length, updatedAt: "2026-10-27T00:00:00Z" };
    },
    deletePageStorageValue: async (pageId, scope, key) => {
      calls.push(["delete", pageId, scope, key]);
      if (fail) throw fail;
      return { key, scope, deleted: values.delete(at(pageId, scope, key)) };
    },
    pageStorageKeys: async (pageId, scope, query = {}) => {
      calls.push(["list", pageId, scope, query.prefix ?? ""]);
      if (fail) throw fail;
      const keys = [...values.keys()].filter((path) => path.startsWith(`${pageId}/${scope}/${query.prefix ?? ""}`)).map((path) => path.split("/")[2]!);
      return { scope, prefix: query.prefix ?? "", keys };
    },
    pageFile: async (pageId, name, version) => {
      calls.push(["file", pageId, name, String(version)]);
      if (fail) throw fail;
      const file = FILES[name];
      if (!file) throw new CoreError(404, "this page has no file by that name", "page_file_not_found");
      return { name, version: 1, createdAt: "2026-10-27T00:00:00Z", ...file };
    },
  };
  return { client, calls };
}

function setup(overrides: Partial<BridgeDeps> & { complete?: (request: ChatCompletionRequest) => Promise<ChatCompletionResponse> } = {}) {
  const frame = frameWindow();
  const calls: ChatCompletionRequest[] = [];
  const storage = fakeStorage();
  let asked = 0;
  let confirmed = 0;
  const complete = overrides.complete ?? (async () => answerWith("Hi there"));
  const bridge = createBridge({
    frame: () => frame,
    core: () => ({
      chatCompletion: (request: ChatCompletionRequest) => {
        calls.push(request);
        return complete(request);
      },
    }),
    consent: async () => {
      asked++;
      return true;
    },
    pageId: "pag_shown",
    storage: () => storage.client,
    confirmShared: async () => {
      confirmed++;
      return true;
    },
    ...overrides,
  });
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  return { frame, calls, storage, bridge, settle, asked: () => asked, confirmed: () => confirmed };
}

describe("the page bridge", () => {
  it("answers its own frame's call with the model's text, on nexus/auto with capped output", async () => {
    const { frame, calls, bridge, settle } = setup();
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "complete", params: "Hello" } });
    await settle();
    expect(calls).toEqual([{ model: PAGE_MODEL, messages: [{ role: "user", content: "Hello" }], max_tokens: 1024 }]);
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, result: "Hi there" }]);
  });

  it("ignores a message from any other window, or one that is not a call, and calls nothing", async () => {
    const { frame, calls, bridge, settle, asked } = setup();
    const other = frameWindow();
    bridge.onMessage({ source: other, data: { nexus: 1, id: 1, method: "complete", params: "Hello" } });
    bridge.onMessage({ source: null, data: { nexus: 1, id: 1, method: "complete", params: "Hello" } });
    bridge.onMessage({ source: frame, data: "complete" });
    bridge.onMessage({ source: frame, data: { nexus: 2, id: 1, method: "complete", params: "Hello" } });
    bridge.onMessage({ source: frame, data: { nexus: 1, id: { x: 1 }, method: "complete", params: "Hello" } });
    await settle();
    expect(other.replies).toEqual([]);
    expect(frame.replies).toEqual([]);
    expect(calls).toEqual([]);
    expect(asked()).toBe(0);
  });

  it("rejects a bad shape or an unknown method before asking the reader", async () => {
    const { frame, calls, bridge, settle, asked } = setup();
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "complete", params: { prompt: "Hello" } } });
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 2, method: "storage.clear", params: {} } });
    await settle();
    expect(frame.replies.map((reply) => "error" in reply && reply.error.code)).toEqual(["bad_request", "unknown_method"]);
    expect(calls).toEqual([]);
    expect(asked()).toBe(0);
  });

  it("checks every field and caps the input and the output", () => {
    const user = (content: string) => ({ role: "user", content });
    expect(parseComplete({ messages: [user("Hi")], maxTokens: 200 })).toEqual({ messages: [user("Hi")], maxTokens: 200 });
    for (const bad of [
      "",
      42,
      null,
      [user("Hi")],
      { messages: [] },
      { messages: [user("Hi")], model: "gpt-5" },
      { messages: [{ role: "tool", content: "Hi" }] },
      { messages: [{ role: "user", content: "Hi", name: "x" }] },
      { messages: [{ role: "user", content: ["Hi"] }] },
      { messages: [user("Hi")], maxTokens: MAX_OUTPUT_TOKENS + 1 },
      { messages: [user("Hi")], maxTokens: 1.5 },
      "x".repeat(MAX_INPUT_CHARS + 1),
    ]) {
      expect(parseComplete(bad), JSON.stringify(bad).slice(0, 80)).toBeUndefined();
    }
  });

  it("answers signed_out with no call and no prompt when the reader is signed out", async () => {
    const { frame, calls, bridge, settle, asked } = setup({ core: () => null });
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "complete", params: "Hello" } });
    await settle();
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, error: { code: "signed_out", message: expect.stringContaining("Sign in") } }]);
    expect(calls).toEqual([]);
    expect(asked()).toBe(0);
  });

  it("answers denied with no call when the reader says no", async () => {
    const { frame, calls, bridge, settle } = setup({ consent: async () => false });
    bridge.onMessage({ source: frame, data: { nexus: 1, id: "a", method: "complete", params: "Hello" } });
    await settle();
    expect(frame.replies).toEqual([{ nexus: 1, id: "a", error: { code: "denied", message: expect.any(String) } }]);
    expect(calls).toEqual([]);
  });

  it("allows a few calls at once per frame, and says busy to the rest", async () => {
    const waiting: (() => void)[] = [];
    const { frame, calls, bridge, settle } = setup({ complete: () => new Promise((resolve) => waiting.push(() => resolve(answerWith("ok")))) });
    for (let id = 1; id <= MAX_IN_FLIGHT + 1; id++) bridge.onMessage({ source: frame, data: { nexus: 1, id, method: "complete", params: "Hello" } });
    await settle();
    expect(calls).toHaveLength(MAX_IN_FLIGHT);
    expect(frame.replies).toEqual([{ nexus: 1, id: MAX_IN_FLIGHT + 1, error: { code: "busy", message: expect.any(String) } }]);
    waiting.forEach((resolve) => resolve());
    await settle();
    expect(frame.replies).toHaveLength(MAX_IN_FLIGHT + 1);
  });

  it("passes on a code and a fixed message for Gateway's failures, never Gateway's own text", async () => {
    for (const [status, code] of [
      [403, "refused"],
      [402, "limit"],
      [429, "limit"],
      [401, "signed_out"],
      [500, "failed"],
    ] as const) {
      const { frame, bridge, settle } = setup({ complete: () => Promise.reject(new CoreError(status, "secret budget of org_42")) });
      bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "complete", params: "Hello" } });
      await settle();
      expect(frame.replies).toEqual([{ nexus: 1, id: 1, error: { code, message: expect.any(String) } }]);
      expect(JSON.stringify(frame.replies)).not.toContain("secret");
    }
  });
});

describe("window.nexus.storage on the page bridge", () => {
  const call = (id: number, method: string, params: unknown) => ({ nexus: 1, id, method, params });

  it("sets, gets, lists and deletes the reader's personal data, on the page the frame shows, with no AI consent", async () => {
    const { frame, storage, bridge, settle, asked, confirmed } = setup();
    bridge.onMessage({ source: frame, data: call(1, "storage.set", { key: "notes", value: "hi" }) });
    await settle();
    bridge.onMessage({ source: frame, data: call(2, "storage.get", { key: "notes" }) });
    bridge.onMessage({ source: frame, data: call(3, "storage.list", { prefix: "no" }) });
    bridge.onMessage({ source: frame, data: call(4, "storage.get", { key: "missing", options: { shared: false } }) });
    await settle();
    bridge.onMessage({ source: frame, data: call(5, "storage.delete", { key: "notes" }) });
    await settle();
    expect(frame.replies).toEqual([
      { nexus: 1, id: 1, result: { key: "notes", value: "hi", shared: false } },
      { nexus: 1, id: 2, result: { key: "notes", value: "hi", shared: false } },
      { nexus: 1, id: 3, result: { keys: ["notes"], prefix: "no", shared: false } },
      { nexus: 1, id: 4, result: null },
      { nexus: 1, id: 5, result: { key: "notes", deleted: true, shared: false } },
    ]);
    expect(storage.calls.every(([, pageId, scope]) => pageId === "pag_shown" && scope === "personal")).toBe(true);
    expect(asked()).toBe(0);
    expect(confirmed()).toBe(0);
  });

  it("takes the page from the frame, never from the message", async () => {
    const { frame, storage, bridge, settle } = setup();
    bridge.onMessage({ source: frame, data: call(1, "storage.get", { key: "notes", pageId: "pag_other" }) });
    bridge.onMessage({ source: frame, data: call(2, "storage.get", { key: "notes", options: { shared: true, pageId: "pag_other" } }) });
    await settle();
    expect(frame.replies.map((reply) => "error" in reply && reply.error.code)).toEqual(["bad_request", "bad_request"]);
    expect(storage.calls).toEqual([]);
  });

  it("checks every field strictly, and calls nothing for a bad one", () => {
    expect(parseStorage("storage.set", { key: "k", value: "", options: { shared: true } })).toEqual({ method: "storage.set", key: "k", value: "", shared: true });
    expect(parseStorage("storage.list", {})).toEqual({ method: "storage.list", prefix: "", shared: false });
    // Claude's plain boolean, as well as { shared }.
    expect(parseStorage("storage.get", { key: "k", options: true })).toEqual({ method: "storage.get", key: "k", shared: true });
    expect(parseStorage("storage.list", { prefix: "p", options: false })).toEqual({ method: "storage.list", prefix: "p", shared: false });
    expect(parseStorage("storage.get", { key: "é".repeat(MAX_KEY_CHARS) })).toBeDefined();
    for (const [method, params] of [
      ["storage.get", "k"],
      ["storage.get", { key: "" }],
      ["storage.get", { key: 42 }],
      ["storage.get", { key: "k".repeat(MAX_KEY_CHARS + 1) }],
      ["storage.get", { key: ".." }],
      ["storage.delete", { key: "." }],
      ["storage.get", { key: "k", value: "v" }],
      ["storage.get", { key: "k", options: "shared" }],
      ["storage.get", { key: "k", options: 1 }],
      ["storage.get", { key: "k", options: null }],
      ["storage.get", { key: "k", options: { shared: "yes" } }],
      ["storage.set", { key: "k" }],
      ["storage.set", { key: "k", value: { a: 1 } }],
      ["storage.list", { prefix: 7 }],
      ["storage.list", { prefix: "p".repeat(MAX_KEY_CHARS + 1) }],
      ["storage.list", { key: "k" }],
    ] as const) {
      expect(parseStorage(method, params), `${method} ${JSON.stringify(params)}`).toBeUndefined();
    }
  });

  it("asks before a shared write, and calls nothing when the reader cancels", async () => {
    const { frame, storage, bridge, settle } = setup({ confirmShared: async () => false });
    bridge.onMessage({ source: frame, data: call(1, "storage.set", { key: "votes", value: "1", options: { shared: true } }) });
    bridge.onMessage({ source: frame, data: call(2, "storage.delete", { key: "votes", options: { shared: true } }) });
    await settle();
    expect(frame.replies).toEqual([
      { nexus: 1, id: 1, error: { code: "cancelled", message: expect.stringContaining("everyone who can open it") } },
      { nexus: 1, id: 2, error: { code: "cancelled", message: expect.any(String) } },
    ]);
    expect(storage.calls).toEqual([]);
  });

  it("writes shared data once the reader allows it, and reads it with no question", async () => {
    const { frame, storage, bridge, settle, confirmed } = setup();
    bridge.onMessage({ source: frame, data: call(1, "storage.set", { key: "votes", value: "1", options: { shared: true } }) });
    await settle();
    bridge.onMessage({ source: frame, data: call(2, "storage.get", { key: "votes", options: { shared: true } }) });
    bridge.onMessage({ source: frame, data: call(3, "storage.list", { options: { shared: true } }) });
    await settle();
    expect(confirmed()).toBe(1);
    expect(storage.calls).toEqual([
      ["set", "pag_shown", "shared", "votes"],
      ["get", "pag_shown", "shared", "votes"],
      ["list", "pag_shown", "shared", ""],
    ]);
    expect(frame.replies.slice(1)).toEqual([
      { nexus: 1, id: 2, result: { key: "votes", value: "1", shared: true } },
      { nexus: 1, id: 3, result: { keys: ["votes"], prefix: "", shared: true } },
    ]);
  });

  it("answers unavailable when the reader is signed out, with no call and no question", async () => {
    const { frame, bridge, settle, confirmed } = setup({ storage: () => null });
    bridge.onMessage({ source: frame, data: call(1, "storage.set", { key: "votes", value: "1", options: { shared: true } }) });
    await settle();
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, error: { code: "unavailable", message: expect.stringContaining("signed in") } }]);
    expect(confirmed()).toBe(0);
  });

  it("passes on Gateway's storage codes with fixed messages, never Gateway's own text", async () => {
    for (const [failure, code] of [
      [new CoreError(413, "secret: org_42 keeps 20 MiB", "page_storage_full"), "page_storage_full"],
      [new CoreError(413, "secret", "page_storage_value_too_large"), "page_storage_value_too_large"],
      [new CoreError(400, "secret", "page_storage_key_invalid"), "page_storage_key_invalid"],
      [new CoreError(400, "secret", "page_storage_value_invalid"), "page_storage_value_invalid"],
      [new CoreError(400, "secret", "page_storage_scope_invalid"), "failed"],
      [new CoreError(404, "secret page pag_shown", "page_not_found"), "unavailable"],
      [new CoreError(403, "secret"), "unavailable"],
      [new CoreError(401, "secret"), "unavailable"],
      [new CoreError(429, "secret"), "limit"],
      [new Error("secret network"), "failed"],
    ] as const) {
      const failing = fakeStorage(failure as CoreError);
      const { frame, bridge, settle } = setup({ storage: () => failing.client });
      bridge.onMessage({ source: frame, data: call(1, "storage.set", { key: "k", value: "v" }) });
      await settle();
      expect(frame.replies).toEqual([{ nexus: 1, id: 1, error: { code, message: expect.any(String) } }]);
      expect(JSON.stringify(frame.replies)).not.toContain("secret");
    }
  });

  it("follows Gateway's cursor to list every key", async () => {
    const pages: { keys: string[]; nextCursor?: string }[] = [{ keys: ["a", "b"], nextCursor: "b" }, { keys: ["c"] }];
    const cursors: (string | undefined)[] = [];
    const { client } = fakeStorage();
    const listing: StorageClient = {
      ...client,
      pageStorageKeys: async (_pageId, scope, query = {}) => {
        cursors.push(query.cursor);
        return { scope, prefix: "", ...pages[cursors.length - 1]! };
      },
    };
    const { frame, bridge, settle } = setup({ storage: () => listing });
    bridge.onMessage({ source: frame, data: call(1, "storage.list", {}) });
    await settle();
    expect(cursors).toEqual([undefined, "b"]);
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, result: { keys: ["a", "b", "c"], prefix: "", shared: false } }]);
  });
});

describe("a frame that shows unsaved edits", () => {
  const call = (id: number, method: string, params: unknown) => ({ nexus: 1, id, method, params });

  it("refuses complete() and storage writes before any prompt or call, and still reads", async () => {
    const { frame, calls, storage, bridge, settle, asked, confirmed } = setup({ preview: () => true });
    await storage.client.setPageStorageValue("pag_shown", "personal", "k", "v");
    storage.calls.length = 0;
    bridge.onMessage({ source: frame, data: call(1, "complete", "Hello") });
    bridge.onMessage({ source: frame, data: call(2, "storage.set", { key: "k", value: "w" }) });
    bridge.onMessage({ source: frame, data: call(3, "storage.delete", { key: "k", options: { shared: true } }) });
    bridge.onMessage({ source: frame, data: call(4, "storage.get", { key: "k" }) });
    bridge.onMessage({ source: frame, data: call(5, "storage.list", {}) });
    await settle();
    const refused = { code: "preview", message: "Saved versions only: this page shows unsaved edits." };
    expect(frame.replies.slice(0, 3)).toEqual([
      { nexus: 1, id: 1, error: refused },
      { nexus: 1, id: 2, error: refused },
      { nexus: 1, id: 3, error: refused },
    ]);
    expect(frame.replies.slice(3).map((reply) => reply.id)).toEqual([4, 5]);
    expect(frame.replies.slice(3).every((reply) => "result" in reply)).toBe(true);
    expect((frame.replies[3] as { result: unknown }).result).toEqual({ key: "k", value: "v", shared: false });
    expect([calls, asked(), confirmed()]).toEqual([[], 0, 0]);
    expect(storage.calls.every(([method]) => method === "get" || method === "list")).toBe(true);
  });

  it("answers as before once the frame shows a version", async () => {
    let preview = true;
    const { frame, calls, bridge, settle } = setup({ preview: () => preview });
    preview = false;
    bridge.onMessage({ source: frame, data: call(1, "complete", "Hello") });
    bridge.onMessage({ source: frame, data: call(2, "storage.set", { key: "k", value: "w" }) });
    await settle();
    expect(frame.replies).toEqual([
      { nexus: 1, id: 1, result: "Hi there" },
      { nexus: 1, id: 2, result: expect.anything() },
    ]);
    expect(calls).toHaveLength(1);
  });
});

describe("window.fs.readFile on the page bridge", () => {
  const read = (id: number, params: unknown) => ({ source: null as unknown, data: { nexus: 1, id, method: "fs.readFile", params } });

  it("reads a text file as text and a spreadsheet as bytes, from the page and version the frame shows, with no question", async () => {
    const { frame, storage, bridge, settle, asked, confirmed } = setup({ version: () => 2 });
    for (const [id, params] of [
      [1, { name: "sales.csv", options: { encoding: "utf8" } }],
      [2, { name: "sales.csv" }],
      [3, { name: "book.xlsx" }],
      [4, { name: "sales.csv", options: "utf-8" }],
    ] as const) {
      bridge.onMessage({ ...read(id, params), source: frame });
    }
    await settle();
    const results = Object.fromEntries(frame.replies.map((reply) => [reply.id, "result" in reply ? reply.result : reply.error.code]));
    expect(results[1]).toBe("region,sales\nnorth,12\n");
    expect(results[2]).toBe("region,sales\nnorth,12\n");
    expect(results[3]).toEqual(new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0xff, 0x00]));
    expect(results[4]).toBe("region,sales\nnorth,12\n");
    expect(storage.calls.every(([method, pageId, , version]) => method === "file" && pageId === "pag_shown" && version === "2")).toBe(true);
    expect(asked() + confirmed()).toBe(0);
  });

  it("checks the name and the encoding, and calls nothing for a bad one", async () => {
    expect(parseReadFile({ name: "a.csv" })).toEqual({ name: "a.csv", text: false });
    expect(parseReadFile({ name: "a.csv", options: { encoding: "UTF-8" } })).toEqual({ name: "a.csv", text: true });
    expect(parseReadFile({ name: "a.csv", options: null })).toEqual({ name: "a.csv", text: false });
    for (const bad of [
      undefined,
      "a.csv",
      { name: "" },
      { name: "../a.csv" },
      { name: "..", options: {} },
      { name: "x".repeat(201) },
      { name: "a.csv", options: { encoding: "latin1" } },
      { name: "a.csv", options: { flag: "r" } },
      { name: "a.csv", pageId: "pag_other" },
      { name: 3 },
    ]) {
      expect(parseReadFile(bad), JSON.stringify(bad)).toBeUndefined();
    }
    const { frame, storage, bridge, settle } = setup();
    bridge.onMessage({ ...read(1, { name: "a.csv", pageId: "pag_other" }), source: frame });
    await settle();
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, error: { code: "bad_request", message: 'Pass a file name, and { encoding: "utf8" } or nothing.' } }]);
    expect(storage.calls).toEqual([]);
  });

  it("says file_not_found for a missing file, unavailable signed out or without access, never Gateway's text", async () => {
    const missing = setup();
    missing.bridge.onMessage({ ...read(1, { name: "other.csv" }), source: missing.frame });
    await missing.settle();
    expect(missing.frame.replies[0]).toMatchObject({ error: { code: "file_not_found" } });

    const signedOut = setup({ storage: () => null });
    signedOut.bridge.onMessage({ ...read(1, { name: "sales.csv" }), source: signedOut.frame });
    await signedOut.settle();
    expect(signedOut.frame.replies[0]).toMatchObject({ error: { code: "unavailable" } });

    const refused = fakeStorage(new CoreError(404, "no page pag_shown", "page_not_found"));
    const noAccess = setup({ storage: () => refused.client });
    noAccess.bridge.onMessage({ ...read(1, { name: "sales.csv" }), source: noAccess.frame });
    await noAccess.settle();
    const reply = noAccess.frame.replies[0];
    expect(reply).toMatchObject({ error: { code: "unavailable" } });
    expect(JSON.stringify(reply)).not.toContain("pag_shown");
  });
});

describe("the page bridge's queries", () => {
  const result = { name: "signups", paramsKey: "k".repeat(64), columns: [{ name: "week", type: "date" }], rows: [["2026-10-05"]], truncated: false, readAt: "2026-11-10T00:00:00.000Z", refreshSeconds: 300 };

  function withQueries(fail?: CoreError) {
    const runs: unknown[][] = [];
    let follow: ((event: PageEvent) => void) | undefined;
    let followed: AbortSignal | undefined;
    const storage = fakeStorage();
    const client: StorageClient = {
      ...storage.client,
      runPageQuery: async (pageId, name, params) => {
        runs.push([pageId, name, params]);
        if (fail) throw fail;
        return result;
      },
      pageEvents: async (_pageId, onEvent, signal) => {
        follow = onEvent;
        followed = signal;
      },
    };
    const shown = setup({ storage: () => client });
    return { ...shown, runs, emit: (event: PageEvent) => follow?.(event), followed: () => followed };
  }

  it("runs a query of the page it shows by name, and answers the shared result without asking the reader", async () => {
    const { frame, runs, bridge, settle, asked } = withQueries();
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "query", params: { name: "signups", params: { since: "2026-07-01" } } } });
    await settle();
    expect(runs).toEqual([["pag_shown", "signups", { since: "2026-07-01" }]]);
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, result: { columns: result.columns, rows: result.rows, truncated: false, readAt: result.readAt } }]);
    expect(asked()).toBe(0);
  });

  it("takes a name and values only: never a statement, a page or a connection", async () => {
    for (const bad of [{}, { name: "Signups" }, { name: "signups", sql: "delete from t" }, { name: "signups", params: [1] }, { name: "signups", pageId: "pag_other" }, "signups"]) {
      expect(parseQuery(bad), JSON.stringify(bad)).toBeUndefined();
    }
    expect(parseQuery({ name: "signups" })).toEqual({ name: "signups" });
    const { frame, runs, bridge, settle } = withQueries();
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "query", params: { name: "signups", sql: "select 1" } } });
    await settle();
    expect(frame.replies[0]).toMatchObject({ error: { code: "bad_request" } });
    expect(runs).toEqual([]);
  });

  it("passes Gateway's query codes through, and says unavailable when the data is no longer shared", async () => {
    const pending = withQueries(new CoreError(409, "waits", "page_query_pending"));
    pending.bridge.onMessage({ source: pending.frame, data: { nexus: 1, id: 1, method: "query", params: { name: "signups" } } });
    await pending.settle();
    // Not approved yet: an empty result the page can draw as such, not an error.
    expect(pending.frame.replies[0]).toEqual({ nexus: 1, id: 1, result: { columns: [], rows: [], truncated: false, readAt: null, pending: true } });
    const unknown = withQueries(new CoreError(404, "no query", "page_query_not_found"));
    unknown.bridge.onMessage({ source: unknown.frame, data: { nexus: 1, id: 1, method: "query", params: { name: "signups" } } });
    await unknown.settle();
    expect(unknown.frame.replies[0]).toMatchObject({ error: { code: "page_query_not_found" } });
    const gone = withQueries(new CoreError(403, "Postgres: connect db.example.com first", "connection_required"));
    gone.bridge.onMessage({ source: gone.frame, data: { nexus: 1, id: 1, method: "query", params: { name: "signups" } } });
    await gone.settle();
    expect(gone.frame.replies[0]).toMatchObject({ error: { code: "page_query_unavailable" } });
    expect(JSON.stringify(gone.frame.replies[0])).not.toContain("db.example.com");
  });

  it("posts each query event of its page to the frame once the page subscribes, until it closes", async () => {
    const { frame, bridge, settle, emit, followed } = withQueries();
    bridge.onMessage({ source: frame, data: { nexus: 1, id: 1, method: "query.subscribe", params: {} } });
    await settle();
    expect(frame.replies).toEqual([{ nexus: 1, id: 1, result: true }]);
    emit({ type: "query", data: { pageId: "pag_shown", name: "signups", paramsKey: "k", readAt: "2026-11-10T00:05:00.000Z" } });
    emit({ type: "query", data: { pageId: "pag_other", name: "signups", paramsKey: "k", readAt: "2026-11-10T00:05:00.000Z" } });
    emit({ type: "version", data: { pageId: "pag_shown", version: 3 } });
    expect(frame.replies.slice(1)).toEqual([{ nexus: 1, event: "query", data: { name: "signups", paramsKey: "k", readAt: "2026-11-10T00:05:00.000Z" } }]);
    bridge.dispose();
    expect(followed()?.aborted).toBe(true);
  });
});
