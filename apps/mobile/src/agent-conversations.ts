import type { AgentConversationList, AgentConversationSummary, AgentSurface } from "@nexus/kit";
import { ago, spokenAgo } from "./ago";

/**
 * Nexus's conversations from every surface (shared conversations), without React: the label of each surface, a
 * row's line, and what each read of the agent API's list makes of the list on screen. The server's list is the source
 * of truth. The phone keeps only the id of the conversation on screen (`agent-conversation`), as before.
 */

/** A page of the list. */
export const PAGE = 20;

/** The one line when the list cannot be read (a 503 or the network). The chat still works. */
export const HISTORY_UNAVAILABLE = "Your history could not be read. Chat still works.";

const LABELS: Partial<Record<AgentSurface, string>> = { slack: "Slack", web: "Web", desktop: "Desktop", mobile: "Phone", cli: "Terminal" };

/** Where a conversation started, as a row names it. An ask that named no surface (`api`) has no label. */
export const surfaceLabel = (surface: AgentSurface): string | undefined => LABELS[surface];

/** A row's line under its title: where it started and when ("Slack · 2h"). */
export const rowLine = (row: AgentConversationSummary, now = Date.now()): string => [surfaceLabel(row.surface), ago(row.updatedAt, now)].filter(Boolean).join(" · ");

/** A row as VoiceOver and TalkBack read it: "Deploy plan, from Slack, 2 hours ago, Ship it Friday." */
export function rowLabel(row: AgentConversationSummary, now = Date.now()): string {
  const where = surfaceLabel(row.surface);
  const when = spokenAgo(row.updatedAt, now);
  return [row.title || "Untitled", where ? `from ${where}` : undefined, when && when !== "just now" ? `${when} ago` : when, row.preview || undefined].filter(Boolean).join(", ");
}

/**
 * The list on screen. `unknown`: the first read has not answered, so the screen shows no list yet. `off`: the API
 * keeps no shared conversations (a 404), so the screen hides the list and behaves as before. `unavailable`: the first
 * read failed. `on`: the rows, and `failed` when the read of a next page failed.
 */
export type History =
  | { kind: "unknown" }
  | { kind: "off" }
  | { kind: "unavailable" }
  | { kind: "on"; rows: AgentConversationSummary[]; next: string | null; failed?: true };

/** Whether the screen offers the list: once the API has answered, unless it keeps none. */
export const offersHistory = (history: History): boolean => history.kind === "on" || history.kind === "unavailable";

const newer = (a: AgentConversationSummary, b: AgentConversationSummary) => (Date.parse(b.updatedAt) || 0) - (Date.parse(a.updatedAt) || 0);

/**
 * What a read makes of the list. A first page replaces it; a next page (`more`) adds to it. The rows stay newest
 * first, each id once. A failed read keeps the rows already shown and their cursor, with the line, so Load more or
 * the next open tries again.
 */
export function historyAfter(now: History, read: AgentConversationList | "unreachable", more: boolean): History {
  if (read === "unreachable" || read.kind === "failed") return now.kind === "on" ? { ...now, failed: true } : { kind: "unavailable" };
  if (read.kind === "off") return { kind: "off" };
  const seen = new Set<string>();
  const rows = [...(more && now.kind === "on" ? now.rows : []), ...read.conversations].filter((row) => !seen.has(row.id) && Boolean(seen.add(row.id))).sort(newer);
  return { kind: "on", rows, next: read.next };
}
