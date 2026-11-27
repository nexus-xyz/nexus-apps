/**
 * Exact money, for every client of the kit: amounts as integer micro-dollars (a safe integer), read from
 * the API's six-decimal strings. The spend, usage and budget reads send each money number with a
 * `<name>Decimal` twin (`"12.245000"`). A client reads the twin first and falls back to the number. It adds and
 * compares in micro-dollars, and turns an amount back into a JS number only to draw it (a chart's height, a meter's
 * share).
 *
 * The formatters are Nexus UI's rules (`usd()` in shared.ts, `money()` and `budgetMoney()` in usage/model.ts), done on
 * the digits. They round half away from zero, so `"12.245000"` is `$12.25` where the float `12.245` is not.
 */
import type { BudgetsResponse, PersonBudget, UsageDay, UsageFigures } from "./api";

/**
 * An amount in micro-dollars: 1 is $0.000001. A safe integer, so adding and comparing are exact up to $9 billion
 * (`Number.MAX_SAFE_INTEGER` micro-dollars). Not `bigint`: Nexus UI's TypeScript target (ES2017) has no `bigint` literals.
 */
export type Micros = number;

const PER_DOLLAR = 1_000_000;
const PER_CENT = 10_000;
const DECIMAL = /^(-?)(\d+)(?:\.(\d{1,6}))?$/;

/** A decimal string in US dollars (`"12.245000"`) as micro-dollars, exactly; null when it is not one (or has more than six decimals). */
export function parseMicros(value: string): Micros | null {
  const match = DECIMAL.exec(value.trim());
  if (!match) return null;
  const [, sign, whole = "0", fraction = ""] = match;
  const amount = Number(whole) * PER_DOLLAR + Number(fraction.padEnd(6, "0"));
  if (!Number.isSafeInteger(amount)) return null;
  return sign && amount ? -amount : amount;
}

/** A dollar number as micro-dollars, to the nearest one: the fallback for a Gateway that sends no twin. */
export function microsOfNumber(value: number): Micros {
  return Number.isFinite(value) ? Math.round(value * PER_DOLLAR) : 0;
}

/**
 * The amount of one money field: its decimal twin when it parses, else the number beside it, else null. A null twin
 * with no number (rule 9's empty day) is null too.
 */
export function amountOrNull(decimal: string | null | undefined, value: number | null | undefined): Micros | null {
  const exact = typeof decimal === "string" ? parseMicros(decimal) : null;
  if (exact !== null) return exact;
  return typeof value === "number" ? microsOfNumber(value) : null;
}

/** {@link amountOrNull}, with no amount as zero. */
export function amount(decimal: string | null | undefined, value: number | null | undefined): Micros {
  return amountOrNull(decimal, value) ?? 0;
}

export function sumMicros(values: Iterable<Micros>): Micros {
  let total = 0;
  for (const value of values) total += value;
  return total;
}

/** The amount as a JS number of dollars: only for drawing (a chart, a meter), never to add or to show as text. */
export function dollars(value: Micros): number {
  return value / PER_DOLLAR;
}

/** `part / whole` as a number, for a meter or a percent; 0 when `whole` is not above zero. */
export function share(part: Micros, whole: Micros): number {
  return whole > 0 ? part / whole : 0;
}

/** Whether `part` reaches `numerator / denominator` of `whole`, exactly (`0.8` is `4, 5`). */
export function reaches(part: Micros, whole: Micros, numerator: number, denominator: number): boolean {
  return part * denominator >= whole * numerator;
}

/** `value / unit`, rounded half away from zero. */
function roundTo(value: Micros, unit: number): number {
  const rounded = Math.floor((Math.abs(value) + unit / 2) / unit);
  return value < 0 ? -rounded : rounded;
}

const grouped = (digits: number) => String(digits).replace(/\B(?=(\d{3})+(?!\d))/g, ",");

/** A non-negative count of `1 / 10^places` dollars as `"1,234.56"`. */
function fixed(units: number, places: number, group: boolean): string {
  const scale = 10 ** places;
  const whole = Math.floor(units / scale);
  const fraction = String(units % scale).padStart(places, "0");
  return `${group ? grouped(whole) : String(whole)}.${fraction}`;
}

/**
 * `usd()`'s rule, exactly: cents (grouped) from $100 or for whole cents, four decimals from a cent, else six.
 * A negative amount keeps `usd()`'s `$-` form.
 */
export function usdText(value: Micros): string {
  if (value === 0) return "$0.00";
  const sign = value < 0 ? "-" : "";
  const abs = Math.round(Math.abs(value));
  if (abs >= 100 * PER_DOLLAR || abs % PER_CENT === 0) return `$${sign}${fixed(roundTo(abs, PER_CENT), 2, true)}`;
  if (abs >= PER_CENT) return `$${sign}${fixed(roundTo(abs, 100), 4, false)}`;
  return `$${sign}${fixed(abs, 6, false)}`;
}

/** `money()`'s rule (Usage), exactly: whole cents, grouped; `−` before a negative, and `+` too when `signed`. */
export function centsText(value: Micros, signed = false): string {
  const cents = roundTo(value, PER_CENT);
  const text = `$${fixed(Math.abs(cents), 2, true)}`;
  if (cents < 0) return `−${text}`;
  return signed && cents > 0 ? `+${text}` : text;
}

/** `budgetMoney()`'s rule, exactly: whole dollars without cents (`"$100"`), any other amount as {@link centsText}. */
export function budgetText(value: Micros): string {
  const cents = roundTo(value, PER_CENT);
  if (cents % 100 !== 0) return centsText(value);
  const whole = cents / 100;
  return `${whole < 0 ? "−" : ""}$${grouped(Math.abs(whole))}`;
}

/* ---------------------------------------------------------------- the reads' fields */

/** A usage figure's spend (`GET /v1/analytics/usage`: totals, a group, a day): `spentUsdDecimal` first, else `spentUsd`. */
export const spentOf = (figures: Pick<UsageFigures, "spentUsd" | "spentUsdDecimal">): Micros => amount(figures.spentUsdDecimal, figures.spentUsd);

/** One group's spend on one day of a usage report's series: `groupsDecimal` first, else `groups`. */
export const dayGroupOf = (day: Pick<UsageDay, "groups" | "groupsDecimal">, key: string): Micros => amount(day.groupsDecimal?.[key], day.groups?.[key]);

/** The group keys a day of the series carries, in either form. */
export const dayGroupKeys = (day: Pick<UsageDay, "groups" | "groupsDecimal">): string[] => [...new Set([...Object.keys(day.groupsDecimal ?? {}), ...Object.keys(day.groups ?? {})])];

/** A person's budget (`GET /v1/budgets`, `GET /v1/people/{id}/budget`), each figure from its twin first. */
export interface PersonAmounts {
  /** Null: no limit of their own. */
  limit: Micros | null;
  override: Micros | null;
  spent: Micros;
  reserved: Micros;
  remaining: Micros | null;
}

type PersonMoney = "limitUsd" | "limitUsdDecimal" | "overrideUsd" | "overrideUsdDecimal" | "spentUsd" | "spentUsdDecimal" | "reservedUsd" | "reservedUsdDecimal" | "remainingUsd" | "remainingUsdDecimal";

export function personAmounts(person: Partial<Pick<PersonBudget, PersonMoney>>): PersonAmounts {
  return {
    limit: amountOrNull(person.limitUsdDecimal, person.limitUsd),
    override: amountOrNull(person.overrideUsdDecimal, person.overrideUsd),
    spent: amount(person.spentUsdDecimal, person.spentUsd),
    reserved: amount(person.reservedUsdDecimal, person.reservedUsd),
    remaining: amountOrNull(person.remainingUsdDecimal, person.remainingUsd),
  };
}

/** The organization's default monthly limit per person (`GET /v1/budgets`); null: none. */
export const personDefaultOf = (budgets: Pick<BudgetsResponse, "personDefault">): Micros | null => amountOrNull(budgets.personDefault.monthlyUsdDecimal, budgets.personDefault.monthlyUsd);
