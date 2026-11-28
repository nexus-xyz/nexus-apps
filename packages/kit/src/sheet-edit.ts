/**
 * Typing into a sheet, and moving formulas: what the grid editor and the CSV and XLSX importers share. No
 * DOM here.
 */
import { columnIndex, columnName, parseAddress, SHEET_LIMITS, type CellFormat, type SheetCell } from "./sheet";

/** Days from 1899-12-30 (a spreadsheet's day 0) to 1970-01-01. */
const EPOCH_DAYS = 25_569;

/**
 * What a person typed, as a cell: `=` starts a formula; TRUE and FALSE; a number, with `$`, `€` or `£` (currency), `%`
 * (percent) or thousands commas; an ISO date (a date); anything else is text. `format` is the number format the input
 * implies, for a cell that has none yet.
 */
export function parseInput(text: string): { cell: Omit<SheetCell, "format"> | undefined; format?: Partial<CellFormat> } {
  if (text === "") return { cell: undefined };
  if (text.startsWith("=") && text.length > 1) return { cell: { formula: text } };
  if (/^(true|false)$/i.test(text.trim())) return { cell: { value: /^true$/i.test(text.trim()) } };
  const trimmed = text.trim();
  const money = /^(-)?([$€£])\s?(-)?([0-9][0-9,]*(?:\.[0-9]+)?)$/.exec(trimmed);
  if (money) {
    const value = Number(money[4]!.replace(/,/g, "")) * (money[1] || money[3] ? -1 : 1);
    const currency = money[2] === "€" ? "EUR" : money[2] === "£" ? "GBP" : "USD";
    if (Number.isFinite(value)) return { cell: { value }, format: { number: "currency", ...(currency === "USD" ? {} : { currency }) } };
  }
  const percent = /^(-?[0-9]+(?:\.[0-9]+)?)\s?%$/.exec(trimmed);
  if (percent) return { cell: { value: Number(percent[1]) / 100 }, format: { number: "percent" } };
  if (/^-?(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?$/i.test(trimmed)) {
    const value = Number(trimmed.replace(/,/g, ""));
    if (Number.isFinite(value)) return { cell: { value } };
  }
  const date = /^(\d{4})-(\d{2})-(\d{2})$/.exec(trimmed);
  if (date) {
    const ms = Date.UTC(Number(date[1]), Number(date[2]) - 1, Number(date[3]));
    if (!Number.isNaN(ms)) return { cell: { value: Math.round(ms / 86_400_000) + EPOCH_DAYS }, format: { number: "date" } };
  }
  return { cell: { value: text.slice(0, SHEET_LIMITS.textChars) } };
}

/** The text an editor starts from: a formula as written, a value as typed. */
export function inputOf(cell: SheetCell | undefined): string {
  if (!cell) return "";
  if (cell.formula !== undefined) return cell.formula;
  if (cell.value === undefined) return "";
  if (typeof cell.value === "boolean") return cell.value ? "TRUE" : "FALSE";
  if (typeof cell.value === "number") {
    if (cell.format?.number === "percent") return `${Number((cell.value * 100).toPrecision(12))}%`;
    if (cell.format?.number === "date") return new Date((cell.value - EPOCH_DAYS) * 86_400_000).toISOString().slice(0, 10);
  }
  return String(cell.value);
}

// ---------------------------------------------------------------- references

const REFERENCE = /('(?:[^']|'')+'!|[A-Za-z_][A-Za-z0-9_.]*!)?(\$?)([A-Z]{1,3})(\$?)([0-9]+)(?::(\$?)([A-Z]{1,3})(\$?)([0-9]+))?(?![A-Za-z0-9_(!])/g;

/** Which end of a reference `move` gets: a single cell, or a range's first or last corner. */
export type ReferenceEnd = "single" | "start" | "end";

/** `formula` with each reference rewritten by `move`, leaving text in quotes alone. A gone reference is `#REF!`. */
export function rewriteReferences(formula: string, move: (tab: string | undefined, at: { column: number; row: number; fixedColumn: boolean; fixedRow: boolean }, end: ReferenceEnd) => { row: number; column: number } | undefined): string {
  const parts = formula.split(/("(?:[^"]|"")*")/);
  return parts
    .map((part, index) => {
      if (index % 2 === 1) return part;
      return part.replace(
        REFERENCE,
        (match: string, prefix: string | undefined, fc1: string, c1: string, fr1: string, r1: string, fc2: string | undefined, c2: string | undefined, fr2: string | undefined, r2: string | undefined, offset: number, whole: string) => {
          const before = whole[offset - 1];
          if (before && /[A-Za-z0-9_.$]/.test(before)) return match;
          const tab = prefix ? prefix.slice(0, -1).replace(/^'|'$/g, "").replace(/''/g, "'") : undefined;
          const one = (fc: string, letters: string, fr: string, digits: string, end: ReferenceEnd) => {
            const next = move(tab, { column: columnIndex(letters), row: Number(digits), fixedColumn: fc === "$", fixedRow: fr === "$" }, end);
            if (!next || next.row < 1 || next.column < 1 || next.row > SHEET_LIMITS.rows || next.column > SHEET_LIMITS.columns) return undefined;
            return `${fc}${columnName(next.column)}${fr}${next.row}`;
          };
          if (c2 === undefined) {
            const only = one(fc1, c1, fr1, r1, "single");
            return only ? `${prefix ?? ""}${only}` : "#REF!";
          }
          const first = one(fc1, c1, fr1, r1, "start");
          const second = one(fc2 ?? "", c2, fr2 ?? "", r2!, "end");
          const ordered = first && second && parseAddress(first.replace(/\$/g, "")) && parseAddress(second.replace(/\$/g, ""));
          if (!ordered) return "#REF!";
          const [a, b] = [parseAddress(first.replace(/\$/g, ""))!, parseAddress(second.replace(/\$/g, ""))!];
          return a.row <= b.row && a.column <= b.column ? `${prefix ?? ""}${first}:${second}` : "#REF!";
        },
      );
    })
    .join("");
}

/** The formula copied `rows` down and `columns` right: references without `$` move with it. */
export function offsetFormula(formula: string, rows: number, columns: number): string {
  return rewriteReferences(formula, (_, at) => ({ row: at.fixedRow ? at.row : at.row + rows, column: at.fixedColumn ? at.column : at.column + columns }));
}

