import type { AgentSessionSummary } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { elapsed, folder, launcherList, runLine, runsLine, splitRuns } from "./agent-runs";

const run = (id: string, status: AgentSessionSummary["status"], startedAt: string, more: Partial<AgentSessionSummary> = {}): AgentSessionSummary => ({
  id,
  agent: "claude",
  model: "anthropic/claude-sonnet-5-5",
  via: "nexus",
  person: "u_1",
  startedAt,
  status,
  ...more,
});

describe("the Agents screen's sessions", () => {
  it("lists every agent's launcher from kit's catalog", () => {
    expect(launcherList()).toBe("nexus claude, nexus codex, nexus agy or nexus cursor-agent");
    expect(launcherList(["nexus claude"])).toBe("nexus claude");
    expect(launcherList([])).toBe("");
  });

  it("puts the running ones first, each list newest first", () => {
    const { running, done } = splitRuns([
      run("a", "succeeded", "2026-11-03T10:00:00Z"),
      run("b", "running", "2026-11-03T11:00:00Z"),
      run("c", "failed", "2026-11-03T12:00:00Z"),
      run("d", "running", "2026-11-03T12:30:00Z"),
    ]);
    expect(running.map((s) => s.id)).toEqual(["d", "b"]);
    expect(done.map((s) => s.id)).toEqual(["c", "a"]);
  });

  it("counts them, and the running ones only when there are some", () => {
    expect(runsLine([run("a", "running", "2026-11-03T10:00:00Z"), run("b", "lost", "2026-11-03T10:00:00Z")])).toBe("2 sessions · 1 running");
    expect(runsLine([run("a", "succeeded", "2026-11-03T10:00:00Z")])).toBe("1 session");
  });

  it("shows a running session's time to now, and a finished one's duration", () => {
    const now = Date.parse("2026-11-03T12:04:12Z");
    expect(elapsed(run("a", "running", "2026-11-03T12:00:00Z"), now)).toBe("04:12");
    expect(elapsed(run("b", "succeeded", "2026-11-03T10:00:00Z", { durationMs: 3_727_000 }), now)).toBe("1:02:07");
    expect(elapsed(run("c", "lost", "2026-11-03T10:00:00Z", { endedAt: "2026-11-03T10:00:52Z" }), now)).toBe("00:52");
    expect(elapsed(run("d", "interrupted", "2026-11-03T10:00:00Z"), now)).toBe("");
  });

  it("says how it ended and where", () => {
    expect(runLine(run("a", "running", "", { cwd: "/Users/maya/agents/spike-sort/" }))).toBe("Running in spike-sort");
    expect(runLine(run("b", "failed", "", { exitCode: 1 }))).toBe("Failed · exit 1");
    expect(folder(undefined)).toBeUndefined();
  });
});
