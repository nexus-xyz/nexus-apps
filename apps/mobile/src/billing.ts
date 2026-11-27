import {
  BILLING_TERMS,
  CoreError,
  TOPUP_OFF_REASON,
  afterTheFactNotice,
  topupOffText,
  billingAmount,
  billingCutoff,
  billingDay,
  billingSetting,
  parseMicros,
  usageLimitResetText,
  usdText,
  type AfterTheFactNotice,
  type BillingResponse,
  type CardSummary,
  type Micros,
  type PastDue,
  type Plan,
  type Receipt,
} from "@nexus/kit";

// The phone's Billing page (pricing v2.0, brand v5's P40·C), read first: what it shows, as text. In every storefront an
// owner makes the changes that buy nothing here: auto top-up off, a lower usage limit. Where the store
// allows it (`storefront.ts`), the rest opens Nexus UI's Billing in the browser.

/** Micro-dollars as the boards write money: "$1", "$406", "$50,000", "$184.20", and part of a cent in full ("$0.004000"). */
export const usdOf = (value: Micros) => usdText(value).replace(/\.00$/, "");

/** An amount to the cent, as every surface writes a balance, a receipt or a charge (kit's `billingAmount`): "$50.00". */
export const usd = billingAmount;

/** A setting, as every surface writes a limit or a top-up line (kit's `billingSetting`): "$25", "$10.50". */
export const setting = billingSetting;

/** A Gateway without billing, as Nexus UI reads it: 404 on the TypeScript gateway, 503 `not_configured` without Stripe. */
export const noBilling = (error: unknown) => error instanceof CoreError && (error.status === 404 || error.code === "not_configured");

/** `YYYY-MM` as "October 2026", in UTC: the month Gateway priced, whatever the viewer's time zone. */
export function monthLabel(period: string): string {
  const match = /^(\d{4})-(\d{2})$/.exec(period);
  if (!match) return period;
  return new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, 1)).toLocaleDateString("en-US", { month: "long", year: "numeric", timeZone: "UTC" });
}

/** The first day of this UTC month, as "1 Nov": paying after the fact counts from it. */
export const monthStart = (now = new Date()) =>
  new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1)).toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });

const BRANDS: Record<string, string> = { amex: "American Express", diners: "Diners Club", discover: "Discover", jcb: "JCB", mastercard: "Mastercard", unionpay: "UnionPay", visa: "Visa" };

/** Stripe's card brand (`visa`, `amex`…) as people write it. */
export const brandName = (brand: string) => BRANDS[brand] ?? (brand === "unknown" || !brand ? "Card" : brand.charAt(0).toUpperCase() + brand.slice(1));

/** A card is good through the last day of its expiry month. */
export function cardExpired(card: CardSummary, now = new Date()): boolean {
  const year = now.getUTCFullYear();
  return card.expYear < year || (card.expYear === year && card.expMonth < now.getUTCMonth() + 1);
}

/** The payment method on file, short: "Visa ·· 4242", "Chase ·· 6789", "Link"; `undefined` for none. */
export function paymentLabel(billing: Pick<BillingResponse, "paymentMethod" | "defaultPaymentMethod">): string | undefined {
  const card = billing.paymentMethod;
  if (card) return `${brandName(card.brand)} ·· ${card.last4}`;
  const method = billing.defaultPaymentMethod;
  if (!method) return undefined;
  const name = method.brand ? brandName(method.brand) : (method.bankName ?? (method.type === "link" ? "Link" : method.type === "us_bank_account" ? "Bank account" : "Payment method"));
  return method.last4 ? `${name} ·· ${method.last4}` : name;
}

/** "Pay as you go"'s second line: credits left, and what the card paid this month: "$406 in credits · $184.20 since 1 Nov". */
export function paygLine(billing: Pick<BillingResponse, "credits" | "afterTheFact">, now = new Date()): string {
  const parts: string[] = [];
  if (billing.credits) parts.push(`${usd(billing.credits.remainingUsd)} in credits`);
  if (billing.afterTheFact?.enabled) parts.push(`${usd(billing.afterTheFact.monthSpentUsd)} since ${monthStart(now)}`);
  return parts.join(" · ");
}

/**
 * What Billing says about paying after the fact beyond its usual line, from the charge's phase (kit's
 * `afterTheFactNotice`): a charge that failed, is tried again, waits for the bank or was disputed, a charge going
 * through, or the final charge. None while nothing needs saying (on, off, closed).
 */
export function afterTheFactNoticeOf(billing: Pick<BillingResponse, "afterTheFact" | "pricing">): AfterTheFactNotice | null {
  const after = billing.afterTheFact;
  return after ? afterTheFactNotice(after, billing.pricing?.minFinalChargeUsd) : null;
}

/**
 * Whether Billing shows the usage limit row. It shows while the organization pays as it goes. It also shows after an
 * owner turned paying after the fact off with a failed charge or unpaid usage: `planOf` then says Free.
 */
export function showsUsageLimit(plan: Plan, notice: AfterTheFactNotice | null): boolean {
  return plan.kind === "payg" || plan.kind === "paid" || notice !== null;
}

/**
 * The usage limit row's line. The charge's phase comes first, with "Paused." only while calls on the card stop: with
 * paying after the fact off, nothing runs on the card. Else what this month used, and when the limit resets.
 * `problem`: an owner must act on the charge.
 */
export function usageLimitLine(
  after: Pick<NonNullable<BillingResponse["afterTheFact"]>, "enabled" | "monthSpentUsd"> | null | undefined,
  notice: AfterTheFactNotice | null,
  now = new Date(),
): { text?: string; problem: boolean } {
  if (notice) return { text: `${notice.title}.${notice.stopsCalls ? " Paused." : ""}`, problem: notice.action !== undefined };
  if (after?.enabled) return { text: `${usd(after.monthSpentUsd)} used. ${usageLimitResetText(now)}`, problem: false };
  return { problem: false };
}

/** "Free"'s second line: what is left of the one-time free start ("$0.40 of your free $1 left"), and until when; once expired, when. */
export function freeLine(billing: Pick<BillingResponse, "freeStart">): string {
  const start = billing.freeStart;
  if (!start) return "Your own keys and Claude or ChatGPT plans are free.";
  if (start.expiredUsd !== undefined && start.expiredUsd !== null) return `Your free start expired${start.expiresAt ? ` on ${billingCutoff(start.expiresAt)}` : ""}`;
  if (parseMicros(start.remainingUsd) === 0) return `Your free start of ${setting(start.grantedUsd)} is used up`;
  return `${usd(start.remainingUsd)} of your free ${setting(start.grantedUsd)} left${start.expiresAt ? ` · until ${billingCutoff(start.expiresAt)}` : ""}`;
}

/**
 * What is left of the free start, as a share of what Nexus gave (0 to 1), for the meter under "Free"; `undefined` when
 * there is none, or once it expired.
 */
export function freeStartLeft(start: BillingResponse["freeStart"]): number | undefined {
  if (!start || (start.expiredUsd !== undefined && start.expiredUsd !== null)) return undefined;
  const granted = parseMicros(start.grantedUsd) ?? 0;
  if (granted <= 0) return undefined;
  return Math.min(Math.max((parseMicros(start.remainingUsd) ?? 0) / granted, 0), 1);
}

/**
 * What stays free for an organization with no way to pay: its own provider keys, and people's own Claude or
 * ChatGPT plans. Neither is a purchase, so every storefront shows them, most of all once the free start ran out.
 */
export const ALWAYS_FREE = [
  { label: "Your own provider keys", detail: "Nexus doesn't charge for calls on your own key.", href: "/keys" },
  { label: "Your Claude or ChatGPT plan", detail: "Claude Code and Codex run on your own plan.", href: "/agents" },
] as const;

/** Receipts still to pay: an open one, or one whose payment failed. */
export const openReceipts = (receipts: Pick<Receipt, "status">[]) => receipts.filter((r) => r.status === "open" || r.status === "failed").length;

/** Whether model calls have stopped for an unpaid month: `blockedFrom` is null when Gateway has no gate. */
export const pastDueStopped = (pastDue: PastDue, now = Date.now()) => pastDue.blockedFrom !== null && Date.parse(pastDue.blockedFrom) <= now;

/** The credits' second line (brand v5's P40·D): the auto top-up as set, "Auto top-up: under $1,000, add $5,000"; none while off. */
export function topupLine(topup: BillingResponse["autoTopup"]): string | undefined {
  if (!topup?.enabled) return undefined;
  return `${BILLING_TERMS.autoTopup}: under ${setting(topup.belowUsd)}, add ${setting(topup.addUsd)}`;
}

/** A receipt's line under its description: its UTC day, and its state unless it is paid ("8 Nov · Open"). */
export function receiptLine(receipt: Pick<Receipt, "at" | "status">): string {
  const day = billingDay(receipt.at);
  const state = { paid: undefined, open: "Open", failed: "Payment failed", void: "Void" }[receipt.status];
  return state ? `${day} · ${state}` : day;
}

/** Available rewards stay visible even after welcome credit expires. */
export function promotionalLine(billing: Pick<BillingResponse, "promotionalCreditUsd">): string | undefined {
  const amount = billing.promotionalCreditUsd;
  return amount && (parseMicros(amount) ?? 0) > 0 ? usd(amount) : undefined;
}

/** The credits' second line while Nexus keeps auto top-up off, in the glossary's words (`topupOffText`); none otherwise. */
export function topupOffLine(topup: BillingResponse["autoTopup"]): string | undefined {
  if (!topup?.offReason) return undefined;
  return topupOffText(topup.offReason);
}

/** How many receipts Billing lists; "All receipts" opens the rest on their own screen. */
export const RECEIPTS_SHOWN = 5;

/** Whether Billing shows only some of the receipts, so "All receipts" opens the full list. */
export const moreReceipts = (receipts: unknown[] | undefined) => (receipts?.length ?? 0) > RECEIPTS_SHOWN;

/**
 * What a receipt opens outside the app: Stripe's receipt page for a payment (minted on the tap), or a settled month's
 * invoice. Never a page that takes a payment, so an open or failed invoice opens nothing.
 */
export function receiptLink(receipt: Pick<Receipt, "receipt" | "url" | "status">): "receipt" | "invoice" | undefined {
  if (receipt.receipt) return "receipt";
  if (receipt.url && (receipt.status === "paid" || receipt.status === "void")) return "invoice";
  return undefined;
}

/** A typed amount as Gateway takes it: no "$" and no thousands separators ("$1,000" is "1000"). */
export const limitInput = (text: string) => text.trim().replace(/^\$/, "").replaceAll(",", "");

/**
 * Why the phone cannot set `text` as the usage limit, or null. The phone only lowers it: a lower limit buys
 * nothing, so it is allowed in every storefront. Gateway checks too.
 */
export function lowerLimitProblem(text: string, currentUsd: string): string | null {
  const value = limitInput(text);
  if (!/^\d+(\.\d{1,2})?$/.test(value)) return "Type the limit in dollars, such as 25 or 10.50.";
  const micros = parseMicros(value) ?? 0;
  if (micros < 1_000_000) return "The limit is at least $1.";
  if (micros >= (parseMicros(currentUsd) ?? 0)) return `Type less than ${setting(currentUsd)}. The phone only lowers the limit.`;
  return null;
}

/** Receipts under their UTC month ("November 2026"), in the order Gateway lists them: newest first. */
export function receiptsByMonth(receipts: Receipt[]): { month: string; receipts: Receipt[] }[] {
  const groups: { month: string; receipts: Receipt[] }[] = [];
  for (const receipt of receipts) {
    const month = monthLabel(receipt.at.slice(0, 7));
    const last = groups.at(-1);
    if (last?.month === month) last.receipts.push(receipt);
    else groups.push({ month, receipts: [receipt] });
  }
  return groups;
}

/**
 * The amber card while Gateway keeps auto top-up off (the owners were told, `billing.auto_topup_off`): why, and what it
 * means, with no purchase prompt. None for a reason it does not know, such as a closed organization.
 */
export function topupOffAlert(topup: BillingResponse["autoTopup"]): { title: string; body: string } | undefined {
  // A closed organization has nothing to fix: it shows no card.
  const reason = topup?.offReason && topup.offReason !== "closed" && topup.offReason in TOPUP_OFF_REASON ? topup.offReason : undefined;
  if (!reason) return undefined;
  return { title: `${topupOffText(reason)}.`, body: `${BILLING_TERMS.credits} are no longer bought when they run low.` };
}
