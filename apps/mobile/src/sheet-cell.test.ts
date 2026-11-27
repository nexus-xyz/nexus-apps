// @vitest-environment happy-dom
import { parseRange, serializeSheet } from "@nexus/kit/sheet";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { expect, it, vi } from "vitest";
import { goToCellScript, readSheetSelection, sheetCellScript } from "./sheet-cell";

// The pages host renders the sheet; it lives in nexus-pages-service. Set NEXUS_PAGES_SERVICE to a checkout
// of it to run this test against its page; unset, it is skipped.
const PAGES_SERVICE = process.env.NEXUS_PAGES_SERVICE;

it.skipIf(!PAGES_SERVICE)("selects a range in the active rendered tab and leaves it selected after an out-of-range jump", async () => {
  const { renderSheetPage } = await import(pathToFileURL(join(PAGES_SERVICE!, "eng/factory/services/frontend/core/pages-host/src/sheet.ts")).href);
  const html = new DOMParser().parseFromString(renderSheetPage(serializeSheet({ tabs: [
    { id: "early", name: "First", cells: { B2: { value: "Wrong tab" } } },
    { id: "later", name: "Second", cells: { B2: { value: "Right tab" } } },
  ] })).replace(/<link[^>]*>/g, ""), "text/html");
  document.head.replaceChildren(...html.head.children);
  document.body.replaceChildren(...html.body.children);
  const bridge = vi.fn<(message: string) => void>();
  vi.stubGlobal("ReactNativeWebView", { postMessage: bridge });
  const latest = () => readSheetSelection(JSON.parse(bridge.mock.lastCall![0]));
  const tab = document.querySelector<HTMLInputElement>("#tab-1")!;
  const target = document.querySelector<HTMLTableCellElement>("#pane-1 tbody tr:nth-child(2) td:nth-child(3)")!;
  const jump = vi.spyOn(target, "scrollIntoView");
  try {
    new Function(sheetCellScript)();
    expect(latest()).toEqual({ kind: "selected", address: "A1", value: "", jumped: false });
    document.querySelector<HTMLInputElement>("#tab-0")!.checked = false;
    tab.checked = true;
    tab.dispatchEvent(new Event("change", { bubbles: true }));
    new Function(goToCellScript(parseRange("C3:B2")!))();
    expect(latest()).toEqual({ kind: "selected", address: "B2", value: "Right tab", jumped: true });
    expect(jump).toHaveBeenCalledOnce();
    const selection = document.head.lastElementChild;
    expect(selection?.textContent).toContain("tr:nth-child(n+2):nth-child(-n+3) > td:nth-child(n+3):nth-child(-n+4)");
    new Function(goToCellScript(parseRange("C301")!))();
    expect(latest()).toEqual({ kind: "outside", boundary: "Z30" });
    expect(selection?.textContent).toContain("tr:nth-child(n+2):nth-child(-n+3)");
  } finally {
    vi.unstubAllGlobals();
    jump.mockRestore();
  }
});

it("rejects selections above the Sheet address limit", () => {
  expect(readSheetSelection({ kind: "nexus.sheet-selected", column: 703, row: 1, value: "" })).toBeUndefined();
});
