import { liteAdaptor } from "mathjax-full/js/adaptors/liteAdaptor.js";
import { RegisterHTMLHandler } from "mathjax-full/js/handlers/html.js";
import { TeX } from "mathjax-full/js/input/tex.js";
import { AllPackages } from "mathjax-full/js/input/tex/AllPackages.js";
import { mathjax } from "mathjax-full/js/mathjax.js";
import { SVG } from "mathjax-full/js/output/svg.js";

/**
 * A formula drawn as SVG paths in `currentColor`, and its size in ems of the text it sits in: `depth` is how far it
 * reaches below the baseline.
 */
export interface Formula {
  svg: string;
  width: number;
  height: number;
  depth: number;
}

// MathJax, all in JavaScript: TeX to SVG paths here, drawn by react-native-svg, with no web view and no network. Every
// TeX package but `html` (\href and \style, which nothing on a phone draws) and `noerrors`, so TeX that MathJax cannot
// read throws, and the answer shows its source instead, as Nexus UI does.
let toSvg: ((tex: string, display: boolean) => string) | undefined;

function setUp() {
  const adaptor = liteAdaptor();
  RegisterHTMLHandler(adaptor);
  const packages = AllPackages.filter((name) => name !== "html" && name !== "noerrors");
  const formatError = (_jax: unknown, error: Error): never => {
    throw error;
  };
  const doc = mathjax.document("", { InputJax: new TeX({ packages, formatError }), OutputJax: new SVG({ fontCache: "none" }) });
  return (tex: string, display: boolean) => adaptor.innerHTML(doc.convert(tex, { display }));
}

// Drawn formulas, in memory only. A streaming formula adds one per partial source, so past the limit it starts again.
const drawn = new Map<string, Formula | null>();
const LIMIT = 256;
// MathJax draws on the JS thread, and a streaming formula is drawn again at each token: longer TeX shows as its source.
// 2,000 characters took 43 ms in Node; Hermes on a phone is slower. MathJax's own limits (`maxMacros`, `maxBuffer`)
// stop TeX that expands without end.
const MAX_TEX = 2_000;

/** `tex` as a display formula or an inline one, or undefined when it is not TeX that MathJax can draw. */
export function formula(tex: string, display: boolean): Formula | undefined {
  if (tex.length > MAX_TEX) return undefined;
  const key = `${display ? "display" : "inline"}\n${tex}`;
  let hit = drawn.get(key);
  if (hit === undefined) {
    hit = draw(tex, display);
    if (drawn.size >= LIMIT) drawn.clear();
    drawn.set(key, hit);
  }
  return hit ?? undefined;
}

function draw(tex: string, display: boolean): Formula | null {
  try {
    toSvg ??= setUp();
    const markup = toSvg(tex, display);
    // MathJax's units are thousandths of an em; the view box starts at the top, above the baseline.
    const [, top = Number.NaN, width = Number.NaN, height = Number.NaN] = /viewBox="([^"]+)"/.exec(markup)?.[1]?.split(/\s+/).map(Number) ?? [];
    if (![top, width, height].every(Number.isFinite)) return null;
    // Its width, height and style (`vertical-align`) are in ex, which react-native-svg does not read: the view sizes it.
    const svg = markup.replace(/^<svg([^>]*)>/, (_, attributes: string) => `<svg${attributes.replace(/\s(?:style|width|height)="[^"]*"/g, "")}>`);
    return { svg, width: width / 1000, height: height / 1000, depth: (height + top) / 1000 };
  } catch {
    return null;
  }
}
