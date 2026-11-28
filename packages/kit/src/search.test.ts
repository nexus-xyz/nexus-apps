import { describe, expect, it } from "vitest";
import { mergeRanges, segments } from "./search";

describe("search highlighting", () => {
  it("cuts text into plain and matched parts, joining overlaps and dropping bad ranges", () => {
    expect(segments("Claude Opus", [[0, 6]])).toEqual([
      { text: "Claude", match: true },
      { text: " Opus", match: false },
    ]);
    expect(segments("abcdef", [[3, 5], [1, 2], [4, 6], [-1, 0], [5, 5]])).toEqual([
      { text: "a", match: false },
      { text: "b", match: true },
      { text: "c", match: false },
      { text: "def", match: true },
    ]);
    expect(mergeRanges([[4, 6], [0, 2], [2, 3]])).toEqual([[0, 3], [4, 6]]);
  });
});
