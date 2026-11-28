import { describe, expect, it } from "vitest";
import { messageMetaParts, usd } from "./meta";

describe("the line under an answer", () => {
  it("names the model, says when Auto chose it, and prices the call", () => {
    const name = (id: string) => (id === "anthropic/claude-sonnet-5.5" ? "Claude Sonnet 5.5" : id);
    expect(messageMetaParts({ model: "anthropic/claude-sonnet-5.5", requestedModel: "nexus/auto", costUsd: 0.0012 }, name)).toEqual([
      "Claude Sonnet 5.5",
      "auto",
      "$0.001200",
    ]);
    expect(messageMetaParts({ mode: "ask", costUsd: 0 }, name)).toEqual(["no model called", "$0.00"]);
  });

  it("prices to the precision the amount needs", () => {
    expect([usd(12), usd(0.05), usd(0.0123), usd(0.000034)]).toEqual(["$12.00", "$0.05", "$0.0123", "$0.000034"]);
  });
});
