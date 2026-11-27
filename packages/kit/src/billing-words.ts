/**
 * The billing glossary: one name for each billing concept, one money format and one date format, for
 * every surface. Nexus UI, the phone and the TypeScript CLI import these. The desktop app and the Rust CLI cannot, so
 * `fixtures/billing-words.json` holds the same strings and examples, and their tests check them against it.
 *
 * Billing dates are UTC: Gateway closes a month, and resets the usage limit, at midnight UTC. A date that decides
 * something (when calls stop, when the limit resets) carries its UTC time: "1 Dec, 00:00 UTC".
 */
import type { AfterTheFact, AutoTopup, NexusPaysReason, PricingChange } from "./api";
import { budgetText, parseMicros, type Micros } from "./money";
import type { Plan } from "./plan";

/** One name for each billing concept. Change a name here and in `fixtures/billing-words.json` together. */
export const BILLING_TERMS = {
  /**
   * What Nexus gives once to start with (an internal decision's subsidy). Never "welcome credit", "free credit(s)" or "Nexus credit".
   * User copy also never names Gateway: it says Nexus.
   */
  freeStart: "Free start",
  /** Referral rewards and other grants from Nexus, apart from the free start. */
  promotionalCredit: "Promotional credit",
  /** Credits the organization bought. Never "purchased credits" or plain "credits" as a label. */
  credits: "Nexus credits",
  /** The card pays for what Nexus ran, after it ran. */
  payAfterTheFact: "Pay after the fact",
  /** The most the card pays after the fact in a UTC month. Never "card spending limit" or "card limit". */
  usageLimit: "Usage limit",
  /** The action that changes the usage limit, on every surface. */
  changeLimit: "Change limit",
  /** Buying more Nexus credits when the balance falls under a line. */
  autoTopup: "Auto top-up",
  /** Credits, paying after the fact, or both. */
  payAsYouGo: "Pay as you go",
  /** What the free start paid for a call, on Spend and in the CLI. Never "subsidized by Nexus". */
  paidByFreeStart: "Paid by the free start",
  /** The note under a page of billing dates. */
  datesInUtc: "Billing dates are in UTC.",
  /** The action that opens Stripe's page for an unpaid invoice, on every surface that may take a payment. */
  payInvoice: "Pay invoice",
  /** The action that saves another card on Stripe's page, for an expired card or an unpaid invoice. */
  updateCard: "Update card",
  /** The action after a charge failed: save a card that pays, on Stripe's page. */
  fixPayment: "Fix payment",
  /** The action after a payment on the card was disputed: that card never pays again, so another one must. */
  addAnotherCard: "Add another card",
  /** The action for a charge the bank wants the cardholder to confirm (3-D Secure): the same charge, on the bank's page. */
  confirmPayment: "Confirm payment",
  /**
   * The action that starts pay as you go, for a free account (brand v5 P100, P73). Never "Set up pay as you go",
   * "Choose how to pay" or "Switch to pay as you go".
   */
  setUpBilling: "Set up billing",
  /**
   * The action that buys Nexus credits, as `nexus billing credits buy` and brand v5's P73 say it. Never "Add credits":
   * a purchase charges the card.
   */
  buyCredits: "Buy credits",
  /** The action that opens Billing when no other action fixes the state. */
  openBilling: "Open billing",
} as const;

/** One action per reason Nexus cannot pay the next call (`nexusPays.reason`), named as {@link BILLING_TERMS} names it. */
export const PAY_NOTICE_ACTION: Record<NexusPaysReason, keyof typeof BILLING_TERMS> = {
  free_start_used_up: "setUpBilling",
  free_start_expired: "setUpBilling",
  billing_off: "setUpBilling",
  credits_out: "buyCredits",
  usage_limit_reached: "changeLimit",
  charge_failed: "fixPayment",
  card_needs_confirming: "confirmPayment",
  card_disputed: "addAnotherCard",
  past_due: "payInvoice",
  charge_processing: "buyCredits",
};

/** {@link PAY_NOTICE_ACTION}'s action for `reason`; "Open billing" for a reason this build does not know (a newer Gateway's). */
export function payNoticeActionTerm(reason: string): keyof typeof BILLING_TERMS {
  return (PAY_NOTICE_ACTION as Record<string, keyof typeof BILLING_TERMS | undefined>)[reason] ?? "openBilling";
}

/**
 * Why Nexus turned auto top-up off (`autoTopup.offReason`), the same words on every surface: the billing
 * page's row and banner, the credit dialog, the phone, the desktop app and both CLIs. Change them here and in
 * `fixtures/billing-words.json` together.
 */
export const TOPUP_OFF_REASON: Record<NonNullable<AutoTopup["offReason"]>, string> = {
  closed: "the organization was closed",
  declined: "the card was declined",
  authentication_required: "the bank wanted the cardholder to confirm the payment",
  payment_failed: "a bank payment failed",
  no_payment_method: "no payment method is on file",
  disputed: "a payment on the card was disputed",
  refunded: "a credit purchase was refunded",
};

/** "Auto top-up is off: the card was declined": why Nexus turned it off, in the glossary's words. */
export function topupOffText(reason: string): string {
  const why = (TOPUP_OFF_REASON as Record<string, string | undefined>)[reason] ?? reason;
  return `${BILLING_TERMS.autoTopup} is off: ${why}`;
}

/** The amounts the credit dialog offers, in dollars, on every surface; "Other" takes any amount from the minimum. */
export const CREDIT_AMOUNTS = ["10", "25", "50"] as const;

const AMOUNT = /^(-?)(\d+)(?:\.(\d+))?$/;
const grouped = (whole: string) => whole.replace(/\B(?=(\d{3})+$)/g, ",");

/**
 * An amount of money to the cent, for a balance, a receipt, a charge or what is owed: "$184.20", "$50.00",
 * "$1,180.00". Sub-cent digits are cut, never rounded up, so it never shows more than Nexus holds. Text that is not a
 * decimal shows as it came.
 */
export function billingAmount(decimal: string): string {
  const match = AMOUNT.exec(decimal.trim());
  if (!match) return decimal;
  const [, sign, whole = "0", fraction = ""] = match;
  return `${sign}$${grouped(whole)}.${fraction.padEnd(2, "0").slice(0, 2)}`;
}

/**
 * A setting in dollars, such as a usage limit, a top-up line or the free start's size: "$25", "$10.50", "$50,000".
 * Cents only when there are some, and no digit lost.
 */
export function billingSetting(decimal: string): string {
  const match = AMOUNT.exec(decimal.trim());
  if (!match) return decimal;
  const [, sign, whole = "0", fraction = ""] = match;
  const cents = fraction.replace(/0+$/, "");
  return `${sign}$${grouped(whole)}${cents ? `.${cents.padEnd(2, "0")}` : ""}`;
}

const DAY: Intl.DateTimeFormatOptions = { day: "numeric", month: "short", timeZone: "UTC" };

/** An ISO time as its UTC day: "10 Nov", or "10 Nov 2026" with the year. A time that does not parse shows as it came. */
export function billingDay(iso: string, withYear = false): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;
  return at.toLocaleDateString("en-GB", withYear ? { ...DAY, year: "numeric" } : DAY);
}

/** A date that decides something, with its UTC time: "10 Nov, 23:30 UTC". A time that does not parse shows as it came. */
export function billingCutoff(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return iso;
  const time = `${String(at.getUTCHours()).padStart(2, "0")}:${String(at.getUTCMinutes()).padStart(2, "0")}`;
  return `${billingDay(iso)}, ${time} UTC`;
}

/**
 * A change to Nexus pricing and the Customer Terms, as Billing's banner opens: "change on 26 Nov 2026."
 * while the day is ahead of `now`, "changed on" from the day on.
 */
export function pricingChangeLead(change: PricingChange, now = new Date()): string {
  const at = `${change.effectiveOn}T00:00:00.000Z`;
  const day = billingDay(at, true);
  const verb = Date.parse(at) > now.getTime() ? "change" : "changed";
  return `Nexus pricing and the Customer Terms ${verb} on ${day}.`;
}

/** When this month's usage limit resets: the first day of the next UTC month, as ISO. */
export function usageLimitResetsAt(now = new Date()): string {
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() + 1, 1)).toISOString();
}

/** "1 Dec, 00:00 UTC": when this month's usage limit resets. */
export function usageLimitResetDay(now = new Date()): string {
  return billingCutoff(usageLimitResetsAt(now));
}

/** "Resets on 1 Dec, 00:00 UTC": the usage limit's reset, said on every surface that shows the limit. */
export function usageLimitResetText(now = new Date()): string {
  return `Resets on ${usageLimitResetDay(now)}`;
}

/**
 * The note over billing that loaded once and then failed to refresh, beside a Try again (brand v5 P44's error: plain
 * words, the reason when known, and what is kept). Grey, not red: red is only for refusals and destructive actions.
 */
export function billingStaleNote(error: string): string {
  const reason = error.trim().replace(/[.\s]+$/, "");
  return `Billing didn't refresh.${reason ? ` ${reason.charAt(0).toUpperCase()}${reason.slice(1)}.` : ""} The figures shown are from the last time it loaded.`;
}

/** The action beside a billing error, on every surface (brand v5 P44). */
export const TRY_AGAIN = "Try again";

/**
 * A charge for usage that is still going through, such as a bank debit (402 `charge_processing` and `nexusPays.reason`):
 * the card pays for no more calls until it is paid. The same words on every surface. Change them here and
 * in `fixtures/billing-words.json` together.
 */
export const CHARGE_PROCESSING = {
  title: "A payment is still going through",
  body: "Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work.",
} as const;

/** The CLIs' way on after {@link CHARGE_PROCESSING}'s words: the apps show a button instead. */
export const CHARGE_PROCESSING_CLI_ACTION = "An owner can buy Nexus credits: `nexus billing credits buy <usd>`.";

/**
 * The composer notice's fixed titles (`payNoticeTitle`), and the 402 refusal titles of the same blocks. The free start's
 * titles name its size, so `payNoticeTitle` writes those. Change them here and in `fixtures/billing-words.json` together.
 */
export const PAY_NOTICE_TITLE = {
  usage_limit_reached: "This month's usage limit is used up",
  charge_failed: "A payment didn't go through",
  card_needs_confirming: "The bank wants you to confirm a payment",
  card_disputed: "A payment on the card was disputed",
  past_due: "A payment is past due",
  billing_off: "Pay as you go isn't set up",
  charge_processing: CHARGE_PROCESSING.title,
} as const satisfies Partial<Record<NexusPaysReason, string>>;

/** The composer notice for a `nexusPays.reason` this build does not know: a newer Gateway's. Its action opens Billing. */
export const PAY_NOTICE_FALLBACK_TITLE = "Nexus can't pay for model calls now";

/** 402 `credit_exhausted` while the organization pays as it goes: the credits left are less than the call may cost. */
export const CREDITS_SHORT_TITLE = "Your Nexus credits don't cover this call";

/**
 * The title of a 402 refusal of a call that Nexus pays, from `error.code` and `error.nexus.reason`, the same on every
 * surface; `undefined` for a code with words of its own (`usage_limit_reached` names its amounts, `subsidy_exhausted`
 * the free start). A `payment_failed` for the bank's check (`authentication_required`) asks to confirm, as the
 * composer's notice does.
 */
export function paymentRefusalTitle(code: string | undefined, reason?: unknown): string | undefined {
  switch (code) {
    case "credit_exhausted":
      return CREDITS_SHORT_TITLE;
    case "payment_failed":
      if (reason === "disputed") return PAY_NOTICE_TITLE.card_disputed;
      if (reason === "authentication_required") return PAY_NOTICE_TITLE.card_needs_confirming;
      return PAY_NOTICE_TITLE.charge_failed;
    case "payment_past_due":
      return PAY_NOTICE_TITLE.past_due;
    case "charge_processing":
      return PAY_NOTICE_TITLE.charge_processing;
    default:
      return undefined;
  }
}

/** Why a charge for usage after the fact failed (`afterTheFact.failed.reason`). */
export type ChargeFailedReason = NonNullable<AfterTheFact["failed"]>["reason"];

/**
 * Why a charge for usage after the fact failed, the same words on every surface. Change them here and in
 * `fixtures/billing-words.json` together. A reason missing here (a newer Gateway's) is not said: see {@link chargeFailedText}.
 */
export const CHARGE_FAILED_REASON: Record<ChargeFailedReason, string> = {
  declined: "the card was declined",
  authentication_required: "the bank wants the cardholder to confirm it",
  payment_failed: "the payment failed",
  no_payment_method: "no payment method is on file",
  disputed: "a payment on the card was disputed",
};

/**
 * What a charge's words name: its amount (a decimal dollar string), and whether it is the final charge (paying after the
 * fact is off). An amount under a cent is not said, so no text says "$0.00".
 */
export interface ChargeWords {
  amountUsd?: string | null;
  final?: boolean;
}

/** " of $12.00", or nothing for no amount or one under a cent. */
function ofAmount(amountUsd: string | null | undefined): string {
  const micros = amountUsd ? parseMicros(amountUsd) : null;
  return amountUsd && micros !== null && micros >= 10_000 ? ` of ${billingAmount(amountUsd)}` : "";
}

const lastCharge = (words: ChargeWords) => `The ${words.final ? "final" : "last"} charge${ofAmount(words.amountUsd)}`;

/** "The last charge of $12.00 didn't go through: the card was declined". An unknown reason is left out. */
export function chargeFailedText(reason: string | undefined, words: ChargeWords = {}): string {
  const why = reason ? (CHARGE_FAILED_REASON as Record<string, string | undefined>)[reason] : undefined;
  return `${lastCharge(words)} didn't go through${why ? `: ${why}` : ""}`;
}

/** "The last charge of $12.00 didn't go through. Nexus is trying it again now". */
export function chargeRetryingText(words: ChargeWords = {}): string {
  return `${lastCharge(words)} didn't go through. Nexus is trying it again now`;
}

/** "The charge of $12.00 needs the cardholder's confirmation" (3-D Secure). */
export function chargeConfirmText(words: ChargeWords = {}): string {
  return `The ${words.final ? "final charge" : "charge"}${ofAmount(words.amountUsd)} needs the cardholder's confirmation`;
}

/** "A charge of $12.00 is going through": calm, nothing to do. */
export function chargeRunningText(words: ChargeWords = {}): string {
  return `${words.final ? "The final charge" : "A charge"}${ofAmount(words.amountUsd)} is going through`;
}

/**
 * Paying after the fact is off and usage is unpaid: the final charge takes it once the open calls settle, from
 * `minimumUsd` (Gateway's `pricing.minFinalChargeUsd`, 50 cents). Less stays unpaid until it is on again.
 */
export function finalChargeText(unpaidUsd: string, minimumUsd = "0.50"): string {
  const unpaid = parseMicros(unpaidUsd) ?? 0;
  const minimum = parseMicros(minimumUsd) ?? 500_000;
  if (unpaid >= minimum) return `A final charge of ${billingAmount(unpaidUsd)} runs once open calls settle`;
  return `Less than ${billingSetting(minimumUsd)} of usage is unpaid. It stays unpaid until paying after the fact is on again`;
}

/**
 * The title once Nexus has nothing left to pay a call with, the same on every app and in the composer's notice
 * (`payNoticeTitle`). `granted`: the free start's size from Gateway, said when known ("Your free $1 is used up").
 */
export function usedUpTitle(plan: Plan, granted?: Micros): string {
  if (plan.kind === "free") return granted ? `Your free ${budgetText(granted)} is used up` : "Your free start is used up";
  if (plan.kind === "payg") return "Your Nexus credits are used up";
  if (plan.kind === "paid") return `${plan.name}'s Nexus usage is used up`;
  return "Your organization's Nexus usage is used up";
}
