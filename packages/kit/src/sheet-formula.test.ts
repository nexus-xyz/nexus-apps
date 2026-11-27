import { describe, expect, it } from "vitest";
import type { Sheet, SheetCell } from "./sheet";
import { formatCellValue } from "./sheet-display";
import { computeSheet, FUNCTIONS, parseFormula, type Scalar } from "./sheet-formula";

const TODAY = new Date("2026-11-10T15:00:00Z");

/** The value of `formula` in C1 of a tab holding `cells` (plain values, or `=…` formulas). */
function calc(formula: string, cells: Record<string, string | number | boolean> = {}, more: Sheet["tabs"] = []): Scalar {
  const tab: Record<string, SheetCell> = {};
  for (const [address, value] of Object.entries(cells)) tab[address] = typeof value === "string" && value.startsWith("=") ? { formula: value } : { value };
  tab.Z99 = { formula };
  const sheet: Sheet = { tabs: [{ id: "t1", name: "Sheet1", cells: tab }, ...more] };
  return computeSheet(sheet, { today: TODAY }).value("t1", "Z99");
}

const NUMBERS = { A1: 1, A2: 2, A3: 3, A4: "text", A5: true };

describe("formula engine", () => {
  it("does arithmetic with Excel's precedence", () => {
    expect(calc("=1+2*3")).toBe(7);
    expect(calc("=(1+2)*3")).toBe(9);
    expect(calc("=-2^2")).toBe(4);
    expect(calc("=2^3^2")).toBe(64);
    expect(calc("=50%*10")).toBe(5);
    expect(calc("=10/4")).toBe(2.5);
    expect(calc('="a"&1&TRUE')).toBe("a1TRUE");
    expect(calc("=1<2")).toBe(true);
    expect(calc('="B"="b"')).toBe(true);
    expect(calc("=3<>3")).toBe(false);
    expect(calc("=A1+A2", { A1: 1, A2: "2" })).toBe(3);
    expect(calc("=A9+1")).toBe(1);
  });

  it("reads references: absolute, ranges, whole columns and other tabs", () => {
    expect(calc("=$A$1+A$2", NUMBERS)).toBe(3);
    expect(calc("=SUM(A1:A3)", NUMBERS)).toBe(6);
    expect(calc("=SUM(A3:A1)", NUMBERS)).toBe(6);
    expect(calc("=SUM(A:A)", NUMBERS)).toBe(6);
    const other: Sheet["tabs"] = [
      { id: "t2", name: "Q3 plan", cells: { B2: { value: 40 } } },
      { id: "t3", name: "Data", cells: { A1: { value: 2 } } },
    ];
    expect(calc("='Q3 plan'!B2+Data!A1", {}, other)).toBe(42);
    expect(calc("=SUM('q3 PLAN'!A1:C9)", {}, other)).toBe(40);
  });

  it("chains formulas in any order", () => {
    expect(calc("=B3*2", { B1: "=B2+1", B2: 4, B3: "=B1*10" })).toBe(100);
  });

  it("computes a chain of 20,000 rows without recursion", () => {
    const cells: Record<string, SheetCell> = { A1: { value: 1 } };
    for (let row = 2; row <= 20_000; row++) cells[`A${row}`] = { formula: `=A${row - 1}+1` };
    const sheet: Sheet = { tabs: [{ id: "t1", name: "S", cells }] };
    expect(computeSheet(sheet).value("t1", "A20000")).toBe(20_000);
  });

  it("gives a cycle #CYCLE! in every cell of it, and the cells that read it the error", () => {
    const sheet: Sheet = { tabs: [{ id: "t1", name: "S", cells: { A1: { formula: "=B1" }, B1: { formula: "=A1+1" }, C1: { formula: "=C1" }, D1: { formula: "=A1*2" }, E1: { formula: "=SUM(E2:E3)" }, E2: { formula: "=E1" } } }] };
    const computed = computeSheet(sheet);
    for (const address of ["A1", "B1", "C1", "D1", "E1", "E2"]) expect(computed.value("t1", address)).toMatchObject({ error: "#CYCLE!" });
  });

  it("gives errors as values, not exceptions", () => {
    expect(calc("=1/0")).toMatchObject({ error: "#DIV/0!" });
    expect(calc("=Nowhere!A1")).toMatchObject({ error: "#REF!" });
    expect(calc("=ABC1")).toMatchObject({ error: "#REF!" });
    expect(calc("=NOPE(1)")).toMatchObject({ error: "#NAME?" });
    expect(calc("=revenue")).toMatchObject({ error: "#NAME?" });
    expect(calc('=1+"a"')).toMatchObject({ error: "#VALUE!" });
    expect(calc("=SUM(1,")).toMatchObject({ error: "#ERROR!" });
    expect(calc('="open')).toMatchObject({ error: "#ERROR!" });
    expect(calc("=1+")).toMatchObject({ error: "#ERROR!" });
    expect(calc("=(1")).toMatchObject({ error: "#ERROR!" });
    expect(calc("=ROUND()")).toMatchObject({ error: "#VALUE!" });
    expect(calc("=A1:A3", NUMBERS)).toMatchObject({ error: "#VALUE!" });
    expect(calc("=(-8)^0.5")).toMatchObject({ error: "#NUM!" });
    expect(calc("=10^400")).toMatchObject({ error: "#NUM!" });
    expect(calc("=A1+1", { A1: "=1/0" })).toMatchObject({ error: "#DIV/0!" });
    expect(parseFormula("=1 2")).toMatchObject({ error: "#ERROR!" });
  });

  it("SUM, AVERAGE, MIN and MAX skip text in ranges and convert what is written", () => {
    expect(calc("=SUM(A1:A5,10)", NUMBERS)).toBe(16);
    expect(calc('=SUM("2",TRUE)')).toBe(3);
    expect(calc('=SUM("x")')).toMatchObject({ error: "#VALUE!" });
    expect(calc("=SUM(A1, A4)", NUMBERS)).toBe(1);
    expect(calc("=AVERAGE(A1:A5)", NUMBERS)).toBe(2);
    expect(calc("=AVERAGE(B1:B3)")).toMatchObject({ error: "#DIV/0!" });
    expect(calc("=MIN(A1:A5,0.5)", NUMBERS)).toBe(0.5);
    expect(calc("=MAX(A1:A5)", NUMBERS)).toBe(3);
    expect(calc("=MAX(B1:B3)")).toBe(0);
  });

  it("COUNT counts numbers and COUNTA anything", () => {
    expect(calc("=COUNT(A1:A6)", NUMBERS)).toBe(3);
    expect(calc('=COUNT(1,"2","x")')).toBe(2);
    expect(calc("=COUNTA(A1:A6)", NUMBERS)).toBe(5);
  });

  it("IF, AND, OR and NOT", () => {
    expect(calc('=IF(A1>2,"big","small")', { A1: 3 })).toBe("big");
    expect(calc('=IF(A1>2,"big","small")', { A1: 1 })).toBe("small");
    expect(calc("=IF(FALSE,1)")).toBe(false);
    expect(calc("=IF(TRUE,1,1/0)")).toBe(1);
    expect(calc("=AND(TRUE,1,A1:A2)", { A1: true, A2: "x" })).toBe(true);
    expect(calc("=AND(TRUE,0)")).toBe(false);
    expect(calc("=OR(FALSE,A1)", { A1: 1 })).toBe(true);
    expect(calc("=OR(B1:B2)")).toMatchObject({ error: "#VALUE!" });
    expect(calc("=NOT(1)")).toBe(false);
  });

  it("ROUND rounds half away from zero, also to tens", () => {
    expect(calc("=ROUND(2.5)")).toBe(3);
    expect(calc("=ROUND(-2.5,0)")).toBe(-3);
    expect(calc("=ROUND(1.005,2)")).toBe(1.01);
    expect(calc("=ROUND(1234,-2)")).toBe(1200);
    expect(calc("=ABS(-4)")).toBe(4);
  });

  it("CONCAT, LEN, UPPER, LOWER and TRIM", () => {
    expect(calc('=CONCAT(A1:A3,"!",0.1+0.2)', NUMBERS)).toBe("123!0.3");
    expect(calc('=LEN("héllo")')).toBe(5);
    expect(calc('=UPPER("ab")&LOWER("CD")')).toBe("ABcd");
    expect(calc('=TRIM("  a   b ")')).toBe("a b");
  });

  it("VLOOKUP finds exact and sorted matches", () => {
    const table = { A1: 1, B1: "one", A2: 5, B2: "five", A3: 10, B3: "ten", D1: "Ten" };
    expect(calc("=VLOOKUP(5,A1:B3,2,FALSE)", table)).toBe("five");
    expect(calc('=VLOOKUP("ten",B1:B3,1,FALSE)', table)).toBe("ten");
    expect(calc("=VLOOKUP(7,A1:B3,2)", table)).toBe("five");
    expect(calc("=VLOOKUP(99,A1:B3,2,TRUE)", table)).toBe("ten");
    expect(calc("=VLOOKUP(0,A1:B3,2)", table)).toMatchObject({ error: "#N/A" });
    expect(calc("=VLOOKUP(4,A1:B3,2,FALSE)", table)).toMatchObject({ error: "#N/A" });
    expect(calc("=VLOOKUP(5,A1:B3,3,FALSE)", table)).toMatchObject({ error: "#REF!" });
  });

  it("SUMIF and COUNTIF with criteria and wildcards", () => {
    const rows = { A1: "apple", B1: 3, A2: "banana", B2: 5, A3: "apricot", B3: 7, A4: "", B4: 1 };
    expect(calc('=SUMIF(A1:A4,"ap*",B1:B4)', rows)).toBe(10);
    expect(calc('=SUMIF(B1:B4,">4")', rows)).toBe(12);
    expect(calc('=SUMIF(A1:A4,"banana",B1:B4)', rows)).toBe(5);
    expect(calc('=COUNTIF(B1:B4,"<=5")', rows)).toBe(3);
    expect(calc('=COUNTIF(A1:A4,"<>banana")', rows)).toBe(3);
    expect(calc('=COUNTIF(A1:A4,"?pple")', rows)).toBe(1);
    expect(calc("=COUNTIF(B1:B4,5)", rows)).toBe(1);
    expect(calc('=COUNTIF(C1:C3,"")')).toBe(3);
  });

  it("IFERROR and TODAY", () => {
    expect(calc('=IFERROR(1/0,"none")')).toBe("none");
    expect(calc("=IFERROR(2,0)")).toBe(2);
    expect(calc("=TODAY()")).toBe(46336);
    expect(formatCellValue(calc("=TODAY()"), { number: "date" })).toBe("2026-11-10");
  });

  it("has a test above for every function it states", () => {
    expect(Object.keys(FUNCTIONS).sort()).toEqual(
      ["ABS", "AND", "AVERAGE", "CONCAT", "COUNT", "COUNTA", "COUNTIF", "IF", "IFERROR", "LEN", "LOWER", "MAX", "MIN", "NOT", "OR", "ROUND", "SUM", "SUMIF", "TODAY", "TRIM", "UPPER", "VLOOKUP"].sort(),
    );
  });

  it("shows values under their formats", () => {
    expect(formatCellValue(1234.5, { number: "number" })).toBe("1,234.50");
    expect(formatCellValue(0.256, { number: "percent", decimals: 1 })).toBe("25.6%");
    expect(formatCellValue(9.5, { number: "currency" })).toBe("$9.50");
    expect(formatCellValue(9.5, { number: "currency", currency: "EUR", decimals: 0 })).toBe("€10");
    expect(formatCellValue(0.1 + 0.2)).toBe("0.3");
    expect(formatCellValue(0.032979976443)).toBe("0.032979976");
    expect(formatCellValue(-1234567.891234)).toBe("-1234567.89");
    expect(formatCellValue(true)).toBe("TRUE");
    expect(formatCellValue({ error: "#REF!" })).toBe("#REF!");
    expect(formatCellValue(null)).toBe("");
  });
});
