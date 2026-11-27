/**
 * Paying after the fact's state as every surface shows it (`GET /v1/billing`'s `afterTheFact`). Gateway names the
 * charge phase (`phase`); an older Gateway does not, and then {@link afterTheFactPhaseOf} works it out from the other
 * fields. {@link afterTheFactNotice} gives each phase its words, from `billing-words.ts`. Nexus UI reads these; the
 * desktop app, the phone and the CLIs check their own words against `fixtures/billing-words.json`.
 */
import type { AfterTheFact, AfterTheFactPhase } from "./api";
import {
  billingAmount,
  billingSetting,
  chargeConfirmText,
  chargeFailedText,
  chargeRetryingText,
  chargeRunningText,
  finalChargeText,
  PAY_NOTICE_TITLE,
} from "./billing-words";
import { parseMicros } from "./money";

const PHASES: ReadonlySet<string> = new Set<AfterTheFactPhase>([
  "off",
  "off_owing",
  "on",
  "charging",
  "failed",
  "retrying",
  "needs_confirmation",
  "disputed",
  "closed",
]);

/** The usage the card has not paid, a running charge included: `unpaidUsd`, else (an older Gateway) `owedUsd`. */
export function unpaidUsdOf(after: Pick<AfterTheFact, "unpaidUsd" | "owedUsd">): string {
  return after.unpaidUsd ?? after.owedUsd;
}

/**
 * The charge phase: Gateway's `phase`, else (an older Gateway, or a phase this build does not know) the same order
 * from the other fields. `closed` comes only from Gateway.
 */
export function afterTheFactPhaseOf(after: AfterTheFact): AfterTheFactPhase {
  if (after.phase && PHASES.has(after.phase)) return after.phase;
  const reason = after.failed?.reason;
  if (reason === "disputed") return "disputed";
  if (reason === "authentication_required") return "needs_confirmation";
  if (after.failed) return after.pending ? "retrying" : "failed";
  if (after.pending) return "charging";
  if (after.enabled) return "on";
  return (parseMicros(unpaidUsdOf(after)) ?? 0) > 0 ? "off_owing" : "off";
}

/** What an empty Receipts list says when nothing is waiting for a charge. */
export const NO_RECEIPTS = "No payments yet. Credits you buy and charges for usage show here.";

/**
 * What an empty Receipts list says. While paying after the fact is on and its unpaid usage is from a cent
 * to under the threshold, no charge has run yet: the list names that usage and when the card pays it, so an owner
 * does not take the empty list for missing receipts. Otherwise {@link NO_RECEIPTS}; the banner speaks for a charge.
 */
export function receiptsEmptyText(after: AfterTheFact | null | undefined): string {
  if (!after || afterTheFactPhaseOf(after) !== "on") return NO_RECEIPTS;
  const unpaidUsd = unpaidUsdOf(after);
  const unpaid = parseMicros(unpaidUsd) ?? 0;
  const threshold = parseMicros(after.thresholdUsd) ?? 0;
  if (unpaid < 10_000 || unpaid >= threshold) return NO_RECEIPTS;
  return `No payments yet. ${billingAmount(unpaidUsd)} of usage is unpaid. Nexus charges the card when unpaid usage reaches ${billingSetting(after.thresholdUsd)}.`;
}

/** The phases in which a charge failed or waits for the cardholder: an owner must act. */
export function chargeProblem(phase: AfterTheFactPhase): boolean {
  return phase === "failed" || phase === "retrying" || phase === "needs_confirmation" || phase === "disputed";
}

/** The owner's action for a charge problem: a key of `BILLING_TERMS`, which names it. */
export type AfterTheFactAction = "fixPayment" | "confirmPayment" | "addAnotherCard";

/**
 * What a surface says about paying after the fact, beyond its usual line. `title` and `body` have no final period.
 * `stopsCalls`: Gateway refuses calls the card pays (402 `payment_failed`), so a surface shows it as urgent. With
 * paying after the fact off, nothing runs on the card: a failed final charge stops nothing, and an owner fixes the
 * card only so the charge goes through. `action`: the owner's way on; none for a calm note.
 */
export interface AfterTheFactNotice {
  phase: AfterTheFactPhase;
  title: string;
  body?: string;
  action?: AfterTheFactAction;
  stopsCalls: boolean;
}

const KEEPS = "Credits and your own keys still work";
const NOTHING_RUNS = "Nothing else runs on the card";

/**
 * The notice for `after`, or `null` while nothing needs saying (on, off, closed). `minFinalChargeUsd`: Gateway's
 * `pricing.minFinalChargeUsd`. The amount named is the unpaid usage (`unpaidUsd`), never `owedUsd`, which leaves a
 * running charge out and reads "$0.00" during a retry.
 */
export function afterTheFactNotice(after: AfterTheFact, minFinalChargeUsd?: string): AfterTheFactNotice | null {
  const phase = afterTheFactPhaseOf(after);
  const final = !after.enabled;
  const words = { amountUsd: unpaidUsdOf(after), final };
  const stopsCalls = !final && chargeProblem(phase);
  switch (phase) {
    case "failed":
      return {
        phase,
        title: chargeFailedText(after.failed?.reason, words),
        body: final ? `${NOTHING_RUNS}. An owner fixes the card so the charge goes through` : `Nexus-run AI the card pays is paused until a charge goes through. ${KEEPS}`,
        action: "fixPayment",
        stopsCalls,
      };
    case "retrying":
      return {
        phase,
        title: chargeRetryingText(words),
        body: final ? NOTHING_RUNS : `Nexus-run AI the card pays stays paused until it's paid. ${KEEPS}`,
        action: "fixPayment",
        stopsCalls,
      };
    case "needs_confirmation":
      return {
        phase,
        title: chargeConfirmText(words),
        body: final ? `${NOTHING_RUNS}. An owner confirms the charge on the bank's page` : `Until then, Nexus-run AI the card pays is paused. ${KEEPS}`,
        action: "confirmPayment",
        stopsCalls,
      };
    case "disputed":
      return {
        phase,
        title: PAY_NOTICE_TITLE.card_disputed,
        body: final ? `${NOTHING_RUNS}. An owner adds another card so the charge goes through` : `Nexus-run AI the card pays is paused until an owner adds another card. ${KEEPS}`,
        action: "addAnotherCard",
        stopsCalls,
      };
    case "charging":
      return { phase, title: chargeRunningText({ amountUsd: after.chargeUsd, final }), stopsCalls: false };
    case "off_owing":
      return { phase, title: finalChargeText(unpaidUsdOf(after), minFinalChargeUsd), stopsCalls: false };
    default:
      return null;
  }
}
