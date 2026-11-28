/**
 * Pages (Rust gateway only): standalone HTML, Markdown or React pages in versions that never change. The wire
 * types of `/v1/pages*`; the calls are on `createArtifactsClient`.
 */
import type { ChatSessionSummary } from "./api";

/**
/**
 * `jsx`: one React component, default-exported, that the sandbox host transpiles and mounts. `sheet`:
 * a spreadsheet as JSON data (`sheet.ts`), which the host renders as a table. `board`: a
 * whiteboard, JSON data in `./board`'s schema that the host draws as SVG.
 */
export type PageFormat = "html" | "md" | "jsx" | "sheet" | "board";
export type PageCreatedVia = "chat_tool" | "api" | "cli" | "restore" | "remix";

/** `PageSummary`: a page, as its owner lists it. */
export interface PageSummary {
  /** `pag_…`. */
  id: string;
  /** The latest version's title. */
  title: string;
  format: PageFormat;
  latestVersion: number;
  createdAt: string;
  /** When the latest version was written. */
  updatedAt: string;
}

/** `PageVersion`: one version, which never changes. */
export interface PageVersion {
  pageId: string;
  version: number;
  title: string;
  /** SHA-256 of the content, lowercase hex. */
  contentSha256: string;
  /** Size in bytes (UTF-8). */
  bytes: number;
  createdVia: PageCreatedVia;
  /** A restore's source version. */
  restoredFrom?: number;
  /** A remix's source: the share it was read through and the page version it copies. */
  remixedFrom?: PageRemixedFrom;
  createdAt: string;
}

export interface PageRemixedFrom {
  artifactId: string;
  pageId: string;
  version: number;
}

/** `GET /v1/pages/{id}`: the page and its versions, newest first. */
export interface PageDetail extends PageSummary {
  versions: PageVersion[];
}

export interface PageList {
  pages: PageSummary[];
  contentMatches?: Record<string, string>;
  nextCursor?: string;
}

/** `POST /v1/pages`, `/versions` and `/restore`. */
export interface PageWritten {
  page: PageSummary;
  version: PageVersion;
}

/** `POST /v1/artifacts/{id}/remix`: your copy (version 1) and the new chat that edits it. */
export interface PageRemixed {
  page: PageSummary;
  version: PageVersion;
  session: ChatSessionSummary;
}

/** One authored additional path in a saved Site version. The page's content remains the root. */
export interface SiteRouteWrite {
  path: string;
  title: string;
  content: string;
}

export interface SiteRoute {
  path: string;
  title: string;
}

export interface SiteRoutes {
  pageId: string;
  version: number;
  routes: SiteRoute[];
}

export interface CreatePageRequest {
  title: string;
  /** Default `html`. */
  format?: PageFormat;
  /** At most 16 MiB of UTF-8. */
  content: string;
  /** Additional routes. An omitted list on a version write carries the previous routes. */
  routes?: SiteRouteWrite[];
}

export interface WritePageVersionRequest {
  content: string;
  /** Additional routes. An omitted list on a version write carries the previous routes. */
  routes?: SiteRouteWrite[];
  /** A new title; unset keeps the latest one. */
  title?: string;
  /** The version this was built on: a 409 `page_version_conflict` when it is no longer the latest. */
  baseVersion?: number;
  /** An agent writing for its person: its name on the canvas, such as "Claude". */
  agent?: string;
}

/** The largest page, in bytes of UTF-8. */
export const MAX_PAGE_BYTES = 16 * 1024 * 1024;

/** `POST /v1/pages/{id}/share`: a chat share's audiences and expiry, without continuing or code sources. */
export interface SharePageRequest {
  audience: "org" | "teams" | "people" | "external" | "public";
  teams?: string[];
  /** Members' emails or ids (`people`); outsiders' emails (`external`). */
  people?: string[];
  /** `public`: `30d`, `12h` or a number of days. */
  expiresIn?: string | number;
  /** `public`: an ISO time. */
  expiresAt?: string;
  /** Pin this version; unset: readers see the latest one. */
  version?: number;
  /** `edit` lets the readers edit the page with its owner. Only for `org`, `teams` and `people`, never pinned. Default `view`. */
  role?: PageShareRole;
}

/** What a page share lets its readers do. */
export type PageShareRole = "view" | "edit";

/** `PageVersionEvent`: the data of a `version` event on `GET /v1/pages/{id}/events`, the version just written. */
export interface PageVersionEvent {
  pageId: string;
  version: number;
  title: string;
  format: PageFormat;
  /**
   * SHA-256 of the content, lowercase hex: a shared draft tells its own saves from other writes by it.
   * Gateway sends it; it is optional so a client built before it, or an older Gateway's event, still reads.
   */
  contentSha256?: string;
}

/**
 * `PagePreviewEvent`: the data of a `preview` event, unsaved text an editor posted on top of the latest
 * version, and `PUT /v1/pages/{id}/preview`'s answer. It names the text: readers reload the page's live frame.
 */
export interface PagePreviewEvent {
  pageId: string;
  /** The version the text is built on. */
  version: number;
  contentSha256: string;
}

/** `PageDocEvent`: the data of a `doc` event, one Yjs update to the page's shared draft. */
export interface PageDocEvent {
  pageId: string;
  /** The version the draft was started from. */
  seed: number;
  id: number;
  /** base64url. */
  update: string;
}

/**
 * `PagePeer`: who posted an `awareness` or `presence` event, as Gateway resolved them. A client never names
 * itself, so nobody shows under another person's name.
 */
export interface PagePeer {
  userId: string;
  /** Their name; unset when the account has no name. */
  name?: string;
}

/** `PageAwarenessEvent`: the data of an `awareness` event, an editor's cursor and name. Never stored. */
export interface PageAwarenessEvent {
  pageId: string;
  /** base64url. */
  update: string;
  /** Who posted it. Unset from a Gateway older than it. */
  from?: PagePeer;
}

/**
 * `PagePresenceEvent`: the data of a `presence` event, one reader's place on the page, never stored.
 * `state` is relayed as the client posted it, so check it; `from` is Gateway's.
 */
export interface PagePresenceEvent {
  pageId: string;
  from: PagePeer;
  state: Record<string, unknown>;
}

/** `GET /v1/pages/{id}/doc`: the shared draft, its updates oldest first. No `seed`: no draft. */
export interface PageDoc {
  seed?: number;
  /** The newest update's id: a snapshot with `upTo` set to it replaces every update here. */
  last?: number;
  /** base64url. */
  updates: string[];
}

/**
 * `POST /v1/pages/{id}/doc`: `update` and `seed` (with `snapshot` and `upTo` for a snapshot), or `awareness` alone, or
 * `presence` alone (a JSON object of at most 4 KiB). An editor may post `awareness`; a viewer may post `presence`.
 */
export interface PostPageDocRequest {
  seed?: number;
  update?: string;
  snapshot?: boolean;
  upTo?: number;
  awareness?: string;
  presence?: Record<string, unknown>;
}

/** One event of a page's stream: its type and its JSON data: `version`, `preview`, `doc`, `awareness` or `presence`. */
export interface PageEvent {
  type: string;
  data: unknown;
}

export interface SiteFrameRoute {
  path: string;
  title: string;
  url: string;
}

/**
 * `PageFrame`: where a page opens. `url` is on the sandbox origin, signed, and stops working at `expiresAt` (five
 * minutes); ask again for a new one. Load it in an `<iframe sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox">`.
 */
export interface PageFrame {
  url: string;
  expiresAt: string;
  pageId: string;
  version: number;
  format: PageFormat;
  title: string;
  /** Present on the authorized saved version when a caller requests siteVersion. */
  siteRoutes?: SiteFrameRoute[];
}

/**
 * A page's storage: text values by key, which the page's code keeps through Nexus UI. `personal` is the
 * caller's own copy, which no other reader sees; `shared` is one copy for everyone who may open the page.
 */
export type PageStorageScope = "personal" | "shared";

/** The most one person reaches on a page, keys and values of the shared scope and their own personal one, in UTF-8
 * bytes (413 `page_storage_full`). */
export const MAX_PAGE_STORAGE_BYTES = 20 * 1024 * 1024;
/** The largest value, in UTF-8 bytes (413 `page_storage_value_too_large`). */
export const MAX_PAGE_STORAGE_VALUE_BYTES = 5 * 1024 * 1024;

/** `GET /v1/pages/{id}/storage/{scope}/{key}`. */
export interface PageStorageValue {
  key: string;
  scope: PageStorageScope;
  value: string;
  updatedAt: string;
}

/** A data file's type: CSV, TSV, JSON, text, Markdown or XLSX. */
export type DataFileType =
  | "text/csv"
  | "text/tab-separated-values"
  | "application/json"
  | "text/plain"
  | "text/markdown"
  | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/**
 * One of a page version's data files: a read-only copy of a file attached in the chat that wrote it, which
 * the version's source names. `data` (standard base64) only when one file is read (`GET /v1/pages/{id}/files/{name}`).
 */
export interface PageFile {
  name: string;
  mediaType: DataFileType;
  bytes: number;
  /** The page version it belongs to. */
  version: number;
  createdAt: string;
  data?: string;
}

/** `GET /v1/pages/{id}/files`: a page's data files, by name, without their bytes. */
export interface PageFiles {
  files: PageFile[];
}

/** `PUT /v1/pages/{id}/storage/{scope}/{key}`. */
export interface PageStorageWritten {
  key: string;
  scope: PageStorageScope;
  /** The key's and the value's size in UTF-8 bytes, as the page's limit counts them. */
  bytes: number;
  updatedAt: string;
}

/** `DELETE /v1/pages/{id}/storage/{scope}/{key}`. A missing key is no error. */
export interface PageStorageDeleted {
  key: string;
  scope: PageStorageScope;
  deleted: boolean;
}

/** `GET /v1/pages/{id}/storage/{scope}`: the keys, in order. */
export interface PageStorageKeys {
  scope: PageStorageScope;
  prefix: string;
  keys: string[];
  /** Pass as `cursor` for the next keys; absent on the last list. */
  nextCursor?: string;
}

/**
 * `PageComment`: a comment on a page. A thread's first comment quotes a passage of the version it was made
 * on; a reply names its thread.
 */
export interface PageComment {
  /** `pcm_…`. */
  id: string;
  pageId: string;
  /** The thread's first comment; absent on that first comment. */
  threadId?: string;
  /** The author's name or email. Absent for a reader outside the page's organization, except on their own comments. */
  author?: string;
  /** Whether you wrote it. */
  mine: boolean;
  body: string;
  /** The passage, and the text just before and after it. */
  quote?: string;
  prefix?: string;
  suffix?: string;
  /** The point on a design's board the thread is pinned to. */
  pin?: PagePin;
  version: number;
  /** When the thread was resolved; absent while it is open. */
  resolvedAt?: string;
  /** The chat "Ask Nexus" opened, on the reply it posted. */
  chatSessionId?: string;
  createdAt: string;
}

/** `GET /v1/pages/{id}/comments`: oldest first. */
export interface PageComments {
  comments: PageComment[];
  /** Whether you own the page: you may delete any comment, resolve any thread, and ask Nexus. */
  owner: boolean;
}

/**
 * Where a thread on a design is pinned: a point on a board, in the board's pixels, and the part under it
 * when the canvas knew one. Gateway refuses any other field.
 */
export interface PagePin {
  /** The board's `data-id`, else its `data-artboard` name; on a deck, the slide's title. */
  board: string;
  x: number;
  y: number;
  /** The part's `data-nx-at`: its place in the source of the comment's version. */
  at?: number;
  /** The part's tag, lower case. */
  tag?: string;
  /** The part's `data-layer` name. */
  layer?: string;
  /** The part's place under the board, as child indices (`0.2.1`): finds it again in a later version. */
  path?: string;
  /** The point inside the part, as fractions of its width and height. */
  px?: number;
  py?: number;
}

/** `POST /v1/pages/{id}/comments`: a new thread (with an optional passage or pin) or a reply (`threadId`, neither). */
export interface CreatePageCommentRequest {
  body: string;
  threadId?: string;
  quote?: string;
  prefix?: string;
  suffix?: string;
  /** A point on a design's board, instead of a passage. */
  pin?: PagePin;
  /** The version the passage is on; unset: the newest you can open. */
  version?: number;
}

/** `POST /v1/pages/{id}/comments/{commentId}/ask`: the reply posted in the thread, the chat it opened, and what to send there. */
export interface PageCommentAsked {
  comment: PageComment;
  session: ChatSessionSummary;
  instruction: string;
}

/** `POST /v1/pages/{id}/comments/ask`: the reply in each thread, the one chat it opened, and what to send there. */
export interface PageCommentsAsked {
  comments: PageComment[];
  session: ChatSessionSummary;
  instruction: string;
}

// ---------------------------------------------------------------- Nexus Design's design systems

/** `DesignSystem`: a design system, as the organization lists it. The built-in is `ds_nexus_brand`, `@nexus/brand`. */
export interface DesignSystem {
  id: string;
  name: string;
  builtIn: boolean;
  isDefault: boolean;
  source: string;
  variableCount: number;
  componentCount: number;
  updatedAt?: string;
}

/** `DesignVariable`: one CSS custom property, in its selector's block. */
export interface DesignVariable {
  name: string;
  value: string;
  selector: string;
  kind: "color" | "length" | "font" | "other";
}

/** `DesignSystemDetail`: `GET /v1/design-systems/{id}`. */
export interface DesignSystemDetail extends DesignSystem {
  css: string;
  variables: DesignVariable[];
  notes: string;
  components: string[];
}

/** `DesignSystemList`: `GET /v1/design-systems`. */
export interface DesignSystemList {
  designSystems: DesignSystem[];
  defaultId: string;
}

/** `SaveDesignSystemRequest`: `POST /v1/design-systems`, which creates the named design system or replaces it. */
export interface SaveDesignSystemRequest {
  name: string;
  css?: string;
  files?: { path: string; text: string }[];
  notes?: string;
  components?: string[];
  source?: string;
  makeDefault?: boolean;
}

/** `DesignSystemSaved`. */
export interface DesignSystemSaved {
  designSystem: DesignSystem;
  created: boolean;
  read: string[];
}

/** `DesignSystemDeleted`. */
export interface DesignSystemDeleted {
  id: string;
  deleted: boolean;
  defaultId: string;
}

export type PageCheckKind = "color" | "font-size" | "spacing" | "contrast" | "overflow" | "clipped-text" | "outside-board" | "misaligned-rows";

/** `PageCheckFinding`: a value off the design system's tokens, text below WCAG AA contrast, or a layout slip (overflow, clipped text, outside the board, misaligned rows). */
export interface PageCheckFinding {
  kind: PageCheckKind;
  board?: string;
  boardId?: string;
  layer?: string;
  selector: string;
  /** CSS pixels from the board's top-left corner (the page's, outside a board). */
  box: { x: number; y: number; w: number; h: number };
  property: string;
  value: string;
  expected?: string;
  ratio?: number;
  message: string;
}

/** `PageCheck`: `POST /v1/pages/{id}/check`. It reads the page and never changes it. */
export interface PageCheck {
  pageId: string;
  version: number;
  designSystem: DesignSystem;
  linked: boolean;
  findings: PageCheckFinding[];
  counts: { color: number; fontSize: number; spacing: number; contrast: number; overflow?: number; clippedText?: number; outsideBoard?: number; misalignedRows?: number };
  truncated: boolean;
  elements: number;
}

/** What `POST /v1/pages/{id}/boards/ask` asks Nexus to do with a design's board. */
export type PageBoardAsk = "regenerate" | "variants" | "keep";

/** `POST /v1/pages/{id}/boards/ask`'s body: what to do, and the board's `data-id` (for `keep`, the variant's). */
export interface AskNexusOnPageBoardRequest {
  ask: PageBoardAsk;
  board: string;
}

/** `POST /v1/pages/{id}/boards/ask`: the chat it opened on the page, and what to send there. */
export interface PageBoardAsked {
  session: ChatSessionSummary;
  instruction: string;
}

// ---------------------------------------------------------------- Nexus Data

/** `PageQueryParam`: one parameter of a page's query, in the order its statement binds them (`$1`, `$2`). */
export interface PageQueryParam {
  name: string;
  type: "text" | "integer" | "number" | "boolean" | "date";
  /** What a run that names no value binds; with none, a run must name one. */
  default?: unknown;
}

/** `PageQuery`: one of a page's queries. A reader who is not the page's owner gets `name`, `params` and `refreshSeconds` only. */
export interface PageQuery {
  name: string;
  params: PageQueryParam[];
  refreshSeconds: number;
  status?: "approved" | "pending";
  connector?: "postgres" | "snowflake";
  account?: string;
  statement?: string;
  statementSha256?: string;
  reason?: string;
  proposedBy?: string;
  approvedBy?: string;
  approvedAt?: string;
  createdAt?: string;
  updatedAt?: string;
  /** When its most recent result was read: every reader sees it. */
  lastReadAt?: string;
  /** The owner's only: that result's rows, and why its last refresh failed. */
  lastRowCount?: number;
  lastError?: PageQueryError;
}

/** `PageQueryList`: `GET /v1/pages/{id}/queries`. `owner` says whether the caller reads the owner's fields. */
export interface PageQueryList {
  queries: PageQuery[];
  owner: boolean;
}

/** `SavePageQueryRequest`: `PUT /v1/pages/{id}/queries/{name}`, which saves and approves it. */
export interface SavePageQueryRequest {
  connector: "postgres" | "snowflake";
  account: string;
  statement: string;
  params?: PageQueryParam[];
  refreshSeconds?: number;
  reason?: string;
}

export interface PageQuerySaved {
  query: PageQuery;
  created: boolean;
}

export interface PageQueryDeleted {
  name: string;
  deleted: boolean;
}

/** `RunPageQueryRequest`: the query's parameter values by name. */
export interface RunPageQueryRequest {
  params?: Record<string, unknown>;
  /** Read it again now: the page's owner only. */
  refresh?: boolean;
}

export interface PageQueryColumn {
  name: string;
  type: string;
}

/** Why the last refresh failed: a stable code, and a message that never quotes the database. */
export interface PageQueryError {
  code: string;
  message: string;
}

/** `PageQueryResult`: the snapshot every reader of the page shares for these parameter values. */
export interface PageQueryResult {
  name: string;
  paramsKey: string;
  columns: PageQueryColumn[];
  /** Each row as an array, in the columns' order. */
  rows: unknown[][];
  truncated: boolean;
  readAt: string;
  refreshSeconds: number;
  /** Set when the last refresh failed: these rows are older than the interval. */
  error?: PageQueryError;
}

/** `PageQueryEvent` (`type: "query"` on a page's events): a query's result was refreshed. */
export interface PageQueryEvent {
  pageId: string;
  name: string;
  paramsKey: string;
  readAt: string;
}
