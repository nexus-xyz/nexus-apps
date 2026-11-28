import { describe, expect, it } from "vitest";
import { contextUse, tokens } from "./context";

const windowOf = (model?: string) => (model === "anthropic/claude-sonnet-5-5" ? 1_000_000 : undefined);

describe("the chat's context", () => {
  it("counts what the latest answer read and wrote, out of its model's window", () => {
    const use = contextUse(
      [
        { role: "user" },
        { role: "assistant", model: "anthropic/claude-sonnet-5-5", usage: { promptTokens: 100, completionTokens: 10 } },
        { role: "user" },
        { role: "assistant", model: "anthropic/claude-sonnet-5-5", usage: { promptTokens: 600_000, completionTokens: 18_000 } },
      ],
      windowOf,
    );
    expect(use).toEqual({ used: 618_000, window: 1_000_000, share: 0.618, input: 600_000, output: 18_000, model: "anthropic/claude-sonnet-5-5" });
  });

  it("is unknown without a count or a window", () => {
    expect(contextUse([{ role: "assistant", model: "anthropic/claude-sonnet-5-5" }], windowOf)).toBeUndefined();
    expect(contextUse([{ role: "assistant", model: "x/y", usage: { promptTokens: 1, completionTokens: 1 } }], windowOf)).toBeUndefined();
  });

  it("writes tokens short", () => {
    expect([940, 8200, 472_000, 1_000_000, 1_200_000].map(tokens)).toEqual(["940", "8.2k", "472k", "1M", "1.2M"]);
  });
});
