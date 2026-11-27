import { describe, expect, it } from "vitest";
import { briefOf, briefRead, type AgentBrief } from "./agent-brief";
import { createAgentClient } from "./agent-chat";

const BRIEF: AgentBrief = {
  day: "2026-11-17",
  at: "2026-11-17T13:00:00.000Z",
  head: "Morning, Maya. Two things today:",
  lines: ["2 events today. The first is Standup at 09:30.", "Gift for Ana is due tomorrow."],
  errands: "Dinner Friday needs you.",
};

describe("the morning brief (A1)", () => {
  it("reads a brief strictly, and null for none today", () => {
    expect(briefOf({ brief: { ...BRIEF, lang: "en" } })).toEqual(BRIEF);
    expect(briefOf({ brief: null })).toBeNull();
    const { errands: _, ...plain } = BRIEF;
    expect(briefOf({ brief: plain })).toEqual(plain);
    for (const body of [
      undefined,
      {},
      { brief: "yes" },
      { brief: { ...BRIEF, day: "today" } },
      { brief: { ...BRIEF, at: "soon" } },
      { brief: { ...BRIEF, head: " " } },
      { brief: { ...BRIEF, lines: ["a", 1] } },
      { brief: { ...BRIEF, lines: ["a", "b", "c", "d"] } },
      { brief: { ...BRIEF, errands: 3 } },
      { brief: { ...BRIEF, head: "x".repeat(401) } },
    ]) {
      expect(briefOf(body), JSON.stringify(body)).toBeUndefined();
    }
  });

  it("reads `off` on a 404 or brief_off, `none`, and a failure otherwise", () => {
    expect(briefRead(200, { brief: BRIEF })).toEqual({ kind: "brief", brief: BRIEF });
    expect(briefRead(200, { brief: null })).toEqual({ kind: "none" });
    expect(briefRead(404, { error: "not_found" })).toEqual({ kind: "off" });
    expect(briefRead(503, { error: "brief_off" })).toEqual({ kind: "off" });
    expect(briefRead(503, { error: "brief_unavailable" })).toEqual({ kind: "failed", status: 503 });
    expect(briefRead(200, { brief: { day: 1 } })).toEqual({ kind: "failed", status: 502 });
  });

  it("is the client's `brief`, with the person's own key", async () => {
    const calls: string[] = [];
    const agent = createAgentClient({
      baseUrl: "https://agents.test/",
      apiKey: "nxk_ann",
      fetch: async (input, init) => {
        calls.push(`${init?.method} ${new URL(String(input)).pathname} ${new Headers(init?.headers).get("authorization")}`);
        return Response.json({ brief: BRIEF });
      },
    });
    expect(await agent.brief()).toEqual({ kind: "brief", brief: BRIEF });
    expect(calls).toEqual(["GET /agent/brief/today Bearer nxk_ann"]);
  });
});
