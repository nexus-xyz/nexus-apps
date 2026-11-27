import { PROACTIVITY_DIAL, type AgentProactivityView } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { batchLine, clockOf, dialText, failedText, mutedLine, quietHoursOf, quietText, stepLine } from "./proactivity-words";

// Proactivity on the phone (A10): the dial's rows, quiet hours from typed times, and what each change says.

const VIEW: AgentProactivityView = { level: "balanced", ownLevel: true, agentProactivity: "ask_first", muted: [], quietHours: null, batchMinutes: 15 };

describe("Proactivity on the phone", () => {
  it("reads a typed time as hours and minutes, and refuses one that is not", () => {
    expect(clockOf("22:00")).toBe("22:00");
    expect(clockOf("7:30")).toBe("07:30");
    expect(clockOf("0730")).toBe("07:30");
    expect(clockOf("7.30")).toBe("07:30");
    expect(clockOf("24:00")).toBeUndefined();
    expect(clockOf("soon")).toBeUndefined();
    expect(quietHoursOf("22:00", "7:00", "Europe/Madrid")).toEqual({ start: "22:00", end: "07:00", timezone: "Europe/Madrid" });
    expect(quietHoursOf("22:00", "22:00", "UTC")).toBe("Quiet hours start and end at different times.");
    expect(quietHoursOf("late", "07:00", "UTC")).toBe("Enter both times as hours and minutes, such as 22:00.");
  });

  it("draws each step of the dial with what it sends and how often", () => {
    expect(PROACTIVITY_DIAL.map(stepLine)).toEqual([
      "Only what you asked for and what's urgent. No brief. ≈ 2 a day",
      "A morning brief, reminders, follow-ups that matter. ≈ 6 a day",
      "Also an evening wrap-up, ideas and nudges it notices. ≈ 12 a day",
    ]);
  });

  it("says what each change did, in Nexus UI's words", () => {
    expect(dialText({ ...VIEW, level: "hands_on" })).toBe("Proactive is set to Hands-on.");
    expect(quietText({ ...VIEW, quietHours: { start: "22:00", end: "07:00", timezone: "UTC" } })).toBe("Quiet from 22:00 to 07:00.");
    expect(quietText(VIEW)).toBe("Quiet hours are off.");
    expect(mutedLine(VIEW)).toBeUndefined();
    expect(mutedLine({ ...VIEW, muted: ["brief", "birthday"] })).toBe("Off at every step: the morning brief, birthdays.");
    expect(batchLine(VIEW)).toBe("More than one push in 15 minutes becomes one. Approvals and what you asked for always come.");
    expect(failedText({ kind: "off" })).toBe("Proactivity is not on for Nexus here yet.");
    expect(failedText({ kind: "failed", status: 500 })).toBe("That was not saved. Try again in a moment.");
  });
});
