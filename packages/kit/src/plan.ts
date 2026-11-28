import type { BillingPlan, BillingResponse, CataloguePlan, ModelEntry, PlanAllowance, NexusPaysReason, SpendResponse } from "./api";
import { BILLING_TERMS, billingCutoff, billingDay, PAY_NOTICE_FALLBACK_TITLE, PAY_NOTICE_TITLE, usageLimitResetsAt, usedUpTitle } from "./billing-words";
import { amount, budgetText, dollars, parseMicros, reaches, usdText, type Micros } from "./money";

// How the organization pays for what Nexus runs, as the phone and Nexus UI read it. Pricing v2.0 sells no plans: a free
// account spends its one-time free start (an internal decision's subsidy), then the owner switches to pay as you go: prepaid credits,
// paying after the fact on the card, or both. Plans stay in the code for when they come back: a Gateway
// with `NEXUS_PLANS` sells them, and an organization on one is `paid`. Each app writes its own words around these.

/**
 * Free (the free start, until billing is set up), pay as you go, a paid plan where this Gateway sells plans, or not
 * known (no answer yet, or not an owner).
 */
export type Plan =
  | { kind: "free" }
  | { kind: "payg"; credits: boolean; afterTheFact: boolean }
  | { kind: "paid"; name: string }
  | { kind: "unknown" };

/** `GET /v1/billing`, or the parts of it this reads. */
type Billed = Pick<Partial<BillingResponse>, "credits" | "afterTheFact"> & { plan?: { plan: string; status: string } | null };

/**
 * Stripe's statuses under which a subscription still holds its plan, paid or not, as Gateway's `holds_the_plan` reads
 * them. `GET /v1/billing` keeps a `canceled` or `incomplete_expired` one, but its organization holds no plan again.
 */
const HOLDS_THE_PLAN = new Set(["active", "trialing", "past_due", "unpaid", "incomplete", "paused"]);

/**
 * When a cancelled plan ends (ISO 8601), null while it renews: Stripe's `cancelAt`, which can fall before the period's
 * end or in a later period, else the period's end. A cancelled plan cannot change until it is renewed.
 */
export function planEnds(plan: Pick<BillingPlan, "currentPeriodEnd" | "cancelAtPeriodEnd" | "cancelAt">): string | null {
  return plan.cancelAt ?? (plan.cancelAtPeriodEnd ? plan.currentPeriodEnd : null);
}

/** `GET /v1/billing`'s plan while its subscription holds it; null without one, and once the subscription is over. */
export function livePlan<P extends { status: string }>(plan: P | null | undefined): P | null {
  return plan && HOLDS_THE_PLAN.has(plan.status) ? plan : null;
}

/** A plan's key as people read it. */
export function planName(plan: string): string {
  switch (plan) {
    case "teams_free": return "Teams Free";
    case "teams_plus": return "Teams Plus";
    case "teams_pro": return "Teams Pro";
    default: {
      const words = plan.replaceAll("_", " ");
      return words.charAt(0).toUpperCase() + words.slice(1);
    }
  }
}

/** A catalogue entry that belongs to Pricing v3. Legacy plans have no included cloud hours. */
export function v3Plan(plan: CataloguePlan): boolean {
  return (plan.includedHours ?? 0) > 0 && ["plus", "pro", "teams_plus", "teams_pro"].includes(plan.plan);
}

const hours = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
const money = (decimal: string) => usdText(parseMicros(decimal) ?? 0);

/** Labels and balances shared by web and phone. */
export function allowanceRows(allowance: PlanAllowance): { label: string; detail: string }[] {
  const rows = [];
  if (allowance.cloud) {
    const { usedHours, leftHours } = allowance.cloud;
    const duration = (value: number) => value > 0 && value < 0.01 ? "<0.01" : hours.format(value);
    rows.push({ label: "Cloud hours", detail: `${duration(usedHours)} h used · ${duration(leftHours)} h left` });
  }
  if (allowance.ai) {
    const { usedUsd, leftUsd } = allowance.ai;
    rows.push({ label: "AI allowance", detail: `${money(usedUsd)} used · ${money(leftUsd)} left` });
  }
  rows.push({ label: "Resets", detail: billingDay(allowance.resetAt) });
  return rows;
}

/**
 * How the organization pays, from `GET /v1/billing`: `null` is a Gateway that does not bill, `undefined` no answer
 * (loading, failed, or not an owner). A live plan comes first; otherwise it pays as it goes once it bought credits or
 * turned paying after the fact on.
 */
export function planOf(personal: boolean, billing: Billed | null | undefined): Plan {
  if (billing === undefined) return { kind: "unknown" };
  const live = livePlan(billing?.plan);
  if (live) return { kind: "paid", name: planName(live.plan) };
  const credits = Boolean(billing?.credits);
  const afterTheFact = Boolean(billing?.afterTheFact?.enabled);
  if (credits || afterTheFact) return { kind: "payg", credits, afterTheFact };
  if (billing !== null || personal) return { kind: "free" };
  return { kind: "unknown" };
}

export interface Usage {
  /** What is left of the free start (or a plan's included usage), at provider cost, in micro-dollars; never below 0. */
  left: Micros;
  granted: Micros;
  /** `left` and `granted` as dollar numbers, for a meter. Show `left` and `granted` as text with `usdText`. */
  leftUsd: number;
  grantedUsd: number;
  /** Nexus cannot pay the next call: `nexusPays` says so, or (an older Gateway) nothing is left and nothing is behind. */
  usedUp: boolean;
  /** The free start pays, and 80% or more of it is used. */
  low: boolean;
  /** This Gateway pays model calls from it (`usableForModelCalls`); if not, each call needs your own key. */
  paysCalls: boolean;
}

/** Spend's parts these read. */
type Spent = Pick<SpendResponse, "subsidy" | "credits" | "payAfterTheFact" | "nexusPays">;

/**
 * The organization's Nexus usage, from `GET /v1/spend`; none for a member, or an organization Nexus never gave any.
 * `nexusPays` decides whether anything still pays: a card that is on can have a failed charge or a reached
 * limit, so `payAfterTheFact` alone does not.
 */
export function usageOf(spend: Spent): Usage | undefined {
  const s = spend.subsidy;
  if (!s) return undefined;
  // Each amount from its decimal twin first. Less than a micro-dollar, the ledger's unit, is nothing left.
  const remaining = amount(s.remainingUsdDecimal, s.remainingUsd);
  const granted = amount(s.grantedUsdDecimal, s.grantedUsd);
  const left = Math.max(0, remaining);
  const pays = spend.nexusPays;
  // An older Gateway: credits left or the card on count as behind the free start.
  const behind = (parseMicros(spend.credits?.remainingUsd ?? "0") ?? 0) >= 1 || Boolean(spend.payAfterTheFact);
  const usedUp = pays ? !pays.pays : remaining < 1 && !behind;
  const freeStartPays = pays ? pays.payer === "free_start" : remaining >= 1 && !behind;
  return {
    left,
    granted,
    leftUsd: dollars(left),
    grantedUsd: dollars(granted),
    usedUp,
    low: freeStartPays && reaches(granted - left, granted, 4, 5),
    paysCalls: s.usableForModelCalls,
  };
}

const DAY_MS = 86_400_000;
/** The composer says when the free start ends from this many days before. */
const EXPIRY_NOTICE_DAYS = 7;

/** The composer says how much of the month's usage limit the card paid from this share, in percent (as a budget's 80%). */
const LIMIT_NEAR_PERCENT = 80;

/**
 * What the composer says about paying, before a send is refused. `refused`: Nexus cannot pay the next
 * call, for `reason` (a card above the composer). `line`: a quiet line, while the free start pays and its end is near,
 * or while the card pays and has paid 80% or more of this month's usage limit (`limitUsedPercent`, until `resetsAt`).
 */
export type PayNotice =
  | { kind: "refused"; reason: NexusPaysReason; granted?: Micros; changesAt?: string }
  | { kind: "line"; granted: Micros; usedPercent?: number; daysLeft?: number }
  | { kind: "line"; limitUsedPercent: number; resetsAt: string };

/** {@link PayNotice} from `GET /v1/spend` at `nowMs`; nothing while Nexus pays and nothing is near its end. */
export function payNoticeOf(spend: Spent, nowMs: number): PayNotice | undefined {
  const usage = usageOf(spend);
  const pays = spend.nexusPays;
  const granted = usage && usage.granted > 0 ? usage.granted : undefined;
  if (pays && !pays.pays && pays.reason) return { kind: "refused", reason: pays.reason, granted, changesAt: pays.changesAt ?? undefined };
  // An older Gateway says nothing of why: only the free start's end is known.
  if (!pays && usage?.usedUp) return { kind: "refused", reason: "free_start_used_up", granted };
  const limitUsed = pays?.payer === "card" ? pays.limitUsedPercent : undefined;
  if (typeof limitUsed === "number" && limitUsed >= LIMIT_NEAR_PERCENT) {
    return { kind: "line", limitUsedPercent: limitUsed, resetsAt: usageLimitResetsAt(new Date(nowMs)) };
  }
  if (!usage || !granted || !(pays ? pays.payer === "free_start" : !usage.usedUp)) return undefined;
  const ends = pays?.changesAt ? Date.parse(pays.changesAt) : NaN;
  const daysLeft = Number.isFinite(ends) && ends - nowMs <= EXPIRY_NOTICE_DAYS * DAY_MS ? Math.max(0, Math.ceil((ends - nowMs) / DAY_MS)) : undefined;
  const usedPercent = usage.low ? Math.floor(((granted - usage.left) * 100) / granted) : undefined;
  if (usedPercent === undefined && daysLeft === undefined) return undefined;
  return { kind: "line", granted, ...(usedPercent !== undefined ? { usedPercent } : {}), ...(daysLeft !== undefined ? { daysLeft } : {}) };
}

/** "Free $1": the free start with its amount from Gateway, or "Free start" when Gateway did not say it. */
const freeStart = (granted?: Micros) => (granted ? `free ${budgetText(granted)}` : "free start");

/** "1 Dec, 00:00 UTC": when a block lifts or the free start ends, as the billing glossary writes a date that decides something. */
export const dayText = billingCutoff;

/**
 * The composer notice's title for each reason, the same words on every app, in Brand v5's billing voice (P73). The apps
 * write the body and the action: what an owner does in Billing, and what a member asks an owner.
 */
export function payNoticeTitle(notice: Extract<PayNotice, { kind: "refused" }>): string {
  switch (notice.reason) {
    case "free_start_used_up":
      return usedUpTitle({ kind: "free" }, notice.granted);
    case "free_start_expired":
      return `Your ${freeStart(notice.granted)} expired`;
    case "credits_out":
      return usedUpTitle({ kind: "payg", credits: true, afterTheFact: false });
    case "usage_limit_reached":
    case "charge_failed":
    case "card_needs_confirming":
    case "card_disputed":
    case "past_due":
    case "billing_off":
    case "charge_processing":
      return PAY_NOTICE_TITLE[notice.reason];
    default:
      // A reason a newer Gateway sends and this build does not know.
      return PAY_NOTICE_FALLBACK_TITLE;
  }
}

/**
 * The quiet line in parts, so each app sets the share apart: what it is (`label`), how much is used (`used`, with
 * `usedPercent` for a meter, at most 100), and when it changes by itself (`when`).
 */
export interface PayLineParts {
  label: string;
  usedPercent?: number;
  used?: string;
  when?: string;
}

export function payLineParts(notice: Extract<PayNotice, { kind: "line" }>): PayLineParts {
  const share = (percent: number) => ({ usedPercent: Math.min(100, Math.max(0, percent)), used: `${percent}% used` });
  if ("limitUsedPercent" in notice) {
    return { label: BILLING_TERMS.usageLimit, ...share(notice.limitUsedPercent), when: `resets on ${billingCutoff(notice.resetsAt)}` };
  }
  const days = notice.daysLeft;
  return {
    label: `Free ${budgetText(notice.granted)}`,
    ...(notice.usedPercent !== undefined ? share(notice.usedPercent) : {}),
    ...(days !== undefined ? { when: days === 0 ? "expires today" : `expires in ${days} ${days === 1 ? "day" : "days"}` } : {}),
  };
}

/** The quiet line: "Free $1 · 82% used · expires in 3 days", or "Usage limit · 85% used · resets on 1 Dec, 00:00 UTC". */
export function payLineText(notice: Extract<PayNotice, { kind: "line" }>): string {
  const { label, used, when } = payLineParts(notice);
  return [label, used, when].filter(Boolean).join(" · ");
}

/**
 * Whether the free start pays for `model` (pricing v2.0): Auto picks a model it pays for, and a named model
 * qualifies when its listed output price is at most `maxUsd` per million tokens (`GET /v1/billing`'s
 * `pricing.freeStartMaxOutputUsd`). Gateway measures the cheapest host, and the catalog lists the first host's price,
 * never below the cheapest; so an app never marks a model that Gateway refuses. `undefined` when the catalog has no
 * price for the model (a catalog cached by an older build).
 */
export function freeStartPays(model: Pick<ModelEntry, "id" | "pricing">, maxUsd: number): boolean | undefined {
  if (model.id === "nexus/auto") return true;
  return model.pricing ? model.pricing.output <= maxUsd : undefined;
}
