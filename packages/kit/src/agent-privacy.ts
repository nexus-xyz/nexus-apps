/**
 * The Assistant's privacy and trust (A11), for every app: the agent API's pause, what Nexus can see per
 * account, and Revoke all access, read strictly. The agent API writes the facts (`agents/slack/src/api/privacy.ts`);
 * this module holds the wire types, the readers, and the words every app shows.
 *
 * - `GET` and `PUT /agent/pause`: whether the person paused the Assistant. While paused, it answers each ask with one
 *   line and runs no tools, routines, briefs or writes.
 * - `GET /agent/privacy`: the pause, the person's live agent links, and per connected account what Nexus reads, what it
 *   writes after the person approves, and what it never does there. A part that could not be read carries its `error`.
 * - `POST /agent/revoke` with `{ "confirm": true, "accounts": true }`: ends every agent link of the person and, with
 *   `accounts`, disconnects the person's own accounts. It never touches the organization's accounts.
 *
 * An older agent API answers 404 for each path: the apps then hide the Privacy view.
 */
import { connectorName } from "./connector-names";

export const PAUSE_PATH = "/agent/pause";
export const PRIVACY_PATH = "/agent/privacy";
export const REVOKE_PATH = "/agent/revoke";

/** The one sentence about training, the same everywhere (P64). */
export const NEVER_TRAINS = "Nexus never trains models on your data.";

/** `GET` and `PUT /agent/pause`. */
export interface AgentPauseView {
  paused: boolean;
  /** When the person paused it (ISO 8601). */
  pausedAt?: string;
}

/** One live agent link of the person, as Gateway lists it. Never its token. */
export interface AgentPrivacyLink {
  id: string;
  /** What the agent is, as a slug (`slack`, `nexus-assistant`). */
  agent: string;
  label: string;
  createdAt: string;
  lastUsedAt?: string;
}

/** One connected account, and what Nexus does with it, by the connector catalogue's own summaries. */
export interface AgentPrivacyAccount {
  connector: string;
  /** Who the vendor says the holder is, else the account's name. */
  account: string;
  /** `user`: the person's own, which Revoke all access disconnects. `org`: the organization's, which it never touches. */
  scope: "user" | "org";
  /** The reads Nexus may run. */
  reads: string[];
  /** The writes the organization allows, each only after the person approves it. */
  writes: string[];
  /** The writes Nexus never makes here: the organization does not allow them. */
  never: string[];
  /** Why the operations could not be read; the lists are then empty. */
  error?: string;
}

/** `GET /agent/privacy`. */
export interface AgentPrivacyView {
  pause: AgentPauseView;
  links: AgentPrivacyLink[];
  /** The agent links could not be read: `links` is empty. */
  linksError?: string;
  accounts: AgentPrivacyAccount[];
  /** The accounts could not be read: `accounts` is empty. */
  accountsError?: string;
}

/** `POST /agent/revoke`'s body. */
export interface AgentRevokeRequest {
  confirm: true;
  /** Disconnect the person's own accounts too. */
  accounts: boolean;
}

/** One part that did not end, and why. */
export interface AgentRevokeFailure {
  id: string;
  error: string;
}

/** `POST /agent/revoke`: what ended, and what did not. */
export interface AgentRevokeResult {
  links: { ended: AgentPrivacyLink[]; failed: AgentRevokeFailure[] };
  accounts: { disconnected: { connector: string; account: string }[]; failed: AgentRevokeFailure[] };
}

const MAX_TEXT = 400;
const MAX_ITEMS = 200;

const text = (value: unknown): string | undefined => (typeof value === "string" && value.trim() && value.length <= MAX_TEXT ? value.trim() : undefined);
const time = (value: unknown): string | undefined => (typeof value === "string" && !Number.isNaN(Date.parse(value)) ? value : undefined);
const record = (value: unknown): Record<string, unknown> | undefined => (value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined);

function texts(value: unknown): string[] | undefined {
  if (!Array.isArray(value) || value.length > MAX_ITEMS) return undefined;
  const read = value.map(text);
  return read.every((item): item is string => item !== undefined) ? read : undefined;
}

/** `GET` or `PUT /agent/pause`'s body, or undefined when it is not one. */
export function pauseViewOf(body: unknown): AgentPauseView | undefined {
  const view = record(body);
  if (typeof view?.paused !== "boolean") return undefined;
  if (!view.paused) return { paused: false };
  const at = time(view.pausedAt);
  return at ? { paused: true, pausedAt: at } : { paused: true };
}

function linkOf(value: unknown): AgentPrivacyLink | undefined {
  const link = record(value);
  const id = text(link?.id);
  const agent = text(link?.agent);
  const label = text(link?.label);
  const createdAt = time(link?.createdAt);
  if (!id || !agent || !label || !createdAt) return undefined;
  const used = time(link?.lastUsedAt);
  return { id, agent, label, createdAt, ...(used ? { lastUsedAt: used } : {}) };
}

function accountOf(value: unknown): AgentPrivacyAccount | undefined {
  const account = record(value);
  const connector = text(account?.connector);
  const name = text(account?.account);
  const scope = account?.scope;
  const reads = texts(account?.reads);
  const writes = texts(account?.writes);
  const never = texts(account?.never);
  if (!connector || !name || (scope !== "user" && scope !== "org") || !reads || !writes || !never) return undefined;
  const error = text(account?.error);
  return { connector, account: name, scope, reads, writes, never, ...(error ? { error } : {}) };
}

function listOf<T>(value: unknown, read: (item: unknown) => T | undefined): T[] | undefined {
  if (!Array.isArray(value) || value.length > MAX_ITEMS) return undefined;
  const items = value.map(read);
  return items.every((item): item is T => item !== undefined) ? items : undefined;
}

/** `GET /agent/privacy`'s body, or undefined when any part of it is not what the agent API sends. */
export function privacyViewOf(body: unknown): AgentPrivacyView | undefined {
  const view = record(body);
  const pause = pauseViewOf(view?.pause);
  const links = listOf(view?.links, linkOf);
  const accounts = listOf(view?.accounts, accountOf);
  if (!pause || !links || !accounts) return undefined;
  const linksError = text(view?.linksError);
  const accountsError = text(view?.accountsError);
  return { pause, links, accounts, ...(linksError ? { linksError } : {}), ...(accountsError ? { accountsError } : {}) };
}

function failureOf(value: unknown): AgentRevokeFailure | undefined {
  const failure = record(value);
  const id = text(failure?.id);
  const error = text(failure?.error);
  return id && error ? { id, error } : undefined;
}

/** `POST /agent/revoke`'s body, or undefined when it is not one. */
export function revokeResultOf(body: unknown): AgentRevokeResult | undefined {
  const result = record(body);
  const links = record(result?.links);
  const accounts = record(result?.accounts);
  const ended = listOf(links?.ended, linkOf);
  const linksFailed = listOf(links?.failed, failureOf);
  const disconnected = listOf(accounts?.disconnected, (value) => {
    const item = record(value);
    const connector = text(item?.connector);
    const account = text(item?.account);
    return connector && account ? { connector, account } : undefined;
  });
  const accountsFailed = listOf(accounts?.failed, failureOf);
  if (!ended || !linksFailed || !disconnected || !accountsFailed) return undefined;
  return { links: { ended, failed: linksFailed }, accounts: { disconnected, failed: accountsFailed } };
}

/** The paused bar (A11·b): `Paused at 14:02 · nothing runs until you resume`. `clock` formats the time. */
export function pausedLine(view: AgentPauseView, clock: (at: Date) => string): string {
  const at = view.pausedAt ? ` at ${clock(new Date(view.pausedAt))}` : "";
  return `Paused${at} · nothing runs until you resume`;
}

/** The Pause dialog's words (A11·b). */
export const PAUSE_TITLE = "Pause the Assistant?";
export const PAUSE_BODY = "Every errand, watch, brief and message stops at once. Nothing is deleted. Writes already made stay made.";

/** A link as the confirm names it: its label, or the agent's slug. */
function linkName(link: Pick<AgentPrivacyLink, "label" | "agent">): string {
  return link.label || link.agent;
}

/** A list in words: `a`, `a and b`, `a, b and c`. */
function listed(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`;
}

/**
 * The Revoke all access dialog's body: exactly what ends, by name, and what stays. Only the person's own accounts are
 * disconnected; the organization's stay.
 */
export function revokeText(view: Pick<AgentPrivacyView, "links" | "accounts">): string {
  const links = view.links.map(linkName);
  const own = view.accounts.filter((account) => account.scope === "user").map((account) => `${connectorName(account.connector)} (${account.account})`);
  const shared = view.accounts.filter((account) => account.scope === "org").length;
  const parts = [
    links.length ? `Ends ${links.length === 1 ? "your agent link" : `your ${links.length} agent links`}: ${listed(links)}.` : "You have no agent links to end.",
    own.length ? `Disconnects ${own.length === 1 ? "your account" : `your ${own.length} accounts`}: ${listed(own)}.` : "You have no accounts of your own to disconnect.",
    shared ? `Your organization's ${shared === 1 ? "account stays" : `${shared} accounts stay`} connected.` : "",
    "You can reconnect one at a time.",
  ];
  return parts.filter(Boolean).join(" ");
}

/** What Revoke all access did, in one line. A part that failed says so, so the person can try again. */
export function revokedText(result: AgentRevokeResult): string {
  const ended = result.links.ended.length;
  const gone = result.accounts.disconnected.length;
  const failed = result.links.failed.length + result.accounts.failed.length;
  const done = `Ended ${ended} agent ${ended === 1 ? "link" : "links"} and disconnected ${gone} ${gone === 1 ? "account" : "accounts"}.`;
  return failed ? `${done} ${failed} did not end. Try again.` : done;
}
