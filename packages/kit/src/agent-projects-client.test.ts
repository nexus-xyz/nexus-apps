import { describe, expect, it } from "vitest";
import { createAgentClient } from "./agent-chat";

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const client = (answer: (url: string, init?: RequestInit) => Response) => createAgentClient({ baseUrl: "https://agents.test", apiKey: "k", fetch: async (url, init) => answer(String(url), init) });
const project = { name: "Fundraise", repos: [], errands: 0 };

describe("the agent client's project calls (A14)", () => {
  it("reads the list and one project by id or name, and a missing project is gone", async () => {
    const seen: string[] = [];
    const api = client((url) => {
      seen.push(url);
      if (url.includes("name=Nope")) return json({ error: "not_found" }, 404);
      return url.endsWith("/agent/projects") ? json({ projects: [project], gateway: "off" }) : json({ project, gateway: "off", errands: [], sessions: [], files: { source: "repositories", repos: [] }, notes: [] });
    });
    expect((await api.projects()).kind).toBe("list");
    expect((await api.projectView({ name: "Fundraise" })).kind).toBe("project");
    expect((await api.projectView({ id: "p_1", name: "Lumen" })).kind).toBe("project");
    expect((await api.projectView({ name: "Nope" })).kind).toBe("gone");
    expect(seen).toContain("https://agents.test/agent/projects/view?id=p_1");
  });

  it("reads a body that is not a list as a failure, and errands off as off", async () => {
    expect((await client(() => json({ nope: 1 })).projects()).kind).toBe("failed");
    expect((await client(() => json({ error: "errands_off" }, 503)).projects()).kind).toBe("off");
  });

  it("links a project and scopes an added note", async () => {
    const bodies: unknown[] = [];
    const api = client((url, init) => {
      bodies.push(JSON.parse(String(init?.body)));
      return url.endsWith("/link")
        ? json({ linked: { errands: 1, notes: 0 }, project: { name: "Lumen", id: "p_1", repos: [], errands: 1 } })
        : json({ note: { id: "n", text: "t", createdAt: "2026-11-19T10:00:00Z", audience: "dm" } }, 201);
    });
    expect((await api.linkProject("Lumen", "p_1")).kind).toBe("linked");
    expect((await api.changeMemory({ add: "t", project: { id: "p_1", name: "Lumen" } })).kind).toBe("note");
    expect(bodies).toEqual([
      { name: "Lumen", projectId: "p_1" },
      { text: "t", project: { id: "p_1", name: "Lumen" } },
    ]);
  });
});
