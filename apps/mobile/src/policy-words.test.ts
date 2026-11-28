import { describe, expect, it } from "vitest";
import { hasRules, listOf, patternName, policyItems, policySentences } from "./policy-words";

const nameOf = (id: string) => ({ "deepseek/deepseek-v4": "DeepSeek V4", "anthropic/claude-sonnet-5-5": "Claude Sonnet 5.5" })[id] ?? id;

describe("a policy in sentences", () => {
  it("names providers' models and single models", () => {
    expect(patternName("anthropic/*", nameOf)).toBe("Anthropic models");
    expect(patternName("deepseek/deepseek-v4", nameOf)).toBe("DeepSeek V4");
    expect(listOf(["A", "B", "C"])).toBe("A, B and C");
  });

  it("says what is allowed, refused and capped, and where calls go", () => {
    expect(
      policySentences(
        {
          allow: ["anthropic/*", "openai/*"],
          deny: ["deepseek/deepseek-v4"],
          maxPrice: { output: 15 },
          defaults: { chat: "anthropic/claude-sonnet-5-5" },
          denyCountries: { maker: ["CN"] },
        },
        nameOf,
      ),
    ).toEqual({
      models: ["Only Anthropic and OpenAI models", "Never DeepSeek V4", "Up to $15 per M output", "Chats start on Claude Sonnet 5.5"],
      where: ["No models made in CN"],
    });
  });

  it("says every model when the rule is empty", () => {
    expect(policySentences({}, nameOf)).toEqual({ models: ["Every model the catalog lists"], where: [] });
  });

  it("names each rule for its own page", () => {
    expect(policyItems({ maxPrice: { output: 15 }, providers: { allow: ["anthropic"] } }, nameOf)).toEqual({
      models: [{ title: "Price cap", text: "Up to $15 per M output" }],
      where: [{ title: "Allowed providers", text: "Only through Anthropic" }],
    });
  });

  it("knows an empty rule from one that says something", () => {
    expect(hasRules({})).toBe(false);
    expect(hasRules({ allow: [], providers: {} })).toBe(false);
    expect(hasRules({ deny: ["deepseek/*"] })).toBe(true);
  });
});
