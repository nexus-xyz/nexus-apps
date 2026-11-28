/**
 * Nexus Sheets: the content of a `sheet` page. One JSON document, its tabs in order, and for each tab its
 * cells by address. This file is the schema. `nexus-types` mirrors it (`api/sheets.rs`), and Gateway refuses a version
 * that breaks it (`page_sheet_invalid`). `kit/fixtures/sheet-cases.json` holds the cases both sides test.
 *
 * - **Canonical.** `serializeSheet` sorts every object's keys and drops what is empty, so equal sheets give equal text
 *   and equal SHA-256. Gateway checks a sheet; it does not rewrite one.
 * - **Formulas.** A cell holds a `value` or a `formula` (`=SUM(A1:A3)`), never both. `sheet-formula.ts` computes them,
 *   with no DOM, for the pages host's render and for the editor.
 */
export { computeSheet, FUNCTIONS, isError, numberText, parseFormula, type ComputedSheet, type ErrorValue, type Scalar, type SheetErrorCode } from "./sheet-formula";
export { alignsRight, formatCellValue, inkOn, SHEET_FILLS } from "./sheet-display";
export { inputOf, offsetFormula, parseInput, rewriteReferences, type ReferenceEnd } from "./sheet-edit";

/** A sheet: its tabs, in order. */
export interface Sheet {
  tabs: SheetTab[];
}

export interface SheetTab {
  /** Stable across renames: `[A-Za-z0-9_-]`, 1 to 32 characters, unique in the sheet. */
  id: string;
  /** What people see and formulas name (`'Q3 plan'!A1`). Unique ignoring case. */
  name: string;
  /** By address, `A1` to `ZZ100000`, upper case, no `$`. */
  cells?: Record<string, SheetCell>;
  /** Column widths in pixels, by column letters. */
  columns?: Record<string, number>;
  /** Rows and columns that stay in view as the reader scrolls. */
  frozen?: { rows?: number; columns?: number };
  /** Cells whose value is picked from a list. */
  dropdowns?: SheetDropdown[];
}

export interface SheetCell {
  value?: string | number | boolean;
  /** Starts with `=`. */
  formula?: string;
  format?: CellFormat;
}

export interface CellFormat {
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  /** `#rrggbb`. */
  color?: string;
  /** `#rrggbb`. */
  background?: string;
  align?: "left" | "center" | "right";
  wrap?: boolean;
  number?: NumberFormat;
  /** Digits after the point, 0 to 10. */
  decimals?: number;
  /** ISO 4217, for `number: "currency"`. Unset: USD. */
  currency?: string;
}

export type NumberFormat = "general" | "number" | "percent" | "currency" | "date" | "text";

export interface SheetDropdown {
  /** `C2:C40` or `C2`. */
  range: string;
  options: string[];
}

export const SHEET_LIMITS = {
  tabs: 50,
  cells: 200_000,
  rows: 100_000,
  /** `ZZ`. */
  columns: 702,
  formulaChars: 8_192,
  textChars: 50_000,
  nameChars: 100,
  dropdowns: 100,
  options: 500,
  optionChars: 255,
  frozenRows: 100,
  frozenColumns: 26,
  minWidth: 16,
  maxWidth: 2_000,
} as const;

export const NUMBER_FORMATS: readonly NumberFormat[] = ["general", "number", "percent", "currency", "date", "text"];

// ---------------------------------------------------------------- addresses

const ADDRESS = /^([A-Z]{1,2})([1-9][0-9]{0,5})$/;
const COLUMN = /^[A-Z]{1,2}$/;

/** `A` is 1, `Z` 26, `AA` 27. */
export function columnIndex(letters: string): number {
  let index = 0;
  for (const char of letters) index = index * 26 + (char.charCodeAt(0) - 64);
  return index;
}

/** 1 is `A`, 27 `AA`. */
export function columnName(index: number): string {
  let name = "";
  for (let at = index; at > 0; at = Math.floor((at - 1) / 26)) name = String.fromCharCode(65 + ((at - 1) % 26)) + name;
  return name;
}

/** `B7` as column 2, row 7; undefined when it is no address in bounds. */
export function parseAddress(address: string): { column: number; row: number } | undefined {
  const match = ADDRESS.exec(address);
  if (!match) return undefined;
  const column = columnIndex(match[1]!);
  const row = Number(match[2]);
  return column <= SHEET_LIMITS.columns && row <= SHEET_LIMITS.rows ? { column, row } : undefined;
}

export const cellAddress = (column: number, row: number) => `${columnName(column)}${row}`;

/** `C2:C40` (either corner first) or `C2`, as its corners, top left first. */
export function parseRange(range: string): { top: number; left: number; bottom: number; right: number } | undefined {
  const [from = "", to = from, extra] = range.split(":");
  if (extra !== undefined) return undefined;
  const a = parseAddress(from);
  const b = parseAddress(to);
  if (!a || !b) return undefined;
  return { top: Math.min(a.row, b.row), left: Math.min(a.column, b.column), bottom: Math.max(a.row, b.row), right: Math.max(a.column, b.column) };
}

// ---------------------------------------------------------------- checking

/** A problem with a sheet: where (`tabs[0].cells.A1.formula`) and why. */
export interface SheetProblem {
  path: string;
  message: string;
}

const TAB_ID = /^[A-Za-z0-9_-]{1,32}$/;
const COLOR = /^#[0-9A-Fa-f]{6}$/;
const CURRENCY = /^[A-Z]{3}$/;
const TAB_NAME_FORBIDDEN = /[\\/?*[\]:]/;
const chars = (text: string) => [...text].length;
const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const isWhole = (value: unknown, min: number, max: number): value is number => typeof value === "number" && Number.isInteger(value) && value >= min && value <= max;

class Problem extends Error {
  constructor(
    readonly path: string,
    message: string,
  ) {
    super(message);
  }
}

function only(value: Record<string, unknown>, keys: readonly string[], path: string): void {
  for (const key of Object.keys(value)) if (!keys.includes(key)) throw new Problem(path, `unknown field ${key}`);
}

function checkFormat(format: unknown, path: string): void {
  if (!isObject(format)) throw new Problem(path, "a format is an object");
  only(format, ["bold", "italic", "underline", "color", "background", "align", "wrap", "number", "decimals", "currency"], path);
  for (const key of ["bold", "italic", "underline", "wrap"] as const) {
    if (format[key] !== undefined && typeof format[key] !== "boolean") throw new Problem(`${path}.${key}`, `${key} is true or false`);
  }
  for (const key of ["color", "background"] as const) {
    if (format[key] !== undefined && !(typeof format[key] === "string" && COLOR.test(format[key]))) throw new Problem(`${path}.${key}`, `${key} is a colour as #rrggbb`);
  }
  if (format.align !== undefined && !["left", "center", "right"].includes(format.align as string)) throw new Problem(`${path}.align`, "align is left, center or right");
  if (format.number !== undefined && !NUMBER_FORMATS.includes(format.number as NumberFormat)) throw new Problem(`${path}.number`, `number is one of ${NUMBER_FORMATS.join(", ")}`);
  if (format.decimals !== undefined && !isWhole(format.decimals, 0, 10)) throw new Problem(`${path}.decimals`, "decimals is a whole number from 0 to 10");
  if (format.currency !== undefined && !(typeof format.currency === "string" && CURRENCY.test(format.currency))) throw new Problem(`${path}.currency`, "currency is an ISO 4217 code, such as USD");
}

function checkCell(cell: unknown, path: string): void {
  if (!isObject(cell)) throw new Problem(path, "a cell is an object");
  only(cell, ["value", "formula", "format"], path);
  if (cell.value !== undefined && cell.formula !== undefined) throw new Problem(path, "a cell has a value or a formula, not both");
  const value = cell.value;
  if (value !== undefined) {
    if (typeof value === "number") {
      if (!Number.isFinite(value)) throw new Problem(`${path}.value`, "a number is finite");
    } else if (typeof value === "string") {
      if (chars(value) > SHEET_LIMITS.textChars) throw new Problem(`${path}.value`, `text is at most ${SHEET_LIMITS.textChars} characters`);
    } else if (typeof value !== "boolean") {
      throw new Problem(`${path}.value`, "a value is text, a number, or true or false");
    }
  }
  const formula = cell.formula;
  if (formula !== undefined) {
    if (typeof formula !== "string" || !formula.startsWith("=") || formula.length < 2) throw new Problem(`${path}.formula`, "a formula is text that starts with =");
    if (chars(formula) > SHEET_LIMITS.formulaChars) throw new Problem(`${path}.formula`, `a formula is at most ${SHEET_LIMITS.formulaChars} characters`);
  }
  if (cell.format !== undefined) checkFormat(cell.format, `${path}.format`);
}

function checkTab(tab: unknown, path: string, seen: { ids: Set<string>; names: Set<string>; cells: number }): void {
  if (!isObject(tab)) throw new Problem(path, "a tab is an object");
  only(tab, ["id", "name", "cells", "columns", "frozen", "dropdowns"], path);
  if (typeof tab.id !== "string" || !TAB_ID.test(tab.id)) throw new Problem(`${path}.id`, "a tab's id is 1 to 32 letters, digits, _ or -");
  if (seen.ids.has(tab.id)) throw new Problem(`${path}.id`, `two tabs have the id ${tab.id}`);
  seen.ids.add(tab.id);
  const name = tab.name;
  if (typeof name !== "string" || name.trim() === "" || chars(name) > SHEET_LIMITS.nameChars) throw new Problem(`${path}.name`, `a tab's name is 1 to ${SHEET_LIMITS.nameChars} characters`);
  if (TAB_NAME_FORBIDDEN.test(name) || name.startsWith("'") || name.endsWith("'")) throw new Problem(`${path}.name`, "a tab's name has none of \\ / ? * [ ] : and does not start or end with '");
  if (seen.names.has(name.toLowerCase())) throw new Problem(`${path}.name`, `two tabs are named ${name}`);
  seen.names.add(name.toLowerCase());
  if (tab.cells !== undefined) {
    if (!isObject(tab.cells)) throw new Problem(`${path}.cells`, "cells is an object of cells by address");
    for (const [address, cell] of Object.entries(tab.cells)) {
      if (!parseAddress(address)) throw new Problem(`${path}.cells.${address}`, `${address} is not a cell address from A1 to ZZ${SHEET_LIMITS.rows}`);
      checkCell(cell, `${path}.cells.${address}`);
      if (++seen.cells > SHEET_LIMITS.cells) throw new Problem(`${path}.cells`, `a sheet has at most ${SHEET_LIMITS.cells} cells`);
    }
  }
  if (tab.columns !== undefined) {
    if (!isObject(tab.columns)) throw new Problem(`${path}.columns`, "columns is an object of widths by column");
    for (const [column, width] of Object.entries(tab.columns)) {
      if (!COLUMN.test(column)) throw new Problem(`${path}.columns.${column}`, `${column} is not a column from A to ZZ`);
      if (!isWhole(width, SHEET_LIMITS.minWidth, SHEET_LIMITS.maxWidth)) throw new Problem(`${path}.columns.${column}`, `a width is a whole number of pixels from ${SHEET_LIMITS.minWidth} to ${SHEET_LIMITS.maxWidth}`);
    }
  }
  if (tab.frozen !== undefined) {
    if (!isObject(tab.frozen)) throw new Problem(`${path}.frozen`, "frozen is an object");
    only(tab.frozen, ["rows", "columns"], `${path}.frozen`);
    if (tab.frozen.rows !== undefined && !isWhole(tab.frozen.rows, 0, SHEET_LIMITS.frozenRows)) throw new Problem(`${path}.frozen.rows`, `frozen rows are 0 to ${SHEET_LIMITS.frozenRows}`);
    if (tab.frozen.columns !== undefined && !isWhole(tab.frozen.columns, 0, SHEET_LIMITS.frozenColumns)) throw new Problem(`${path}.frozen.columns`, `frozen columns are 0 to ${SHEET_LIMITS.frozenColumns}`);
  }
  if (tab.dropdowns !== undefined) {
    if (!Array.isArray(tab.dropdowns) || tab.dropdowns.length > SHEET_LIMITS.dropdowns) throw new Problem(`${path}.dropdowns`, `dropdowns is a list of at most ${SHEET_LIMITS.dropdowns}`);
    tab.dropdowns.forEach((dropdown: unknown, index) => {
      const at = `${path}.dropdowns[${index}]`;
      if (!isObject(dropdown)) throw new Problem(at, "a dropdown is an object");
      only(dropdown, ["range", "options"], at);
      if (typeof dropdown.range !== "string" || !parseRange(dropdown.range)) throw new Problem(`${at}.range`, "a dropdown's range is a cell (C2) or cells (C2:C40)");
      const options = dropdown.options;
      if (!Array.isArray(options) || options.length < 1 || options.length > SHEET_LIMITS.options) throw new Problem(`${at}.options`, `a dropdown has 1 to ${SHEET_LIMITS.options} options`);
      for (const option of options) {
        if (typeof option !== "string" || chars(option) > SHEET_LIMITS.optionChars) throw new Problem(`${at}.options`, `an option is text of at most ${SHEET_LIMITS.optionChars} characters`);
      }
    });
  }
}

/** The first problem with `value` as a sheet, or undefined when it is one. Gateway's check (`check_sheet`) agrees. */
export function sheetProblem(value: unknown): SheetProblem | undefined {
  try {
    if (!isObject(value)) throw new Problem("", "a sheet is an object with its tabs");
    only(value, ["tabs"], "");
    const tabs = value.tabs;
    if (!Array.isArray(tabs) || tabs.length < 1 || tabs.length > SHEET_LIMITS.tabs) throw new Problem("tabs", `a sheet has 1 to ${SHEET_LIMITS.tabs} tabs`);
    const seen = { ids: new Set<string>(), names: new Set<string>(), cells: 0 };
    tabs.forEach((tab: unknown, index) => checkTab(tab, `tabs[${index}]`, seen));
    return undefined;
  } catch (failure) {
    if (failure instanceof Problem) return { path: failure.path, message: failure.message };
    throw failure;
  }
}

/** `text` as a sheet, or why it is not one. */
export function parseSheet(text: string): { sheet: Sheet } | { problem: SheetProblem } {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (failure) {
    return { problem: { path: "", message: `a sheet is JSON: ${(failure as Error).message}` } };
  }
  const problem = sheetProblem(value);
  return problem ? { problem } : { sheet: value as Sheet };
}

// ---------------------------------------------------------------- canonical text

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (!isObject(value)) return value;
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(value).sort()) {
    const inner = canonical(value[key]);
    if (inner === undefined || (isObject(inner) && Object.keys(inner).length === 0)) continue;
    out[key] = inner;
  }
  return out;
}

/**
 * The sheet as canonical text: every object's keys sorted, no white space, and empty objects (a cell with nothing in
 * it, a format with nothing set) left out. Lists keep their order.
 */
export function serializeSheet(sheet: Sheet): string {
  return JSON.stringify(canonical(sheet));
}

/** A new sheet: one empty tab. */
export function emptySheet(name = "Sheet1"): Sheet {
  return { tabs: [{ id: "t1", name }] };
}
