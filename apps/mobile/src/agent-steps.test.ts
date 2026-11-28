import { agentEntry, setupSteps, type AgentAccessEntry } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { agentSetupAvailable, phoneSteps } from "./agent-steps";

const ids = (entry: AgentAccessEntry, route: "gateway" | "subscription", agentSubscriptions?: boolean) =>
  phoneSteps(setupSteps(entry, route, { agentSubscriptions })).map((step) => step.id);

describe("phoneSteps", () => {
  const claude = agentEntry(undefined, "claude");

  it("drops the steps run on the computer", () => {
    expect(ids(claude, "subscription")).toEqual([]);
  });

  it("keeps what an owner or admin still has to do", () => {
    expect(ids(claude, "subscription", false)).toEqual(["allow_subscriptions"]);
  });

  it("keeps a missing provider key on the Gateway route", () => {
    const keyless: AgentAccessEntry = {
      ...claude,
      route: "gateway",
      routes: claude.routes.map((r) => (r.route === "gateway" ? { ...r, available: false, provider: "anthropic" } : r)),
    };
    expect(ids(keyless, "gateway")).toEqual(["provider_key"]);
  });
});

describe("agent setup availability", () => {
  const claude = agentEntry(undefined, "claude");
  const entry = {
    ...claude,
    routes: claude.routes.map((route) => ({ ...route, available: true })),
  };

  it("keeps a blocked organization's route from offering setup", () => {
    const blocked = {
      ...entry,
      routes: entry.routes.map((route) => ({ ...route, available: route.route !== "gateway" })),
    };
    expect(agentSetupAvailable(blocked, "gateway", true)).toBe(false);
    expect(agentSetupAvailable(entry, "subscription", false)).toBe(false);
  });

  it("offers setup when the signed-in Gateway permits the route", () => {
    expect(agentSetupAvailable(entry, "gateway", false)).toBe(true);
    expect(agentSetupAvailable(entry, "subscription", true)).toBe(true);
  });
});
