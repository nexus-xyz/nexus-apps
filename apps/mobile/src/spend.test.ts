import type { PersonBudget, SpendResponse } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { budgetCard, costOf, creditCard, leftForYou, limitCard, meter, productRows, settingsLine, spendSource, spentLabel, thisMonth } from "./spend";

const calls = { byProduct: [], recent: [] };
// An owner's answer as the Rust gateway sends it: each number beside its exact decimal twin. The numbers are the
// floats that round the wrong way, so a screen that reads them shows a different cent.
const owner: SpendResponse = {
  budgetUsd: 100,
  budgetUsdDecimal: "100.000000",
  spentUsd: 12.244999,
  spentUsdDecimal: "12.245000",
  reservedUsd: 1.004999,
  reservedUsdDecimal: "1.005000",
  remainingUsd: 86.75,
  remainingUsdDecimal: "86.750000",
  scope: "org",
  ...calls,
};
// The same answer from a Gateway that sends no twin.
const ownerNumbers: SpendResponse = { budgetUsd: 100, spentUsd: 12.245, reservedUsd: 1.005, remainingUsd: 86.75, scope: "org", ...calls };

describe("spend", () => {
  it("names sources as Nexus UI does, a module by its manifest", () => {
    expect(spendSource("core")).toEqual({ product: "Gateway", capability: "Direct API calls" });
    expect(spendSource("review", "pr")).toEqual({ product: "Review", capability: "Pr" });
    const manifests = [{ product: { slug: "review", name: "Nexus Review" }, capability: { slug: "pr", name: "PR review" } }] as never;
    expect(spendSource("review", "pr", manifests)).toEqual({ product: "Nexus Review", capability: "PR review" });
  });

  it("lists products costliest first with their share of the exact total", () => {
    const rows = productRows([
      { product: "chat", costUsd: 1, calls: 3 },
      { product: "core", costUsd: 3, calls: 1 },
    ]);
    expect(rows.map((r) => [r.product, r.cost, r.share])).toEqual([
      ["core", "$3.00", 0.75],
      ["chat", "$1.00", 0.25],
    ]);
    expect(productRows([{ product: "chat", costUsd: 0, calls: 0 }])[0]?.share).toBe(0);
  });

  it("reads a product's and a call's cost from the twin, else the number", () => {
    const rows = productRows([
      { product: "chat", costUsd: 0.1, costUsdDecimal: "0.100000", calls: 1 },
      { product: "core", costUsd: 0.2, costUsdDecimal: "0.200000", calls: 1 },
      { product: "agents", costUsd: 10.00015, costUsdDecimal: "10.000150", calls: 1 },
    ]);
    expect(rows.map((r) => r.cost)).toEqual(["$10.0002", "$0.20", "$0.10"]);
    // The total is the integers' sum: 0.1 and 0.2 add to 300,000 micro-dollars, not to 0.30000000000000004.
    const dimes = productRows([
      { product: "chat", costUsd: 0.1, calls: 1 },
      { product: "core", costUsd: 0.2, calls: 1 },
    ]);
    expect(dimes.map((row) => row.share)).toEqual([200_000 / 300_000, 100_000 / 300_000]);
    expect(costOf({ costUsd: 1.005, costUsdDecimal: "1.005000" })).toBe(1_005_000);
    expect(costOf({ costUsd: 1.005 })).toBe(1_005_000);
  });

  it("keeps the meter within the budget, reserved after spent", () => {
    expect(meter(10_000_000, 4_000_000, 1_000_000)).toEqual({ spent: 0.4, reserved: 0.1 });
    expect(meter(10_000_000, 12_000_000, 3_000_000)).toEqual({ spent: 1, reserved: 0 });
    expect(meter(0, 5_000_000, 1_000_000)).toEqual({ spent: 0, reserved: 0 });
  });

  it("shows a member only what is left for them, with when it resets and what Nexus paid", () => {
    expect(leftForYou({ remainingUsd: 1.25, resetsAt: "2026-10-18T00:00:00.000Z", scope: "own", ...calls })).toEqual({ label: "Left for you", left: 1_250_000, note: "Resets 2026-10-18 (UTC)" });
    expect(leftForYou({ remainingUsd: 3, scope: "own", subsidizedUsd: 0.3, ...calls })).toEqual({ label: "Left for you", left: 3_000_000, subsidized: 300_000 });
    expect(leftForYou({ remainingUsd: 3, remainingUsdDecimal: "2.999999", subsidizedUsd: 0.3, subsidizedUsdDecimal: "0.300001", scope: "own", ...calls })).toEqual({ label: "Left for you", left: 2_999_999, subsidized: 300_001 });
    expect(leftForYou(owner)).toBeUndefined();
    // A member of the TypeScript gateway, which still sends them the organization's budget, sees it as before.
    expect(leftForYou({ ...ownerNumbers, scope: "own" })).toBeUndefined();
  });
});

describe("the Spend and Gateway budget card", () => {
  it("shows the twins exactly, and the percent from the integers", () => {
    expect(budgetCard(owner)).toEqual({
      budget: "$100.00",
      spent: "$12.2450",
      reserved: "$1.0050",
      remaining: "$86.75",
      percent: "12.25% of budget",
      meter: { spent: 0.12245, reserved: 0.01005 },
    });
  });

  it("falls back to the numbers without the twins", () => {
    const card = budgetCard(ownerNumbers);
    expect([card.budget, card.spent, card.reserved, card.remaining]).toEqual(["$100.00", "$12.2450", "$1.0050", "$86.75"]);
    expect(budgetCard({ remainingUsd: 0, scope: "org", ...calls }).percent).toBeUndefined();
  });
});

describe("Spend's own limit", () => {
  const person = { limitUsd: 5, spentUsd: 1.004999, reservedUsd: 0, remainingUsd: 3.995 } as Partial<PersonBudget>;

  it("reads the twins first", () => {
    const card = limitCard({ ...person, limitUsdDecimal: "5.000000", spentUsdDecimal: "1.005000", reservedUsdDecimal: "0.000000", remainingUsdDecimal: "3.995000" });
    expect(card).toEqual({ limit: "$5.00", spent: "$1.0050", reserved: "$0.00", remaining: "$3.9950", meter: { spent: 0.201, reserved: 0 } });
  });

  it("falls back to the numbers, and has no card without a limit", () => {
    expect(limitCard(person)?.spent).toBe("$1.0050");
    expect(limitCard({ ...person, remainingUsd: null })?.remaining).toBe("None");
    expect(limitCard({ ...person, limitUsd: null, limitUsdDecimal: null })).toBeNull();
  });
});

describe("Spend's Nexus credit", () => {
  const subsidy = { grantedUsd: 0.2, spentUsd: 0.1, reservedUsd: 0, remainingUsd: 0.1, fundedBy: "nexus" as const, usableForModelCalls: true };

  it("reads the twins first", () => {
    const card = creditCard({ ...owner, subsidizedUsd: 0.1, subsidizedUsdDecimal: "0.100001", subsidy: { ...subsidy, grantedUsdDecimal: "0.200000", spentUsdDecimal: "0.100001", reservedUsdDecimal: "0.000002", remainingUsdDecimal: "0.099997" } });
    expect(card).toMatchObject({ granted: "$0.20", spent: "$0.1000", remaining: "$0.1000", subsidized: "$0.1000", reserved: "$0.000002" });
    // The meter shows what is left, as the sidebar does.
    expect(card?.left).toEqual({ spent: 0.499985, reserved: 0 });
  });

  it("falls back to the numbers, and has no card without a subsidy", () => {
    expect(creditCard({ ...ownerNumbers, subsidy })).toMatchObject({ granted: "$0.20", spent: "$0.10", remaining: "$0.10", subsidized: "$0.00", reserved: null });
    expect(creditCard(ownerNumbers)).toBeNull();
  });
});

describe("Settings' budget line", () => {
  it("writes the twins exactly", () => {
    expect(settingsLine(owner)).toBe("$86.75 of $100.00 left");
    const member: SpendResponse = { remainingUsd: 1.005, remainingUsdDecimal: "1.005000", scope: "own", ...calls };
    expect(settingsLine(member)).toBe("$1.0050 left for you");
  });

  it("falls back to the numbers without the twins", () => {
    expect(settingsLine(ownerNumbers)).toBe("$86.75 of $100.00 left");
    expect(settingsLine({ ...ownerNumbers, budgetUsd: 0 })).toBeUndefined();
  });
});

describe("Usage's hero", () => {
  const now = new Date("2026-11-05T12:00:00Z");
  it("names what the spent figure counts by the budget's period", () => {
    expect([spentLabel(undefined, now), spentLabel("month", now), spentLabel("week", now), spentLabel("day", now)]).toEqual(["Spent in November", "Spent in November", "Spent this week", "Spent today"]);
  });
  it("counts the teams from the first of the UTC month", () => {
    expect(thisMonth(now)).toEqual({ from: "2026-11-01", to: "2026-11-05" });
  });
});
