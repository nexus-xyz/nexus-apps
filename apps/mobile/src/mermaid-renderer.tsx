"use dom";

import type { DOMProps } from "expo/dom";
import mermaid, { type MermaidConfig } from "mermaid";
import { useEffect } from "react";
import type { DiagramColors, DiagramJob } from "./diagrams";

type Drawn = { svg: string; width: number; height: number };

/**
 * The one renderer every Mermaid diagram on the phone is drawn by (`diagrams.ts`). A DOM component: Expo bundles
 * Mermaid into its web page, with no network, and the app mounts one, hidden, while a diagram waits
 * (`DiagramRenderer` in markdown.tsx). Mermaid lays each diagram out here, measuring its text in the system font that
 * react-native-svg draws it in. The styles Mermaid's stylesheet gives each shape are then written onto the shape,
 * because react-native-svg reads no stylesheet, and the SVG goes back to the app.
 */
export default function MermaidRenderer({ jobs, onDrawn }: { jobs: DiagramJob[]; onDrawn: (key: string, drawing: Drawn | null) => Promise<void>; dom?: DOMProps }) {
  useEffect(() => {
    for (const job of jobs) {
      if (queued.has(job.key)) continue;
      queued.add(job.key);
      queue = queue
        .then(async () => onDrawn(job.key, await draw(job)))
        .catch(() => undefined)
        .finally(() => queued.delete(job.key));
    }
  }, [jobs, onDrawn]);
  return null;
}

// The diagrams queued or being drawn, so one the app sends again while it waits is drawn once. A drawn one leaves:
// the app may ask for it again once its drawings start over (`diagrams.ts`).
const queued = new Set<string>();
// Mermaid draws one diagram at a time (it measures text in a scratch element of the page).
let queue: Promise<void> = Promise.resolve();
let drawings = 0;

async function draw({ source, colors }: DiagramJob): Promise<Drawn | null> {
  const id = `diagram-${++drawings}`;
  try {
    mermaid.initialize(config(colors));
    await mermaid.parse(source);
    const { svg } = await mermaid.render(id, source);
    return flatten(svg);
  } catch {
    return null;
  } finally {
    document.getElementById(`d${id}`)?.remove();
    document.getElementById(id)?.remove();
  }
}

const SHAPES = "path, rect, circle, ellipse, polygon, polyline, line, text, tspan";
// What Mermaid's stylesheet sets on a shape that react-native-svg can draw.
const PAINT = ["fill", "fill-opacity", "stroke", "stroke-width", "stroke-dasharray", "stroke-opacity", "opacity"];
const TYPE = ["font-size", "font-weight", "font-style"];

/** Mermaid's SVG with its stylesheet written onto each shape, and its size; null when there is nothing to draw. */
function flatten(markup: string): Drawn | null {
  const host = document.createElement("div");
  host.innerHTML = markup;
  document.body.append(host);
  try {
    const svg = host.querySelector("svg");
    const box = svg?.viewBox.baseVal;
    if (!svg || !box?.width || !box.height) return null;
    // A diagram's own colours on its nodes and groups (`style A fill:#f9f`) go, so the theme's apply, as on Nexus UI.
    for (const shape of svg.querySelectorAll(".node [style], .cluster [style]")) shape.removeAttribute("style");
    for (const shape of svg.querySelectorAll(SHAPES)) {
      const style = getComputedStyle(shape);
      const text = shape.tagName === "text";
      // A tspan (a line or a word of a label) gets only what differs from its text: its own `text-anchor` above all
      // would centre each word on the same point.
      const parent = shape.tagName === "tspan" && shape.parentElement ? getComputedStyle(shape.parentElement) : undefined;
      for (const name of text ? [...PAINT, ...TYPE, "text-anchor"] : parent ? [...PAINT, ...TYPE] : PAINT) {
        const value = style.getPropertyValue(name);
        if (!value || value === parent?.getPropertyValue(name) || (name === "stroke-dasharray" && value === "none")) continue;
        shape.setAttribute(name, value.replace(/(\d)px\b/g, "$1").replace(/url\("(#[^"]+)"\)/, "url($1)"));
      }
      // react-native-svg reads `alignment-baseline` and not `dominant-baseline`; CSS corners (`rx: 8px`) as attributes.
      const baseline = style.getPropertyValue("dominant-baseline");
      if (text && baseline && baseline !== "auto") shape.setAttribute("alignment-baseline", baseline);
      if (shape.tagName === "rect") for (const corner of ["rx", "ry"]) if (/^\d/.test(style.getPropertyValue(corner))) shape.setAttribute(corner, style.getPropertyValue(corner).replace("px", ""));
    }
    // react-native-svg lays out each word of a line (a tspan in a tspan) on its own, so they overlap: a line is one run.
    for (const line of svg.querySelectorAll("tspan")) if (line.querySelector("tspan")) line.textContent = line.textContent;
    for (const sheet of svg.querySelectorAll("style, foreignObject")) sheet.remove();
    // Mermaid puts its arrowheads in a group, and react-native-svg draws a marker where it stands unless it is in defs.
    const defs = document.createElementNS("http://www.w3.org/2000/svg", "defs");
    defs.append(...svg.querySelectorAll("marker"));
    svg.prepend(defs);
    for (const element of [svg, ...svg.querySelectorAll("*")]) for (const name of ["class", "style", "font-family"]) element.removeAttribute(name);
    return { svg: new XMLSerializer().serializeToString(svg), width: box.width, height: box.height };
  } finally {
    host.remove();
  }
}

/** The system font: react-native-svg draws text in it when a diagram names none. */
const FONT = "system-ui, -apple-system, sans-serif";

/**
 * Nexus UI's settings (`lib/diagrams/mermaid.ts`) in the app's colours. Safety as there: the strict security level (no
 * click handlers, labels encoded, the SVG through DOMPurify), labels as SVG text, and the settings in `secure`, which a
 * diagram's own `%%{init}%%` or front matter cannot change.
 */
function config(colors: DiagramColors): MermaidConfig {
  return {
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    htmlLabels: false,
    theme: "base",
    look: "classic",
    layout: "dagre",
    fontFamily: FONT,
    fontSize: 14,
    themeVariables: {
      fontFamily: FONT,
      fontSize: "14px",
      background: colors.ground,
      primaryColor: colors.node,
      primaryTextColor: colors.text,
      primaryBorderColor: colors.border,
      secondaryColor: colors.ground,
      tertiaryColor: colors.ground,
      lineColor: colors.line,
      textColor: colors.text,
      mainBkg: colors.node,
      nodeBorder: colors.border,
      clusterBkg: colors.ground,
      clusterBorder: colors.border,
      edgeLabelBackground: colors.ground,
      noteBkgColor: colors.ground,
      noteBorderColor: colors.border,
      noteTextColor: colors.muted,
      actorBkg: colors.node,
      actorBorder: colors.border,
      actorTextColor: colors.text,
      actorLineColor: colors.border,
      signalColor: colors.line,
      signalTextColor: colors.text,
      labelBoxBkgColor: colors.ground,
      labelBoxBorderColor: colors.border,
      labelTextColor: colors.muted,
      loopTextColor: colors.muted,
      activationBkgColor: colors.ground,
      activationBorderColor: colors.border,
      sequenceNumberColor: colors.node,
    },
    themeCSS: ".node rect, .node .basic.label-container, .cluster rect, rect.actor { rx: 8px; ry: 8px; }",
    secure: ["secure", "securityLevel", "startOnLoad", "maxTextSize", "suppressErrorRendering", "maxEdges", "theme", "themeVariables", "themeCSS", "fontFamily", "altFontFamily", "fontSize", "htmlLabels", "look", "darkMode"],
    maxTextSize: 50_000,
    maxEdges: 500,
    flowchart: { htmlLabels: false, curve: "basis", padding: 14, nodeSpacing: 36, rankSpacing: 44, diagramPadding: 12, useMaxWidth: false, wrappingWidth: 200 },
    sequence: { useMaxWidth: false, diagramMarginX: 12, diagramMarginY: 12, actorMargin: 48, width: 128, height: 44, boxMargin: 10, noteMargin: 12, messageMargin: 36, mirrorActors: false },
    state: { useMaxWidth: false },
    class: { useMaxWidth: false, htmlLabels: false },
    er: { useMaxWidth: false },
  };
}
