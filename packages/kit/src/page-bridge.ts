/**
 * The parent's side of a page's bridge: a page calls `window.nexus.complete()` (the sandbox host's
 * `BRIDGE_SCRIPT`, `pages-host/src/host.ts`), which posts `{nexus: 1, id, method, params}` to its frame's parent, or to
 * the phone app when the page is its WebView's top document. Nexus UI and Nexus for desktop
 * (`ui/.../pages/page-frame.tsx`) and the phone app (`mobile/src/page-view.tsx`) answer it here, with a
 * model call on the READER's own session, never the author's. The page never sees a token, a cookie or anything but
 * its answer.
 *
 * - Only a message whose `source` is this frame's window counts: under the CSP sandbox every page's origin is "null",
 *   so the window, not the origin, says which frame spoke. Anything else is ignored without a reply.
 * - The shape is checked strictly, the input is capped (`MAX_INPUT_CHARS`, `MAX_MESSAGES`), and so are the output
 *   tokens (`MAX_OUTPUT_TOKENS`) and the calls in flight per frame (`MAX_IN_FLIGHT`).
 * - Signed out: an error, and no call. Then the reader's consent, once per page (`consentKey`): Deny is an error.
 * - The reply goes to `event.source` only, and holds the answer's text or an error with a fixed message. Gateway's own
 *   error text never reaches the page.
 *
 * `method` + `params` is the protocol. Storage adds `storage.get|set|delete|list`, on the page's storage
 * routes with the reader's session:
 * - The page is the one this frame shows (`pageId`), never one the message names.
 * - No AI consent: it costs the reader nothing. But a page's first shared write (set or delete with `shared: true`)
 *   asks the reader once (`sharedWriteKey`), because every reader of the page sees it.
 * - Signed out is `unavailable`. Gateway's storage codes pass through, each with a fixed message.
 *
 * A frame that shows unsaved edits (a live page's preview) refuses `complete()` and storage writes (`set`,
 * `delete`) with `preview`, before any prompt or call. Readers' frames reload at each preview, several times a second
 * while someone types, so a page that calls on load would otherwise bill every reader, or rewrite storage, at every
 * reload. Reads (`get`, `list`) still work. A frame that shows a version answers as before.
 *
 * Files add `fs.readFile`: one of the page's data files by name, the read-only copies of the files
 * attached in the chat that wrote it, on `GET /v1/pages/{id}/files/{name}` with the reader's session. The same page
 * as storage, the same access check as opening the page, and no prompt: reading costs nothing and writes nothing. It
 * reads the files of the version the frame shows, which its source names. It answers text with `{ encoding }` or for
 * a text file, else the bytes (a `Uint8Array`).
 *
 * Data adds `query`: one of the page's approved queries by name, with its parameter values, on
 * `POST /v1/pages/{id}/queries/{name}/run` with the reader's session. It answers the result every reader shares
 * (`{ columns, rows, truncated, readAt, error? }`): the page never names a statement or a connection, and Gateway runs
 * the query as its approver. No prompt: it costs the reader nothing and writes nothing, so it works in a preview too.
 * A query its owner has not approved yet answers no rows and `pending: true`, so the page shows an empty
 * state rather than an error.
 * `query.subscribe` (the page's first `onQuery`) follows the page's events and posts each `query` event to the frame
 * as `{ nexus: 1, event: "query", data: { name, paramsKey, readAt } }`, so every open reader reads the new result.
 */
import type { ChatCompletionRequest, ChatMessage } from "./api";
import type { ArtifactsClient } from "./artifacts";
import { CoreError, type CoreClient } from "./core-client";
import type { PageEvent, PageFile, PageQueryEvent, PageStorageScope } from "./pages";

/** Gateway picks the model (the Rust gateway's `AUTO_MODEL`). */
export const PAGE_MODEL = "nexus/auto";
export const MAX_INPUT_CHARS = 50_000;
export const MAX_MESSAGES = 50;
export const DEFAULT_OUTPUT_TOKENS = 1_024;
export const MAX_OUTPUT_TOKENS = 4_096;
export const MAX_IN_FLIGHT = 2;
/** Storage calls are cheap and quick, so a page may run more of them at once (`Promise.all` over its keys). */
export const MAX_STORAGE_IN_FLIGHT = 16;
/** Gateway's key and prefix limit, in characters (`nexus-pages/src/pages/storage.rs`, `MAX_KEY_CHARS`). */
export const MAX_KEY_CHARS = 200;
/** The longest file name Gateway keeps (`nexus-base/src/files.rs`, `MAX_NAME_CHARS`). */
export const MAX_FILE_NAME_CHARS = 200;
/** `list` follows Gateway's cursor for at most this many keys (Gateway lists 1,000 per call). */
export const MAX_LISTED_KEYS = 10_000;
/** A call Gateway has not answered by then is cancelled; the page gets `failed`. */
export const CALL_TIMEOUT_MS = 120_000;
const ROLES = new Set<ChatMessage["role"]>(["system", "user", "assistant"]);
const STORAGE_METHODS = new Set(["storage.get", "storage.set", "storage.delete", "storage.list"]);
/** Gateway's storage codes the page may see as they are. */
const STORAGE_CODES = new Set(["page_storage_full", "page_storage_value_too_large", "page_storage_value_invalid", "page_storage_key_invalid"]);

export type BridgeErrorCode =
  | "bad_request"
  | "unknown_method"
  | "busy"
  | "signed_out"
  | "denied"
  | "refused"
  | "limit"
  | "failed"
  | "unavailable"
  | "cancelled"
  | "preview"
  | "page_storage_full"
  | "page_storage_value_too_large"
  | "page_storage_value_invalid"
  | "page_storage_key_invalid"
  | "file_not_found"
  | "page_query_not_found"
  | "page_query_pending"
  | "page_query_failed"
  | "page_query_unconfigured"
  | "page_queries_off"
  | "page_query_params_invalid"
  | "page_query_unavailable";

const MESSAGES: Record<BridgeErrorCode, string> = {
  bad_request: "Pass a prompt string, or { messages: [{ role, content }], maxTokens? }.",
  unknown_method: "Nexus does not know that method.",
  busy: "Too many Nexus calls at once. Wait for one to finish.",
  signed_out: "Sign in to Nexus to use AI on this page.",
  denied: "The reader did not allow this page to use Nexus AI.",
  refused: "Your organization's policy does not allow this call.",
  limit: "The reader's account has reached a limit. Try again later.",
  failed: "The Nexus call failed.",
  unavailable: "This works only when the page is open in Nexus, signed in.",
  cancelled: "The reader did not allow this page to save data that everyone who can open it will see.",
  preview: "Saved versions only: this page shows unsaved edits.",
  page_storage_full: "This page's storage is full (20 MB). Delete some keys first.",
  page_storage_value_too_large: "A value is at most 5 MB.",
  page_storage_value_invalid: "A value is text with no NUL character. Store JSON as a string.",
  page_storage_key_invalid: "A key is 1 to 200 characters, with no spaces, slashes or quotes.",
  file_not_found: "This page has no file by that name. Files attached in its chat are copied when the page is written.",
  page_query_not_found: "This page has no query by that name.",
  page_query_pending: "This query waits for the page's owner to approve it.",
  page_query_failed: "The query failed, and there is no earlier result to show.",
  page_query_unconfigured: "This Nexus does not offer the query's connector.",
  page_queries_off: "Your organization has not turned on live data for pages.",
  page_query_params_invalid: "Pass the query's parameter values by name, each of its declared type.",
  page_query_unavailable: "The query's data is no longer shared: its connection or its approval has gone.",
};
const QUERY_BAD_REQUEST = "Pass a query name, and an object of parameter values or nothing.";
const READ_FILE_BAD_REQUEST = 'Pass a file name, and { encoding: "utf8" } or nothing.';
const STORAGE_BAD_REQUEST = "Pass a key string, a string value for set, and true, { shared: true } or nothing last.";

export type BridgeReply = { nexus: 1; id: number | string; result: unknown } | { nexus: 1; id: number | string; error: { code: BridgeErrorCode; message: string } };

/** A page's call, checked: what goes to Gateway. */
export interface CompleteParams {
  messages: ChatMessage[];
  maxTokens: number;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const onlyKeys = (value: Record<string, unknown>, keys: string[]) => Object.keys(value).every((key) => keys.includes(key));

/** The request id the reply carries: the shim's counter, or a short string. Anything else gets no reply. */
export function requestId(data: unknown): number | string | undefined {
  if (!isRecord(data) || data.nexus !== 1 || typeof data.method !== "string") return undefined;
  const id = data.id;
  if (typeof id === "number" && Number.isSafeInteger(id)) return id;
  if (typeof id === "string" && id.length > 0 && id.length <= 64) return id;
  return undefined;
}

/** `complete`'s input: a prompt string, or `{ messages, maxTokens? }` and nothing else. undefined when it is not one. */
export function parseComplete(params: unknown): CompleteParams | undefined {
  let messages: unknown = params;
  let maxTokens: unknown = DEFAULT_OUTPUT_TOKENS;
  if (typeof params === "string") messages = [{ role: "user", content: params }];
  else if (isRecord(params) && onlyKeys(params, ["messages", "maxTokens"])) {
    messages = params.messages;
    maxTokens = params.maxTokens ?? DEFAULT_OUTPUT_TOKENS;
  } else return undefined;
  if (!Array.isArray(messages) || messages.length === 0 || messages.length > MAX_MESSAGES) return undefined;
  if (typeof maxTokens !== "number" || !Number.isInteger(maxTokens) || maxTokens < 1 || maxTokens > MAX_OUTPUT_TOKENS) return undefined;
  let chars = 0;
  const checked: ChatMessage[] = [];
  for (const message of messages) {
    if (!isRecord(message) || !onlyKeys(message, ["role", "content"])) return undefined;
    const { role, content } = message;
    if (typeof content !== "string" || !ROLES.has(role as ChatMessage["role"])) return undefined;
    chars += content.length;
    checked.push({ role: role as ChatMessage["role"], content });
  }
  if (chars === 0 || chars > MAX_INPUT_CHARS) return undefined;
  return { messages: checked, maxTokens };
}

export function completionRequest(params: CompleteParams): ChatCompletionRequest {
  return { model: PAGE_MODEL, messages: params.messages, max_tokens: params.maxTokens };
}

/** A storage call, checked: one of the four methods, its key or prefix, its value for `set`, and its scope. */
export type StorageCall =
  | { method: "storage.get" | "storage.delete"; key: string; shared: boolean }
  | { method: "storage.set"; key: string; value: string; shared: boolean }
  | { method: "storage.list"; prefix: string; shared: boolean };

const keyText = (value: unknown, empty: boolean): value is string =>
  typeof value === "string" && (empty || value.length > 0) && [...value].length <= MAX_KEY_CHARS;

/**
 * A storage method's params, as the shim sends them: `{ key | prefix, value?, options? }`, where `options` is a boolean
 * or `{ shared? }`. undefined when bad.
 */
export function parseStorage(method: string, params: unknown): StorageCall | undefined {
  if (!isRecord(params)) return undefined;
  const options = params.options;
  // Claude's artifact storage takes a plain boolean last; Nexus also takes { shared }. Nothing else.
  const object = isRecord(options) && onlyKeys(options, ["shared"]) && (options.shared === undefined || typeof options.shared === "boolean");
  if (options !== undefined && typeof options !== "boolean" && !object) return undefined;
  const shared = options === true || (isRecord(options) && options.shared === true);
  if (method === "storage.list") {
    if (!onlyKeys(params, ["prefix", "options"])) return undefined;
    const prefix = params.prefix ?? "";
    return keyText(prefix, true) ? { method, prefix, shared } : undefined;
  }
  const key = params.key;
  // "." and ".." would climb the route's path once the URL is resolved: Gateway would never see them as a key.
  if (!keyText(key, false) || key === "." || key === "..") return undefined;
  if (method === "storage.set") {
    if (!onlyKeys(params, ["key", "value", "options"]) || typeof params.value !== "string") return undefined;
    return { method, key, value: params.value, shared };
  }
  if ((method === "storage.get" || method === "storage.delete") && onlyKeys(params, ["key", "options"])) return { method, key, shared };
  return undefined;
}

/** A storage call's answer, in the shape of Claude's artifact storage. `get` of a missing key is null. */
export async function runStorage(client: StorageClient, pageId: string, call: StorageCall): Promise<unknown> {
  const scope: PageStorageScope = call.shared ? "shared" : "personal";
  const shared = call.shared;
  switch (call.method) {
    case "storage.get":
      try {
        const stored = await client.pageStorageValue(pageId, scope, call.key);
        return { key: stored.key, value: stored.value, shared };
      } catch (failure) {
        if (failure instanceof CoreError && failure.code === "page_storage_key_not_found") return null;
        throw failure;
      }
    case "storage.set":
      await client.setPageStorageValue(pageId, scope, call.key, call.value);
      return { key: call.key, value: call.value, shared };
    case "storage.delete": {
      const { deleted } = await client.deletePageStorageValue(pageId, scope, call.key);
      return { key: call.key, deleted, shared };
    }
    case "storage.list": {
      const keys: string[] = [];
      let cursor: string | undefined;
      // Stops at MAX_LISTED_KEYS. Pass a cursor to the page when a page needs more keys than that.
      do {
        const page = await client.pageStorageKeys(pageId, scope, { prefix: call.prefix || undefined, cursor });
        keys.push(...page.keys);
        cursor = page.nextCursor;
      } while (cursor && keys.length < MAX_LISTED_KEYS);
      return { keys: keys.slice(0, MAX_LISTED_KEYS), prefix: call.prefix, shared };
    }
  }
}

/** `fs.readFile`'s call, checked: the file's name, and whether to answer text whatever the file is. */
export interface ReadFileCall {
  name: string;
  text: boolean;
}

const ENCODINGS = new Set(["utf8", "utf-8"]);

/** `fs.readFile`'s params, as the shim sends them: `{ name, options? }`, `options` being `{ encoding? }` or an encoding. */
export function parseReadFile(params: unknown): ReadFileCall | undefined {
  if (!isRecord(params) || !onlyKeys(params, ["name", "options"])) return undefined;
  const { name, options } = params;
  if (typeof name !== "string" || name.length === 0 || [...name].length > MAX_FILE_NAME_CHARS || name.includes("/") || name === "." || name === "..") return undefined;
  let encoding: unknown = undefined;
  if (typeof options === "string") encoding = options;
  else if (isRecord(options) && onlyKeys(options, ["encoding"])) encoding = options.encoding;
  else if (options !== undefined && options !== null) return undefined;
  if (encoding !== undefined && encoding !== null && !(typeof encoding === "string" && ENCODINGS.has(encoding.toLowerCase()))) return undefined;
  return { name, text: typeof encoding === "string" };
}

/** Whether a data file of `mediaType` is text: every type but XLSX. */
export const isTextFile = (mediaType: string) => mediaType !== "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/** A file's bytes from Gateway's base64. */
export function fileBytes(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

/** `fs.readFile`'s answer: the file's text (UTF-8, without a byte-order mark) or its bytes. */
export async function runReadFile(client: Pick<ArtifactsClient, "pageFile">, pageId: string, call: ReadFileCall, version?: number): Promise<string | Uint8Array> {
  const file: PageFile = await client.pageFile(pageId, call.name, version);
  const bytes = fileBytes(file.data ?? "");
  return call.text || isTextFile(file.mediaType) ? new TextDecoder().decode(bytes) : bytes;
}

/** A storage failure as the page may see it: Gateway's storage code, or a code of the bridge's own. */
export function storageFailureCode(failure: unknown): BridgeErrorCode {
  if (!(failure instanceof CoreError)) return "failed";
  if (failure.code && STORAGE_CODES.has(failure.code)) return failure.code as BridgeErrorCode;
  if (failure.code === "page_file_not_found") return "file_not_found";
  // Signed out, or a reader who may no longer open the page (a revoked share, a public link with no Nexus identity).
  if (failure.status === 401 || failure.status === 403 || failure.status === 404) return "unavailable";
  if (failure.status === 429) return "limit";
  return "failed";
}

/** A Gateway failure as the page may see it: a code and a fixed message, never Gateway's text. */
export function failureCode(failure: unknown): BridgeErrorCode {
  if (!(failure instanceof CoreError)) return "failed";
  if (failure.status === 401) return "signed_out";
  if (failure.status === 403) return "refused";
  if (failure.status === 402 || failure.status === 429) return "limit";
  return "failed";
}

/** `query`'s call, checked: the query's name and its parameter values. */
export interface QueryCall {
  name: string;
  params?: Record<string, unknown>;
}

/** `query`'s params, as the shim sends them: `{ name, params? }`. undefined when it is not one. */
export function parseQuery(params: unknown): QueryCall | undefined {
  if (!isRecord(params) || !onlyKeys(params, ["name", "params"])) return undefined;
  const { name, params: values } = params;
  if (typeof name !== "string" || !QUERY_NAME.test(name)) return undefined;
  if (values === undefined || values === null) return { name };
  if (!isRecord(values)) return undefined;
  return { name, params: values };
}

/** A query failure as the page may see it: Gateway's query code, or a code of the bridge's own. */
export function queryFailureCode(failure: unknown): BridgeErrorCode {
  if (!(failure instanceof CoreError)) return "failed";
  if (failure.code && QUERY_CODES.has(failure.code)) return failure.code as BridgeErrorCode;
  // The approver's access to the connection ended, or they left: the results went with it.
  if (failure.status === 403 || failure.status === 409) return "page_query_unavailable";
  return storageFailureCode(failure);
}

/** Where a reader's Allow or Deny for one page is kept, in this browser. */
export const consentKey = (pageId: string) => `nexus.pageAi.${pageId}`;
/** Where a reader's Allow or Cancel for one page's shared writes is kept, in this browser. */
export const sharedWriteKey = (pageId: string) => `nexus.pageSharedStorage.${pageId}`;

export type StorageClient = Pick<ArtifactsClient, "pageStorageValue" | "setPageStorageValue" | "deletePageStorageValue" | "pageStorageKeys" | "pageFile"> &
  Partial<Pick<ArtifactsClient, "runPageQuery" | "pageEvents">>;
/** Gateway's query codes the page may see as they are (`nexus-pages/src/pages/query_runs.rs`). */
const QUERY_CODES = new Set(["page_query_not_found", "page_query_pending", "page_query_failed", "page_query_unconfigured", "page_queries_off", "page_query_params_invalid"]);
/** A query's name, as Gateway takes it. */
const QUERY_NAME = /^[a-z][a-z0-9_]{0,62}$/;

/** A message event, as much of it as the bridge reads. */
export interface BridgeEvent {
  source: unknown;
  data: unknown;
}

export interface BridgeDeps {
  /** The frame's window (`iframe.contentWindow`): the only source answered. */
  frame: () => unknown;
  /** The reader's Gateway client; null when signed out. */
  core: () => Pick<CoreClient, "chatCompletion"> | null | undefined;
  /** Whether the reader allows this page: a remembered choice, or the prompt's answer. */
  consent: () => Promise<boolean>;
  /** The page this frame shows: the only page whose storage it reaches. */
  pageId: string;
  /** The reader's storage client; null when signed out. */
  storage: () => StorageClient | null | undefined;
  /** Whether the reader allows this page's shared writes: a remembered choice, or the prompt's answer. */
  confirmShared: () => Promise<boolean>;
  /** Whether the frame shows unsaved edits: then `complete()` and storage writes are refused. */
  preview?: () => boolean;
  /** The version the frame shows: `fs.readFile` reads that version's files. None: the newest the reader may open. */
  version?: () => number | undefined;
}

/** What a frame is sent when one of its page's queries has a new result. */
export interface BridgeQueryEvent {
  nexus: 1;
  event: "query";
  data: Pick<PageQueryEvent, "name" | "paramsKey" | "readAt">;
}

type Outcome = { result: unknown } | { error: BridgeErrorCode; message?: string };

/** One frame's bridge: `onMessage` for `window`'s `message` events; `dispose` cancels the calls in flight. */
export function createBridge(deps: BridgeDeps) {
  const inFlight = new Set<AbortController>();
  let storageInFlight = 0;

  // No timeout or abort here, as the kit's storage calls take no signal. Gateway's own timeouts bound them.
  async function store(params: unknown, method: string): Promise<Outcome> {
    const call = parseStorage(method, params);
    if (!call) return { error: "bad_request", message: STORAGE_BAD_REQUEST };
    const write = call.method === "storage.set" || call.method === "storage.delete";
    if (write && deps.preview?.()) return { error: "preview" };
    if (storageInFlight >= MAX_STORAGE_IN_FLIGHT) return { error: "busy" };
    storageInFlight++;
    try {
      if (!deps.storage()) return { error: "unavailable" };
      if (call.shared && write && !(await deps.confirmShared())) return { error: "cancelled" };
      // Read again after the prompt: the session may have ended while the reader decided.
      const client = deps.storage();
      if (!client) return { error: "unavailable" };
      return { result: await runStorage(client, deps.pageId, call) };
    } catch (failure) {
      return { error: storageFailureCode(failure) };
    } finally {
      storageInFlight--;
    }
  }

  // One of the page's files: no prompt, and the storage calls' cap, since it reads through the same client.
  async function readFile(params: unknown): Promise<Outcome> {
    const call = parseReadFile(params);
    if (!call) return { error: "bad_request", message: READ_FILE_BAD_REQUEST };
    if (storageInFlight >= MAX_STORAGE_IN_FLIGHT) return { error: "busy" };
    storageInFlight++;
    try {
      const client = deps.storage();
      if (!client) return { error: "unavailable" };
      return { result: await runReadFile(client, deps.pageId, call, deps.version?.()) };
    } catch (failure) {
      return { error: storageFailureCode(failure) };
    } finally {
      storageInFlight--;
    }
  }

  // One of the page's queries: no prompt, and the storage calls' cap, since it reads through the same client.
  async function query(params: unknown): Promise<Outcome> {
    const call = parseQuery(params);
    if (!call) return { error: "bad_request", message: QUERY_BAD_REQUEST };
    if (storageInFlight >= MAX_STORAGE_IN_FLIGHT) return { error: "busy" };
    storageInFlight++;
    try {
      const client = deps.storage();
      if (!client?.runPageQuery) return { error: "unavailable" };
      const result = await client.runPageQuery(deps.pageId, call.name, call.params);
      return { result: { columns: result.columns, rows: result.rows, truncated: result.truncated, readAt: result.readAt, ...(result.error ? { error: result.error } : {}) } };
    } catch (failure) {
      // A query chat proposed that its owner has not approved yet: no rows, and `pending`, so the page shows
      // its empty state, not an error. Approving it refreshes it, and `onQuery` tells the page.
      if (failure instanceof CoreError && failure.code === "page_query_pending") return { result: { columns: [], rows: [], truncated: false, readAt: null, pending: true } };
      return { error: queryFailureCode(failure) };
    } finally {
      storageInFlight--;
    }
  }

  // The page's `onQuery`: one stream of the page's events per frame, for as long as the frame is open.
  let following: AbortController | undefined;
  function subscribe(source: Window): Outcome {
    if (following) return { result: true };
    const client = deps.storage();
    if (!client?.pageEvents) return { error: "unavailable" };
    const controller = new AbortController();
    following = controller;
    const onEvent = (event: PageEvent) => {
      if (event.type !== "query" || !isRecord(event.data)) return;
      const { pageId, name, paramsKey, readAt } = event.data as Partial<PageQueryEvent>;
      if (pageId !== deps.pageId || typeof name !== "string" || typeof paramsKey !== "string" || typeof readAt !== "string") return;
      const message: BridgeQueryEvent = { nexus: 1, event: "query", data: { name, paramsKey, readAt } };
      source.postMessage(message, "*");
    };
    // A stream that ends (a deploy, a lost share) is not followed again; the page still reads on each call.
    void client.pageEvents(deps.pageId, onEvent, controller.signal).catch(() => undefined);
    return { result: true };
  }

  async function answer(params: unknown, method: string, source: Window): Promise<Outcome> {
    if (STORAGE_METHODS.has(method)) return store(params, method);
    if (method === "fs.readFile") return readFile(params);
    if (method === "query") return query(params);
    if (method === "query.subscribe") return subscribe(source);
    if (method !== "complete") return { error: "unknown_method" };
    const checked = parseComplete(params);
    if (!checked) return { error: "bad_request" };
    if (deps.preview?.()) return { error: "preview" };
    // A cap per frame, not per reader or per page. A page in two tabs gets twice as many; Gateway's own
    // limits and budgets still bound the spend.
    if (inFlight.size >= MAX_IN_FLIGHT) return { error: "busy" };
    const controller = new AbortController();
    inFlight.add(controller);
    try {
      if (!deps.core()) return { error: "signed_out" };
      if (!(await deps.consent())) return { error: "denied" };
      // Read again after the prompt: the session may have ended while the reader decided.
      const core = deps.core();
      if (!core || controller.signal.aborted) return { error: core ? "failed" : "signed_out" };
      const timer = setTimeout(() => controller.abort(), CALL_TIMEOUT_MS);
      try {
        const response = await core.chatCompletion(completionRequest(checked), controller.signal);
        return { result: response.choices[0]?.message.content ?? "" };
      } finally {
        clearTimeout(timer);
      }
    } catch (failure) {
      return { error: failureCode(failure) };
    } finally {
      inFlight.delete(controller);
    }
  }

  return {
    onMessage(event: BridgeEvent) {
      const source = event.source;
      if (!source || source !== deps.frame()) return;
      const id = requestId(event.data);
      if (id === undefined) return;
      const data = event.data as { method: string; params?: unknown };
      void answer(data.params, data.method, source as Window).then((outcome) => {
        const reply: BridgeReply =
          "result" in outcome ? { nexus: 1, id, result: outcome.result } : { nexus: 1, id, error: { code: outcome.error, message: outcome.message ?? MESSAGES[outcome.error] } };
        // `*`: the page's origin is "null" under the sandbox, so no origin names it. The window does.
        (source as Window).postMessage(reply, "*");
      });
    },
    dispose() {
      following?.abort();
      for (const controller of inFlight) controller.abort();
      inFlight.clear();
    },
  };
}
