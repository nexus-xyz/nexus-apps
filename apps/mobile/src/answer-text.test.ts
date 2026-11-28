import { describe, expect, it } from "vitest";
import { answerText, speechChunks } from "./answer-text";

describe("response text", () => {
  it("copies the displayed words while preserving code, math, citations and Unicode", () => {
    expect(answerText("# **Café** 👋\n\nRead [the `guide`](https://example.com) and _this_. [2]\n\n> Keep ~~these~~ words.\n\n```ts\nconst literal = '**text**';\n```\n\n$$x_y + 1$$"))
      .toBe("Café 👋\n\nRead the guide and this. [2]\n\nKeep these words.\n\nconst literal = '**text**';\n\nx_y + 1");
  });

  it("keeps nested lists, task states and readable table rows", () => {
    expect(answerText("3. First\n4. Next\n  - [x] **Done**\n  - [ ] Waiting\n\n| Name | Value |\n| --- | --- |\n| **Total** | `$5` |\n\n---"))
      .toBe("3. First\n4. Next\n  ☑ Done\n  ☐ Waiting\n\nName\tValue\nTotal\t$5");
    expect(answerText("")).toBe("");
  });
});

describe("speech input", () => {
  it("reads a long response completely within the native engine's limit", () => {
    const source = "Read each word. ".repeat(600).trim();
    const chunks = speechChunks(source, 4000);
    expect(chunks.length).toBeGreaterThan(1);
    expect(chunks.every((chunk) => chunk.length <= 4000)).toBe(true);
    expect(chunks.join(" ")).toBe(source);
  });

  it("keeps surrogate pairs intact in text without spaces", () => {
    const source = "👋日本語".repeat(12);
    const chunks = speechChunks(source, 8);
    expect(chunks.join("")).toBe(source);
    expect(chunks.every((chunk) => chunk.length <= 8 && !/[\uD800-\uDBFF]$/u.test(chunk))).toBe(true);
    expect(speechChunks(" ", 4000)).toEqual([]);
  });
});
