import { describe, expect, it } from "vitest";
import { greeting, initials } from "./greeting";

describe("a new chat's greeting", () => {
  const localTime = (hour: number) => new Date(2026, 10, 23, hour);

  it("uses local time and the person's first name", () => {
    expect(greeting("Daniel Marin", localTime(11))).toBe("Good morning, Daniel");
    expect(greeting("  Eduardo  ", localTime(12))).toBe("Good afternoon, Eduardo");
    expect(greeting("Daniel Marin", localTime(18))).toBe("Good evening, Daniel");
  });

  it("greets without a name when the account has none, or only an email", () => {
    expect(greeting(undefined, localTime(12))).toBe("Good afternoon");
    expect(greeting("", localTime(11))).toBe("Good morning");
    expect(greeting("daniel@example.com", localTime(18))).toBe("Good evening");
  });
});

describe("the sidebar's avatar", () => {
  it("takes the first and last words' first letters", () => {
    expect(initials("Daniel Marin")).toBe("DM");
    expect(initials("eduardo cepeda torres")).toBe("ET");
    expect(initials(" Eduardo ")).toBe("E");
  });

  it("has none without a name, or with only an email", () => {
    expect(initials(undefined)).toBeNull();
    expect(initials("daniel@example.com")).toBeNull();
  });
});
