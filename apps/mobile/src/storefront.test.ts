import { describe, expect, it } from "vitest";
import { payAsYouGoPitch, webBillingActions, webBillingAllowed } from "./storefront";

const pastDue = { period: "2026-10", amountUsd: "12.000000", blockedFrom: null } as never;
const card = { brand: "visa", last4: "4242", expMonth: 1, expYear: 2030 } as never;

describe("storefront", () => {
  it("links to the web only from the US App Store storefront", () => {
    expect(webBillingAllowed("ios", "USA")).toBe(true);
    expect(webBillingAllowed("ios", "usa")).toBe(true);
  });

  it("links nowhere else: other storefronts, an unknown one, and Android", () => {
    for (const storefront of ["MEX", "BRA", "COL", "ARG", "CHL", "CAN", "GBR", "DEU", "JPN", "KOR", "US", ""]) {
      expect(webBillingAllowed("ios", storefront)).toBe(false);
    }
    expect(webBillingAllowed("ios", null)).toBe(false);
    expect(webBillingAllowed("ios", undefined)).toBe(false);
    // Google Play's US link-out needs its own program: Android never links, whatever the country.
    expect(webBillingAllowed("android", "USA")).toBe(false);
    expect(webBillingAllowed("web", "USA")).toBe(false);
  });

  it("offers setting billing up to an organization that does not pay as it goes", () => {
    expect(webBillingActions({ pastDue: null }, false, 0)).toEqual([{ label: "Set up billing", path: "/settings/billing" }]);
  });

  it("puts a bill to pay first, then what pay as you go changes on the web, in the glossary's words", () => {
    expect(webBillingActions({ pastDue, paymentMethod: card }, true, 0).map((a) => a.label)).toEqual([
      "Pay invoice",
      "Buy credits",
      "Auto top-up",
      "Change limit",
      "Update card",
    ]);
    expect(webBillingActions({ pastDue: null }, true, 1)[0]).toEqual({ label: "Pay invoice", path: "/settings/billing" });
  });

  it("opens the parts of Nexus UI's Billing that read a deep link", () => {
    const paths = Object.fromEntries(webBillingActions({ pastDue: null }, true, 0).map((a) => [a.label, a.path]));
    expect(paths).toEqual({
      "Buy credits": "/settings/billing?buy=credits",
      "Auto top-up": "/settings/billing?topup=1",
      "Change limit": "/settings/budgets#usage-limit",
      "Add payment method": "/settings/billing?card=1",
    });
  });

  it("links to the plan picker when the catalogue offers v3 plans", () => {
    expect(webBillingActions({ pastDue: null }, false, 0, true)).toEqual([
      { label: "Pick a plan to keep going", path: "/settings/billing#plans" },
      { label: "Set up billing", path: "/settings/billing" },
    ]);
  });

  it("says what pay as you go costs from Gateway's rate, and none of its own", () => {
    expect(payAsYouGoPitch({ markupBps: 800 })).toBe(
      "Models Nexus runs cost the provider's price plus 8%. Buy Nexus credits, or let a card pay after the fact, up to a monthly usage limit you set.",
    );
    expect(payAsYouGoPitch(undefined)).toMatch(/^Models Nexus runs cost the provider's price\. /);
  });
});
