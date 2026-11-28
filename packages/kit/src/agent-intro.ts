/**
 * Assistant's first open (A19), for every app: the agent API's `GET /agent/intro`, read strictly, and the
 * words around it. The web, desktop and the phone show the same first open. The agent API builds the line, the starters
 * and what the agent can read from what is on for the person (`agents/slack/src/api/intro.ts`); this module adds their
 * first name and their organization, which each app already knows.
 *
 * The API answers 404 while its `NEXUS_AGENTS_FIRST_OPEN` is off: every app then opens on its empty chat, as before.
 */

export const INTRO_PATH = "/agent/intro";
export const INTRO_SEEN_PATH = "/agent/intro/seen";
/** The most starters an app shows (A19). */
export const INTRO_STARTERS = 4;
/** What "See what I can do" sends: `discover` lists example asks (A15), with no model call. */
export const DISCOVER_LINE = "discover";

const MAX_LINE = 300;
const MAX_STARTER = 300;
const MAX_READS = 12;
const MAX_NAME = 60;

/** The first open: one line, up to four starters, what the agent can read, and whether each quiet link applies. */
export interface AgentIntro {
  /** What the agent takes, such as `Hand me anything: questions, drafts and research.` */
  line: string;
  /** Lines the person can send as they are. Each asks or reads; none writes. */
  starters: string[];
  /** The connectors the agent can read for the person, by name. Empty for none. */
  reads: string[];
  /** "Connect your accounts" applies: the agent can use connectors here. */
  accounts: boolean;
  /** "See what I can do" applies: `discover` lists example asks. */
  discover: boolean;
}

/** How a read of the first open ended. `seen`: the person saw it once already. `off`: a 404. */
export type AgentIntroRead = { kind: "intro"; intro: AgentIntro } | { kind: "seen" } | { kind: "off" } | { kind: "failed"; status: number };

const text = (value: unknown, max: number): string | undefined => (typeof value === "string" && value.trim() && value.length <= max ? value.trim() : undefined);

/** `GET /agent/intro`'s body, read strictly: the first open, `seen`, or undefined when it is neither. */
export function introOf(body: unknown): AgentIntro | "seen" | undefined {
  const value = body as Record<string, unknown> | null | undefined;
  if (!value || typeof value !== "object") return undefined;
  if (value.firstOpen === false) return "seen";
  if (value.firstOpen !== true) return undefined;
  const line = text(value.line, MAX_LINE);
  if (!line || !Array.isArray(value.starters)) return undefined;
  const starters = [...new Set(value.starters.flatMap((item) => text(item, MAX_STARTER) ?? []))].slice(0, INTRO_STARTERS);
  const reads = Array.isArray(value.reads) ? [...new Set(value.reads.flatMap((item) => text(item, MAX_NAME) ?? []))].slice(0, MAX_READS) : [];
  return { line, starters, reads, accounts: value.accounts === true, discover: value.discover === true };
}

/** The first word of a person's name, for the greeting; undefined when there is none. */
export function firstNameOf(name: string | null | undefined): string | undefined {
  const first = name?.trim().split(/\s+/)[0];
  return first && first.length <= MAX_NAME ? first : undefined;
}

/**
 * The first open's first line: `Hi Maya. Hand me anything: …`, or `Hi, I'm Nexus. …` without a name. `assistant`: the
 * Assistant's name, which the person may change; Nexus by default.
 */
export function introHeadline(intro: AgentIntro, firstName?: string, assistant = "Nexus"): string {
  return `${firstName ? `Hi ${firstName}.` : `Hi, I'm ${assistant}.`} ${intro.line}`;
}

/**
 * What the agent can read, in one line: `Acme's Nexus can read GitHub and Linear.` at work, or `Nexus can read …` in
 * a personal organization or with no name. `assistant`: the Assistant's name, Nexus by default. Undefined when it
 * reads nothing.
 */
export function introReads(intro: AgentIntro, org?: { name?: string; personal?: boolean }, assistant = "Nexus"): string | undefined {
  if (!intro.reads.length) return undefined;
  const owner = org?.name?.trim() && !org.personal ? `${org.name.trim()}'s ${assistant}` : assistant;
  return `${owner} can read ${listed(intro.reads)}.`;
}

/** `a`, `a and b`, `a, b and c`. */
function listed(items: string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}
