/**
 * Typed client for the Nexus API (11 §4 `kit/core-client`). The CLI, Nexus UI and every module use it.
 * It is a thin HTTP client, not an SDK: the API is the contract.
 */
import { recordClockOffset } from "./code-control-clock";
import type { CodeCommand, CodeCommandCancel, CodeCommandChanged, CodeCommandClaim, CodeCommandEnvelope, CodeCommandResult, CodeControl, CodeControlLease } from "./code-control-types";
import type { ComputerList, ComputerPresence, ComputerReplyEnvelope, ComputerRequest, ComputerRequestChanged, ComputerRequestClaim, ComputerRequestEnvelope, PhonePairingEnvelope, PhonePairingRequest } from "./computer-types";
import type {
  App, AppList, AppClaim, AppDeleted, DeployAppRequest, SetAppCapRequest,
  AgentAdoption,
  ApproveDeviceRequest,
  DeviceList,
  SetThreadAnswerRating,
  ThreadAnswerRating,
  ThreadAnswerRatings,
  SyncedEventPage,
  SessionPin,
  SessionPinList,
  SessionPinSource,
  SetSessionPin,
  SyncedThread,
  SyncedThreadDeleted,
  SyncedThreadAdvanced,
  SyncedThreadList,
  SyncedThreadPresence,
  ThreadKey,
  ThreadKeyList,
  PutThreadKeysRequest,
  ThreadKeysStored,
  RecoverDeviceRequest,
  RecoveryChallenge,
  E2eDevice,
  RegisterDeviceRequest,
  RevokeDeviceRequest,
  TeamKeyHolderList,
  TeamKeyState,
  TeamThreadKeyList,
  AgentLink,
  AgentLinkList,
  DeveloperClient,
  DeveloperClientList,
  RegisterDeveloperClientRequest,
  RevokeDeveloperClientRequest,
  AgentProfile,
  AgentProfileList,
  CreateAgentProfileRequest,
  SetAgentProfileRequest,
  AgentAdoptionQuery,
  AuditQuery,
  AuditResponse,
  AgentAccessResponse,
  AgentCheckResponse,
  AgentModelPolicy,
  AgentName,
  AgentPolicyResponse,
  AgentRoute,
  AgentSessionSummary,
  ApiError,
  ChatCompletionChunk,
  ChatCompletionRequest,
  ToolChatCompletionRequest,
  ChatToolCall,
  ChatCompletionResponse,
  ChatMessageResult,
  ChatAttachment,
  ChatChannelBinding,
  ChatChannelList,
  ChatChannelSettings,
  ChatChannelUnbound,
  ChatWorkspaceList,
  ChatWorkspaceRemoved,
  CompleteSlackInstallRequest,
  ChatSessionDetail,
  ChatSessionMessage,
  ChatSessionsQuery,
  ChatSessionsResponse,
  ChatSessionSummary,
  ChatSuggestion,
  ChatStreamChunk,
  ChatUsageReport,
  GuestChatRequest,
  GuestChatStatus,
  CreateChatSessionRequest,
  DiagnosticReport,
  Feedback,
  OrgRecoveryKeyState,
  OrgSettings,
  SendChatMessageRequest,
  AskCodeRequest,
  AskWebRequest,
  ChatWebResult,
  ConnectorSummary,
  ConnectorDetail,
  Connection,
  ConnectionScope,
  ConnectionsResponse,
  ConnectorAvailability,
  DisconnectedResponse,
  OAuthFinishRequest,
  OAuthFinishedResponse,
  SetConnectorAvailability,
  Approval,
  CreateApproval,
  AgentWriteSetting,
  ChatWriteSetting,
  SetChatWrites,
  CreateTenantResponse,
  CurrentTenantResponse,
  DeviceCodeResponse,
  DeviceScope,
  DeviceTokenResponse,
  EndAgentSessionRequest,
  GithubAppStatus,
  InvitationsRequest,
  InvitationsResponse,
  PendingInvitationList,
  ReceivedInvitationList,
  InvitationResent,
  CreateOrgRequest,
  OrgProfileChange,
  AddDomainRequest,
  DomainList,
  DomainSuggestionList,
  OrgDomain,
  MemberRecovered,
  AccountDeleted,
  MembersResponse,
  RemovedMembersResponse,
  ModelListResponse,
  ModelPolicyView,
  ModelRule,
  PolicyTestRequest,
  PolicyTestResponse,
  PendingPeopleResponse,
  RepositoriesResponse,
  Role,
  TokenSummary,
  ModulesResponse,
  OpenAgentSessionRequest,
  ProviderKeyChange,
  ProviderKeyHolder,
  ProviderKeySummary,
  ProviderList,
  RunReport,
  RunSummary,
  RunVersions,
  RunsPage,
  RunsQuery,
  WriteGrantList,
  ReviewEventLevel,
  ReviewEventSetting,
  PresenceHeartbeat,
  PresenceList,
  PresenceSettings,
  ReportedActivity,
  ReviewStatus,
  WriteGrantsSet,
  SpendResponse,
  OrgSummary,
  OrgWaitingCounts,
  SwitchedOrg,
  BudgetSetting,
  BillingPortalResponse,
  OrgBudget,
  OrgBudgetChange,
  BillingResponse,
  ReferralList,
  BillingStatementResponse,
  BuyCreditsResponse,
  BillingOperation,
  SetAutoTopupResponse,
  SlackInstall,
  SlackInstallStarted,
  BillingStatementsResponse,
  BudgetsResponse,
  LimitManagerChange,
  PaymentConfirmationResponse,
  PaymentMethodSetupResponse,
  EmbeddedCardSetup,
  ReceiptsResponse,
  SetAfterTheFactResponse,
  PlanChoice,
  PlanCatalogue,
  PlanChange,
  BillingPlan,
  PersonBudget,
  PersonLimitSetting,
  AddTeamMemberResponse,
  PendingTeamMember,
  TeamBudget,
  TeamBudgetChange,
  ProductBudget,
  ProductBudgetChange,
  BudgetEvery,
  TeamDetail,
  TeamImport,
  TeamImportDiff,
  TeamRole,
  TeamTreeResponse,
  TeamsResponse,
  UsageQuery,
  UsageReport,
  AgentSessionFilters,
  ExportFile,
  ExportFormat,
  SpendCall,
  SpendCallFilters,
  SpendCallPage,
  SpendCallsQuery,
  UsageFilters,
  CloudHistoryEventPage,
  CreateSession,
  Session,
  SessionList,
  SessionMessage,
  VoiceSpeechRequest,
  VoiceToken,
  Whoami,
} from "./api";
import { Etags, needsIfMatch, readPathOf } from "./if-match";
import type { SearchQuery, SearchResponse } from "./search";

export type Fetch = (input: Request | string | URL, init?: RequestInit) => Promise<Response>;


export interface CoreClientOptions {
  /** Core's base URL; empty string for same-origin calls from Nexus UI. */
  baseUrl: string;
  apiKey?: string;
  /** Extra headers on every call (Nexus UI's server: its shared secret and the Clerk ids). */
  headers?: Record<string, string>;
  fetch?: Fetch;
  /** Request timeout in milliseconds. */
  timeoutMs?: number;
  /**
   * Send an `Idempotency-Key` (a fresh UUID per call) on every write, and retry a write that may not have
   * reached Core, or that Core could not finish (a dropped connection, 502, 503, 504, or 409 `idempotency_in_progress`),
   * up to twice with the same key: Core runs it at most once. The `nexus` CLI turns this on.
   */
  idempotency?: boolean;
  /**
   * Send `If-Match` on the edits of an editable resource (`if-match.ts`): the `ETag` of the path's last read
   * by this client, read first when it never was. A caller's own `If-Match` header wins. The phone app turns this on.
   */
  ifMatch?: boolean;
  /** Sent as `x-nexus-client`, e.g. `cli/0.1.0`, so Core's audit log records the surface. */
  client?: string;
}

export class CoreError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly code?: string,
    /**
     * `slow_down` from `POST /v1/auth/token`: the interval to poll at from now on, in seconds. A refused download's
     * `Retry-After`, in seconds (interfaces' raw.ts `bytes()`), and a 503 `cutover_paused`'s.
     */
    readonly interval?: number,
    /** Core's `error.nexus`: a refused model's rule, a chat refusal's session and stored message. */
    readonly details?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "CoreError";
  }
}

/**
 * The code of a 503 on sign-in or an organization write while the operator pauses sign-in for the auth cutover
 * (`NEXUS_AUTH_CUTOVER_PAUSE`). Nothing changed; a write is not retried on it.
 */
export const CUTOVER_PAUSED = "cutover_paused";

/** A paused sign-in as a person reads it: the wait is `Retry-After`'s seconds, rounded up to whole minutes. */
export function cutoverPausedMessage(retryAfterSeconds?: number): string {
  const minutes = retryAfterSeconds && retryAfterSeconds > 0 ? Math.ceil(retryAfterSeconds / 60) : undefined;
  const wait = minutes === undefined ? "a few minutes" : minutes === 1 ? "1 minute" : `${minutes} minutes`;
  return `Sign-in is paused for maintenance; try again in ${wait}.`;
}

/** An exported file whose body broke off after Gateway's headers: what arrived is not the whole file. */
export class ExportCutError extends Error {
  /** Cut by this client's own time limit, not by the connection. */
  readonly timedOut: boolean;
  constructor(
    readonly path: string,
    cause?: unknown,
  ) {
    const timedOut = cause instanceof Error && cause.name === "TimeoutError";
    super(`The file from ${path} is cut: ${timedOut ? "the time limit ran out" : "the connection closed"} before the end of its body.`, { cause });
    this.timedOut = timedOut;
    this.name = "ExportCutError";
  }
}

export function createCoreClient(options: CoreClientOptions) {
  const doFetch: Fetch = options.fetch ?? ((input, init) => fetch(input, init));
  const base = options.baseUrl.replace(/\/$/, "");
  const requestTimeoutMs = options.timeoutMs ?? 120_000;
  const etags = options.ifMatch ? new Etags() : undefined;

  /**
   * One HTTP exchange; with `idempotency`, a write carries a key and is retried safely. `timeoutMs` bounds the whole
   * exchange, the body's read included (default: the client's, else 120 s). A caller's `signal` replaces that limit, as
   * `streamChatMessage`'s does. With `ifMatch`, an edit of an editable resource carries its version.
   */
  async function send(
    method: string,
    path: string,
    body?: unknown,
    requestId?: string,
    timeoutMs?: number,
    signal?: AbortSignal,
    extraHeaders?: Record<string, string>,
  ): Promise<Response> {
    const headers: Record<string, string> = { ...options.headers, ...extraHeaders };
    if (options.apiKey) headers.authorization = `Bearer ${options.apiKey}`;
    if (options.client) headers["x-nexus-client"] = options.client;
    // Bytes (an upload) go as they are, typed by the caller's `content-type`; anything else is JSON.
    const raw = body instanceof Uint8Array;
    if (body !== undefined && !raw) headers["content-type"] = "application/json";
    const write = method !== "GET" && method !== "HEAD";
    if (write && (requestId || options.idempotency)) headers["idempotency-key"] = requestId?.trim() || newIdempotencyKey();
    if (etags && needsIfMatch(method, path) && !Object.keys(headers).some((name) => name.toLowerCase() === "if-match")) {
      // Never read by this client: read it now, so the edit names a version. A failed read leaves the edit without one.
      if (etags.get(path) === undefined) await (await send("GET", readPathOf(path), undefined, undefined, timeoutMs, signal)).text().catch(() => "");
      const version = etags.get(path);
      if (version) headers["if-match"] = version;
    }
    const attempts = write && options.idempotency ? RETRY_DELAYS_MS.length + 1 : 1;
    for (let attempt = 0; ; attempt++) {
      try {
        const response = await doFetch(`${base}${path}`, {
          method,
          headers,
          body: body === undefined ? undefined : raw ? (body as Uint8Array<ArrayBuffer>) : JSON.stringify(body),
          signal: signal ?? AbortSignal.timeout(timeoutMs ?? requestTimeoutMs),
        });
        if (attempt + 1 < attempts && (await retryable(response))) {
          await sleep(RETRY_DELAYS_MS[attempt] ?? 0);
          continue;
        }
        etags?.learn(method, path, response.status, response.headers.get("etag"));
        return response;
      } catch (error) {
        if (attempt + 1 >= attempts || !dropped(error)) throw error;
        await sleep(RETRY_DELAYS_MS[attempt] ?? 0);
      }
    }
  }

  async function call<T>(method: string, path: string, body?: unknown, requestId?: string, signal?: AbortSignal, headers?: Record<string, string>): Promise<T> {
    const response = await send(method, path, body, requestId, undefined, signal, headers);
    return read<T>(response);
  }

  /** A whole JSON answer: its body, or a `CoreError` with Gateway's status, message and code. */
  async function read<T>(response: Response): Promise<T> {
    const text = await response.text();
    const data: unknown = text ? JSON.parse(text) : {};
    if (!response.ok) {
      const error = (data as Partial<ApiError>).error as (ApiError["error"] & { nexus?: Record<string, unknown> }) | undefined;
      if (response.status === 503 && error?.code === CUTOVER_PAUSED) {
        const wait = Number(response.headers.get("retry-after"));
        const seconds = Number.isFinite(wait) && wait > 0 ? wait : undefined;
        throw new CoreError(503, cutoverPausedMessage(seconds), CUTOVER_PAUSED, seconds, error.nexus);
      }
      throw new CoreError(response.status, error?.message ?? `HTTP ${response.status}`, error?.code, (data as { interval?: number }).interval, error?.nexus);
    }
    return data as T;
  }

  /**
   * `POST /v1/chat/completions` with `stream: true`, read as it streams. `onChunk` gets each
   * `chat.completion.chunk`. It resolves with the whole answer, as `chatCompletion` gives it: the text joined, and the
   * last chunk's `usage` and `nexus`. A refusal or an error before the stream starts rejects as `chatCompletion` does,
   * and is retried the same way. An `error` event in the stream rejects with a `CoreError` (status 502, Gateway's
   * code), as does a stream that ends before its last chunk. Nothing is retried once the stream has started. A Gateway
   * that answers whole (JSON: a replayed key, or a gateway that does not stream) is read whole, with no chunks.
   * With `tools` (Rust gateway), the tool calls' pieces are joined by `index` into the answer's `tool_calls`.
   */
  async function streamChatCompletion(
    request: ToolChatCompletionRequest & { team?: string },
    onChunk: (chunk: ChatCompletionChunk) => void,
    signal?: AbortSignal,
    requestId?: string,
  ): Promise<ChatCompletionResponse> {
    const response = await send("POST", "/v1/chat/completions", { ...request, stream: true }, requestId, options.timeoutMs ?? STREAM_TIMEOUT_MS, signal);
    if (!response.ok || !response.body || !response.headers.get("content-type")?.includes("text/event-stream")) return read<ChatCompletionResponse>(response);
    let id = "";
    let created = 0;
    let model = "";
    let content = "";
    let finish = "stop";
    const calls: ChatToolCall[] = [];
    let last: ChatCompletionChunk | undefined;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { done, value } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      let at: number;
      while ((at = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        const data = block
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (!data || data === "[DONE]") continue;
        const parsed = JSON.parse(data) as ChatCompletionChunk & { error?: { message: string; code?: string } };
        if (parsed.error) throw new CoreError(502, parsed.error.message, parsed.error.code);
        id ||= parsed.id ?? "";
        created ||= parsed.created ?? 0;
        model ||= parsed.model ?? "";
        const choice = parsed.choices?.[0];
        if (choice?.delta.content) content += choice.delta.content;
        for (const piece of choice?.delta.tool_calls ?? []) {
          const joined = (calls[piece.index] ??= { id: "", type: "function", function: { name: "", arguments: "" } });
          if (piece.id) joined.id = piece.id;
          if (piece.function?.name) joined.function.name = piece.function.name;
          if (piece.function?.arguments) joined.function.arguments += piece.function.arguments;
        }
        if (choice?.finish_reason) finish = choice.finish_reason;
        if (parsed.nexus) last = parsed;
        onChunk(parsed);
      }
      if (done) break;
    }
    if (!last?.nexus) throw new CoreError(502, "the answer's stream ended before its last chunk");
    return {
      id,
      object: "chat.completion",
      created,
      model: last.model || model,
      choices: [{ index: 0, message: { role: "assistant", content, ...(calls.length ? { tool_calls: calls.filter(Boolean) } : {}) }, finish_reason: finish }],
      usage: last.usage ?? { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
      nexus: last.nexus,
    };
  }

  /** A query string from the set fields of `query`. */
  const search = (query: Record<string, string | number | undefined>) => {
    const params = new URLSearchParams(
      Object.entries(query)
        .filter((entry): entry is [string, string | number] => entry[1] !== undefined && entry[1] !== "")
        .map(([key, value]) => [key, String(value)]),
    ).toString();
    return params ? `?${params}` : "";
  };

  /**
   * One exported file, its text and the export's id (`x-nexus-export-id`), which finds its audit event.
   * Sent once: a body that breaks after the headers rejects with `ExportCutError`; trying again is a new export. A file
   * streams for up to 300 s, as a chat answer does (and as the Rust client's download).
   */
  async function download(path: string): Promise<ExportFile> {
    const response = await send("GET", path, undefined, undefined, options.timeoutMs ?? 300_000);
    let text: string;
    try {
      text = await response.text();
    } catch (error) {
      if (response.ok) throw new ExportCutError(path, error);
      throw error;
    }
    if (!response.ok) {
      const error = (JSON.parse(text || "{}") as Partial<ApiError>).error;
      throw new CoreError(response.status, error?.message ?? `HTTP ${response.status}`, error?.code);
    }
    return { body: text, exportId: response.headers.get("x-nexus-export-id") };
  }

  /**
   * Post a message and read the answer as it streams. `onChunk` gets every event (the first carries the stored
   * question, the deltas the text, the last the stored answer). Resolves with the session and the answer; a refusal
   * (403, 402) or a failure mid-way rejects with a `CoreError` (`details` names the session and stored message).
   * Sent once: a stream is never retried, though it carries an `Idempotency-Key` when the client sends them.
   */
  async function streamChatMessage(
    id: string,
    body: SendChatMessageRequest,
    onChunk: (chunk: ChatStreamChunk) => void,
    signal?: AbortSignal,
    requestId?: string,
  ): Promise<{ session: ChatSessionSummary; question: ChatSessionMessage; message: ChatSessionMessage }> {
    const headers: Record<string, string> = { ...options.headers, "content-type": "application/json", accept: "text/event-stream" };
    if (options.apiKey) headers.authorization = `Bearer ${options.apiKey}`;
    if (options.client) headers["x-nexus-client"] = options.client;
    if (requestId || options.idempotency) headers["idempotency-key"] = requestId?.trim() || newIdempotencyKey();
    const response = await doFetch(`${base}/v1/chat/sessions/${encodeURIComponent(id)}/messages`, {
      method: "POST",
      headers,
      body: JSON.stringify({ ...body, stream: true }),
      signal: signal ?? AbortSignal.timeout(options.timeoutMs ?? 300_000),
    });
    // The message bumps the session's version: its next edit reads it again.
    etags?.learn("POST", `/v1/chat/sessions/${encodeURIComponent(id)}/messages`, response.status, null);
    if (!response.ok || !response.body || !response.headers.get("content-type")?.includes("text/event-stream")) {
      const text = await response.text();
      const data = (text ? JSON.parse(text) : {}) as Partial<ApiError> & { error?: { nexus?: Record<string, unknown> } };
      throw new CoreError(response.status, data.error?.message ?? `HTTP ${response.status}`, data.error?.code, undefined, data.error?.nexus);
    }
    let session: ChatSessionSummary | undefined;
    let question: ChatSessionMessage | undefined;
    let message: ChatSessionMessage | undefined;
    for await (const parsed of chatEvents(response.body)) {
      if (parsed.nexus?.session) session = parsed.nexus.session;
      if (parsed.nexus?.question) question = parsed.nexus.question;
      if (parsed.nexus?.message) message = parsed.nexus.message;
      onChunk(parsed);
    }
    if (!session || !question || !message) throw new CoreError(502, "the answer's stream ended early; reload the session to see what was kept");
    return { session, question, message };
  }

  /**
   * Rust gateway: one guest chat round for a signed-out visitor, read as it streams. `guestId` is the
   * browser's guest id (a UUID it keeps); the body is the whole conversation and a Turnstile token. The events are
   * `streamChatMessage`'s, with no stored session or message: `onChunk` gets each. Resolves with the answer's text, its
   * model and the rounds after this one (`X-Nexus-Guest-Rounds-Used` and `-Limit`). A refusal rejects with a
   * `CoreError` before the stream (`guest_rounds_used`, `guest_address_limit`, `guest_chat_paused`, `turnstile_failed`,
   * `guest_request_invalid`, `guest_chat_not_configured`); an `error` event or a stream that ends before its last chunk
   * rejects with a 502. Sent once, never retried: a round may have counted.
   */
  async function streamGuestChatMessage(
    guestId: string,
    body: GuestChatRequest,
    onChunk: (chunk: ChatStreamChunk) => void,
    signal?: AbortSignal,
  ): Promise<{ content: string; model: string | null; roundsUsed: number | null; roundsLimit: number | null }> {
    const headers: Record<string, string> = { ...options.headers, "content-type": "application/json", accept: "text/event-stream", "x-nexus-guest": guestId };
    if (options.client) headers["x-nexus-client"] = options.client;
    const response = await doFetch(`${base}/v1/guest/chat/messages`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
      signal: signal ?? AbortSignal.timeout(options.timeoutMs ?? 300_000),
    });
    if (!response.ok || !response.body || !response.headers.get("content-type")?.includes("text/event-stream")) {
      await read<unknown>(response);
      throw new CoreError(502, "guest chat answered without an event stream");
    }
    const rounds = (name: string) => {
      const value = response.headers.get(name);
      return value === null || value === "" || Number.isNaN(Number(value)) ? null : Number(value);
    };
    let content = "";
    let model: string | null = null;
    let finished = false;
    for await (const parsed of chatEvents(response.body)) {
      const choice = parsed.choices?.[0];
      if (choice?.delta.content) content += choice.delta.content;
      model ??= parsed.model;
      if (choice?.finish_reason) finished = true;
      onChunk(parsed);
    }
    if (!finished) throw new CoreError(502, "the answer's stream ended before its last chunk");
    return { content, model, roundsUsed: rounds("x-nexus-guest-rounds-used"), roundsLimit: rounds("x-nexus-guest-rounds-limit") };
  }

  /**
   * Follows a synced thread's live stream from `after`, the last event number you hold. `onAdvance` gets
   * each `thread` event (no body: read the events after your last with `syncedEvents`). Resolves `"deleted"` after the
   * `deleted` event, and `"closed"` when the stream ends otherwise; reconnect then with your new last. A refusal
   * (404, 410, 429, 503) rejects with a `CoreError`. `onPresence` gets each watcher's `presence` event (ADR 0011, M11).
   */
  async function streamSyncedThread(
    threadId: string,
    after: number,
    onAdvance: (event: SyncedThreadAdvanced) => void,
    signal?: AbortSignal,
    onOpen?: () => void,
    onPresence?: (event: SyncedThreadPresence) => void,
    /**
     * Phone control, for the thread's person only: given, the stream asks for them (`commands=true`). `live`
     * on the `commands` event, which says the stream carries the thread's command receipts (an older Gateway sends
     * none), and `command` for each new state.
     */
    onCommands?: { live?: () => void; command?: (event: CodeCommandChanged) => void },
  ): Promise<"deleted" | "closed"> {
    const query = { after, commands: onCommands ? "true" : undefined };
    const response = await send("GET", `/v1/threads/${encodeURIComponent(threadId)}/stream${search(query)}`, undefined, undefined, undefined, signal ?? new AbortController().signal, {
      accept: "text/event-stream",
    });
    if (!response.ok) await read<never>(response);
    if (!response.body || !response.headers.get("content-type")?.includes("text/event-stream")) throw new CoreError(502, "the thread's stream answered without an event stream");
    onOpen?.();
    const deleted = await eachEvent(response.body, (type, data) => {
      if (type === "deleted") return true;
      if (type === "thread" && data) onAdvance(JSON.parse(data) as SyncedThreadAdvanced);
      if (type === "presence" && data) onPresence?.(JSON.parse(data) as SyncedThreadPresence);
      if (type === "commands") onCommands?.live?.();
      if (type === "command" && data) onCommands?.command?.(JSON.parse(data) as CodeCommandChanged);
      return false;
    });
    return deleted ? "deleted" : "closed";
  }

  /**
   * Rust gateway only: your computer requests' and Code commands' new states, live. `request` and `command`
   * carry ids and state only: read the receipt then. Resolves when the stream ends; reconnect then, and read your
   * pending receipts again, since an event sent while the stream was down is lost. A refusal (403, 404 from an older
   * Gateway, 429, 503) rejects with a `CoreError`.
   */
  async function streamComputerEvents(
    on: { request?: (event: ComputerRequestChanged) => void; command?: (event: CodeCommandChanged) => void },
    signal?: AbortSignal,
    onOpen?: () => void,
  ): Promise<"closed"> {
    const response = await send("GET", "/v1/computers/stream", undefined, undefined, undefined, signal ?? new AbortController().signal, {
      accept: "text/event-stream",
    });
    if (!response.ok) await read<never>(response);
    if (!response.body || !response.headers.get("content-type")?.includes("text/event-stream")) throw new CoreError(502, "the computers' stream answered without an event stream");
    onOpen?.();
    await eachEvent(response.body, (type, data) => {
      if (type === "request" && data) on.request?.(JSON.parse(data) as ComputerRequestChanged);
      if (type === "command" && data) on.command?.(JSON.parse(data) as CodeCommandChanged);
      return false;
    });
    return "closed";
  }

  return {
    /**
     * One typed request with this client's key, headers, retries and versions, as every method below sends it: for an
     * operation this client has no method for.
     */
    request: call,
    apps: () => call<AppList>("GET", "/v1/apps"),
    app: (id: string) => call<App>("GET", `/v1/apps/${encodeURIComponent(id)}`),
    setAppCap: (id: string, body: SetAppCapRequest) => call<App>("PUT", `/v1/apps/${encodeURIComponent(id)}/cap`, body),
    deployApp: (body: DeployAppRequest) => call<App>("POST", "/v1/apps", body),
    deleteApp: (id: string) => call<AppDeleted>("DELETE", `/v1/apps/${encodeURIComponent(id)}`),
    claimApp: (id: string) => call<AppClaim>("POST", `/v1/apps/${encodeURIComponent(id)}/claim`, {}),
    /** an internal decision: chat sessions, your own only (private to you), most recently active first. */
    chatSessions: (query: ChatSessionsQuery = {}, signal?: AbortSignal) => call<ChatSessionsResponse>("GET", `/v1/chat/sessions${search({ ...query })}`, undefined, undefined, signal),
    chatSession: (id: string, signal?: AbortSignal) => call<ChatSessionDetail>("GET", `/v1/chat/sessions/${encodeURIComponent(id)}`, undefined, undefined, signal),
    /**
     * A caller-owned requestId reconciles explicit retries after a lost creation response. A retry sends the same body;
     * a blank requestId gets a fresh key.
     */
    createChatSession: (body: CreateChatSessionRequest = {}, requestId?: string) => call<ChatSessionSummary>("POST", "/v1/chat/sessions", body, requestId),
    /** Rename (`title`), archive or restore (`archived`), or change the next messages' model, mode, web search switch (`webSearch`, Rust gateway only) or team. */
    updateChatSession: (id: string, changes: { title?: string; archived?: boolean; model?: string; mode?: "chat" | "ask"; webSearch?: boolean; team?: string | null }, signal?: AbortSignal) =>
      call<ChatSessionSummary>("PATCH", `/v1/chat/sessions/${encodeURIComponent(id)}`, changes, undefined, signal),
    deleteChatSession: (id: string) => call<{ deleted: boolean; id: string; messages: number }>("DELETE", `/v1/chat/sessions/${encodeURIComponent(id)}`),
    /** Rewind before a user message. Original messages remain saved and marked as rewound. Rust gateway only. */
    rewindChatSession: (id: string, messageId: string) => call<ChatSessionDetail>("POST", `/v1/chat/sessions/${encodeURIComponent(id)}/rewind`, { messageId }),
    /** Rust Gateway: predict one next message from your completed Chat answer. This can incur a model charge. */
    suggestChatMessage: (id: string, signal?: AbortSignal) =>
      call<ChatSuggestion>("POST", `/v1/chat/sessions/${encodeURIComponent(id)}/suggest`, {}, undefined, signal),
    /** Post a message and wait for the whole answer (`stream: false`). */
    sendChatMessage: (id: string, body: SendChatMessageRequest) =>
      call<ChatMessageResult>("POST", `/v1/chat/sessions/${encodeURIComponent(id)}/messages`, { ...body, stream: false }),
    streamChatMessage,
    /** Rust gateway: a signed-out visitor's guest chat, by the browser's guest id: its rounds, whether it may send one, and the model's display name. */
    guestChatStatus: (guestId: string, signal?: AbortSignal) => call<GuestChatStatus>("GET", "/v1/guest/chat", undefined, undefined, signal, { "x-nexus-guest": guestId }),
    streamGuestChatMessage,
    /**
     * Rust gateway: Ask your code alone, and wait for the whole answer (`stream: false`). `requestId` is the
     * `Idempotency-Key`.
     */
    askCode: (body: AskCodeRequest, requestId?: string) => call<ChatMessageResult>("POST", "/v1/chat/ask", { ...body, stream: false }, requestId),
    /**
     * Rust gateway: web search alone, and wait for the whole answer (`stream: false`). `requestId` is the
     * `Idempotency-Key`.
     */
    askWeb: (body: AskWebRequest, requestId?: string) => call<ChatWebResult>("POST", "/v1/chat/web", { ...body, stream: false }, requestId),
    /** Rust gateway only: an image attached to a question in one of your sessions, with its bytes as base64. */
    chatAttachment: (sessionId: string, attachmentId: string) =>
      call<ChatAttachment>("GET", `/v1/chat/sessions/${encodeURIComponent(sessionId)}/attachments/${encodeURIComponent(attachmentId)}`),
    /** an internal decision: the organization's settings (chat retention); owners and admins change them. */
    orgSettings: () => call<OrgSettings>("GET", "/v1/orgs/settings"),
    setOrgSettings: (settings: { chatRetentionDays?: number | null; agentSubscriptions?: boolean | null; hiddenMobilePlaces?: OrgSettings["hiddenMobilePlaces"] | null } & Record<string, unknown>) => call<OrgSettings>("PUT", "/v1/orgs/settings", settings),
    /** Rust gateway: the organization's recovery key and its fingerprint, or `key: null`; every member reads it. */
    orgRecoveryKey: () => call<OrgRecoveryKeyState>("GET", "/v1/orgs/recovery-key"),
    /** an internal decision: chat counts (never content): the organization's for owners and admins, your own otherwise. */
    chatUsage: (days?: number) => call<ChatUsageReport>("GET", `/v1/analytics/chat${search({ days })}`),
    /** `GET /v1/analytics/agents` (Rust gateway only): agent adoption per day, by route: through Nexus Gateway or on their own subscription. */
    getAgentAdoption: (query: AgentAdoptionQuery = {}) => call<AgentAdoption>("GET", `/v1/analytics/agents${search({ ...query })}`),
    /** an internal decision: the audit log, newest first: everything (owners, admins), your teams' (leads) or your own actions. */
    audit: (query: AuditQuery = {}) => call<AuditResponse>("GET", `/v1/audit${search({ ...query })}`),
    /** The same events as CSV (owners, admins and leads), every page. */
    auditCsv: async (query: Omit<AuditQuery, "limit" | "cursor"> = {}) => {
      const response = await send("GET", `/v1/audit/export.csv${search({ ...query })}`);
      const text = await response.text();
      if (!response.ok) {
        const error = (JSON.parse(text || "{}") as Partial<ApiError>).error;
        throw new CoreError(response.status, error?.message ?? `HTTP ${response.status}`, error?.code);
      }
      return text;
    },
    createTenant: (name: string, email?: string) => call<CreateTenantResponse>("POST", "/v1/tenants", { name, email }),
    /** `GET /v1/tenants/me` */
    me: (signal?: AbortSignal) => call<CurrentTenantResponse>("GET", "/v1/tenants/me", undefined, undefined, signal),
    /** `GET /v1/auth/whoami` (Rust gateway): who the calling credential is; any valid credential may ask. */
    whoami: () => call<Whoami>("GET", "/v1/auth/whoami"),
    /**
     * `nexus auth login`: start a device login, then poll for its token. With `scope`, the approval issues an
     * agent link (`nxo_…`) that holds only those matrix actions, never the person's own token.
     */
    startDeviceLogin: (client?: string, scope?: DeviceScope) =>
      call<DeviceCodeResponse>("POST", "/v1/auth/device", { client, ...(scope ? { agent: scope.agent, scope: scope.scope } : {}) }),
    pollDeviceLogin: (deviceCode: string) =>
      call<DeviceTokenResponse>("POST", "/v1/auth/token", { device_code: deviceCode, grant_type: "urn:ietf:params:oauth:grant-type:device_code" }),
    exchangeAppCode: (code: string, codeVerifier: string, redirectUri: string) =>
      call<DeviceTokenResponse>("POST", "/v1/auth/token", { grant_type: "authorization_code", code, code_verifier: codeVerifier, redirect_uri: redirectUri }),
    /** Revoke the calling token. */
    logout: () => call<{ revoked: boolean }>("POST", "/v1/auth/logout", {}),
    /** A short-lived token to dictate with (503 `voice_not_configured` when this Gateway has no speech key). */
    createVoiceToken: () => call<VoiceToken>("POST", "/v1/chat/voice-token"),
    /** A piece of a voice-mode answer, said by the Gateway's ElevenLabs voice: MP3 bytes. A `CoreError` 503
     * `voice_speech_not_configured` means the Gateway has no key. `signal` stops it when the speaker stops. */
    createVoiceSpeech: async (text: string, signal?: AbortSignal): Promise<ArrayBuffer> => {
      const response = await send("POST", "/v1/chat/voice-speech", { text } satisfies VoiceSpeechRequest, undefined, undefined, signal);
      if (!response.ok) await read<never>(response);
      return response.arrayBuffer();
    },
    /** Rust only: the app's own error and crash reports, 1 to 20, each one a line in the Gateway's log. */
    sendDiagnosticReports: (reports: DiagnosticReport[]) => call<{ accepted: number }>("POST", "/v1/diagnostics/reports", { reports }),
    /** Rust only: feedback, or a crash the person chose to send, posted to Nexus's feedback channel. */
    sendFeedback: (feedback: Feedback) => call<{ sent: true }>("POST", "/v1/diagnostics/feedback", feedback),
    tokens: (all = false) => call<{ tokens: TokenSummary[] }>("GET", `/v1/auth/tokens${all ? "?all=1" : ""}`),
    /** A personal API key for CI, scripts and the API; the key is in the answer once. */
    createToken: (label: string) => call<TokenSummary & { token: string }>("POST", "/v1/auth/tokens", { label }),
    revokeToken: (id: string) => call<{ revoked: boolean }>("DELETE", `/v1/auth/tokens/${encodeURIComponent(id)}`),
    /**
     * The key directory: your devices for end-to-end encrypted session sync. `pairing.ts` builds
     * the signatures; these only carry them.
     */
    e2eDevices: (signal?: AbortSignal) => call<DeviceList>("GET", "/v1/devices", undefined, undefined, signal),
    registerE2eDevice: (body: RegisterDeviceRequest, signal?: AbortSignal) => call<E2eDevice>("POST", "/v1/devices", body, undefined, signal),
    /** The way back's first step: a challenge sealed to each of your trusted recovery keys. */
    recoverE2eChallenge: () => call<RecoveryChallenge>("POST", "/v1/devices/recover/challenge", {}),
    /** The way back: with a recovery key's proof, revokes your devices and registers this one as a new root. */
    recoverE2eDevice: (body: RecoverDeviceRequest) => call<E2eDevice>("POST", "/v1/devices/recover", body),
    approveE2eDevice: (id: string, body: ApproveDeviceRequest) => call<E2eDevice>("POST", `/v1/devices/${encodeURIComponent(id)}/approve`, body),
    revokeE2eDevice: (id: string, body: RevokeDeviceRequest) => call<E2eDevice>("POST", `/v1/devices/${encodeURIComponent(id)}/revoke`, body),
    /** The team key's newest epoch and `holder`'s copies of every epoch (ADR 0011, M5). */
    teamKey: (holder: string) => call<TeamKeyState>("GET", `/v1/orgs/team-key${search({ holder })}`),
    /** A team-visible thread's content key versions locked to the team key (M6). */
    teamThreadKeys: (threadId: string) => call<TeamThreadKeyList>("GET", `/v1/threads/${encodeURIComponent(threadId)}/team-keys`),
    /** A member's trusted holders, to verify a team copy's signer (M7). */
    memberKeyHolders: (userId: string) => call<TeamKeyHolderList>("GET", `/v1/orgs/members/${encodeURIComponent(userId)}/holders`),
    /** A device's locked copies of thread content keys, a page at a time (`after` is the last page's `nextAfter`). */
    deviceThreadKeys: (id: string, query: { after?: string; limit?: number } = {}) =>
      call<ThreadKeyList>("GET", `/v1/devices/${encodeURIComponent(id)}/keys${search({ ...query })}`),
    /** The copy of a thread's content key for `device`: at `version`, or the newest the device holds. */
    threadKey: (threadId: string, device: string, version?: number) =>
      call<ThreadKey>("GET", `/v1/threads/${encodeURIComponent(threadId)}/keys${search({ device, version })}`),
    /** Stores copies of a thread's content key at one version, signed by your trusted device `senderDeviceId`. */
    putThreadKeys: (threadId: string, body: PutThreadKeysRequest) =>
      call<ThreadKeysStored>("PUT", `/v1/threads/${encodeURIComponent(threadId)}/keys`, body),
    /**
     * The session store: your synced threads, newest first. Rust gateway only. `scope: "team"` lists the
     * organization's team-visible threads instead, or one member's with `person` (ADR 0011, M10). `scope: "following"`
     * lists the team-visible ones you follow.
     */
    syncedThreads: (query: { before?: string; limit?: number; scope?: "team" | "following"; person?: string } = {}) => call<SyncedThreadList>("GET", `/v1/threads${search({ ...query })}`),
    activeSyncedThreads: (query: { cursor?: string; limit?: number; scope?: "team" } = {}, signal?: AbortSignal) =>
      call<SyncedThreadList & { order: "activity"; nextCursor: string | null }>("GET", `/v1/threads${search({ ...query, order: "activity" })}`, undefined, undefined, signal),
    syncedThread: (threadId: string) => call<SyncedThread>("GET", `/v1/threads/${encodeURIComponent(threadId)}`),
    deleteSyncedThread: (threadId: string) =>
      call<SyncedThreadDeleted>("DELETE", `/v1/threads/${encodeURIComponent(threadId)}`),
    /** A page of sealed events in number order: after `after` (-1 for all), or ending just before `before`. */
    syncedEvents: (threadId: string, query: { after?: number; before?: number; limit?: number } = {}) =>
      call<SyncedEventPage>("GET", `/v1/threads/${encodeURIComponent(threadId)}/events${search({ ...query })}`),
    streamSyncedThread,
    sessionPins: (signal?: AbortSignal) =>
      call<SessionPinList>("GET", "/v1/session-pins", undefined, undefined, signal),
    setSessionPin: (source: SessionPinSource, id: string, request: SetSessionPin, signal?: AbortSignal) =>
      call<SessionPin>("PUT", `/v1/session-pins/${encodeURIComponent(source)}/${encodeURIComponent(id)}`, request, undefined, signal),
    threadAnswerRatings: (threadId: string, before?: number, signal?: AbortSignal) =>
      call<ThreadAnswerRatings>("GET", `/v1/threads/${encodeURIComponent(threadId)}/answer-ratings${search({ before })}`, undefined, undefined, signal),
    orgThreadAnswerRatings: (threadId: string, before?: number, signal?: AbortSignal) =>
      call<ThreadAnswerRatings>("GET", `/v1/threads/${encodeURIComponent(threadId)}/org-answer-ratings${search({ before })}`, undefined, undefined, signal),
    setThreadAnswerRating: (threadId: string, answerSeq: number, body: SetThreadAnswerRating, signal?: AbortSignal) =>
      call<ThreadAnswerRating>("PUT", `/v1/threads/${encodeURIComponent(threadId)}/answer-ratings/${encodeURIComponent(answerSeq)}`, body, undefined, signal),
    /** Your cloud agent sessions, newest first. Answers 404 when `NEXUS_SESSIONS_API` is off. */
    cloudSessions: (signal?: AbortSignal) => call<SessionList>("GET", "/v1/sessions", undefined, undefined, signal),
    /** Start a cloud coding agent session on a GitHub repository. Always sends `Idempotency-Key`. */
    createCloudSession: (body: CreateSession, options: { idempotencyKey?: string; githubToken?: string; githubAccount?: string; signal?: AbortSignal } = {}) =>
      call<Session>(
        "POST",
        "/v1/sessions",
        body,
        options.idempotencyKey ?? newIdempotencyKey(),
        options.signal,
        {
          ...(options.githubToken ? { "x-nexus-github-token": options.githubToken } : {}),
          ...(options.githubAccount ? { "x-nexus-github-account": options.githubAccount } : {}),
        },
      ),
    /** One cloud agent session and its cloud machine state. */
    cloudSession: (id: string, signal?: AbortSignal) =>
      call<Session>("GET", `/v1/sessions/${encodeURIComponent(id)}`, undefined, undefined, signal),
    /** Send a follow-up message to a cloud agent session (wakes a sleeping machine). */
    sendCloudSessionMessage: (id: string, body: SessionMessage, idempotencyKey?: string, signal?: AbortSignal) =>
      call<Session>("POST", `/v1/sessions/${encodeURIComponent(id)}/messages`, body, idempotencyKey ?? newIdempotencyKey(), signal),
    /** Stop a running or starting cloud agent session. */
    stopCloudSession: (id: string, idempotencyKey?: string, signal?: AbortSignal) =>
      call<Session>("POST", `/v1/sessions/${encodeURIComponent(id)}/stop`, undefined, idempotencyKey ?? newIdempotencyKey(), signal),
    /** A page of stored workbench events from a cloud machine's history. */
    cloudHistoryEvents: (machineId: string, threadId: string, query: { after?: number; before?: number; limit?: number } = {}, signal?: AbortSignal) =>
      call<CloudHistoryEventPage>(
        "GET",
        `/v1/cloud/machines/${encodeURIComponent(machineId)}/history/${encodeURIComponent(threadId)}/events${search({ ...query })}`,
        undefined,
        undefined,
        signal,
      ),
    /** ADR 0011, M2: keeps one of your threads from your teammates (`true`), or shares it with them again. */
    setSyncedThreadPrivate: (threadId: string, value: boolean) => call<SyncedThread>("PUT", `/v1/threads/${encodeURIComponent(threadId)}/private`, { private: value }),
    /** Follow a teammate's thread; the thread, with its followers. */
    followSyncedThread: (threadId: string) => call<SyncedThread>("PUT", `/v1/threads/${encodeURIComponent(threadId)}/follow`),
    /** Stop following a teammate's thread; the thread, with its followers. */
    unfollowSyncedThread: (threadId: string) => call<SyncedThread>("DELETE", `/v1/threads/${encodeURIComponent(threadId)}/follow`),
    /** ADR 0011, M11: your place on a thread you watch, relayed to its watchers as `presence`; `leave` when you stop. */
    postSyncedThreadPresence: (threadId: string, body: { client: string; seq?: number | null; leave?: boolean }) =>
      call<void>("POST", `/v1/threads/${encodeURIComponent(threadId)}/presence`, body),
    /** Phone control of your own synced Code sessions. An older Gateway answers 404. Records Gateway's clock offset. */
    codeControl: async (threadId: string, signal?: AbortSignal) => recordClockOffset(await call<CodeControl>("GET", `/v1/threads/${encodeURIComponent(threadId)}/control`, undefined, undefined, signal)),
    setCodeControl: async (threadId: string, body: CodeControlLease) => recordClockOffset(await call<CodeControl>("PUT", `/v1/threads/${encodeURIComponent(threadId)}/control`, body)),
    sendCodeCommand: (threadId: string, body: CodeCommandEnvelope, signal?: AbortSignal) => call<CodeCommand>("POST", `/v1/threads/${encodeURIComponent(threadId)}/commands`, body, body.id, signal),
    codeCommands: (threadId: string, runnerDeviceId: string) =>
      call<{ commands: CodeCommand[] }>("GET", `/v1/threads/${encodeURIComponent(threadId)}/commands${search({ runnerDeviceId })}`),
    codeRunnerCommands: (runnerDeviceId: string) => call<{ commands: CodeCommand[] }>("GET", `/v1/devices/${encodeURIComponent(runnerDeviceId)}/code-commands`),
    codeCommand: (threadId: string, id: string, signal?: AbortSignal) => call<CodeCommand>("GET", `/v1/threads/${encodeURIComponent(threadId)}/commands/${encodeURIComponent(id)}`, undefined, undefined, signal),
    claimCodeCommand: (threadId: string, id: string, body: CodeCommandClaim) =>
      call<{ claimed: boolean; command: CodeCommand }>("POST", `/v1/threads/${encodeURIComponent(threadId)}/commands/${encodeURIComponent(id)}/claim`, body),
    finishCodeCommand: (threadId: string, id: string, body: CodeCommandResult) =>
      call<CodeCommand>("POST", `/v1/threads/${encodeURIComponent(threadId)}/commands/${encodeURIComponent(id)}/result`, body),
    /** Takes back a command this device sent, before a computer claims it. An older Gateway answers 404. */
    cancelCodeCommand: (threadId: string, id: string, body: CodeCommandCancel, signal?: AbortSignal) =>
      call<CodeCommand>("POST", `/v1/threads/${encodeURIComponent(threadId)}/commands/${encodeURIComponent(id)}/cancel`, body, undefined, signal),
    computers: (signal?: AbortSignal) => call<ComputerList>("GET", "/v1/computers", undefined, undefined, signal),
    streamComputerEvents,
    publishComputerPresence: (computerId: string, body: ComputerPresence) =>
      call<ComputerPresence>("PUT", `/v1/computers/${encodeURIComponent(computerId)}/presence`, body),
    submitComputerRequest: (computerId: string, body: ComputerRequestEnvelope, signal?: AbortSignal) =>
      call<ComputerRequest>("POST", `/v1/computers/${encodeURIComponent(computerId)}/requests`, body, body.id, signal),
    computerRequestInbox: (computerId: string) =>
      call<{ requests: ComputerRequest[]; serverTime: number }>("GET", `/v1/computers/${encodeURIComponent(computerId)}/requests/inbox`),
    computerRequest: (computerId: string, id: string, signal?: AbortSignal) =>
      call<ComputerRequest>("GET", `/v1/computers/${encodeURIComponent(computerId)}/requests/${encodeURIComponent(id)}`, undefined, undefined, signal),
    claimComputerRequest: (computerId: string, id: string, body: ComputerRequestClaim) =>
      call<{ claimed: boolean; request: ComputerRequest }>("POST", `/v1/computers/${encodeURIComponent(computerId)}/requests/${encodeURIComponent(id)}/claim`, body),
    replyComputerRequest: (computerId: string, id: string, body: ComputerReplyEnvelope) =>
      call<ComputerRequest>("POST", `/v1/computers/${encodeURIComponent(computerId)}/requests/${encodeURIComponent(id)}/reply`, body),
    submitPhonePairingRequest: (inviterDeviceId: string, body: PhonePairingEnvelope, signal?: AbortSignal) =>
      call<PhonePairingRequest>("POST", `/v1/devices/${encodeURIComponent(inviterDeviceId)}/pairing-requests`, body, body.id, signal),
    // The pairing relay deduplicates the signed envelope by body.id. A fresh HTTP key avoids a cached pending receipt.
    phonePairingReceipt: (body: PhonePairingEnvelope, signal?: AbortSignal) =>
      call<PhonePairingRequest>("POST", `/v1/devices/${encodeURIComponent(body.inviterDeviceId)}/pairing-requests`, body, newIdempotencyKey(), signal),
    phonePairingRequests: (inviterDeviceId: string, signal?: AbortSignal) =>
      call<{ requests: PhonePairingRequest[] }>("GET", `/v1/devices/${encodeURIComponent(inviterDeviceId)}/pairing-requests`, undefined, undefined, signal),
    finishPhonePairingRequest: (inviterDeviceId: string, id: string) =>
      call<{ finished: boolean }>("POST", `/v1/devices/${encodeURIComponent(inviterDeviceId)}/pairing-requests/${encodeURIComponent(id)}/finish`, {}),
    /** Rust gateway only: your live agent links (`nxo_…`), never their tokens. */
    listAgentLinks: () => call<AgentLinkList>("GET", "/v1/agent-links"),
    /** End one of your agent links now; the agent's next call is refused. An ended link answers as it was. */
    unlinkAgentLink: (id: string) => call<AgentLink>("DELETE", `/v1/agent-links/${encodeURIComponent(id)}`),
    /** Rust gateway only: the OAuth clients you registered for your forks. 404 `oauth_client_tiers_off` while off. */
    listDeveloperClients: (signal?: AbortSignal) => call<DeveloperClientList>("GET", "/v1/developer/clients", undefined, undefined, signal),
    /** Register a fork: a 201 with its `client_id` (`id`). */
    registerDeveloperClient: (body: RegisterDeveloperClientRequest) => call<DeveloperClient>("POST", "/v1/developer/clients", body),
    /** Revoke one of your forks: its next sign-in gets 400 `invalid_client`. A revoked client answers as it was. */
    revokeDeveloperClient: (id: string, body: RevokeDeveloperClientRequest = {}) =>
      call<DeveloperClient>("POST", `/v1/developer/clients/${encodeURIComponent(id)}/revoke`, body),
    /** Rust gateway only: your personal agent's profile and every company agent's. */
    listAgentProfiles: (signal?: AbortSignal) => call<AgentProfileList>("GET", "/v1/agents/profiles", undefined, undefined, signal),
    /** Your personal agent's profile, or with `team` a company agent's (owners and admins). 409 when one exists. */
    createAgentProfile: (body: CreateAgentProfileRequest) => call<AgentProfile>("POST", "/v1/agents/profiles", body),
    /** Replaces a profile's fields but its owner (the personality fields, when left out, keep their values). */
    setAgentProfile: (id: string, body: SetAgentProfileRequest) => call<AgentProfile>("PUT", `/v1/agents/profiles/${encodeURIComponent(id)}`, body),
    members: () => call<MembersResponse>("GET", "/v1/orgs/members"),
    setMemberRole: (userId: string, role: Exclude<Role, "owner">) =>
      call<{ userId: string; role: Role }>("POST", `/v1/orgs/members/${encodeURIComponent(userId)}/role`, { role }),
    removeMember: (userId: string) => call<{ removed: boolean }>("DELETE", `/v1/orgs/members/${encodeURIComponent(userId)}`),
    removedMembers: () => call<RemovedMembersResponse>("GET", "/v1/orgs/members/removed"),
    /** Who in the organization is online now. */
    presence: () => call<PresenceList>("GET", "/v1/presence"),
    /** Your heartbeat while Nexus UI is open and visible, with what you are doing. */
    presenceHeartbeat: (activity?: ReportedActivity) => call<PresenceSettings>("POST", "/v1/presence", activity ? ({ activity } satisfies PresenceHeartbeat) : {}),
    presenceSettings: () => call<PresenceSettings>("GET", "/v1/presence/settings"),
    setPresenceSettings: (settings: PresenceSettings) => call<PresenceSettings>("PUT", "/v1/presence/settings", settings),
    recoverMember: (userId: string, options: { dryRun?: boolean } = {}) =>
      call<MemberRecovered>("POST", `/v1/orgs/members/${encodeURIComponent(userId)}/recover`, options),
    /**
     * Rust gateway only: delete your own account, from a sign-in (an API key is refused). Refused while you
     * own a company organization (409 `owns_organization`). A deletion needs `confirm: "DELETE"`, which the person
     * typed (400 `confirmation_required`). `dryRun` says what would go and changes nothing.
     */
    deleteAccount: (options: { dryRun?: boolean; confirm?: "DELETE" } = {}) => call<AccountDeleted>("POST", "/v1/auth/account/delete", options),
    /** Invite people to the organization (owners and admins); Clerk emails them. `dryRun` sends nothing. */
    invite: (body: InvitationsRequest) => call<InvitationsResponse>("POST", "/v1/orgs/invitations", body),
    /** Revoke a pending invitation by Clerk's id (owners and admins; the Rust gateway only). Audited. */
    revokeInvitation: (id: string) => call<{ revoked: boolean }>("DELETE", `/v1/orgs/invitations/${encodeURIComponent(id)}`),
    /** The pending invitations Core holds (`NEXUS_ORG_OWNER=core`; owners and admins). 409 `use_nexus_ui` while Clerk holds them. */
    invitations: () => call<PendingInvitationList>("GET", "/v1/orgs/invitations"),
    /** A new link for a Core invitation, emailed now; the old link stops working. */
    resendInvitation: (id: string) => call<InvitationResent>("POST", `/v1/orgs/invitations/${encodeURIComponent(id)}/resend`),
    /** The signed-in person joins the organization a Core invitation's link names (`token`); the answer is that organization. */
    acceptInvitation: (token: string) => call<OrgSummary>("POST", "/v1/orgs/invitations/accept", { token }),
    /** The signed-in person's own pending invitations, by the emails their sign-ins verified. 409 `use_nexus_ui` while Clerk holds them. */
    myInvitations: () => call<ReceivedInvitationList>("GET", "/v1/orgs/invitations/mine"),
    /** The signed-in person joins the organization of one of their own invitations, by its id from `myInvitations`. */
    acceptInvitationById: (id: string) => call<OrgSummary>("POST", "/v1/orgs/invitations/accept", { id }),
    transferOwnership: (userId: string) => call<{ previousOwner: string; owner: string }>("POST", "/v1/orgs/owner", { userId }),
    /** The organization's verified email domains (owners and admins; Clerk keeps them, the Rust gateway only). */
    domains: () => call<DomainList>("GET", "/v1/orgs/domains"),
    /** Add a domain the caller has a verified email on; Clerk invites people with a verified email there. */
    addDomain: (body: AddDomainRequest) => call<OrgDomain>("POST", "/v1/orgs/domains", body),
    removeDomain: (id: string) => call<{ removed: boolean }>("DELETE", `/v1/orgs/domains/${encodeURIComponent(id)}`),
    /** The company organizations the signed-in person may join by their Google sign-in's domain. 409 `use_nexus_ui` while Clerk holds them. */
    domainSuggestions: () => call<DomainSuggestionList>("GET", "/v1/orgs/domain-suggestions"),
    /** The signed-in person joins one of `domainSuggestions` as a member, by its organization's id. */
    joinByDomain: (org: string) => call<OrgSummary>("POST", "/v1/orgs/domain-suggestions/join", { org }),
    /**
     * The organization's budget (owners and admins): a cap per `period` on the Rust gateway; no period keeps the current one.
     * A string such as `"12.245"` goes as `budgetUsdDecimal`, which the Rust gateway reads exactly.
     */
    setBudget: (budgetUsd: number | string, period?: OrgBudget["period"]) =>
      call<OrgBudget>("POST", "/v1/orgs/budget", { ...budgetAmount(budgetUsd), ...(period ? { period } : {}) }),
    /** What `setBudget` would change, with the same checks; changes nothing (Rust gateway). */
    previewBudget: (budgetUsd: number | string, period?: OrgBudget["period"]) =>
      call<OrgBudgetChange>("POST", "/v1/orgs/budget", { ...budgetAmount(budgetUsd), ...(period ? { period } : {}), dryRun: true }),
    githubApp: () => call<GithubAppStatus>("GET", "/v1/github/app"),
    repositories: () => call<RepositoriesResponse>("GET", "/v1/github/repositories"),
    /**
     * `code`: the setup redirect's, when the App asks for user authorization on install (Gateway checks the installer).
     * `installerChecked` comes from the Rust gateway; the frozen TypeScript one does not send it.
     */
    linkInstallation: (installationId: string, state: string, code?: string) =>
      call<RepositoriesResponse & { installerChecked?: boolean }>("POST", "/v1/github/installations", {
        installationId,
        state,
        ...(code ? { code } : {}),
      }),
    unlinkInstallation: (installationId: string) =>
      call<{ removed: boolean }>("DELETE", `/v1/github/installations/${encodeURIComponent(installationId)}`),
    /** Bind a Slack channel of a workspace binding (`chw_…`) to a team, by slug or id. */
    bindSlackChannel: (workspaceId: string, channelId: string, team: string) =>
      call<ChatChannelBinding>("POST", `/v1/slack/workspaces/${encodeURIComponent(workspaceId)}/channels`, { channelId, team }),
    listSlackChannels: (workspaceId: string) =>
      call<ChatChannelList>("GET", `/v1/slack/workspaces/${encodeURIComponent(workspaceId)}/channels`),
    unbindSlackChannel: (workspaceId: string, channelId: string) =>
      call<ChatChannelUnbound>("DELETE", `/v1/slack/workspaces/${encodeURIComponent(workspaceId)}/channels/${encodeURIComponent(channelId)}`),
    /** A bound channel's effective settings, each with the level that set it. */
    slackChannelSettings: (workspaceId: string, channelId: string) =>
      call<ChatChannelSettings>("GET", `/v1/slack/workspaces/${encodeURIComponent(workspaceId)}/channels/${encodeURIComponent(channelId)}/settings`),
    /** The providers the Gateway calls, with the organization's key state for each BYOK one. */
    listProviders: () => call<ProviderList>("GET", "/v1/providers"),
    addProviderKey: (provider: string, key: string, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeySummary>("POST", "/v1/keys/providers", { provider, key, ...holder }),
    listProviderKeys: () =>
      call<{ providers: ProviderKeySummary[]; scopedKeys?: ProviderKeySummary[] }>("GET", "/v1/keys/providers"),
    /** One key, in any state, with its versions. */
    providerKey: (provider: string, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeySummary>("GET", `/v1/keys/providers/${encodeURIComponent(provider)}${search({ ...holder })}`),
    /** Replace, disable, enable or remove a stored key; `dryRun` checks and changes nothing. */
    replaceProviderKey: (provider: string, key: string, dryRun = false, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeyChange>("POST", `/v1/keys/providers/${encodeURIComponent(provider)}/replace`, { key, dryRun, ...holder }),
    disableProviderKey: (provider: string, dryRun = false, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeyChange>("POST", `/v1/keys/providers/${encodeURIComponent(provider)}/disable`, { dryRun, ...holder }),
    enableProviderKey: (provider: string, dryRun = false, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeyChange>("POST", `/v1/keys/providers/${encodeURIComponent(provider)}/enable`, { dryRun, ...holder }),
    removeProviderKey: (provider: string, dryRun = false, holder: ProviderKeyHolder = {}) =>
      call<ProviderKeyChange>("POST", `/v1/keys/providers/${encodeURIComponent(provider)}/remove`, { dryRun, ...holder }),
    /**
     * `team`: the team the call is attributed to (and whose policy applies). `signal` cancels it. A caller-owned
     * `requestId` is the `Idempotency-Key`, so a redelivered event runs and is charged once (the agent core).
     */
    chatCompletion: (request: ChatCompletionRequest & { team?: string }, signal?: AbortSignal, requestId?: string) =>
      call<ChatCompletionResponse>("POST", "/v1/chat/completions", request, requestId, signal),
    /**
     * Rust gateway: `chatCompletion` with tools (OpenAI's `tools`, `tool_calls` and `tool` turns), as the agent core
     * uses them. The answer's `tool_calls` are the calls the model asks for; nothing runs them here.
     */
    chatCompletionWithTools: (request: ToolChatCompletionRequest & { team?: string }, signal?: AbortSignal, requestId?: string) =>
      call<ChatCompletionResponse>("POST", "/v1/chat/completions", request, requestId, signal),
    streamChatCompletion,
    /** an internal decision: the models you may use; `all` the whole catalog with what you may use marked, `coming_soon` only those. */
    models: (options: { status?: "all" | "supported" | "coming_soon"; team?: string } = {}) =>
      call<ModelListResponse>("GET", `/v1/models${search({ status: options.status, team: options.team })}`),
    /** an internal decision: a level's model policy: the organization, a team (slug), or a coding agent. */
    policy: (level: "organization" | "team" | "agent", subject?: string) =>
      call<ModelPolicyView>("GET", policyPath(level, subject)),
    setPolicy: (level: "organization" | "team" | "agent", subject: string | undefined, rule: ModelRule) =>
      call<ModelPolicyView>("PUT", policyPath(level, subject), rule),
    /** an internal decision: would this person (or key) be allowed this model, and by which rule? The same decision Core enforces. */
    testPolicy: (request: PolicyTestRequest) => call<PolicyTestResponse>("POST", "/v1/policies/test", request),
    spend: () => call<SpendResponse>("GET", "/v1/spend"),
    /** Rust only, owners and admins: one page of the organization's model calls, newest first. */
    spendCalls: (query: SpendCallsQuery = {}) => call<SpendCallPage>("GET", `/v1/spend/calls${search({ ...query })}`),
    /** One of the organization's model calls, as its row in the files: what was reserved, charged and refused. */
    spendCall: (id: string) => call<SpendCall>("GET", `/v1/spend/calls/${encodeURIComponent(id)}`),
    /** Every matching call as a CSV or JSON Lines file. */
    spendCallsExport: (format: ExportFormat, query: SpendCallFilters = {}) => download(`/v1/spend/calls/export.${format}${search({ ...query })}`),
    /** The organization's usage as a file: one row per day, team, person, model, provider and product. */
    usageExport: (format: ExportFormat, query: UsageFilters = {}) => download(`/v1/analytics/usage/export.${format}${search({ ...query })}`),
    /** Every matching coding-agent session as a file. */
    agentSessionsExport: (format: ExportFormat, query: AgentSessionFilters = {}) => download(`/v1/agents/sessions/export.${format}${search({ ...query })}`),
    /** The directory file: each person's and team's id with its name. */
    directoryExport: (format: ExportFormat) => download(`/v1/orgs/directory/export.${format}`),
    /** Confirm this organization's exact Checkout Session or credit PaymentIntent. */
    billingOperation: (operationId: string) => call<BillingOperation>("GET", `/v1/billing/operations/${encodeURIComponent(operationId)}`),
    /** Rust gateway, owners: the card on file and the Stripe mode (the TypeScript gateway answers 404). */
    billing: () => call<BillingResponse>("GET", "/v1/billing"),
    /** Rust gateway, everyone: your referral code and the people who joined through it, a page at a time (a Gateway without it answers 404). */
    referrals: (cursor?: string) => call<ReferralList>("GET", `/v1/referrals${search({ cursor })}`),
    /** Rust gateway, owners: a Stripe Checkout link that saves a card; open `url` before `expiresAt`. */
    startPaymentMethodSetup: () => call<PaymentMethodSetupResponse>("POST", "/v1/billing/payment-method"),
    /** The same card setup as a form inside Nexus UI's billing page; Stripe's own page when Gateway can't. */
    startEmbeddedCardSetup: () => call<PaymentMethodSetupResponse | EmbeddedCardSetup>("POST", "/v1/billing/payment-method", { embedded: true }),
    /** 3-D Secure: the charge the bank wants the cardholder to confirm, confirmed again on session. 409 `nothing_to_confirm` when none waits. */
    confirmPayment: () => call<PaymentConfirmationResponse>("POST", "/v1/billing/confirm-payment"),
    /** A Stripe Checkout link that starts a plan: the same shape as the card setup's. */
    startPlan: (choice: PlanChoice) => call<PaymentMethodSetupResponse>("POST", "/v1/billing/plan", choice),
    /** Rust gateway, owners: a link to Stripe's customer portal (cards, invoices, billing details, tax ids); open `url` at once. */
    startBillingPortal: () => call<BillingPortalResponse>("POST", "/v1/billing/portal"),
    /** Rust gateway, owners: buy Nexus credits (`amountUsd` in whole cents, such as "50.00"); credited now, on its way, or a Checkout `url`. */
    buyCredits: (amountUsd: string, options: { paymentToken?: string; requestId?: string } = {}) =>
      call<BuyCreditsResponse>("POST", "/v1/billing/credits", { amountUsd, ...(options.paymentToken === undefined ? {} : { paymentToken: options.paymentToken }) }, options.requestId),
    /** Rust gateway, owners: turn auto top-up of Nexus credits on (with `belowUsd` and `addUsd`, or the ones set before) or off. */
    setAutoTopup: (body: { enabled: boolean; belowUsd?: string; addUsd?: string }) => call<SetAutoTopupResponse>("PUT", "/v1/billing/auto-topup", body),
    /** Rust gateway, owners: the plans this organization's kind can take, priced as Stripe holds them. */
    billingPlans: () => call<PlanCatalogue>("GET", "/v1/billing/plans"),
    /** Rust gateway, owners: change the plan, interval or seats; now when it costs more, at the period's end otherwise. */
    changePlan: (choice: PlanChoice & { dryRun?: boolean }) => call<PlanChange>("POST", "/v1/billing/plan/change", choice),
    /** Rust gateway, owners: cancel the plan change pending for the period's end. */
    cancelPlanChange: () => call<{ plan: BillingPlan | null }>("DELETE", "/v1/billing/plan/change"),
    /** Rust gateway, owners (pricing v2.0): turn paying after the fact on or off, or change its monthly usage limit (`limitUsd` in whole cents). */
    setAfterTheFact: (body: { enabled: boolean; limitUsd?: string }) => call<SetAfterTheFactResponse>("PUT", "/v1/billing/after-the-fact", body),
    /** Rust gateway, owners (pricing v2.0): every payment, newest first: credits, usage charged after the fact, invoiced months. */
    receipts: () => call<ReceiptsResponse>("GET", "/v1/billing/receipts"),
    /** Rust gateway, owners (pricing v2.0): Stripe's receipt page of one payment (`pi_…`). */
    receipt: (paymentId: string) => call<{ url: string }>("GET", `/v1/billing/receipts/${encodeURIComponent(paymentId)}`),
    /** Rust gateway, owners: a month's bill (`period` is `YYYY-MM`; default the current month): a stored month as it closed, else priced now. */
    billingStatement: (period?: string) =>
      call<BillingStatementResponse>("GET", `/v1/billing/statement${search({ period })}`),
    /** Rust gateway, owners: the stored months of Gateway's Stripe mode, newest first, at most 24. */
    billingStatements: () => call<BillingStatementsResponse>("GET", "/v1/billing/statements"),
    /** Rust gateway, owners and admins: start a Slack install; open `authorizeUrl` and approve the app in Slack. */
    startSlackInstall: () => call<SlackInstallStarted>("POST", "/v1/slack/installs"),
    /** Rust gateway: an install and its workspace, once it completed (404 `install_not_found` before). */
    slackInstall: (id: string) => call<SlackInstall>("GET", `/v1/slack/installs/${encodeURIComponent(id)}`),
    /**
     * Rust gateway: finish an install from Slack's redirect, as the signed-in person who started it (Nexus UI's install
     * page, with the redirect's `code` and `state`). Anyone else gets 403 `not_installer`. The binding is then pending.
     */
    completeSlackInstall: (id: string, request: CompleteSlackInstallRequest) =>
      call<SlackInstall>("POST", `/v1/slack/installs/${encodeURIComponent(id)}/complete`, request),
    /** Rust gateway: bind a pending install; only the person who started it, before its `state` expires. */
    confirmSlackInstall: (id: string) => call<SlackInstall>("POST", `/v1/slack/installs/${encodeURIComponent(id)}/confirm`),
    /** Rust gateway, owners and admins: the organization's pending and active Slack workspaces. */
    slackWorkspaces: () => call<ChatWorkspaceList>("GET", "/v1/slack/workspaces"),
    /** Rust gateway, owners and admins: unbind a workspace, or decline a pending install; either uninstalls the app. */
    disconnectSlackWorkspace: (id: string) => call<ChatWorkspaceRemoved>("DELETE", `/v1/slack/workspaces/${encodeURIComponent(id)}`),
    /** Rust gateway: your organizations, Personal first (the TypeScript gateway answers 404). */
    orgs: (signal?: AbortSignal) => call<{ orgs: OrgSummary[] }>("GET", "/v1/orgs", undefined, undefined, signal),
    orgWaitingCounts: (signal?: AbortSignal) => call<OrgWaitingCounts>("GET", "/v1/orgs/waiting", undefined, undefined, signal),
    /** Rust gateway, `NEXUS_ORG_OWNER=core`: a company organization Core alone holds, with you as its owner. */
    createOrg: (body: CreateOrgRequest) => call<OrgSummary>("POST", "/v1/orgs", body),
    /** Rust gateway, `NEXUS_ORG_OWNER=core`: rename this company organization, or remove its logo. */
    setOrgProfile: (body: OrgProfileChange) => call<OrgSummary>("PATCH", "/v1/orgs/profile", body),
    /**
     * Rust gateway, `NEXUS_ORG_OWNER=core`: this company organization's new logo, the image's bytes (PNG,
     * JPEG or WebP, 1 MB at most), stored in Nexus's bucket. 503 `logo_storage_unconfigured` without one.
     */
    setOrgLogo: (bytes: Uint8Array<ArrayBuffer>, type: string) => call<OrgSummary>("POST", "/v1/orgs/logo", bytes, undefined, undefined, { "content-type": type }),
    /** Rust gateway: a token for another of your organizations (a slug, an id, or `personal`). */
    switchOrg: (org: string) => call<SwitchedOrg>("POST", "/v1/orgs/switch", { org }),
    modules: () => call<ModulesResponse>("GET", "/v1/modules"),
    connectors: () => call<{ connectors: ConnectorSummary[] }>("GET", "/v1/connectors"),
    /**
     * `requestId` is the `Idempotency-Key`, so a write sent again runs once (the agent core's confirmed write).
     * `approval` is a chat agent's confirmed approval record for exactly this call, sent as `x-nexus-approval`.
     */
    connector: <T>(name: string, operation: string, params: Record<string, unknown>, requestId?: string, approval?: string) =>
      call<T>(
        "POST",
        `/v1/connectors/${segment(name)}/${segment(operation)}`,
        params,
        requestId,
        undefined,
        approval ? { "x-nexus-approval": approval } : undefined,
      ),
    /** Rust gateway, a chat agent's link only: Policy's record for one exact connector write. */
    createApproval: (body: CreateApproval) => call<Approval>("POST", "/v1/approvals", body),
    /** Rust gateway: the record, for the agent link that asked or for its person. */
    approval: (id: string) => call<Approval>("GET", `/v1/approvals/${encodeURIComponent(id)}`),
    /** Rust gateway, the record's person only (Nexus UI's approval page): confirm it, once. An agent link gets 403. */
    confirmApproval: (id: string) => call<Approval>("POST", `/v1/approvals/${encodeURIComponent(id)}/confirm`),
    /** Rust gateway, the record's person only: deny it, once. An agent link gets 403 and cancels instead. */
    denyApproval: (id: string) => call<Approval>("POST", `/v1/approvals/${encodeURIComponent(id)}/deny`),
    /** Rust gateway, the agent link that asked only: cancel its own pending record. Cancelling never confirms. */
    cancelApproval: (id: string) => call<Approval>("POST", `/v1/approvals/${encodeURIComponent(id)}/cancel`),
    /** Rust gateway: whether the organization lets a confirmed chat write run, and which operations. */
    chatWrites: () => call<ChatWriteSetting>("GET", "/v1/policies/chat-writes"),
    /** Rust gateway, `policy.manage`: turn chat writes on or off, and set the allowlist. */
    setChatWrites: (body: SetChatWrites) => call<ChatWriteSetting>("PUT", "/v1/policies/chat-writes", body),
    /** Rust gateway: whether your agents make the organization's allowed writes with no record per call. */
    agentWrites: () => call<AgentWriteSetting>("GET", "/v1/policies/agent-writes"),
    /** Rust gateway, `approvals.decide`: turn your own agents' writes on or off. Only you, never an agent. */
    setAgentWrites: (body: AgentWriteSetting) => call<AgentWriteSetting>("PUT", "/v1/policies/agent-writes", body),
    /** Rust gateway: one connector, with each operation's effect and request schema. */
    describeConnector: (name: string) => call<ConnectorDetail>("GET", `/v1/connectors/${segment(name)}`),
    /** Rust gateway: a connector read; Gateway refuses an operation that changes anything (403 `not_a_read`). */
    readConnector: <T>(name: string, operation: string, params: Record<string, unknown>) =>
      call<T>("POST", `/v1/connectors/${segment(name)}/${segment(operation)}/read`, params),
    /** Rust gateway: which connectors the organization's members may use. */
    connectorAvailability: () => call<{ connectors: ConnectorAvailability[] }>("GET", "/v1/policies/connectors"),
    /** Rust gateway, owners and admins: who may use a connector. */
    setConnectorAvailability: (name: string, body: SetConnectorAvailability) =>
      call<ConnectorAvailability>("PUT", `/v1/policies/connectors/${encodeURIComponent(name)}`, body),
    /** Rust gateway: your own connections and the organization's. */
    connections: () => call<ConnectionsResponse>("GET", "/v1/connections"),
    /** Connect an account with the connector's form fields; the vendor checks them first. `org` needs an owner or admin. */
    connect: (connector: string, fields: Record<string, string>, scope: ConnectionScope = "user") =>
      call<{ connections: Connection[] }>("POST", `/v1/connections/${encodeURIComponent(connector)}`, { ...fields, scope }),
    /** `account` is the account's id, name or host. */
    disconnect: (connector: string, account: string, scope: ConnectionScope = "user") =>
      call<DisconnectedResponse>("DELETE", `/v1/connections/${encodeURIComponent(connector)}/${encodeURIComponent(account)}${search({ scope: scope === "org" ? "org" : undefined })}`),
    /**
     * The vendor's authorize URL; 503 `not_configured` when Gateway has no OAuth app for it. `access: "write"`
     * only after a write answered 403 `write_consent_required`; without it the body is as before.
     */
    startOAuth: (connector: string, scope: ConnectionScope = "user", access?: "write") =>
      call<{ authorizeUrl: string }>("POST", `/v1/connections/${encodeURIComponent(connector)}/oauth`, access ? { scope, access } : { scope }),
    /** Only the person who started may finish: call it with their session. */
    finishOAuth: (request: OAuthFinishRequest) => call<OAuthFinishedResponse>("POST", "/v1/connections/oauth/finish", request),
    /** `GET /v1/agents/policy`: allowed and default models per coding agent (deprecated, see `policy`). */
    agentPolicy: () => call<AgentPolicyResponse>("GET", "/v1/agents/policy"),
    /**
     * An agent's allowed models and default, through `PUT /v1/policies/agents/{agent}`: the rest of the
     * agent's own rule stays, and its `defaults` holds this agent's only, as the deprecated `POST /v1/agents/policy` did.
     */
    setAgentPolicy: async (agent: AgentName, policy: AgentModelPolicy): Promise<{ agent: AgentName } & AgentModelPolicy> => {
      const own = await call<ModelPolicyView>("GET", `/v1/policies/agents/${encodeURIComponent(agent)}`);
      const rule: ModelRule = { ...own.rule, allow: policy.allowedModels, defaults: { [agent]: policy.defaultModel } };
      const saved = await call<ModelPolicyView>("PUT", `/v1/policies/agents/${encodeURIComponent(agent)}`, rule);
      return {
        agent,
        allowedModels: saved.rule.allow ?? policy.allowedModels,
        defaultModel: (saved.effective?.defaults as Partial<Record<string, { model?: string }>> | undefined)?.[agent]?.model ?? policy.defaultModel,
      };
    },
    /** an internal decision's decision for an agent (no model: the default it resolves to); `team` as for its calls. */
    checkAgent: (agent: AgentName, model?: string, team?: string) => call<AgentCheckResponse>("POST", "/v1/agents/check", { agent, model, ...(team ? { team } : {}) }),
    openAgentSession: (request: OpenAgentSessionRequest) => call<AgentSessionSummary>("POST", "/v1/agents/sessions", request),
    endAgentSession: (id: string, request: EndAgentSessionRequest) =>
      call<AgentSessionSummary>("POST", `/v1/agents/sessions/${encodeURIComponent(id)}/end`, request),
    /**
     * Rust gateway: the agent still runs. A running session with no heartbeat for 15 minutes reads as lost. One that never
     * sent a heartbeat (a CLI from before them) reads as lost 24 hours after it started. The TypeScript gateway returns 404.
     */
    heartbeatAgentSession: (id: string) => call<AgentSessionSummary>("POST", `/v1/agents/sessions/${encodeURIComponent(id)}/heartbeat`, {}),
    /**
     * A subscription session's usage as an OTLP logs export (`{ resourceLogs: [...] }`): Gateway counts each
     * `api_request` record once by its `request_id`. `signal` bounds the call, its transport retries included.
     */
    reportAgentSessionUsage: (id: string, logs: { resourceLogs: unknown[] }, signal?: AbortSignal) =>
      call<{ partialSuccess?: { rejectedLogRecords?: number; errorMessage?: string } }>("POST", `/v1/agents/sessions/${encodeURIComponent(id)}/usage`, logs, undefined, signal),
    agentSessions: () => call<{ sessions: AgentSessionSummary[] }>("GET", "/v1/agents/sessions"),
    /** One readable run. Caller cancellation and the client's request deadline both bound the lookup. */
    agentSession: (id: string, signal?: AbortSignal) =>
      call<AgentSessionSummary>("GET", `/v1/agents/sessions/${encodeURIComponent(id)}`, undefined, undefined,
        signal ? AbortSignal.any([signal, AbortSignal.timeout(requestTimeoutMs)]) : undefined),
    /** Rust gateway: your coding agents' routes, who pays on each, and the commands that set each agent up (404 on the TypeScript gateway). */
    agentAccess: () => call<AgentAccessResponse>("GET", "/v1/agents/access"),
    /** Rust gateway: save the route each agent takes when you name none; `null` forgets it. */
    setAgentAccess: (routes: Partial<Record<AgentName, AgentRoute | null>>) => call<AgentAccessResponse>("PUT", "/v1/agents/access", routes),
    openapi: () => call<Record<string, unknown>>("GET", "/v1/openapi.json"),
    /** Teams: list, one, and (owners and admins) create, rename, delete, add and remove members. */
    teams: () => call<TeamsResponse>("GET", "/v1/teams"),
    team: (slug: string) => call<TeamDetail>("GET", `/v1/teams/${encodeURIComponent(slug)}`),
    /** an internal decision: the teams nested under their parents, with leads and headcount. */
    teamTree: (depth?: number) => call<TeamTreeResponse>("GET", `/v1/teams?tree=1${depth ? `&depth=${depth}` : ""}`),
    createTeam: (team: { slug: string; name?: string; description?: string; parent?: string | null; level?: string }) => call<TeamDetail>("POST", "/v1/teams", team),
    /** Rename, relabel, or move (`parent`: a slug, or null for the top level). */
    updateTeam: (slug: string, changes: { slug?: string; name?: string; description?: string | null; parent?: string | null; level?: string | null }) =>
      call<TeamDetail>("PATCH", `/v1/teams/${encodeURIComponent(slug)}`, changes),
    /** `cascade` also deletes every team under it (else a team with child teams is refused). */
    deleteTeam: (slug: string, options: { cascade?: boolean } = {}) =>
      call<{ deleted: boolean; id: string; slug: string; teams: string[] }>("DELETE", `/v1/teams/${encodeURIComponent(slug)}${options.cascade ? "?cascade=1" : ""}`),
    /** By user id or email; an email not in the organization yet is pending until they join. */
    addTeamMember: (slug: string, person: { userId?: string; email?: string; role?: TeamRole; home?: boolean }) =>
      call<AddTeamMemberResponse>("POST", `/v1/teams/${encodeURIComponent(slug)}/members`, person),
    /** an internal decision: pending team members (owners and admins). */
    pendingTeamMembers: (team?: string) =>
      call<{ pending: PendingTeamMember[] }>("GET", `/v1/teams/pending${team ? `?team=${encodeURIComponent(team)}` : ""}`),
    /** The same, one row per person with all their teams, and whether Core can invite them. */
    pendingPeople: (team?: string) =>
      call<PendingPeopleResponse>("GET", `/v1/teams/pending?by=person${team ? `&team=${encodeURIComponent(team)}` : ""}`),
    cancelPendingTeamMembers: (which: { id?: string; email?: string; team?: string }) => {
      const params = new URLSearchParams(Object.entries(which).filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1] !== ""));
      return call<{ cancelled: PendingTeamMember[] }>("DELETE", `/v1/teams/pending?${params}`);
    },
    /** an internal decision: load the organization's teams from a file (owners and admins). */
    importTeams: (body: TeamImport) => call<TeamImportDiff>("POST", "/v1/teams/import", body),
    removeTeamMember: (slug: string, userId: string) =>
      call<{ removed: boolean }>("DELETE", `/v1/teams/${encodeURIComponent(slug)}/members/${encodeURIComponent(userId)}`),
    /** Your default team (`me`), or anyone's for owners and admins; `null` clears it. */
    setDefaultTeam: (team: string | null, userId = "me") =>
      call<{ userId: string; team: { id: string; slug: string; name: string } | null }>("PUT", `/v1/people/${encodeURIComponent(userId)}/default-team`, { team }),
    /** an internal decision team caps (Rust gateway): what the caller may see of one; the setter is for owners and admins. */
    teamBudget: (slug: string) => call<TeamBudget>("GET", `/v1/teams/${encodeURIComponent(slug)}/budget`),
    setTeamBudget: (slug: string, budget: BudgetSetting) => call<TeamBudgetChange>("PUT", `/v1/teams/${encodeURIComponent(slug)}/budget`, budget),
    /** A module capability's budget (owners and admins), such as Nexus Review's `robustness` / `review`. */
    productBudget: (product: string, capability: string) =>
      call<ProductBudget>("GET", `/v1/budgets/products/${encodeURIComponent(product)}/${encodeURIComponent(capability)}`),
    /** Set the budget as a decimal string in US dollars per `every`, or `null` to remove it. */
    setProductBudget: (product: string, capability: string, limitUsd: string | null, every: BudgetEvery = "month") =>
      call<ProductBudgetChange>("PUT", `/v1/budgets/products/${encodeURIComponent(product)}/${encodeURIComponent(capability)}`, limitUsd === null ? { limitUsd: null } : { limitUsd, every }),
    /** an internal decision person budgets: this month's limits; everyone's for owners and admins, your own for members. */
    budgets: () => call<BudgetsResponse>("GET", "/v1/budgets"),
    /**
     * The organization's default monthly limit per person in USD; `null` clears it (owners and admins).
     * A string such as `"12.245"` goes as `monthlyUsdDecimal`, which the Rust gateway reads exactly.
     */
    setPersonDefaultBudget: (monthlyUsd: number | string | null) =>
      call<BudgetsResponse>("PUT", "/v1/budgets/person-default", monthlyAmount(monthlyUsd)),
    /** One person's limit this month: yours (`me`), or anyone's for owners and admins. */
    personBudget: (userId = "me") => call<PersonBudget>("GET", `/v1/people/${encodeURIComponent(userId)}/budget`),
    /**
     * One person's override in USD; `null` removes it so the organization's default applies (an owner, or an admin an owner allowed; the owner's own limit is owner-only).
     * A string goes as `monthlyUsdDecimal`, read exactly.
     */
    setPersonBudget: (userId: string, monthlyUsd: number | string | null) =>
      call<PersonBudget>("PUT", `/v1/people/${encodeURIComponent(userId)}/budget`, monthlyAmount(monthlyUsd)),
    /** Owners: allow an admin to set person limits. Granting anyone but an admin answers 409 `not_an_admin`. */
    grantLimitManager: (userId: string) => call<LimitManagerChange>("PUT", `/v1/budgets/limit-managers/${encodeURIComponent(userId)}`),
    /** Owners: stop an admin setting person limits. */
    revokeLimitManager: (userId: string) => call<LimitManagerChange>("DELETE", `/v1/budgets/limit-managers/${encodeURIComponent(userId)}`),
    /** `GET /v1/analytics/usage`: totals, groups and a daily series. */
    usage: (query: UsageQuery = {}) => {
      const params = new URLSearchParams(Object.entries(query).filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1] !== ""));
      const search = params.toString();
      return call<UsageReport>("GET", `/v1/analytics/usage${search ? `?${search}` : ""}`);
    },
    /** `GET /v1/policies/grants` (`policy.read`): the organization's module write grants. Rust gateway only. */
    writeGrants: () => call<WriteGrantList>("GET", "/v1/policies/grants"),
    /** `PUT /v1/policies/grants/{product}/{capability}` (`policy.manage`): set the module's granted writes of one connector. */
    setWriteGrants: (product: string, capability: string, body: { connector: string; actions: string[] }) =>
      call<WriteGrantsSet>("PUT", `/v1/policies/grants/${encodeURIComponent(product)}/${encodeURIComponent(capability)}`, body),
    /** `GET /v1/policies/review-events` (`policy.read`): which review events modules may post. Rust gateway only. */
    reviewEvents: () => call<ReviewEventSetting>("GET", "/v1/policies/review-events"),
    /** `PUT /v1/policies/review-events` (`policy.manage`): choose whether modules may request changes and approve. */
    setReviewEvents: (level: ReviewEventLevel) => call<ReviewEventSetting>("PUT", "/v1/policies/review-events", { level }),
    /**
     * `GET /v1/robustness/review/status?repo=&pr=` (`products.use`): what Nexus Review will do for the repository, and
     * why. `null` when the backend predates the status contract (404 or 405): show the reduced view.
     */
    reviewStatus: async (repo: string, pr?: number): Promise<ReviewStatus | null> => {
      const params = new URLSearchParams({ repo });
      if (pr !== undefined) params.set("pr", String(pr));
      try {
        return await call<ReviewStatus>("GET", `/v1/robustness/review/status?${params}`);
      } catch (error) {
        if (error instanceof CoreError && (error.status === 404 || error.status === 405)) return null;
        throw error;
      }
    },
    /**
     * `POST /v1/robustness/review/status` (owners): evaluate a draft `config` without saving it. `null` on a backend
     * older than the status contract.
     */
    evaluateReviewStatus: async (body: { repo: string; config: Record<string, unknown>; pr?: number }): Promise<ReviewStatus | null> => {
      try {
        return await call<ReviewStatus>("POST", "/v1/robustness/review/status", body);
      } catch (error) {
        if (error instanceof CoreError && (error.status === 404 || error.status === 405)) return null;
        throw error;
      }
    },
    /** `POST /v1/{product}/{capability}/{verb}` */
    moduleVerb: (product: string, capability: string, verb: string, input: Record<string, string>) =>
      call<RunSummary & Record<string, unknown>>("POST", `/v1/${product}/${capability}/${verb}`, input),
    /** One page of a capability's runs. The paging and filter apply only where the manifest says `paged_reports`. */
    moduleReports: (product: string, capability: string, query: RunsQuery = {}) => {
      const params = new URLSearchParams();
      if (query.limit !== undefined) params.set("limit", String(query.limit));
      if (query.after) params.set("after", query.after);
      if (query.status?.length) params.set("status", query.status.join(","));
      const search = params.toString();
      return call<RunsPage>("GET", `/v1/${product}/${capability}/report${search ? `?${search}` : ""}`);
    },
    /** Nexus Wiki: one repository's runs as versions, newest first (`RunVersions`). */
    moduleVersions: (product: string, capability: string, repo: string) =>
      call<RunVersions>("GET", `/v1/${product}/${capability}/report?repo=${encodeURIComponent(repo)}`),
    /** The report's shape depends on the capability's panel kind: `RunReport` for `pages`. */
    moduleReport: <T extends RunSummary = RunReport>(product: string, capability: string, id: string) =>
      call<T>("GET", `/v1/${product}/${capability}/report/${encodeURIComponent(id)}`),
    /**
     * `GET /v1/search` (Rust gateway only): your own chats, artifacts shared with or by you, people, teams, models and
     * policies, grouped and ranked. `signal` cancels it (Nexus UI's palette drops a query you typed past).
     */
    search: async (query: SearchQuery, signal?: AbortSignal): Promise<SearchResponse> => {
      const headers: Record<string, string> = { ...options.headers };
      if (options.apiKey) headers.authorization = `Bearer ${options.apiKey}`;
      if (options.client) headers["x-nexus-client"] = options.client;
      const timeout = AbortSignal.timeout(options.timeoutMs ?? 120_000);
      const response = await doFetch(`${base}/v1/search${search({ q: query.q, types: query.types, limit: query.limit })}`, {
        method: "GET",
        headers,
        signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      });
      const text = await response.text();
      const data: unknown = text ? JSON.parse(text) : {};
      if (!response.ok) {
        const error = (data as Partial<ApiError>).error;
        throw new CoreError(response.status, error?.message ?? `HTTP ${response.status}`, error?.code);
      }
      return data as SearchResponse;
    },
  };
}

export type CoreClient = ReturnType<typeof createCoreClient>;
export type { SearchGroup, SearchQuery, SearchResponse, SearchResult, SearchSnippet, SearchType } from "./search";

/**
 * Hands each server-sent event of `body` to `on`, with its type (`message` when it names none) and its data lines.
 * `on` returns true to stop: the body is cancelled and this resolves true. Else it resolves false when the body ends.
 */
async function eachEvent(body: ReadableStream<Uint8Array>, on: (type: string, data: string) => boolean): Promise<boolean> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { done, value } = await reader.read();
    buffer += (done ? decoder.decode() : decoder.decode(value, { stream: true })).replace(/\r\n?/g, "\n");
    let at: number;
    while ((at = buffer.indexOf("\n\n")) >= 0) {
      const lines = buffer.slice(0, at).split("\n");
      buffer = buffer.slice(at + 2);
      const type = lines.find((line) => line.startsWith("event:"))?.slice(6).trim() ?? "message";
      const data = lines.filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trimStart()).join("\n");
      if (on(type, data)) {
        await reader.cancel().catch(() => undefined);
        return true;
      }
    }
    if (done) return false;
  }
}

/**
 * One path segment from a name, such as a connector's: encoded, and never empty, `.` or `..`, which `fetch` would
 * resolve to another route.
 */
function segment(value: string): string {
  if (value === "" || value === "." || value === "..") throw new TypeError(`not a path segment: "${value}"`);
  return encodeURIComponent(value);
}

function policyPath(level: "organization" | "team" | "agent", subject?: string): string {
  if (level === "organization") return "/v1/policies/organization";
  return `/v1/policies/${level === "team" ? "teams" : "agents"}/${encodeURIComponent(subject ?? "")}`;
}

/** The organization's budget as `POST /v1/orgs/budget` takes it: a number as `budgetUsd`, a decimal string as `budgetUsdDecimal`. */
function budgetAmount(budgetUsd: number | string): { budgetUsd: number } | { budgetUsdDecimal: string } {
  return typeof budgetUsd === "string" ? { budgetUsdDecimal: budgetUsd } : { budgetUsd };
}

/** A person limit as its `PUT` takes it: a number or `null` as `monthlyUsd`, a decimal string as `monthlyUsdDecimal`. */
function monthlyAmount(monthlyUsd: number | string | null): PersonLimitSetting {
  return typeof monthlyUsd === "string" ? { monthlyUsdDecimal: monthlyUsd } : { monthlyUsd };
}

/** Waits before the first and second retry of a write. */
const RETRY_DELAYS_MS = [250, 1_000];
/**
 * How long a streamed model call may run, its body included, unless the client sets `timeoutMs`. Gateway holds a
 * keyed stream to 14 minutes (`NEXUS_UPSTREAM_STREAM_TOTAL_MS`); a chat answer needs far less.
 */
const STREAM_TIMEOUT_MS = 600_000;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * A chat answer's server-sent events (`streamChatMessage`, `streamGuestChatMessage`), each `data:` parsed, until
 * `[DONE]` or the end. An `error` event throws a `CoreError` (status 502, Gateway's code, `details` its `nexus`).
 */
async function* chatEvents(body: ReadableStream<Uint8Array>): AsyncGenerator<ChatStreamChunk> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { done, value } = await reader.read();
    buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
    let at: number;
    while ((at = buffer.indexOf("\n\n")) >= 0) {
      const block = buffer.slice(0, at);
      buffer = buffer.slice(at + 2);
      const data = block
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trimStart())
        .join("\n");
      if (!data || data === "[DONE]") continue;
      const parsed = JSON.parse(data) as ChatStreamChunk & { error?: { message: string; code?: string; nexus?: Record<string, unknown> } };
      if (parsed.error) throw new CoreError(502, parsed.error.message, parsed.error.code, undefined, parsed.error.nexus);
      yield parsed;
    }
    if (done) break;
  }
}

/** A fresh idempotency key: a UUID where the runtime has `crypto.randomUUID`, else time and randomness (React Native). */
export function newIdempotencyKey(): string {
  const random = globalThis.crypto?.randomUUID?.();
  return random ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Worth retrying with the same key: Core or a proxy could not finish it, or the first try is still running. Not a 503
 * `cutover_paused`: the pause lasts minutes, and the person is told how long.
 */
async function retryable(response: Response): Promise<boolean> {
  if (response.status === 502 || response.status === 504) return true;
  if (response.status === 503) return !(await response.clone().text()).includes(CUTOVER_PAUSED);
  if (response.status !== 409) return false;
  const text = await response.clone().text();
  return text.includes("idempotency_in_progress");
}

/**
 * The connection dropped after the request may have been sent (reset, socket closed): a retry with the same key is
 * safe. A refused connection or an unknown host means Core never saw it and is not there; a timeout is the caller's.
 */
function dropped(error: unknown): boolean {
  const text = error instanceof Error ? `${error.message} ${String((error as { cause?: unknown }).cause ?? "")}` : String(error);
  if (/ECONNREFUSED|ENOTFOUND|EAI_AGAIN|timeout|aborted/i.test(text)) return false;
  return /ECONNRESET|socket hang up|other side closed|UND_ERR_SOCKET|fetch failed|network/i.test(text);
}
