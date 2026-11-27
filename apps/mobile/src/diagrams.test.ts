import { describe, expect, it, vi } from "vitest";
import { diagramKey, diagramLabel, drawingOf, pendingDiagrams, requestDiagram, settleDiagram, subscribeDiagrams, type DiagramColors } from "./diagrams";

const colors: DiagramColors = { ground: "#efeff1", node: "#ffffff", border: "#71717b", line: "#6f6f79", text: "#27272a", muted: "#6f6f79" };

describe("the diagram queue", () => {
  it("queues a diagram once, until the renderer draws it, and then keeps the drawing", () => {
    const source = "flowchart LR\n  A --> B";
    const key = diagramKey(source, false);
    const changed = vi.fn();
    const stop = subscribeDiagrams(changed);
    requestDiagram({ key, source, colors });
    requestDiagram({ key, source, colors });
    const queued = pendingDiagrams();
    expect(queued.map((job) => job.key)).toEqual([key]);
    expect(changed).toHaveBeenCalledTimes(1);

    settleDiagram(key, { svg: "<svg/>", width: 120, height: 80 });
    expect(drawingOf(key)).toEqual({ svg: "<svg/>", width: 120, height: 80 });
    expect(pendingDiagrams()).toEqual([]);
    requestDiagram({ key, source, colors });
    expect(pendingDiagrams()).toEqual([]);
    expect(changed).toHaveBeenCalledTimes(2);
    stop();
  });

  it("draws a diagram again in the other appearance, and keeps one Mermaid can't read as failed", () => {
    const source = "sequenceDiagram\n  A->>B: hi";
    expect(diagramKey(source, true)).not.toBe(diagramKey(source, false));
    requestDiagram({ key: diagramKey(source, true), source, colors });
    settleDiagram(diagramKey(source, true), { failed: true });
    expect(drawingOf(diagramKey(source, true))).toEqual({ failed: true });
    expect(drawingOf(diagramKey(source, false))).toBeUndefined();
  });

  it("returns the same queue until it changes, as useSyncExternalStore needs", () => {
    expect(pendingDiagrams()).toBe(pendingDiagrams());
  });
});

describe("diagramLabel", () => {
  it("names the kind of diagram from its first statement, past front matter and comments", () => {
    expect(diagramLabel("flowchart TD\n  A --> B")).toBe("Flowchart");
    expect(diagramLabel("---\ntitle: Login\n---\n%% the steps\nsequenceDiagram\n  A->>B: hi")).toBe("Sequence");
    expect(diagramLabel("stateDiagram-v2\n  [*] --> On")).toBe("State diagram");
    expect(diagramLabel("quadrantChart")).toBe("Diagram");
  });
});
