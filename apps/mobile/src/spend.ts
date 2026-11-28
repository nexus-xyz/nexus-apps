import { amount, amountOrNull, personAmounts, share, sumMicros, usdText, type Micros, type ModuleManifest, type PersonBudget, type SpendEntry, type SpendResponse } from "@nexus/kit";

// A copy of Nexus UI's `spendSource` (shared.ts, beside a Next-only import) and `leftForYou`
// (spend-card.ts); move them into @nexus/kit if a third client needs them.
//
// Money here is exact: each field is read from its `<name>Decimal` twin first, else its number, as
// integer micro-dollars from @nexus/kit's money.ts. Sums and comparisons stay in micro-dollars; text comes from
// `usdText`, and only the meters' and share bars' fractions are JS numbers.

/**
 * Where a call's spend is attributed, as people read it: `core` is a direct call to the gateway (shown as Gateway),
 * `chat` the chat, `agents` Claude Code and Codex through `nexus claude` and `nexus codex`; a module's own slugs as
 * its manifest names them, else title-cased.
 */
export function spendSource(product: string, capability?: string, manifests: Pick<ModuleManifest, "product" | "capability">[] = []): { product: string; capability: string } {
  const manifest = manifests.find((m) => m.product.slug === product && m.capability.slug === capability);
  if (manifest) return { product: manifest.product.name, capability: manifest.capability.name };
  const known: Record<string, { product: string; capability: string }> = {
    core: { product: "Gateway", capability: "Direct API calls" },
    chat: { product: "Chat", capability: "Chat" },
    agents: { product: "Coding agents", capability: "nexus claude and nexus codex" },
  };
  const title = (slug: string) => slug.charAt(0).toUpperCase() + slug.slice(1).replace(/-/g, " ");
  return known[product] ?? { product: title(product), capability: capability ? title(capability) : "" };
}

type ProductRow = SpendResponse["byProduct"][number];

/** A cost on Spend (a product row, a recent call): `costUsdDecimal` first, else `costUsd`. */
export const costOf = (row: Pick<ProductRow | SpendEntry, "costUsd" | "costUsdDecimal">): Micros => amount(row.costUsdDecimal, row.costUsd);

/** Spend by product, the costliest first, each with its cost as text and its share of the exact total (0 to 1). */
export function productRows(byProduct: SpendResponse["byProduct"]) {
  const costs = byProduct.map((row) => ({ row, cost: costOf(row) }));
  const total = sumMicros(costs.map((entry) => entry.cost));
  return costs.sort((a, b) => b.cost - a.cost).map(({ row, cost }) => ({ ...row, cost: usdText(cost), share: share(cost, total) }));
}

/** Whether the answer carries the organization's budget (owners and admins): either form of it. */
const hasBudget = (spend: Pick<SpendResponse, "budgetUsd" | "budgetUsdDecimal">) => spend.budgetUsd !== undefined || spend.budgetUsdDecimal !== undefined;

/**
 * What a member sees in place of the organization's budget: only what is left for them, and when the cap that binds it
 * resets. `undefined` when the answer carries the organization's budget (owners and admins), shown as it always was.
 */
export function leftForYou(spend: SpendResponse): { label: string; left: Micros; note?: string; subsidized?: Micros } | undefined {
  if (hasBudget(spend)) return undefined;
  const subsidized = amount(spend.subsidizedUsdDecimal, spend.subsidizedUsd);
  return {
    label: "Left for you",
    left: amount(spend.remainingUsdDecimal, spend.remainingUsd),
    ...(spend.resetsAt ? { note: `Resets ${spend.resetsAt.slice(0, 10)} (UTC)` } : {}),
    // What Nexus paid for their own calls; the organization's Nexus subsidy is not in a member's answer.
    ...(subsidized ? { subsidized } : {}),
  };
}

/** The meter's two parts, as fractions of the budget, each kept within 0 to 1. Amounts in micro-dollars. */
export function meter(budget: Micros, spent: Micros, reserved: Micros) {
  if (budget <= 0) return { spent: 0, reserved: 0 };
  const used = Math.min(1, Math.max(0, share(spent, budget)));
  return { spent: used, reserved: Math.min(1 - used, Math.max(0, share(reserved, budget))) };
}

/** The organization's budget, spent, reserved and what is left, in micro-dollars: each twin first, else its number. */
export function spendAmounts(spend: SpendResponse) {
  return {
    budget: amount(spend.budgetUsdDecimal, spend.budgetUsd),
    spent: amount(spend.spentUsdDecimal, spend.spentUsd),
    reserved: amount(spend.reservedUsdDecimal, spend.reservedUsd),
    remaining: amount(spend.remainingUsdDecimal, spend.remainingUsd),
  };
}

/** Spend's and Gateway's budget card (owners and admins): its figures as text, the percent spent, and the meter. */
export function budgetCard(spend: SpendResponse) {
  const { budget, spent, reserved, remaining } = spendAmounts(spend);
  return {
    budget: usdText(budget),
    spent: usdText(spent),
    reserved: usdText(reserved),
    remaining: usdText(remaining),
    // Hundredths of a percent, rounded from the integers: 12.245 of 100 is 12.25%, where the float says 12.24%.
    percent: budget > 0 ? `${(Math.round((spent * 10_000) / budget) / 100).toFixed(2)}% of budget` : undefined,
    meter: meter(budget, spent, reserved),
  };
}

/** Spend's "Your limit this month": null when they have no limit of their own. */
export function limitCard(person: Partial<PersonBudget>) {
  const { limit, spent, reserved, remaining } = personAmounts(person);
  if (limit === null) return null;
  return {
    limit: usdText(limit),
    spent: usdText(spent),
    reserved: usdText(reserved),
    remaining: remaining === null ? "None" : usdText(remaining),
    meter: meter(limit, spent, reserved),
  };
}

/** Spend's free start; null without one. Its meter shows what is left, as the sidebar does. */
export function creditCard(spend: SpendResponse) {
  const s = spend.subsidy;
  if (!s) return null;
  const granted = amount(s.grantedUsdDecimal, s.grantedUsd);
  const spent = amount(s.spentUsdDecimal, s.spentUsd);
  const reserved = amount(s.reservedUsdDecimal, s.reservedUsd);
  const remaining = amount(s.remainingUsdDecimal, s.remainingUsd);
  return {
    granted: usdText(granted),
    spent: usdText(spent),
    remaining: usdText(remaining),
    subsidized: usdText(amount(spend.subsidizedUsdDecimal, spend.subsidizedUsd)),
    reserved: reserved > 0 ? usdText(reserved) : null,
    left: { spent: share(Math.max(0, remaining), granted), reserved: 0 },
  };
}

/** Settings' budget line beside the role; undefined when the answer has neither a member's cap nor a budget above 0. */
export function settingsLine(spend: SpendResponse): string | undefined {
  const mine = leftForYou(spend);
  if (mine) return `${usdText(mine.left)} left for you`;
  const budget = amountOrNull(spend.budgetUsdDecimal, spend.budgetUsd);
  return budget ? `${usdText(amount(spend.remainingUsdDecimal, spend.remainingUsd))} of ${usdText(budget)} left` : undefined;
}

/** Usage's hero (brand v5's P71f·E): what the spent figure counts, by the budget's period ("Spent in November"). */
export function spentLabel(period: SpendResponse["budgetPeriod"], now = new Date()): string {
  if (period === "day") return "Spent today";
  if (period === "week") return "Spent this week";
  return `Spent in ${now.toLocaleDateString("en-US", { month: "long", timeZone: "UTC" })}`;
}

/** The first day of this UTC month and today, as `YYYY-MM-DD`: Usage's teams count this month. */
export function thisMonth(now = new Date()) {
  const today = now.toISOString().slice(0, 10);
  return { from: `${today.slice(0, 8)}01`, to: today };
}
