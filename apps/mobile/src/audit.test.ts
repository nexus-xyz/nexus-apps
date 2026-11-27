import { describe, expect, it } from "vitest";
import { actionText, actorName, byDay, changeRows, dayTitle, show, targetName } from "./audit";

const event = (over: object) => ({ id: "1", at: "", actor: { keyId: "k", kind: "person" }, surface: "ui", action: "team.updated", target: { type: "team", id: "t1", slug: "core" }, ...over }) as never;

describe("audit", () => {
  it("names who and what, as desktop does", () => {
    expect(actorName(event({ actor: { keyId: "github", kind: "system" } }))).toBe("GitHub");
    expect(actorName(event({ actor: { keyId: "k", kind: "person", email: "ada@x.io" } }))).toBe("ada@x.io");
    expect(targetName(event({ target: { type: "member", id: "u1" }, after: { email: "bo@x.io" } }))).toBe("bo@x.io");
    expect(targetName(event({ target: { type: "org", id: "o1" } }))).toBe("This organization");
  });

  it("keeps only the fields that changed when both sides are there", () => {
    expect(changeRows({ before: { name: "A", level: "pod" }, after: { name: "B", level: "pod" } }).rows).toEqual([{ field: "name", from: "A", to: "B" }]);
    expect(changeRows({ after: { name: "B" } })).toEqual({ hasBefore: false, hasAfter: true, rows: [{ field: "name", from: undefined, to: "B" }] });
    expect([show(undefined), show(null), show({ a: 1 })]).toEqual(["–", "none", '{"a":1}']);
  });
});

describe("the log's days", () => {
  const now = new Date(2026, 10, 5, 15, 0);
  const at = (day: number, hour = 12) => new Date(2026, 10, day, hour).toISOString();

  it("heads them Today, Yesterday, then the date", () => {
    expect(dayTitle(at(5, 1), now)).toBe("Today");
    expect(dayTitle(at(4, 23), now)).toBe("Yesterday");
    expect(dayTitle(at(2), now)).toBe("Mon 2 Nov");
  });

  it("keeps the order and groups each day's events", () => {
    const days = byDay([{ at: at(5, 14) }, { at: at(5, 9) }, { at: at(3) }], now);
    expect(days.map((d) => [d.title, d.events.length])).toEqual([["Today", 2], ["Tue 3 Nov", 1]]);
  });

  it("writes an action as words", () => {
    expect(actionText("member.role_changed")).toBe("Member role changed");
    expect(actionText("org.settings_changed")).toBe("Org settings changed");
  });
});
