import { describe, expect, it } from "vitest";
import { formatQuotedPrompt, readQuotedPrompt } from "./quoted-prompt";

describe("a quoted question", () => {
  it("sends ordered passages, including blank lines, as Markdown blocks", () => {
    const content = formatQuotedPrompt("Do these agree?", ["Before\n\nAfter", "The API uses seconds."]);
    expect(content).toBe("> Before\n> \n> After\n\n> The API uses seconds.\n\nDo these agree?");
    expect(readQuotedPrompt(content)).toEqual({ quotes: ["Before\n\nAfter", "The API uses seconds."], prompt: "Do these agree?" });
  });

  it("can send a quote on its own", () => {
    expect(readQuotedPrompt(formatQuotedPrompt("", ["A passage"]))).toEqual({ quotes: ["A passage"], prompt: "" });
  });

  it("keeps greater-than comparisons and ordinary message text", () => {
    expect(readQuotedPrompt(">10 seconds\n\nTry this")).toEqual({ quotes: [], prompt: ">10 seconds\n\nTry this" });
    expect(readQuotedPrompt("Compare\n\n> either version")).toEqual({ quotes: [], prompt: "Compare\n\n> either version" });
  });
});
