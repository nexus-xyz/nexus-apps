/**
 * Discover (A15), for every app: what Nexus can do, and how to let it do more. This module keeps the static
 * words: the ideas under "Try these", what each connector unlocks under "Connect more", and the lines under "What's
 * new". Each app reads its live parts itself: the connector catalog, the approval settings and the memory.
 *
 * Discover grows with the agent. A new capability or connector adds an idea here and a line to `DISCOVER_NEWS`. Each
 * idea is a line the agent answers today (`agents/core/src/capabilities.ts`), so "Try it" never promises more.
 *
 * The autonomy ladder (A15·c) is here too: the three approval tiers, with the person's spend line and the kinds that
 * still ask first, from `GET /agent/approvals/settings`.
 */
import type { AgentApprovalRule, AgentApprovalsView } from "./agent-chat";

/** The Assistant's view that shows Discover: `?view=discover`. */
export const DISCOVER_VIEW = "discover";

/** One idea under "Try these": a short title, and the line Nexus gets as the person's message. */
export interface DiscoverIdea {
  id: string;
  title: string;
  line: string;
  /** The connector the line reads, when it needs one. Without it connected, Nexus answers with a Connect card. */
  connector?: string;
}

export const DISCOVER_IDEAS: readonly DiscoverIdea[] = [
  { id: "prep-meeting", title: "Prep a meeting", line: "What is on my calendar tomorrow?", connector: "google-calendar" },
  { id: "tidy-inbox", title: "Tidy my inbox", line: "Summarize my unread Gmail threads", connector: "gmail" },
  { id: "draft-reply", title: "Draft a reply", line: "Draft a polite reply that moves our Thursday meeting to Friday" },
  { id: "my-issues", title: "Catch up on work", line: "List my open Linear issues", connector: "linear" },
  { id: "my-pulls", title: "Check my reviews", line: "List my open pull requests on GitHub", connector: "github" },
  { id: "find-doc", title: "Find a doc", line: "Find the Notion page about onboarding", connector: "notion" },
  { id: "my-spend", title: "Check my AI spend", line: "How much did I spend on AI last week?" },
  { id: "what-it-knows", title: "Ask what it knows", line: "What do you remember about me?" },
];

/**
 * What connecting each account unlocks, in the board's words: "Unlocks: …". A connector this map does not name reads
 * its catalog line instead.
 */
export const CONNECTOR_UNLOCKS: Readonly<Record<string, string>> = {
  asana: "List and track your open tasks",
  confluence: "Find pages and answer from them",
  github: "Your pull requests, issues and reviews",
  gitlab: "Your merge requests and issues",
  gmail: "Summarize your mail and draft replies",
  "google-calendar": "Read your day and prep your meetings",
  "google-drive": "Find docs and answer from them",
  hubspot: "Read your deals and contacts",
  jira: "List and track your open issues",
  linear: "List and track your open issues",
  notion: "Find pages and answer from them",
  salesforce: "Read your accounts and opportunities",
  slack: "Read your channels and threads",
  zendesk: "List your open tickets",
};

/** Where a line of "What's new" takes the person: a line sent to Nexus, or another view of the Assistant. */
export type DiscoverNewsAction = { kind: "ask"; line: string } | { kind: "view"; view: "chat" | "errands" | "memory" | "approvals" };

/** One line of "What's new": shown with a dot until the person has seen it. */
export interface DiscoverNews {
  id: string;
  title: string;
  line: string;
  action: DiscoverNewsAction;
}

/** Newest first. Keep each id: the dot reads them. */
export const DISCOVER_NEWS: readonly DiscoverNews[] = [
  { id: "2026-11-brief", title: "Your morning brief", line: "Today's thread starts with what changed overnight.", action: { kind: "view", view: "chat" } },
  { id: "2026-11-hand-off", title: "Hand off from ⌘K", line: "Type anything in ⌘K, then press ⌘J. Nexus takes it from there.", action: { kind: "view", view: "chat" } },
  { id: "2026-11-every-surface", title: "One conversation, every surface", line: "Start in Slack, go on here. The list beside the chat has them all.", action: { kind: "view", view: "chat" } },
  { id: "2026-11-memory", title: "Correct what it knows", line: "Add a note, edit one, or pause learning. It forgets at once.", action: { kind: "view", view: "memory" } },
];

/** The ids of `news` the person has not seen yet, in order. `seen` is what this device keeps. */
export function unseenNews(seen: readonly string[], news: readonly DiscoverNews[] = DISCOVER_NEWS): string[] {
  const known = new Set(seen);
  return news.filter((item) => !known.has(item.id)).map((item) => item.id);
}

/** The seen ids as this device keeps them: a JSON array of strings, or none. */
export function seenNewsOf(stored: string | null | undefined): string[] {
  if (!stored) return [];
  try {
    const value: unknown = JSON.parse(stored);
    return Array.isArray(value) ? value.filter((id): id is string => typeof id === "string").slice(0, 200) : [];
  } catch {
    return [];
  }
}

/** "Unlocks: …" for a connector: its own line from `CONNECTOR_UNLOCKS`, else `fallback` (its catalog line). */
export function unlocksOf(connector: string, fallback: string): string {
  return CONNECTOR_UNLOCKS[connector] ?? fallback;
}

/** Each kind that can ask first, as the person reads it on the board (A9·a). */
const APPROVAL_KINDS: Record<string, string> = {
  send: "Send or reply",
  book: "Book",
  spend: "Spend",
  rsvp: "Accept or decline invites",
  "share-inside": "Share inside your organization",
};

/** An approval rule in words: what it lets the agent do, and to whom. A write of no known kind is named by its operation. */
export function approvalRuleText(rule: Pick<AgentApprovalRule, "kind" | "to">): { what: string; to: string } {
  const what = rule.kind.startsWith("write:") ? rule.kind.slice("write:".length) : (APPROVAL_KINDS[rule.kind] ?? rule.kind);
  return { what, to: rule.to ? `To ${rule.to}` : "When it names no one" };
}

/** A decimal string of dollars as the person reads it: `$50` for whole dollars, `$49.99` otherwise. */
export function approvalDollars(amount: string): string {
  return `$${amount.endsWith(".00") ? amount.slice(0, -3) : amount}`;
}

/** One rung of Discover's autonomy ladder (A15·c). */
export interface Rung {
  id: "always-asks" | "asks-first" | "freely";
  title: string;
  /** The quiet word beside the title. */
  tag: string;
  line: string;
}

/**
 * The ladder, top to bottom: what always asks (fixed), what asks first (where most things are), and what Nexus does
 * freely. The spend line is the person's own when the view has one.
 */
export function autonomyLadder(view?: Pick<AgentApprovalsView, "spendLimitUsd">): Rung[] {
  const line = view ? approvalDollars(view.spendLimitUsd) : "your line";
  return [
    { id: "always-asks", title: "Always asks", tag: "Fixed", line: `Spending over ${line}, passwords, sharing outside, deleting. You confirm it in Nexus.` },
    { id: "asks-first", title: "Asks first", tag: "Where most things are", line: `Sending, booking, spending up to ${line}, invites.` },
    { id: "freely", title: "Does freely", tag: "Safe to leave on", line: "Reading, drafting, reminders, tracking." },
  ];
}

/** The kinds that ask first, in the order the next safe step reads them. */
const NEXT_KINDS = ["send", "rsvp", "share-inside", "book", "spend"] as const;

/**
 * The safe next steps: each kind that asks first and has no "Always allow" rule yet. A rule comes from a card's
 * "Always allow this kind", so each step says to choose it on the next card of that kind.
 */
export function safeNextSteps(view: Pick<AgentApprovalsView, "rules">): { kind: string; what: string }[] {
  const allowed = new Set(view.rules.map((rule) => rule.kind));
  return NEXT_KINDS.filter((kind) => !allowed.has(kind)).map((kind) => ({ kind, what: approvalRuleText({ kind }).what }));
}

/** The approvals summary on the section's overview: "Asks first", or "Every write asks" where every write gets a card. */
export function autonomySummary(view?: Pick<AgentApprovalsView, "tiers" | "rules">): string {
  if (!view) return "Asks first";
  if (view.tiers === "off") return "Every write asks";
  return view.rules.length ? `${view.rules.length} always allowed` : "Asks first";
}
