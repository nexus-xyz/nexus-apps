import { describe, expect, it } from "vitest";
import type { MemberSummary } from "./api";
import { askedLine, blockOf, blockOfReason, ownersLine, ownersOf, shortName } from "./ask-owner";

const member = (userId: string, role: MemberSummary["role"], name?: string, email?: string): MemberSummary => ({ userId, role, name, email, joinedAt: "2026-11-01T00:00:00.000Z" });

describe("asking an owner", () => {
  it("knows the blocks only an owner fixes, and no other refusal", () => {
    expect(blockOf("usage_limit_reached")).toBe("usage_limit");
    expect(blockOf("credit_exhausted", { kind: "payg", credits: true, afterTheFact: false })).toBe("credits_out");
    expect(blockOf("subsidy_exhausted", { kind: "unknown" })).toBe("credits_out");
    expect(blockOf("credit_exhausted", { kind: "free" })).toBe("free_start_used");
    expect(blockOf("payment_failed")).toBe("payment_failed");
    expect(blockOf("payment_past_due")).toBe("past_due");
    expect(blockOf("charge_processing")).toBe("billing");
    expect(blockOf("budget_exceeded")).toBeUndefined();
    expect(blockOf(undefined)).toBeUndefined();
  });

  it("asks about the block a composer pay notice names", () => {
    expect(blockOfReason("usage_limit_reached")).toBe("usage_limit");
    expect(blockOfReason("credits_out")).toBe("credits_out");
    expect(blockOfReason("free_start_used_up")).toBe("free_start_used");
    expect(blockOfReason("free_start_expired")).toBe("free_start_used");
    expect(blockOfReason("charge_failed")).toBe("payment_failed");
    expect(blockOfReason("card_needs_confirming")).toBe("payment_failed");
    expect(blockOfReason("card_disputed")).toBe("payment_failed");
    expect(blockOfReason("past_due")).toBe("past_due");
    expect(blockOfReason("billing_off")).toBe("billing");
    expect(blockOfReason("charge_processing")).toBe("billing");
  });

  it("names a person by first name and last initial, never by a full email", () => {
    expect(shortName({ name: "Maya Costa" })).toBe("Maya C.");
    expect(shortName({ name: "  Ana  María  souza " })).toBe("Ana S.");
    expect(shortName({ name: "Prince" })).toBe("Prince");
    expect(shortName({ email: "bruno@acme.com" })).toBe("bruno");
    expect(shortName({})).toBe("An owner");
  });

  it("lists the owners but not the person asking, and says them in one line", () => {
    const members = [member("u1", "owner", "Maya Costa"), member("u2", "admin", "Bo Li"), member("u3", "owner", "Ana Souza"), member("me", "owner", "Me Too")];
    expect(ownersOf(members, "me")).toEqual(["Ana S.", "Maya C."]);
    expect(ownersLine([])).toBe("");
    expect(ownersLine(["Maya C."])).toBe("Owner: Maya C.");
    expect(ownersLine(["Ana S.", "Maya C."])).toBe("Owners: Ana S. and Maya C.");
    expect(ownersLine(["A.", "B.", "C.", "D."])).toBe("Owners: A., B. and 2 more");
  });

  it("says what a question did", () => {
    expect(askedLine("sent")).toMatch(/^Asked\./);
    expect(askedLine("already_asked")).toMatch(/^You asked today\./);
  });
});
