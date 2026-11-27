import { describe, expect, it } from "vitest";
import { moveDrawerSession } from "./drawer-organization";
import { dropSection, type DropRect, type Point } from "./drawer-section-drop";

const root = { x: 20, y: -70, width: 280, height: 560 };
const visible = { x: 20, y: 80, width: 280, height: 400 };
const sections = new Map<string, DropRect>([
  ["Off screen", { x: 0, y: 100, width: 280, height: 40 }],
  ["Current", { x: 0, y: 150, width: 280, height: 60 }],
  ["Removed", { x: 0, y: 210, width: 280, height: 60 }],
  ["Release week", { x: 0, y: 210, width: 280, height: 60 }],
  ["Below footer", { x: 0, y: 550, width: 280, height: 10 }],
]);
const names = new Set(["Off screen", "Current", "Release week", "Below footer"]);
const target = (point: Point) => dropSection(point, root, visible, sections, "Current", names);

describe("drawer section drop", () => {
  it("assigns a Chat row to the visible section and preserves other rows", () => {
    const destination = target({ x: 96, y: 152 });
    expect(destination).toBe("Release week");
    const organization = {
      pinned: ["chat:pinned"],
      sections: [
        { name: "Current", sessions: ["chat:moving", "chat:stays"] },
        { name: "Release week", sessions: ["chat:there"] },
      ],
    };
    expect(moveDrawerSession(organization, "chat:moving", destination ?? null)).toEqual({
      pinned: ["chat:pinned"],
      sections: [
        { name: "Current", sessions: ["chat:stays"] },
        { name: "Release week", sessions: ["chat:moving", "chat:there"] },
      ],
    });
  });

  it.each([
    { x: 40, y: 60 },
    { x: 50, y: 120 },
    { x: 50, y: 480 },
    { x: 318, y: 152 },
    { x: 50, y: 230 },
  ])("leaves the Chat in place when the drop is outside an eligible section (%j)", (point) => {
    expect(target(point)).toBeUndefined();
  });
});
