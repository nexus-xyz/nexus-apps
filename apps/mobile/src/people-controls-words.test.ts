import type { TeamBudget } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { checkedLine, insideLine, keyEnd, keyState, memberLine, teamBudgetLine, teamLine, typedAmount } from "./people-controls-words";

const budget = (b: Partial<NonNullable<TeamBudget["budget"]>>): TeamBudget["budget"] => ({
  remainingUsd: 0,
  period: "month",
  resetsAt: "2026-12-01T00:00:00Z",
  ...b,
});

describe("people and teams", () => {
  it("puts the role first, and the email when a name stands above it", () => {
    expect(memberLine({ role: "owner", name: "Maya Chen", email: "maya@acme.example" })).toBe("Owner · maya@acme.example");
    expect(memberLine({ role: "member", email: "ana@acme.example" })).toBe("Member");
  });

  it("says a team's headcount, its leads and whether it is your default", () => {
    expect(teamLine({ headcount: 14, leads: [{ name: "Dev Lal" }], isDefault: true })).toBe("14 people · Dev Lal · your default");
    expect(teamLine({ headcount: 1, leads: [] })).toBe("1 person");
  });

  it("writes where a team sits from the organization down", () => {
    expect(insideLine("Acme", [{ name: "R&D" }])).toBe("Acme › R&D");
    expect(insideLine("Acme", [])).toBe("Acme");
  });
});

describe("team budgets", () => {
  it("says what a team spent of its cap, and the period when it is not the month", () => {
    expect(teamBudgetLine(budget({ limitUsdDecimal: "30000", spentUsdDecimal: "18600", remainingUsdDecimal: "11400" }))).toEqual({
      cap: "$30,000.00",
      line: "$18,600.00 of $30,000.00",
      atLimit: false,
    });
    expect(teamBudgetLine(budget({ period: "week", limitUsd: 800, spentUsd: 410, remainingUsd: 390 }))?.line).toBe("$410.00 of $800.00 this week");
  });

  it("says when a team at its limit can call again", () => {
    expect(teamBudgetLine(budget({ limitUsd: 10000, spentUsd: 10000, remainingUsd: 0 }))?.line).toBe("At its limit · new calls stop until 1 Dec");
  });

  it("has no line without a cap the caller may see", () => {
    expect(teamBudgetLine(null)).toBeNull();
    expect(teamBudgetLine(budget({ remainingUsd: 20 }))).toBeNull();
  });

  it("reads a typed amount exactly, and refuses anything else", () => {
    expect(typedAmount("$15,000.50")).toBe("15000.50");
    expect(typedAmount(" 800 ")).toBe("800");
    expect(typedAmount("0")).toBeNull();
    expect(typedAmount("12.1234567")).toBeNull();
    expect(typedAmount("ten")).toBeNull();
  });
});

describe("provider keys", () => {
  it("masks a key to its end", () => {
    expect(keyEnd("4F2A")).toBe("····4F2A");
  });

  it("says nothing of a key that works, and why one does not", () => {
    expect(keyState({ state: "valid" })).toBeUndefined();
    expect(keyState({ state: "rejected" })).toEqual({ text: "Rejected", danger: true });
    expect(keyState({ unused: true })).toEqual({ text: "Unused", danger: false });
    expect(checkedLine({ state: "valid", checkedAt: "2026-11-21T10:00:00Z" })).toBe("Accepted · 21 Nov");
    expect(checkedLine({})).toBeUndefined();
  });
});
