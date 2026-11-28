import { createAgentClient, createCoreClient } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { readProjectReference } from "./project-reference";

const project = { id: "prj_fixture", name: "Checkout", repos: ["test-org/checkout"], errands: 0 };
const linkedRepos = project.repos.map((name) => ({ fullName: name, url: `https://github.com/${name}` }));
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const note = (text: string, overrides: object = {}) => ({
  id: text,
  text,
  createdAt: "2026-11-20T10:00:00Z",
  audience: "dm",
  project: { id: project.id, name: project.name },
  ...overrides,
});
const view = {
  project,
  gateway: "on",
  errands: [{ id: "errand-excluded", title: "An unrelated task" }],
  files: { source: "repositories", repos: linkedRepos },
  notes: [
    note("Keep cookie expiry in one place."),
    note("Another project's private text", { project: { id: "prj_other", name: project.name } }),
    note("Channel-only context", { audience: "channel" }),
    note("Personal global context", { project: undefined }),
  ],
};

function clients(body: Response, readTree: (url: string, init?: RequestInit) => Response) {
  const readProject = vi.fn<typeof fetch>(async () => body);
  const readRepo = vi.fn<typeof fetch>(async (url, init) => readTree(String(url), init));
  return {
    readProject,
    readRepo,
    agent: createAgentClient({ baseUrl: "https://assistant.test", apiKey: "fixture", fetch: readProject }),
    core: createCoreClient({ baseUrl: "https://gateway.test", apiKey: "fixture", fetch: readRepo }),
  };
}

describe("project references", () => {
  it("reads the selected project and its repository paths without reading file contents", async () => {
    const { readProject, readRepo, agent, core } = clients(json(view), () => json({
      owner: "Test-Org",
      name: "checkout",
      paths: ["README.md", "src/cart.ts", "bad\npath"],
      truncated: false,
      files: [{ path: "README.md", content: "File contents are excluded" }],
    }));
    const preview = await readProjectReference(agent, core, project, new AbortController().signal);

    expect(readProject).toHaveBeenCalledWith("https://assistant.test/agent/projects/view?id=prj_fixture", expect.anything());
    expect(readRepo).toHaveBeenCalledExactlyOnceWith("https://gateway.test/v1/connectors/github/repo/read", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ repo: "https://github.com/test-org/checkout" }),
    }));
    expect(preview).toContain("Keep cookie expiry in one place.");
    expect(preview).toContain("src/cart.ts");
    expect(preview).not.toMatch(/Another project's|Channel-only|Personal global|bad\npath|File contents are excluded|unrelated task/);
  });

  it("does not ask the connector about a missing or different project", async () => {
    for (const body of [json({ error: "not_found" }, 404), json({ ...view, project: { ...project, id: "prj_other" } })]) {
      const { agent, core, readRepo } = clients(body, () => json({}));
      await expect(readProjectReference(agent, core, project, new AbortController().signal)).rejects.toThrow(/could not be read|changed/);
      expect(readRepo).not.toHaveBeenCalled();
    }
  });

  it("keeps readable notes and discloses when the connector denies the file list", async () => {
    const { agent, core } = clients(json(view), () => json({ error: "forbidden" }, 403));
    const preview = await readProjectReference(agent, core, project, new AbortController().signal);
    expect(preview).toContain("Keep cookie expiry in one place.");
    expect(preview).toContain("File list unavailable.");
  });

  it("caps the snapshot and labels paths omitted by the connector or app", async () => {
    const paths = Array.from({ length: 60 }, (_, index) => `file-${index}.txt`);
    const { agent, core } = clients(json(view), () => json({ owner: "test-org", name: "checkout", paths, truncated: true }));
    const preview = await readProjectReference(agent, core, project, new AbortController().signal);
    expect(preview).toContain("file-39.txt");
    expect(preview).not.toContain("file-40.txt");
    expect(preview).toContain("Additional file paths omitted.");
  });
});
