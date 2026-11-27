import { describe, expect, it } from "vitest";
import { ago, spokenAgo } from "./ago";

const NOW = Date.parse("2026-11-03T12:00:00Z");
const before = (ms: number) => new Date(NOW - ms).toISOString();
const MIN = 60_000;

describe("the drawer's age of a chat", () => {
  it("counts in the largest whole unit", () => {
    expect(ago(before(20_000), NOW)).toBe("now");
    expect(ago(before(2 * MIN), NOW)).toBe("2m");
    expect(ago(before(59 * MIN), NOW)).toBe("59m");
    expect(ago(before(3 * 60 * MIN), NOW)).toBe("3h");
    expect(ago(before(26 * 60 * MIN), NOW)).toBe("1d");
    expect(ago(before(15 * 24 * 60 * MIN), NOW)).toBe("2w");
  });

  it("gives the date past a month, and says now for a clock a little ahead", () => {
    expect(ago("2026-09-03T12:00:00Z", NOW)).toBe("Sep 3");
    expect(ago(new Date(NOW + 5000).toISOString(), NOW)).toBe("now");
    expect(ago("not a date", NOW)).toBe("");
  });
});

describe("spokenAgo", () => {
  it("says the unit in words, singular for one", () => {
    expect(spokenAgo(before(2 * MIN), NOW)).toBe("2 minutes");
    expect(spokenAgo(before(60 * MIN), NOW)).toBe("1 hour");
    expect(spokenAgo(before(26 * 60 * MIN), NOW)).toBe("1 day");
    expect(spokenAgo(before(15 * 24 * 60 * MIN), NOW)).toBe("2 weeks");
  });

  it("says just now, and keeps the date as it is", () => {
    expect(spokenAgo(before(20_000), NOW)).toBe("just now");
    expect(spokenAgo("2026-09-03T12:00:00Z", NOW)).toBe("Sep 3");
  });
});
