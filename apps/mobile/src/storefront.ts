import { BILLING_TERMS, type BillingResponse } from "@nexus/kit";

// Where the phone may link to Nexus UI's Billing, and what those links are. The store's rules follow the
// storefront, never the device's region, its IP or the organization's address.
//
// - App Store, US storefront: guideline 3.1.1(a) lets an app link to a purchase on the web, with no entitlement, when
//   the link opens outside the app. The rows open Safari (`Linking.openURL`), not an in-app browser.
// - App Store, every other storefront: no link, button or text that points to a purchase outside the app.
//   Brazil allows links only with in-app purchase beside them; the EU storefronts are excluded.
// - Google Play: no link. A US link-out needs Play's external content links program, its APIs and its fees.
//
// An unknown storefront (Expo Go, no App Store account) gets no link.

/** App Store storefronts (ISO 3166-1 alpha-3, as StoreKit gives them) where Billing links to the web. */
const LINK_STOREFRONTS: ReadonlySet<string> = new Set(["USA"]);

/** Whether Billing may show rows that open Nexus UI's Billing in the browser. */
export function webBillingAllowed(platform: string, storefront: string | null | undefined): boolean {
  if (platform !== "ios" || !storefront) return false;
  return LINK_STOREFRONTS.has(storefront.trim().toUpperCase());
}

/** A row that opens a part of Nexus UI's Billing; `path` follows `/app/<org>`. */
export interface WebBillingAction {
  label: string;
  path: string;
}

/** Nexus UI's deep links (its billing page reads `buy`, `topup` and `card` once, then drops them). */
const PAGES = {
  billing: "/settings/billing",
  plans: "/settings/billing#plans",
  credits: "/settings/billing?buy=credits",
  topup: "/settings/billing?topup=1",
  card: "/settings/billing?card=1",
  limit: "/settings/budgets#usage-limit",
} as const;

/**
 * The web rows, most urgent first, in the glossary's words (kit's `BILLING_TERMS`): a bill to pay, then what pay as
 * you go changes on the web. An organization that does not pay as it goes gets one row, to set billing up.
 * `payg`: the organization pays as it goes. `due`: receipts still to pay.
 */
export function webBillingActions(billing: Pick<BillingResponse, "pastDue" | "paymentMethod" | "defaultPaymentMethod">, payg: boolean, due: number, plans = false): WebBillingAction[] {
  const actions: WebBillingAction[] = [];
  if (billing.pastDue || due > 0) actions.push({ label: BILLING_TERMS.payInvoice, path: PAGES.billing });
  if (plans) actions.push({ label: payg ? "View plans" : "Pick a plan to keep going", path: PAGES.plans });
  if (!payg) return [...actions, { label: BILLING_TERMS.setUpBilling, path: PAGES.billing }];
  const card = billing.paymentMethod ?? billing.defaultPaymentMethod;
  actions.push(
    { label: BILLING_TERMS.buyCredits, path: PAGES.credits },
    { label: BILLING_TERMS.autoTopup, path: PAGES.topup },
    { label: BILLING_TERMS.changeLimit, path: PAGES.limit },
    { label: card ? BILLING_TERMS.updateCard : "Add payment method", path: PAGES.card },
  );
  return actions;
}

/**
 * What pay as you go is and costs, from Gateway's numbers (no price of the phone's own): only beside a link to set it
 * up, so only where the store allows one. Elsewhere no screen says how to pay.
 */
export function payAsYouGoPitch(pricing: Pick<NonNullable<BillingResponse["pricing"]>, "markupBps"> | undefined): string {
  const rate = pricing ? ` plus ${pricing.markupBps / 100}%` : "";
  return `Models Nexus runs cost the provider's price${rate}. Buy ${BILLING_TERMS.credits}, or let a card pay after the fact, up to a monthly ${BILLING_TERMS.usageLimit.toLowerCase()} you set.`;
}
