/** How a cell shows its computed value, in the pages host's render and in the editor. Always `en-US`. */
import type { CellFormat } from "./sheet";
import { isError, numberText, type Scalar } from "./sheet-formula";

/** Days from 1899-12-30 (a spreadsheet's day 0) to 1970-01-01. */
const EPOCH_DAYS = 25_569;

function grouped(value: number, decimals: number): string {
  return new Intl.NumberFormat("en-US", { minimumFractionDigits: decimals, maximumFractionDigits: decimals }).format(value);
}

/** `value` as a cell shows it under `format`. */
export function formatCellValue(value: Scalar, format: CellFormat = {}): string {
  if (value === null) return "";
  if (isError(value)) return value.error;
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  if (typeof value === "string") return value;
  switch (format.number) {
    case "number":
      return grouped(value, format.decimals ?? 2);
    case "percent":
      return `${grouped(value * 100, format.decimals ?? 0)}%`;
    case "currency":
      return new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: format.currency ?? "USD",
        minimumFractionDigits: format.decimals ?? 2,
        maximumFractionDigits: format.decimals ?? 2,
      }).format(value);
    case "date": {
      const date = new Date((Math.floor(value) - EPOCH_DAYS) * 86_400_000);
      return Number.isNaN(date.getTime()) ? numberText(value) : date.toISOString().slice(0, 10);
    }
    case "text":
      return numberText(value);
    default:
      if (format.decimals !== undefined) return value.toFixed(format.decimals);
      if (value !== 0 && (Math.abs(value) >= 1e15 || Math.abs(value) < 1e-9)) return value.toExponential(5).replace(/\.?0+e/, "e");
      // General shows at most 11 characters, as a spreadsheet's General does: fewer digits until it fits.
      for (let digits = 11; digits > 1; digits--) {
        const text = String(Number(value.toPrecision(digits)));
        if (text.length <= 11) return text;
      }
      return String(Math.round(value));
  }
}

/** Dark ink on a light fill, light ink on a dark one (WCAG relative luminance, split at 0.179): an author's fill reads in either theme. */
export function inkOn(hex: string): string {
  const channel = (at: number) => {
    const value = parseInt(hex.slice(at, at + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  const luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
  return luminance > 0.179 ? "#1c1c1e" : "#ececec";
}

/** Whether a value lines up on the right by default: numbers do, text does not. */
export const alignsRight = (value: Scalar) => typeof value === "number" || typeof value === "boolean";

/** The fills the grid offers: light, so the ink on them stays dark in either theme (`inkOn`). */
export const SHEET_FILLS = [
  { label: "Grey", color: "#eeeef0" },
  { label: "Blue", color: "#dbe8fb" },
  { label: "Green", color: "#dcf1e4" },
  { label: "Yellow", color: "#fbf1c7" },
  { label: "Red", color: "#fbe0df" },
  { label: "Purple", color: "#ece2f8" },
] as const;
