/**
 * Nexus Artifacts (Rust gateway only): a chat session shared as an immutable snapshot, inside the organization (the
 * whole organization, chosen teams or chosen people) or outside it (named people by verified email, or anyone with the
 * link), under the organization's `artifacts.external*` settings. The types of openapi.json's `Artifact`,
 * `ArtifactDetail`, `ArtifactAudience`, `ArtifactLink`, `ChatSessionSharing`, `ArtifactAccess`,
 * `ExternalArtifactPage` and `ArtifactAttachment`, and a small client for their operations (kept apart from `core-client.ts` so the chat client
 * stays as the TypeScript gateway serves it).
 */
import type { ApiError, ChatAttachmentRef, ChatMode, ChatSessionMessage, ChatSessionSummary, OrgSummary } from "./api";
import { CoreError, newIdempotencyKey } from "./core-client";
import { Etags, needsIfMatch, readPathOf } from "./if-match";
import type { AskNexusOnPageBoardRequest, PageBoardAsk, PageBoardAsked, CreatePageCommentRequest, CreatePageRequest, PageComment, PageCommentAsked, PageComments, PageCommentsAsked, PageDetail, PageDoc, PageEvent, PageFile, PageFiles, PageFormat, PageFrame, PageList, PagePreviewEvent, PageRemixed, PageShareRole, PageStorageDeleted, PageStorageKeys, PageStorageScope, PageStorageValue, PageStorageWritten, PageVersion, PageWritten, PostPageDocRequest, SharePageRequest, SiteRoutes, WritePageVersionRequest, DesignSystem, DesignSystemDeleted, DesignSystemDetail, DesignSystemList, DesignSystemSaved, PageCheck, SaveDesignSystemRequest, PageQueryDeleted, PageQueryList, PageQueryResult, PageQuerySaved, SavePageQueryRequest } from "./pages";
import type {
  CreateSkillRequest,
  DecideSkillPromotionRequest,
  RequestSkillPromotionRequest,
  SkillDeleted,
  SkillDetail,
  SkillLibraryList,
  SkillList,
  SkillPromotion,
  SkillPromotionDetail,
  SkillPromotionList,
  SkillPromotionsQuery,
  SkillPromotionWithdrawn,
  SkillVersionDetail,
  SkillWritten,
  WithdrawSkillPromotionRequest,
  WriteSkillVersionRequest,
} from "./skills";
import { parseAskOwners, parseCollectedNotifications, parseNotificationDevice, parseNotificationDevices, parseNotificationItem, parseNotificationKinds, parseNotificationPage, parseNotificationPushKey, parseNotificationSettings, type BillingBlock, type NotificationDevicePlatform, type NotificationSettingChange, type NotificationsQuery } from "./notifications";
import type { SessionSnapshot, ShareAgentSessionRequest } from "./session-share";

/**
 * `POST /v1/artifacts/{id}/continue`: the new session and (Rust gateway) the organization it was made in, which pays
 * for it. `org.current` is false when that is another of your organizations than the one you called with;
 * `orgCreated` when it is your personal organization, made just now.
 */
export type ContinuedChatSession = ChatSessionSummary & ChatSessionState & { org?: OrgSummary; orgCreated?: boolean };

export type ArtifactAudienceKind = "org" | "teams" | "people" | "external" | "public";

/** The audiences outside the organization. */
export const EXTERNAL_AUDIENCES: ArtifactAudienceKind[] = ["external", "public"];

/**
 * Who may read an artifact besides its author. `teams` are slugs, `people` emails (members for `people`, the named
 * outsiders for `external`). Readers outside the organization see neither.
 */
export interface ArtifactAudience {
  kind: ArtifactAudienceKind;
  teams: string[];
  people: string[];
}

/** One version of a session's artifact, as the session lists it. */
export interface ArtifactLink {
  id: string;
  version: number;
  status: "active" | "revoked";
  audience: ArtifactAudienceKind;
  createdAt: string;
  revokedAt?: string;
}

/** How a shared session is shared: its newest active artifact. */
export interface ChatSessionSharing {
  artifactId: string;
  version: number;
  audience: ArtifactAudience;
  allowContinue: boolean;
  url: string;
  path: string;
  /** The fast viewer: `<origin>/a/<id>`, or `/a/<token>` for a public link. */
  viewerUrl?: string;
  publicUrl?: string;
  expiresAt?: string;
  includeCode?: boolean;
}

export interface ArtifactAuthor {
  email?: string;
  name?: string;
}

/** A session's state as the Rust gateway answers it (absent from the TypeScript gateway). */
export interface ChatSessionState {
  visibility?: "private" | "shared";
  sharing?: ChatSessionSharing;
  artifacts?: ArtifactLink[];
  author?: ArtifactAuthor;
  forkedFrom?: { artifactId: string; version: number; title?: string };
}

export interface Artifact {
  id: string;
  version: number;
  sessionId: string;
  title: string;
  mode: ChatMode;
  model: string;
  models: string[];
  author: ArtifactAuthor;
  audience: ArtifactAudience;
  allowContinue: boolean;
  status: "active" | "revoked";
  mine: boolean;
  latest?: boolean;
  messageCount: number;
  /** Hidden from readers outside the organization. */
  costUsd?: number;
  url: string;
  path: string;
  markdownUrl: string;
  /** The fast viewer for this artifact (`/a/<id>`; for a public link its author and admins get `publicUrl`). */
  viewerUrl?: string;
  /** Anyone with this link can read it (`/a/<token>`): public artifacts, for their author and the org's admins. */
  publicUrl?: string;
  /** A public link's expiry. */
  expiresAt?: string;
  /** Shared outside the organization (`external` or `public`). */
  external?: boolean;
  /** Whether Ask your code's sources are in the external snapshot (the organization must allow it). */
  includeCode?: boolean;
  /** The organization's name, shown to every reader. */
  organization?: string;
  createdAt: string;
  sessionCreatedAt?: string;
  revokedAt?: string;
  /**
   * `page` for a shared page: no messages and an empty `markdownUrl`. `session` for a shared coding-agent
   * session: messages derived from its timeline, which `ArtifactDetail.session` holds. Absent for a chat.
   */
  kind?: "page" | "session";
  pageId?: string;
  pageFormat?: PageFormat;
  /** The version the share pins; absent: the page's latest version. */
  pageVersion?: number;
  /** A shared page's role: `edit` when its readers may edit the page with its owner. */
  pageRole?: PageShareRole;
}

/** `member`: as the organization's members see it; `external`: cost, author email, team names and (unless allowed) code sources left out. */
export type ArtifactVariant = "member" | "external";

export interface ArtifactDetail extends Artifact {
  messages: ChatSessionMessage[];
  hiddenSources: number;
  versions?: ArtifactLink[];
  variant?: ArtifactVariant;
  /** A shared coding-agent session's timeline (kind `session`). */
  session?: SessionSnapshot;
}

/**
 * `GET /v1/artifacts/{id}/attachments/{attachmentId}` and its public link's twin: an image of an artifact's question
 * with its bytes. The snapshot's own copy, readable by everyone who may read the artifact.
 */
export interface ArtifactAttachment extends Omit<ChatAttachmentRef, "type"> {
  artifactId: string;
  messageId: string;
  /** Standard base64: `data:${mediaType};base64,${data}` shows it. */
  data: string;
  createdAt: string;
}

/** One person on an external artifact: granted, asking, denied or removed; and how often they opened it. */
export interface ArtifactAccessPerson {
  email: string;
  status: "granted" | "requested" | "denied" | "removed";
  opens: number;
  lastOpenedAt?: string;
  requestedAt?: string;
  grantedAt?: string;
  message?: string;
}

/** `GET|POST /v1/artifacts/{id}/access`: who has it and who asked; opens (named: per person; public: a count). */
export interface ArtifactAccess {
  id: string;
  audience: ArtifactAudienceKind;
  status: "active" | "revoked";
  people: ArtifactAccessPerson[];
  opens: number;
  lastOpenedAt?: string;
  reports: number;
  expiresAt?: string;
  publicUrl?: string;
}

/** An artifact in the admin view of everything shared outside. */
export interface ExternalArtifact extends Artifact {
  opens: number;
  readers: number;
  requests: number;
  reports: number;
  team?: string;
}

export interface ExternalArtifactPage {
  artifacts: ExternalArtifact[];
  nextCursor?: string;
  totals: { active: number; named: number; public: number };
}

export interface ExternalArtifactsQuery {
  audience?: "external" | "public";
  author?: string;
  team?: string;
  status?: "active" | "revoked" | "all";
  q?: string;
  limit?: number;
  cursor?: string;
}

/** The organization's settings for sharing outside it (an internal decision typed settings: limits set by owners and admins). */
export interface ArtifactSharingSettings {
  "artifacts.external": "off" | "named" | "public";
  "artifacts.external.code": boolean;
  "artifacts.external.domains": { allow: string[]; deny: string[] };
  "artifacts.external.expiry": { maxDays: number | null; required: boolean };
  "artifacts.external.continue": boolean;
  "artifacts.external.who": "everyone" | "admins";
  /** Whether people may make pages (Nexus Pages). Off keeps what exists readable by its owner. */
  "artifacts.pages": boolean;
  /** Whether pages may have public links. The default follows the organization's kind: off for a company, on for a personal one. */
  "artifacts.pages.public": boolean;
  /** Whether a page's owner may approve live queries on a connection for its readers (Nexus Data). Off by default. */
  "artifacts.pages.queries": boolean;
}

export const ARTIFACT_SETTING_KEYS = [
  "artifacts.external",
  "artifacts.external.code",
  "artifacts.external.domains",
  "artifacts.external.expiry",
  "artifacts.external.continue",
  "artifacts.external.who",
  "artifacts.pages",
  "artifacts.pages.public",
  "artifacts.pages.queries",
] as const;

export type ArtifactSettingKey = (typeof ARTIFACT_SETTING_KEYS)[number];

export const ARTIFACT_SETTING_DEFAULTS: ArtifactSharingSettings = {
  "artifacts.external": "public",
  "artifacts.external.code": false,
  "artifacts.external.domains": { allow: [], deny: [] },
  "artifacts.external.expiry": { maxDays: null, required: false },
  "artifacts.external.continue": true,
  "artifacts.external.who": "everyone",
  "artifacts.pages": true,
  // A company organization's default; a personal one answers true. Gateway's `defaults` is the source.
  "artifacts.pages.public": false,
  "artifacts.pages.queries": false,
};

/**
 * Whether `role` may share to `kind` under `settings`, and why not: the Share dialog greys out what the policy forbids,
 * with this reason. The Gateway checks the same rules (403 with these codes).
 */
export function externalShareAllowed(
  kind: ArtifactAudienceKind,
  settings: Partial<ArtifactSharingSettings> | undefined,
  role: string | undefined,
): { allowed: true } | { allowed: false; code: string; reason: string } {
  if (kind !== "external" && kind !== "public") return { allowed: true };
  const mode = settings?.["artifacts.external"] ?? ARTIFACT_SETTING_DEFAULTS["artifacts.external"];
  if (mode === "off") return { allowed: false, code: "external_sharing_off", reason: "Your organization has turned off sharing outside it." };
  if (kind === "public" && mode !== "public")
    return { allowed: false, code: "public_links_off", reason: "Your organization allows sharing with named people only, not public links." };
  const who = settings?.["artifacts.external.who"] ?? ARTIFACT_SETTING_DEFAULTS["artifacts.external.who"];
  if (who === "admins" && role !== "owner" && role !== "admin")
    return { allowed: false, code: "external_sharing_admins_only", reason: "Only owners and admins can share outside your organization." };
  return { allowed: true };
}

export interface ArtifactPage {
  artifacts: Artifact[];
  nextCursor?: string;
}

export type LibraryOrder = "recent" | "name" | "kind" | "created";

export interface ArtifactsQuery {
  order?: LibraryOrder;
  /** `shared_with_me` includes artifacts other organizations shared with your verified email. */
  scope?: "all" | "shared_with_me" | "shared_by_me";
  status?: "active" | "revoked" | "all";
  session?: string;
  q?: string;
  limit?: number;
  cursor?: string;
}

export interface ShareChatSessionRequest {
  audience: ArtifactAudienceKind;
  teams?: string[];
  /** Members' emails or ids (`people`); outsiders' emails (`external`). */
  people?: string[];
  allowContinue?: boolean;
  /** `external` and `public`: keep Ask your code's sources (only when the organization allows it). */
  includeCode?: boolean;
  /** `public`: `30d`, `12h` or a number of days. */
  expiresIn?: string | number;
  /** `public`: an ISO time. */
  expiresAt?: string;
  /** Revoke the session's earlier active versions in the same step, so the session has exactly this audience. */
  replace?: boolean;
}

/** `DELETE /v1/chat/sessions/{id}/share`: the session is private again, with the versions revoked now. */
export interface ChatSessionUnshared {
  sessionId: string;
  visibility: "private";
  revoked: { id: string; version: number }[];
}

/**
 * The share menu's three levels, as Claude's: only you, everyone in the organization, anyone with the link. Teams,
 * people and people outside by email are `custom` (the menu's "More options").
 */
export type ShareLevel = "private" | "org" | "public" | "custom";

/** A session's level from its newest active share (none: `private`). */
export function shareLevel(audience: { kind: ArtifactAudienceKind } | undefined): ShareLevel {
  if (!audience) return "private";
  if (audience.kind === "org" || audience.kind === "public") return audience.kind;
  return "custom";
}

/**
 * A shared link's key (`/a/<key>`): an artifact's id (`art_…`), read with the reader's identity, or else a public
 * link's token. Nexus UI's viewer and the phone app both decide it here, so they never disagree.
 */
export function isArtifactId(key: string): boolean {
  return /^art_[A-Za-z0-9_-]+$/.test(key);
}

export type ReportReason = "spam" | "phishing" | "malware" | "illegal" | "other";

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

/**
 * The events of a server-sent event stream as they arrive: each one's `event:` type (`message` when unset) and its
 * `data:` lines parsed as JSON. Comments (`: keep-alive`) are skipped. Resolves when the stream ends, or at once when
 * `signal` aborts it.
 */
export async function readEvents(body: ReadableStream<Uint8Array>, onEvent: (event: PageEvent) => void, signal?: AbortSignal): Promise<void> {
  const reader = body.getReader();
  const stop = () => void reader.cancel().catch(() => undefined);
  if (signal?.aborted) stop();
  signal?.addEventListener("abort", stop, { once: true });
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });
      let at: number;
      while ((at = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        let type = "message";
        const data: string[] = [];
        for (const line of block.split("\n")) {
          if (line.startsWith("event:")) type = line.slice(6).trim();
          else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
        }
        if (data.length) onEvent({ type, data: JSON.parse(data.join("\n")) as unknown });
      }
      if (done) return;
    }
  } finally {
    signal?.removeEventListener("abort", stop);
  }
}

export interface TrashedPageList {
  pages: TrashedPage[];
}

export interface TrashedPage {
  page: PageList["pages"][number];
  trashedAt: string;
  purgeAfter: string;
}

/** The files a page exports as (`GET /v1/pages/{id}/export`). */
export type PageExportFormat = "png" | "svg" | "pdf" | "pptx" | "html" | "zip";

/** A board that did not render, and why. */
export interface PageExportFailure {
  board: string;
  error: string;
}

/** An exported file: its bytes, its name, and the boards that did not render. */
export interface PageExport {
  blob: Blob;
  filename: string;
  failed: PageExportFailure[];
}

/** The boards an export did not render, from `x-nexus-export-failed` (percent-encoded JSON). */
export function exportFailures(header: string | null): PageExportFailure[] {
  if (!header) return [];
  try {
    const parsed = JSON.parse(decodeURIComponent(header)) as unknown;
    return Array.isArray(parsed) ? parsed.filter((entry): entry is PageExportFailure => typeof entry?.board === "string" && typeof entry?.error === "string") : [];
  } catch {
    return [];
  }
}

/** The file name in a `content-disposition` header, else `fallback`. */
export function dispositionName(header: string | null, fallback: string): string {
  return /filename="([^"]+)"/.exec(header ?? "")?.[1] ?? fallback;
}

/**
 * The artifacts operations, over the same credential and base URL as `createCoreClient`. `ifMatch`: as
 * `createCoreClient`'s, with a version memory of its own.
 */
export function createArtifactsClient(options: { baseUrl: string; apiKey?: string; fetch?: Fetch; ifMatch?: boolean }) {
  const doFetch: Fetch = options.fetch ?? ((input, init) => fetch(input, init));
  const base = options.baseUrl.replace(/\/$/, "");
  const id = (value: string) => encodeURIComponent(value);
  const etags = options.ifMatch ? new Etags() : undefined;

  /** `keyed`: a write carries an idempotency key, except one that is harmless to repeat (a draft's update). */
  async function send(method: string, path: string, body?: unknown, keyed = true, signal?: AbortSignal, keepalive?: boolean): Promise<Response> {
    const headers: Record<string, string> = {};
    if (options.apiKey) headers.authorization = `Bearer ${options.apiKey}`;
    if (body !== undefined) headers["content-type"] = "application/json";
    if (method !== "GET" && keyed) headers["idempotency-key"] = `ui_${newIdempotencyKey()}`;
    if (etags && needsIfMatch(method, path)) {
      // Never read by this client: read it now, so the edit names a version. A failed read leaves the edit without one.
      if (etags.get(path) === undefined) await send("GET", readPathOf(path), undefined, true, signal).then((read) => read.text(), () => "");
      const version = etags.get(path);
      if (version) headers["if-match"] = version;
    }
    // `signal` reaches the request itself, so closing a stream ends its connection, not only the reading of it: React
    // Native's fetch otherwise keeps the request open, and a few open ones use up the connections to the Gateway.
    // `keepalive` lets a small post outlive the page that sends it (a reader's `leave` as the tab closes).
    const response = await doFetch(`${base}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), ...(signal ? { signal } : {}), ...(keepalive ? { keepalive } : {}) });
    etags?.learn(method, path, response.status, response.headers.get("etag"));
    if (!response.ok) {
      const text = await response.text();
      let error: (ApiError["error"] & { nexus?: Record<string, unknown> }) | undefined;
      try {
        error = (JSON.parse(text || "{}") as Partial<ApiError>).error;
      } catch {
        error = undefined;
      }
      throw new CoreError(response.status, error?.message ?? `HTTP ${response.status}`, error?.code, undefined, error?.nexus);
    }
    return response;
  }
  const json = async <T>(method: string, path: string, body?: unknown) => (await (await send(method, path, body)).json()) as T;
  const search = (query: Record<string, string | number | undefined>) => {
    const params = new URLSearchParams(
      Object.entries(query)
        .filter((entry): entry is [string, string | number] => entry[1] !== undefined && entry[1] !== "")
        .map(([key, value]) => [key, String(value)]),
    ).toString();
    return params ? `?${params}` : "";
  };

  /**
   * A page's server-sent events in `response` to `onEvent`, as they arrive. Resolves when the stream ends (the Gateway
   * copy drained, the reader lost access) or `signal` closes it; rejects when it breaks. A refusal rejects in `send`, as
   * a `CoreError`. Each caller sends its own route, so every route the kit calls stays a literal.
   */
  async function events(response: Response, onEvent: (event: PageEvent) => void, signal?: AbortSignal): Promise<void> {
    if (!response.body || !response.headers.get("content-type")?.includes("text/event-stream")) {
      throw new CoreError(502, "the page's events did not come as a stream");
    }
    await readEvents(response.body, onEvent, signal);
  }

  return {
    shareChatSession: (sessionId: string, body: ShareChatSessionRequest) => json<Artifact>("POST", `/v1/chat/sessions/${id(sessionId)}/share`, body),
    /** Only you again: every active version of the session's artifact revoked at once. */
    unshareChatSession: (sessionId: string) => json<ChatSessionUnshared>("DELETE", `/v1/chat/sessions/${id(sessionId)}/share`),
    /** A coding-agent session's snapshot, as the next version of its artifact: `fitSessionSnapshot` first. */
    shareAgentSession: (body: ShareAgentSessionRequest) => json<Artifact>("POST", "/v1/artifacts/sessions", body),
    /** Stop sharing a coding session: every active version revoked at once. `key` is its share's `sessionId` (`cds_…`). */
    unshareAgentSession: (key: string) => json<ChatSessionUnshared>("DELETE", `/v1/artifacts/sessions/${id(key)}`),
    exportChatSession: async (sessionId: string) => (await send("GET", `/v1/chat/sessions/${id(sessionId)}/export.md`)).text(),
    artifacts: (query: ArtifactsQuery = {}) => json<ArtifactPage>("GET", `/v1/artifacts${search({ ...query })}`),
    artifact: (artifactId: string) => json<ArtifactDetail>("GET", `/v1/artifacts/${id(artifactId)}`),
    exportArtifact: async (artifactId: string) => (await send("GET", `/v1/artifacts/${id(artifactId)}.md`)).text(),
    /** An image of an artifact's question, with its bytes as base64: to everyone who may read the artifact. */
    artifactAttachment: (artifactId: string, attachmentId: string) =>
      json<ArtifactAttachment>("GET", `/v1/artifacts/${id(artifactId)}/attachments/${id(attachmentId)}`),
    revokeArtifact: (artifactId: string) => json<{ revoked: boolean; id: string; version: number }>("DELETE", `/v1/artifacts/${id(artifactId)}`),
    /**
     * An artifact id, or a public link's token. `org` picks one of your organizations for the copy; by default, from
     * outside the sharer's organization, your personal organization.
     */
    continueArtifact: (artifactId: string, body: { title?: string; team?: string; org?: string } = {}) =>
      json<ContinuedChatSession>("POST", `/v1/artifacts/${id(artifactId)}/continue`, body),
    artifactAccess: (artifactId: string) => json<ArtifactAccess>("GET", `/v1/artifacts/${id(artifactId)}/access`),
    changeArtifactAccess: (artifactId: string, body: { grant?: string[]; deny?: string[]; remove?: string[] }) =>
      json<ArtifactAccess>("POST", `/v1/artifacts/${id(artifactId)}/access`, body),
    requestArtifactAccess: (artifactId: string, body: { message?: string } = {}) =>
      json<{ status: ArtifactAccessPerson["status"]; requested: boolean }>("POST", `/v1/artifacts/${id(artifactId)}/access-requests`, body),
    externalArtifacts: (query: ExternalArtifactsQuery = {}) => json<ExternalArtifactPage>("GET", `/v1/artifacts/external${search({ ...query })}`),
    revokeArtifacts: (ids: string[]) =>
      json<{ revoked: { id: string; version: number }[]; skipped: { id: string; reason: string }[] }>("POST", "/v1/artifacts/revoke", { ids }),
    publicArtifact: (token: string) => json<ArtifactDetail>("GET", `/v1/public/artifacts/${id(token)}`),
    publicArtifactAttachment: (token: string, attachmentId: string) =>
      json<ArtifactAttachment>("GET", `/v1/public/artifacts/${id(token)}/attachments/${id(attachmentId)}`),
    reportPublicArtifact: (token: string, body: { reason: ReportReason; details?: string }) =>
      json<{ reported: boolean }>("POST", `/v1/public/artifacts/${id(token)}/report`, body),
    // Pages: your own, private for now.
    createPage: (body: CreatePageRequest) => json<PageWritten>("POST", "/v1/pages", body),
    pages: (query: { limit?: number; cursor?: string; q?: string; order?: LibraryOrder; docText?: boolean } = {}) => json<PageList>("GET", `/v1/pages${search({ ...query, docText: query.docText === undefined ? undefined : String(query.docText) })}`),
    pageTrash: () => json<TrashedPageList>("GET", "/v1/pages/trash").then(({ pages }) => pages),
    setPageTrash: async (pageId: string, trashed: boolean) => {
      await send("PUT", `/v1/pages/${id(pageId)}/trash`, { trashed }, false);
    },
    purgePageTrash: async (pageId: string) => {
      await send("DELETE", `/v1/pages/${id(pageId)}/trash`);
    },
    page: (pageId: string) => json<PageDetail>("GET", `/v1/pages/${id(pageId)}`),
    writePageVersion: (pageId: string, body: WritePageVersionRequest) => json<PageWritten>("POST", `/v1/pages/${id(pageId)}/versions`, body),
    restorePage: (pageId: string, body: { version: number; baseVersion?: number }) => json<PageWritten>("POST", `/v1/pages/${id(pageId)}/restore`, body),
    pageVersion: (pageId: string, version: number | "latest") => json<PageVersion>("GET", `/v1/pages/${id(pageId)}/versions/${version}`),
    siteRoutes: (pageId: string, version: number | "latest") => json<SiteRoutes>("GET", `/v1/pages/${id(pageId)}/versions/${version}/site-routes`),
    /** A version's source as written (plain text, never rendered). */
    pageSource: async (pageId: string, version: number | "latest" = "latest") =>
      (await send("GET", `/v1/pages/${id(pageId)}/versions/${version}/source`)).text(),
    deletePage: (pageId: string) => json<{ deleted: boolean; id: string; versions: number }>("DELETE", `/v1/pages/${id(pageId)}`),
    /**
     * The page's live events (`GET /v1/pages/{id}/events`) to `onEvent`, as they arrive: `version` with the
     * latest version first, then each new one. Resolves when the stream ends (the Gateway copy drained, the reader lost
     * access) or `signal` closes it; rejects when it is refused (a `CoreError`) or breaks.
     */
    pageEvents: async (pageId: string, onEvent: (event: PageEvent) => void, signal?: AbortSignal) =>
      events(await send("GET", `/v1/pages/${id(pageId)}/events`, undefined, true, signal), onEvent, signal),
    /** The page's shared draft, every Yjs update oldest first (`GET /v1/pages/{id}/doc`). */
    pageDoc: (pageId: string) => json<PageDoc>("GET", `/v1/pages/${id(pageId)}/doc`),
    /**
     * An update to the shared draft, a snapshot of it, an editor's awareness, or a reader's presence.
     * Sent without an idempotency key: Yjs applies an update once however often it arrives, and a key would cost a
     * stored answer per keystroke. `keepalive` lets a small post finish after the page closes.
     */
    postPageDoc: async (pageId: string, body: PostPageDocRequest, options?: { keepalive?: boolean }) =>
      (await (await send("POST", `/v1/pages/${id(pageId)}/doc`, body, false, undefined, options?.keepalive)).json()) as { id?: number },
    /**
     * The shared draft's whole text as the page's live preview, built on `baseVersion` (the latest), for
     * every reader. Without an idempotency key, as the draft's posts: a repeat changes nothing.
     */
    putPagePreview: async (pageId: string, body: { baseVersion: number; content: string }) =>
      (await (await send("PUT", `/v1/pages/${id(pageId)}/preview`, body, false)).json()) as PagePreviewEvent,
    /** Check a design against its design system (the one it links, else the default). It never changes it. */
    checkPage: (pageId: string, version?: number) => json<PageCheck>("POST", `/v1/pages/${id(pageId)}/check`, version === undefined ? {} : { version }),
    // Nexus Design's design systems: anyone who chats reads them; owners and admins change them.
    designSystems: () => json<DesignSystemList>("GET", "/v1/design-systems"),
    designSystem: (systemId: string) => json<DesignSystemDetail>("GET", `/v1/design-systems/${id(systemId)}`),
    saveDesignSystem: (body: SaveDesignSystemRequest) => json<DesignSystemSaved>("POST", "/v1/design-systems", body),
    setDefaultDesignSystem: (systemId: string) => json<DesignSystem>("POST", `/v1/design-systems/${id(systemId)}/default`),
    deleteDesignSystem: (systemId: string) => json<DesignSystemDeleted>("DELETE", `/v1/design-systems/${id(systemId)}`),
    // Shared pages: an artifact of kind `page`, opened through a short-lived signed frame URL.
    sharePage: (pageId: string, body: SharePageRequest) => json<Artifact>("POST", `/v1/pages/${id(pageId)}/share`, body),
    /**
     * The owner's own preview. `version`: a version number, `latest`, or `live`: the page's live state, its
     * unsaved edits included.
     */
    /**
     * A version the caller may open as a file (`GET /v1/pages/{id}/export`). `board` picks one board (its
     * `data-id`, name or number); a PNG or SVG of a design with several needs one.
     */
    exportPage: async (pageId: string, format: PageExportFormat, options: { board?: string; scale?: 1 | 2; version?: number; notes?: boolean; signal?: AbortSignal } = {}): Promise<PageExport> => {
      const response = await send("GET", `/v1/pages/${id(pageId)}/export${search({ format, board: options.board, scale: options.scale, version: options.version, notes: options.notes === undefined ? undefined : String(options.notes) })}`, undefined, true, options.signal);
      return {
        blob: await response.blob(),
        filename: dispositionName(response.headers.get("content-disposition"), `${pageId}.${format}`),
        failed: exportFailures(response.headers.get("x-nexus-export-failed")),
      };
    },
    framePageVersion: (pageId: string, version: number | "latest" | "live", siteVersion?: number) => json<PageFrame>("GET", `/v1/pages/${id(pageId)}/versions/${version}/frame${search({ siteVersion })}`),
    /**
     * A small picture of a version (WebP), to whoever may open that version. A 404 when the format has none
     * or Gateway has no posters yet; a 503 while the renderer is busy or not configured.
     */
    pagePoster: async (pageId: string, version: number | "latest") => (await send("GET", `/v1/pages/${id(pageId)}/versions/${version}/poster`)).blob(),
    /** An unpinned page share's frame shows the page's live state, unsaved edits included; a pinned one, its version. */
    frameArtifact: (artifactId: string, siteVersion?: number) => json<PageFrame>("GET", `/v1/artifacts/${id(artifactId)}/frame${search({ siteVersion })}`),
    framePublicArtifact: (token: string, siteVersion?: number) => json<PageFrame>("GET", `/v1/public/artifacts/${id(token)}/frame${search({ siteVersion })}`),
    /**
     * A shared page's live events by the share's id, for a reader without the page's own stream: `version`
     * and `preview`, as `pageEvents`. The stream ends when the share is revoked or expires. A pinned share is a 409.
     */
    artifactEvents: async (artifactId: string, onEvent: (event: PageEvent) => void, signal?: AbortSignal) =>
      events(await send("GET", `/v1/artifacts/${id(artifactId)}/events`, undefined, true, signal), onEvent, signal),
    /** The same through a public link, rate limited per address. */
    publicArtifactEvents: async (token: string, onEvent: (event: PageEvent) => void, signal?: AbortSignal) =>
      events(await send("GET", `/v1/public/artifacts/${id(token)}/events`, undefined, true, signal), onEvent, signal),
    /** A shared page (its artifact's id, or a public link's token) copied into a page of your own, with a new chat on it. */
    remixArtifact: (artifactId: string) => json<PageRemixed>("POST", `/v1/artifacts/${id(artifactId)}/remix`),
    // A page's storage: for whoever may open the page, its owner or a reader of a share.
    pageStorageKeys: (pageId: string, scope: PageStorageScope, query: { prefix?: string; limit?: number; cursor?: string } = {}) =>
      json<PageStorageKeys>("GET", `/v1/pages/${id(pageId)}/storage/${scope}${search({ ...query })}`),
    pageStorageValue: (pageId: string, scope: PageStorageScope, key: string) =>
      json<PageStorageValue>("GET", `/v1/pages/${id(pageId)}/storage/${scope}/${id(key)}`),
    setPageStorageValue: (pageId: string, scope: PageStorageScope, key: string, value: string) =>
      json<PageStorageWritten>("PUT", `/v1/pages/${id(pageId)}/storage/${scope}/${id(key)}`, { value }),
    deletePageStorageValue: (pageId: string, scope: PageStorageScope, key: string) =>
      json<PageStorageDeleted>("DELETE", `/v1/pages/${id(pageId)}/storage/${scope}/${id(key)}`),
    // A page's queries (Nexus Data): its owner saves and removes them; whoever may open the page lists them
    // (without the statement and the connection) and runs them, each run answering the result every reader shares.
    pageQueries: (pageId: string) => json<PageQueryList>("GET", `/v1/pages/${id(pageId)}/queries`),
    savePageQuery: (pageId: string, name: string, body: SavePageQueryRequest) => json<PageQuerySaved>("PUT", `/v1/pages/${id(pageId)}/queries/${id(name)}`, body),
    deletePageQuery: (pageId: string, name: string) => json<PageQueryDeleted>("DELETE", `/v1/pages/${id(pageId)}/queries/${id(name)}`),
    runPageQuery: (pageId: string, name: string, params?: Record<string, unknown>, refresh?: boolean) =>
      json<PageQueryResult>("POST", `/v1/pages/${id(pageId)}/queries/${id(name)}/run`, { ...(params ? { params } : {}), ...(refresh ? { refresh } : {}) }),
    // A page's data files: a version's copies of the chat files its source names, for whoever may open that
    // version. No version: the newest the caller may open.
    pageFiles: (pageId: string, version?: number) => json<PageFiles>("GET", `/v1/pages/${id(pageId)}/files${search({ version })}`),
    pageFile: (pageId: string, name: string, version?: number) => json<PageFile>("GET", `/v1/pages/${id(pageId)}/files/${id(name)}${search({ version })}`),
    // Comments on a page: whoever may open the page reads and writes them; its owner asks Nexus from one.
    pageComments: (pageId: string) => json<PageComments>("GET", `/v1/pages/${id(pageId)}/comments`),
    createPageComment: (pageId: string, body: CreatePageCommentRequest) => json<PageComment>("POST", `/v1/pages/${id(pageId)}/comments`, body),
    resolvePageComment: (pageId: string, commentId: string, resolved: boolean) =>
      json<PageComment>("PATCH", `/v1/pages/${id(pageId)}/comments/${id(commentId)}`, { resolved }),
    deletePageComment: (pageId: string, commentId: string) => json<{ deleted: boolean; id: string }>("DELETE", `/v1/pages/${id(pageId)}/comments/${id(commentId)}`),
    /** Opens a chat of yours on the page and answers the instruction to send there as its first message. */
    askNexusOnPageComment: (pageId: string, commentId: string) => json<PageCommentAsked>("POST", `/v1/pages/${id(pageId)}/comments/${id(commentId)}/ask`),
    /** The same for several threads in one chat: the ones named, else every open one. */
    askNexusOnPageComments: (pageId: string, threadIds?: string[]) =>
      json<PageCommentsAsked>("POST", `/v1/pages/${id(pageId)}/comments/ask`, threadIds ? { threadIds } : {}),
    /** Regenerate a design's board, make 3 variants of it, or keep a variant: a chat of yours on the page, as above. */
    askNexusOnPageBoard: (pageId: string, board: string, ask: PageBoardAsk) => json<PageBoardAsked>("POST", `/v1/pages/${id(pageId)}/boards/ask`, { ask, board } satisfies AskNexusOnPageBoardRequest),
    // Skills: your own SKILL.md files, in versions that never change.
    skills: () => json<SkillList>("GET", "/v1/skills"),
    skill: (skillId: string) => json<SkillDetail>("GET", `/v1/skills/${id(skillId)}`),
    createSkill: (body: CreateSkillRequest) => json<SkillWritten>("POST", "/v1/skills", body),
    writeSkillVersion: (skillId: string, body: WriteSkillVersionRequest) => json<SkillWritten>("POST", `/v1/skills/${id(skillId)}/versions`, body),
    /** A version and its SKILL.md as a string. A byte-order mark is part of the string; to save the file, use `exportSkill`. */
    skillVersion: (skillId: string, version: number | "latest") => json<SkillVersionDetail>("GET", `/v1/skills/${id(skillId)}/versions/${version}`),
    /** A version's SKILL.md as the bytes Gateway stored: a byte-order mark, CRLF line ends and a missing final newline survive. */
    exportSkill: async (skillId: string, version: number | "latest") => (await send("GET", `/v1/skills/${id(skillId)}/versions/${version}/export`)).arrayBuffer(),
    deleteSkill: (skillId: string) => json<SkillDeleted>("DELETE", `/v1/skills/${id(skillId)}`),
    // The skill library: the approved skills you may load besides your own. `skill`, `skillVersion` and
    // `exportSkill` also take a library skill's id, at an approved version.
    /** The library skills you may load, at the version a session loads. No skill text. */
    skillLibrary: () => json<SkillLibraryList>("GET", "/v1/skill-library"),
    // Skill promotions: ask to share a version of your skill with the organization or a team; an approver
    // who is not you answers. Every write carries an idempotency key, so a retry within 24 hours gets the first answer.
    /** A copy of the version is held until an approver answers. Your skill stays yours. */
    requestSkillPromotion: (skillId: string, version: number | "latest", body: RequestSkillPromotionRequest) =>
      json<SkillPromotion>("POST", `/v1/skills/${id(skillId)}/versions/${version}/promotions`, body),
    /** The promotions you reach, newest request first, with no skill text. */
    skillPromotions: (query: SkillPromotionsQuery = {}) =>
      json<SkillPromotionList>("GET", `/v1/skill-promotions${search({ ...query, mine: query.mine ? "true" : undefined })}`),
    /** One promotion and its snapshot, for its requester and the approvers who reach it. */
    skillPromotion: (promotionId: string) => json<SkillPromotionDetail>("GET", `/v1/skill-promotions/${id(promotionId)}`),
    /** Approve or refuse. A Nexus UI session or a CLI sign-in decides; an API key gets 403 `person_session_required`. */
    decideSkillPromotion: (promotionId: string, body: DecideSkillPromotionRequest) =>
      json<SkillPromotion>("POST", `/v1/skill-promotions/${id(promotionId)}/decision`, body),
    withdrawSkillPromotion: (promotionId: string, body: WithdrawSkillPromotionRequest = {}) =>
      json<SkillPromotionWithdrawn>("POST", `/v1/skill-promotions/${id(promotionId)}/withdraw`, body),
    // The notification inbox: parsed here, so a malformed answer fails at the boundary.
    notifications: async (query: NotificationsQuery = {}) =>
      parseNotificationPage(await json<unknown>("GET", `/v1/notifications${search({ cursor: query.cursor, unread: query.unread ? "true" : undefined, group: query.group })}`)),
    notificationKinds: async () => parseNotificationKinds(await json<unknown>("GET", "/v1/notifications/kinds")),
    markNotificationRead: async (notificationId: string) => parseNotificationItem(await json<unknown>("POST", `/v1/notifications/${id(notificationId)}/read`)),
    markNotificationDone: async (notificationId: string) => parseNotificationItem(await json<unknown>("POST", `/v1/notifications/${id(notificationId)}/done`)),
    // Settings: the caller's own, per group and channel; a PUT answers every setting.
    notificationSettings: async () => parseNotificationSettings(await json<unknown>("GET", "/v1/notifications/settings")),
    setNotificationSettings: async (settings: NotificationSettingChange[]) =>
      parseNotificationSettings(await json<unknown>("PUT", "/v1/notifications/settings", { settings })),
    // A member asks the owners about a billing block: once per person, block and UTC day.
    askOwners: async (block: BillingBlock) => parseAskOwners(await json<unknown>("POST", "/v1/notifications/ask-owner", { block })),
    // Devices for push: the token goes in once and never comes back.
    registerNotificationDevice: async (platform: NotificationDevicePlatform, token: string) =>
      parseNotificationDevice(await json<unknown>("POST", "/v1/notifications/devices", { platform, token })),
    notificationDevices: async () => parseNotificationDevices(await json<unknown>("GET", "/v1/notifications/devices")),
    // Web Push: the VAPID public key a browser subscribes with; a 503 when push is off.
    notificationPushKey: async () => parseNotificationPushKey(await json<unknown>("GET", "/v1/notifications/push-key")),
    revokeNotificationDevice: async (deviceId: string, reason?: "lost" | "signed_out") =>
      parseNotificationDevice(await json<unknown>("DELETE", `/v1/notifications/devices/${id(deviceId)}${search({ reason })}`)),
    // A desktop app collects its own push deliveries: each is answered once.
    collectNotificationDeliveries: async (deviceId: string) =>
      parseCollectedNotifications(await json<unknown>("POST", `/v1/notifications/devices/${id(deviceId)}/collect`)),
  };
}

export type ArtifactsClient = ReturnType<typeof createArtifactsClient>;
