import { memoryFiles, type AgentMemoryNote, type AgentMemoryView } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { editDraft, exportText, fileAt, forgotText, learnedFrom, lineRemoval, lineSource, memoryFileHref, memoryHead } from "./memory-words";

// The Assistant's Memory on the phone (A16): the files, each fact's source, Delete and Show where.

const note = (id: string, text: string, extra: Partial<AgentMemoryNote> = {}): AgentMemoryNote => ({ id, text, createdAt: "2026-10-01T09:00:00.000Z", audience: "dm", ...extra });
const view = (notes: AgentMemoryNote[], more: Partial<AgentMemoryView> = {}): AgentMemoryView => ({ notes, savedModel: "auto", ...more });

const ana = note("n1", "- **Ana R.**: my manager at Acme.\n- Dev L.: works with me on cortex-sim.", { kind: "people", source: { kind: "told", surface: "api" } });
const dinner = note("n2", "Nopa for four.", { source: { kind: "conversation", errand: { id: "i4", goal: "Dinner Friday" } } });
const old = note("n3", "Aisle seats.", { kind: "preferences" });

describe("the Memory list", () => {
  it("says whose memory, how many facts, and whether learning is on", () => {
    expect(memoryHead(view([ana, dinner, old], { learning: { paused: false }, controls: true }), "Personal")).toBe("Personal · 4 facts · learning on");
    expect(memoryHead(view([old], { learning: { paused: true } }), "Acme")).toBe("Acme · 1 fact · learning paused");
    expect(memoryHead(view([]), undefined)).toBe("0 facts");
  });

  it("opens a file by its path, and finds none once it is gone", () => {
    expect(memoryFileHref("errands/dinner-friday.md")).toEqual({ pathname: "/memory-file", params: { path: "errands/dinner-friday.md" } });
    expect(fileAt(view([ana, dinner]), "errands/dinner-friday.md")?.title).toBe("Dinner Friday");
    expect(fileAt(view([ana]), "errands/dinner-friday.md")).toBeUndefined();
  });
});

describe("a memory file", () => {
  const [people] = memoryFiles([ana]);
  const [first, second] = people!.lines;

  it("shows each fact with where it came from and its day", () => {
    expect(first!.text).toBe("Ana R.: my manager at Acme.");
    expect(lineSource(first!)).toBe("You told it in Nexus · 1 Oct");
    expect(lineSource(memoryFiles([old])[0]!.lines[0]!)).toBe("DM · 1 Oct");
  });

  it("deletes a line and keeps the rest of its note with the controls on, else forgets the whole note", () => {
    expect(lineRemoval(second!, true, "Nexus")).toEqual({ title: "Delete this line?", body: "Nexus keeps the other lines of its note.", edit: "Ana R.: my manager at Acme." });
    expect(lineRemoval(second!, false, "Nexus")).toEqual({ title: "Delete its note?", body: "The line is one of 2 in a note. Nexus forgets the whole note, on every surface." });
    expect(lineRemoval(memoryFiles([old])[0]!.lines[0]!, true, "Ava").edit).toBeUndefined();
  });

  it("says where a line was learned, and where Nexus uses it", () => {
    expect(learnedFrom(first!, "Nexus")).toBe("You told it in Nexus · 1 Oct.\n\nDM. Used only in your direct messages with Nexus.");
    expect(learnedFrom(memoryFiles([old])[0]!.lines[0]!, "Ava")).toContain("Ava did not keep where this came from.");
  });

  it("edits a note as its lines, without the Markdown markers", () => {
    expect(editDraft(first!)).toBe("Ana R.: my manager at Acme.\nDev L.: works with me on cortex-sim.");
  });
});

describe("the export and the status lines", () => {
  it("shares each Markdown file under its path where a share takes text", () => {
    const shared = exportText(view([ana, dinner]));
    expect(shared).toContain("<!-- people.md -->\n# People");
    expect(shared).toContain("<!-- errands/dinner-friday.md -->\n# Dinner Friday");
  });

  it("says what it forgot", () => {
    expect(forgotText("Nexus", "note", 1)).toBe("Nexus forgot the note.");
    expect(forgotText("Nexus", "all", 3)).toBe("Nexus forgot 3 notes.");
  });
});
