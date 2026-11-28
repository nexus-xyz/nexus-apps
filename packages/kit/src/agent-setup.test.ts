import { describe, expect, it } from "vitest";
import { AGENT_LAUNCHERS, agentCatalogEntry, agentEntry, agentLabel, agentPlan, knownAgentName, offlineAgentAccess, payerLabel, resumeCommand, routeLabel, routeOf, sessionPayer, setupReady, setupSteps } from "./agent-setup";
import type { AgentAccessEntry, AgentName } from "./api";

const claude = agentEntry(undefined, "claude");
const keyless: AgentAccessEntry = {
  ...claude,
  routes: claude.routes.map((route) => (route.route === "gateway" ? { ...route, access: null, available: false, reason: "no Anthropic key" } : route)),
};
const ids = (steps: ReturnType<typeof setupSteps>) => steps.map((step) => [step.id, step.done]);

describe("an agent's setup steps", () => {
  it("on Gateway: install, sign in to Nexus, and a key when the organization has none (an admin's step)", () => {
    const steps = setupSteps(keyless, "gateway", { local: { agent: "claude", installed: false }, signedIn: true });
    expect(ids(steps)).toEqual([
      ["install", false],
      ["nexus_login", true],
      ["provider_key", false],
    ]);
    expect(steps[0]?.command).toBe("curl -fsSL https://claude.ai/install.sh | bash");
    expect(steps[2]).toMatchObject({ who: "admin", command: "nexus keys add anthropic <key>" });
    expect(setupReady(steps)).toBe(false);
  });

  it("on the plan: the agent signed in to it, once, on this machine", () => {
    const signedOut = setupSteps(claude, "subscription", { local: { agent: "claude", installed: true, login: "signed_out" }, signedIn: true });
    expect(ids(signedOut)).toEqual([
      ["install", true],
      ["nexus_login", true],
      ["plan_login", false],
    ]);
    expect(signedOut[2]?.command).toBe("claude auth login");
    const ready = setupSteps(claude, "subscription", { local: { agent: "claude", installed: true, login: "plan" }, signedIn: true });
    expect(setupReady(ready)).toBe(true);
  });

  it("an API-key sign-in bills the API, not the plan: sign out and in again", () => {
    const steps = setupSteps(agentEntry(undefined, "codex"), "subscription", { local: { agent: "codex", installed: true, login: "api_key" }, signedIn: true });
    expect(steps.at(-1)).toMatchObject({ id: "plan_relogin", done: false, command: "codex login" });
    expect(steps.at(-1)?.text).toContain("codex logout");
  });

  it("an organization that turned subscriptions off needs an admin first", () => {
    const steps = setupSteps(claude, "subscription", { local: { agent: "claude", installed: true, login: "plan" }, signedIn: true, agentSubscriptions: false });
    expect(steps.find((step) => step.id === "allow_subscriptions")).toMatchObject({ who: "admin", command: "nexus org settings set agentSubscriptions true" });
  });

  it("a browser cannot look at the machine: those steps are left open, never claimed done", () => {
    const steps = setupSteps(claude, "subscription", { signedIn: true });
    expect(ids(steps)).toEqual([
      ["install", undefined],
      ["nexus_login", true],
      ["plan_login", undefined],
    ]);
    expect(setupReady(steps)).toBeUndefined();
  });

  it("names who pays on each route", () => {
    expect(payerLabel(routeOf(claude, "subscription"))).toBe("your Claude plan");
    expect(payerLabel({ route: "gateway", access: "byok", available: true, provider: "anthropic" })).toBe("your organization's Anthropic key (BYOK)");
    expect(payerLabel({ route: "gateway", access: "nexus", available: true, provider: "openai" })).toBe("Nexus credit");
    expect(payerLabel(routeOf(keyless, "gateway"))).toBe("no Anthropic key yet");
  });
});

// Each agent's own text, pinned as it read when the kit branched on the agent's name: moving it into the catalog
// must not change a word for Claude Code or Codex.
describe("each agent's text", () => {
  const agents: AgentName[] = ["claude", "codex"];
  const relogin = (agent: AgentName) => setupSteps(agentEntry(undefined, agent), "subscription", { local: { agent, installed: true, login: "api_key" } }).at(-1);

  it("tells an API-key sign-in how to sign out", () => {
    expect(agents.map(relogin)).toEqual([
      {
        id: "plan_relogin",
        who: "you",
        text: "Claude Code is signed in with an API key, which bills the API, not your plan: sign out (claude auth logout), then sign in with your Claude account",
        command: "claude auth login",
        done: false,
      },
      {
        id: "plan_relogin",
        who: "you",
        text: "Codex is signed in with an API key, which bills the API, not your plan: sign out (codex logout), then sign in with your ChatGPT account",
        command: "codex login",
        done: false,
      },
    ]);
  });

  it("resumes a conversation with the agent's own syntax", () => {
    const resume = (agent: AgentName, via: "nexus" | "subscription" | "ori", team?: string) =>
      resumeCommand({ agent, agentSessionId: "0b1c-2d3e", status: "succeeded", via, team });
    expect(agents.flatMap((agent) => [resume(agent, "nexus"), resume(agent, "subscription", "web"), resume(agent, "ori")])).toEqual([
      "nexus claude --gateway --resume 0b1c-2d3e",
      "nexus claude --subscription --team web --resume 0b1c-2d3e",
      "nexus claude --resume 0b1c-2d3e",
      "nexus codex --gateway resume 0b1c-2d3e",
      "nexus codex --subscription --team web resume 0b1c-2d3e",
      "nexus codex resume 0b1c-2d3e",
    ]);
  });

  it("names the plan that paid a subscription session", () => {
    expect(agents.map((agent) => sessionPayer({ agent, via: "subscription" }))).toEqual([
      { label: "Claude plan", title: "The person's own plan: calls straight to the vendor, not metered by Gateway" },
      { label: "ChatGPT plan", title: "The person's own plan: calls straight to the vendor, not metered by Gateway" },
    ]);
  });

  it("names the configured provider instead of a plan for OpenCode sessions", () => {
    expect(sessionPayer({ agent: "opencode", via: "subscription" })).toEqual({
      label: "OpenCode provider",
      title: "Model calls go directly to the agent's configured provider",
    });
  });

  it("names each route and who pays on it", () => {
    expect(
      agents.flatMap((agent) => {
        const entry = agentEntry(undefined, agent);
        const keyless = { ...routeOf(entry, "gateway")!, available: false };
        return [routeLabel(entry, "gateway"), routeLabel(entry, "subscription"), payerLabel(routeOf(entry, "gateway")), payerLabel(keyless), payerLabel({ ...keyless, access: "byok" })];
      }),
    ).toEqual([
      "Gateway",
      "your Claude plan",
      "your organization's Anthropic key or Nexus credit",
      "no Anthropic key yet",
      "your organization's Anthropic key (BYOK)",
      "Gateway",
      "your ChatGPT plan",
      "your organization's OpenAI key or Nexus credit",
      "no OpenAI key yet",
      "your organization's OpenAI key (BYOK)",
    ]);
  });

  it("asks an admin for the provider's key when the organization has none", () => {
    const step = (agent: AgentName) => {
      const entry = agentEntry(undefined, agent);
      const routes = entry.routes.map((view) => (view.route === "gateway" ? { ...view, available: false } : view));
      return setupSteps({ ...entry, routes }, "gateway").at(-1);
    };
    expect(agents.map(step)).toEqual([
      { id: "provider_key", who: "admin", text: "Add your organization's Anthropic key (owners and admins)", command: "nexus keys add anthropic <key>", done: false },
      { id: "provider_key", who: "admin", text: "Add your organization's OpenAI key (owners and admins)", command: "nexus keys add openai <key>", done: false },
    ]);
  });

  it("gives an agent this kit does not know none of another agent's text", () => {
    // A newer Gateway can name an agent before this kit lists it.
    const agent = "devin" as AgentName;
    const entry: AgentAccessEntry = { agent, name: "Devin", install: "devin", route: "subscription", from: "default", routes: [{ route: "subscription", access: "subscription", available: true, plan: "Devin" }] };
    const relogin = setupSteps(entry, "subscription", { local: { agent, installed: true, login: "api_key" } }).at(-1);
    expect(relogin?.text).toBe("Devin is signed in with an API key, which bills the API, not your plan: sign out, then sign in with your Devin account");
    expect(resumeCommand({ agent, agentSessionId: "0b1c-2d3e", status: "succeeded", via: "subscription" })).toBeUndefined();
    expect(sessionPayer({ agent, via: "subscription" }).label).toBeUndefined();
    expect(knownAgentName(agent)).toBeUndefined();
    expect(agentLabel(agent)).toBe("devin");
    expect(agentPlan(agent)).toBeUndefined();
  });

  it("names each catalog agent and advertises only released launchers", () => {
    const agents: AgentName[] = ["claude", "codex", "antigravity", "cursor"];
    expect(agents.map(agentLabel)).toEqual(["Claude Code", "Codex", "Antigravity", "Cursor"]);
    expect(agents.map(knownAgentName)).toEqual(["Claude Code", "Codex", "Antigravity", "Cursor"]);
    expect(agents.map(agentPlan)).toEqual(["Claude", "ChatGPT", "Google AI", "Cursor"]);
    expect(resumeCommand({ agent: "cursor", agentSessionId: "c_1", status: "succeeded", via: "subscription" })).toBe("nexus cursor-agent --subscription --resume c_1");
    expect(AGENT_LAUNCHERS).toEqual(["nexus claude", "nexus codex", "nexus agy", "nexus cursor-agent"]);
  });

  it("lists the Gateway route of an agent without one as unavailable, with the reason the Rust client gives", () => {
    const definition = { agent: "antigravity" as AgentName, executable: "agy", name: "Antigravity", install: "agy", plan: "Google AI", vendor: "Google", login: "agy", loginStatus: "agy", logout: "agy", resume: "--conversation" };
    expect(routeOf(agentCatalogEntry(definition), "gateway")).toEqual({
      route: "gateway",
      access: null,
      available: false,
      reason: "Antigravity runs only on your Google AI plan through Nexus for now",
    });
    expect(routeOf(agentCatalogEntry({ ...definition, gateway: { provider: "google" } }), "gateway")).toEqual({ route: "gateway", access: null, available: true, provider: "google" });
  });

  it("falls back to Gateway's answer as it was, shaped as Gateway's", () => {
    expect(offlineAgentAccess()).toEqual({
      agentSubscriptions: true,
      agents: [
        {
          agent: "claude",
          name: "Claude Code",
          install: "curl -fsSL https://claude.ai/install.sh | bash",
          route: "subscription",
          from: "default",
          routes: [
            { route: "gateway", access: null, available: true, provider: "anthropic" },
            { route: "subscription", access: "subscription", available: true, plan: "Claude", vendor: "Anthropic", login: "claude auth login", loginStatus: "claude auth status --text" },
          ],
        },
        {
          agent: "codex",
          name: "Codex",
          install: "npm install -g @openai/codex",
          route: "subscription",
          from: "default",
          routes: [
            { route: "gateway", access: null, available: true, provider: "openai" },
            { route: "subscription", access: "subscription", available: true, plan: "ChatGPT", vendor: "OpenAI", login: "codex login", loginStatus: "codex login status" },
          ],
        },
        // Its own test is below.
        agentEntry(undefined, "antigravity"),
        agentEntry(undefined, "cursor"),
      ],
    });
    expect(agentEntry(undefined, "codex")).toEqual(offlineAgentAccess().agents[1]);
  });
});

// As the Rust client's `antigravity_runs_only_on_its_plan` (`agent_setup.rs`).
describe("Antigravity", () => {
  const antigravity = agentEntry(undefined, "antigravity");
  const installed = { agent: "antigravity" as const, installed: true };

  it("is in the fallback answer with its Gateway route unavailable and no provider", () => {
    expect(offlineAgentAccess().agents.map((entry) => entry.agent)).toEqual(["claude", "codex", "antigravity", "cursor"]);
    expect(antigravity).toEqual({
      agent: "antigravity",
      name: "Antigravity",
      install: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
      route: "subscription",
      from: "default",
      routes: [
        { route: "gateway", access: null, available: false, reason: "Antigravity runs only on your Google AI plan through Nexus for now" },
        { route: "subscription", access: "subscription", available: true, plan: "Google AI", vendor: "Google", login: "agy", loginStatus: "agy" },
      ],
    });
  });

  it("on Gateway: a step that is never done and suggests no key", () => {
    const steps = setupSteps(antigravity, "gateway", { local: installed, signedIn: true });
    expect(steps.at(-1)).toEqual({ id: "route_unsupported", who: "you", text: "Antigravity runs only on your Google AI plan through Nexus for now", done: false });
    expect(setupReady(steps)).toBe(false);
  });

  it("on its plan: the sign-in is not checked, and an API key says how to undo it", () => {
    const plan = setupSteps(antigravity, "subscription", { local: installed, signedIn: true });
    expect(plan.at(-1)).toMatchObject({ id: "plan_login", command: "agy", done: undefined });
    expect(setupReady(plan)).toBeUndefined();
    const relogin = setupSteps(antigravity, "subscription", { local: { ...installed, login: "api_key" }, signedIn: true }).at(-1);
    expect(relogin).toEqual({
      id: "plan_relogin",
      who: "you",
      text: 'Antigravity is signed in with an API key, which bills the API, not your plan: sign out (remove "modelProvider" from ~/.gemini/antigravity-cli/settings.json), then sign in with your Google AI account',
      command: "agy",
      done: false,
    });
    expect(payerLabel(routeOf(antigravity, "subscription"))).toBe("your Google AI plan");
    expect(sessionPayer({ agent: "antigravity", via: "subscription" }).label).toBe("Google AI plan");
  });

  it("resumes with its command, not its id", () => {
    const id = "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d";
    expect(resumeCommand({ agent: "antigravity", agentSessionId: id, status: "succeeded", via: "subscription", team: "web" })).toBe(`nexus agy --subscription --team web --conversation ${id}`);
  });
});
