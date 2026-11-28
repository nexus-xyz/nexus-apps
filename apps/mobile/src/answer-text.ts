import { parseMarkdownBlocks, parseMarkdownInline, type MarkdownInline, type MarkdownList } from "@nexus/kit";

function inlineText(parts: MarkdownInline[]): string {
  return parts.map((part): string => {
    switch (part.kind) {
      case "text": case "code": case "math": return part.text;
      case "strong": case "em": case "strike": return inlineText(part.children);
      case "link": return inlineText(parseMarkdownInline(part.label));
      case "image": return `Image: ${part.alt || part.src}`;
      case "cite": return `[${part.n}]`;
      default: { const exhaustive: never = part; return exhaustive; }
    }
  }).join("");
}

const words = (source: string) => inlineText(parseMarkdownInline(source));

function listText(list: MarkdownList, depth = 0): string {
  return list.items.map((item, index) => {
    const marker = item.checked === undefined ? (list.ordered ? `${list.start + index}.` : "•") : item.checked ? "☑" : "☐";
    return `${"  ".repeat(depth)}${marker} ${words(item.text)}${item.children ? `\n${listText(item.children, depth + 1)}` : ""}`;
  }).join("\n");
}

/** The response's displayed words for copying, selecting and sharing. Code and math stay literal. */
export function answerText(source: string): string {
  return parseMarkdownBlocks(source).map((block): string => {
    switch (block.kind) {
      case "heading": case "paragraph": case "quote": return words(block.text);
      case "list": return listText(block);
      case "code": return block.code;
      case "math": return block.tex;
      case "table": return [block.head, ...block.rows].map((row) => row.map(words).join("\t")).join("\n");
      case "rule": return "";
      default: { const exhaustive: never = block; return exhaustive; }
    }
  }).filter(Boolean).join("\n\n");
}

/** Native speech limits count UTF-16 units. Prefer word boundaries and never split a surrogate pair. */
export function speechChunks(source: string, limit: number): string[] {
  if (!Number.isInteger(limit) || limit < 2) throw new RangeError("Speech input limit must be at least two characters.");
  const chunks: string[] = [];
  let remaining = source.trim();
  while (remaining.length > limit) {
    let end = limit;
    const boundary = remaining.lastIndexOf(" ", end);
    if (boundary > limit / 2) end = boundary;
    if (/[\uD800-\uDBFF]/u.test(remaining[end - 1] ?? "")) end -= 1;
    chunks.push(remaining.slice(0, end));
    remaining = remaining.slice(end).trimStart();
  }
  if (remaining) chunks.push(remaining);
  return chunks;
}
