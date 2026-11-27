type OutlineLayer = { name: string; depth: number };
type OutlineBoard = { name: string; selector: string; layers: OutlineLayer[] };
type OutlinePage = { name: string; boards: OutlineBoard[] };

export type DesignOutline = { active: number; pages: OutlinePage[] };
export type DesignFocus = { page: number; board?: number; layer?: number };

const label = (value: unknown) => typeof value === "string" && value.length <= 200;

export function readDesignOutline(data: unknown): DesignOutline | undefined {
  if (!data || typeof data !== "object" || !("kind" in data) || data.kind !== "nexus.design-outline") return;
  const { active, pages } = data as Partial<DesignOutline>;
  if (typeof active !== "number" || !Number.isInteger(active) || !Array.isArray(pages) || active < 0 || active >= pages.length ||
      pages.length > 80) return;
  let boardsLeft = 120;
  let layersLeft = 500;
  for (const page of pages) {
    if (!page || !label(page.name) || !Array.isArray(page.boards) || page.boards.length > 120) return;
    boardsLeft -= page.boards.length;
    if (boardsLeft < 0) return;
    for (const board of page.boards) {
      if (!board || !label(board.name) || !label(board.selector) || !Array.isArray(board.layers) || board.layers.length > 80) return;
      layersLeft -= board.layers.length;
      if (layersLeft < 0) return;
      if (board.layers.some((layer: OutlineLayer) => !layer || !label(layer.name) || !Number.isInteger(layer.depth) ||
          layer.depth < 0 || layer.depth > 8)) return;
    }
  }
  return { active, pages };
}

export const designOutlineScript = `(() => {
  if (window.nexusMobileDesignOutline) return;
  window.nexusMobileDesignOutline = true;
  let previous = "";
  const top = (scope, selector) => Array.from(scope.querySelectorAll(selector)).filter((node) => !node.parentElement?.closest(selector));
  const title = (node, attr, fallback) => (node.getAttribute(attr) || "").trim().slice(0, 200) || fallback;

  function report() {
    const all = top(document, "[data-artboard]");
    if (!all.length) return;
    const nodes = top(document, "[data-page]").slice(0, 80);
    const active = Math.max(0, nodes.findIndex((node) => !node.hasAttribute("data-nx-page-hidden")));
    let boardsLeft = 120;
    let layersLeft = 500;
    const pages = (nodes.length ? nodes : [document]).map((node) => {
      const boards = top(node, "[data-artboard]").slice(0, boardsLeft);
      boardsLeft -= boards.length;
      return {
        name: node === document ? "Canvas" : title(node, "data-page", "Page"),
        boards: boards.map((board) => {
          const layers = Array.from(board.querySelectorAll("[data-layer]")).slice(0, Math.min(80, layersLeft));
          layersLeft -= layers.length;
          const number = all.indexOf(board) + 1;
          return {
            name: title(board, "data-artboard", "Artboard " + number),
            selector: title(board, "data-id", String(number)),
            layers: layers.map((layer) => {
              let depth = 0;
              for (let parent = layer.parentElement; parent && parent !== board; parent = parent.parentElement) {
                if (parent.hasAttribute("data-layer")) depth++;
              }
              return { name: title(layer, "data-layer", layer.tagName.toLowerCase()), depth: Math.min(8, depth) };
            }),
          };
        }),
      };
    });
    const data = JSON.stringify({ kind: "nexus.design-outline", active, pages });
    if (previous === data) return;
    previous = data;
    window.ReactNativeWebView.postMessage(data);
  }

  report();
  let update;
  new MutationObserver(() => {
    clearTimeout(update);
    update = setTimeout(report, 75);
  }).observe(document.body, { subtree: true, childList: true, attributes: true,
    attributeFilter: ["data-page", "data-artboard", "data-layer", "data-id", "data-nx-page-hidden"] });
})(); true;`;

export function designFocusScript(focus: DesignFocus): string {
  return `window.dispatchEvent(new CustomEvent("nexus-mobile-design-focus", { detail: ${JSON.stringify(focus)} })); true;`;
}
