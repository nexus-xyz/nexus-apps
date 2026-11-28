import { cellAddress, parseRange, SHEET_LIMITS } from "@nexus/kit/sheet";

export type CellRange = NonNullable<ReturnType<typeof parseRange>>;
export type SheetSelection = { kind: "selected"; address: string; value: string; jumped: boolean } | { kind: "outside"; boundary: string };

export function readSheetSelection(data: unknown): SheetSelection | undefined {
  if (!data || typeof data !== "object" || !("kind" in data)) return;
  if (data.kind === "nexus.sheet-selected" && "column" in data && "row" in data &&
      typeof data.column === "number" && typeof data.row === "number" &&
      Number.isInteger(data.column) && Number.isInteger(data.row) && data.column >= 1 && data.column <= SHEET_LIMITS.columns &&
      data.row >= 1 && data.row <= SHEET_LIMITS.rows && "value" in data && typeof data.value === "string" && data.value.length <= 400) {
    return { kind: "selected", address: cellAddress(data.column, data.row), value: data.value, jumped: "jumped" in data && data.jumped === true };
  }
  if (data.kind === "nexus.sheet-outside" && "columns" in data && "rows" in data &&
      typeof data.columns === "number" && typeof data.rows === "number" &&
      Number.isInteger(data.columns) && Number.isInteger(data.rows) && data.columns >= 1 && data.columns <= SHEET_LIMITS.columns &&
      data.rows >= 1 && data.rows <= SHEET_LIMITS.rows) {
    return { kind: "outside", boundary: cellAddress(data.columns, data.rows) };
  }
}

export const sheetCellScript = `(() => {
  if (window.nexusMobileSheetSelect) return;
  const pane = () => {
    const tab = document.querySelector("input.tab-input:checked");
    return tab ? document.getElementById(tab.id.replace("tab-", "pane-")) : null;
  };
  const style = document.createElement("style");
  document.head.append(style);
  const post = (result) => window.ReactNativeWebView.postMessage(JSON.stringify(result));
  window.nexusMobileSheetSelect = (range, scroll) => {
    const body = pane()?.querySelector("tbody");
    if (!body) return;
    const rows = body.children.length;
    const columns = Math.max(0, (body.firstElementChild?.children.length ?? 1) - 1);
    if (range.bottom > rows || range.right > columns) {
      post({ kind: "nexus.sheet-outside", rows, columns });
      return;
    }
    const target = body.children[range.top - 1]?.children[range.left];
    if (!target) return;
    const row = "tbody > tr:nth-child(n+" + range.top + "):nth-child(-n+" + range.bottom + ")";
    const column = "td:nth-child(n+" + (range.left + 1) + "):nth-child(-n+" + (range.right + 1) + ")";
    style.textContent = ".pane table " + row + " > " + column + " { outline: 2px solid var(--text-2); outline-offset: -2px; }";
    if (scroll) target.scrollIntoView({ block: "center", inline: "center" });
    post({ kind: "nexus.sheet-selected", column: range.left, row: range.top, value: (target.textContent || "").slice(0, 400), jumped: scroll });
  };
  document.addEventListener("click", (event) => {
    const cell = event.target instanceof Element && event.target.closest("td");
    if (!cell || !pane()?.contains(cell)) return;
    const row = Array.prototype.indexOf.call(cell.parentElement.parentElement.children, cell.parentElement) + 1;
    const column = Array.prototype.indexOf.call(cell.parentElement.children, cell);
    window.nexusMobileSheetSelect({ top: row, bottom: row, left: column, right: column }, false);
  });
  const first = { top: 1, bottom: 1, left: 1, right: 1 };
  document.addEventListener("change", (event) => {
    if (event.target instanceof Element && event.target.matches("input.tab-input")) window.nexusMobileSheetSelect(first, false);
  });
  window.nexusMobileSheetSelect(first, false);
})(); true;`;

export function goToCellScript(range: CellRange): string {
  return `window.nexusMobileSheetSelect?.(${JSON.stringify(range)}, true); true;`;
}
