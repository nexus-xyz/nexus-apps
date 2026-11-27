import { amount, amountOrNull, parseMicros, usdText, type ProviderKeySummary, type Role, type TeamBudget, type TeamNode } from "@nexus/kit";

/**
 * The words People and Controls read on the phone (brand v5's P105·D and the pages drawn on it): a person's line, a
 * team's line and path, a budget's line, a provider key's state. Plain functions, so vitest can check them.
 */

export const ROLE: Record<Role, string> = { owner: "Owner", admin: "Admin", member: "Member" };

/** Under a person's name (P71e·F, "Owner · Neuro platform"): the role, then the email when the name is shown above it. */
export function memberLine(person: { role: Role; name?: string; email?: string }): string {
  return [ROLE[person.role], person.name && person.email ? person.email : undefined].filter(Boolean).join(" · ");
}

/** "1 person", "14 people". */
export const people = (n: number) => `${n} ${n === 1 ? "person" : "people"}`;

/** Under a team's name (P71c·T5, "14 people · Dev Lal"): its headcount, its leads, and yours when it is your default. */
export function teamLine(team: Pick<TeamNode, "headcount" | "leads" | "isDefault">): string {
  const leads = team.leads.map((l) => l.name || l.email).filter(Boolean);
  return [people(team.headcount), leads.join(", ") || undefined, team.isDefault ? "your default" : undefined].filter(Boolean).join(" · ");
}

/** Where a team sits (P71c·T5's "Inside"): the organization, then its ancestors, "Acme › R&D". */
export const insideLine = (org: string, path: { name: string }[]) => [org, ...path.map((p) => p.name)].filter(Boolean).join(" › ");

type Period = "day" | "week" | "month";
/** "$15,000 a month". */
export const perPeriod = (period: Period) => ({ day: "a day", week: "a week", month: "a month" })[period];
/** "this month", for a figure that resets with the period. */
export const thisPeriod = (period: Period) => ({ day: "today", week: "this week", month: "this month" })[period];

/** "1 Dec", as the boards write a day. */
export const shortDay = (iso: string) => {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? iso.slice(0, 10) : at.toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
};

/**
 * A team's budget as Budgets lists it (P71b·F): the cap on the right, and under the name what it has spent of it, or
 * that it is at its limit and until when. Null when there is no cap, or the caller may not see the team's figures.
 */
export function teamBudgetLine(budget: TeamBudget["budget"]): { cap: string; line: string; atLimit: boolean } | null {
  if (!budget) return null;
  const limit = amountOrNull(budget.limitUsdDecimal, budget.limitUsd);
  if (limit === null) return null;
  const spent = amount(budget.spentUsdDecimal, budget.spentUsd);
  const left = amount(budget.remainingUsdDecimal, budget.remainingUsd);
  const atLimit = left <= 0;
  const after = budget.period === "month" ? "" : ` ${thisPeriod(budget.period)}`;
  return {
    cap: usdText(limit),
    line: atLimit ? `At its limit · new calls stop until ${shortDay(budget.resetsAt)}` : `${usdText(spent)} of ${usdText(limit)}${after}`,
    atLimit,
  };
}

/**
 * A budget as a person types it ("15000", "$15,000.50"): the decimal string `setTeamBudget` reads exactly, or null when
 * it is not an amount above zero.
 */
export function typedAmount(typed: string): string | null {
  const plain = typed.trim().replace(/^\$/, "").replace(/,/g, "");
  if (!/^\d+(\.\d{1,6})?$/.test(plain)) return null;
  const micros = parseMicros(plain);
  return micros !== null && micros > 0 ? plain : null;
}

/** A key's end as the boards mask it (P26·D): "····4F2A". */
export const keyEnd = (last4: string) => `····${last4}`;

const STATE: Record<NonNullable<ProviderKeySummary["state"]>, string | undefined> = {
  valid: undefined,
  rejected: "Rejected",
  unreachable: "Not checked",
  disabled: "Off",
  removed: "Removed",
};

/** A provider key's state on the right of its row (P26·D's "Rejected"): nothing when it works. */
export function keyState(key: Pick<ProviderKeySummary, "state" | "unused">): { text: string; danger: boolean } | undefined {
  const word = key.state ? STATE[key.state] : undefined;
  if (word) return { text: word, danger: key.state === "rejected" };
  return key.unused ? { text: "Unused", danger: false } : undefined;
}

/** What the provider answered when the key was last checked, for a key's page. */
export function checkedLine(key: Pick<ProviderKeySummary, "state" | "checkedAt">): string | undefined {
  if (!key.state) return undefined;
  const word = { valid: "Accepted", rejected: "Rejected by the provider", unreachable: "The provider did not answer", disabled: "Turned off by an admin", removed: "Removed" }[key.state];
  return key.checkedAt ? `${word} · ${shortDay(key.checkedAt)}` : word;
}
