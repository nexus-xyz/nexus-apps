import { describe, expect, it } from "vitest";
import { agentErrandOf, errandGoalBody } from "./agent-errands";
import { memoryNoteOf } from "./agent-chat";
import { agentProjectLinkedOf, agentProjectListOf, agentProjectOf, agentProjectViewOf, noteInProject, projectLinkBody, projectNoteBody, projectSubtitle, projectViewQuery } from "./agent-projects";

const FUND = { name: "Fundraise", id: "prj_fund", repos: ["acme/data-room"], role: "member", errands: 2 };

describe("the agent API's projects (A14)", () => {
  it("reads a project row strictly: a bad id or name drops it, a bad repository is left out, a link only on a name-only row", () => {
    expect(agentProjectOf(FUND)).toEqual(FUND);
    expect(agentProjectOf({ ...FUND, id: "bad id" })).toBeUndefined();
    expect(agentProjectOf({ ...FUND, name: "" })).toBeUndefined();
    expect(agentProjectOf({ ...FUND, repos: ["acme/ok", "../etc", 3], role: "owner", errands: -1 })).toEqual({ name: "Fundraise", id: "prj_fund", repos: ["acme/ok"], errands: 0 });
    expect(agentProjectOf({ name: "fundraise", errands: 1, link: { id: "prj_fund", name: "Fundraise" } })).toEqual({ name: "fundraise", repos: [], errands: 1, link: { id: "prj_fund", name: "Fundraise" } });
    expect(agentProjectOf({ ...FUND, link: { id: "prj_x", name: "X" } })).not.toHaveProperty("link");
  });

  it("reads the list and needs its gateway state", () => {
    expect(agentProjectListOf({ projects: [FUND, { nope: 1 }], gateway: "on" })).toEqual({ projects: [FUND], gateway: "on" });
    expect(agentProjectListOf({ projects: [], gateway: "maybe" })).toBeUndefined();
    expect(agentProjectListOf({ gateway: "off" })).toBeUndefined();
  });

  it("reads a view: errands, sessions with a safe link only, files from GitHub only, and notes", () => {
    const errand = { id: "i3", goal: "Book investor meetings", status: "running", steps: 0, at: "2026-11-18T00:00:00Z", project: "Fundraise", projectId: "prj_fund" };
    const session = { title: "Metrics", state: "ready", where: "desktop", url: "nexus://new?repo=acme/data-room", at: "t", errand: "i3" };
    const view = agentProjectViewOf({
      project: FUND,
      gateway: "on",
      errands: [errand],
      history: [],
      sessions: [session, { ...session, url: "javascript:alert(1)" }],
      files: { source: "repositories", repos: [{ fullName: "acme/data-room", url: "https://github.com/acme/data-room" }, { fullName: "x/y", url: "https://evil.test" }] },
      notes: [{ id: "n1", text: "Lead by 15 Dec.", createdAt: "t", audience: "dm", project: { id: "prj_fund", name: "Fundraise" } }],
    });
    expect(view?.errands[0]).toMatchObject({ projectId: "prj_fund" });
    expect(view?.sessions).toEqual([session]);
    expect(view?.files.repos).toEqual([{ fullName: "acme/data-room", url: "https://github.com/acme/data-room" }]);
    expect(view?.notes[0]?.project).toEqual({ id: "prj_fund", name: "Fundraise" });
    expect(agentProjectViewOf({ project: FUND, gateway: "on", errands: [], files: { source: "drive" } })).toBeUndefined();
  });

  it("reads a link's answer", () => {
    expect(agentProjectLinkedOf({ linked: { errands: 1, notes: 0 }, project: FUND })).toEqual({ linked: { errands: 1, notes: 0 }, project: FUND });
    expect(agentProjectLinkedOf({ linked: {}, project: FUND })).toBeUndefined();
  });

  it("writes the bodies and the view's query", () => {
    expect(projectViewQuery({ id: "prj_fund", name: "Fundraise" })).toBe("id=prj_fund");
    expect(projectViewQuery({ name: "Northwind Labs" })).toBe("name=Northwind%20Labs");
    expect(JSON.parse(projectLinkBody("fundraise", "prj_fund"))).toEqual({ name: "fundraise", projectId: "prj_fund" });
    expect(JSON.parse(projectNoteBody("Lead by 15 Dec.", { id: "prj_fund", name: "Fundraise" }))).toEqual({ text: "Lead by 15 Dec.", project: { id: "prj_fund", name: "Fundraise" } });
    expect(JSON.parse(errandGoalBody("Book", "Fundraise", "prj_fund"))).toEqual({ goal: "Book", projectId: "prj_fund" });
    expect(JSON.parse(errandGoalBody("Book", "Fundraise"))).toEqual({ goal: "Book", project: "Fundraise" });
  });

  it("scopes a note by the Gateway id when the project has one, else by the name alone", () => {
    expect(noteInProject({ project: { id: "prj_fund", name: "Old name" } }, FUND)).toBe(true);
    expect(noteInProject({ project: { name: "Fundraise" } }, FUND)).toBe(false);
    expect(noteInProject({ project: { name: "Fundraise" } }, { name: "Fundraise" })).toBe(true);
    expect(noteInProject({ project: { id: "prj_fund", name: "Fundraise" } }, { name: "Fundraise" })).toBe(false);
    expect(noteInProject({}, { name: "Fundraise" })).toBe(false);
  });

  it("reads a note's scope and an errand's Gateway id strictly", () => {
    expect(memoryNoteOf({ id: "n", text: "t", createdAt: "t", audience: "dm", project: { id: "bad id", name: "x" } })).not.toHaveProperty("project");
    expect(memoryNoteOf({ id: "n", text: "t", createdAt: "t", audience: "dm", project: { name: "Fundraise" } })?.project).toEqual({ name: "Fundraise" });
    const row = { id: "i1", goal: "g", status: "running", steps: 0, at: "t" };
    expect(agentErrandOf({ ...row, projectId: "prj_fund" })).not.toHaveProperty("projectId");
    expect(agentErrandOf({ ...row, project: "Fundraise", projectId: "prj_fund" })).toMatchObject({ projectId: "prj_fund" });
  });

  it("says what a project's page holds", () => {
    expect(projectSubtitle(agentProjectOf(FUND)!, "on")).toBe("A Gateway project with 1 repository. Its files come from them.");
    expect(projectSubtitle({ name: "Offsite", repos: [], errands: 1 }, "off")).toBe("A project of your errands. No repository.");
    expect(projectSubtitle({ name: "fundraise", repos: [], errands: 1, link: { id: "prj_fund", name: "Fundraise" } }, "on")).toContain("link them");
  });
});
