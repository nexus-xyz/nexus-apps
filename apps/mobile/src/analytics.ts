import { dayGroupKeys, dayGroupOf, dollars, spentOf, sumMicros, usdText, type Micros, type UsageDay, type UsageFigures, type UsageGroup, type UsageGroupBy } from "@nexus/kit";
import { spendSource } from "./spend";

// Copies of Nexus UI's Analytics helpers (`analytics/page.tsx`, `queries.ts`'s `rangeOf`, `shared.ts`'s
// `spendSourceLabel`); move them into @nexus/kit if a third client needs them.

export type Grouping = Exclude<UsageGroupBy, "day" | "hour">;
export const GROUPINGS: { id: Grouping; label: string }[] = [
  { id: "team", label: "Team" },
  { id: "person", label: "Person" },
  { id: "model", label: "Model" },
  { id: "product", label: "Product" },
];
export const RANGES = [7, 30, 90] as const;
export const NOUNS: Record<Grouping, [string, string]> = { team: ["team", "teams"], person: ["person", "people"], model: ["model", "models"], provider: ["provider", "providers"], key: ["key", "keys"], product: ["product", "products"] };

/** The last `days` UTC days, today included, as `YYYY-MM-DD`. */
export function rangeOf(days: number, now = new Date()) {
  const from = new Date(now.getTime() - (days - 1) * 86_400_000);
  return { from: from.toISOString().slice(0, 10), to: now.toISOString().slice(0, 10) };
}

/** A product group's key (`product/capability`) as people read it: `Chat › Chat`. */
export function sourceLabel(key: string): string {
  const [product = key, capability] = key.split("/");
  const source = spendSource(product, capability);
  return capability ? `${source.product} › ${source.capability}` : source.product;
}

/** The chart's series: the costliest groups in their own colours; one left over keeps its name, more fold into Other. */
export function seriesOf(groups: Pick<UsageGroup, "key" | "label">[], colors: readonly string[], other: string) {
  const series = groups.slice(0, colors.length).map((group, n) => ({ key: group.key, label: group.label, color: colors[n]! }));
  const rest = groups.slice(colors.length);
  if (rest.length === 1) return { series: [...series, { key: rest[0]!.key, label: rest[0]!.label, color: other }], restKeys: new Set<string>() };
  if (rest.length) series.push({ key: "__other", label: `Other (${rest.length})`, color: other });
  return { series, restKeys: new Set(rest.map((group) => group.key)) };
}

/**
 * A day's column: each series' spend that day, Other summing the groups folded into it; empty parts left out. Each
 * group from its decimal twin first, added in micro-dollars; `usd` is the part as a dollar number, only
 * for the column's geometry. Not `value`: Reanimated's Babel plugin warns on every render about a `.value` read inside
 * an inline style.
 */
export function stackOf(day: Pick<UsageDay, "groups" | "groupsDecimal">, series: { key: string; color: string }[], restKeys: Set<string>) {
  return series
    .map((entry) => {
      const spent: Micros = entry.key === "__other" ? sumMicros(dayGroupKeys(day).filter((key) => restKeys.has(key)).map((key) => dayGroupOf(day, key))) : dayGroupOf(day, entry.key);
      return { ...entry, usd: dollars(spent) };
    })
    .filter((part) => part.usd > 0);
}

type Figures = Pick<UsageFigures, "spentUsd" | "spentUsdDecimal">;

/** The chart's days: each day's spend as text and its column's height, 0 to 1 of the costliest day. */
export function chartDays<D extends Figures>(days: D[]) {
  const spent = days.map(spentOf);
  const max = Math.max(0, ...spent);
  return {
    max: usdText(max),
    days: days.map((day, n) => ({ day, spent: usdText(spent[n]!), height: max > 0 ? spent[n]! / max : 0, any: spent[n]! > 0, usd: dollars(spent[n]!) })),
  };
}

/** Analytics' figures as text: the range's spend and its average a call, rounded half away from zero to a micro-dollar. */
export function reportFigures(totals: Figures & Pick<UsageFigures, "calls">) {
  const spent = spentOf(totals);
  return { spent: usdText(spent), average: totals.calls ? usdText(Math.floor((spent + totals.calls / 2) / totals.calls)) : "—" };
}

/** A usage figure's spend as text (a group, a day, a team's totals), from its twin first. */
export const spentText = (figures: Figures): string => usdText(spentOf(figures));

/** A count with thousands separators. */
export const count = (n: number) => n.toLocaleString("en-US");
