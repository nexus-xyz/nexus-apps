import { describe, expect, it } from "vitest";
import { base64, draftCount, insertedRun, insertSlash, linesOf, matchSlash, pastedFile, referenceToken, slashQuery } from "./composer-text";

describe("linesOf", () => {
  it("counts 22 pt lines, at least one", () => {
    expect(linesOf(0)).toBe(1);
    expect(linesOf(22)).toBe(1);
    expect(linesOf(66)).toBe(3);
    expect(linesOf(111)).toBe(5);
  });
});

describe("draftCount", () => {
  it("counts words while short", () => {
    expect(draftCount("")).toBe("0 words");
    expect(draftCount("one")).toBe("1 word");
    expect(draftCount("  Put the table\nin the doc  ")).toBe("6 words");
  });

  it("estimates tokens from 1,000, a file included", () => {
    expect(draftCount("a".repeat(8400))).toBe("~2.1k tokens");
    expect(draftCount("short", 4000)).toBe("~1k tokens");
  });
});

describe("insertedRun", () => {
  it("finds what a paste added, wherever the caret was", () => {
    expect(insertedRun("ab", "aXYb")).toEqual({ start: 1, text: "XY" });
    expect(insertedRun("", "pasted")).toEqual({ start: 0, text: "pasted" });
    expect(insertedRun("abc", "abcdef")).toEqual({ start: 3, text: "def" });
  });

  it("is null when text was removed or replaced", () => {
    expect(insertedRun("abc", "ab")).toBeNull();
    expect(insertedRun("abc", "xyzw")).toBeNull();
  });
});

describe("pastedFile", () => {
  it("is Markdown named pasted-text.md, its UTF-8 bytes in a data URL", () => {
    const file = pastedFile("héllo");
    expect(file.name).toBe("pasted-text.md");
    expect(file.bytes).toBe(6);
    expect(file.data).toBe("data:text/markdown;base64,aMOpbGxv");
  });

  it("pads base64 as a data URL needs", () => {
    expect(base64(new Uint8Array([104]))).toBe("aA==");
    expect(base64(new Uint8Array([104, 105]))).toBe("aGk=");
  });
});

describe("the / menu", () => {
  it("opens on a / that starts the text or a word", () => {
    expect(slashQuery("/", 1)).toBe("");
    expect(slashQuery("run /bench", 10)).toBe("bench");
    expect(slashQuery("a/b", 3)).toBeNull();
    expect(slashQuery("/bench now", 10)).toBeNull();
  });

  it("puts the skill in place of the word, the caret after it", () => {
    expect(insertSlash("run /be", 7, "benchmark-decoder")).toEqual({ text: "run /benchmark-decoder ", caret: 23 });
    expect(insertSlash("/re then", 3, "review-pr")).toEqual({ text: "/review-pr then", caret: 11 });
  });

  it("lists names that start with it first", () => {
    const skills = [{ name: "write-up" }, { name: "review-pr" }, { name: "pr-summary" }];
    expect(matchSlash(skills, "pr").map((s) => s.name)).toEqual(["pr-summary", "review-pr"]);
    expect(matchSlash(skills, "").length).toBe(3);
  });
});

describe("the @ menu", () => {
  it.each([
    ["@", 1, { start: 0, end: 1, query: "" }],
    ["Read @brief today", 8, { start: 5, end: 11, query: "br" }],
    ["Write to me@example.org", 18, undefined],
    ["Read @brief today", 17, undefined],
  ] as const)("finds the reference at the caret in %s", (draft, caret, reference) => {
    expect(referenceToken(draft, caret)).toEqual(reference);
  });
});
