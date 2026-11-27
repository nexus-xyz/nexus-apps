import type { SessionEvent } from "@nexus/kit";
import type { OpenedEvent } from "@nexus/kit/synced-sessions";
import { describe, expect, it } from "vitest";
import { activityText, planProgress, turnStartedAt } from "./session-activity";

const todos = (statuses: string[], parent?: string): SessionEvent =>
  ({ kind: "tool", id: `t${statuses.join()}`, name: "TodoWrite", ...(parent ? { parent } : {}), input: { todos: statuses.map((status, i) => ({ content: `Step ${i}`, activeForm: `Doing step ${i}`, status })) } }) as SessionEvent;

describe("the plan's progress", () => {
  it("counts the agent's latest top-level plan, and names the step in progress", () => {
    const events = [todos(["completed", "pending"]), todos(["completed", "completed", "in_progress", "pending"]), todos(["pending"], "sub-agent")];
    expect(planProgress(events)).toEqual({ done: 2, total: 4, current: "Doing step 2" });
  });

  it("has nothing before a plan, or for an empty one", () => {
    expect(planProgress([{ kind: "text", text: "hi" }])).toBeNull();
    expect(planProgress([todos([])])).toBeNull();
    expect(planProgress([todos(["completed"])])).toEqual({ done: 1, total: 1, current: null });
  });
});

describe("when the current turn started", () => {
  const open = (seq: number, at: number, kind: string): OpenedEvent => ({ state: "open", seq, at, thread: {} as never, event: { kind } });
  it("is the latest turn's start, and none once you wrote again", () => {
    expect(turnStartedAt([open(1, 10, "user"), open(2, 20, "turnStart"), open(3, 30, "text")])).toBe(20);
    expect(turnStartedAt([open(1, 10, "turnStart"), open(2, 40, "user")])).toBeNull();
    expect(turnStartedAt([{ state: "no-key", seq: 1, at: 5 }])).toBeNull();
  });
});

describe("the activity line", () => {
  it("says Waiting for you over Working, and nothing at rest", () => {
    expect(activityText({ running: true, waiting: true, since: 0, now: 90_000 })).toBe("Waiting for you");
    expect(activityText({ running: false, waiting: true, since: null, now: 0 })).toBe("Waiting for you");
    expect(activityText({ running: true, waiting: false, since: 0, now: 83_000 })).toBe("Working 1m 23s");
    expect(activityText({ running: true, waiting: false, since: null, now: 0 })).toBe("Working");
    expect(activityText({ running: false, waiting: false, since: 0, now: 1 })).toBeNull();
  });
});
