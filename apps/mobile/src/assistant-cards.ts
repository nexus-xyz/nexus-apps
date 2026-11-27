import {
  answerConnects,
  answerSessions,
  answerSuggestions,
  connectLine,
  errandName,
  noticeErrand,
  safeUrl,
  sessionCardLine,
  writeAmountText,
  writeChoices,
  writeDecisionProblem,
  writeEditChanges,
  writeEditFields,
  writeEditProblem,
  writeEditValues,
  writeExpiryText,
  writeTextParts,
  writeTierLabel,
  writeToText,
  writeWhyText,
  type AgentWriteEditField,
  type AgentWriteEditResult,
  type AgentConnect,
  type AgentErrand,
  type AgentMessage,
  type AgentSession,
  type AgentWriteDecided,
  type AgentWriteResult,
} from "@nexus/kit";
import { errandLine } from "./agent-errands";

/**
 * The cards under Nexus's bubble in the Assistant's thread on the phone, without React: the Connect card (A6·e), the
 * approval card (A9), the quick replies (A1·D), and the errand and session link cards (A1·5). The words and the wire types are the kit's (`agent-chat.ts`, `agent-writes.ts`), which Nexus
 * UI and desktop read too. `connect-card.tsx` and `approval-card.tsx` draw them.
 */

/** One Connect card: the service, its first letter for the tile, one line, and where Connect goes. */
export interface ConnectCardView {
  connector: string;
  label: string;
  /** "Connect Linear": the card's title, and what Connect says to VoiceOver. */
  title: string;
  /** The tile's letter, as the board draws a service without its mark. */
  initial: string;
  line: string;
  url: string;
}

/** One account's Connect card (A6·e). `assistant`: the Assistant's name, Nexus by default. */
export function connectCard(hint: AgentConnect, assistant = "Nexus"): ConnectCardView {
  return {
    connector: hint.connector,
    label: hint.label,
    title: `Connect ${hint.label}`,
    initial: (hint.label.trim()[0] ?? hint.connector[0] ?? "?").toUpperCase(),
    line: connectLine(hint, assistant),
    url: hint.url,
  };
}

/** An answer's Connect cards (A6·e). Any other message has none. */
export function connectCards(message: AgentMessage, assistant = "Nexus"): ConnectCardView[] {
  if (message.kind !== "answer") return [];
  return answerConnects(message).map((hint) => connectCard(hint, assistant));
}

export type PendingMessage = Extract<AgentMessage, { kind: "pending_action" }>;

/** What the approval card (A9·b) shows, before the person answers. */
export interface ApprovalCardView {
  tier: "asks-first" | "always-asks";
  /** "Asks first" or "Always asks". */
  tierLabel: string;
  /** What it does, verb first. */
  summary: string;
  /** "legal@lumen.example · new to this errand" for a recipient its errand has not written to yet (A24·b). */
  to?: string;
  amount?: string;
  why?: string;
  /**
   * The exact text as it goes, in A7·a's three parts: an email's `subject`, the body (`text`), and the line that ends
   * it (`end`: the signature, or "Sent using Nexus"). Without a text, the write's connector and operation, and whether
   * it can be undone.
   */
  text?: string;
  subject?: string;
  end?: string;
  technical: string;
  /** "Expires in 30 min", or "Expired". */
  expiry?: string;
  expired: boolean;
  /** What the card offers in the chat. Approve is never offered for a write that always asks. */
  approve: boolean;
  decline: boolean;
  always: boolean;
  /** The approval page in Nexus, an http(s) link only. */
  review?: string;
  detail: string;
  /** The person edited the write it replaces (A24·b): the card says "Edited by you". */
  edited: boolean;
  /** The fields Edit opens; none when the card offers no Edit. */
  edit: AgentWriteEditField[];
}

/**
 * The card for a `pending_action`, as drawn at `now`. `canDecide`: the app holds an agent client; without one the card
 * offers only "Review in Nexus". `canEdit`: it can also edit a waiting write (A24·b).
 */
export function approvalCard(message: PendingMessage, now: number, canDecide = true, canEdit = false): ApprovalCardView {
  const ask = message.asks;
  const write = message.write;
  const choices = canDecide ? writeChoices(message) : { approve: false, decline: false, always: false };
  const expiry = writeExpiryText(message.expiresAt, now);
  const to = writeToText(ask);
  const exact = ask?.text?.trim() ? writeTextParts(ask.text, write.arguments) : undefined;
  return {
    tier: ask?.tier ?? "asks-first",
    tierLabel: writeTierLabel(ask),
    summary: write.summary,
    ...(to ? { to } : {}),
    ...optional("amount", writeAmountText(ask?.amountUsd)),
    ...optional("why", writeWhyText(ask?.why)),
    ...optional("text", exact?.body.trim() || undefined),
    ...optional("subject", exact?.subject),
    ...optional("end", exact?.end),
    technical: `${write.connector}.${write.operation} · ${write.effect === "irreversible" ? "cannot be undone" : "can be undone"}`,
    ...optional("expiry", expiry),
    expired: expiry === "Expired",
    ...choices,
    ...optional("review", safeUrl(message.approveUrl)),
    detail: message.detail,
    edited: message.edited === true,
    edit: canEdit && choices.approve ? writeEditFields(message) : [],
  };
}

/** What Edit's fields start with, by field name (A24·b). */
export function editStart(message: PendingMessage): Record<string, string> {
  return writeEditValues(message);
}

/** Why Save did not go when nothing changed. */
export const NOTHING_CHANGED = "Change a field, or approve it as it is.";

/** The fields the person changed, as the edit sends them; empty when nothing changed. */
export function editChanges(message: PendingMessage, values: Record<string, string>): Record<string, string | string[]> {
  return writeEditChanges(message, values);
}

/** The edited card that replaces the card, or why the edit failed, in the person's words. */
export function afterEdit(result: AgentWriteEditResult): { kind: "edited"; card: PendingMessage } | { kind: "refused"; text: string } {
  if (result.kind === "edited") return { kind: "edited", card: result.action };
  return { kind: "refused", text: writeEditProblem(result.failure.status, result.failure) };
}

/** What Save tells the person when the network or an abort ended it. */
export const EDIT_UNSENT = writeEditProblem(502);

function optional<K extends string>(key: K, value: string | undefined): Partial<Record<K, string>> {
  return value ? ({ [key]: value } as Record<K, string>) : {};
}

/** How the card stands: open, waiting on the answer, ended in place, or refused with the reason. */
export type ApprovalPhase =
  | { kind: "open" }
  | { kind: "working"; decision: "approve" | "decline" }
  | { kind: "ended"; decided: AgentWriteDecided }
  | { kind: "refused"; text: string };

/** The phase after `decideWrite` answered. */
export function phaseAfter(result: AgentWriteResult): ApprovalPhase {
  if (result.kind === "decided") return { kind: "ended", decided: result.decided };
  return { kind: "refused", text: writeDecisionProblem(result.failure.status, result.failure) };
}

/** The phase after `decideWrite` rejected: the network, or an abort. */
export const UNSENT: ApprovalPhase = { kind: "refused", text: writeDecisionProblem(502) };

/** How it ended, in the card's place: the agent's words, its link, and the line for "Always allow". */
export function endedView(decided: AgentWriteDecided): { text: string; url?: string; title?: string; always?: string; tone: "done" | "quiet" | "danger" } {
  const url = safeUrl(decided.result?.url);
  // A written result with a title shows as its own row with Open (A7·f), as the event it made.
  const title = decided.outcome === "written" ? decided.result?.title?.trim() : undefined;
  return {
    text: decided.text,
    ...(url ? { url } : {}),
    ...(title ? { title } : {}),
    ...(decided.always ? { always: "I won’t ask for this kind again. Change it in Account › Assistant › Approvals." } : {}),
    tone: decided.outcome === "failed" ? "danger" : decided.outcome === "written" ? "done" : "quiet",
  };
}

/** The quick replies of a reply's last answer (A1·D): at most three, and none when its last message is not an answer. */
export function lastSuggestions(messages: readonly AgentMessage[]): string[] {
  const last = messages.at(-1);
  return last?.kind === "answer" ? answerSuggestions(last) : [];
}

/** The bubble over an errand's link card (A1·5), in the web's words. */
export const OPENED_ERRAND = "I opened an errand for this.";

/** The word VoiceOver says for each errand state. */
const ERRAND_STATE: Record<AgentErrand["status"], string> = { running: "Running", "needs-you": "Needs you", done: "Done", failed: "Failed" };

/** One errand link card: its name, one line, and what VoiceOver says. The whole card opens the errand's own thread. */
export interface ErrandCardView {
  id: string;
  status: AgentErrand["status"];
  name: string;
  line: string;
  label: string;
}

/**
 * The errand card of an "I opened an errand" notice; undefined for any other message. `now` gives the errand as the
 * list has it now. The card shows that state, or the state the notice carried when the list no longer has it.
 */
export function errandCard(message: AgentMessage, now?: (id: string) => AgentErrand | undefined): ErrandCardView | undefined {
  const opened = noticeErrand(message);
  if (!opened) return undefined;
  const errand = now?.(opened.id) ?? opened;
  const name = errandName(errand);
  // A failed errand says why, when the agent said.
  const line = errand.status === "failed" && errand.detail ? errand.detail : errandLine(errand);
  return { id: errand.id, status: errand.status, name, line, label: `Open the errand ${name}, ${ERRAND_STATE[errand.status]}, ${line}` };
}

/**
 * One session link card (A1·5): the title, the state with a detail, the repository, and where it opens. A page of
 * Nexus opens in the in-app browser. A desktop draft (`nexus://new?…`) opens only in Nexus for desktop, so the phone
 * draws that card without a link and says where to open it.
 */
export interface SessionCardView {
  title: string;
  state: AgentSession["state"];
  line: string;
  repo?: string;
  url?: string;
  label: string;
}

/** The line of a session card the phone cannot open. */
export const ON_DESKTOP = "Open it in Nexus for desktop";

/** An answer's session cards; any other message has none. */
export function sessionCards(message: AgentMessage): SessionCardView[] {
  if (message.kind !== "answer") return [];
  return answerSessions(message).map((session) => {
    const parts = sessionCardLine(session);
    const url = safeUrl(session.url);
    const line = [parts.state, parts.detail].filter(Boolean).join(" · ");
    const said = [session.title, parts.state, parts.detail, parts.repo].filter(Boolean).join(", ");
    return {
      title: session.title,
      state: session.state,
      line: url ? line : `${line} · ${ON_DESKTOP}`,
      ...(parts.repo ? { repo: parts.repo } : {}),
      ...(url ? { url } : {}),
      label: url ? `Open ${said}` : `${said}. ${ON_DESKTOP}`,
    };
  });
}
