import type { RunSummary } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { nextCursor, pageQuery, pageRuns, withFirstPage } from "./run-pages";

const run = (id: string, status: RunSummary["status"] = "succeeded") => ({ id, status }) as RunSummary;

describe("run pages", () => {
  it("asks a paged capability for a page, and any other for its whole list", () => {
    expect(pageQuery({ paged_reports: true })).toEqual({ limit: 50 });
    expect(pageQuery({ paged_reports: true }, "c1")).toEqual({ limit: 50, after: "c1" });
    expect(pageQuery({}, "c1")).toEqual({});
  });

  it("lists a run on two pages once, as the earlier page has it", () => {
    const runs = pageRuns([{ runs: [run("b", "running"), run("a")] }, { runs: [run("b", "failed"), run("z")] }]);
    expect(runs.map((r) => [r.id, r.status])).toEqual([["b", "running"], ["a", "succeeded"], ["z", "succeeded"]]);
  });

  it("reads the next cursor from the last page", () => {
    expect(nextCursor(null)).toBeUndefined();
    expect(nextCursor([{ runs: [], next_cursor: "c1" }, { runs: [], next_cursor: null }])).toBeUndefined();
    expect(nextCursor([{ runs: [], next_cursor: "c1" }])).toBe("c1");
  });

  it("replaces a lone first page", () => {
    const fresh = { runs: [run("c"), run("b")], next_cursor: "b" };
    expect(withFirstPage(null, fresh)).toEqual([fresh]);
    expect(withFirstPage([{ runs: [run("b"), run("a")], next_cursor: "a" }], fresh)).toEqual([fresh]);
  });

  it("keeps the runs a new run pushed off the first page when later pages are loaded", () => {
    const pages = [
      { runs: [run("b", "running"), run("a")], next_cursor: "a" },
      { runs: [run("z")], next_cursor: null },
    ];
    const next = withFirstPage(pages, { runs: [run("c"), run("b")], next_cursor: "b" });
    expect(pageRuns(next).map((r) => [r.id, r.status])).toEqual([["c", "succeeded"], ["b", "succeeded"], ["a", "succeeded"], ["z", "succeeded"]]);
    expect(nextCursor(next)).toBeUndefined();
  });
});
