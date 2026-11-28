import { describe, expect, it } from "vitest";
import { assistantPlace, drawerPlaces, errandHref, mobilePlaceVisibility, opensInDrawer } from "./drawer-places";

describe("the drawer's places", () => {
  it("puts the Assistant at the top, with the Nexus mark, in a build with the agent API", () => {
    const fakeAgent = { ask: () => undefined };
    const assistant = assistantPlace(fakeAgent);
    expect(assistant && [assistant.label, assistant.icon, assistant.href]).toEqual(["Assistant", "mark", "/agent"]);
  });

  it("leaves the Assistant out without one, as its screen and desktop do", () => {
    expect(assistantPlace(null)).toBeNull();
    expect(assistantPlace(undefined)).toBeNull();
  });

  it("shows the phone's primary destinations and opens Customize on Connectors", () => {
    expect(drawerPlaces().map(({ label, href }) => [label, href])).toEqual([
      ["Artifacts", "/pages"],
      ["Customize", { pathname: "/integrations", params: { customize: "1" } }],
      ["More", "/settings-group/more"],
    ]);
  });

  it("keeps a hidden drawer row for its owner and removes it for other roles", () => {
    expect(mobilePlaceVisibility("artifacts", ["artifacts"], "owner")).toBe("owner");
    expect(mobilePlaceVisibility("artifacts", ["artifacts"], "admin")).toBe("hidden");
    expect(mobilePlaceVisibility("artifacts", ["artifacts"], "member")).toBe("hidden");
    expect(mobilePlaceVisibility("artifacts", [], "member")).toBe("visible");
    expect(mobilePlaceVisibility("more", ["artifacts"], "member")).toBe("visible");
  });

  it("open the Assistant, Search's Ask Nexus and a session in the drawer, beside Chat, and every other page in the sheet", () => {
    expect(opensInDrawer("/agent")).toBe(true);
    expect(opensInDrawer({ pathname: "/agent", params: { ask: "dinner friday" } })).toBe(true);
    expect(opensInDrawer({ pathname: "/session/[id]", params: { id: "new" } })).toBe(true);
    // An errand's thread takes the chat's place too (A12).
    expect(opensInDrawer(errandHref("i3"))).toBe(true);
    expect(opensInDrawer("/inbox")).toBe(false);
    expect(opensInDrawer({ pathname: "/artifact/[id]", params: { id: "a" } })).toBe(false);
  });
});
