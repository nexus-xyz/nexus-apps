import { describe, expect, it } from "vitest";
import { contextLabel, findModels, shortList, splitCurrent } from "./current-models";

const names = ["Auto", "Claude Sonnet 5.5", "Claude Opus 5.5", "Claude Haiku 5.5", "Claude Fable 5.1", "Claude Fable 5", "Claude Opus 5", "Claude Sonnet 5", "Claude Opus 4.8", "Claude Opus 4.7", "Claude Sonnet 4.6", "Claude Haiku 4.5", "GPT-6.1 Sol", "GPT-6 Luna", "GPT-6 Sol", "GPT-6 Astra", "GPT-5.3-Codex"];
const models = names.map((name) => ({ id: name, name }));

describe("splitCurrent", () => {
  it("keeps the newest of each line in front and the rest under Other models", () => {
    const { current, other } = splitCurrent(models, () => false);
    expect(current.map((m) => m.name)).toEqual(["Auto", "Claude Sonnet 5.5", "Claude Opus 5.5", "Claude Haiku 5.5", "Claude Fable 5.1", "GPT-6.1 Sol", "GPT-6 Luna", "GPT-6 Astra", "GPT-5.3-Codex"]);
    expect(other.map((m) => m.name)).toEqual(["Claude Fable 5", "Claude Opus 5", "Claude Sonnet 5", "Claude Opus 4.8", "Claude Opus 4.7", "Claude Sonnet 4.6", "Claude Haiku 4.5", "GPT-6 Sol"]);
  });

  it("keeps the default and the model in use in front, however old", () => {
    const { current } = splitCurrent(models, (m) => m.name === "Claude Opus 4.7");
    expect(current.map((m) => m.name)).toContain("Claude Opus 4.7");
  });
});

describe("the model sheet's rows", () => {
  it("says a context window in thousands or millions", () => {
    expect(contextLabel(200_000)).toBe("200k");
    expect(contextLabel(1_000_000)).toBe("1M");
    expect(contextLabel(1_048_576)).toBe("1M");
    expect(contextLabel(undefined)).toBeUndefined();
  });

  it("finds models by every word of the search, by name or id", () => {
    const list = [
      { id: "anthropic/claude-opus-5.5", name: "Claude Opus 5.5" },
      { id: "openai/gpt-5.5", name: "GPT-5.5" },
    ];
    expect(findModels(list, "opus").map((m) => m.name)).toEqual(["Claude Opus 5.5"]);
    expect(findModels(list, "openai gpt").map((m) => m.name)).toEqual(["GPT-5.5"]);
    expect(findModels(list, " ")).toEqual(list);
  });
});

describe("the model sheet's short list", () => {
  const latest = [
    { id: "a/one", name: "One" },
    { id: "b/two", name: "Two" },
    { id: "c/three", name: "Three" },
    { id: "d/four", name: "Four" },
  ];
  it("puts the model in use first, then the default, then the rest, three in all", () => {
    expect(shortList(latest, "d/four", (m) => m.id === "c/three").map((m) => m.name)).toEqual(["Four", "Three", "One"]);
  });
  it("lists each once, and the latest in order with nothing chosen", () => {
    expect(shortList(latest, "c/three", (m) => m.id === "c/three").map((m) => m.name)).toEqual(["Three", "One", "Two"]);
    expect(shortList(latest, "nexus/auto", () => false).map((m) => m.name)).toEqual(["One", "Two", "Three"]);
  });
});
