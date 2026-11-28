import { CoreError, planOf, type AfterTheFact, type BillingPricing, type Plan, type Receipt } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { ALWAYS_FREE, RECEIPTS_SHOWN, afterTheFactNoticeOf, freeStartLeft, showsUsageLimit, usageLimitLine, brandName, receiptsByMonth, cardExpired, freeLine, limitInput, lowerLimitProblem, moreReceipts, promotionalLine, monthLabel, monthStart, noBilling, openReceipts, pastDueStopped, paygLine, paymentLabel, receiptLine, receiptLink, setting, topupLine, topupOffAlert, topupOffLine, usd } from "./billing";

const atf = (monthSpentUsd: string): AfterTheFact => ({
  enabled: true,
  limitUsd: "50000.000000",
  monthSpentUsd,
  reservedUsd: "0",
  leftUsd: "49815.800000",
  owedUsd: "4.200000",
  thresholdUsd: "10.000000",
  pending: false,
  failed: null,
});
const credits = (remainingUsd: string) => ({ purchasedUsd: "500.000000", spentUsd: "94.000000", reservedUsd: "0", remainingUsd, markupBps: 800 });
const now = new Date("2026-11-03T12:00:00Z");

describe("billing", () => {
  it("only lowers the usage limit on the phone, which buys nothing in any storefront", () => {
    expect(lowerLimitProblem("10", "25.000000")).toBeNull();
    expect(lowerLimitProblem("$1,000.50", "5000")).toBeNull();
    expect(lowerLimitProblem("1", "25")).toBeNull();
    expect(lowerLimitProblem("25", "25.000000")).toBe("Type less than $25. The phone only lowers the limit.");
    expect(lowerLimitProblem("30", "25")).toBe("Type less than $25. The phone only lowers the limit.");
    expect(lowerLimitProblem("0.50", "25")).toBe("The limit is at least $1.");
    expect(lowerLimitProblem("ten", "25")).toBe("Type the limit in dollars, such as 25 or 10.50.");
    expect(lowerLimitProblem("10.505", "25")).toBe("Type the limit in dollars, such as 25 or 10.50.");
    expect(limitInput(" $1,000.50 ")).toBe("1000.50");
  });

  it("meters what is left of the free start, and nothing once it expired or there is none", () => {
    expect(freeStartLeft({ grantedUsd: "1.000000", remainingUsd: "0.400000" })).toBeCloseTo(0.4);
    expect(freeStartLeft({ grantedUsd: "1", remainingUsd: "0" })).toBe(0);
    expect(freeStartLeft({ grantedUsd: "1", remainingUsd: "0.4", expiredUsd: "0.4" })).toBeUndefined();
    expect(freeStartLeft({ grantedUsd: "0", remainingUsd: "0" })).toBeUndefined();
    expect(freeStartLeft(null)).toBeUndefined();
  });

  it("offers only free ways to keep going, which every storefront may show", () => {
    expect(ALWAYS_FREE.map((way) => way.href)).toEqual(["/keys", "/agents"]);
    for (const way of ALWAYS_FREE) {
      expect(`${way.label} ${way.detail}`).not.toMatch(/\$|buy|purchase|billing|credits|card|upgrade|on the web|pay as you go/i);
    }
  });

  it("writes an amount to the cent and a setting without empty cents, as every surface does", () => {
    expect(usd("406.000000")).toBe("$406.00");
    expect(usd("50000")).toBe("$50,000.00");
    expect(usd("184.2")).toBe("$184.20");
    expect(usd("0.004000")).toBe("$0.00");
    expect(usd("0")).toBe("$0.00");
    expect(usd("not money")).toBe("not money");
    expect(setting("25.000000")).toBe("$25");
    expect(setting("10.5")).toBe("$10.50");
  });

  it("says how the organization pays, in one line", () => {
    expect(paygLine({ credits: credits("406.000000"), afterTheFact: atf("184.200000") }, now)).toBe("$406.00 in credits · $184.20 since 1 Nov");
    expect(paygLine({ credits: credits("406"), afterTheFact: null }, now)).toBe("$406.00 in credits");
    expect(paygLine({ credits: null, afterTheFact: atf("0") }, now)).toBe("$0.00 since 1 Nov");
    expect(monthStart(new Date("2026-12-31T23:30:00Z"))).toBe("1 Dec");
    expect(freeLine({ freeStart: { grantedUsd: "1.000000", remainingUsd: "0.400000" } })).toBe("$0.40 of your free $1 left");
    expect(freeLine({ freeStart: { grantedUsd: "1.000000", remainingUsd: "0" } })).toBe("Your free start of $1 is used up");
    expect(freeLine({ freeStart: null })).toBe("Your own keys and Claude or ChatGPT plans are free.");
    const dated = { grantedUsd: "1.000000", remainingUsd: "0.400000", expiresAt: "2026-12-03T09:30:00.000Z", expiredUsd: null };
    expect(freeLine({ freeStart: dated })).toBe("$0.40 of your free $1 left · until 3 Dec, 09:30 UTC");
    expect(freeLine({ freeStart: { ...dated, remainingUsd: "0.000000", expiredUsd: "0.400000" } })).toBe("Your free start expired on 3 Dec, 09:30 UTC");
    expect(freeLine({ freeStart: { ...dated, expiresAt: "soon" } })).toBe("$0.40 of your free $1 left · until soon");
  });

  it("names the payment method and counts the receipts still to pay", () => {
    expect(paymentLabel({ paymentMethod: { brand: "visa", last4: "4242", expMonth: 4, expYear: 2027 } })).toBe("Visa ·· 4242");
    expect(paymentLabel({ paymentMethod: null, defaultPaymentMethod: { type: "us_bank_account", bankName: "Chase", last4: "6789" } })).toBe("Chase ·· 6789");
    expect(paymentLabel({ paymentMethod: null, defaultPaymentMethod: { type: "link" } })).toBe("Link");
    expect(paymentLabel({ paymentMethod: null })).toBeUndefined();
    expect(openReceipts([{ status: "paid" }, { status: "open" }, { status: "failed" }, { status: "void" }])).toBe(2);
  });

  it("names months and cards", () => {
    expect(monthLabel("2026-10")).toBe("October 2026");
    expect(brandName("amex")).toBe("American Express");
    expect(brandName("unknown")).toBe("Card");
    expect(cardExpired({ brand: "visa", last4: "4242", expMonth: 9, expYear: 2026 }, new Date("2026-10-22T00:00:00Z"))).toBe(true);
    expect(cardExpired({ brand: "visa", last4: "4242", expMonth: 10, expYear: 2026 }, new Date("2026-10-22T00:00:00Z"))).toBe(false);
  });

  it("says whether calls have stopped for an unpaid month", () => {
    expect(pastDueStopped({ since: "", period: "2026-09", blockedFrom: "2026-10-01T00:00:00Z" }, Date.parse("2026-10-02T00:00:00Z"))).toBe(true);
    expect(pastDueStopped({ since: "", period: "2026-09", blockedFrom: null })).toBe(false);
  });

  it("reads a Gateway without Stripe as no billing, and any other failure as a failure", () => {
    expect(noBilling(new CoreError(503, "Stripe needs STRIPE_SECRET_KEY", "not_configured"))).toBe(true);
    expect(noBilling(new CoreError(404, "Not found"))).toBe(true);
    expect(noBilling(new CoreError(503, "Upstream down", "upstream_unavailable"))).toBe(false);
    expect(noBilling(new Error("fetch failed"))).toBe(false);
  });
});

describe("Billing's credits and receipts (P40·D)", () => {
  const topup = { enabled: true, belowUsd: "1000.00", addUsd: "5000.00", offReason: null, todayCount: 0, todayUsd: "0", pending: false };
  it("writes the auto top-up as set, and nothing while it is off", () => {
    expect(topupLine(topup)).toBe("Auto top-up: under $1,000, add $5,000");
    expect(topupLine({ ...topup, enabled: false })).toBeUndefined();
    expect(topupLine(null)).toBeUndefined();
  });
  it("gives a receipt its day, and its state unless it is paid", () => {
    expect(receiptLine({ at: "2026-11-08T12:00:00Z", status: "paid" })).toBe("8 Nov");
    expect(receiptLine({ at: "2026-11-08T12:00:00Z", status: "open" })).toBe("8 Nov · Open");
  });
});


it("keeps promotional rewards separate and available after welcome credit expires", () => {
  expect(promotionalLine({ promotionalCreditUsd: "2.50" })).toBe("$2.50");
  expect(promotionalLine({ promotionalCreditUsd: "0" })).toBeUndefined();
  expect(promotionalLine({})).toBeUndefined();
});

describe("Billing's auto top-up when Gateway turned it off", () => {
  const off = { enabled: false, belowUsd: "10.00", addUsd: "25.00", todayCount: 0, todayUsd: "0", pending: false };
  it("says why, refunds and disputes included, and nothing while an owner keeps it", () => {
    expect(topupOffLine({ ...off, offReason: "refunded" })).toBe("Auto top-up is off: a credit purchase was refunded");
    expect(topupOffLine({ ...off, offReason: "disputed" })).toBe("Auto top-up is off: a payment on the card was disputed");
    expect(topupOffLine({ ...off, offReason: "declined" })).toBe("Auto top-up is off: the card was declined");
    expect(topupOffLine({ ...off, offReason: null })).toBeUndefined();
    expect(topupOffLine(null)).toBeUndefined();
  });
});

describe("which receipts open from the phone", () => {
  const receipt = (status: Receipt["status"], extra: Partial<Receipt> = {}): Receipt => ({
    id: "pi_1", at: "2026-11-08T10:00:00Z", kind: "credits", description: "Nexus credits", amountUsd: "25.000000", status, url: null, receipt: false, ...extra,
  });
  it("opens a payment's receipt page, and a settled month's invoice, never a page that takes a payment", () => {
    expect(receiptLink(receipt("paid", { receipt: true }))).toBe("receipt");
    expect(receiptLink(receipt("paid", { id: "month:2026-10", kind: "invoice", url: "https://invoice.test/i/1" }))).toBe("invoice");
    expect(receiptLink(receipt("void", { id: "month:2026-09", kind: "invoice", url: "https://invoice.test/i/2" }))).toBe("invoice");
    expect(receiptLink(receipt("open", { id: "month:2026-10", kind: "invoice", url: "https://invoice.test/i/1" }))).toBeUndefined();
    expect(receiptLink(receipt("failed", { id: "month:2026-10", kind: "invoice", url: "https://invoice.test/i/1" }))).toBeUndefined();
    expect(receiptLink(receipt("paid"))).toBeUndefined();
  });
  it("lists every receipt on its own screen only when Billing shows some of them", () => {
    expect(moreReceipts(undefined)).toBe(false);
    expect(moreReceipts(Array.from({ length: RECEIPTS_SHOWN }, () => receipt("paid")))).toBe(false);
    expect(moreReceipts(Array.from({ length: RECEIPTS_SHOWN + 1 }, () => receipt("paid")))).toBe(true);
  });
});

describe("every receipt, by month", () => {
  const at = (iso: string, id: string): Receipt => ({ id, at: iso, kind: "credits", description: "Nexus credits", amountUsd: "10.000000", status: "paid", url: null, receipt: true });
  it("groups them under their UTC month, newest first, as Gateway lists them", () => {
    const groups = receiptsByMonth([at("2026-11-08T10:00:00Z", "a"), at("2026-11-01T00:30:00Z", "b"), at("2026-10-31T23:59:00Z", "c")]);
    expect(groups.map((g) => [g.month, g.receipts.map((r) => r.id)])).toEqual([["November 2026", ["a", "b"]], ["October 2026", ["c"]]]);
    expect(receiptsByMonth([])).toEqual([]);
  });
});

describe("paying after the fact, by the charge's phase", () => {
  const free: Plan = { kind: "free" };
  const failed = (reason: string) => ({ at: "2026-11-02T00:00:00.000Z", reason }) as AfterTheFact["failed"];
  const of = (patch: Partial<AfterTheFact>, pricing?: BillingPricing) => {
    const after = { ...atf("184.200000"), ...patch };
    const notice = afterTheFactNoticeOf({ afterTheFact: after, pricing });
    return { notice, line: usageLimitLine(after, notice, now) };
  };

  it("says what this month used while on, and nothing more", () => {
    const { notice, line } = of({ phase: "on" });
    expect(notice).toBeNull();
    expect(line).toEqual({ text: "$184.20 used. Resets on 1 Dec, 00:00 UTC", problem: false });
  });

  it("names the unpaid usage in each problem phase, never the $0.00 owed, and says Paused while calls stop", () => {
    const cases = [
      [{ phase: "failed", unpaidUsd: "12.000000", failed: failed("declined") }, "The last charge of $12.00 didn't go through: the card was declined. Paused.", "fixPayment"],
      [{ phase: "retrying", pending: true, owedUsd: "0", unpaidUsd: "12.300000", chargeUsd: "12.300000", failed: failed("declined") }, "The last charge of $12.30 didn't go through. Nexus is trying it again now. Paused.", "fixPayment"],
      [{ phase: "needs_confirmation", pending: true, owedUsd: "0", unpaidUsd: "12.300000", failed: failed("authentication_required") }, "The charge of $12.30 needs the cardholder's confirmation. Paused.", "confirmPayment"],
      [{ phase: "disputed", unpaidUsd: "12.300000", failed: failed("disputed") }, "A payment on the card was disputed. Paused.", "addAnotherCard"],
    ] as const;
    for (const [patch, text, action] of cases) {
      const { notice, line } = of(patch);
      expect(line).toEqual({ text, problem: true });
      expect(notice).toMatchObject({ action, stopsCalls: true });
      expect(notice?.body).toMatch(/Credits and your own keys still work$/);
      expect(line.text).not.toContain("$0.00");
    }
  });

  it("says a charge is going through calmly, with nothing to do", () => {
    const { notice, line } = of({ phase: "charging", pending: true, owedUsd: "0", unpaidUsd: "10.400000", chargeUsd: "10.400000" });
    expect(line).toEqual({ text: "A charge of $10.40 is going through.", problem: false });
    expect(notice?.action).toBeUndefined();
  });

  it("says an off setting's final charge failed, and pauses nothing: nothing runs on the card", () => {
    const { notice, line } = of({ phase: "failed", enabled: false, unpaidUsd: "3.200000", failed: failed("no_payment_method") });
    expect(line).toEqual({ text: "The final charge of $3.20 didn't go through: no payment method is on file.", problem: true });
    expect(notice).toMatchObject({ action: "fixPayment", stopsCalls: false, body: "Nothing else runs on the card. An owner fixes the card so the charge goes through" });
    expect(`${line.text} ${notice?.body}`).not.toMatch(/paused/i);
  });

  it("says when the final charge runs once off, from Gateway's minimum", () => {
    expect(of({ phase: "off_owing", enabled: false, unpaidUsd: "3.200000" }).line).toEqual({ text: "A final charge of $3.20 runs once open calls settle.", problem: false });
    const small = of({ phase: "off_owing", enabled: false, unpaidUsd: "0.300000" }, { minFinalChargeUsd: "0.75" } as BillingPricing);
    expect(small.line.text).toBe("Less than $0.75 of usage is unpaid. It stays unpaid until paying after the fact is on again.");
  });

  it("says nothing more once off with nothing owed, or closed", () => {
    expect(of({ phase: "off", enabled: false, unpaidUsd: "0", owedUsd: "0" })).toEqual({ notice: null, line: { problem: false } });
    expect(of({ phase: "closed", enabled: false, unpaidUsd: "3.200000" }).notice).toBeNull();
  });

  it("never says 'undefined' for a failed reason it does not know", () => {
    const { line } = of({ phase: "failed", unpaidUsd: "12.000000", failed: failed("card_expired") });
    expect(line.text).toBe("The last charge of $12.00 didn't go through. Paused.");
  });

  it("works the phase out on an older Gateway, which sends no phase and no unpaid usage", () => {
    expect(of({ owedUsd: "12.000000", failed: failed("authentication_required") }).line.text).toBe("The charge of $12.00 needs the cardholder's confirmation. Paused.");
    expect(of({ owedUsd: "12.000000", failed: failed("declined") }).line.text).toBe("The last charge of $12.00 didn't go through: the card was declined. Paused.");
    // No `chargeUsd` either: the running charge is not named, rather than named wrongly.
    expect(of({ pending: true }).line).toEqual({ text: "A charge is going through.", problem: false });
    expect(of({ enabled: false, owedUsd: "3.200000" }).line.text).toBe("A final charge of $3.20 runs once open calls settle.");
    expect(of({}).notice).toBeNull();
  });

  it("keeps the usage limit row for an organization that turned it off with a failed charge or money owed", () => {
    const off = { ...atf("0"), enabled: false, unpaidUsd: "3.200000" };
    // `planOf` says Free: no credits, and paying after the fact is off.
    expect(planOf(false, { credits: null, afterTheFact: off })).toEqual(free);
    expect(showsUsageLimit(free, afterTheFactNoticeOf({ afterTheFact: { ...off, phase: "off_owing" } }))).toBe(true);
    expect(showsUsageLimit(free, afterTheFactNoticeOf({ afterTheFact: { ...off, phase: "failed", failed: failed("declined") } }))).toBe(true);
    expect(showsUsageLimit(free, afterTheFactNoticeOf({ afterTheFact: { ...off, phase: "off", unpaidUsd: "0" } }))).toBe(false);
    expect(showsUsageLimit(free, afterTheFactNoticeOf({ afterTheFact: null }))).toBe(false);
    expect(showsUsageLimit({ kind: "payg", credits: true, afterTheFact: false }, null)).toBe(true);
  });
});

describe("Billing's card while Gateway keeps auto top-up off", () => {
  const off = { enabled: false, belowUsd: "10.00", addUsd: "25.00", todayCount: 0, todayUsd: "0", pending: false };
  it("says why and that credits are no longer bought, and nothing for a reason it does not know", () => {
    expect(topupOffAlert({ ...off, offReason: "declined" })).toEqual({
      title: "Auto top-up is off: the card was declined.",
      body: "Nexus credits are no longer bought when they run low.",
    });
    expect(topupOffAlert({ ...off, offReason: "refunded" })?.title).toBe("Auto top-up is off: a credit purchase was refunded.");
    expect(topupOffAlert({ ...off, offReason: null })).toBeUndefined();
    expect(topupOffAlert({ ...off, offReason: "closed" })).toBeUndefined();
    expect(topupOffLine({ ...off, offReason: "closed" })).toBe("Auto top-up is off: the organization was closed");
    expect(topupOffAlert(null)).toBeUndefined();
  });
});
