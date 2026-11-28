import { describe, expect, it } from "vitest";
import { chartDays, rangeOf, reportFigures, seriesOf, sourceLabel, spentText, stackOf } from "./analytics";

describe("analytics", () => {
  it("takes the last N UTC days, today included", () => {
    expect(rangeOf(7, new Date("2026-10-22T12:00:00Z"))).toEqual({ from: "2026-10-16", to: "2026-10-22" });
  });

  it("names products as people read them", () => {
    expect(sourceLabel("chat/chat")).toBe("Chat › Chat");
    expect(sourceLabel("core")).toBe("Gateway");
  });

  it("folds the groups past the colours into Other, unless only one is left", () => {
    const groups = ["a", "b", "c", "d"].map((key) => ({ key, label: key.toUpperCase() }));
    const two = seriesOf(groups.slice(0, 3), ["x", "y"], "o");
    expect(two.series.map((s) => s.label)).toEqual(["A", "B", "C"]);
    const many = seriesOf(groups, ["x", "y"], "o");
    expect(many.series.map((s) => s.label)).toEqual(["A", "B", "Other (2)"]);
    expect(stackOf({ groups: { a: 1, c: 2, d: 3 } }, many.series, many.restKeys).map((p) => [p.key, p.usd])).toEqual([
      ["a", 1],
      ["__other", 5],
    ]);
  });

  it("stacks a day's groups from their twins first, adding Other in micro-dollars", () => {
    const many = seriesOf(["a", "b", "c", "d"].map((key) => ({ key, label: key })), ["x", "y"], "o");
    const day = { groups: { a: 9, c: 0.1, d: 0.2 }, groupsDecimal: { a: "1.500000", c: "0.100000", d: "0.200000" } };
    // 0.1 + 0.2 is 0.30000000000000004 in floats; the twins add to 0.3.
    expect(stackOf(day, many.series, many.restKeys).map((p) => [p.key, p.usd])).toEqual([
      ["a", 1.5],
      ["__other", 0.3],
    ]);
  });
});

describe("the Analytics screen's figures", () => {
  const totals = { spentUsd: 12.244999, spentUsdDecimal: "12.245000", calls: 3 };

  it("shows the twins exactly", () => {
    expect(reportFigures(totals)).toEqual({ spent: "$12.2450", average: "$4.0817" });
    expect(spentText({ spentUsd: 10.0001, spentUsdDecimal: "10.000150" })).toBe("$10.0002");
  });

  it("falls back to the numbers without the twins, and to the number when the twin is null", () => {
    expect(reportFigures({ spentUsd: 12.245, calls: 3 })).toEqual({ spent: "$12.2450", average: "$4.0817" });
    expect(reportFigures({ spentUsd: 0, spentUsdDecimal: null, calls: 0 })).toEqual({ spent: "$0.00", average: "—" });
    expect(spentText({ spentUsd: 1.005 })).toBe("$1.0050");
  });

  it("draws the columns from the exact spend and labels each day exactly", () => {
    const chart = chartDays([
      { day: "2026-10-01", spentUsd: 1, spentUsdDecimal: "2.000000", calls: 1 },
      { day: "2026-10-02", spentUsd: 1, calls: 1 },
      { day: "2026-10-03", spentUsd: 0, spentUsdDecimal: null, calls: 0 },
    ]);
    expect(chart.max).toBe("$2.00");
    expect(chart.days.map((d) => [d.spent, d.height, d.any])).toEqual([
      ["$2.00", 1, true],
      ["$1.00", 0.5, true],
      ["$0.00", 0, false],
    ]);
    expect(chartDays([]).max).toBe("$0.00");
  });
});
