import { describe, expect, it } from "vitest";
import { parseMarkdownBlocks, parseMarkdownInline } from "./markdown-parse";

describe("parseMarkdownBlocks", () => {
  it("reads the blocks an answer is made of", () => {
    const source = [
      "## Plan",
      "First line.",
      "- one",
      "* two",
      "3. three",
      "4) four",
      "> a quote",
      "> goes on",
      "```ts",
      "const x = 1;",
      "```",
      "| a | b |",
      "|:-:|--:|",
      "| 1 | 2 |",
      "$$ e = mc^2 $$",
    ].join("\n");
    expect(parseMarkdownBlocks(source)).toEqual([
      { kind: "heading", level: 2, text: "Plan" },
      { kind: "paragraph", text: "First line." },
      { kind: "list", ordered: false, start: 1, items: [{ text: "one" }, { text: "two" }] },
      { kind: "list", ordered: true, start: 3, items: [{ text: "three" }, { text: "four" }] },
      { kind: "quote", text: "a quote goes on" },
      { kind: "code", language: "ts", code: "const x = 1;", closed: true },
      { kind: "table", head: ["a", "b"], align: ["center", "right"], rows: [["1", "2"]] },
      { kind: "math", tex: " e = mc^2 " },
    ]);
  });

  it("keeps a fence the stream has not closed yet, as code", () => {
    expect(parseMarkdownBlocks("Here:\n```py\nprint(1)")).toEqual([
      { kind: "paragraph", text: "Here:" },
      { kind: "code", language: "py", code: "print(1)", closed: false },
    ]);
  });

  it("reads rules: ---, *** and ___", () => {
    expect(parseMarkdownBlocks("above\n---\nbelow\n* * *\n___")).toEqual([
      { kind: "paragraph", text: "above" },
      { kind: "rule" },
      { kind: "paragraph", text: "below" },
      { kind: "rule" },
      { kind: "rule" },
    ]);
  });

  it("nests indented items under the item above, of either kind", () => {
    const [list] = parseMarkdownBlocks("- Item one\n  - Nested item\n    1. Deeper\n  - Another nested item\n- Item two");
    expect(list).toEqual({
      kind: "list",
      ordered: false,
      start: 1,
      items: [
        {
          text: "Item one",
          children: {
            ordered: false,
            start: 1,
            items: [{ text: "Nested item", children: { ordered: true, start: 1, items: [{ text: "Deeper" }] } }, { text: "Another nested item" }],
          },
        },
        { text: "Item two" },
      ],
    });
  });

  it("reads task lists, checked and not", () => {
    const [list] = parseMarkdownBlocks("- [x] Open Nexus on mobile\n- [ ] Send a test message\n- plain");
    expect(list).toMatchObject({ items: [{ text: "Open Nexus on mobile", checked: true }, { text: "Send a test message", checked: false }, { text: "plain" }] });
  });

  it("reads an empty item, and stays fast on a long line of spaces that is not an item", () => {
    expect(parseMarkdownBlocks("- one\n-   ")).toMatchObject([{ kind: "list", items: [{ text: "one" }, { text: "" }] }]);
    const line = `* ${" ".repeat(50_000)}x\ry`;
    const started = performance.now();
    expect(parseMarkdownBlocks(line)).toEqual([{ kind: "paragraph", text: line }]);
    expect(performance.now() - started).toBeLessThan(1_000);
  });
});

describe("parseMarkdownInline", () => {
  it("reads code, bold holding code, both italics, strikethrough, math and links", () => {
    expect(parseMarkdownInline("Run `npm i`, **set `X`**, _go_, *now*, ~~not this~~: $x^2$ [docs](https://nexus.xyz)")).toEqual([
      { kind: "text", text: "Run " },
      { kind: "code", text: "npm i" },
      { kind: "text", text: ", " },
      { kind: "strong", children: [{ kind: "text", text: "set " }, { kind: "code", text: "X" }] },
      { kind: "text", text: ", " },
      { kind: "em", children: [{ kind: "text", text: "go" }] },
      { kind: "text", text: ", " },
      { kind: "em", children: [{ kind: "text", text: "now" }] },
      { kind: "text", text: ", " },
      { kind: "strike", children: [{ kind: "text", text: "not this" }] },
      { kind: "text", text: ": " },
      { kind: "math", text: "x^2" },
      { kind: "text", text: " " },
      { kind: "link", label: "docs", href: "https://nexus.xyz" },
    ]);
  });

  it("reads images, which are never mistaken for a link after a stray !", () => {
    expect(parseMarkdownInline("See ![Placeholder image](https://via.placeholder.com/150) here")).toEqual([
      { kind: "text", text: "See " },
      { kind: "image", alt: "Placeholder image", src: "https://via.placeholder.com/150" },
      { kind: "text", text: " here" },
    ]);
  });

  it("reads [n] citations", () => {
    expect(parseMarkdownInline("Sessions expire [2].")).toEqual([
      { kind: "text", text: "Sessions expire " },
      { kind: "cite", n: 2 },
      { kind: "text", text: "." },
    ]);
  });

  it("leaves prices, a lone star and a spaced product as text, and links as written", () => {
    expect(parseMarkdownInline("$5 and $10")).toEqual([{ kind: "text", text: "$5 and $10" }]);
    expect(parseMarkdownInline("$5 and $10; `$x$`")).toEqual([{ kind: "text", text: "$5 and $10; " }, { kind: "code", text: "$x$" }]);
    expect(parseMarkdownInline("2 * 3 * 4")).toEqual([{ kind: "text", text: "2 * 3 * 4" }]);
    expect(parseMarkdownInline("[x](javascript:alert(1))")[0]).toMatchObject({ kind: "link", label: "x", href: "javascript:alert(1" });
  });
});

describe("underscores inside a word", () => {
  it("stay text, as in CommonMark: snake_case and long_identifiers never turn italic", () => {
    expect(parseMarkdownInline("a very_long_identifier_here and snake_case")).toEqual([{ kind: "text", text: "a very_long_identifier_here and snake_case" }]);
    expect(parseMarkdownInline("an _emphasised_ word")).toEqual([{ kind: "text", text: "an " }, { kind: "em", children: [{ kind: "text", text: "emphasised" }] }, { kind: "text", text: " word" }]);
  });
});
