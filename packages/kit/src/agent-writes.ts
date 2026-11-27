/**
 * The A9 approval card for every app (A1, A9): what a `pending_action` shows, and the agent API's answer when
 * the person approves or declines it in the chat. The agent API's `writes.ts` is the source of the wire types.
 *
 * - `POST /agent/writes/{card}/approve`, with `{}` or `{ "always": true }`, and `POST /agent/writes/{card}/decline`.
 * - Only the person's own credential decides. Another person's card is 404, a decided one 409 `already_decided`, and an
 *   expired one 410 `expired`. An agent link is 403.
 * - `PATCH /agent/writes/{card}` with `{ "arguments": { … } }` edits it while it waits (A24·b): only the
 *   fields its `edit` lists. The answer is the edited card, which replaces it. A card in an errand names the
 *   recipients the errand has not written to yet (`asks.newTo`), and its To line says "new to this errand".
 */
import { MESSAGE_FOOTERS } from "./sent-using";

/** The A9 card a `pending_action` carries (`PendingAsk` in the agent API's `render-tools.ts`). */
export interface AgentWriteAsk {
  tier: "asks-first" | "always-asks";
  kind: string;
  /** Who or where it goes. */
  to: string[];
  /** The exact text that goes, signature included. */
  text?: string;
  /** The amount in dollars, a decimal string. */
  amountUsd?: string;
  /** The person's own words, and when they said them (ISO 8601). */
  why: { said: string; at: string };
  voice: "assistant" | "you";
  /** The card offers "Always allow this kind". */
  always: boolean;
  /** The entries of `to` its errand has not written to yet (A24·b): the To line says "new to this errand". */
  newTo?: string[];
}

export type AgentWriteDecision = "approve" | "decline";

/**
 * One field the person may change on a waiting write (A24·b; `EditField` in the agents core's `edits.ts`).
 * `signed`: it holds the card's exact text with its signature, which the person edits without.
 */
export interface AgentWriteEditField {
  name: string;
  label: string;
  kind: "line" | "lines" | "addresses" | "time";
  required?: true;
  signed?: true;
}

/** The agent API's answer to an edit: the card it replaces, and the edited card, which waits as that one did. */
export interface AgentWriteEdited<P = AgentWritePending> {
  replaces: string;
  action: P;
}

/** What the kit reads of a `pending_action` for its card: the fields `agent-chat.ts`'s message carries. */
export interface AgentWritePending {
  kind: "pending_action";
  card: string;
  write: { connector: string; operation: string; summary: string; effect: string; arguments: Record<string, unknown> };
  approveUrl: string;
  detail: string;
  asks?: AgentWriteAsk;
  expiresAt?: string;
  inChat?: AgentWriteInChat;
  edit?: AgentWriteEditField[];
  edited?: true;
}

/**
 * Each kind's tier before any learned rule, as the agent's core sorts it (`TIER_OF` in `agents/core/src/tiers.ts`, whose
 * test holds the two equal). The Approvals page lists each tier's kinds from it (A9·a). A spend over the person's line,
 * an amount it cannot read and an irreversible write always ask, whatever their kind.
 */
export const AGENT_KIND_TIERS: Readonly<Record<string, "freely" | "asks-first" | "always-asks">> = {
  draft: "freely",
  remind: "freely",
  list: "freely",
  track: "freely",
  answer: "freely",
  send: "asks-first",
  book: "asks-first",
  spend: "asks-first",
  rsvp: "asks-first",
  "share-inside": "asks-first",
  write: "asks-first",
  "book-travel": "always-asks",
  password: "always-asks",
  "share-outside": "always-asks",
  delete: "always-asks",
  cancel: "always-asks",
  unsubscribe: "always-asks",
};

/**
 * What the person may do in the chat: approve or decline a write that asks first, or only decline one that always
 * asks, which they approve on its page in Nexus alone.
 */
export type AgentWriteInChat = "approve-or-decline" | "decline-only";

/** What the card offers, from a `pending_action`'s fields: Approve, Decline and "Always allow" in the chat, or none. */
export function writeChoices(message: { inChat?: AgentWriteInChat; asks?: Pick<AgentWriteAsk, "always"> }): { approve: boolean; decline: boolean; always: boolean } {
  const approve = message.inChat === "approve-or-decline";
  return { approve, decline: message.inChat !== undefined, always: approve && message.asks?.always === true };
}

/** The agent API's answer to a decision. `running` (202): approved, and the write had not ended yet. */
export interface AgentWriteDecided {
  card: string;
  decision: AgentWriteDecision;
  outcome: "written" | "failed" | "declined" | "running";
  text: string;
  result?: { title?: string; url?: string };
  always?: true;
  /**
   * A `failed` write Gateway refused with 403 `write_consent_required`: its `error.nexus`, as
   * `writeConsentOf` reads it. The app asks the person, then makes the `consent` call; the write is not sent again.
   */
  consent?: WriteConsent;
}

/**
 * Gateway's `error.nexus` on a connector write refused with 403 `write_consent_required` (only while
 * `NEXUS_CONNECTOR_WRITE_CONSENT` is on): the connection was consented for reading only. `consent` is the next call,
 * `POST /v1/connections/{connector}/oauth` with `access: "write"`, which answers the vendor's `authorizeUrl`.
 */
export interface WriteConsent {
  connector: string;
  account: string;
  /** The vendor scopes the write needs. */
  scopes: string[];
  consent: { method: "POST"; path: string; body: { scope: "user" | "org"; access: "write" } };
}

export const WRITE_CONSENT_REQUIRED = "write_consent_required";

/**
 * `error.nexus` of a `write_consent_required` refusal, read strictly; undefined for anything else. Only the one call
 * the contract names passes: a POST to this connector's own OAuth start, asking for writes.
 */
export function writeConsentOf(nexus: unknown): WriteConsent | undefined {
  const value = nexus as Partial<WriteConsent> | null | undefined;
  if (!value || typeof value !== "object") return undefined;
  const { connector, account, scopes, consent } = value;
  if (typeof connector !== "string" || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(connector)) return undefined;
  if (typeof account !== "string" || !Array.isArray(scopes) || !scopes.every((scope) => typeof scope === "string")) return undefined;
  if (!consent || consent.method !== "POST" || consent.path !== `/v1/connections/${connector}/oauth`) return undefined;
  const body = consent.body as Partial<WriteConsent["consent"]["body"]> | undefined;
  if (body?.access !== "write" || (body.scope !== "user" && body.scope !== "org")) return undefined;
  return { connector, account, scopes: [...scopes], consent: { method: "POST", path: consent.path, body: { scope: body.scope, access: "write" } } };
}

export const AGENT_WRITES_PREFIX = "/agent/writes/";
const CARD = /^card_[0-9a-f]{32}$/;

/** The decision's path, for a card id of the agent API's own shape; undefined for any other id. */
export function writeDecisionPath(card: string, decision: AgentWriteDecision): string | undefined {
  return CARD.test(card) ? `${AGENT_WRITES_PREFIX}${card}/${decision}` : undefined;
}

/** The decision's body: `{}`, or `{ "always": true }` on an approve. */
export function writeDecisionBody(decision: AgentWriteDecision, always = false): string {
  return JSON.stringify(decision === "approve" && always ? { always: true } : {});
}

const OUTCOMES: readonly string[] = ["written", "failed", "declined", "running"];

/** The answer, read strictly; undefined when it is not one. */
export function writeDecidedOf(body: unknown): AgentWriteDecided | undefined {
  const value = body as Partial<AgentWriteDecided> | null | undefined;
  if (!value || typeof value.card !== "string" || typeof value.text !== "string") return undefined;
  if (value.decision !== "approve" && value.decision !== "decline") return undefined;
  if (typeof value.outcome !== "string" || !OUTCOMES.includes(value.outcome)) return undefined;
  const result = value.result && typeof value.result === "object" ? value.result : undefined;
  const consent = value.outcome === "failed" ? writeConsentOf(value.consent) : undefined;
  return {
    card: value.card,
    decision: value.decision,
    outcome: value.outcome,
    text: value.text,
    ...(result
      ? { result: { ...(typeof result.title === "string" ? { title: result.title } : {}), ...(typeof result.url === "string" ? { url: result.url } : {}) } }
      : {}),
    ...(value.always === true ? { always: true as const } : {}),
    ...(consent ? { consent } : {}),
  };
}

/** What a failed decision tells the person, by the agent API's code. */
export function writeDecisionProblem(status: number, body?: { error?: unknown; detail?: unknown }): string {
  const code = typeof body?.error === "string" ? body.error : "";
  if (status === 410 || code === "expired") return "This approval expired. Nothing was written. Ask again.";
  if (code === "already_decided") return "This write was already approved or declined.";
  if (status === 404) return "This write no longer waits. Nothing more happens.";
  if (status === 403 && code === "person_credential_required") return "Only you can approve this, with your own sign-in.";
  if (typeof body?.detail === "string" && body.detail) return body.detail;
  return "Nexus could not take your answer. Try again in a moment.";
}

/** What an edited card says (A24·b). */
export const WRITE_EDITED_LABEL = "Edited by you";
/** What the To line says after a recipient its errand has not written to yet (A24·b). */
export const WRITE_NEW_TO_LABEL = "new to this errand";

const EDIT_KINDS: readonly string[] = ["line", "lines", "addresses", "time"];
const FIELD_NAME = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;

/**
 * The fields the card's Edit opens, read strictly; none when it offers no Edit. Only a card the person decides in the
 * chat (`inChat`) can be edited there.
 */
export function writeEditFields(message: { inChat?: AgentWriteInChat; edit?: unknown }): AgentWriteEditField[] {
  if (message.inChat === undefined || !Array.isArray(message.edit)) return [];
  return message.edit.flatMap((item): AgentWriteEditField[] => {
    const field = item as Partial<AgentWriteEditField> | null;
    if (!field || typeof field.name !== "string" || !FIELD_NAME.test(field.name)) return [];
    if (typeof field.label !== "string" || !field.label.trim() || field.label.length > 40) return [];
    if (typeof field.kind !== "string" || !EDIT_KINDS.includes(field.kind)) return [];
    return [{ name: field.name, label: field.label, kind: field.kind, ...(field.required === true ? { required: true as const } : {}), ...(field.signed === true ? { signed: true as const } : {}) }];
  });
}

/**
 * The text each field's input starts with: addresses as one line, split by commas, and a signed text without its
 * signature or "Sent using Nexus", which the agent adds again.
 */
export function writeEditValues(message: { write: { arguments: Record<string, unknown> }; inChat?: AgentWriteInChat; edit?: unknown }): Record<string, string> {
  const args = message.write.arguments;
  return Object.fromEntries(
    writeEditFields(message).map((field) => {
      const value = args[field.name];
      if (field.kind === "addresses") return [field.name, (Array.isArray(value) ? value : typeof value === "string" ? [value] : []).filter((item) => typeof item === "string").join(", ")];
      const text = typeof value === "string" ? value : "";
      return [field.name, field.signed ? writeTextParts(text).body : text];
    }),
  );
}

/** The addresses in one input line: split by commas, semicolons or new lines, each trimmed. */
export function writeEditAddresses(text: string): string[] {
  return text
    .split(/[,;\n]/)
    .map((item) => item.trim())
    .filter(Boolean);
}

/**
 * The fields the person changed, as `PATCH /agent/writes/{card}` takes them: only those whose input differs from what
 * it started with, each of its own type. Empty when nothing changed.
 */
export function writeEditChanges(message: { write: { arguments: Record<string, unknown> }; inChat?: AgentWriteInChat; edit?: unknown }, values: Readonly<Record<string, string>>): Record<string, string | string[]> {
  const start = writeEditValues(message);
  const changes: Record<string, string | string[]> = {};
  for (const field of writeEditFields(message)) {
    const value = values[field.name];
    if (value === undefined) continue;
    if (field.kind === "addresses") {
      const next = writeEditAddresses(value);
      if (next.join(",") !== writeEditAddresses(start[field.name] ?? "").join(",")) changes[field.name] = next;
      continue;
    }
    const next = field.kind === "lines" ? value : value.trim();
    if (next !== (field.kind === "lines" ? start[field.name] : start[field.name]?.trim())) changes[field.name] = next;
  }
  return changes;
}

/** The edit's path, for a card id of the agent API's own shape; undefined for any other id. */
export function writeEditPath(card: string): string | undefined {
  return CARD.test(card) ? `${AGENT_WRITES_PREFIX}${card}` : undefined;
}

/** The edit's body: `{ "arguments": { … } }`. */
export function writeEditBody(changes: Readonly<Record<string, string | string[]>>): string {
  return JSON.stringify({ arguments: changes });
}

/** The edit's answer, read strictly: the card it replaces and the edited `pending_action`; undefined when it is not one. */
export function writeEditedOf(body: unknown): AgentWriteEdited | undefined {
  const value = body as { replaces?: unknown; action?: unknown } | null | undefined;
  if (!value || typeof value.replaces !== "string" || !CARD.test(value.replaces)) return undefined;
  const action = value.action as Partial<AgentWritePending> | null | undefined;
  if (!action || action.kind !== "pending_action" || typeof action.card !== "string" || !CARD.test(action.card) || action.card === value.replaces) return undefined;
  const write = action.write;
  if (!write || typeof write !== "object" || typeof write.connector !== "string" || typeof write.operation !== "string" || typeof write.summary !== "string" || typeof write.effect !== "string") return undefined;
  if (!write.arguments || typeof write.arguments !== "object" || Array.isArray(write.arguments)) return undefined;
  if (typeof action.approveUrl !== "string" || typeof action.detail !== "string") return undefined;
  return { replaces: value.replaces, action: action as AgentWritePending };
}

/** What a failed edit tells the person, by the agent API's code. */
export function writeEditProblem(status: number, body?: { error?: unknown; detail?: unknown }): string {
  const code = typeof body?.error === "string" ? body.error : "";
  if (status === 410 || code === "expired") return "This approval expired. Nothing was written. Ask again.";
  if (code === "already_decided" || status === 404) return "This write no longer waits, so it cannot change.";
  if (code === "not_editable") return "This write cannot be edited. Decline it and ask again.";
  if (status === 403 && code === "person_credential_required") return "Only you can edit this, with your own sign-in.";
  if (typeof body?.detail === "string" && body.detail) return body.detail;
  return "Nexus could not save your edit. Try again in a moment.";
}

/** The To line's entries, each with whether its errand has not written to it yet (A24·b). */
export function writeToEntries(ask: Pick<AgentWriteAsk, "to" | "newTo"> | undefined): { to: string; isNew: boolean }[] {
  const fresh = new Set(Array.isArray(ask?.newTo) ? ask.newTo.filter((item) => typeof item === "string") : []);
  return (ask?.to ?? []).filter((to) => typeof to === "string" && to.trim()).map((to) => ({ to, isNew: fresh.has(to) }));
}

/**
 * The To line as text: "legal@lumen.bio · new to this errand" for one new recipient, as A24·b draws it. With several,
 * each new one reads "(new to this errand)" after it, so the note names its recipient. Undefined without one.
 */
export function writeToText(ask: Pick<AgentWriteAsk, "to" | "newTo"> | undefined): string | undefined {
  const entries = writeToEntries(ask);
  if (!entries.length) return undefined;
  if (entries.length === 1) return entries[0]!.isNew ? `${entries[0]!.to} · ${WRITE_NEW_TO_LABEL}` : entries[0]!.to;
  return entries.map((entry) => (entry.isNew ? `${entry.to} (${WRITE_NEW_TO_LABEL})` : entry.to)).join(" · ");
}

/** The card's tier label: "Asks first", or "Always asks" for a write that is never learned away. */
export function writeTierLabel(ask: Pick<AgentWriteAsk, "tier"> | undefined): string {
  return ask?.tier === "always-asks" ? "Always asks" : "Asks first";
}

/** "Expires in 30 min", "Expires in 1 min", or "Expired" once it passed. Undefined without a time. */
export function writeExpiryText(expiresAt: string | undefined, now = Date.now()): string | undefined {
  const at = expiresAt ? Date.parse(expiresAt) : NaN;
  if (!Number.isFinite(at)) return undefined;
  const minutes = Math.ceil((at - now) / 60_000);
  return minutes <= 0 ? "Expired" : `Expires in ${minutes} min`;
}

/** The Why line: "You said “tell them” at 08:24." in the reader's own clock. */
export function writeWhyText(why: AgentWriteAsk["why"] | undefined, time: (at: Date) => string = clock): string | undefined {
  if (!why?.said) return undefined;
  const at = new Date(why.at);
  return Number.isFinite(at.getTime()) ? `You said “${why.said}” at ${time(at)}.` : `You said “${why.said}”.`;
}

function clock(at: Date): string {
  return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
}

/** An amount in dollars as the card shows it: "$31.40". */
export function writeAmountText(amountUsd: string | undefined): string | undefined {
  if (!amountUsd || !/^\d+(?:\.\d{1,2})?$/.test(amountUsd)) return undefined;
  const [whole, cents = ""] = amountUsd.split(".");
  return `$${whole}.${cents.padEnd(2, "0")}`;
}

/** The agents core's signature (`signature` in its `tiers.ts`): one line that starts "Sent by". */
const SIGNATURE = /^Sent by [^\n]+$/;

/**
 * The exact text in the card's three parts (A7·a): an email's subject, the body, and the line that ends it. That line
 * is the signature of a message the agent sends as itself ("Sent by Nexus, Maya's assistant"), or "Sent using Nexus"
 * on one it sends as the person (`MESSAGE_FOOTERS`). The card draws the subject first and the end quieter, under the
 * body. A footer reads as the recipient sees it, so Slack's link markup shows as "Sent using Nexus".
 */
export function writeTextParts(text: string, args?: Record<string, unknown>): { subject?: string; body: string; end?: string } {
  const subject = typeof args?.subject === "string" && args.subject.trim() ? { subject: args.subject.trim() } : {};
  const trimmed = text.trimEnd();
  const footer = Object.values(MESSAGE_FOOTERS).find((candidate) => trimmed.length > candidate.line.length && trimmed.endsWith(candidate.line));
  if (footer) return { ...subject, body: trimmed.slice(0, -footer.line.length).trimEnd(), end: "Sent using Nexus" };
  const cut = trimmed.lastIndexOf("\n\n");
  const last = cut >= 0 ? trimmed.slice(cut + 2).trim() : "";
  if (SIGNATURE.test(last)) return { ...subject, body: trimmed.slice(0, cut).trimEnd(), end: last };
  return { ...subject, body: text };
}

/** One of the two voices a write goes in (A7·b). `sample`: how the message ends, or whose it reads as. */
export interface AgentWriteVoice {
  voice: AgentWriteAsk["voice"];
  title: string;
  line: string;
  sample: string;
}

/**
 * The two voices, as the Approvals page explains them (A7·b): as the person's assistant, signed, or as the person, with
 * only words they approved. `first` and `full`: the person's names, for the samples; absent, plain words. `assistant`:
 * the name the person gave their Assistant, which the signature says; Nexus by default.
 */
export function writeVoices(first?: string, full?: string, assistant = "Nexus"): AgentWriteVoice[] {
  return [
    {
      voice: "assistant",
      title: "As your assistant",
      line: "Logistics, scheduling, follow-ups, first contact. It writes as itself and says so.",
      sample: first ? `Sent by ${assistant}, ${first}'s assistant` : `Sent by ${assistant}, your assistant`,
    },
    {
      voice: "you",
      title: "As you",
      line: "Only words you have read and approved, exactly. It reads as you, with no signature of its own.",
      sample: full || "Your name",
    },
  ];
}
