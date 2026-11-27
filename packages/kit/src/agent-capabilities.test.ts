import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { AgentAccessEntry } from "./api";
import { AGENT_CATALOG, agentEntry, offlineAgentAccess, routeOf } from "./agent-setup";

/**
 * One agent as Rust computes it (`nexus-cli/src/agent_capabilities.rs`): its offline answer as the Rust clients build
 * it, the facts `AgentCatalogEntry` adds, and its capabilities. `usageReporting`, `sessionLog` and `promptArgument` have
 * no field in `AGENT_CATALOG`, so nothing here reads them.
 */
interface FixtureAgent {
  access: AgentAccessEntry;
  executable: string;
  cliLauncher: boolean;
  configuredProvider: string | null;
  logout: string;
  resume: string;
  capabilities: { gateway: string | null; usageReporting: string | null; sessionLog: boolean; promptArgument: boolean };
}

const fixture = JSON.parse(readFileSync(new URL("../fixtures/agent-capabilities.json", import.meta.url), "utf8")) as { agents: FixtureAgent[] };

describe("AGENT_CATALOG against kit/fixtures/agent-capabilities.json", () => {
  it("matches the Rust catalog's agent order, routes and every access field", () => {
    expect(AGENT_CATALOG.map((entry) => agentEntry(undefined, entry.agent))).toEqual(fixture.agents.map((agent) => agent.access));
  });

  it("lists only released launchers in the offline access view", () => {
    expect(offlineAgentAccess().agents).toEqual(fixture.agents.filter((agent) => agent.cliLauncher).map((agent) => agent.access));
  });

  it.each(fixture.agents)("states $access.agent's own facts as Rust does", (rust) => {
    const entry = AGENT_CATALOG.find((candidate) => candidate.agent === rust.access.agent)!;
    expect({ executable: entry.executable, logout: entry.logout, resume: entry.resume }).toEqual({ executable: rust.executable, logout: rust.logout, resume: rust.resume });
    expect(entry.cliLauncher).toBe(rust.cliLauncher);
    expect(entry.configuredProvider ?? null).toBe(rust.configuredProvider);
    expect(routeOf(entry, "gateway")?.provider ?? null).toBe(rust.capabilities.gateway);
  });
});
