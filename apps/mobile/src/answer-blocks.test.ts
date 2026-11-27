import { describe, expect, it } from "vitest";
import { answerBlocks, isMermaidFence } from "./answer-blocks";

describe("answerBlocks", () => {
  it("splits an answer into Markdown, display math and diagrams", () => {
    const answer = [
      "The energy, $E$:",
      "$$ E = mc^2 $$",
      "\\[",
      "a^2 + b^2 = c^2",
      "\\]",
      "```math",
      "\\int_0^1 x\\,dx",
      "```",
      "The flow:",
      "```mermaid",
      "flowchart LR",
      "  A --> B",
      "```",
      "```ts",
      "const x = 1;",
      "```",
    ].join("\n");
    expect(answerBlocks(answer)).toEqual([
      { kind: "paragraph", text: "The energy, $E$:" },
      { kind: "math", tex: " E = mc^2 " },
      { kind: "math", tex: "a^2 + b^2 = c^2\n" },
      { kind: "math", tex: "\\int_0^1 x\\,dx" },
      { kind: "paragraph", text: "The flow:" },
      { kind: "diagram", source: "flowchart LR\n  A --> B" },
      { kind: "code", language: "ts", code: "const x = 1;", closed: true },
    ]);
  });

  it("keeps a diagram the answer is still streaming as code, until its fence closes", () => {
    expect(answerBlocks("```mermaid\nflowchart LR\n  A --")).toEqual([{ kind: "code", language: "mermaid", code: "flowchart LR\n  A --", closed: false }]);
  });

  it("reads ```mmd as a diagram, in any case, and nothing else", () => {
    expect(answerBlocks("```MMD\ngraph TD\n```")).toEqual([{ kind: "diagram", source: "graph TD" }]);
    expect(["mermaid", "Mermaid", " mmd "].every(isMermaidFence)).toBe(true);
    expect([undefined, "", "mermaidjs", "math"].some(isMermaidFence)).toBe(false);
  });
});
