import { describe, expect, it } from "vitest";
import type { ModelPolicyView } from "./api";
import { createCoreClient } from "./core-client";

// `setAgentPolicy` writes through `PUT /v1/policies/agents/{agent}`, not the deprecated `POST /v1/agents/policy`.

function view(rule: ModelPolicyView["rule"], model: string): ModelPolicyView {
  return {
    level: "agent",
    subject: "claude",
    rule,
    custom: true,
    inherited: [],
    effective: {
      allowed: rule.allow ?? [],
      defaults: {
        chat: { from: { level: "none" } },
        claude: { model, from: { level: "agent", subject: "claude" } },
        codex: { from: { level: "none" } },
      },
    },
  };
}

function stub(own: ModelPolicyView["rule"]) {
  const seen: { method: string; path: string; body: unknown; ifMatch: string | null }[] = [];
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    ifMatch: true,
    fetch: async (url, init) => {
      const method = init?.method ?? "GET";
      const text = typeof init?.body === "string" ? init.body : "";
      const body = text ? JSON.parse(text) : undefined;
      seen.push({ method, path: new URL(String(url)).pathname, body, ifMatch: new Headers(init?.headers).get("if-match") });
      if (method === "GET") return Response.json(view(own, "anthropic/old"), { headers: { etag: '"3"' } });
      return Response.json(view(body, body.defaults.claude), { headers: { etag: '"4"' } });
    },
  });
  return { core, seen };
}

describe("setAgentPolicy", () => {
  it("puts the agent's rule at the version it read, and never calls the deprecated alias", async () => {
    const { core, seen } = stub({ deny: ["x/*"], maxPrice: { input: 5 } });
    const saved = await core.setAgentPolicy("claude", { allowedModels: ["anthropic/a", "anthropic/b"], defaultModel: "anthropic/a" });
    expect(seen.map(({ method, path }) => `${method} ${path}`)).toEqual(["GET /v1/policies/agents/claude", "PUT /v1/policies/agents/claude"]);
    expect(seen[1]?.ifMatch).toBe('"3"');
    expect(seen.some(({ path }) => path === "/v1/agents/policy")).toBe(false);
    expect(saved).toEqual({ agent: "claude", allowedModels: ["anthropic/a", "anthropic/b"], defaultModel: "anthropic/a" });
  });

  it("keeps the rest of the agent's own rule and sets only this agent's default, as the alias did", async () => {
    const { core, seen } = stub({ allow: ["old/*"], deny: ["x/*"], defaults: { chat: "c/1", claude: "old/1" }, maxPrice: { input: 5 } });
    await core.setAgentPolicy("claude", { allowedModels: ["anthropic/a"], defaultModel: "anthropic/a" });
    expect(seen[1]?.body).toEqual({ allow: ["anthropic/a"], deny: ["x/*"], defaults: { claude: "anthropic/a" }, maxPrice: { input: 5 } });
  });

  it("starts from an empty rule when the agent has none of its own", async () => {
    const { core, seen } = stub({});
    await core.setAgentPolicy("claude", { allowedModels: ["anthropic/a"], defaultModel: "anthropic/a" });
    expect(seen[1]?.body).toEqual({ allow: ["anthropic/a"], defaults: { claude: "anthropic/a" } });
  });
});
