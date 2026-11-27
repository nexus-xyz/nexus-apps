import { describe, expect, it } from "vitest";
import {
  AGENT_ERRAND_HANDOFF_CHARS,
  ERRAND_HANDED_OFF,
  errandCanHandOff,
  errandHandedOffBody,
  errandHandoff,
  errandHandoffLink,
  errandHandoffOf,
  errandHandoffRepo,
} from "./agent-errands";
import { AGENT_ERRAND_GOAL_CHARS, errandActedOf, errandEditBody, errandGoalBody, errandHistoryOf, errandListOf, errandMessageBody, errandName, errandOpenedOf, errandProjects, errandReplyOf, errandsNeedingYou, errandsOff, errandViewOf, searchErrands } from "./agent-errands";

// The agent API's errands as every app reads them (A12): strictly, leaving out a row that is not one.

const ROW = { id: "i3", goal: "Dinner Friday", status: "needs-you", need: "approval", detail: "Book Nopa", steps: 1, deadline: "2026-11-13", at: "2026-11-10T08:02:00.000Z" };
const VIEW = {
  errand: ROW,
  constraints: ["No shellfish", 7],
  budgetUsd: "120",
  surface: "slack",
  createdAt: "2026-11-10T08:02:00.000Z",
  steps: [{ id: "s1", kind: "web", text: "6 places, compared", urls: ["https://example.test", 1], at: "2026-11-10T08:20:00.000Z" }, { id: "s2" }],
  outcome: { status: "done", at: "2026-11-10T09:00:00.000Z", evidence: [{ kind: "link", url: "https://example.test/r" }, { kind: "citation", text: "Booked" }, { kind: "other" }] },
  actions: ["done", "cancel", "archive"],
};

describe("errands on the wire", () => {
  it("reads the list, and leaves out a row that is not one", () => {
    expect(errandListOf({ errands: [ROW, { ...ROW, id: "../x" }, { ...ROW, status: "paused" }, null] })).toEqual([ROW]);
    expect(errandListOf({ errands: [{ ...ROW, need: "other", steps: -1, deadline: "Friday" }] })).toEqual([{ id: "i3", goal: "Dinner Friday", status: "needs-you", detail: "Book Nopa", steps: 0, at: ROW.at }]);
    expect(errandListOf({})).toBeUndefined();
    expect(errandListOf([ROW])).toBeUndefined();
  });

  it("reads one errand, keeping only what it knows", () => {
    expect(errandViewOf(VIEW)).toEqual({
      errand: ROW,
      constraints: ["No shellfish"],
      budgetUsd: "120",
      surface: "slack",
      createdAt: VIEW.createdAt,
      steps: [{ id: "s1", kind: "web", text: "6 places, compared", urls: ["https://example.test"], at: "2026-11-10T08:20:00.000Z" }],
      outcome: { status: "done", at: "2026-11-10T09:00:00.000Z", evidence: [{ kind: "link", url: "https://example.test/r" }, { kind: "citation", text: "Booked" }] },
      actions: ["done", "cancel"],
      thread: [],
    });
    expect(errandViewOf({ ...VIEW, errand: { id: "i3" } })).toBeUndefined();
    expect(errandViewOf({ ...VIEW, steps: undefined })).toBeUndefined();
  });

  it("reads what an action did", () => {
    expect(errandActedOf({ state: "discarded" })).toEqual({ state: "discarded" });
    expect(errandActedOf({ state: "done", errand: VIEW })?.state).toBe("done");
    expect(errandActedOf({ state: "done" })).toBeUndefined();
    expect(errandActedOf({ state: "archived", errand: VIEW })).toBeUndefined();
  });

  it("reads a new errand, and writes the body that starts one (A21·e)", () => {
    expect(errandOpenedOf({ state: "opened", errand: VIEW })?.errand.errand.id).toBe("i3");
    expect(errandOpenedOf({ state: "opened" })).toBeUndefined();
    expect(errandOpenedOf({ state: "done", errand: VIEW })).toBeUndefined();
    expect(errandGoalBody("  Dinner Friday ")).toBe('{"goal":"Dinner Friday"}');
    expect(JSON.parse(errandGoalBody("x".repeat(AGENT_ERRAND_GOAL_CHARS + 9))).goal).toHaveLength(AGENT_ERRAND_GOAL_CHARS);
  });

  it("reads an errand's thread, its name, pin and archive, and leaves out what is not one", () => {
    const answer = { kind: "answer", text: "Book Nopa?", model: "m", costUsd: 0.01 };
    const view = errandViewOf({
      ...VIEW,
      errand: { ...ROW, title: "Dinner", pinned: true, archived: "yes" },
      thread: [
        { role: "user", text: "Is 7:30 free?", at: ROW.at },
        { role: "assistant", text: "Book Nopa?", at: ROW.at, messages: [answer, { kind: "script" }, null] },
        { role: "system", text: "x", at: ROW.at },
        { role: "user", text: 3, at: ROW.at },
      ],
    });
    expect(view?.errand).toMatchObject({ title: "Dinner", pinned: true });
    expect(view?.errand).not.toHaveProperty("archived");
    expect(view?.thread).toEqual([
      { role: "user", text: "Is 7:30 free?", at: ROW.at },
      { role: "assistant", text: "Book Nopa?", at: ROW.at, messages: [answer] },
    ]);
    expect(errandName({ goal: "Dinner Friday" })).toBe("Dinner Friday");
    expect(errandName({ goal: "Dinner Friday", title: "Dinner" })).toBe("Dinner");
  });

  it("reads the history, the errands that need the person, and a search of them", () => {
    const done = { ...ROW, id: "i1", goal: "Car registration", status: "done", need: undefined, title: "DMV" };
    expect(errandHistoryOf({ errands: [], history: [done, { id: "x" }] })).toEqual([expect.objectContaining({ id: "i1", title: "DMV" })]);
    expect(errandHistoryOf({ errands: [] })).toEqual([]);
    const rows = errandListOf({ errands: [ROW, { ...ROW, id: "i4", pinned: true }, { ...ROW, id: "i5", status: "running", need: undefined }] })!;
    expect(errandsNeedingYou(rows).map((row) => row.id)).toEqual(["i4", "i3"]);
    const history = errandHistoryOf({ history: [done, { ...ROW, id: "i2", goal: "Offsite venue", status: "done" }] });
    expect(searchErrands(history, "dmv").map((row) => row.id)).toEqual(["i1"]);
    expect(searchErrands(history, "  OFFSITE venue ").map((row) => row.id)).toEqual(["i2"]);
    expect(searchErrands(history, "")).toHaveLength(2);
  });

  it("writes an edit and a reply, and reads a reply's body", () => {
    expect(JSON.parse(errandEditBody({ title: null, pinned: true }))).toEqual({ title: null, pinned: true });
    expect(JSON.parse(errandMessageBody("hi"))).toEqual({ message: "hi" });
    const reply = errandReplyOf({ conversation: "errand-i3", messages: [{ kind: "answer", text: "ok", model: "m", costUsd: 0 }, { kind: "x" }], errand: VIEW });
    expect(reply?.messages).toHaveLength(1);
    expect(reply?.errand?.errand.id).toBe("i3");
    expect(errandReplyOf({ messages: [], errand: { bad: true } })).toEqual({ messages: [] });
    expect(errandReplyOf({})).toBeUndefined();
  });

  it("reads an errand's project, writes one, and lists the projects by name (A14)", () => {
    expect(errandListOf({ errands: [{ ...ROW, project: "Fundraise" }, { ...ROW, id: "i4", project: "  " }, { ...ROW, id: "i5", project: 3 }] })?.map((row) => row.project)).toEqual(["Fundraise", undefined, undefined]);
    expect(JSON.parse(errandEditBody({ project: null }))).toEqual({ project: null });
    expect(JSON.parse(errandGoalBody(" Book meetings ", " Fundraise "))).toEqual({ goal: "Book meetings", project: "Fundraise" });
    expect(JSON.parse(errandGoalBody("Book meetings", "  "))).toEqual({ goal: "Book meetings" });
    const rows = errandListOf({ errands: [{ ...ROW, project: "Northwind Labs" }, { ...ROW, id: "i4", project: "Fundraise" }, { ...ROW, id: "i5" }] })!;
    const history = errandHistoryOf({ history: [{ ...ROW, id: "i6", project: "Fundraise" }] });
    expect(errandProjects(rows, history)).toEqual(["Fundraise", "Northwind Labs"]);
  });

  it("knows when the agent API has no errands to show", () => {
    expect(errandsOff(503, { error: "errands_off" })).toBe(true);
    expect(errandsOff(404)).toBe(true);
    expect(errandsOff(503, { error: "errands_unavailable" })).toBe(false);
    expect(errandsOff(500)).toBe(false);
  });
});

describe("an errand handed off to a session (A12·e)", () => {
  const base = errandViewOf({
    errand: { id: "i4", goal: "Fix the flaky decoder test", status: "running", steps: 2, at: "2026-11-10T08:02:00.000Z", deadline: "2026-11-20" },
    constraints: ["Keep the public API"],
    surface: "api",
    createdAt: "2026-11-10T08:02:00.000Z",
    steps: [
      { id: "s1", kind: "code", text: "Found the test in https://github.com/nexus-xyz/cortex-sim/blob/main/t.rs", urls: ["https://github.com/nexus-xyz/cortex-sim/pull/7"], at: "2026-11-10T08:10:00.000Z" },
      { id: "s2", kind: "code", text: ERRAND_HANDED_OFF, urls: [], at: "2026-11-10T08:12:00.000Z" },
    ],
    actions: ["done", "cancel"],
    thread: [
      { role: "user", text: "It fails one run in ten. Mail ana@example.com if you need the logs.", at: "2026-11-10T08:05:00.000Z" },
      { role: "assistant", text: "I think it is a race in the setup.", at: "2026-11-10T08:06:00.000Z", messages: [{ kind: "pending_action", card: "c1", summary: "Email Ana" }] },
    ],
  })!;

  it("writes the goal, the constraints, the steps so far and the last turns, and names the repository", () => {
    const handoff = errandHandoff(base);
    expect(handoff.repo).toBe("nexus-xyz/cortex-sim");
    expect(handoff.prompt).toBe(
      [
        "Continue my errand in this session.",
        "",
        "Goal: Fix the flaky decoder test",
        "It keeps to:",
        "- Keep the public API",
        "Its last day: 2026-11-20",
        "Repository: nexus-xyz/cortex-sim",
        "",
        "What Nexus did so far:",
        "- Found the test in https://github.com/nexus-xyz/cortex-sim/blob/main/t.rs",
        "",
        "The errand's thread, newest last:",
        "Me: It fails one run in ten. Mail [email] if you need the logs.",
        "Nexus: I think it is a race in the setup.",
      ].join("\n"),
    );
    // The Assistant's own name, and the person's name for the errand.
    expect(errandHandoff({ ...base, errand: { ...base.errand, title: "Decoder" } }, "Iris").prompt).toMatch(/^Continue my errand "Decoder" in this session\.[\s\S]*What Iris did so far:[\s\S]*Iris: I think/);
  });

  it("finds the repository in the project, a link or 'repo owner/name', and never in a bare a/b", () => {
    const loose = { ...base, steps: [], thread: [] };
    expect(errandHandoffRepo({ ...loose, errand: { ...base.errand, project: "nexus-xyz/nexus" } })).toBe("nexus-xyz/nexus");
    expect(errandHandoffRepo({ ...loose, errand: { ...base.errand, goal: "Tidy the repo nexus-xyz/wiki.git." } })).toBe("nexus-xyz/wiki");
    expect(errandHandoffRepo({ ...loose, errand: { ...base.errand, goal: "Plan and/or book it" } })).toBeUndefined();
    expect(errandHandoff({ ...loose, errand: { ...base.errand, goal: "Book dinner" } }).repo).toBeUndefined();
  });

  it("takes secrets and control characters out, and stays under the desktop's bound", () => {
    const secret = errandHandoff({ ...base, constraints: ["Use sk-abcdefghijklmnop1234\u0007 only"] }).prompt;
    expect(secret).toContain("- Use [redacted] only");
    const long = errandHandoff({ ...base, thread: Array.from({ length: 40 }, (_, at) => ({ role: "user" as const, text: `${at} ${"x".repeat(5_000)}`, at: base.createdAt })) });
    expect(long.prompt.length).toBeLessThanOrEqual(AGENT_ERRAND_HANDOFF_CHARS);
    expect(long.prompt).toContain("Me: 39 ");
    expect(long.prompt).not.toContain("Me: 33 ");
  });

  it("writes the desktop's draft link, reads the API's body strictly, and writes the step's body", () => {
    expect(errandHandoffLink({ prompt: "Fix it now", repo: "nexus-xyz/cortex-sim" })).toBe("nexus://new?prompt=Fix%20it%20now&repo=nexus-xyz%2Fcortex-sim");
    expect(errandHandoffLink({ prompt: "Fix it" })).toBe("nexus://new?prompt=Fix%20it");
    expect(errandHandoffOf({ prompt: "Go", repo: "a/b" })).toEqual({ prompt: "Go", repo: "a/b" });
    expect(errandHandoffOf({ prompt: "Go", repo: "../x" })).toEqual({ prompt: "Go" });
    expect(errandHandoffOf({ prompt: "x".repeat(AGENT_ERRAND_HANDOFF_CHARS + 1) })).toBeUndefined();
    expect(errandHandoffOf({ prompt: " " })).toBeUndefined();
    expect(errandHandedOffBody("https://nexus.test/app/acme/sessions/s1")).toBe('{"url":"https://nexus.test/app/acme/sessions/s1"}');
    expect(errandHandedOffBody("javascript:alert(1)")).toBe("{}");
    expect(errandHandedOffBody()).toBe("{}");
    expect(errandCanHandOff(base)).toBe(true);
    expect(errandCanHandOff({ actions: ["confirm", "cancel"] })).toBe(false);
    expect(errandCanHandOff({ actions: [] })).toBe(false);
  });
});
