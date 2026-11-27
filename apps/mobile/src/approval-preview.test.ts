import { describe, expect, it } from "vitest";
import { approvalPreview } from "./approval-preview";

describe("an approval's preview, as the desktop's dock shows it", () => {
  it("shows a command in a code box, with the three answers", () => {
    const preview = approvalPreview("Bash", { command: "npm test -- decoder\n" });
    expect(preview.verb).toBe("Wants to run");
    expect(preview.detail).toBe("npm test -- decoder");
    expect(preview.code).toBe(true);
    expect(preview.body).toEqual({ kind: "command", text: "npm test -- decoder" });
    expect(preview.choices).toEqual({ allow: "Allow once", deny: "Deny", always: "Always allow" });
  });

  it("shows an edit as its lines, and a write as added lines", () => {
    const edit = approvalPreview("Edit", { file_path: "/repo/src/codec/decoder.ts", old_string: "a\nb > c\nd", new_string: "a\nb >= c\nd" });
    expect(edit.verb).toBe("Wants to edit");
    expect(edit.detail).toBe("codec/decoder.ts");
    expect(edit.extra).toBe("+3 −3");
    expect(edit.body).toEqual({ kind: "diff", rows: [{ sign: " ", text: "a" }, { sign: "-", text: "b > c" }, { sign: "+", text: "b >= c" }, { sign: " ", text: "d" }] });
    const write = approvalPreview("Write", { file_path: "notes.md", content: "one\ntwo" });
    expect(write.verb).toBe("Wants to write");
    expect(write.body).toEqual({ kind: "diff", rows: [{ sign: "+", text: "one" }, { sign: "+", text: "two" }] });
  });

  it("cuts a long multi-edit after 60 rows and says how many more", () => {
    const edits = Array.from({ length: 40 }, (_, i) => ({ old_string: `old ${i}`, new_string: `new ${i}` }));
    const body = approvalPreview("MultiEdit", { file_path: "a.ts", edits }).body;
    expect(body?.kind).toBe("diff");
    const rows = body?.kind === "diff" ? body.rows : [];
    expect(rows).toHaveLength(61);
    expect(rows.at(-1)?.text).toBe("… 20 more lines");
  });

  it("offers Implement and Keep planning for a plan, and no always", () => {
    const plan = approvalPreview("ExitPlanMode", { plan: "## Fix it\n\n1. Change the check." });
    expect(plan.verb).toBe("Ready to build");
    expect(plan.body).toEqual({ kind: "plan", text: "## Fix it\n\n1. Change the check." });
    expect(plan.choices).toEqual({ allow: "Implement", deny: "Keep planning", always: null });
  });

  it("falls back to the tool's input for another tool, and to nothing for an empty one", () => {
    const fetch = approvalPreview("mcp__linear__save_issue", { title: "Bug" });
    expect(fetch.verb).toBe("linear · save_issue");
    expect(fetch.body).toEqual({ kind: "input", text: '{\n  "title": "Bug"\n}' });
    expect(approvalPreview("Odd", undefined).body).toBeUndefined();
  });
});
