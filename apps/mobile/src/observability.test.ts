import { describe, expect, it } from "vitest";
import { adoptionCounts, dayBefore, lastHour, latestRuns, people, perMinuteText, reachesIntoHour } from "./observability";

const now = Date.parse("2026-11-05T12:00:00Z");
const at = (minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();

describe("lastHour", () => {
  it("counts the calls of the last hour only, with the refused and failed ones", () => {
    const calls = [
      { at: at(1), status: "settled" as const },
      { at: at(30), status: "refused" as const },
      { at: at(59), status: "failed" as const },
      { at: at(61), status: "refused" as const },
    ];
    expect(lastHour(calls, now)).toEqual({ calls: 3, perMinute: 0.05, refused: 1, failed: 1 });
  });

  it("asks for the next page while the oldest call is still within the hour", () => {
    expect(reachesIntoHour([{ at: at(1) }, { at: at(50) }], now)).toBe(true);
    expect(reachesIntoHour([{ at: at(1) }, { at: at(70) }], now)).toBe(false);
    expect(reachesIntoHour([], now)).toBe(false);
    expect(dayBefore(now)).toBe("2026-11-04");
  });

  it("writes calls a minute as the board does", () => {
    expect([perMinuteText(0), perMinuteText(0.05), perMinuteText(6.86), perMinuteText(412.4)]).toEqual(["0", "0.1", "6.9", "412"]);
  });
});

describe("latestRuns", () => {
  it("puts running ones first, then the newest", () => {
    const runs = [
      { id: "a", status: "succeeded" as const, startedAt: at(5) },
      { id: "b", status: "running" as const, startedAt: at(50) },
      { id: "c", status: "failed" as const, startedAt: at(1) },
    ];
    expect(latestRuns(runs, 2).map((r) => r.id)).toEqual(["b", "c"]);
  });
});

describe("adoptionCounts", () => {
  it("counts each person once, and someone on both routes as through Nexus", () => {
    const days = [
      { people: [{ userId: "u1", email: null, calls: 3 }], subscription: [{ userId: "u2", email: null, sessions: 1 }] },
      { people: [{ userId: "u2", email: null, calls: 1 }], subscription: [{ userId: "u3", email: null, sessions: 2 }] },
    ];
    expect(adoptionCounts(days)).toEqual({ nexus: 2, own: 1 });
    expect([people(1), people(2)]).toEqual(["1 person", "2 people"]);
  });
});
