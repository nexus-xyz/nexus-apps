import { describe, expect, it } from "vitest";
import fixture from "./fixtures/billing-words.json";
import { NO_RECEIPTS, afterTheFactNotice, afterTheFactPhaseOf, chargeProblem, receiptsEmptyText, unpaidUsdOf } from "./after-the-fact";
import type { AfterTheFact, AfterTheFactPhase } from "./api";
import { dayText, payNoticeTitle } from "./plan";
import { CHARGE_FAILED_REASON, CREDITS_SHORT_TITLE, PAY_NOTICE_FALLBACK_TITLE, PAY_NOTICE_TITLE, chargeConfirmText, chargeFailedText, chargeRunningText, finalChargeText, payNoticeActionTerm, paymentRefusalTitle, BILLING_TERMS, PAY_NOTICE_ACTION, TOPUP_OFF_REASON, topupOffText, CHARGE_PROCESSING, CHARGE_PROCESSING_CLI_ACTION, CREDIT_AMOUNTS, TRY_AGAIN, billingAmount, billingCutoff, billingDay, billingSetting, billingStaleNote, pricingChangeLead, usageLimitResetDay, usageLimitResetsAt, usageLimitResetText, usedUpTitle } from "./billing-words";

describe("the billing glossary", () => {
  it("is the fixture the desktop app and the Rust CLI check their strings against", () => {
    expect(BILLING_TERMS).toEqual(fixture.terms);
    expect([...CREDIT_AMOUNTS]).toEqual(fixture.creditAmounts);
  });

  it("writes an amount to the cent and a setting without empty cents", () => {
    for (const row of fixture.money) {
      expect(billingAmount(row.decimal), row.decimal).toBe(row.amount);
      expect(billingSetting(row.decimal), row.decimal).toBe(row.setting);
    }
  });

  it("writes billing days in UTC, whatever the viewer's zone", () => {
    for (const row of fixture.days) {
      expect(billingDay(row.iso)).toBe(row.day);
      expect(billingDay(row.iso, true)).toBe(row.dayWithYear);
      expect(billingCutoff(row.iso)).toBe(row.cutoff);
    }
    // 23:30 UTC on 10 Nov is 11 Nov in Tokyo and still 10 Nov here: the day is Gateway's.
    expect(billingDay("2026-11-10T23:30:00.000Z")).toBe("10 Nov");
    expect(billingDay("not a date")).toBe("not a date");
    expect(billingCutoff("not a date")).toBe("not a date");
  });

  it("says a pricing change's UTC day, ahead until its midnight and past from it", () => {
    const change = { effectiveOn: "2026-11-26" };
    expect(pricingChangeLead(change, new Date("2026-11-25T23:59:59.999Z"))).toBe("Nexus pricing and the Customer Terms change on 26 Nov 2026.");
    expect(pricingChangeLead(change, new Date("2026-11-26T00:00:00.000Z"))).toBe("Nexus pricing and the Customer Terms changed on 26 Nov 2026.");
  });

  it("says when the usage limit resets: the first day of the next UTC month", () => {
    for (const row of fixture.resets) expect(usageLimitResetText(new Date(row.now))).toBe(row.text);
    expect(usageLimitResetsAt(new Date("2026-11-30T23:59:59.999Z"))).toBe("2026-12-01T00:00:00.000Z");
    expect(usageLimitResetDay(new Date("2026-11-10T18:00:00.000Z"))).toBe("1 Dec, 00:00 UTC");
  });

  it("says billing didn't refresh, why, and that the figures shown are older", () => {
    expect(billingStaleNote(fixture.stale.error)).toBe(fixture.stale.note);
    expect(TRY_AGAIN).toBe(fixture.stale.action);
    expect(billingStaleNote("  ")).toBe("Billing didn't refresh. The figures shown are from the last time it loaded.");
  });

  it("titles a used-up organization the same way on every surface", () => {
    expect(usedUpTitle({ kind: "free" })).toBe("Your free start is used up");
    expect(usedUpTitle({ kind: "payg", credits: true, afterTheFact: false })).toBe("Your Nexus credits are used up");
    expect(usedUpTitle({ kind: "paid", name: "Team" })).toBe("Team's Nexus usage is used up");
    expect(usedUpTitle({ kind: "unknown" })).toBe("Your organization's Nexus usage is used up");
    expect(usedUpTitle({ kind: "free" }, 1_000_000)).toBe("Your free $1 is used up");
  });

  it("gives the composer's notice the same titles and dates", () => {
    expect(payNoticeTitle({ kind: "refused", reason: "credits_out" })).toBe(usedUpTitle({ kind: "payg", credits: true, afterTheFact: false }));
    expect(payNoticeTitle({ kind: "refused", reason: "free_start_used_up", granted: 1_000_000 })).toBe("Your free $1 is used up");
    expect(dayText("2026-12-01T00:00:00.000Z")).toBe("1 Dec, 00:00 UTC");
  });

  it("gives each reason Nexus cannot pay one title and one action, the fixture's", () => {
    for (const [reason, words] of Object.entries(fixture.payNotices)) {
      const notice = { kind: "refused", reason } as Parameters<typeof payNoticeTitle>[0];
      expect(payNoticeTitle(notice), reason).toBe(words.title);
      expect(BILLING_TERMS[PAY_NOTICE_ACTION[notice.reason]], reason).toBe(words.action);
    }
    expect(Object.keys(fixture.payNotices).sort()).toEqual(Object.keys(PAY_NOTICE_ACTION).sort());
  });

  it("says why auto top-up is off in one way for each reason", () => {
    for (const [reason, text] of Object.entries(fixture.topupOff)) expect(topupOffText(reason), reason).toBe(text);
    expect(Object.keys(fixture.topupOff).sort()).toEqual(Object.keys(TOPUP_OFF_REASON).sort());
    expect(new Set(Object.values(TOPUP_OFF_REASON)).size).toBe(Object.keys(TOPUP_OFF_REASON).length);
  });

  it("says a payment is still going through in the fixture's words", () => {
    const { cliAction, ...words } = fixture.chargeProcessing;
    expect(CHARGE_PROCESSING).toEqual(words);
    expect(CHARGE_PROCESSING_CLI_ACTION).toBe(cliAction);
    expect(payNoticeTitle({ kind: "refused", reason: "charge_processing" })).toBe(fixture.chargeProcessing.title);
  });

  it("titles a pay notice for a reason this build does not know, and opens Billing for it", () => {
    const { reason, title, action } = fixture.payNoticeFallback;
    const notice = { kind: "refused", reason } as unknown as Parameters<typeof payNoticeTitle>[0];
    expect(payNoticeTitle(notice)).toBe(title);
    expect(PAY_NOTICE_FALLBACK_TITLE).toBe(title);
    expect(BILLING_TERMS[payNoticeActionTerm(reason)]).toBe(action);
    expect(BILLING_TERMS[payNoticeActionTerm("card_disputed")]).toBe("Add another card");
  });

  it("titles a 402 refusal of a Nexus-paid call one way on every surface, by its code and reason", () => {
    for (const row of fixture.refusals) {
      expect(paymentRefusalTitle(row.code, "reason" in row ? row.reason : undefined), `${row.code} ${"reason" in row ? row.reason : ""}`).toBe(row.title);
    }
    expect(CREDITS_SHORT_TITLE).toBe("Your Nexus credits don't cover this call");
    // A 3-D Secure refusal asks to confirm, as the composer's notice does; a code with its own words gets none here.
    expect(paymentRefusalTitle("payment_failed", "authentication_required")).toBe(payNoticeTitle({ kind: "refused", reason: "card_needs_confirming" }));
    expect(paymentRefusalTitle("payment_failed", "a_reason_this_build_does_not_know")).toBe(PAY_NOTICE_TITLE.charge_failed);
    expect(paymentRefusalTitle("usage_limit_reached")).toBeUndefined();
    expect(paymentRefusalTitle(undefined)).toBeUndefined();
  });

  it("says why a charge failed in the fixture's words, and leaves an unknown reason out", () => {
    expect(CHARGE_FAILED_REASON).toEqual(fixture.chargeFailedReasons);
    expect(new Set(Object.values(CHARGE_FAILED_REASON)).size).toBe(Object.keys(CHARGE_FAILED_REASON).length);
    expect(chargeFailedText("no_payment_method")).toBe("The last charge didn't go through: no payment method is on file");
    expect(chargeFailedText("card_expired", { amountUsd: "12.000000" })).toBe("The last charge of $12.00 didn't go through");
    expect(chargeFailedText(undefined)).toBe("The last charge didn't go through");
    // An amount under a cent is never said: no "$0.00" charge.
    expect(chargeFailedText("declined", { amountUsd: "0.004000" })).toBe("The last charge didn't go through: the card was declined");
    expect(chargeConfirmText({ amountUsd: "0.000000" })).toBe("The charge needs the cardholder's confirmation");
    expect(chargeRunningText({ amountUsd: "4.500000", final: true })).toBe("The final charge of $4.50 is going through");
    expect(finalChargeText("0.500000")).toBe("A final charge of $0.50 runs once open calls settle");
    expect(finalChargeText("0.900000", "1.00")).toBe("Less than $1 of usage is unpaid. It stays unpaid until paying after the fact is on again");
  });
});

describe("paying after the fact's phase and notice", () => {
  const base = fixture.afterTheFactBase as AfterTheFact;
  const of = (patch: object) => ({ ...base, ...patch }) as AfterTheFact;

  it("gives each phase the fixture's notice, from Gateway's phase or an older Gateway's fields", () => {
    for (const row of fixture.afterTheFact) {
      const after = of(row.patch);
      expect(afterTheFactPhaseOf(after), row.case).toBe(row.phase);
      expect(afterTheFactNotice(after, "0.50"), row.case).toEqual(row.notice);
    }
    const phases = new Set(fixture.afterTheFact.map((row) => row.phase));
    for (const phase of ["off", "off_owing", "on", "charging", "failed", "retrying", "needs_confirmation", "disputed", "closed"]) expect(phases.has(phase), phase).toBe(true);
  });

  it("says why the Receipts list is empty while usage waits for its first charge", () => {
    expect(NO_RECEIPTS).toBe(fixture.receiptsEmpty.plain);
    for (const row of fixture.receiptsEmpty.examples) {
      const after = row.patch ? of(row.patch) : null;
      expect(receiptsEmptyText(after), row.case).toBe(row.text ?? fixture.receiptsEmpty.plain);
    }
  });

  it("names the unpaid usage, and the owed usage only from an older Gateway", () => {
    expect(unpaidUsdOf(of({ unpaidUsd: "12.000000", owedUsd: "0.000000" }))).toBe("12.000000");
    expect(unpaidUsdOf(of({ owedUsd: "3.000000" }))).toBe("3.000000");
  });

  it("stops calls only while the setting is on and a charge has a problem", () => {
    expect(["failed", "retrying", "needs_confirmation", "disputed"].every((phase) => chargeProblem(phase as AfterTheFactPhase))).toBe(true);
    expect(["off", "off_owing", "on", "charging", "closed"].some((phase) => chargeProblem(phase as AfterTheFactPhase))).toBe(false);
    for (const reason of ["declined", "authentication_required", "disputed"] as const) {
      const failed = { at: "2026-11-12T10:00:00.000Z", reason };
      expect(afterTheFactNotice(of({ failed }))?.stopsCalls, reason).toBe(true);
      // Off: nothing runs on the card, so nothing is paused, and no notice says so.
      const off = afterTheFactNotice(of({ failed, enabled: false }));
      expect(off?.stopsCalls, reason).toBe(false);
      expect(`${off?.title} ${off?.body}`, reason).not.toMatch(/paused/i);
      expect(off?.title, reason).toMatch(/^The final charge|disputed/);
    }
  });
});
