/**
 * How an answer's Markdown is read, for every client that draws one: Nexus UI's `Markdown` (React DOM) and the phone
 * app's (React Native) render these blocks and spans in their own elements, so an answer reads alike on both. Plain
 * module, no rendering: headings, paragraphs, ordered, unordered, nested and task lists, quotes, rules, fenced code
 * with its language, display math, tables; inline code, bold, italics (`_x_` or `*x*`), strikethrough, links, images,
 * math and `[n]` citations. Links and images come out as written: each renderer decides what it may open, and none
 * loads an image on its own (a model can be led to write an image URL that carries the chat to someone else's
 * server; the renderers show it as a link to tap). A fence an answer is still streaming is a code block with
 * `closed: false`.
 */

export type MarkdownAlign = "left" | "center" | "right" | undefined;

export interface MarkdownListItem {
  text: string;
  /** A task list item (`- [ ]`, `- [x]`): whether it is checked. Absent on other items. */
  checked?: boolean;
  /** A list indented under this item. */
  children?: MarkdownList;
}

export interface MarkdownList {
  ordered: boolean;
  /** The first item's number, for an ordered list. */
  start: number;
  items: MarkdownListItem[];
}

export type MarkdownBlock =
  | { kind: "heading"; level: number; text: string }
  | { kind: "paragraph"; text: string }
  | ({ kind: "list" } & MarkdownList)
  | { kind: "quote"; text: string }
  | { kind: "rule" }
  | { kind: "code"; language?: string; code: string; closed: boolean }
  | { kind: "math"; tex: string }
  | { kind: "table"; head: string[]; align: MarkdownAlign[]; rows: string[][] };

export type MarkdownInline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "math"; text: string }
  | { kind: "strong" | "em" | "strike"; children: MarkdownInline[] }
  | { kind: "link"; label: string; href: string }
  | { kind: "image"; alt: string; src: string }
  | { kind: "cite"; n: number };

const isTableRow = (line: string) => /^\s*\|.*\|\s*$/.test(line);
const isTableRule = (line: string) => /^\s*\|(\s*:?-+:?\s*\|)+\s*$/.test(line);
const cells = (line: string) =>
  line
    .trim()
    .replace(/^\||\|$/g, "")
    .split("|")
    .map((cell) => cell.trim());

/** `---`, `***` or `___` (spaces allowed between), alone on its line. */
const RULE = /^\s{0,3}([-*_])(?:\s*\1){2,}\s*$/;
/** A list line: its indent, its marker and its text. */
const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const indentOf = (spaces: string) => spaces.replace(/\t/g, "    ").length;

/**
 * Dependent: the desktop's streaming view (`splitBlocks` in `services/frontend/core/desktop/src/workbench/streaming-prose.tsx`)
 * cuts an answer at blank lines outside code fences and `$$` / `\[` formulas, copying the openers below. A block that
 * can span a blank line (loose lists, `~~~` fences, indented code) needs the same rule there.
 */
export function parseMarkdownBlocks(source: string): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  const lines = source.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (isTableRow(line) && isTableRule(lines[i + 1] ?? "")) {
      const head = cells(line);
      // The rule row's colons set each column's alignment (`:--` left, `:-:` centre, `--:` right), as on GitHub.
      const align = cells(lines[i + 1] ?? "").map((rule): MarkdownAlign => (/^:-+:$/.test(rule) ? "center" : /-:$/.test(rule) ? "right" : undefined));
      const rows: string[][] = [];
      for (i += 2; i < lines.length && isTableRow(lines[i] ?? ""); i++) rows.push(cells(lines[i] ?? ""));
      i--;
      blocks.push({ kind: "table", head, align, rows });
      continue;
    }
    const fence = /^\s*```\s*([\w+#.-]*)/.exec(line);
    const mathOpen = /^\s*(\$\$|\\\[)/.exec(line);
    if (mathOpen) {
      // Display math: `$$ … $$` or `\[ … \]`, on one line or across several (until the closing marker, or the end
      // while an answer is still streaming).
      const close = mathOpen[1] === "$$" ? "$$" : "\\]";
      const first = line.trim().slice(2);
      const tex: string[] = [];
      if (first.includes(close)) tex.push(first.slice(0, first.indexOf(close)));
      else {
        if (first) tex.push(first);
        while (++i < lines.length) {
          const next = lines[i] ?? "";
          const at = next.indexOf(close);
          if (at >= 0) {
            tex.push(next.slice(0, at));
            break;
          }
          tex.push(next);
        }
      }
      blocks.push({ kind: "math", tex: tex.join("\n") });
    } else if (fence) {
      const code: string[] = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i] ?? "")) code.push(lines[i] ?? "");
      const closed = i < lines.length;
      if (!closed) i = lines.length - 1;
      if (fence[1] === "math") blocks.push({ kind: "math", tex: code.join("\n") });
      else blocks.push({ kind: "code", language: fence[1] || undefined, code: code.join("\n"), closed });
    } else if (/^#{1,4} /.test(line)) {
      const level = line.indexOf(" ");
      blocks.push({ kind: "heading", level, text: line.slice(level + 1) });
    } else if (RULE.test(line)) {
      blocks.push({ kind: "rule" });
    } else if (LIST_ITEM.test(line)) {
      const end = listEnd(lines, i);
      blocks.push({ kind: "list", ...list(lines.slice(i, end)) });
      i = end - 1;
    } else if (/^> ?/.test(line)) {
      const quote: string[] = [];
      for (; i < lines.length && /^> ?/.test(lines[i] ?? ""); i++) quote.push((lines[i] ?? "").replace(/^> ?/, ""));
      i--;
      blocks.push({ kind: "quote", text: quote.join(" ") });
    } else if (line.trim()) {
      blocks.push({ kind: "paragraph", text: line });
    }
  }
  return blocks;
}

/**
 * Where a list starting at `start` ends: at the first line that is not one of its items. A line of the other kind
 * (`1.` after `-`) at the top level starts a list of its own, as Nexus UI drew them; indented, it nests.
 */
function listEnd(lines: string[], start: number): number {
  const first = LIST_ITEM.exec(lines[start] ?? "");
  const base = indentOf(first?.[1] ?? "");
  const ordered = /\d/.test(first?.[2] ?? "");
  let i = start + 1;
  for (; i < lines.length; i++) {
    const item = LIST_ITEM.exec(lines[i] ?? "");
    if (!item || RULE.test(lines[i] ?? "")) break;
    if (indentOf(item[1] ?? "") <= base && /\d/.test(item[2] ?? "") !== ordered) break;
  }
  return i;
}

/** List lines into a list: each line indented past the item above it nests under that item. */
function list(lines: string[]): MarkdownList {
  const entries = lines.map((line) => {
    const [, spaces = "", marker = "", rest = ""] = LIST_ITEM.exec(line) ?? [];
    const task = /^\[([ xX])\]\s+(.*)$/.exec(rest);
    return { indent: indentOf(spaces), ordered: /\d/.test(marker), number: Number.parseInt(marker, 10), text: task ? (task[2] ?? "") : rest, checked: task ? task[1] !== " " : undefined };
  });
  let at = 0;
  const level = (indent: number): MarkdownList => {
    const head = entries[at];
    const out: MarkdownList = { ordered: head?.ordered ?? false, start: head?.ordered ? head.number : 1, items: [] };
    while (at < entries.length) {
      const entry = entries[at];
      if (!entry || entry.indent < indent) break;
      if (entry.indent > indent && out.items.length) {
        const parent = out.items[out.items.length - 1];
        if (parent) parent.children = level(entry.indent);
        continue;
      }
      out.items.push({ text: entry.text, ...(entry.checked === undefined ? {} : { checked: entry.checked }) });
      at++;
    }
    return out;
  };
  return level(entries[0]?.indent ?? 0);
}

/**
 * Inline math: `\(…\)`, or `$…$` with no space just inside the dollars and no digit right after the closing one, so
 * prices ("$5 and $10") stay text.
 */
const INLINE_MATH = String.raw`\\\((?:[^\\]|\\(?!\)))+?\\\)|\$(?=[^\s$])(?:[^$\n\\\x60]|\\.)+?(?<=[^\s\\])\$(?!\d)`;
/** Bold and the two italics may hold code spans; a single `*` needs a non-space just inside it, so `2 * 3` stays text. */
const INLINE = new RegExp(
  String.raw`(\`[^\`]+\`|${INLINE_MATH}|\*\*(?:[^*\`]|\`[^\`]+\`)+\*\*|~~(?=\S)(?:[^~\`]|\`[^\`]+\`)+?~~|(?<!\w)_(?=[^_\s])(?:[^_\`]|\`[^\`]+\`)+?(?<=[^_\s])_(?!\w)|\*(?=[^*\s])(?:[^*\`]|\`[^\`]+\`)+?(?<=[^*\s])\*|!\[[^\]]*\]\([^)\s]+\)|\[[^\]]+\]\([^)\s]+\)|\[\d+\])`,
);

export function parseMarkdownInline(text: string): MarkdownInline[] {
  return text
    .split(INLINE)
    .filter(Boolean)
    .map((part): MarkdownInline => {
      if (/^`.+`$/.test(part)) return { kind: "code", text: part.slice(1, -1) };
      if (/^\\\([\s\S]*\\\)$/.test(part)) return { kind: "math", text: part.slice(2, -2) };
      if (/^\$[\s\S]+\$$/.test(part)) return { kind: "math", text: part.slice(1, -1) };
      if (/^\*\*.+\*\*$/.test(part)) return { kind: "strong", children: parseMarkdownInline(part.slice(2, -2)) };
      if (/^~~.+~~$/.test(part)) return { kind: "strike", children: parseMarkdownInline(part.slice(2, -2)) };
      if (/^_.+_$/.test(part) || /^\*.+\*$/.test(part)) return { kind: "em", children: parseMarkdownInline(part.slice(1, -1)) };
      const image = /^!\[([^\]]*)\]\(([^)\s]+)\)$/.exec(part);
      if (image) return { kind: "image", alt: image[1] ?? "", src: image[2] ?? "" };
      const link = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(part);
      if (link) return { kind: "link", label: link[1] ?? "", href: link[2] ?? "" };
      const cite = /^\[(\d+)\]$/.exec(part);
      if (cite) return { kind: "cite", n: Number(cite[1]) };
      return { kind: "text", text: part };
    });
}
