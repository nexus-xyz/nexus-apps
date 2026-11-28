import type { AgentErrand, AgentErrandView } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import {
  actedText,
  chipDetail,
  chipLabel,
  chipLine,
  deadlineText,
  editedText,
  errandEntries,
  errandGroups,
  errandLine,
  errandMenu,
  errandsOn,
  errandStateText,
  historyLine,
  menuEdit,
  moveEdit,
  movedText,
  inProject,
  overviewTitle,
  renameEdit,
  replyPlaceholder,
  stripAfter,
  whereToAnswer,
} from "./agent-errands";

// Errands on top of the phone's Assistant (A12): which errands the strip shows, each chip's words, and where a card or
// a question waits.

const AT = "2026-11-10T08:02:00.000Z";
const row = (errand: Partial<AgentErrand>): AgentErrand => ({ id: "i1", goal: "Dinner Friday", status: "running", steps: 0, at: AT, ...errand });

describe("the errands strip", () => {
  it("is on only with the build's flag", () => {
    expect(errandsOn("on")).toBe(true);
    expect(errandsOn(" ON ")).toBe(true);
    expect(errandsOn(undefined)).toBe(false);
    expect(errandsOn("1")).toBe(false);
  });

  it("shows the open errands in the API's order, and hides when there is none, it is off or it fails", () => {
    const errands = [row({ id: "i1", status: "needs-you", need: "approval" }), row({ id: "i2" }), row({ id: "i3", status: "done" })];
    expect(stripAfter({ kind: "list", errands })).toEqual({ kind: "on", errands: errands.slice(0, 2) });
    expect(stripAfter({ kind: "list", errands: [row({ status: "done" })] })).toEqual({ kind: "hidden" });
    expect(stripAfter({ kind: "off" })).toEqual({ kind: "hidden" });
    expect(stripAfter({ kind: "failed", failure: { status: 503 } })).toEqual({ kind: "hidden" });
    expect(stripAfter("unreachable")).toEqual({ kind: "hidden" });
  });

  it("leaves an archived errand out, and puts a pinned one first", () => {
    const errands = [row({ id: "i1" }), row({ id: "i2", pinned: true }), row({ id: "i3", archived: true })];
    expect(stripAfter({ kind: "list", errands })).toEqual({ kind: "on", errands: [errands[1], errands[0]] });
  });

  it("says what each chip needs, as VoiceOver reads it too", () => {
    expect(chipLine(row({ status: "needs-you", need: "confirm" }))).toBe("Confirm it");
    expect(chipLine(row({ status: "needs-you", need: "approval" }))).toBe("Needs your approval");
    expect(chipLine(row({ status: "needs-you", need: "question" }))).toBe("Asked you");
    expect(chipLine(row({ steps: 1 }))).toBe("1 step");
    expect(chipLine(row({ steps: 3 }))).toBe("3 steps");
    expect(chipLabel(row({ status: "needs-you", need: "approval" }))).toBe("Dinner Friday, errand, needs your approval");
    // A name the person gave it names the chip.
    expect(chipLabel(row({ title: "Nopa" }))).toBe("Nopa, errand, started");
    // A2·A: a chip shows its state by shape; only a running errand adds a quiet word after its goal.
    expect(chipDetail(row({ status: "needs-you", need: "approval", steps: 2 }))).toBeUndefined();
    expect(chipDetail(row({ status: "running", steps: 3 }))).toBe("3 steps");
    expect(chipDetail(row({ status: "running", steps: 0 }))).toBeUndefined();
  });

  it("says where a card or a question waits, in Nexus UI's words", () => {
    expect(whereToAnswer({ errand: row({ status: "needs-you", need: "approval" }), surface: "slack" })).toBe("The change waits for your approval in Slack. Approve or cancel it there.");
    expect(whereToAnswer({ errand: row({ status: "needs-you", need: "question" }), surface: "slack" })).toBe("Reply in its Slack thread.");
    expect(whereToAnswer({ errand: row({ status: "needs-you", need: "confirm" }), surface: "slack" })).toBeUndefined();
  });

  it("says what an action did, and reads a deadline against today", () => {
    expect(actedText({ state: "discarded" })).toBe("The draft is deleted.");
    const now = Date.parse("2026-11-10T15:00:00.000Z");
    expect(deadlineText("2026-11-11", now)).toBe("Due tomorrow");
    expect(deadlineText("2026-11-13", now)).toBe("Due Nov 13");
    expect(deadlineText("2026-11-01", now)).toBe("Past its deadline");
  });
});

const view = (more: Partial<AgentErrandView> = {}): AgentErrandView => ({
  errand: row({}),
  constraints: [],
  surface: "api",
  createdAt: "2026-11-10T08:02:00.000Z",
  steps: [],
  actions: ["done", "cancel"],
  thread: [],
  ...more,
});

describe("an errand as its own thread", () => {
  it("is its goal, its steps and its turns, by time, with the agent's text when a turn has no messages", () => {
    const entries = errandEntries(
      view({
        steps: [{ id: "1", kind: "web", text: "Compared 6 places", urls: ["https://nopa.com"], at: "2026-11-10T08:10:00.000Z" }],
        thread: [
          { role: "assistant", text: "Book Nopa?", at: "2026-11-10T08:20:00.000Z" },
          { role: "user", text: "yes", at: "2026-11-10T08:21:00.000Z" },
        ],
      }),
    );
    expect(entries.map((entry) => entry.kind)).toEqual(["you", "step", "nexus", "you"]);
    expect(entries[0]).toMatchObject({ text: "Dinner Friday", at: Date.parse("2026-11-10T08:02:00.000Z") });
    expect(entries[2]).toMatchObject({ messages: [{ kind: "answer", text: "Book Nopa?" }] });
  });

  it("says its state, and replies about it by name, cut to fit", () => {
    expect(errandStateText(view())).toBe("Working on it");
    expect(errandStateText(view({ errand: row({ status: "needs-you" }) }))).toBe("Needs you");
    expect(errandStateText(view({ errand: row({ archived: true }) }))).toBe("Archived");
    expect(errandStateText(view({ outcome: { status: "partly_done", at: AT, evidence: [] } }))).toBe("Partly done");
    expect(replyPlaceholder(row({}))).toBe("Reply about Dinner Friday");
    expect(replyPlaceholder(row({ title: "A very long name for a dinner on Friday night" }))).toBe("Reply about A very long name for a dinner o…");
  });

  it("has A2's long-press menu: Pin, Rename, Mark done while open, Archive", () => {
    expect(errandMenu(row({})).map((item) => item.label)).toEqual(["Pin", "Rename", "Move to project", "Mark done", "Archive"]);
    expect(errandMenu(row({ pinned: true, status: "done" })).map((item) => item.label)).toEqual(["Unpin", "Rename", "Move to project", "Archive"]);
    expect(errandMenu(row({ archived: true })).map((item) => item.label)).toEqual(["Pin", "Rename", "Move to project", "Move out of the archive"]);
    expect(menuEdit("pin", row({ pinned: true }))).toEqual({ pinned: false });
    expect(menuEdit("done", row({}))).toEqual({ done: true });
    expect(menuEdit("archive", row({}))).toEqual({ archived: true });
    expect(renameEdit("  Nopa ")).toEqual({ title: "Nopa" });
    expect(renameEdit("  ")).toEqual({ title: null });
    expect(editedText({ archived: true })).toBe("Archived. It is in the history now.");
    expect(editedText({ title: null })).toBe("The name is gone.");
    expect(editedText({ title: "Nopa" })).toBe("Renamed.");
  });

  it("moves an errand into a project and out of it, and says where a filed one is (A14)", () => {
    expect(moveEdit(" Fundraise ")).toEqual({ project: "Fundraise" });
    expect(moveEdit("x".repeat(80)).project).toHaveLength(60);
    expect(moveEdit("  ")).toEqual({ project: null });
    expect(moveEdit(null)).toEqual({ project: null });
    expect(inProject(row({ project: "Fundraise" }))).toBe("in Fundraise");
    expect(inProject(row({}))).toBeUndefined();
    expect(movedText("Fundraise", row({}))).toBe("Dinner Friday is in Fundraise now.");
    expect(movedText(null, row({ title: "Nopa" }))).toBe("Nopa is loose again, under the Assistant.");
    expect(editedText({ project: "Fundraise" })).toBe("The errand is in Fundraise now.");
    expect(historyLine(row({ status: "done", project: "Fundraise" }))).toBe("in Fundraise · Done");
  });

  it("says how many need you, groups the page, and lines up the History", () => {
    expect(overviewTitle(0)).toBeUndefined();
    expect(overviewTitle(1)).toBe("1 errand needs you");
    expect(overviewTitle(2)).toBe("2 errands need you");
    expect(errandLine(row({ status: "needs-you", need: "approval", detail: "Book Nopa" }))).toBe("Approve: Book Nopa");
    expect(errandLine(row({ steps: 2 }))).toBe("2 steps so far");
    expect(historyLine(row({ status: "done", steps: 3, archived: true }))).toBe("Archived · Done in 3 steps");
    const groups = errandGroups([row({ id: "i1", status: "done" }), row({ id: "i2" }), row({ id: "i3", status: "needs-you" })]);
    expect(groups.map((group) => [group.title, group.errands.map((errand) => errand.id)])).toEqual([
      ["Needs you", ["i3"]],
      ["Running", ["i2"]],
      ["Done this week", ["i1"]],
    ]);
  });
});
