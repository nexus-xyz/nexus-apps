import { describe, expect, it } from "vitest";
import { gatewayNote, linkedText, linkFailedText, linkLabel, projectHref, projectLine, projectOfParams, sessionLine, sessionOpen } from "./project-words";

const named = { name: "Fundraise", repos: [], errands: 1 };
const gateway = { name: "Northwind Labs", id: "p_1", repos: ["acme/lumen", "acme/docs"], errands: 3 };

describe("project words (A14)", () => {
  it("says what a project is, its repositories and its errands", () => {
    expect(projectLine(named)).toBe("Your errands · 1 errand");
    expect(projectLine(gateway)).toBe("Gateway project · 2 repositories · 3 errands");
    expect(projectLine({ ...gateway, repos: ["acme/x"], personal: true })).toBe("Personal Gateway project · 1 repository · 3 errands");
  });

  it("addresses a page by id when it has one, else by name", () => {
    expect(projectHref(gateway).params).toEqual({ id: "p_1", name: "Northwind Labs" });
    expect(projectHref(named).params).toEqual({ name: "Fundraise" });
    expect(projectOfParams({ id: "p_1", name: "Northwind Labs" })).toEqual({ id: "p_1", name: "Northwind Labs" });
    expect(projectOfParams({ name: " " })).toBeUndefined();
  });

  it("says why Gateway's projects are not shown", () => {
    expect(gatewayNote("on")).toBeUndefined();
    expect(gatewayNote("off")).toContain("not on");
    expect(gatewayNote("unavailable")).toContain("could not be reached");
  });

  it("offers the link and says what moved", () => {
    expect(linkLabel(named)).toBeUndefined();
    expect(linkLabel({ link: { id: "p_1", name: "Northwind Labs" } })).toBe("Link to Northwind Labs");
    expect(linkedText({ linked: { errands: 1, notes: 2 }, project: gateway })).toBe("Linked to Northwind Labs. 1 errand and 2 notes moved with it.");
    expect(linkFailedText({ status: 409, error: "projects_off" })).toContain("not on");
    expect(linkFailedText({ status: 404 })).toContain("not one you are in");
  });

  it("never follows a desktop session's nexus:// link on the phone", () => {
    const base = { title: "Fix", state: "running" as const, at: "2026-11-19T10:00:00Z", errand: "e1", repo: "acme/lumen", branch: "main" };
    expect(sessionLine({ ...base, where: "desktop", url: "nexus://session/1" })).toBe("Running · acme/lumen · main");
    expect(sessionOpen({ where: "desktop", url: "nexus://session/1" })).toEqual({ kind: "desktop", text: "Open it in Nexus for desktop" });
    expect(sessionOpen({ where: "synced", url: "nexus://x" }).kind).toBe("desktop");
    expect(sessionOpen({ where: "synced", url: "https://nexus.inc/s/1" })).toEqual({ kind: "link", url: "https://nexus.inc/s/1" });
  });
});
