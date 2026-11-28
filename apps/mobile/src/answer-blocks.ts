import { parseMarkdownBlocks, type MarkdownBlock } from "@nexus/kit";

/**
 * An answer as the phone draws it: @nexus/kit's Markdown blocks, display math among them (`$$…$$`, `\[…\]` and ```math
 * fences), with each closed ```mermaid fence as a diagram. A fence the answer is still streaming stays code until it
 * closes, so a diagram is drawn once, from its whole source, as Nexus UI draws it.
 */
export type AnswerBlock = MarkdownBlock | { kind: "diagram"; source: string };

export function answerBlocks(source: string): AnswerBlock[] {
  return parseMarkdownBlocks(source).map((block) => (block.kind === "code" && block.closed && isMermaidFence(block.language) ? { kind: "diagram", source: block.code } : block));
}

/** Whether a fence's language asks for a Mermaid diagram (```mermaid or ```mmd), as Nexus UI's `isMermaidFence`. */
export function isMermaidFence(language: string | undefined): boolean {
  return /^(mermaid|mmd)$/i.test(language?.trim() ?? "");
}
