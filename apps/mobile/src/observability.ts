import type { AgentAdoptionDay, AgentSessionSummary, SpendCall } from "@nexus/kit";

// Observability on the phone (brand v5's P27·D): the last hour of model calls, the coding agents' runs, and who uses
// them (adoption, which the boards moved here). Only what Gateway reports: it has no latency per call, so no p50 or p95.

const HOUR = 3_600_000;

/** The UTC day before `now`'s, as `YYYY-MM-DD`: the calls' query starts there, so an hour across midnight is whole. */
export const dayBefore = (now: number) => new Date(now - 86_400_000).toISOString().slice(0, 10);

/** Whether a page of calls, newest first, still reaches into the last hour: then the next page is needed too. */
export const reachesIntoHour = (calls: Pick<SpendCall, "at">[], now: number) => calls.length > 0 && Date.parse(calls[calls.length - 1]!.at) >= now - HOUR;

/** The last hour's calls: how many, a minute on average, and how many Gateway refused or that failed. */
export function lastHour(calls: Pick<SpendCall, "at" | "status">[], now: number) {
  const recent = calls.filter((call) => Date.parse(call.at) >= now - HOUR);
  return {
    calls: recent.length,
    perMinute: recent.length / 60,
    refused: recent.filter((call) => call.status === "refused").length,
    failed: recent.filter((call) => call.status === "failed").length,
  };
}

/** Calls a minute as the board writes it: whole from 10, one decimal under it, "0" for none. */
export const perMinuteText = (n: number) => (n === 0 ? "0" : n >= 10 ? String(Math.round(n)) : n.toFixed(1));

/** The newest runs first, running ones before the rest, at most `limit`. */
export function latestRuns<S extends Pick<AgentSessionSummary, "status" | "startedAt">>(sessions: S[], limit = 10): S[] {
  const rank = (s: S) => (s.status === "running" ? 0 : 1);
  return [...sessions].sort((a, b) => rank(a) - rank(b) || Date.parse(b.startedAt) - Date.parse(a.startedAt)).slice(0, limit);
}

/** Over the days given: how many people ran a coding agent through Nexus Gateway, and how many only on their own plan. */
export function adoptionCounts(days: Pick<AgentAdoptionDay, "people" | "subscription">[]) {
  const nexus = new Set(days.flatMap((day) => day.people.map((p) => p.userId)));
  const own = new Set(days.flatMap((day) => day.subscription.map((p) => p.userId)).filter((id) => !nexus.has(id)));
  return { nexus: nexus.size, own: own.size };
}

/** "1 person", "4 people". */
export const people = (n: number) => `${n} ${n === 1 ? "person" : "people"}`;
