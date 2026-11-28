import { describe, expect, it } from "vitest";
import { flattenTeams, SLUG, suggestSlug, teamPicker } from "./teams";

describe("teams", () => {
  it("lists the tree depth first", () => {
    const tree = [{ slug: "a", children: [{ slug: "a1", children: [] }] }, { slug: "b", children: [] }];
    expect(flattenTeams(tree).map((t) => t.slug)).toEqual(["a", "a1", "b"]);
  });

  it("suggests a slug Gateway accepts", () => {
    expect(suggestSlug("  Platform Core! ")).toBe("platform-core");
    expect(SLUG.test(suggestSlug("Platform Core"))).toBe(true);
    expect(SLUG.test("-bad")).toBe(false);
  });

  describe("teamPicker", () => {
    const platform = { id: "t1", slug: "platform", name: "Platform", role: "member" as const };
    const research = { id: "t2", slug: "research", name: "Research", role: "lead" as const };
    const mine = { teams: [platform, research], team: { id: "t2", slug: "research", name: "Research" } };

    it("is not shown when you are in no team", () => {
      expect(teamPicker({})).toBeNull();
      expect(teamPicker({ teams: [] })).toBeNull();
    });

    it("lists your teams with your default marked, and starts on the default", () => {
      const picker = teamPicker(mine);
      expect(picker?.options.map((o) => [o.value, o.detail])).toEqual([
        ["platform", undefined],
        ["research", "Your default"],
      ]);
      expect(picker?.selected).toBe("research");
      expect(picker?.name).toBe("Research");
    });

    it("shows the chat's or the picked team", () => {
      expect(teamPicker(mine, "platform")).toMatchObject({ selected: "platform", name: "Platform" });
      expect(teamPicker(mine, "gone")).toMatchObject({ selected: "gone", name: "gone" });
      expect(teamPicker({ teams: [platform] })).toMatchObject({ selected: "", name: "Team" });
    });
  });
});
