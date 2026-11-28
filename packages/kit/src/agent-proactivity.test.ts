import { describe, expect, it } from "vitest";
import { briefOf } from "./agent-brief";
import { fewerLikeThis, levelOfAgentProactivity, proactivityRead, proactivityViewOf, turnOff, whyOf, type AgentProactivityView } from "./agent-proactivity";

const VIEW: AgentProactivityView = { level: "balanced", ownLevel: false, agentProactivity: "ask_first", muted: [], quietHours: null, batchMinutes: 15 };
const WHY = { kind: "brief", level: "balanced", lines: ["You turned on the morning brief: “brief on at 8:00” on 12 Oct.", "Proactive is set to Balanced."] };

describe("proactivity (A10)", () => {
  it("reads the view strictly", () => {
    expect(proactivityViewOf(VIEW)).toEqual(VIEW);
    const hours = { ...VIEW, quietHours: { start: "22:00", end: "07:00", timezone: "America/New_York" } };
    expect(proactivityViewOf(hours)).toEqual(hours);
    for (const body of [undefined, {}, { ...VIEW, level: "loud" }, { ...VIEW, muted: ["spam"] }, { ...VIEW, quietHours: { start: "7", end: "08:00", timezone: "UTC" } }, { ...VIEW, agentProactivity: "maybe" }]) {
      expect(proactivityViewOf(body)).toBeUndefined();
    }
  });

  it("reads off for a 404 or 503 proactivity_off, and a failure otherwise", () => {
    expect(proactivityRead(200, VIEW)).toEqual({ kind: "view", view: VIEW });
    expect(proactivityRead(404, {})).toEqual({ kind: "off" });
    expect(proactivityRead(503, { error: "proactivity_off" })).toEqual({ kind: "off" });
    expect(proactivityRead(503, { error: "proactivity_unavailable" })).toEqual({ kind: "failed", status: 503 });
    expect(proactivityRead(200, { level: 1 })).toEqual({ kind: "failed", status: 502 });
  });

  it("lowers the dial one step for Fewer like this, and turns the kind off at Quiet or on Turn off", () => {
    expect(fewerLikeThis({ level: "hands_on", muted: [] }, "brief")).toEqual({ level: "balanced" });
    expect(fewerLikeThis({ level: "balanced", muted: [] }, "brief")).toEqual({ level: "quiet" });
    expect(fewerLikeThis({ level: "quiet", muted: ["birthday"] }, "watch")).toEqual({ muted: ["watch", "birthday"] });
    expect(turnOff({ muted: ["birthday"] }, "brief")).toEqual({ muted: ["brief", "birthday"] });
  });

  it("maps W4.3's AgentProactivity one to one", () => {
    expect(["never", "ask_first", "always"].map((value) => levelOfAgentProactivity(value as "never"))).toEqual(["quiet", "balanced", "hands_on"]);
  });

  it("reads a message's reasons, and a brief keeps them; a bad one is left out", () => {
    expect(whyOf(WHY)).toEqual(WHY);
    expect(whyOf({ ...WHY, lines: [] })).toBeUndefined();
    expect(whyOf({ ...WHY, kind: "spam" })).toBeUndefined();
    const brief = { day: "2026-11-17", at: "2026-11-17T13:00:00.000Z", head: "Morning.", lines: [] };
    expect(briefOf({ brief: { ...brief, why: WHY } })).toEqual({ ...brief, why: WHY });
    expect(briefOf({ brief: { ...brief, why: { kind: "brief" } } })).toEqual(brief);
  });
});
