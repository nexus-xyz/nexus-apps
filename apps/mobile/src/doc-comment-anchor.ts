export type DocCommentAnchor = { quote: string; prefix?: string; suffix?: string };

export function readDocCommentAnchor(data: unknown): DocCommentAnchor | undefined {
  if (!data || typeof data !== "object" || !("kind" in data) || data.kind !== "nexus.doc-comment-anchor") return;
  const { quote, prefix, suffix } = data as Partial<DocCommentAnchor>;
  if (typeof quote !== "string" || !quote.trim() || quote.length > 2_000 || quote.includes("\0")) return;
  return {
    quote,
    ...(typeof prefix === "string" && prefix.length <= 200 && !prefix.includes("\0") ? { prefix } : {}),
    ...(typeof suffix === "string" && suffix.length <= 200 && !suffix.includes("\0") ? { suffix } : {}),
  };
}

export const docCommentScript = `(() => {
  if (window.nexusMobileDocComment) return;
  const doc = document.querySelector("main.page-doc");
  if (!doc) return;
  let previous = "";
  document.addEventListener("selectionchange", () => {
    const selection = window.getSelection();
    if (!selection?.rangeCount || selection.isCollapsed) { previous = ""; return; }
    const range = selection.getRangeAt(0);
    if (!doc.contains(range.commonAncestorContainer)) return;
    const quote = range.toString();
    if (!quote.trim() || quote.length > 2000) return;
    const before = document.createRange();
    before.selectNodeContents(doc);
    before.setEnd(range.startContainer, range.startOffset);
    const after = document.createRange();
    after.selectNodeContents(doc);
    after.setStart(range.endContainer, range.endOffset);
    const data = JSON.stringify({
      kind: "nexus.doc-comment-anchor", quote, prefix: before.toString().slice(-200), suffix: after.toString().slice(0, 200)
    });
    if (previous === data) return;
    previous = data;
    window.ReactNativeWebView.postMessage(data);
  });
  window.nexusMobileDocComment = (anchor) => {
    const all = document.createTreeWalker(doc, NodeFilter.SHOW_TEXT);
    const nodes = [];
    let text = "";
    while (all.nextNode()) {
      const node = all.currentNode;
      const value = node.textContent || "";
      if (!value) continue;
      nodes.push({ node, index: text.length });
      text += value;
    }
    let first = -1, chosen = -1;
    for (let at = text.indexOf(anchor.quote); at !== -1; at = text.indexOf(anchor.quote, at + anchor.quote.length)) {
      if (first === -1) first = at;
      if ((!anchor.prefix || text.slice(0, at).endsWith(anchor.prefix)) &&
          (!anchor.suffix || text.startsWith(anchor.suffix, at + anchor.quote.length))) { chosen = at; break; }
    }
    const from = chosen === -1 ? first : chosen;
    if (from === -1) return;
    const until = from + anchor.quote.length;
    const start = nodes.findLast((part) => part.index <= from);
    const end = nodes.findLast((part) => part.index < until);
    if (!start || !end) return;
    const range = document.createRange();
    range.setStart(start.node, from - start.index);
    range.setEnd(end.node, until - end.index);
    window.scrollBy({ top: range.getBoundingClientRect().top - window.innerHeight / 3, behavior: "instant" });
    if (window.Highlight && CSS.highlights) CSS.highlights.set("nexus-mobile-comment", new Highlight(range));
  };
  const css = document.createElement("style");
  css.textContent = "::highlight(nexus-mobile-comment) { background-color: var(--nx-fill, Highlight); color: HighlightText; }";
  document.head.append(css);
})(); true;`;

export function locateDocComment(anchor: DocCommentAnchor): string {
  return `window.nexusMobileDocComment?.(${JSON.stringify(anchor)}); true;`;
}
