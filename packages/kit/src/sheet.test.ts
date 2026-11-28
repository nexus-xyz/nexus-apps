import { describe, expect, it } from "vitest";
import cases from "../fixtures/sheet-cases.json";
import { columnIndex, columnName, parseAddress, parseRange, parseSheet, serializeSheet, sheetProblem, type Sheet } from "./sheet";

describe("sheet schema", () => {
  it.each(cases.valid)("accepts $name", ({ sheet }) => {
    expect(sheetProblem(sheet)).toBeUndefined();
  });

  it.each(cases.invalid)("refuses $name at its path", ({ sheet, path }) => {
    expect(sheetProblem(sheet)?.path).toBe(path);
  });

  it("refuses text that is not JSON, and reads a sheet that is", () => {
    expect(parseSheet("{tabs:")).toMatchObject({ problem: { path: "" } });
    expect(parseSheet('{"tabs":[{"id":"t1","name":"A"}]}')).toEqual({ sheet: { tabs: [{ id: "t1", name: "A" }] } });
  });

  it("serializes canonically: sorted keys, empty objects left out, lists in order", () => {
    const a: Sheet = { tabs: [{ name: "B", id: "t2", cells: { B2: { value: 2 }, A1: { format: {}, value: "x" }, C3: {} } }, { id: "t1", name: "A", frozen: {} }] };
    const b: Sheet = { tabs: [{ id: "t2", cells: { A1: { value: "x" }, B2: { value: 2 } }, name: "B" }, { name: "A", id: "t1" }] };
    expect(serializeSheet(a)).toBe(serializeSheet(b));
    expect(serializeSheet(a)).toBe('{"tabs":[{"cells":{"A1":{"value":"x"},"B2":{"value":2}},"id":"t2","name":"B"},{"id":"t1","name":"A"}]}');
  });

  it("names columns and reads addresses and ranges", () => {
    expect([columnName(1), columnName(26), columnName(27), columnName(702)]).toEqual(["A", "Z", "AA", "ZZ"]);
    expect([columnIndex("A"), columnIndex("AA"), columnIndex("ZZ")]).toEqual([1, 27, 702]);
    expect(parseAddress("B7")).toEqual({ column: 2, row: 7 });
    expect([parseAddress("A0"), parseAddress("a1"), parseAddress("AAA1")]).toEqual([undefined, undefined, undefined]);
    expect(parseRange("C9:A2")).toEqual({ top: 2, left: 1, bottom: 9, right: 3 });
  });
});
