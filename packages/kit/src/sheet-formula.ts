/**
 * The formula engine of Nexus Sheets: pure TypeScript, no DOM, no library. The pages host renders a sheet
 * with it, and the editor computes as people type.
 *
 * - **Grammar.** Numbers, "text" ("" for a quote), TRUE and FALSE, references (`A1`, `$A$1`, `A1:B9`, `A:A`,
 *   `Tab!A1`, `'Q3 plan'!A1:B2`), `+ - * / ^ & %`, `= <> < > <= >=`, and calls. Precedence is Excel's: negation, then
 *   `%`, then `^` (left to right), then `* /`, `+ -`, `&` and the comparisons.
 * - **Functions.** `FUNCTIONS` below: SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, IF, AND, OR, NOT, ROUND, ABS, CONCAT,
 *   VLOOKUP, SUMIF, COUNTIF, IFERROR, LEN, UPPER, LOWER, TRIM, TODAY. Any other name gives `#NAME?`.
 * - **Order.** `computeSheet` finds each formula's references before it computes, orders the cells so that each comes
 *   after what it reads (Tarjan, with no recursion, so a chain of 100,000 rows is fine), and gives every cell of a
 *   cycle `#CYCLE!`. A bad reference is `#REF!`. Nothing a sheet holds makes it throw.
 */
import type { Sheet } from "./sheet";

export type SheetErrorCode = "#REF!" | "#NAME?" | "#VALUE!" | "#DIV/0!" | "#N/A" | "#NUM!" | "#CYCLE!" | "#ERROR!";
export interface ErrorValue {
  error: SheetErrorCode;
  /** Why, for a tooltip. */
  message?: string;
}
/** A computed value. `null` is an empty cell. */
export type Scalar = number | string | boolean | null | ErrorValue;

const MAX_COLUMN = 702;
const MAX_ROW = 100_000;
/** The most cells one range may read. */
const MAX_RANGE_CELLS = 1_000_000;
/** Days from 1899-12-30 (a spreadsheet's day 0) to 1970-01-01. */
const EPOCH_DAYS = 25_569;

const fail = (error: SheetErrorCode, message?: string): ErrorValue => (message ? { error, message } : { error });
export const isError = (value: unknown): value is ErrorValue => typeof value === "object" && value !== null && "error" in value;

// ---------------------------------------------------------------- parsing

/** A reference: `tab` is the tab's name as written, unset for the formula's own tab. Rows and columns from 1. */
interface Area {
  tab?: string;
  top: number;
  left: number;
  bottom: number;
  right: number;
  /** A whole column (`A:A`): its rows end at the tab's last used row. */
  columns?: boolean;
}

export type Node =
  | { kind: "value"; value: Scalar }
  | { kind: "ref"; area: Area; single: boolean }
  | { kind: "unary"; op: "-" | "+"; arg: Node }
  | { kind: "percent"; arg: Node }
  | { kind: "binary"; op: string; left: Node; right: Node }
  | { kind: "call"; name: string; args: Node[] };

type Token =
  | { type: "value"; value: Scalar }
  | { type: "ref"; area: Area; single: boolean }
  | { type: "func"; name: string }
  | { type: "op"; op: string }
  | { type: "(" | ")" | "," };

class Syntax extends Error {}

const letters = (text: string) => {
  let index = 0;
  for (const char of text.toUpperCase()) index = index * 26 + (char.charCodeAt(0) - 64);
  return index;
};

const CELL = /^\$?([A-Za-z]{1,3})\$?([0-9]+)$/;
const COLUMN_ONLY = /^\$?([A-Za-z]{1,3})$/;

function cellOf(text: string): { column: number; row: number } | undefined {
  const match = CELL.exec(text);
  return match ? { column: letters(match[1]!), row: Number(match[2]) } : undefined;
}

/** `text` from `at`, a reference without its tab: `A1`, `A1:B2` or `A:B`. */
function readReference(text: string, at: number): { end: number; area?: Omit<Area, "tab">; single: boolean } {
  const word = /^[$A-Za-z0-9]+/.exec(text.slice(at))?.[0] ?? "";
  let end = at + word.length;
  const first = cellOf(word);
  const firstColumn = first ? undefined : COLUMN_ONLY.exec(word)?.[1];
  if (text[end] === ":") {
    const second = /^[$A-Za-z0-9]+/.exec(text.slice(end + 1))?.[0] ?? "";
    const to = cellOf(second);
    const toColumn = to ? undefined : COLUMN_ONLY.exec(second)?.[1];
    if (first && to) {
      end += 1 + second.length;
      return { end, single: false, area: { top: Math.min(first.row, to.row), bottom: Math.max(first.row, to.row), left: Math.min(first.column, to.column), right: Math.max(first.column, to.column) } };
    }
    if (firstColumn && toColumn) {
      end += 1 + second.length;
      const [a, b] = [letters(firstColumn), letters(toColumn)];
      return { end, single: false, area: { top: 1, bottom: MAX_ROW, left: Math.min(a, b), right: Math.max(a, b), columns: true } };
    }
  }
  if (first) return { end, single: true, area: { top: first.row, bottom: first.row, left: first.column, right: first.column } };
  return { end, single: true };
}

function inBounds(area: Omit<Area, "tab">): boolean {
  return area.top >= 1 && area.left >= 1 && area.bottom <= MAX_ROW && area.right <= MAX_COLUMN;
}

function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let at = 0;
  const reference = (tab: string | undefined, from: number) => {
    const read = readReference(text, from);
    if (!read.area) {
      if (tab !== undefined) throw new Syntax(`${tab}! needs a cell after it`);
      return false;
    }
    at = read.end;
    tokens.push(inBounds(read.area) ? { type: "ref", area: { ...read.area, tab }, single: read.single } : { type: "value", value: fail("#REF!", "outside the sheet") });
    return true;
  };
  while (at < text.length) {
    const char = text[at]!;
    if (/\s/.test(char)) {
      at += 1;
    } else if (/[0-9.]/.test(char)) {
      const number = /^(?:[0-9]+\.?[0-9]*|\.[0-9]+)(?:[eE][+-]?[0-9]+)?/.exec(text.slice(at))?.[0];
      // `1:3` (rows) is not supported; a number is only a number.
      if (!number || number === ".") throw new Syntax(`unexpected ${char}`);
      tokens.push({ type: "value", value: Number(number) });
      at += number.length;
    } else if (char === '"') {
      let value = "";
      at += 1;
      for (;;) {
        if (at >= text.length) throw new Syntax("text has no closing quote");
        if (text[at] === '"') {
          if (text[at + 1] === '"') {
            value += '"';
            at += 2;
            continue;
          }
          at += 1;
          break;
        }
        value += text[at];
        at += 1;
      }
      tokens.push({ type: "value", value });
    } else if (char === "'") {
      const close = text.indexOf("'!", at + 1);
      if (close < 0) throw new Syntax("a quoted tab name needs '! after it");
      const tab = text.slice(at + 1, close).replace(/''/g, "'");
      reference(tab, close + 2);
    } else if (/[A-Za-z_$]/.test(char)) {
      const word = /^[A-Za-z0-9_.$]+/.exec(text.slice(at))![0];
      const after = text[at + word.length];
      if (after === "!") {
        reference(word, at + word.length + 1);
      } else if (after === "(" && /^[A-Za-z_][A-Za-z0-9_.]*$/.test(word)) {
        tokens.push({ type: "func", name: word.toUpperCase() });
        at += word.length;
      } else if (/^(TRUE|FALSE)$/i.test(word) && after !== ":") {
        tokens.push({ type: "value", value: word.toUpperCase() === "TRUE" });
        at += word.length;
      } else if (!reference(undefined, at)) {
        tokens.push({ type: "value", value: fail("#NAME?", `unknown name ${word}`) });
        at += word.length;
      }
    } else if (char === "#") {
      const code = /^#(?:REF!|NAME\?|VALUE!|DIV\/0!|N\/A|NUM!|CYCLE!|ERROR!)/i.exec(text.slice(at))?.[0];
      if (!code) throw new Syntax("unexpected #");
      tokens.push({ type: "value", value: fail(code.toUpperCase() as SheetErrorCode) });
      at += code.length;
    } else if (char === "(" || char === ")" || char === ",") {
      tokens.push({ type: char });
      at += 1;
    } else {
      const op = /^(<=|>=|<>|[-+*/^&%=<>])/.exec(text.slice(at))?.[0];
      if (!op) throw new Syntax(`unexpected ${char}`);
      tokens.push({ type: "op", op });
      at += op.length;
    }
  }
  return tokens;
}

class Parser {
  private at = 0;
  constructor(private readonly tokens: Token[]) {}

  parse(): Node {
    const node = this.comparison();
    if (this.at < this.tokens.length) throw new Syntax("unexpected text after the formula");
    return node;
  }

  private peekOp(...ops: string[]): string | undefined {
    const token = this.tokens[this.at];
    return token?.type === "op" && ops.includes(token.op) ? token.op : undefined;
  }

  private binary(next: () => Node, ops: string[]): Node {
    let left = next();
    for (let op = this.peekOp(...ops); op; op = this.peekOp(...ops)) {
      this.at += 1;
      left = { kind: "binary", op, left, right: next() };
    }
    return left;
  }

  private comparison = (): Node => this.binary(this.concat, ["=", "<>", "<", ">", "<=", ">="]);
  private concat = (): Node => this.binary(this.additive, ["&"]);
  private additive = (): Node => this.binary(this.multiplicative, ["+", "-"]);
  private multiplicative = (): Node => this.binary(this.power, ["*", "/"]);
  private power = (): Node => this.binary(this.unary, ["^"]);

  private unary = (): Node => {
    const op = this.peekOp("-", "+");
    if (op) {
      this.at += 1;
      return { kind: "unary", op: op as "-" | "+", arg: this.unary() };
    }
    let node = this.primary();
    while (this.peekOp("%")) {
      this.at += 1;
      node = { kind: "percent", arg: node };
    }
    return node;
  };

  private primary(): Node {
    const token = this.tokens[this.at++];
    if (!token) throw new Syntax("the formula ends too soon");
    if (token.type === "value") return { kind: "value", value: token.value };
    if (token.type === "ref") return { kind: "ref", area: token.area, single: token.single };
    if (token.type === "(") {
      const inner = this.comparison();
      if (this.tokens[this.at++]?.type !== ")") throw new Syntax("a ( has no )");
      return inner;
    }
    if (token.type === "func") {
      this.at += 1;
      const args: Node[] = [];
      if (this.tokens[this.at]?.type === ")") {
        this.at += 1;
        return { kind: "call", name: token.name, args };
      }
      for (;;) {
        // An empty argument (`IF(A1,,2)`) is an empty value.
        const next = this.tokens[this.at]?.type;
        args.push(next === "," || next === ")" ? { kind: "value", value: null } : this.comparison());
        const separator = this.tokens[this.at++];
        if (separator?.type === ")") return { kind: "call", name: token.name, args };
        if (separator?.type !== ",") throw new Syntax(`${token.name}( has no )`);
      }
    }
    throw new Syntax(token.type === "op" ? `unexpected ${token.op}` : `unexpected ${token.type}`);
  }
}

/** A formula (with or without its `=`) as a tree, or the `#ERROR!` its syntax gives. */
export function parseFormula(formula: string): Node | ErrorValue {
  try {
    return new Parser(tokenize(formula.startsWith("=") ? formula.slice(1) : formula)).parse();
  } catch (failure) {
    if (failure instanceof Syntax) return fail("#ERROR!", failure.message);
    throw failure;
  }
}

// ---------------------------------------------------------------- values

type Grid = Scalar[][];
type Value = Scalar | { grid: Grid };

const isGrid = (value: Value): value is { grid: Grid } => typeof value === "object" && value !== null && "grid" in value;

/** A number as text the way a cell shows it in General: up to 15 significant digits. */
export function numberText(value: number): string {
  if (Object.is(value, -0)) return "0";
  return String(Number(value.toPrecision(15)));
}

function toNumber(value: Scalar): number | ErrorValue {
  if (typeof value === "number") return value;
  if (value === null) return 0;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (typeof value === "string") {
    const trimmed = value.trim();
    const number = trimmed === "" ? NaN : Number(trimmed.replace(/,/g, ""));
    return Number.isFinite(number) ? number : fail("#VALUE!", `"${value.slice(0, 40)}" is not a number`);
  }
  return value;
}

function toText(value: Scalar): string | ErrorValue {
  if (value === null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number") return numberText(value);
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  return value;
}

function toBool(value: Scalar): boolean | ErrorValue {
  if (typeof value === "boolean") return value;
  if (value === null) return false;
  if (typeof value === "number") return value !== 0;
  if (typeof value === "string") {
    if (/^true$/i.test(value)) return true;
    if (/^false$/i.test(value)) return false;
    return fail("#VALUE!", `"${value.slice(0, 40)}" is not TRUE or FALSE`);
  }
  return value;
}

/** A range where one value is wanted: its one cell, else `#VALUE!`. */
function scalar(value: Value): Scalar {
  if (!isGrid(value)) return value;
  const [row] = value.grid;
  return value.grid.length === 1 && row?.length === 1 ? row[0]! : fail("#VALUE!", "a range where one value is wanted");
}

const finite = (value: number): number | ErrorValue => (Number.isFinite(value) ? value : fail("#NUM!", "too large"));

/** Type order for comparing: numbers, then text, then TRUE and FALSE. */
const rank = (value: Scalar) => (typeof value === "number" ? 0 : typeof value === "string" ? 1 : 2);

function compare(a: Scalar, b: Scalar): number {
  // An empty cell takes the other side's type.
  if (a === null) a = typeof b === "string" ? "" : typeof b === "boolean" ? false : 0;
  if (b === null) b = typeof a === "string" ? "" : typeof a === "boolean" ? false : 0;
  if (rank(a) !== rank(b)) return rank(a) - rank(b);
  if (typeof a === "string" && typeof b === "string") {
    const [x, y] = [a.toLowerCase(), b.toLowerCase()];
    return x < y ? -1 : x > y ? 1 : 0;
  }
  return Number(a) - Number(b);
}

function arithmetic(op: string, a: number, b: number): number | ErrorValue {
  switch (op) {
    case "+":
      return finite(a + b);
    case "-":
      return finite(a - b);
    case "*":
      return finite(a * b);
    case "/":
      return b === 0 ? fail("#DIV/0!", "division by zero") : finite(a / b);
    default:
      return Number.isNaN(a ** b) ? fail("#NUM!", "no real result") : finite(a ** b);
  }
}

// ---------------------------------------------------------------- criteria (SUMIF, COUNTIF)

function wildcard(pattern: string): RegExp {
  let source = "";
  for (let at = 0; at < pattern.length; at++) {
    const char = pattern[at]!;
    if (char === "~" && at + 1 < pattern.length) source += pattern[++at]!.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    else if (char === "*") source += "[\\s\\S]*";
    else if (char === "?") source += "[\\s\\S]";
    else source += char.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${source}$`, "i");
}

function criterion(test: Scalar): (value: Scalar) => boolean {
  if (typeof test !== "string") return (value) => value !== null && !isError(value) && compare(value, test) === 0;
  const [, op = "=", operand = ""] = /^(<=|>=|<>|<|>|=)?([\s\S]*)$/.exec(test) ?? [];
  const number = operand.trim() === "" ? NaN : Number(operand);
  const numeric = Number.isFinite(number);
  if (op === "=" || op === "<>") {
    const pattern = operand === "" ? undefined : wildcard(operand);
    const equal = (value: Scalar) => {
      if (operand === "") return value === null || value === "";
      if (isError(value) || value === null) return false;
      if (numeric) return typeof value === "number" ? value === number : typeof value === "string" && pattern!.test(value);
      return pattern!.test(String(toText(value)));
    };
    return op === "=" ? equal : (value) => !isError(value) && !equal(value);
  }
  return (value) => {
    if (value === null || isError(value)) return false;
    const order = numeric ? (typeof value === "number" ? value - number : NaN) : typeof value === "string" ? compare(value, operand) : NaN;
    if (Number.isNaN(order)) return false;
    return op === "<" ? order < 0 : op === ">" ? order > 0 : op === "<=" ? order <= 0 : order >= 0;
  };
}

// ---------------------------------------------------------------- functions

type Evaluate = (node: Node) => Value;
interface CallContext {
  today: Date;
}
type Fn = { min: number; max: number; call: (args: Node[], evaluate: Evaluate, context: CallContext) => Scalar };

/**
 * Every value of the arguments, ranges spread. `direct`: written as the argument itself (`SUM(1, "2")`), not read from
 * a cell, so it is converted rather than skipped.
 */
function* spread(args: Node[], evaluate: Evaluate): Generator<{ value: Scalar; direct: boolean }> {
  for (const arg of args) {
    const value = evaluate(arg);
    if (isGrid(value)) {
      for (const row of value.grid) for (const cell of row) yield { value: cell, direct: false };
    } else {
      yield { value, direct: arg.kind !== "ref" };
    }
  }
}

/** The numbers of the arguments: in a range only numbers count, a direct argument must convert. */
function numbers(args: Node[], evaluate: Evaluate): number[] | ErrorValue {
  const out: number[] = [];
  for (const { value, direct } of spread(args, evaluate)) {
    if (isError(value)) return value;
    if (direct) {
      if (value === null) continue;
      const number = toNumber(value);
      if (isError(number)) return number;
      out.push(number);
    } else if (typeof value === "number") {
      out.push(value);
    }
  }
  return out;
}

const grid = (value: Value): Grid => (isGrid(value) ? value.grid : [[value]]);

function logical(args: Node[], evaluate: Evaluate, every: boolean): Scalar {
  let seen = false;
  let result = every;
  for (const { value, direct } of spread(args, evaluate)) {
    if (isError(value)) return value;
    if (value === null || (!direct && typeof value === "string")) continue;
    const bool = toBool(value);
    if (isError(bool)) return bool;
    seen = true;
    result = every ? result && bool : result || bool;
  }
  return seen ? result : fail("#VALUE!", "no TRUE or FALSE to test");
}

function round(value: number, digits: number): number {
  const factor = 10 ** Math.trunc(digits);
  // Half away from zero, after removing the binary noise (1.005 is 1.00499…).
  const scaled = Number((Math.abs(value) * factor).toPrecision(15));
  return (Math.sign(value) * Math.round(scaled)) / factor;
}

function textFn(transform: (text: string) => Scalar): Fn {
  return {
    min: 1,
    max: 1,
    call: ([arg], evaluate) => {
      const text = toText(scalar(evaluate(arg!)));
      return isError(text) ? text : transform(text);
    },
  };
}

export const FUNCTIONS: Record<string, Fn> = {
  SUM: { min: 1, max: 255, call: (args, ev) => { const all = numbers(args, ev); return isError(all) ? all : finite(all.reduce((a, b) => a + b, 0)); } },
  AVERAGE: {
    min: 1,
    max: 255,
    call: (args, ev) => {
      const all = numbers(args, ev);
      if (isError(all)) return all;
      return all.length ? finite(all.reduce((a, b) => a + b, 0) / all.length) : fail("#DIV/0!", "no numbers to average");
    },
  },
  MIN: { min: 1, max: 255, call: (args, ev) => { const all = numbers(args, ev); return isError(all) ? all : all.length ? all.reduce((a, b) => Math.min(a, b)) : 0; } },
  MAX: { min: 1, max: 255, call: (args, ev) => { const all = numbers(args, ev); return isError(all) ? all : all.length ? all.reduce((a, b) => Math.max(a, b)) : 0; } },
  COUNT: {
    min: 1,
    max: 255,
    call: (args, ev) => {
      let count = 0;
      for (const { value, direct } of spread(args, ev)) {
        if (typeof value === "number" || (direct && value !== null && !isError(value) && !isError(toNumber(value)))) count += 1;
      }
      return count;
    },
  },
  COUNTA: {
    min: 1,
    max: 255,
    call: (args, ev) => {
      let count = 0;
      for (const { value } of spread(args, ev)) if (value !== null) count += 1;
      return count;
    },
  },
  IF: {
    min: 1,
    max: 3,
    call: ([test, then, otherwise], ev) => {
      const condition = toBool(scalar(ev(test!)));
      if (isError(condition)) return condition;
      if (condition) return then ? scalar(ev(then)) : true;
      return otherwise ? scalar(ev(otherwise)) : false;
    },
  },
  AND: { min: 1, max: 255, call: (args, ev) => logical(args, ev, true) },
  OR: { min: 1, max: 255, call: (args, ev) => logical(args, ev, false) },
  NOT: {
    min: 1,
    max: 1,
    call: ([arg], ev) => {
      const bool = toBool(scalar(ev(arg!)));
      return isError(bool) ? bool : !bool;
    },
  },
  ROUND: {
    min: 1,
    max: 2,
    call: ([arg, places], ev) => {
      const value = toNumber(scalar(ev(arg!)));
      const digits = places ? toNumber(scalar(ev(places))) : 0;
      if (isError(value)) return value;
      if (isError(digits)) return digits;
      return finite(round(value, digits));
    },
  },
  ABS: {
    min: 1,
    max: 1,
    call: ([arg], ev) => {
      const value = toNumber(scalar(ev(arg!)));
      return isError(value) ? value : Math.abs(value);
    },
  },
  CONCAT: {
    min: 1,
    max: 255,
    call: (args, ev) => {
      let out = "";
      for (const { value } of spread(args, ev)) {
        const text = toText(value);
        if (isError(text)) return text;
        out += text;
      }
      return out;
    },
  },
  VLOOKUP: {
    min: 3,
    max: 4,
    call: ([lookup, table, column, approximate], ev) => {
      const wanted = scalar(ev(lookup!));
      if (isError(wanted)) return wanted;
      const rows = grid(ev(table!));
      const index = toNumber(scalar(ev(column!)));
      if (isError(index)) return index;
      const width = rows[0]?.length ?? 0;
      if (index < 1 || index > width) return fail("#REF!", `column ${index} is outside the table`);
      const sorted = approximate ? toBool(scalar(ev(approximate))) : true;
      if (isError(sorted)) return sorted;
      const at = Math.trunc(index) - 1;
      if (!sorted) {
        const row = rows.find((row) => row[0] !== null && row[0] !== undefined && !isError(row[0]) && compare(row[0], wanted) === 0);
        return row ? (row[at] ?? null) : fail("#N/A", "not found");
      }
      // Sorted: the last row whose first value is at most the one looked up.
      let found: Scalar[] | undefined;
      for (const row of rows) {
        const key = row[0] ?? null;
        if (key === null || isError(key) || rank(key) !== rank(wanted ?? 0)) continue;
        if (compare(key, wanted) > 0) break;
        found = row;
      }
      return found ? (found[at] ?? null) : fail("#N/A", "not found");
    },
  },
  SUMIF: {
    min: 2,
    max: 3,
    call: ([range, test, sumRange], ev) => {
      const keys = grid(ev(range!));
      const condition = scalar(ev(test!));
      if (isError(condition)) return condition;
      const sums = sumRange ? grid(ev(sumRange)) : keys;
      const matches = criterion(condition);
      let total = 0;
      keys.forEach((row, r) =>
        row.forEach((key, c) => {
          const value = sums[r]?.[c];
          if (matches(key) && typeof value === "number") total += value;
        }),
      );
      return finite(total);
    },
  },
  COUNTIF: {
    min: 2,
    max: 2,
    call: ([range, test], ev) => {
      const condition = scalar(ev(test!));
      if (isError(condition)) return condition;
      const matches = criterion(condition);
      let count = 0;
      for (const row of grid(ev(range!))) for (const value of row) if (matches(value)) count += 1;
      return count;
    },
  },
  IFERROR: {
    min: 2,
    max: 2,
    call: ([value, fallback], ev) => {
      const result = scalar(ev(value!));
      return isError(result) ? scalar(ev(fallback!)) : result;
    },
  },
  LEN: textFn((text) => [...text].length),
  UPPER: textFn((text) => text.toUpperCase()),
  LOWER: textFn((text) => text.toLowerCase()),
  TRIM: textFn((text) => text.trim().replace(/ {2,}/g, " ")),
  TODAY: {
    min: 0,
    max: 0,
    call: (_, __, { today }) => Math.floor(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate()) / 86_400_000) + EPOCH_DAYS,
  },
};

// ---------------------------------------------------------------- the sheet

/** A sheet's computed values. */
export interface ComputedSheet {
  /** The value of the cell at `address` (`B7`) of the tab `tabId`: its value, its formula's result, or `null`. */
  value(tabId: string, address: string): Scalar;
}

const key = (tab: number, column: number, row: number) => `${tab}:${column}:${row}`;

interface FormulaCell {
  key: string;
  tab: number;
  node: Node | ErrorValue;
}

/**
 * Compute every formula of `sheet`. `today` is TODAY()'s date (unset: now, in UTC). Cells are read through `value`;
 * each formula is computed once, here.
 */
export function computeSheet(sheet: Sheet, options: { today?: Date } = {}): ComputedSheet {
  const context: CallContext = { today: options.today ?? new Date() };
  const tabs = sheet.tabs;
  const tabIndex = new Map(tabs.map((tab, index) => [tab.name.toLowerCase(), index]));
  const values = new Map<string, Scalar>();
  const formulas = new Map<string, FormulaCell>();
  /** Per tab, per column: the rows that hold a formula, sorted. */
  const formulaRows = tabs.map(() => new Map<number, number[]>());
  /** Per tab: the last row that holds anything, for whole-column ranges. */
  const lastRow = tabs.map(() => 0);

  tabs.forEach((tab, t) => {
    for (const [address, cell] of Object.entries(tab.cells ?? {})) {
      const at = /^([A-Z]{1,2})([0-9]+)$/.exec(address);
      if (!at) continue;
      const [column, row] = [letters(at[1]!), Number(at[2])];
      const id = key(t, column, row);
      lastRow[t] = Math.max(lastRow[t]!, row);
      if (cell.formula !== undefined) {
        formulas.set(id, { key: id, tab: t, node: parseFormula(cell.formula) });
        const rows = formulaRows[t]!.get(column) ?? [];
        rows.push(row);
        formulaRows[t]!.set(column, rows);
      } else if (cell.value !== undefined) {
        values.set(id, cell.value);
      }
    }
  });
  for (const columns of formulaRows) for (const rows of columns.values()) rows.sort((a, b) => a - b);

  const tabOf = (area: Area, own: number) => (area.tab === undefined ? own : tabIndex.get(area.tab.toLowerCase()));
  const bottomOf = (area: Area, tab: number) => (area.columns ? Math.min(area.bottom, Math.max(lastRow[tab]!, 1)) : area.bottom);

  // The formula cells each formula reads: its references, and the formulas inside its ranges.
  const dependencies = (cell: FormulaCell): string[] => {
    const out: string[] = [];
    const visit = (node: Node) => {
      if (node.kind === "ref") {
        const tab = tabOf(node.area, cell.tab);
        if (tab === undefined) return;
        const bottom = bottomOf(node.area, tab);
        for (let column = node.area.left; column <= node.area.right; column++) {
          const rows = formulaRows[tab]!.get(column);
          if (!rows) continue;
          let low = 0;
          let high = rows.length;
          while (low < high) {
            const mid = (low + high) >> 1;
            if (rows[mid]! < node.area.top) low = mid + 1;
            else high = mid;
          }
          for (let at = low; at < rows.length && rows[at]! <= bottom; at++) out.push(key(tab, column, rows[at]!));
        }
      } else if (node.kind === "unary" || node.kind === "percent") visit(node.arg);
      else if (node.kind === "binary") {
        visit(node.left);
        visit(node.right);
      } else if (node.kind === "call") node.args.forEach(visit);
    };
    if (!isError(cell.node)) visit(cell.node);
    return out;
  };

  const read = (tab: number, column: number, row: number): Scalar => values.get(key(tab, column, row)) ?? null;

  const evaluateIn = (own: number): Evaluate => {
    const evaluate: Evaluate = (node) => {
      switch (node.kind) {
        case "value":
          return node.value;
        case "ref": {
          const tab = tabOf(node.area, own);
          if (tab === undefined) return fail("#REF!", `no tab ${node.area.tab}`);
          if (node.single) return read(tab, node.area.left, node.area.top);
          const bottom = bottomOf(node.area, tab);
          const size = (bottom - node.area.top + 1) * (node.area.right - node.area.left + 1);
          if (size > MAX_RANGE_CELLS) return fail("#REF!", `a range reads at most ${MAX_RANGE_CELLS} cells`);
          const rows: Grid = [];
          for (let row = node.area.top; row <= bottom; row++) {
            const line: Scalar[] = [];
            for (let column = node.area.left; column <= node.area.right; column++) line.push(read(tab, column, row));
            rows.push(line);
          }
          return { grid: rows };
        }
        case "unary": {
          const value = toNumber(scalar(evaluate(node.arg)));
          return isError(value) ? value : node.op === "-" ? -value : value;
        }
        case "percent": {
          const value = toNumber(scalar(evaluate(node.arg)));
          return isError(value) ? value : value / 100;
        }
        case "binary": {
          const left = scalar(evaluate(node.left));
          if (isError(left)) return left;
          const right = scalar(evaluate(node.right));
          if (isError(right)) return right;
          if (node.op === "&") return String(toText(left)) + String(toText(right));
          if (["=", "<>", "<", ">", "<=", ">="].includes(node.op)) {
            const order = compare(left, right);
            return node.op === "=" ? order === 0 : node.op === "<>" ? order !== 0 : node.op === "<" ? order < 0 : node.op === ">" ? order > 0 : node.op === "<=" ? order <= 0 : order >= 0;
          }
          const a = toNumber(left);
          if (isError(a)) return a;
          const b = toNumber(right);
          if (isError(b)) return b;
          return arithmetic(node.op, a, b);
        }
        case "call": {
          const fn = FUNCTIONS[node.name];
          if (!fn) return fail("#NAME?", `unknown function ${node.name}`);
          if (node.args.length < fn.min || node.args.length > fn.max) {
            return fail("#VALUE!", `${node.name} takes ${fn.min === fn.max ? fn.min : `${fn.min} to ${fn.max}`} arguments`);
          }
          return fn.call(node.args, evaluate, context);
        }
      }
    };
    return evaluate;
  };

  // Tarjan's strongly connected components, iteratively: each component comes out after the ones it reads.
  const order: string[][] = [];
  const index = new Map<string, number>();
  const low = new Map<string, number>();
  const onStack = new Set<string>();
  const stack: string[] = [];
  const edges = new Map<string, string[]>();
  let counter = 0;
  for (const start of formulas.keys()) {
    if (index.has(start)) continue;
    const work: { node: string; next: number }[] = [{ node: start, next: 0 }];
    index.set(start, counter);
    low.set(start, counter++);
    stack.push(start);
    onStack.add(start);
    while (work.length) {
      const frame = work[work.length - 1]!;
      let out = edges.get(frame.node);
      if (!out) {
        out = dependencies(formulas.get(frame.node)!);
        edges.set(frame.node, out);
      }
      if (frame.next < out.length) {
        const next = out[frame.next++]!;
        if (!index.has(next)) {
          index.set(next, counter);
          low.set(next, counter++);
          stack.push(next);
          onStack.add(next);
          work.push({ node: next, next: 0 });
        } else if (onStack.has(next)) {
          low.set(frame.node, Math.min(low.get(frame.node)!, index.get(next)!));
        }
        continue;
      }
      work.pop();
      const parent = work[work.length - 1];
      if (parent) low.set(parent.node, Math.min(low.get(parent.node)!, low.get(frame.node)!));
      if (low.get(frame.node) === index.get(frame.node)) {
        const component: string[] = [];
        let member: string;
        do {
          member = stack.pop()!;
          onStack.delete(member);
          component.push(member);
        } while (member !== frame.node);
        order.push(component);
      }
    }
  }

  const evaluators = tabs.map((_, t) => evaluateIn(t));
  for (const component of order) {
    const [only] = component;
    const cyclic = component.length > 1 || edges.get(only!)!.includes(only!);
    for (const id of component) {
      const cell = formulas.get(id)!;
      if (cyclic) {
        values.set(id, fail("#CYCLE!", "the formula reads its own result"));
      } else if (isError(cell.node)) {
        values.set(id, cell.node);
      } else {
        const result = scalar(evaluators[cell.tab]!(cell.node));
        values.set(id, typeof result === "number" ? (Number.isFinite(result) ? result : fail("#NUM!")) : result);
      }
    }
  }

  const tabIds = new Map(tabs.map((tab, t) => [tab.id, t]));
  return {
    value(tabId, address) {
      const t = tabIds.get(tabId);
      const at = /^([A-Z]{1,2})([0-9]+)$/.exec(address);
      if (t === undefined || !at) return null;
      return values.get(key(t, letters(at[1]!), Number(at[2]))) ?? null;
    },
  };
}
