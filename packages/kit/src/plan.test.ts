import { describe, expect, it } from "vitest";
import type { NexusPays, SubsidyView } from "./api";
import { payLineParts, payLineText, payNoticeOf, payNoticeTitle, usageOf } from "./plan";

const NOW = Date.parse("2026-11-10T12:00:00.000Z");
const DAY = 86_400_000;

const subsidy = (left: string, granted = "2.000000"): SubsidyView => ({
  grantedUsd: Number(granted),
  spentUsd: 0,
  reservedUsd: 0,
  remainingUsd: Number(left),
  grantedUsdDecimal: granted,
  remainingUsdDecimal: left,
  fundedBy: "nexus",
  usableForModelCalls: true,
});
const pays = (payer: NexusPays["payer"], changesAt: string | null = null): NexusPays => ({ pays: true, payer, reason: null, changesAt });
const refused = (reason: NexusPays["reason"], changesAt: string | null = null): NexusPays => ({ pays: false, payer: null, reason, changesAt });

describe("usageOf", () => {
  it("trusts nexusPays over payAfterTheFact: a card that is on can still not pay", () => {
    // The card was on, but its last charge failed.
    expect(usageOf({ subsidy: subsidy("0"), payAfterTheFact: true, nexusPays: refused("charge_failed") })?.usedUp).toBe(true);
    expect(usageOf({ subsidy: subsidy("0"), payAfterTheFact: true, nexusPays: pays("card") })?.usedUp).toBe(false);
    // An older Gateway: the card that is on still counts as paying.
    expect(usageOf({ subsidy: subsidy("0"), payAfterTheFact: true })?.usedUp).toBe(false);
  });

  it("is low from 80% used, only while the free start pays", () => {
    expect(usageOf({ subsidy: subsidy("0.400000"), nexusPays: pays("free_start") })?.low).toBe(true);
    expect(usageOf({ subsidy: subsidy("0.410000"), nexusPays: pays("free_start") })?.low).toBe(false);
    expect(usageOf({ subsidy: subsidy("0.100000"), nexusPays: pays("credits") })?.low).toBe(false);
  });
});

describe("payNoticeOf", () => {
  it("names the reason Nexus cannot pay, for a member with no subsidy too", () => {
    expect(payNoticeOf({ nexusPays: refused("credits_out") }, NOW)).toEqual({ kind: "refused", reason: "credits_out", granted: undefined, changesAt: undefined });
    const limit = payNoticeOf({ nexusPays: refused("usage_limit_reached", "2026-12-01T00:00:00.000Z") }, NOW);
    expect(limit).toMatchObject({ reason: "usage_limit_reached", changesAt: "2026-12-01T00:00:00.000Z" });
  });

  it("tells expired from used up, with the amount Gateway granted", () => {
    const expired = payNoticeOf({ subsidy: subsidy("0"), nexusPays: refused("free_start_expired") }, NOW);
    const used = payNoticeOf({ subsidy: subsidy("0", "1.000000"), nexusPays: refused("free_start_used_up") }, NOW);
    expect(expired?.kind === "refused" && payNoticeTitle(expired)).toBe("Your free $2 expired");
    expect(used?.kind === "refused" && payNoticeTitle(used)).toBe("Your free $1 is used up");
    expect(payNoticeTitle({ kind: "refused", reason: "free_start_used_up" })).toBe("Your free start is used up");
  });

  it("says quietly when the free start is 80% used or ends within a week", () => {
    const soon = new Date(NOW + 3 * DAY - 1000).toISOString();
    const line = payNoticeOf({ subsidy: subsidy("0.300000"), nexusPays: pays("free_start", soon) }, NOW);
    expect(line).toEqual({ kind: "line", granted: 2_000_000, usedPercent: 85, daysLeft: 3 });
    expect(line?.kind === "line" && payLineText(line)).toBe("Free $2 · 85% used · expires in 3 days");
    const later = new Date(NOW + 20 * DAY).toISOString();
    expect(payNoticeOf({ subsidy: subsidy("1.000000"), nexusPays: pays("free_start", later) }, NOW)).toBeUndefined();
    const today = payNoticeOf({ subsidy: subsidy("1.000000"), nexusPays: pays("free_start", new Date(NOW).toISOString()) }, NOW);
    expect(today?.kind === "line" && payLineText(today)).toBe("Free $2 · expires today");
    // Credits pay: the free start's end changes nothing.
    expect(payNoticeOf({ subsidy: subsidy("0.100000"), nexusPays: pays("credits") }, NOW)).toBeUndefined();
  });

  it("says quietly when the card has paid 80% of this month's usage limit, with when it resets", () => {
    const card = (limitUsedPercent: number | null | undefined): NexusPays => ({ ...pays("card"), limitUsedPercent });
    const line = payNoticeOf({ nexusPays: card(85) }, NOW);
    expect(line).toEqual({ kind: "line", limitUsedPercent: 85, resetsAt: "2026-12-01T00:00:00.000Z" });
    expect(line?.kind === "line" && payLineText(line)).toBe("Usage limit · 85% used · resets on 1 Dec, 00:00 UTC");
    expect(payNoticeOf({ nexusPays: card(80) }, NOW)).toMatchObject({ kind: "line", limitUsedPercent: 80 });
    expect(payNoticeOf({ nexusPays: card(79) }, NOW)).toBeUndefined();
    expect(payNoticeOf({ nexusPays: card(null) }, NOW)).toBeUndefined();
    expect(payNoticeOf({ nexusPays: card(undefined) }, NOW)).toBeUndefined();
  });

  it("splits the quiet line into what it is, how much is used and when it changes, so each app sets the share apart", () => {
    expect(payLineParts({ kind: "line", limitUsedPercent: 85, resetsAt: "2026-12-01T00:00:00.000Z" })).toEqual({
      label: "Usage limit", usedPercent: 85, used: "85% used", when: "resets on 1 Dec, 00:00 UTC",
    });
    expect(payLineParts({ kind: "line", granted: 2_000_000, usedPercent: 82, daysLeft: 3 })).toEqual({
      label: "Free $2", usedPercent: 82, used: "82% used", when: "expires in 3 days",
    });
    expect(payLineParts({ kind: "line", granted: 2_000_000, daysLeft: 0 })).toEqual({ label: "Free $2", when: "expires today" });
    // A share past the meter's end shows in full, and the meter stays within it.
    expect(payLineParts({ kind: "line", limitUsedPercent: 140, resetsAt: "2026-12-01T00:00:00.000Z" }).usedPercent).toBe(100);
  });

  it("falls back to the free start's balance on an older Gateway", () => {
    expect(payNoticeOf({ subsidy: subsidy("0") }, NOW)).toMatchObject({ kind: "refused", reason: "free_start_used_up" });
    expect(payNoticeOf({ subsidy: subsidy("0"), payAfterTheFact: true }, NOW)).toBeUndefined();
  });
});
