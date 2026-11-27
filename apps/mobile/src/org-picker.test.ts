import type { OrgSummary } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { pickerSections, rememberOrg, roleLine } from "./org-picker";

const org = (name: string, extra: Partial<OrgSummary> = {}): OrgSummary =>
  ({ id: name.toLowerCase(), slug: name.toLowerCase().replace(/\s+/g, "-"), name, kind: "company", role: "member", current: false, ...extra }) as OrgSummary;

const fifty = [org("Me", { kind: "personal", role: "owner" }), ...Array.from({ length: 49 }, (_, i) => org(`Team ${String(i + 1).padStart(2, "0")}`, { current: i === 0 }))];

const view = (sections: ReturnType<typeof pickerSections>) => sections.map((s) => [s.key, s.orgs.map((o) => o.name)]);
const names = (orgs: OrgSummary[]) => orgs.map((o) => o.name);

describe("rememberOrg", () => {
  it("puts the newest first, keeps each once, and keeps eight", () => {
    expect(rememberOrg(["a", "b", "c"], "b")).toEqual(["b", "a", "c"]);
    expect(rememberOrg(["a", "b", "c", "d", "e", "f", "g", "h"], "z")).toEqual(["z", "a", "b", "c", "d", "e", "f", "g"]);
  });
});

describe("the sheet's list", () => {
  it("lists a few organizations as they are, Personal first, in a fixed order", () => {
    const orgs = [org("Zeta"), org("Acme", { current: true, role: "owner" }), org("Me", { kind: "personal" }), org("Beta")];
    expect(view(pickerSections(orgs, "", ["zeta"]))).toEqual([["all", ["Me", "Acme", "Beta", "Zeta"]]]);
  });

  it("puts the ones you were in last first when there are many", () => {
    const sections = pickerSections(fifty, "", ["team 20", "team 01", "me"]);
    expect(sections.map((s) => [s.key, s.title, s.orgs.length])).toEqual([
      ["recent", "Recent", 2],
      ["all", "All 50 organizations", 50],
    ]);
    expect(names(sections[0]!.orgs)).toEqual(["Team 20", "Me"]);
    expect(pickerSections(fifty).map((s) => s.key)).toEqual(["all"]);
  });

  it("finds an organization among fifty by any word of its name or slug, ignoring case and accents", () => {
    const orgs = [...fifty, org("Café Société", { slug: "cafe-soc" })];
    expect(view(pickerSections(orgs, "team 07"))).toEqual([["results", ["Team 07"]]]);
    expect(view(pickerSections(orgs, "cafe"))).toEqual([["results", ["Café Société"]]]);
    expect(view(pickerSections(orgs, "SOC"))).toEqual([["results", ["Café Société"]]]);
    expect(pickerSections(orgs, "personal")[0]?.orgs.map((o) => o.kind)).toEqual(["personal"]);
  });

  it("says how many match, and that none do", () => {
    // "1" is in Team 01, 10 to 19, 21, 31 and 41.
    expect(pickerSections(fifty, "team 1")[0]?.title).toBe("14 organizations");
    expect(pickerSections(fifty, "team 07")[0]?.title).toBe("1 organization");
    expect(pickerSections(fifty, "nothing like this")).toEqual([{ key: "results", title: "0 organizations", orgs: [] }]);
  });
});

describe("roleLine", () => {
  it("names your role, or Just you in Personal", () => {
    expect(roleLine({ kind: "company", role: "owner" })).toBe("Owner");
    expect(roleLine({ kind: "personal", role: "owner" })).toBe("Just you");
    expect(roleLine(undefined)).toBe("");
  });
});
