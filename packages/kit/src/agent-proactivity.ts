/**
 * The Assistant's proactivity (A10), for every app: the agent API's `GET` and `PUT /agent/proactivity`, read
 * strictly, the dial's words, and "Why am I seeing this?" with its two ways out.
 *
 * - One dial: Quiet, Balanced (the default) and Hands-on. W4.3's `AgentProactivity` maps one to one: `never`, `ask_first`
 *   and `always` (`levelOfAgentProactivity`).
 * - Quiet hours: nothing proactive is sent from `start` to `end` in the person's time zone; it waits.
 * - More than one push in `batchMinutes` becomes one.
 * - "Fewer like this" lowers the dial one step; at Quiet, it turns that kind off. "Turn off" turns that kind off at
 *   every step.
 *
 * The API answers 503 `proactivity_off` while it has no store for it, and an older API answers 404: both read as `off`,
 * and the app shows no dial and no "Why" actions.
 */
import type { AgentProactivity } from "./api";

export const PROACTIVITY_PATH = "/agent/proactivity";

export type ProactivityLevel = "quiet" | "balanced" | "hands_on";
export const PROACTIVITY_LEVELS: readonly ProactivityLevel[] = ["quiet", "balanced", "hands_on"];

/** What a proactive message is about. */
export type ProactiveKind = "watch" | "brief" | "follow-up" | "reminder" | "renewal" | "birthday";
export const PROACTIVE_KINDS: readonly ProactiveKind[] = ["watch", "brief", "follow-up", "reminder", "renewal", "birthday"];

export interface QuietHours {
  /** `HH:MM`, 24-hour. */
  start: string;
  end: string;
  /** An IANA time zone. */
  timezone: string;
}

/** `GET /agent/proactivity`'s answer. */
export interface AgentProactivityView {
  level: ProactivityLevel;
  ownLevel: boolean;
  agentProactivity: AgentProactivity;
  muted: ProactiveKind[];
  quietHours: QuietHours | null;
  batchMinutes: number;
}

/** A `PUT`'s body: each field present is set; `null` resets it. */
export interface ProactivityChange {
  level?: ProactivityLevel | null;
  muted?: ProactiveKind[];
  quietHours?: QuietHours | null;
}

/** The dial's steps, as A10·a draws them. */
export const PROACTIVITY_DIAL: readonly { level: ProactivityLevel; label: string; does: string; perDay: string }[] = [
  { level: "quiet", label: "Quiet", does: "Only what you asked for and what's urgent. No brief.", perDay: "≈ 2 a day" },
  { level: "balanced", label: "Balanced", does: "A morning brief, reminders, follow-ups that matter.", perDay: "≈ 6 a day" },
  { level: "hands_on", label: "Hands-on", does: "Also an evening wrap-up, ideas and nudges it notices.", perDay: "≈ 12 a day" },
];

/** Each kind's name in "Turn off …". */
export const KIND_NAMES: Record<ProactiveKind, string> = {
  watch: "watches",
  brief: "the morning brief",
  "follow-up": "follow-ups",
  reminder: "reminders",
  renewal: "renewals",
  birthday: "birthdays",
};

/** W4.3's `AgentProactivity` as a step of the dial. */
export function levelOfAgentProactivity(proactivity: AgentProactivity): ProactivityLevel {
  return proactivity === "never" ? "quiet" : proactivity === "always" ? "hands_on" : "balanced";
}

const CLOCK = /^([01]\d|2[0-3]):[0-5]\d$/;
const isLevel = (value: unknown): value is ProactivityLevel => PROACTIVITY_LEVELS.includes(value as ProactivityLevel);
const isKind = (value: unknown): value is ProactiveKind => PROACTIVE_KINDS.includes(value as ProactiveKind);

function quietHoursOf(value: unknown): QuietHours | null | undefined {
  if (value === null) return null;
  if (!value || typeof value !== "object") return undefined;
  const { start, end, timezone } = value as Record<string, unknown>;
  if (typeof start !== "string" || typeof end !== "string" || !CLOCK.test(start) || !CLOCK.test(end) || typeof timezone !== "string" || !timezone || timezone.length > 64) return undefined;
  return { start, end, timezone };
}

/** `GET /agent/proactivity`'s body, read strictly; undefined when it is not one. */
export function proactivityViewOf(body: unknown): AgentProactivityView | undefined {
  if (!body || typeof body !== "object") return undefined;
  const { level, ownLevel, agentProactivity, muted, quietHours, batchMinutes } = body as Record<string, unknown>;
  if (!isLevel(level) || typeof ownLevel !== "boolean" || !Array.isArray(muted) || !muted.every(isKind)) return undefined;
  if (agentProactivity !== "always" && agentProactivity !== "ask_first" && agentProactivity !== "never") return undefined;
  const hours = quietHoursOf(quietHours);
  if (hours === undefined || typeof batchMinutes !== "number" || !Number.isInteger(batchMinutes) || batchMinutes < 0) return undefined;
  return { level, ownLevel, agentProactivity, muted, quietHours: hours, batchMinutes };
}

/** How a read of the proactivity ended. `off`: the API has none to give. */
export type AgentProactivityRead = { kind: "view"; view: AgentProactivityView } | { kind: "off" } | { kind: "failed"; status: number };

/** A read from its status and body: a 404 or 503 `proactivity_off` is `off`, a body that is no view failed. */
export function proactivityRead(status: number, body: unknown): AgentProactivityRead {
  const error = body && typeof body === "object" ? (body as { error?: unknown }).error : undefined;
  if (status === 404 || (status === 503 && error === "proactivity_off")) return { kind: "off" };
  if (status < 200 || status >= 300) return { kind: "failed", status };
  const view = proactivityViewOf(body);
  return view ? { kind: "view", view } : { kind: "failed", status: 502 };
}

/** "Fewer like this": the dial one step down, or at Quiet, that kind turned off. */
export function fewerLikeThis(view: Pick<AgentProactivityView, "level" | "muted">, kind: ProactiveKind): ProactivityChange {
  const lower = PROACTIVITY_LEVELS[PROACTIVITY_LEVELS.indexOf(view.level) - 1];
  return lower ? { level: lower } : turnOff(view, kind);
}

/** "Turn off": that kind off at every step. */
export function turnOff(view: Pick<AgentProactivityView, "muted">, kind: ProactiveKind): ProactivityChange {
  return { muted: PROACTIVE_KINDS.filter((each) => each === kind || view.muted.includes(each)) };
}

/** "Why am I seeing this?" (A10·b): what the message is about, the step it came at, and the reasons in its language. */
export interface AgentWhy {
  kind: ProactiveKind;
  level: ProactivityLevel;
  lines: string[];
}

/** A message's `why`, read strictly: at most four lines of text. */
export function whyOf(value: unknown): AgentWhy | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { kind, level, lines } = value as Record<string, unknown>;
  if (!isKind(kind) || !isLevel(level) || !Array.isArray(lines) || !lines.length || lines.length > 4) return undefined;
  if (!lines.every((line) => typeof line === "string" && line.trim() && line.length <= 400)) return undefined;
  return { kind, level, lines: (lines as string[]).map((line) => line.trim()) };
}
