/**
 * The Assistant's morning brief (A1), for every app: the agent API's `GET /agent/brief/today`, read strictly.
 * The brief is the one Slack's DM got that morning. An app shows it at the top of today's thread: the greeting with the
 * count, up to three lines, then the errands that need the person in one line. The agent API writes the words
 * (`agents/slack/src/api/brief.ts`).
 *
 * The API answers 503 `brief_off` while the brief is off for it, and an older API answers 404: both read as `off`, and
 * the app shows no brief.
 */
import { whyOf, type AgentWhy } from "./agent-proactivity";

export const BRIEF_PATH = "/agent/brief/today";
/** The most lines a brief has (A1: "three lines, then the errands in one line"). */
export const BRIEF_LINES = 3;

const MAX_TEXT = 400;
const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** Today's brief, as an app draws it. */
export interface AgentBrief {
  /** The brief's local day, `YYYY-MM-DD`. */
  day: string;
  /** When it was composed (ISO 8601). */
  at: string;
  /** `Morning, Maya. Three things today:` */
  head: string;
  lines: string[];
  /** The errands that need the person, in one line. */
  errands?: string;
  /** "Why am I seeing this?" (A10). An older API, or one without proactivity, gives none. */
  why?: AgentWhy;
}

/** How a read of the brief ended. `none`: no brief today. `off`: the API has none to give. */
export type AgentBriefRead = { kind: "brief"; brief: AgentBrief } | { kind: "none" } | { kind: "off" } | { kind: "failed"; status: number };

const text = (value: unknown): string | undefined => (typeof value === "string" && value.trim() && value.length <= MAX_TEXT ? value.trim() : undefined);

/**
 * `GET /agent/brief/today`'s body, read strictly: the brief, `null` for none today, or undefined when it is neither. A
 * brief with a line that is not text, or more than three lines, is not one.
 */
export function briefOf(body: unknown): AgentBrief | null | undefined {
  if (!body || typeof body !== "object" || !("brief" in body)) return undefined;
  const brief = (body as { brief: unknown }).brief;
  if (brief === null) return null;
  if (!brief || typeof brief !== "object") return undefined;
  const { day, at, head, lines, errands, why } = brief as Record<string, unknown>;
  if (typeof day !== "string" || !DAY.test(day) || typeof at !== "string" || Number.isNaN(Date.parse(at))) return undefined;
  const greeting = text(head);
  if (!greeting || !Array.isArray(lines) || lines.length > BRIEF_LINES) return undefined;
  const read = lines.map(text);
  if (read.some((line) => line === undefined)) return undefined;
  if (errands !== undefined && text(errands) === undefined) return undefined;
  const line = text(errands);
  // A reason that is not one is left out: the brief still shows.
  const reason = whyOf(why);
  return { day, at, head: greeting, lines: read as string[], ...(line ? { errands: line } : {}), ...(reason ? { why: reason } : {}) };
}

/** A read of the brief, from its status and body: a 404 or 503 `brief_off` is `off`, a body that is no brief failed. */
export function briefRead(status: number, body: unknown): AgentBriefRead {
  const error = body && typeof body === "object" ? (body as { error?: unknown }).error : undefined;
  if (status === 404 || (status === 503 && error === "brief_off")) return { kind: "off" };
  if (status < 200 || status >= 300) return { kind: "failed", status };
  const brief = briefOf(body);
  if (brief === null) return { kind: "none" };
  return brief ? { kind: "brief", brief } : { kind: "failed", status: 502 };
}
