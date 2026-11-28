import { describe, expect, it } from "vitest";
import { amount, amountOrNull, budgetText, centsText, dayGroupKeys, dayGroupOf, microsOfNumber, parseMicros, personAmounts, personDefaultOf, reaches, share, spentOf, sumMicros, usdText } from "./money";

describe("parseMicros", () => {
  it("reads a six-decimal string into integer micro-dollars", () => {
    expect(parseMicros("12.245000")).toBe(12_245_000);
    expect(parseMicros("0.000001")).toBe(1);
    expect(parseMicros("1234")).toBe(1_234_000_000);
    expect(parseMicros("-0.500000")).toBe(-500_000);
    expect(parseMicros("-0.000000")).toBe(0);
  });
  it("refuses what is not a decimal of six places or fewer", () => {
    for (const bad of ["", "abc", "1.2345678", "1e3", "$1.00", "1,000.00", "99999999999.000000"]) expect(parseMicros(bad)).toBeNull();
  });
});

describe("a field's amount", () => {
  it("reads the twin first", () => {
    expect(amount("12.245000", 12.24)).toBe(12_245_000);
  });
  it("falls back to the number when the twin is missing, null or not a decimal", () => {
    expect(amount(undefined, 12.5)).toBe(12_500_000);
    expect(amount(null, 0)).toBe(0);
    expect(amount("garbage", 1)).toBe(1_000_000);
    expect(amount(undefined, undefined)).toBe(0);
    expect(amountOrNull(undefined, null)).toBeNull();
    expect(amountOrNull(null, null)).toBeNull();
    expect(microsOfNumber(Number.NaN)).toBe(0);
  });
});

describe("the formatters, exactly", () => {
  it("rounds 12.245000 to $12.25 under Usage's whole-cent rule, and a half cent up where the float rounds down", () => {
    expect(centsText(parseMicros("12.245000")!)).toBe("$12.25");
    // The float 1.005 times 100 is 100.4999…: the old path shows a cent less.
    expect(centsText(parseMicros("1.005000")!)).toBe("$1.01");
    expect((1.005).toFixed(2)).toBe("1.00");
  });

  it("keeps usd()'s rule: cents from $100 or for whole cents, four decimals from a cent, else six", () => {
    expect(usdText(parseMicros("12.245000")!)).toBe("$12.2450");
    expect(usdText(parseMicros("100.005000")!)).toBe("$100.01");
    // The float 10.00015 sits just under its half: toFixed(4) rounds it down.
    expect(usdText(parseMicros("10.000150")!)).toBe("$10.0002");
    expect((10.00015).toFixed(4)).toBe("10.0001");
    expect(usdText(parseMicros("18342.170000")!)).toBe("$18,342.17");
    expect(usdText(parseMicros("12.500000")!)).toBe("$12.50");
    expect(usdText(parseMicros("0.012345")!)).toBe("$0.0123");
    expect(usdText(parseMicros("0.001000")!)).toBe("$0.001000");
    expect(usdText(0)).toBe("$0.00");
    expect(usdText(-1_500_000)).toBe("$-1.50");
  });

  it("reads a dollar number to the nearest micro-dollar", () => {
    expect(usdText(microsOfNumber(12.244999999))).toBe("$12.2450");
    expect(centsText(microsOfNumber(0.1 + 0.2))).toBe("$0.30");
  });

  it("signs a change and writes a budget without its cents when it is whole dollars", () => {
    expect(centsText(3_120_000, true)).toBe("+$3.12");
    expect(centsText(-1_000_000, true)).toBe("−$1.00");
    expect(centsText(-1_000_000)).toBe("−$1.00");
    expect(centsText(4_000, true)).toBe("$0.00");
    expect(budgetText(100_000_000)).toBe("$100");
    expect(budgetText(25_000_000_000)).toBe("$25,000");
    expect(budgetText(84_600_000)).toBe("$84.60");
    expect(budgetText(99_999_000)).toBe("$100");
  });
});

describe("sums and shares", () => {
  it("adds many small amounts exactly, where floats drift", () => {
    const tenth = Array.from({ length: 10_000 }, () => "0.100000");
    expect(sumMicros(tenth.map((value) => parseMicros(value)!))).toBe(1_000_000_000);
    expect(tenth.reduce((sum, value) => sum + Number(value), 0)).not.toBe(1000);
    const calls = Array.from({ length: 3 }, () => "0.000001");
    expect(usdText(sumMicros(calls.map((value) => parseMicros(value)!)))).toBe("$0.000003");
    const dimes = [0.1, 0.2];
    expect(dimes.reduce((sum, value) => sum + value, 0)).not.toBe(0.3);
    expect(sumMicros(dimes.map(microsOfNumber))).toBe(300_000);
  });

  it("compares a share exactly at its boundary", () => {
    // 4 of 5 dollars is 80%, exactly.
    expect(reaches(4_000_000, 5_000_000, 4, 5)).toBe(true);
    expect(reaches(3_999_999, 5_000_000, 4, 5)).toBe(false);
    expect(share(1, 4)).toBe(0.25);
    expect(share(1, 0)).toBe(0);
  });
});

describe("the reads' fields", () => {
  it("reads a usage figure and a day's group from their twins first", () => {
    expect(spentOf({ spentUsd: 1, spentUsdDecimal: "2.000000" })).toBe(2_000_000);
    expect(spentOf({ spentUsd: 1 })).toBe(1_000_000);
    // Rule 9's empty day: a null twin, so the number.
    expect(spentOf({ spentUsd: 0, spentUsdDecimal: null })).toBe(0);
    const day = { groups: { a: 1, b: 2 }, groupsDecimal: { a: "1.500000" } };
    expect(dayGroupOf(day, "a")).toBe(1_500_000);
    expect(dayGroupOf(day, "b")).toBe(2_000_000);
    expect(dayGroupOf(day, "c")).toBe(0);
    expect(dayGroupKeys(day)).toEqual(["a", "b"]);
  });

  it("reads a person's budget and the default from their twins first", () => {
    const twins = personAmounts({
      limitUsd: 1,
      limitUsdDecimal: "5.000000",
      overrideUsd: null,
      overrideUsdDecimal: null,
      spentUsd: 1,
      spentUsdDecimal: "4.310000",
      reservedUsd: 0,
      reservedUsdDecimal: "0.010000",
      remainingUsd: 1,
      remainingUsdDecimal: "0.680000",
    });
    expect(twins).toEqual({ limit: 5_000_000, override: null, spent: 4_310_000, reserved: 10_000, remaining: 680_000 });
    expect(personAmounts({ limitUsd: null, overrideUsd: null, spentUsd: 2, reservedUsd: 0, remainingUsd: null })).toEqual({ limit: null, override: null, spent: 2_000_000, reserved: 0, remaining: null });
    expect(personDefaultOf({ personDefault: { monthlyUsd: 1, monthlyUsdDecimal: "25.000000" } })).toBe(25_000_000);
    expect(personDefaultOf({ personDefault: { monthlyUsd: 25 } })).toBe(25_000_000);
    expect(personDefaultOf({ personDefault: { monthlyUsd: null, monthlyUsdDecimal: null } })).toBeNull();
  });
});
