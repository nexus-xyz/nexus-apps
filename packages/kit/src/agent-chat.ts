/**
 * Nexus in Assistant, for every app (Agent Interfaces S2): the agent API's wire types, its server-sent events, what each failure
 * tells the person, and a client for the app that calls the API itself (the phone app, S2.5). Nexus UI (S2.4) and
 * desktop (S2.3) call it through their own server or core, and read the same types and events from here.
 *
 * The agent API is not Gateway: it is the person's own agent, the one `@nexus` is in Slack
 * (`eng/factory/services/frontend/agents/slack/src/api/`). The source of each wire type is its `render.ts`. It takes
 * the person's own Nexus credential as `Authorization: Bearer <token>`.
 */
import { BRIEF_PATH, briefRead, type AgentBriefRead } from "./agent-brief";
import {
  agentErrandOf,
  errandActedOf,
  errandEditBody,
  errandHandedOffBody,
  errandHandoffOf,
  errandHistoryOf,
  errandListOf,
  errandMessageBody,
  errandReplyOf,
  errandsOff,
  errandViewOf,
  type AgentErrand,
  type AgentErrandAction,
  type AgentErrandActed,
  type AgentErrandEdit,
  type AgentErrandHandoff,
  type AgentErrandReply,
  type AgentErrandView,
} from "./agent-errands";
import {
  agentProjectLinkedOf,
  agentProjectListOf,
  agentProjectViewOf,
  AGENT_PROJECT_LINK_PATH,
  AGENT_PROJECT_VIEW_PATH,
  AGENT_PROJECTS_PATH,
  projectLinkBody,
  projectViewQuery,
  type AgentNoteProject,
  type AgentProject,
  type AgentProjectLinked,
  type AgentProjectList,
  type AgentProjectView,
} from "./agent-projects";
import { INTRO_PATH, INTRO_SEEN_PATH, introOf, type AgentIntroRead } from "./agent-intro";
import { PROACTIVITY_PATH, proactivityRead, type AgentProactivityRead, type ProactivityChange } from "./agent-proactivity";
import { CONTEXT_PATH, contextViewOf, MODEL_PATH, modelBody, modelViewOf, type AgentContextRef, type AgentContextView, type AgentModelView } from "./agent-composer";
import { connectorName, connectorPurpose } from "./connector-names";
import { PAUSE_PATH, pauseViewOf, PRIVACY_PATH, privacyViewOf, REVOKE_PATH, revokeResultOf, type AgentPauseView, type AgentPrivacyView, type AgentRevokeResult } from "./agent-privacy";
import { newIdempotencyKey, type Fetch } from "./core-client";
import {
  VAULT_ACTIVITY_PATH,
  VAULT_ASK_PREFIX,
  VAULT_CARD_PATH,
  VAULT_ID,
  VAULT_LOGIN_PREFIX,
  VAULT_PATH,
  vaultActivityOf,
  vaultChangedOf,
  vaultOff,
  vaultRead,
  type AgentVaultChanged,
  type AgentVaultRead,
  type VaultActivityEntry,
  type VaultAskApproval,
  type VaultLoginChange,
} from "./agent-vault";
import {
  writeDecidedOf,
  writeDecisionBody,
  writeDecisionPath,
  writeEditBody,
  writeEditedOf,
  writeEditPath,
  type AgentWriteAsk,
  type AgentWriteDecided,
  type AgentWriteDecision,
  type AgentWriteEditField,
  type AgentWriteInChat,
  type WriteConsent,
} from "./agent-writes";

/** A connector write the agent asked for. The API never runs it. */
export interface AgentWrite {
  connector: string;
  operation: string;
  summary: string;
  effect: string;
  arguments: Record<string, unknown>;
}

export interface AgentSource {
  title: string;
  url: string;
}

/**
 * A connector the answer needed and the person has not connected (A6·e): a tool call was refused for want of a
 * connection. An app draws it as a Connect card: the maker's mark, `label`, `summary` and a Connect button.
 */
export interface AgentConnect {
  /** The connector's id in Gateway's catalogue, such as `linear`. */
  connector: string;
  /** Its name as a person reads it, such as `Linear` (`connectorName`). */
  label: string;
  /** Where the person connects it: its card on Nexus UI's Integrations page. http(s) only. */
  url: string;
  /** What Nexus can do with it, in plain words. Apps draw `connectLine` instead, from the kit's map. */
  summary?: string;
}

/**
 * One of the person's coding sessions, as the A1 link card shows it: a bigger job handed to a session, or a
 * session the answer read. `where`: `synced` (it syncs end-to-end encrypted; its title and repository stay sealed, so
 * `title` names its agent) or `desktop` (set up for the desktop app, not started). `state` is drawn by shape: running,
 * done ✓, failed ✕, stopped, or ready. `url` is Open: Nexus UI's page, or the desktop's `nexus://new?…` link, which
 * fills a draft and never sends it.
 */
export interface AgentSession {
  where: "synced" | "desktop";
  state: AgentSessionState;
  title: string;
  repo?: string;
  branch?: string;
  detail?: string;
  url: string;
  id?: string;
}

/**
 * A text the agent prepared on the phone app: `to` is the recipient's name as the person said it,
 * which the app looks up in the device's contacts; `text` is what the person sends from their Messages app. `invite`:
 * an invitation to Nexus, so the app adds the person's referral link. Nothing was sent.
 */
export interface AgentText {
  to: string;
  text: string;
  invite?: true;
}

export const AGENT_SESSION_STATES = ["running", "done", "failed", "stopped", "ready"] as const;
export type AgentSessionState = (typeof AGENT_SESSION_STATES)[number];

export type AgentMessage =
  | {
      kind: "answer";
      /** Standard Markdown. Untrusted model output. */
      text: string;
      model: string;
      costUsd: number;
      choice?: { keyword: "fast" | "best" | "model"; model?: string; default?: true };
      notesUsed?: number;
      conversation?: { answers: number; costUsd: number };
      aiNotice?: true;
      sources?: AgentSource[];
      /** The connectors to connect before Nexus can do what was asked. Read it with `answerConnects`. */
      connect?: AgentConnect[];
      /** The person's coding sessions the answer read or set up (A1). Read it with `answerSessions`. */
      sessions?: AgentSession[];
      /** Texts the agent prepared for the person to send from the phone. Read it with `answerTexts`. */
      texts?: AgentText[];
      /** Up to three quick replies, for an answer that ends with a question (A1·D). Read it with `answerSuggestions`. */
      suggestions?: string[];
    }
  /**
   * `consent`: a write Gateway refused with 403 `write_consent_required`, as the agent API keeps it on the
   * conversation. Read it with `writeConsentOf`; the write is not sent again.
   */
  | { kind: "refused"; rule: string; detail: string; code?: string; billingUrl?: string; notice?: string; consent?: WriteConsent }
  /**
   * A write that waits for the person. `asks`: the A9 card (`agent-writes.ts`). `inChat`: what the person may do in the
   * chat (`POST /agent/writes/{card}/approve` or `…/decline`); without it, they decide only on `approveUrl`. `edit`: the
   * fields the person may change there (`PATCH /agent/writes/{card}`, read them with `writeEditFields`). `edited`: the
   * person edited the write it replaces (A24·b).
   */
  | {
      kind: "pending_action";
      card: string;
      write: AgentWrite;
      approveUrl: string;
      detail: string;
      asks?: AgentWriteAsk;
      expiresAt?: string;
      inChat?: AgentWriteInChat;
      edit?: AgentWriteEditField[];
      edited?: true;
    }
  /** `errand`: the errand the agent opened (`topic: "errands"`, `state: "opened"`), for its link card. Read it with `noticeErrand`. */
  | { kind: "notice"; topic: string; state: string; text: string; errand?: AgentErrand; /** `topic: "watches"`, `state: "proposed"`: the proposal (A3·b). Read it with `noticeWatchProposal`. */ watch?: unknown }
  /** `savedModel`: the person's saved model (`fast`, `best`, a model id, or `auto`), as a DM's answer names it (A16). */
  | { kind: "memories"; enabled: boolean; notes: { text: string; createdAt: string }[]; savedModel?: string }
  | { kind: "forgotten"; enabled: boolean; scope: "last" | "all"; count: number }
  | { kind: "insight"; text: string; url: string }
  | { kind: "error"; status: number; requestId?: string; cut?: true };

/**
 * Where a note came from (A16, with the memory controls on): a conversation's summary, a fact the person told the agent,
 * or a note they added. `surface`: the platform it was made on, such as `slack` or `api`. `errand`: the errand whose
 * thread it came from, with its goal.
 */
export interface AgentNoteSource {
  kind: "conversation" | "told" | "added";
  surface?: string;
  errand?: { id: string; goal: string };
}

/** The kinds of a note: the memory file it goes in. A note with none goes in "Notes". */
export const AGENT_NOTE_KINDS = ["people", "preferences", "places", "routines"] as const;
export type AgentNoteKind = (typeof AGENT_NOTE_KINDS)[number];

/** One note of `GET /agent/memory` (A16). `platform`: for a channel note, the surface it was made on, such as `slack`. */
export interface AgentMemoryNote {
  id: string;
  text: string;
  createdAt: string;
  audience: "dm" | "channel";
  platform?: string;
  /** Its kind, when the agent kept one. An older note has none. */
  kind?: AgentNoteKind;
  /** With the memory controls on: where it came from. */
  source?: AgentNoteSource;
  /** With the memory controls on: when the person last edited it. */
  editedAt?: string;
  /** The project it is scoped to (A14 "What Nexus knows here"): a Gateway project's id and name, or a name. */
  project?: { id?: string; name: string };
}

/** The learning switch (A16·b): paused, and since when. */
export interface AgentLearning {
  paused: boolean;
  pausedAt?: string;
}

/** `GET /agent/memory` (A16): every note the agent keeps about the person, newest first, and their saved model. */
export interface AgentMemoryView {
  notes: AgentMemoryNote[];
  /** `fast`, `best`, a model id, or `auto` for none. */
  savedModel: string;
  /** With the memory controls on: the learning switch. */
  learning?: AgentLearning;
  /** With the memory controls on: the person may add and edit notes and pause learning. */
  controls?: true;
}

const SOURCES: readonly string[] = ["conversation", "told", "added"];

/** One note of the view, or of an add's or an edit's `{ note }`, read strictly; undefined when it is not one. */
export function memoryNoteOf(item: unknown): AgentMemoryNote | undefined {
  const note = item as Partial<AgentMemoryNote> | null;
  if (!note || typeof note.id !== "string" || typeof note.text !== "string" || typeof note.createdAt !== "string") return undefined;
  if (note.audience !== "dm" && note.audience !== "channel") return undefined;
  const source = note.source as Partial<AgentNoteSource> | undefined;
  const errand = source?.errand as Partial<NonNullable<AgentNoteSource["errand"]>> | undefined;
  const kind = (AGENT_NOTE_KINDS as readonly unknown[]).includes(note.kind) ? note.kind : undefined;
  return {
    id: note.id,
    text: note.text,
    createdAt: note.createdAt,
    audience: note.audience,
    ...(typeof note.platform === "string" ? { platform: note.platform } : {}),
    ...(kind ? { kind } : {}),
    ...(source && typeof source.kind === "string" && SOURCES.includes(source.kind)
      ? {
          source: {
            kind: source.kind,
            ...(typeof source.surface === "string" ? { surface: source.surface } : {}),
            ...(errand && typeof errand.id === "string" && errand.id && typeof errand.goal === "string" ? { errand: { id: errand.id, goal: errand.goal } } : {}),
          },
        }
      : {}),
    ...(typeof note.editedAt === "string" ? { editedAt: note.editedAt } : {}),
    ...noteProjectOf(note.project),
  };
}

/** A note's project scope, read strictly: a name of 1 to 60 characters, and a Gateway id of its shape when it has one. */
function noteProjectOf(value: unknown): { project?: { id?: string; name: string } } {
  if (typeof value !== "object" || value === null) return {};
  const { id, name } = value as { id?: unknown; name?: unknown };
  if (typeof name !== "string" || !name.trim() || name.length > 60) return {};
  if (id !== undefined && (typeof id !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(id))) return {};
  return { project: { ...(typeof id === "string" ? { id } : {}), name } };
}

/** The learning switch, from the view or from `PUT /agent/memory/learning`'s `{ learning }`; undefined when it is none. */
export function learningOf(value: unknown): AgentLearning | undefined {
  const learning = value as Partial<AgentLearning> | null | undefined;
  if (!learning || typeof learning.paused !== "boolean") return undefined;
  return { paused: learning.paused, ...(typeof learning.pausedAt === "string" ? { pausedAt: learning.pausedAt } : {}) };
}

/** The longest note the agent API keeps, in UTF-16 units (its core's `NOTE_CHARS`). */
export const AGENT_NOTE_CHARS = 600;

/** The surfaces a channel note can come from, by the slug the agent keeps. */
const NOTE_PLATFORMS: Record<string, string> = { slack: "Slack", telegram: "Telegram" };
/** The surfaces a note's source names: the agent API is Nexus's own apps. */
const NOTE_SURFACES: Record<string, string> = { ...NOTE_PLATFORMS, api: "Nexus" };

/** A note's audience: where the agent may use it, which is the board's "each note shows its audience" (A16). */
export function noteAudience(note: Pick<AgentMemoryNote, "audience" | "platform">): { label: string; hint: string } {
  if (note.audience === "dm") return { label: "DM", hint: "Used only in your direct messages with Nexus." };
  const where = note.platform ? NOTE_PLATFORMS[note.platform] : undefined;
  return { label: where ? `${where} channel` : "Channel", hint: "Used only in the channel where you said it." };
}

/**
 * Where a note came from, in words (A16, "Show where"): "From a conversation in Slack", "You told it in Nexus" or "You
 * added it". A note from an errand names it: "Errand: Dinner on Friday". Undefined for a note made before
 * sources were kept. Nexus UI and the phone say it alike.
 */
export function noteSourceText(note: Pick<AgentMemoryNote, "source">): string | undefined {
  const source = note.source;
  if (!source) return undefined;
  if (source.kind === "added") return "You added it";
  if (source.errand) return `Errand: ${source.errand.goal}`;
  const where = source.surface ? NOTE_SURFACES[source.surface] : undefined;
  const what = source.kind === "told" ? "You told it" : "From a conversation";
  return where ? `${what} in ${where}` : what;
}

/** A note's text as the person typed it, or why it cannot be kept, before it is sent. */
export function noteDraft(text: string): { text: string } | { error: string } {
  const trimmed = text.trim();
  if (!trimmed) return { error: "Write something for Nexus to keep, or forget the note." };
  if (trimmed.length > AGENT_NOTE_CHARS) return { error: `A note is at most ${AGENT_NOTE_CHARS} characters. This one has ${trimmed.length}.` };
  return { text: trimmed };
}

/** `GET /agent/memory`'s body, read strictly; undefined when it is not one. A note that is not one is left out. */
export function memoryViewOf(body: unknown): AgentMemoryView | undefined {
  const value = body as { notes?: unknown; savedModel?: unknown; learning?: unknown; controls?: unknown } | null | undefined;
  if (!value || typeof value !== "object" || !Array.isArray(value.notes) || typeof value.savedModel !== "string") return undefined;
  const notes = value.notes.flatMap((item) => memoryNoteOf(item) ?? []);
  const learning = learningOf(value.learning);
  return { notes, savedModel: value.savedModel, ...(learning ? { learning } : {}), ...(value.controls === true ? { controls: true as const } : {}) };
}

/** One "Always allow" rule (A9·g): a kind of write, to one recipient when the card had one. */
export interface AgentApprovalRule {
  id: string;
  /** Such as `send`, or `write:<connector>.<operation>` for a write of no known kind. */
  kind: string;
  to?: string;
  since: string;
}

/** `GET /agent/approvals/settings` (A9·g, h). Money is a decimal string of dollars. */
export interface AgentApprovalsView {
  /** `off`: the agent does not sort writes by tier in this deployment yet, so every write gets a card. */
  tiers: "on" | "off";
  spendLimitUsd: string;
  ownLine: boolean;
  defaultSpendLimitUsd: string;
  maxSpendLimitUsd: string;
  rules: AgentApprovalRule[];
  /** How long a card waits for an answer, in minutes: after it, nothing is done. Absent from an older agent API. */
  expiresAfterMinutes?: number;
  /** Messages Nexus sends as the person end with "Sent using Nexus". Absent from an older agent API. */
  signMessages?: boolean;
}

/** The setting's label, the same on every surface. */
export const AGENT_SIGN_MESSAGES_LABEL = "Sign messages with Sent using Nexus";

const DOLLARS = /^\d{1,13}\.\d{2}$/;

/** `GET` or `PUT /agent/approvals/settings`'s body, read strictly; undefined when it is not one. A bad rule is left out. */
export function approvalsViewOf(body: unknown): AgentApprovalsView | undefined {
  const value = body as Partial<Record<keyof AgentApprovalsView, unknown>> | null | undefined;
  if (!value || typeof value !== "object" || (value.tiers !== "on" && value.tiers !== "off") || typeof value.ownLine !== "boolean" || !Array.isArray(value.rules)) return undefined;
  const { spendLimitUsd, defaultSpendLimitUsd, maxSpendLimitUsd } = value;
  if (![spendLimitUsd, defaultSpendLimitUsd, maxSpendLimitUsd].every((amount) => typeof amount === "string" && DOLLARS.test(amount))) return undefined;
  const rules = value.rules.flatMap((item): AgentApprovalRule[] => {
    const rule = item as Partial<AgentApprovalRule> | null;
    if (!rule || typeof rule.id !== "string" || typeof rule.kind !== "string" || typeof rule.since !== "string") return [];
    return [{ id: rule.id, kind: rule.kind, ...(typeof rule.to === "string" ? { to: rule.to } : {}), since: rule.since }];
  });
  const minutes = value.expiresAfterMinutes;
  const expiry = typeof minutes === "number" && Number.isInteger(minutes) && minutes > 0 && minutes <= 24 * 60 ? { expiresAfterMinutes: minutes } : {};
  const sign = typeof value.signMessages === "boolean" ? { signMessages: value.signMessages } : {};
  return { tiers: value.tiers, spendLimitUsd: spendLimitUsd as string, ownLine: value.ownLine, defaultSpendLimitUsd: defaultSpendLimitUsd as string, maxSpendLimitUsd: maxSpendLimitUsd as string, rules, ...expiry, ...sign };
}

/** `POST /agent/ask`'s answer, and the `done` event's data. */
export interface AgentReply {
  status: number;
  conversation: string;
  messages: AgentMessage[];
}

/** One turn of `GET /agent/conversations/{id}`. */
export interface AgentTurn {
  role: "user" | "assistant";
  text: string;
  at: string;
  model?: string;
  costUsd?: number;
  /** Complete reply messages, when served by an API that retains them. Older servers return only text/model/cost. */
  messages?: AgentMessage[];
  /** Where this turn was asked, when the API keeps shared conversations. */
  surface?: AgentSurface;
}

/** Where a conversation started, or where a turn was asked. `api`: an ask that named no surface. */
export type AgentSurface = "slack" | "web" | "desktop" | "mobile" | "cli" | "api";

const SURFACES: readonly string[] = ["slack", "web", "desktop", "mobile", "cli", "api"];

/** One row of `GET /agent/conversations`: one of the person's conversations, from any surface. */
export interface AgentConversationSummary {
  id: string;
  /** The first ask, cut to 80 characters. */
  title: string;
  /** Where it started. A surface the kit does not know reads as `api`. */
  surface: AgentSurface;
  updatedAt: string;
  turns: number;
  /** The first 140 characters of the last turn. */
  preview: string;
}

/**
 * `GET /agent/conversations`'s body, read strictly: the rows, newest activity first, and the cursor of the next page
 * (`null` on the last). Undefined when it is not one. A row that is not one is left out.
 */
export function conversationListOf(body: unknown): { conversations: AgentConversationSummary[]; next: string | null } | undefined {
  const value = body as { conversations?: unknown; next?: unknown } | null | undefined;
  if (!value || typeof value !== "object" || !Array.isArray(value.conversations)) return undefined;
  if (value.next !== undefined && value.next !== null && typeof value.next !== "string") return undefined;
  const conversations = value.conversations.flatMap((item): AgentConversationSummary[] => {
    const row = item as Partial<Record<keyof AgentConversationSummary, unknown>> | null;
    if (!row || typeof row.id !== "string" || !row.id || typeof row.title !== "string" || typeof row.updatedAt !== "string") return [];
    return [
      {
        id: row.id,
        title: row.title,
        surface: typeof row.surface === "string" && SURFACES.includes(row.surface) ? (row.surface as AgentSurface) : "api",
        updatedAt: row.updatedAt,
        turns: typeof row.turns === "number" && Number.isFinite(row.turns) ? row.turns : 0,
        preview: typeof row.preview === "string" ? row.preview : "",
      },
    ];
  });
  return { conversations, next: typeof value.next === "string" && value.next ? value.next : null };
}

/**
 * The agent API's events: `started`, `model`, `text` (each new piece) and `step` (the tool that runs now, as one line
 * such as "Searching Linear", A1·C b), then `done` or `error`.
 */
export type AgentEvent =
  | { type: "started"; conversation: string }
  | { type: "model"; model: string }
  | { type: "text"; delta: string }
  | { type: "step"; line: string }
  | { type: "done"; reply: AgentReply }
  | { type: "error"; status: number; error: string; detail: string };

/**
 * Reads server-sent events as they arrive. `push` takes the next piece of text and returns the events it completed;
 * the rest waits for the next piece. Comment lines (`: keepalive`) and unknown events are skipped.
 */
export class AgentEventReader {
  private buffer = "";

  push(chunk: string): AgentEvent[] {
    this.buffer += chunk.replace(/\r\n?/g, "\n");
    const events: AgentEvent[] = [];
    let end = this.buffer.indexOf("\n\n");
    while (end !== -1) {
      const block = this.buffer.slice(0, end);
      this.buffer = this.buffer.slice(end + 2);
      const event = parseBlock(block);
      if (event) events.push(event);
      end = this.buffer.indexOf("\n\n");
    }
    return events;
  }
}

function parseBlock(block: string): AgentEvent | undefined {
  let name = "message";
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    const value = colon === -1 ? "" : line.slice(colon + 1).replace(/^ /, "");
    if (field === "event") name = value;
    else if (field === "data") data.push(value);
  }
  if (!data.length) return undefined;
  let value: unknown;
  try {
    value = JSON.parse(data.join("\n"));
  } catch {
    return undefined;
  }
  if (!value || typeof value !== "object") return undefined;
  const body = value as Record<string, unknown>;
  switch (name) {
    case "started":
      return typeof body.conversation === "string" ? { type: "started", conversation: body.conversation } : undefined;
    case "model":
      return typeof body.model === "string" ? { type: "model", model: body.model } : undefined;
    case "text":
      return typeof body.delta === "string" ? { type: "text", delta: body.delta } : undefined;
    case "step": {
      const line = stepLineOf(body.line);
      return line ? { type: "step", line } : undefined;
    }
    case "done":
      return { type: "done", reply: replyOf(body) };
    case "error":
      return {
        type: "error",
        status: typeof body.status === "number" ? body.status : 502,
        error: typeof body.error === "string" ? body.error : "error",
        detail: typeof body.detail === "string" ? body.detail : "",
      };
    default:
      return undefined;
  }
}

const replyOf = (body: Record<string, unknown>): AgentReply => ({
  status: typeof body.status === "number" ? body.status : 200,
  conversation: typeof body.conversation === "string" ? body.conversation : "",
  messages: Array.isArray(body.messages) ? (body.messages as AgentMessage[]) : [],
});

/** The longest step line an app shows. */
export const AGENT_STEP_CHARS = 80;

/** A step line, read strictly: plain text on one line, at most `AGENT_STEP_CHARS`; undefined for anything else. */
export function stepLineOf(value: unknown): string | undefined {
  if (typeof value !== "string" || /[\u0000-\u001f\u007f]/.test(value)) return undefined;
  const line = value.trim();
  return line && line.length <= AGENT_STEP_CHARS ? line : undefined;
}

/** Reads a streamed answer to its end, calling `onEvent` for each event. */
export async function readAgentStream(body: ReadableStream<Uint8Array>, onEvent: (event: AgentEvent) => void): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const events = new AgentEventReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    for (const event of events.push(decoder.decode(value, { stream: true }))) onEvent(event);
  }
  for (const event of events.push(decoder.decode() + "\n\n")) onEvent(event);
}

/** What an app says when an ask fails. `signIn`: the app offers to sign in again. */
export interface AgentProblem {
  title: string;
  detail: string;
  signIn?: true;
}

/**
 * Whether a failure reads the same in every view of Assistant: a sign-in that ended (401), a refused credential (403),
 * and an agent API that is not available here (503 `agents_api_unconfigured`, or `agents_api_refused_token` when the
 * agents service runs on another Gateway). Each view words the rest itself.
 */
export function agentWide(status: number, body?: { error?: unknown }): boolean {
  const code = typeof body?.error === "string" ? body.error : "";
  return status === 401 || status === 403 || (status === 503 && (code === "agents_api_unconfigured" || code === "agents_api_refused_token"));
}

/**
 * The words for a failed ask or read: 401 means sign in again, 403 shows the API's own hint, and 429 and 504 get plain
 * words. `body` is the failure's JSON (`{ error, detail }`), when there is one. `again` is what the person does after
 * signing in again, so a view that asks no question can say its own step. Each app may word its own failures (no API
 * configured, its own sign-in) before this.
 */
export function agentProblem(status: number, body?: { error?: unknown; detail?: unknown }, again = "ask your question again"): AgentProblem {
  const detail = typeof body?.detail === "string" ? body.detail : "";
  const code = typeof body?.error === "string" ? body.error : "";
  if (status === 401) return { title: "Sign in again", detail: `Your sign-in ended. Sign in again, then ${again}.`, signIn: true };
  if (status === 403) return { title: "Nexus cannot answer with this sign-in", detail: detail || "Nexus refused this sign-in." };
  if (status === 429) return { title: "Too many questions at once", detail: "Wait a moment, then ask again." };
  if (status === 504) return { title: "The answer took too long", detail: "It may still be billed. Ask again to get a new answer." };
  if (status === 503 && code === "agents_api_unconfigured") return { title: "Nexus is not available here", detail: "This Nexus UI is not connected to the agent service." };
  if (status === 503 && code === "agents_api_refused_token") return { title: "Nexus is not available here", detail: "Your sign-in works, but the agent service runs on another Nexus and refused it." };
  if (status === 503) return { title: "Nexus is busy", detail: detail || "Ask again in a moment." };
  if (status === 409) return { title: "This question was already asked", detail: "Ask it again to get a new answer." };
  if (status === 400 || status === 413) return { title: "The question was not sent", detail: detail || "Check the question and send it again." };
  return { title: "Nexus did not answer", detail: detail || "The app could not reach Nexus. Try again in a moment." };
}

/** A legacy turn from `GET /agent/conversations/{id}`, as an answer when it has no complete messages. */
export function turnAnswer(turn: AgentTurn): Extract<AgentMessage, { kind: "answer" }> {
  return { kind: "answer", text: turn.text, model: turn.model ?? "", costUsd: turn.costUsd ?? Number.NaN };
}

/** A link an app may open: http(s) only, so a `javascript:` URL from a reply never becomes a link. */
export function safeUrl(url: unknown): string | undefined {
  if (typeof url !== "string") return undefined;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.toString() : undefined;
  } catch {
    return undefined;
  }
}

/** A connector id as Gateway names one: letters, digits, `-` and `_`. */
const CONNECTOR_ID = /^[a-z0-9][a-z0-9_-]{0,63}$/i;
/** The longest summary a Connect card shows. */
const CONNECT_SUMMARY_CHARS = 200;
/** The most Connect cards one answer shows. */
const MAX_CONNECTS = 4;

/**
 * An answer's Connect cards (A6·e), read strictly: a known connector id, an http(s) link and plain text. An entry that
 * is not one is left out, a missing label is the kit's name for the connector, and each connector shows once.
 */
export function answerConnects(message: object): AgentConnect[] {
  const connect: unknown = "connect" in message ? message.connect : undefined;
  if (!Array.isArray(connect)) return [];
  const found: AgentConnect[] = [];
  for (const item of connect) {
    const entry = item as Partial<Record<keyof AgentConnect, unknown>> | null;
    if (!entry || typeof entry !== "object" || typeof entry.connector !== "string" || !CONNECTOR_ID.test(entry.connector)) continue;
    const url = safeUrl(entry.url);
    if (!url || found.some((seen) => seen.connector === entry.connector)) continue;
    const label = typeof entry.label === "string" && entry.label.trim() ? entry.label.trim().slice(0, 64) : connectorName(entry.connector);
    const summary = typeof entry.summary === "string" ? entry.summary.replace(/\s+/g, " ").trim().slice(0, CONNECT_SUMMARY_CHARS) : "";
    found.push({ connector: entry.connector, label, url, ...(summary ? { summary } : {}) });
    if (found.length === MAX_CONNECTS) break;
  }
  return found;
}

/** The most session cards one answer shows. */
const MAX_SESSIONS = 3;
const SESSION_TEXT_CHARS = 120;
const SESSION_REPO = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/[A-Za-z0-9._-]{1,100}$/;

/** Plain text of at most `limit` characters on one line, or undefined for none. */
function cardText(value: unknown, limit = SESSION_TEXT_CHARS): string | undefined {
  if (typeof value !== "string") return undefined;
  const text = value.replace(/\s+/g, " ").trim().slice(0, limit).trim();
  return text || undefined;
}

/**
 * Where a session card's Open goes: an http(s) page, or, for a desktop session, the desktop's `nexus://new?…` draft
 * link and no other `nexus:` link. Anything else is no link, so a `javascript:` URL never becomes one.
 */
export function sessionUrl(url: unknown, where: AgentSession["where"]): string | undefined {
  if (where === "desktop") return typeof url === "string" && url.startsWith("nexus://new?") && url.length <= 100_000 ? url : undefined;
  return safeUrl(url);
}

/**
 * An answer's session cards (A1), read strictly: a known place and state, a title, an allowed link, plain text. An entry
 * that is not one is left out, and each link shows once.
 */
export function answerSessions(message: object): AgentSession[] {
  const sessions: unknown = "sessions" in message ? message.sessions : undefined;
  if (!Array.isArray(sessions)) return [];
  const found: AgentSession[] = [];
  for (const item of sessions) {
    const entry = item as Partial<Record<keyof AgentSession, unknown>> | null;
    if (!entry || typeof entry !== "object") continue;
    const where = entry.where === "synced" || entry.where === "desktop" ? entry.where : undefined;
    const state = AGENT_SESSION_STATES.find((known) => known === entry.state);
    const title = cardText(entry.title);
    const url = where && sessionUrl(entry.url, where);
    if (!where || !state || !title || !url || found.some((seen) => seen.url === url)) continue;
    const repo = typeof entry.repo === "string" && SESSION_REPO.test(entry.repo) ? entry.repo : undefined;
    const branch = cardText(entry.branch, 100);
    const detail = cardText(entry.detail, 80);
    const id = cardText(entry.id, 128);
    found.push({ where, state, title, url, ...(repo ? { repo } : {}), ...(branch ? { branch } : {}), ...(detail ? { detail } : {}), ...(id ? { id } : {}) });
    if (found.length === MAX_SESSIONS) break;
  }
  return found;
}

/** The most text cards one answer shows. */
export const AGENT_MAX_TEXTS = 3;
/** A text card's longest recipient name. */
export const AGENT_TEXT_TO_CHARS = 80;
/** A text card's longest text. */
export const AGENT_TEXT_CHARS = 1_000;

/**
 * An answer's text cards, read strictly: a recipient name on one line and a text, each within its bound,
 * with no control character but a line break in the text. An entry that is not one is left out, and each recipient and
 * text shows once.
 */
export function answerTexts(message: object): AgentText[] {
  const texts: unknown = "texts" in message ? message.texts : undefined;
  if (!Array.isArray(texts)) return [];
  const found: AgentText[] = [];
  for (const item of texts) {
    const entry = item as Partial<Record<keyof AgentText, unknown>> | null;
    if (!entry || typeof entry !== "object" || typeof entry.text !== "string") continue;
    const to = cardText(entry.to, AGENT_TEXT_TO_CHARS);
    const text = entry.text.trim();
    if (!to || !text || text.length > AGENT_TEXT_CHARS || /[\u0000-\u0009\u000b-\u001f\u007f]/.test(text)) continue;
    if (found.some((seen) => seen.to.toLowerCase() === to.toLowerCase() && seen.text === text)) continue;
    found.push({ to, text, ...(entry.invite === true ? { invite: true as const } : {}) });
    if (found.length === AGENT_MAX_TEXTS) break;
  }
  return found;
}

/** The most quick replies one answer shows (A1·D). */
export const AGENT_MAX_SUGGESTIONS = 3;
/** The longest quick reply. */
export const AGENT_SUGGESTION_CHARS = 40;

/**
 * An answer's quick replies (A1·D), read strictly: plain text on one line, at most `AGENT_SUGGESTION_CHARS` each, each
 * once, at most `AGENT_MAX_SUGGESTIONS`. Anything else is left out.
 */
export function answerSuggestions(message: object): string[] {
  const suggestions: unknown = "suggestions" in message ? message.suggestions : undefined;
  if (!Array.isArray(suggestions)) return [];
  const found: string[] = [];
  for (const item of suggestions) {
    if (typeof item !== "string" || /[\u0000-\u001f\u007f]/.test(item)) continue;
    const reply = item.trim();
    if (!reply || reply.length > AGENT_SUGGESTION_CHARS || found.some((seen) => seen.toLowerCase() === reply.toLowerCase())) continue;
    found.push(reply);
    if (found.length === AGENT_MAX_SUGGESTIONS) break;
  }
  return found;
}

/** The errand an "I opened an errand" notice carries (A1), read strictly; undefined for any other message. */
export function noticeErrand(message: AgentMessage): AgentErrand | undefined {
  if (message.kind !== "notice" || message.topic !== "errands" || message.state !== "opened") return undefined;
  return agentErrandOf(message.errand);
}

/** The state as the card's line says it (A1): "Running", "Done", "Failed", "Stopped", or "Ready to open". */
export function sessionStateText(state: AgentSessionState): string {
  return { running: "Running", done: "Done", failed: "Failed", stopped: "Stopped", ready: "Ready to open" }[state];
}

/**
 * The card's line under its title (A1): the state, then the branch or a detail, then the repository's name, as
 * "Done · on decoder-gru · cortex-sim". The app draws the state's shape before it and greys the repository.
 */
export function sessionCardLine(session: AgentSession): { state: string; detail?: string; repo?: string } {
  const detail = session.branch ? `on ${session.branch}` : session.detail;
  const repo = session.repo?.split("/").pop();
  return { state: sessionStateText(session.state), ...(detail ? { detail } : {}), ...(repo ? { repo } : {}) };
}

/** Names as a person says them: "Linear", "Linear and Gmail", "Linear, Gmail and Drive". */
const spokenList = (names: string[]) => (names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`);

/**
 * What Nexus says when an answer needs an account first (A6·e): it asks to connect it, as a person would, instead of
 * reading out that it is not connected. The Connect card shows with it; "yes" opens the first one.
 */
export function connectAsk(hints: AgentConnect[]): string {
  return `To do that, I need to connect ${spokenList(hints.map((hint) => hint.label))}. Can I? Say yes, or tap Connect.`;
}

/** A spoken yes to a question: a short reply that starts with one, in English or Spanish. */
export function isYes(words: string): boolean {
  const said = words.trim().toLowerCase().replace(/[.!?,]+$/, "");
  return said.split(/\s+/).length <= 5 && /^(yes|yeah|yep|sure|ok|okay|go ahead|do it|please|connect it|s[ií]|claro|dale|va|hazlo|con[eé]ctalo)(?=$|[\s,.!?])/.test(said);
}

/** A spoken no to a question: a short reply that starts with one. */
export function isNo(words: string): boolean {
  const said = words.trim().toLowerCase().replace(/[.!?,]+$/, "");
  return said.split(/\s+/).length <= 5 && /^(no|nope|not now|later|ahora no|despu[eé]s)(?=$|[\s,.!?])/.test(said);
}

/**
 * The Connect card's one line (A6·e): what Nexus can do with the account, in plain words. The agent sends it as
 * `summary`, from the kit's map (`connectorPurpose`); a hint without one gets the map's line here.
 * `assistant`: the Assistant's name, which the person may change; Nexus by default.
 */
export function connectLine(connect: AgentConnect, assistant = "Nexus"): string {
  let summary = connect.summary?.trimEnd();
  while (summary?.endsWith(".")) summary = summary.slice(0, -1).trimEnd();
  return summary ? `${summary}.` : connectorPurpose(connect.connector, assistant);
}

/** The agent API's limit on a question (`MAX_MESSAGE_CHARS` in its `server.ts`). */
export const AGENT_MAX_CHARS = 8_000;

/** A failed ask or read: its status and the API's `{ error, detail }`, when it sent one. */
export interface AgentFailure {
  status: number;
  error?: string;
  detail?: string;
}

/** How an ask ended: the reply, a failure, or a stream that stopped before its `done` event. */
export type AgentAsk = { kind: "reply"; reply: AgentReply } | { kind: "failed"; failure: AgentFailure } | { kind: "cut" };

/** How a read of a conversation ended. `gone`: the agent no longer holds it (it keeps one for an hour). */
export type AgentConversation = { kind: "turns"; turns: AgentTurn[] } | { kind: "gone" } | { kind: "failed"; failure: AgentFailure };

/**
 * How a read of the list of conversations ended. `off`: a 404, so this agent API keeps no shared conversations
 * (`NEXUS_AGENTS_SHARED_CONVERSATIONS` is off, or the API predates it), and an app hides the list.
 */
export type AgentConversationList =
  | { kind: "page"; conversations: AgentConversationSummary[]; next: string | null }
  | { kind: "off" }
  | { kind: "failed"; failure: AgentFailure };

/**
 * How a read of the errands ended (A12). `off`: errands are off at the agent API, or it predates them; an app hides them.
 * `history`: the closed and archived errands, newest first; absent from an API that predates it.
 */
export type AgentErrandList = { kind: "list"; errands: AgentErrand[]; history?: AgentErrand[] } | { kind: "off" } | { kind: "failed"; failure: AgentFailure };

/** How a read of one errand ended. `gone`: the person has no errand with this id. */
export type AgentErrandRead = { kind: "errand"; view: AgentErrandView } | { kind: "gone" } | { kind: "failed"; failure: AgentFailure };

/** How a read of an errand's hand-off ended (A12·e). `gone`: no errand with this id, or an API that predates it. */
export type AgentErrandHandoffRead = { kind: "handoff"; handoff: AgentErrandHandoff } | { kind: "gone" } | { kind: "failed"; failure: AgentFailure };

/** How an errand's action ended: what it did, or the API's refusal (such as 409 `closed`). */
export type AgentErrandResult = { kind: "acted"; acted: AgentErrandActed } | { kind: "failed"; failure: AgentFailure };

/**
 * How a vault write ended (A8): the vault as it is now, or a failure. `off`: the agent API has no vault here. `gone`:
 * the ask no longer waits (410).
 */
export type AgentVaultWrite = { kind: "changed"; changed: AgentVaultChanged } | { kind: "failed"; status: number; reason?: "off" | "gone" };

/** The vault's activity log (A8·e), `off` where the agent API has no vault, or a failed read. */
export type AgentVaultActivity = { kind: "entries"; entries: VaultActivityEntry[] } | { kind: "off" } | { kind: "failed"; status: number };

/**
 * How a privacy call ended (A11): its answer, `off` from an agent API that predates it (404), or a failure with the
 * API's `{ error, detail }`.
 */
export type AgentPrivacyCall<T> = { kind: "ok"; value: T } | { kind: "off" } | { kind: "failed"; failure: AgentFailure };

/** `decideWrite`'s answer: how the write ended, or why the decision was refused. */
export type AgentWriteResult = { kind: "decided"; decided: AgentWriteDecided } | { kind: "failed"; failure: AgentFailure };

/** `editWrite`'s answer: the edited card that replaces this one, or why the edit was refused. */
export type AgentWriteEditResult =
  | { kind: "edited"; replaces: string; action: Extract<AgentMessage, { kind: "pending_action" }> }
  | { kind: "failed"; failure: AgentFailure };

/** `GET /agent/memory` (A16): the view, or a failure. */
export type AgentMemoryRead = { kind: "view"; view: AgentMemoryView } | { kind: "failed"; failure: AgentFailure };

/** A change of the person's memory (A16): forget one note or all, add a note, edit one, or pause or resume learning. */
export type AgentMemoryEdit = { forget: string } | { forgetAll: true } | { add: string; project?: AgentNoteProject } | { edit: string; text: string } | { paused: boolean };

/** `GET /agent/projects` (A14): the person's projects, or `off` while the API keeps no errands. */
export type AgentProjectsRead = { kind: "list"; list: AgentProjectList } | { kind: "off" } | { kind: "failed"; failure: AgentFailure };

/** `GET /agent/projects/view`: one project, read whole; `gone` for a project that is not the person's. */
export type AgentProjectRead = { kind: "project"; view: AgentProjectView } | { kind: "gone" } | { kind: "failed"; failure: AgentFailure };

/** `POST /agent/projects/link`: the errands and notes that moved, or the API's refusal. */
export type AgentProjectLinkResult = ({ kind: "linked" } & AgentProjectLinked) | { kind: "failed"; failure: AgentFailure };

/** How a change of memory ended: how many notes a forget dropped, the note an add or an edit kept, or the switch. */
export type AgentMemoryChange =
  | { kind: "forgotten"; count: number }
  | { kind: "note"; note: AgentMemoryNote }
  | { kind: "learning"; learning: AgentLearning }
  | { kind: "failed"; failure: AgentFailure };

/** `GET /agent/approvals/settings` (A9·g, h): the view, or a failure. */
export type AgentApprovalsRead = { kind: "view"; view: AgentApprovalsView } | { kind: "failed"; failure: AgentFailure };

/** A composer read (`agent-composer.ts`): the view, `off` on an older API's 404, or a failure. */
export type AgentComposerRead<T> = { kind: "ok"; value: T } | { kind: "off" } | { kind: "failed"; failure: AgentFailure };

/** A composer read from its status and body: a 404 is an API without the route. */
export function composerRead<T>(status: number, body: unknown, read: (body: unknown) => T | undefined): AgentComposerRead<T> {
  if (status === 404) return { kind: "off" };
  if (status < 200 || status >= 300) return { kind: "failed", failure: failureFrom(status, body) };
  const value = read(body);
  return value ? { kind: "ok", value } : { kind: "failed", failure: { status: 502 } };
}

export interface AgentClientOptions {
  /** The agent API's base URL, such as `https://agents.example.com`; with `viaGateway`, Gateway's. */
  baseUrl: string;
  /**
   * Through Gateway's `/v1/agent/*`: `baseUrl` is Gateway's, every call goes to `<baseUrl>/v1/agent/…`,
   * and every write carries an `Idempotency-Key`. Gateway's own refusals read as the agent API's. Unset: the agent API.
   */
  viaGateway?: boolean;
  /** The person's own Nexus credential: a CLI or app sign-in token, an API key or an agent link. */
  apiKey?: string;
  /** `x-nexus-client`, which the API logs. */
  client?: string;
  fetch?: Fetch;
}

/**
 * A failure from a status and a body already read: the agent API's `{ error, detail }`, or Gateway's own
 * `{ error: { code, message } }` (403 `not_a_person`, 502 `agents_api_unreachable`, 504 `upstream_timeout`).
 */
const failureFrom = (status: number, body: unknown): AgentFailure => {
  const raw = (body && typeof body === "object" ? body : {}) as Record<string, unknown>;
  const gateway = raw.error && typeof raw.error === "object" ? (raw.error as Record<string, unknown>) : undefined;
  const fields = gateway ? { error: gateway.code, detail: gateway.message } : raw;
  return {
    status,
    ...(typeof fields.error === "string" ? { error: fields.error } : {}),
    ...(typeof fields.detail === "string" ? { detail: fields.detail } : {}),
  };
};

const failureOf = async (response: Response): Promise<AgentFailure> => failureFrom(response.status, await response.json().catch(() => undefined));

/**
 * A client of the agent API, for an app that holds the person's token itself. A network failure or an abort rejects;
 * every answer of the API resolves, as a reply or a failure. It never sends a connector write: the API runs none, and
 * a `pending_action` names the page where the person approves it. An errand's action changes only the person's own
 * record of it (A12).
 */
export function createAgentClient(options: AgentClientOptions) {
  const root = options.baseUrl.replace(/\/+$/, "");
  const base = options.viaGateway ? `${root}/v1` : root;
  const send: Fetch = options.fetch ?? ((input, init) => fetch(input, init));
  // Through Gateway, a write carries an `Idempotency-Key`, which Gateway may require (`NEXUS_IDEMPOTENCY_KEY_MODE`).
  const doFetch: Fetch =
    options.viaGateway
      ? (input, init) =>
          send(input, init?.method && init.method !== "GET" ? { ...init, headers: { ...(init.headers as Record<string, string>), "idempotency-key": newIdempotencyKey() } } : init)
      : send;
  const headers = (more: Record<string, string>) => ({
    ...more,
    ...(options.apiKey ? { authorization: `Bearer ${options.apiKey}` } : {}),
    ...(options.client ? { "x-nexus-client": options.client } : {}),
  });

  return {
    /**
     * `POST /agent/ask`, streamed: `onEvent` gets each event as it arrives. In `conversation` when there is one, a
     * conversation of the list included. `surface`: the app that asks, which the API keeps on the turn.
     */
    async ask(
      message: string,
      ask: {
        conversation?: string;
        surface?: Exclude<AgentSurface, "slack" | "api">;
        /** What `@` picked (A1, `agent-composer.ts`), and the `/` skill. */
        context?: AgentContextRef[];
        skill?: string;
        signal?: AbortSignal;
        onEvent?: (event: AgentEvent) => void;
      } = {},
    ): Promise<AgentAsk> {
      const response = await doFetch(`${base}/agent/ask`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "text/event-stream" }),
        body: JSON.stringify({
          message,
          stream: true,
          ...(ask.conversation ? { conversation: ask.conversation } : {}),
          ...(ask.surface ? { surface: ask.surface } : {}),
          ...(ask.context?.length ? { context: ask.context } : {}),
          ...(ask.skill ? { skill: ask.skill } : {}),
        }),
        signal: ask.signal,
      });
      const type = response.headers.get("content-type") ?? "";
      if (!response.ok) return { kind: "failed", failure: await failureOf(response) };
      // An API that answers in one piece (no stream) still answers.
      if (!type.startsWith("text/event-stream") || !response.body) {
        const body = (await response.json().catch(() => undefined)) as Record<string, unknown> | undefined;
        return body ? { kind: "reply", reply: replyOf(body) } : { kind: "failed", failure: { status: 502 } };
      }
      let outcome: AgentAsk = { kind: "cut" };
      await readAgentStream(response.body, (event) => {
        if (event.type === "done") outcome = { kind: "reply", reply: event.reply };
        else if (event.type === "error") outcome = { kind: "failed", failure: { status: event.status, error: event.error, detail: event.detail } };
        ask.onEvent?.(event);
      });
      return outcome;
    },

    /** `GET /agent/conversations/{id}`: the turns the agent still holds. Another person's conversation is `gone` too. */
    async conversation(id: string, signal?: AbortSignal): Promise<AgentConversation> {
      const response = await doFetch(`${base}/agent/conversations/${encodeURIComponent(id)}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: await failureOf(response) };
      const body = (await response.json().catch(() => undefined)) as { turns?: unknown; displayTurns?: unknown } | undefined;
      const turns = Array.isArray(body?.displayTurns) ? body.displayTurns : body?.turns;
      return { kind: "turns", turns: Array.isArray(turns) ? (turns as AgentTurn[]) : [] };
    },

    /**
     * `GET /agent/conversations`: one page of the person's conversations from every surface, newest first. `before`
     * is the last page's `next`. A 404 is `off`; a body that is no list is a failed read.
     */
    async conversations(page: { limit?: number; before?: string; signal?: AbortSignal } = {}): Promise<AgentConversationList> {
      // By hand: React Native's URLSearchParams does not implement `set`.
      const query = [page.limit ? `limit=${page.limit}` : "", page.before ? `before=${encodeURIComponent(page.before)}` : ""].filter(Boolean).join("&");
      const response = await doFetch(`${base}/agent/conversations${query ? `?${query}` : ""}`, { method: "GET", headers: headers({ accept: "application/json" }), signal: page.signal });
      if (response.status === 404) return { kind: "off" };
      if (!response.ok) return { kind: "failed", failure: await failureOf(response) };
      const list = conversationListOf(await response.json().catch(() => undefined));
      return list ? { kind: "page", ...list } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/errands` (A12): the person's errands, what needs them first, and the history. `off` while the API has none to show. */
    async errands(signal?: AbortSignal): Promise<AgentErrandList> {
      const response = await doFetch(`${base}/agent/errands`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return errandsOff(response.status, body as { error?: unknown }) ? { kind: "off" } : { kind: "failed", failure: failureFrom(response.status, body) };
      const errands = errandListOf(body);
      if (!errands) return { kind: "failed", failure: { status: 502 } };
      const history = errandHistoryOf(body);
      return history.length ? { kind: "list", errands, history } : { kind: "list", errands };
    },

    /** `GET /agent/projects` (A14): the person's projects, and whether Gateway's projects are on. */
    async projects(signal?: AbortSignal): Promise<AgentProjectsRead> {
      const response = await doFetch(`${base}${AGENT_PROJECTS_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return errandsOff(response.status, body as { error?: unknown }) ? { kind: "off" } : { kind: "failed", failure: failureFrom(response.status, body) };
      const list = agentProjectListOf(body);
      return list ? { kind: "list", list } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/projects/view` (A14): one project's errands, repositories, sessions and notes, by its Gateway id or its name. */
    async projectView(project: Pick<AgentProject, "id" | "name">, signal?: AbortSignal): Promise<AgentProjectRead> {
      const response = await doFetch(`${base}${AGENT_PROJECT_VIEW_PATH}?${projectViewQuery(project)}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = agentProjectViewOf(body);
      return view ? { kind: "project", view } : { kind: "failed", failure: { status: 502 } };
    },

    /** `POST /agent/projects/link` (A14): binds a name-only project's errands and notes to the Gateway project of the same name. */
    async linkProject(name: string, projectId: string, signal?: AbortSignal): Promise<AgentProjectLinkResult> {
      const response = await doFetch(`${base}${AGENT_PROJECT_LINK_PATH}`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: projectLinkBody(name, projectId),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const linked = agentProjectLinkedOf(body);
      return linked ? { kind: "linked", ...linked } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/errands/{id}`: one errand, read whole. Another person's errand is `gone` too. */
    async errand(id: string, signal?: AbortSignal): Promise<AgentErrandRead> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = errandViewOf(body);
      return view ? { kind: "errand", view } : { kind: "failed", failure: { status: 502 } };
    },

    /** `POST /agent/errands/{id}`: confirms a draft, marks it done, or cancels it, as the DM commands do. */
    async errandAction(id: string, action: AgentErrandAction, signal?: AbortSignal): Promise<AgentErrandResult> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: JSON.stringify({ action }),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const acted = errandActedOf(body);
      return acted ? { kind: "acted", acted } : { kind: "failed", failure: { status: 502 } };
    },

    /** `PATCH /agent/errands/{id}`: renames, pins, archives or marks done one errand; its view as it is now. */
    async errandEdit(id: string, edit: AgentErrandEdit, signal?: AbortSignal): Promise<AgentErrandRead> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: errandEditBody(edit),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = errandViewOf(body);
      return view ? { kind: "errand", view } : { kind: "failed", failure: { status: 502 } };
    },

    /** `POST /agent/errands/{id}/messages`: a reply about one errand, answered in its context. */
    async errandReply(id: string, message: string, signal?: AbortSignal): Promise<{ kind: "reply"; reply: AgentErrandReply } | { kind: "failed"; failure: AgentFailure }> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}/messages`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: errandMessageBody(message),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const reply = errandReplyOf(body);
      return reply ? { kind: "reply", reply } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/errands/{id}/handoff` (A12·e): the errand as a session's prompt, and the repository it names. */
    async errandHandoff(id: string, signal?: AbortSignal): Promise<AgentErrandHandoffRead> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}/handoff`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const handoff = errandHandoffOf(body);
      return handoff ? { kind: "handoff", handoff } : { kind: "failed", failure: { status: 502 } };
    },

    /** `POST /agent/errands/{id}/handoff` (A12·e): the errand records "Handed off to a session", with the session's link when known. */
    async errandHandedOff(id: string, url?: string, signal?: AbortSignal): Promise<AgentErrandRead> {
      const response = await doFetch(`${base}/agent/errands/${encodeURIComponent(id)}/handoff`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: errandHandedOffBody(url),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (response.status === 404) return { kind: "gone" };
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = errandViewOf(body);
      return view ? { kind: "errand", view } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/intro` (A19, `agent-intro.ts`): the first open, `seen` once shown, or `off` on a 404. */
    async intro(signal?: AbortSignal): Promise<AgentIntroRead> {
      const response = await doFetch(`${base}${INTRO_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      if (response.status === 404) return { kind: "off" };
      if (!response.ok) return { kind: "failed", status: response.status };
      const intro = introOf(await response.json().catch(() => undefined));
      return intro === "seen" ? { kind: "seen" } : intro ? { kind: "intro", intro } : { kind: "failed", status: 502 };
    },

    /** `GET /agent/model` (A1, `agent-composer.ts`): the saved model and the models to pick from; `off` on a 404. */
    async model(signal?: AbortSignal): Promise<AgentComposerRead<AgentModelView>> {
      const response = await doFetch(`${base}${MODEL_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      return composerRead(response.status, await response.json().catch(() => undefined), modelViewOf);
    },

    /** `PUT /agent/model`: saves `auto`, `fast`, `best` or a model id. A model the policy does not allow is a failure. */
    async setModel(model: string, signal?: AbortSignal): Promise<AgentComposerRead<AgentModelView>> {
      const response = await doFetch(`${base}${MODEL_PATH}`, { method: "PUT", headers: headers({ "content-type": "application/json", accept: "application/json" }), body: modelBody(model), signal });
      return composerRead(response.status, await response.json().catch(() => undefined), modelViewOf);
    },

    /** `GET /agent/context` (A1): what `@` may pick, each part with its error; `off` on a 404. */
    async context(signal?: AbortSignal): Promise<AgentComposerRead<AgentContextView>> {
      const response = await doFetch(`${base}${CONTEXT_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      return composerRead(response.status, await response.json().catch(() => undefined), contextViewOf);
    },

    /** `GET /agent/brief/today` (A1, `agent-brief.ts`): today's brief, `none`, or `off` while the API has none to give. */
    async brief(signal?: AbortSignal): Promise<AgentBriefRead> {
      const response = await doFetch(`${base}${BRIEF_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      return briefRead(response.status, await response.json().catch(() => undefined));
    },

    /** `GET /agent/memory` (A16): every note the agent keeps about the person, and with the controls on, the switch. */
    async memory(signal?: AbortSignal): Promise<AgentMemoryRead> {
      const response = await doFetch(`${base}/agent/memory`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = memoryViewOf(body);
      return view ? { kind: "view", view } : { kind: "failed", failure: { status: 502 } };
    },

    /**
     * Changes the person's memory (A16): forgets one note or all of them, or, with the agent API's memory controls on,
     * adds a note, edits one, or pauses or resumes learning. A 404 of an add, an edit or the switch is a failure: the
     * controls are off there.
     */
    async changeMemory(change: AgentMemoryEdit, signal?: AbortSignal): Promise<AgentMemoryChange> {
      const note = (id: string) => `${base}/agent/memory/notes/${encodeURIComponent(id)}`;
      const [method, url, body]: [string, string, unknown] =
        "forget" in change
          ? ["DELETE", note(change.forget), undefined]
          : "forgetAll" in change
            ? ["DELETE", `${base}/agent/memory`, undefined]
            : "add" in change
              ? ["POST", `${base}/agent/memory/notes`, { text: change.add, ...(change.project ? { project: change.project } : {}) }]
              : "edit" in change
                ? ["PATCH", note(change.edit), { text: change.text }]
                : ["PUT", `${base}/agent/memory/learning`, { paused: change.paused }];
      const response = await doFetch(url, {
        method,
        headers: headers({ accept: "application/json", ...(body ? { "content-type": "application/json" } : {}) }),
        ...(body ? { body: JSON.stringify(body) } : {}),
        signal,
      });
      const answer = (await response.json().catch(() => undefined)) as { forgotten?: unknown; note?: unknown; learning?: unknown } | undefined;
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, answer) };
      if (typeof answer?.forgotten === "number") return { kind: "forgotten", count: answer.forgotten };
      const kept = memoryNoteOf(answer?.note);
      if (kept) return { kind: "note", note: kept };
      const learning = learningOf(answer?.learning);
      return learning ? { kind: "learning", learning } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/approvals/settings` (A9·g, h): the tiers, the spend line and the kinds always allowed. */
    async approvals(signal?: AbortSignal): Promise<AgentApprovalsRead> {
      const response = await doFetch(`${base}/agent/approvals/settings`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const view = approvalsViewOf(body);
      return view ? { kind: "view", view } : { kind: "failed", failure: { status: 502 } };
    },

    /** `GET /agent/proactivity` (A10, `agent-proactivity.ts`): the dial, quiet hours and the kinds turned off; `off` on a 404. */
    async proactivity(signal?: AbortSignal): Promise<AgentProactivityRead> {
      const response = await doFetch(`${base}${PROACTIVITY_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      return proactivityRead(response.status, await response.json().catch(() => undefined));
    },

    /** `PUT /agent/proactivity`: sets each field the change holds (`null` resets it). The answer is the setting as it is now. */
    async setProactivity(change: ProactivityChange, signal?: AbortSignal): Promise<AgentProactivityRead> {
      const response = await doFetch(`${base}${PROACTIVITY_PATH}`, {
        method: "PUT",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: JSON.stringify(change),
        signal,
      });
      return proactivityRead(response.status, await response.json().catch(() => undefined));
    },

    /**
     * `POST /agent/writes/{card}/approve` or `…/decline` (A9): the person decides a write of theirs that waits. Only
     * with their own credential. The answer says how it ended; a refusal is a failure with the API's code.
     */
    async decideWrite(card: string, decision: AgentWriteDecision, options: { always?: boolean; signal?: AbortSignal } = {}): Promise<AgentWriteResult> {
      const path = writeDecisionPath(card, decision);
      if (!path) return { kind: "failed", failure: { status: 400, error: "bad_request", detail: "The card id is not valid." } };
      const response = await doFetch(`${base}${path}`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: writeDecisionBody(decision, options.always),
        signal: options.signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const decided = writeDecidedOf(body);
      return decided ? { kind: "decided", decided } : { kind: "failed", failure: { status: 502 } };
    },

    /**
     * `PATCH /agent/writes/{card}` (A24·b): the person edits a write of theirs that waits, with their own
     * credential. `changes` holds only the fields they changed (`writeEditChanges`). The answer is the edited card.
     */
    async editWrite(card: string, changes: Record<string, string | string[]>, signal?: AbortSignal): Promise<AgentWriteEditResult> {
      const path = writeEditPath(card);
      if (!path) return { kind: "failed", failure: { status: 400, error: "bad_request", detail: "The card id is not valid." } };
      const response = await doFetch(`${base}${path}`, {
        method: "PATCH",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: writeEditBody(changes),
        signal,
      });
      const body: unknown = await response.json().catch(() => undefined);
      if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
      const edited = writeEditedOf(body);
      return edited ? { kind: "edited", replaces: edited.replaces, action: edited.action as Extract<AgentMessage, { kind: "pending_action" }> } : { kind: "failed", failure: { status: 502 } };
    },

    /**
     * `GET /agent/vault` (A8, `agent-vault.ts`): the password manager, the logins and their grants, the card, and the
     * asks that wait. The kit's reader refuses a body shaped like a secret, so a bug upstream shows nothing.
     */
    async vault(signal?: AbortSignal): Promise<AgentVaultRead> {
      const response = await doFetch(`${base}${VAULT_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      return vaultRead(response.status, await response.json().catch(() => undefined));
    },

    /** `GET /agent/vault/activity` (A8·e): every use and every payment, who approved it and where. */
    async vaultActivity(signal?: AbortSignal): Promise<AgentVaultActivity> {
      const response = await doFetch(`${base}${VAULT_ACTIVITY_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal });
      const body: unknown = await response.json().catch(() => undefined);
      if (vaultOff(response.status, body)) return { kind: "off" };
      const entries = response.ok ? vaultActivityOf(body) : undefined;
      return entries ? { kind: "entries", entries } : { kind: "failed", status: response.ok ? 502 : response.status };
    },

    /**
     * A vault write (A8): `PUT /agent/vault/logins/{id}` with a grant, `PUT /agent/vault/card` with `{ frozen }`, or
     * `POST /agent/vault/asks/{id}/approve` and `…/decline` (the grant sheet). Its answer is the vault as it is now.
     */
    async vaultWrite(write: { login: string; change: VaultLoginChange } | { frozen: boolean } | { ask: string; decision: VaultAskApproval | "decline" }, signal?: AbortSignal): Promise<AgentVaultWrite> {
      const id = "login" in write ? write.login : "ask" in write ? write.ask : undefined;
      if (id !== undefined && !VAULT_ID.test(id)) return { kind: "failed", status: 400 };
      const [method, path, body] =
        "login" in write
          ? ["PUT", `${VAULT_LOGIN_PREFIX}${encodeURIComponent(write.login)}`, write.change]
          : "frozen" in write
            ? ["PUT", VAULT_CARD_PATH, { frozen: write.frozen }]
            : write.decision === "decline"
              ? ["POST", `${VAULT_ASK_PREFIX}${encodeURIComponent(write.ask)}/decline`, {}]
              : ["POST", `${VAULT_ASK_PREFIX}${encodeURIComponent(write.ask)}/approve`, write.decision];
      const response = await doFetch(`${base}${path}`, { method, headers: headers({ "content-type": "application/json", accept: "application/json" }), body: JSON.stringify(body), signal });
      const answer: unknown = await response.json().catch(() => undefined);
      if (vaultOff(response.status, answer)) return { kind: "failed", status: response.status, reason: "off" };
      if (response.status === 410) return { kind: "failed", status: 410, reason: "gone" };
      const changed = response.ok ? vaultChangedOf(answer) : undefined;
      return changed ? { kind: "changed", changed } : { kind: "failed", status: response.ok ? 502 : response.status };
    },

    /** `POST /agent/intro/seen`: keeps that the person saw the first open, on every app. True once kept. */
    async introSeen(signal?: AbortSignal): Promise<boolean> {
      const response = await doFetch(`${base}${INTRO_SEEN_PATH}`, { method: "POST", headers: headers({ accept: "application/json" }), signal });
      return response.ok;
    },

    /** `GET /agent/privacy` (A11, `agent-privacy.ts`): the pause, the agent links, and what Nexus can see per account. */
    async privacy(signal?: AbortSignal): Promise<AgentPrivacyCall<AgentPrivacyView>> {
      return privacyCall(await doFetch(`${base}${PRIVACY_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal }), privacyViewOf);
    },

    /** `GET /agent/pause`: whether the person paused the Assistant, for a row that shows Paused. */
    async pause(signal?: AbortSignal): Promise<AgentPrivacyCall<AgentPauseView>> {
      return privacyCall(await doFetch(`${base}${PAUSE_PATH}`, { method: "GET", headers: headers({ accept: "application/json" }), signal }), pauseViewOf);
    },

    /** `PUT /agent/pause`: pauses or resumes the Assistant. While paused, it runs no tools, routines, briefs or writes. */
    async setPaused(paused: boolean, signal?: AbortSignal): Promise<AgentPrivacyCall<AgentPauseView>> {
      const response = await doFetch(`${base}${PAUSE_PATH}`, {
        method: "PUT",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: JSON.stringify({ paused }),
        signal,
      });
      return privacyCall(response, pauseViewOf);
    },

    /**
     * `POST /agent/revoke` with the confirm (A11·b): ends every agent link of the person and, with `accounts`,
     * disconnects their own accounts. It never touches the organization's accounts.
     */
    async revokeAll(accounts: boolean, signal?: AbortSignal): Promise<AgentPrivacyCall<AgentRevokeResult>> {
      const response = await doFetch(`${base}${REVOKE_PATH}`, {
        method: "POST",
        headers: headers({ "content-type": "application/json", accept: "application/json" }),
        body: JSON.stringify({ confirm: true, accounts }),
        signal,
      });
      return privacyCall(response, revokeResultOf);
    },
  };
}

export type AgentClient = ReturnType<typeof createAgentClient>;

/** A privacy call's answer, read strictly: a 404 is `off`, a body that is not one is a failed read. */
async function privacyCall<T>(response: Response, read: (body: unknown) => T | undefined): Promise<AgentPrivacyCall<T>> {
  const body: unknown = await response.json().catch(() => undefined);
  if (response.status === 404) return { kind: "off" };
  if (!response.ok) return { kind: "failed", failure: failureFrom(response.status, body) };
  const value = read(body);
  return value === undefined ? { kind: "failed", failure: { status: 502 } } : { kind: "ok", value };
}
