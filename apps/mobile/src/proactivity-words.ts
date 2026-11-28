import { KIND_NAMES, PROACTIVITY_DIAL, type AgentProactivityRead, type AgentProactivityView, type ProactivityLevel, type QuietHours } from "@nexus/kit";

/**
 * Proactivity on the phone (A10), without React Native: the dial's rows, quiet hours from two typed times,
 * and what the page says after each change. The wire types, the strict reader and the dial's words are the kit's
 * (`agent-proactivity.ts`); the rest are Nexus UI's words (`agent-proactivity.tsx` there).
 */

/** Quiet hours when the person first turns them on. */
export const DEFAULT_QUIET_HOURS = { start: "22:00", end: "07:00" } as const;

const CLOCK = /^([01]\d|2[0-3]):[0-5]\d$/;

/** A typed time as `HH:MM`: "7:30" and "0730" read as "07:30"; undefined when it is no time of day. */
export function clockOf(typed: string): string | undefined {
  const digits = typed.trim().replace(/[.h]/gi, ":");
  const match = /^(\d{1,2}):?(\d{2})$/.exec(digits);
  if (!match) return undefined;
  const clock = `${match[1].padStart(2, "0")}:${match[2]}`;
  return CLOCK.test(clock) ? clock : undefined;
}

/** Quiet hours from two typed times, or why not. */
export function quietHoursOf(start: string, end: string, timezone: string): QuietHours | string {
  const from = clockOf(start);
  const to = clockOf(end);
  if (!from || !to) return "Enter both times as hours and minutes, such as 22:00.";
  if (from === to) return "Quiet hours start and end at different times.";
  return { start: from, end: to, timezone };
}

/** A dial step's label: "Balanced". */
export const levelLabel = (level: ProactivityLevel): string => PROACTIVITY_DIAL.find((step) => step.level === level)?.label ?? level;

/** A dial step's line under its label: what it sends, and about how often. */
export const stepLine = (step: (typeof PROACTIVITY_DIAL)[number]): string => `${step.does} ${step.perDay}`;

/** What the page says once the dial moved. */
export const dialText = (view: AgentProactivityView): string => `Proactive is set to ${levelLabel(view.level)}.`;

/** What the page says once quiet hours changed. */
export const quietText = (view: AgentProactivityView): string => (view.quietHours ? `Quiet from ${view.quietHours.start} to ${view.quietHours.end}.` : "Quiet hours are off.");

/** The kinds turned off at every step, in words; undefined with none. */
export function mutedLine(view: AgentProactivityView): string | undefined {
  return view.muted.length ? `Off at every step: ${view.muted.map((kind) => KIND_NAMES[kind]).join(", ")}.` : undefined;
}

/** The page's foot: how pushes batch, and what always comes. */
export const batchLine = (view: AgentProactivityView): string => `More than one push in ${view.batchMinutes} minutes becomes one. Approvals and what you asked for always come.`;

/** What a save that did not end in a view says. */
export function failedText(read: Exclude<AgentProactivityRead, { kind: "view" }>): string {
  return read.kind === "off" ? "Proactivity is not on for Nexus here yet." : "That was not saved. Try again in a moment.";
}
