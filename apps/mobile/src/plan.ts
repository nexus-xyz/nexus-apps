import {
  BILLING_TERMS,
  blockOfReason,
  CHARGE_PROCESSING,
  CREDITS_SHORT_TITLE,
  dayText,
  microsOfNumber,
  parseMicros,
  paymentRefusalTitle,
  payNoticeTitle,
  usedUpTitle,
  type BillingBlock,
  type Micros,
  type NexusPaysReason,
  type PayNotice,
  type Plan,
  type Usage,
} from "@nexus/kit";
import { usdOf } from "./billing";

// The phone's words for how the organization pays (pricing v2.0) and what is left of its Nexus usage. `planOf` and
// `usageOf` read them, shared with Nexus UI in `@nexus/kit`. Pricing v2.0 sells no plans (`paid` comes back where
// Gateway has `NEXUS_PLANS`): a free account spends its one-time
// free start. Notices offer model access without a purchase prompt in every storefront; only Billing
// links to the web, where the store allows it.

export { freeStartPays, payLineParts, payLineText, payNoticeOf, planOf, usageOf, type PayLineParts, type PayNotice, type Plan, type Usage } from "@nexus/kit";

const OWN_KEY = "add your own provider key under Settings → API keys";

type Amounts = Pick<Usage, "leftUsd" | "grantedUsd"> & Partial<Pick<Usage, "left" | "granted">>;

/**
 * The usage's amounts in micro-dollars. A usage cached by an older build has only `leftUsd` and
 * `grantedUsd`, so those are the fallback.
 */
function amounts(usage: Amounts): { left: Micros; granted: Micros } {
  return { left: usage.left ?? microsOfNumber(usage.leftUsd), granted: usage.granted ?? microsOfNumber(usage.grantedUsd) };
}

/** What Billing sets, for an owner: the phone's Billing page. */
export const BILLING = { href: "/billing", label: BILLING_TERMS.openBilling } as const;
const KEYS = { href: "/keys", label: "Open API keys" } as const;

/** The plan's name on a row: "Free", "Pay as you go" or a paid plan's name; nothing while it is not known. */
export function planLabel(plan: Plan): string | undefined {
  if (plan.kind === "paid") return plan.name;
  return plan.kind === "free" ? "Free" : plan.kind === "payg" ? "Pay as you go" : undefined;
}

/** What is left of the free start, in a few words: "$0.40 of $1 free start left", or that it is used up. */
export function usageLine(usage: Amounts & Partial<Pick<Usage, "usedUp">>): string {
  const { left, granted } = amounts(usage);
  return usage.usedUp || left < 1 ? `Free ${usdOf(granted)} used up` : `${usdOf(left)} of ${usdOf(granted)} free start left`;
}

export interface Notice {
  title: string;
  body: string;
  /** The phone's page that shows it: Billing, or the provider keys. */
  open: { href: "/keys" | "/billing"; label: string };
  /** Someone must act on a payment: the amber dot (Brand v5 P73·H). A limit reached as set has none. */
  act?: boolean;
  /**
   * A block only an owner fixes, for a member who is not one: the notice names the owners and asks them in
   * one tap. An owner's notice has none and opens Billing.
   */
  ask?: BillingBlock;
}

/**
 * An owner reads billing status in Billing; a member asks an owner about `block`. A member's plan is `unknown`: only an
 * owner reads `GET /v1/billing`.
 */
function keepGoing(plan: Plan, owner: string, member: string, block: BillingBlock): Pick<Notice, "body" | "open" | "ask"> {
  return plan.kind === "unknown" ? { body: `${member}, or ${OWN_KEY}.`, open: KEYS, ask: block } : { body: owner, open: BILLING };
}

/** The glossary's words for a payment still going through; a member is told to ask an owner for credits. */
function chargeProcessing(plan: Plan): Pick<Notice, "body" | "open" | "ask"> {
  return keepGoing(plan, CHARGE_PROCESSING.body, `${CHARGE_PROCESSING.body} Ask an owner to buy credits`, "billing");
}

/** The notice above the composer once the Nexus usage is used up. */
export function usedUpNotice(plan: Plan, usage?: Amounts): Notice {
  // The title is the glossary's, the same as Nexus UI's and the composer's.
  const title = usedUpTitle(plan, usage ? amounts(usage).granted : undefined);
  if (plan.kind === "unknown") return { title, body: `Ask an owner about model access, or ${OWN_KEY}.`, open: KEYS, ask: "billing" };
  return { title, body: "Your own provider keys still work.", open: BILLING };
}

type Refused = Extract<PayNotice, { kind: "refused" }>;

/** What stops and until when, in Brand v5's billing voice (P73); the title is the same on every app (`payNoticeTitle`). */
function payWhy(notice: Refused): string {
  const paused = "Models Nexus runs are paused";
  switch (notice.reason) {
    case "usage_limit_reached":
      return notice.changesAt ? `${paused} until ${dayText(notice.changesAt)}, or until an owner raises the limit.` : `${paused} until an owner raises the limit.`;
    case "charge_failed":
      return `${paused} until a payment goes through.`;
    case "card_needs_confirming":
      return `${paused} until the payment is confirmed. Nothing is charged until then.`;
    case "card_disputed":
      return `${paused} until an owner adds another card.`;
    case "past_due":
      return `${paused} until it's paid.`;
    default:
      return `${paused}.`;
  }
}

/**
 * The notice above the composer when Nexus cannot pay the next call, one per reason, before a send is
 * refused. An owner opens Billing; anyone else is told to ask an owner, or to use a key of their own.
 */
export function payNotice(notice: Refused, plan: Plan): Notice {
  // A payment still going through says the glossary's words, as every app does.
  if (notice.reason === "charge_processing") return { title: payNoticeTitle(notice), ...chargeProcessing(plan) };
  const why = payWhy(notice);
  const act = notice.reason === "charge_failed" || notice.reason === "card_needs_confirming" || notice.reason === "card_disputed" || notice.reason === "past_due";
  return { title: payNoticeTitle(notice), ...keepGoing(plan, `${why} Your own keys keep working.`, `${why} Ask an owner about model access`, blockOfReason(notice.reason)), ...(act ? { act } : {}) };
}

/** The composer's title for the same block (`payNoticeTitle`), so a refusal and the notice say it one way on every app. */
const titleOf = (reason: NexusPaysReason) => payNoticeTitle({ kind: "refused", reason });

/** Gateway's words for a 402 `payment_failed` whose card had a payment disputed: a chat's stored error has no details. */
const DISPUTED = /payment on the card was disputed/i;

/**
 * A stored chat error's `details` (Gateway's allow-listed `error.nexus`: `reason`, `next`, the `*Usd` amounts), or
 * `undefined` when it has none, such as from an older Gateway. Read with a guard: the kit's type may not name it yet.
 */
export function storedDetails(error: unknown): Record<string, unknown> | undefined {
  if (typeof error !== "object" || error === null || !("details" in error)) return undefined;
  const { details } = error;
  return typeof details === "object" && details !== null && !Array.isArray(details) ? (details as Record<string, unknown>) : undefined;
}

/** What the chat knows about a refused model on the free start: its name, and models the free start pays for. */
export interface FreeStartModels {
  /** The refused model's name. */
  model?: string;
  /** The names of models the free start pays for, Auto not among them, most useful first. */
  covered: string[];
}

/** "Auto", "Auto or Claude Haiku 5.5", "Auto, Claude Haiku 5.5 or GPT-6 Luna". */
const either = (names: string[]) => (names.length > 1 ? `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}` : (names[0] ?? ""));

/** Gateway's words for a 402 `usage_limit_reached`: what the call may cost, and what the month's limit has left. */
const LIMIT_NAMED = /may cost up to (\$[\d,.]+).*?and (\$[\d,.]+) of this month's usage limit/i;

/** Less than a cent: Gateway's message says "$0.00", so nothing is left. */
const CENT: Micros = 10_000;

/**
 * What the month's usage limit has left, from the refusal's `details.remainingUsd`, else from its message (an older
 * Gateway stores no details); `undefined` when neither says.
 */
function limitLeft(message: string, details?: Record<string, unknown>): Micros | undefined {
  const remaining = typeof details?.remainingUsd === "string" ? parseMicros(details.remainingUsd) : null;
  if (remaining !== null) return remaining;
  const left = LIMIT_NAMED.exec(message)?.[2];
  return left ? (parseMicros(left.slice(1).replaceAll(",", "")) ?? undefined) : undefined;
}

/**
 * A usage-limit refusal's title, in an internal pull request's words: Gateway refuses a call that MAY cost more than what the
 * month's limit has left, so the title names both, from its message, and never says the limit is used while money is left.
 */
function usageLimitTitle(message: string, left: Micros | undefined): string {
  if (left !== undefined && left < CENT) return titleOf("usage_limit_reached");
  const named = LIMIT_NAMED.exec(message);
  if (!named) return "This month's usage limit doesn't cover this call";
  const [, cost, leftText] = named;
  return `This call may cost up to ${cost}, and ${leftText} of this month's usage limit is left`;
}

export interface Explained extends Notice {
  /** The refusal says the Nexus usage is used up: read it again. */
  usedUp: boolean;
  /** The free start pays for cheaper models only, and this was not one: asking Auto again gets an answer. */
  cheaperModel?: boolean;
}

/**
 * A refused or failed answer for a reason the person can act on, in plain words and with what to do on the phone, in
 * place of Gateway's words for the CLI (`nexus keys add …`). Anything else is `undefined`, shown as Gateway said it.
 * `details`: Gateway's `error.nexus`, live or stored (`storedDetails`); without it (an older Gateway), the message decides.
 */
export function explainRefusal(failure: { code?: string; message: string; details?: Record<string, unknown> }, plan: Plan, free?: FreeStartModels): Explained | undefined {
  const { code, message, details } = failure;
  // The free start has money left, but not for this model (pricing v2.0): it is not used up. It names the
  // model refused and the models the free start pays for, so the person knows what to pick.
  if (/free start pays for cheaper models only/i.test(message)) {
    const pick = `Pick ${either(["Auto", ...(free?.covered.slice(0, 2) ?? [])])}`;
    return {
      title: free?.model ? `Your free start doesn't pay for ${free.model}` : "Your free start covers cheaper models only",
      ...keepGoing(plan, `${pick}.`, `${pick}. Or ask an owner about model access`, "billing"),
      usedUp: false,
      cheaperModel: true,
    };
  }
  if (code === "subsidy_exhausted" || /of the Nexus credit is left/i.test(message)) {
    return {
      title:
        plan.kind === "unknown" ? "Your organization's Nexus usage doesn't cover this answer" : plan.kind === "paid" ? "Your plan's Nexus usage doesn't cover this answer" : "Your free start doesn't cover this answer",
      ...keepGoing(plan, "Try a shorter answer or use your own provider key.", "Ask an owner about model access", "credits_out"),
      usedUp: true,
    };
  }
  if (code === "credit_exhausted" || /of the Nexus credits is left/i.test(message)) {
    return {
      // The kit's title, the same on every app.
      title: CREDITS_SHORT_TITLE,
      ...keepGoing(plan, "Try a shorter answer or use your own provider key.", "Ask an owner about model access", "credits_out"),
      usedUp: true,
    };
  }
  if (code === "usage_limit_reached" || /of this month's usage limit/i.test(message)) {
    // Gateway refuses a call that MAY cost more than what is left: the limit is used up only when nothing is left.
    // A refusal that does not say what is left counts as used up, so the plan is read again.
    const left = limitLeft(message, details);
    return {
      title: usageLimitTitle(message, left),
      ...keepGoing(plan, "Your own provider keys still work.", "Ask an owner to raise the limit", "usage_limit"),
      usedUp: left === undefined || left < CENT,
    };
  }
  // Gateway's `details.reason` decides; only an error without details (an older Gateway's) is read from its message.
  const payReason = typeof details?.reason === "string" ? details.reason : DISPUTED.test(message) ? "disputed" : undefined;
  if (code === "payment_failed" && payReason === "disputed") {
    return {
      title: titleOf("card_disputed"),
      ...keepGoing(plan, "Model calls on Nexus are paused until an owner adds another card. Your own provider keys still work.", "Ask an owner to add another card", "payment_failed"),
      usedUp: false,
    };
  }
  if (code === "payment_failed" && payReason === "authentication_required") {
    // 3-D Secure: the bank wants the cardholder to confirm the charge. The kit's title, as the composer's notice says it.
    return {
      title: paymentRefusalTitle(code, payReason) ?? titleOf("card_needs_confirming"),
      ...keepGoing(plan, "Model calls on Nexus are paused until the payment is confirmed. Your own provider keys still work.", "Ask an owner to confirm the payment", "payment_failed"),
      usedUp: false,
    };
  }
  if (code === "payment_failed") {
    return {
      title: paymentRefusalTitle(code, payReason) ?? titleOf("charge_failed"),
      ...keepGoing(plan, "Model calls on Nexus are paused. Your own provider keys still work.", "Ask an owner about model access", "payment_failed"),
      usedUp: false,
    };
  }
  if (code === "payment_past_due") {
    return {
      title: titleOf("past_due"),
      // Only an owner opens Billing: a member asks one to pay it.
      ...keepGoing(plan, "Your own provider keys still work.", "Ask an owner to pay it", "past_due"),
      usedUp: false,
    };
  }
  if (code === "charge_processing" || /for usage is still going through/i.test(message)) {
    return {
      title: titleOf("charge_processing"),
      ...chargeProcessing(plan),
      usedUp: false,
    };
  }
  if (/Nexus credit is used up/i.test(message)) {
    return { ...usedUpNotice(plan), usedUp: true };
  }
  const unpaid = /does not pay (\w+) calls with it yet/i.exec(message)?.[1];
  if (unpaid) {
    return {
      title: `Nexus usage doesn't pay for ${unpaid} models yet`,
      body: `Pick another model, such as Auto, or add your own ${unpaid} key under Settings → API keys.`,
      usedUp: false,
      open: KEYS,
    };
  }
  const missing = /no (\w+) key for this organization/i.exec(message)?.[1];
  if (missing) {
    return { title: `No ${missing} key for this organization`, body: "An owner or admin adds one under Settings → API keys.", usedUp: false, open: KEYS };
  }
  return undefined;
}
