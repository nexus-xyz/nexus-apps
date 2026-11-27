import { describe, expect, it } from "vitest";
import { composerTrigger, contextRefsOf, contextViewOf, filterContext, filterSkills, modelChipLabel, modelViewOf, moveActive, skillMessage, skillOf } from "./agent-composer";

describe("the model", () => {
  it("reads the view strictly and drops a bad row", () => {
    expect(modelViewOf({ model: "auto", models: [{ id: "a/b", name: "B", maker: "a" }, { id: "bad id" }, 3] })).toEqual({ model: "auto", models: [{ id: "a/b", name: "B", maker: "a" }] });
    expect(modelViewOf({ model: "bad id", models: [] })).toBeUndefined();
    expect(modelViewOf({ model: "auto" })).toBeUndefined();
  });
  it("names the chip", () => {
    expect(modelChipLabel("auto")).toBe("Auto");
    expect(modelChipLabel("a/b", [{ id: "a/b", name: "B 1", maker: "a" }])).toBe("B 1");
    expect(modelChipLabel("a/b")).toBe("b");
  });
});

describe("context", () => {
  it("reads picks strictly, once each, at most eight", () => {
    expect(contextRefsOf(undefined)).toEqual([]);
    expect(contextRefsOf([{ kind: "note", id: "n" }, { kind: "note", id: "n" }])).toEqual([{ kind: "note", id: "n" }]);
    for (const bad of ["x", [1], [{ kind: "x", id: "n" }], [{ kind: "note", id: "" }], [{ kind: "note", id: "n", more: 1 }], Array.from({ length: 9 }, (_, n) => ({ kind: "note", id: `n${n}` }))]) {
      expect(contextRefsOf(bad)).toHaveProperty("error");
    }
  });
  it("reads the list and its errors", () => {
    expect(contextViewOf({ items: [{ kind: "connector", id: "gmail", label: "Gmail" }, { kind: "x" }], errors: { errand: "down", nope: "x" } })).toEqual({ items: [{ kind: "connector", id: "gmail", label: "Gmail" }], errors: { errand: "down" } });
  });
  it("filters by the start of a word", () => {
    const items = [{ kind: "connector" as const, id: "gmail", label: "Gmail" }, { kind: "errand" as const, id: "i1", label: "Dinner with Ana" }];
    expect(filterContext(items, "an").map((item) => item.id)).toEqual(["i1"]);
    expect(filterContext(items, "")).toHaveLength(2);
  });
});

describe("skills and triggers", () => {
  it("maps each skill to what the agent obeys", () => {
    expect(skillMessage(skillOf("brief")!, "")).toBe("brief now");
    expect(skillMessage(skillOf("errand")!, " plan dinner ")).toBe("errand: plan dinner");
    expect(skillMessage(skillOf("web")!, "")).toBeUndefined();
    expect(skillOf("rm")).toBeUndefined();
    expect(filterSkills("re").map((skill) => skill.id)).toEqual(["remember"]);
  });
  it("opens the menu a caret is in", () => {
    expect(composerTrigger("/br", 3)).toEqual({ kind: "skill", query: "br", start: 0 });
    expect(composerTrigger("see /br", 7)).toBeUndefined();
    expect(composerTrigger("ask @gm", 7)).toEqual({ kind: "context", query: "gm", start: 4 });
    expect(composerTrigger("a@gm", 4)).toBeUndefined();
    expect(composerTrigger("ask @gm done", 12)).toBeUndefined();
  });
  it("wraps the menu's active row", () => {
    expect(moveActive(0, 3, -1)).toBe(2);
    expect(moveActive(2, 3, 1)).toBe(0);
    expect(moveActive(0, 0, 1)).toBe(0);
  });
});
