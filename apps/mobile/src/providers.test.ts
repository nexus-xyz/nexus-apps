import { describe, expect, it } from "vitest";
import { provider, providerOf, providerRank } from "./providers";

describe("providers", () => {
  it("names the provider of an id or a bare family name, as Nexus UI does", () => {
    expect(providerOf("anthropic/claude-sonnet-5.5")).toBe("anthropic");
    expect(providerOf("claude-haiku-5-5")).toBe("anthropic");
    expect(providerOf("gpt-5")).toBe("openai");
    expect(providerOf("nexus/auto")).toBe("nexus");
    expect(providerOf("kimi-k3")).toBe("moonshot");
  });

  it("draws a mark for every catalog maker the phone showed as a letter", () => {
    for (const id of ["moonshot", "zai", "nvidia", "xai", "reflection", "thinkingmachines", "inclusionai", "typesafe"]) {
      expect(provider(id).path, id).toMatch(/^m/i);
    }
    expect(provider("zai").name).toBe("Z.ai");
    expect(provider("nvidia").name).toBe("NVIDIA");
    expect(provider("reflection").name).toBe("Reflection AI");
  });

  it("draws a known provider's mark, a chip for the rest, and orders them as Nexus UI lists them", () => {
    expect(provider("anthropic")).toMatchObject({ name: "Anthropic", hex: "D97757" });
    expect(provider("openai")).toMatchObject({ name: "OpenAI", path: expect.stringMatching(/^M/) });
    expect(provider("openai").hex).toBeUndefined();
    expect(provider("acme")).toEqual({ id: "acme", name: "Acme" });
    expect(providerRank("anthropic")).toBeLessThan(providerRank("openai"));
    expect(providerRank("acme")).toBeGreaterThan(providerRank("qwen"));
  });
});
