export type DocHeading = { index: number; title: string; level: number };
export type DocOutline = { headings: DocHeading[]; active: number };

const KIND = "nexus.doc-contents";
const HEADINGS = "h1,h2,h3,h4,h5,h6";
const MAX_HEADINGS = 200;
const MAX_TITLE = 200;

export const docOutlineScript = `(() => {
  if (window.__nexusDocContents) return;
  window.__nexusDocContents = true;
  const nodes = [...document.querySelectorAll(${JSON.stringify(HEADINGS)})].slice(0, ${MAX_HEADINGS});
  const headings = nodes.flatMap((node, index) => {
    const title = (node.textContent || "").trim().slice(0, ${MAX_TITLE});
    return title ? [{ index, title, level: Number(node.tagName[1]) }] : [];
  });
  let active = -2;
  let scheduled = false;
  function update() {
    scheduled = false;
    let next = -1;
    for (const heading of headings) {
      if (nodes[heading.index].getBoundingClientRect().top <= 24) next = heading.index;
      else break;
    }
    if (next === active) return;
    active = next;
    window.ReactNativeWebView.postMessage(JSON.stringify({ kind: "${KIND}", headings, active }));
  }
  document.addEventListener("scroll", () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(update);
  }, { passive: true, capture: true });
  update();
})(); true;`;

export function jumpToDocHeading(index: number): string {
  if (!Number.isInteger(index) || index < 0 || index >= MAX_HEADINGS) return "true;";
  return `document.querySelectorAll(${JSON.stringify(HEADINGS)})[${index}]?.scrollIntoView({ block: "start" }); true;`;
}

export function readDocOutline(data: unknown): DocOutline | undefined {
  if (!data || typeof data !== "object" || !("kind" in data) || data.kind !== KIND ||
      !("headings" in data) || !Array.isArray(data.headings) || data.headings.length > MAX_HEADINGS ||
      !("active" in data) || typeof data.active !== "number") return;
  const headings: DocHeading[] = [];
  const rows: unknown[] = data.headings;
  for (const value of rows) {
    if (!value || typeof value !== "object" ||
        !("index" in value) || typeof value.index !== "number" || !Number.isInteger(value.index) ||
        value.index < 0 || value.index >= MAX_HEADINGS || value.index <= (headings.at(-1)?.index ?? -1) ||
        !("title" in value) || typeof value.title !== "string" || !value.title.trim() || value.title.length > MAX_TITLE ||
        !("level" in value) || typeof value.level !== "number" || !Number.isInteger(value.level) ||
        value.level < 1 || value.level > 6) return;
    headings.push({ index: value.index, title: value.title, level: value.level });
  }
  const active = data.active;
  if (active !== -1 && !headings.some(({ index }) => index === active)) return;
  return { headings, active };
}
