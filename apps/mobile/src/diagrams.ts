/**
 * Mermaid diagrams on the phone: the queue that one shared renderer draws from (`mermaid-renderer.tsx`, a DOM
 * component: one web view for every diagram, never one each), and the drawings it sends back, as SVG that
 * react-native-svg draws. Kept in memory only, by appearance and source: nothing is stored on the device.
 */

/** The colours a diagram is drawn in, one appearance's. */
export interface DiagramColors {
  /** The card's ground, under the drawing. */
  ground: string;
  /** A node's fill. */
  node: string;
  /** A node's outline. */
  border: string;
  /** Lines and arrows. */
  line: string;
  text: string;
  /** Labels on lines, and notes. */
  muted: string;
}

/** A diagram for the renderer to draw. */
export interface DiagramJob {
  key: string;
  source: string;
  colors: DiagramColors;
}

/** A drawn diagram, its size in points, or one Mermaid could not read. */
export type Drawing = { svg: string; width: number; height: number } | { failed: true };

const drawings = new Map<string, Drawing>();
let jobs: DiagramJob[] = [];
const listeners = new Set<() => void>();
// A long chat's diagrams, in two appearances; past this the drawings start again.
const LIMIT = 64;

/** A diagram's key: its source, in the light or dark appearance. */
export const diagramKey = (source: string, dark: boolean) => `${dark ? "dark" : "light"}\n${source}`;

export const drawingOf = (key: string): Drawing | undefined => drawings.get(key);

/** The diagrams waiting for the renderer, oldest first. The same array until one is added or drawn. */
export const pendingDiagrams = (): DiagramJob[] => jobs;

export function subscribeDiagrams(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Queues a diagram, unless it is drawn or queued already. */
export function requestDiagram(job: DiagramJob) {
  if (drawings.has(job.key) || jobs.some((queued) => queued.key === job.key)) return;
  jobs = [...jobs, job];
  notify();
}

/** The renderer's answer for a queued diagram. */
export function settleDiagram(key: string, drawing: Drawing) {
  if (drawings.size >= LIMIT) drawings.clear();
  drawings.set(key, drawing);
  jobs = jobs.filter((job) => job.key !== key);
  notify();
}

function notify() {
  for (const listener of listeners) listener();
}

const KINDS: [RegExp, string][] = [
  [/^(flowchart|graph)\b/, "Flowchart"],
  [/^sequenceDiagram\b/, "Sequence"],
  [/^classDiagram/, "Class diagram"],
  [/^stateDiagram/, "State diagram"],
  [/^erDiagram\b/, "Data model"],
  [/^journey\b/, "Journey"],
  [/^gantt\b/, "Timeline"],
  [/^timeline\b/, "Timeline"],
  [/^mindmap\b/, "Mind map"],
  [/^pie\b/, "Chart"],
  [/^(architecture|block)/, "Architecture"],
  [/^gitGraph\b/, "Branches"],
];

/** The diagram's kind, for its card's head, from its first statement: Nexus UI's `diagramLabel`. "Diagram" otherwise. */
export function diagramLabel(source: string): string {
  const body = source.replace(/^\s*---[\s\S]*?\n---\s*\n/, "");
  const first = body
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line && !line.startsWith("%%"));
  return KINDS.find(([pattern]) => pattern.test(first ?? ""))?.[1] ?? "Diagram";
}
