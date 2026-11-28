import { describe, expect, it } from "vitest";
import type { TimelineItem } from "./code-timeline";
import { HANDOFF_LIMIT, handoffPrompt } from "./session-handoff";

describe("the editable handoff preview", () => {
  it("seeds visible prompt and answer text and excludes raw tools, approval details and image IDs", () => {
    expect(handoffPrompt([
      { kind: "user", key: "1", text: "Fix the mobile test.", images: ["private-image-id"] },
      { kind: "work", key: "2", steps: [{ key: "tool", kind: "tool", verb: "Read", detail: "private-path", code: true, state: "done" }] },
      { kind: "approval", key: "3", text: "private-approval" },
      { kind: "text", key: "4", text: "The clock also reads twice." },
    ])).toBe("Continue this work using the context below. Ask me for any missing details.\n\nI asked\nFix the mobile test.\n\nThe agent replied\nThe clock also reads twice.");
  });

  it("keeps a bounded preview of recent text in order", () => {
    const messages: TimelineItem[] = Array.from({ length: 7 }, (_, n) => ({
      kind: "text", key: String(n), text: `message ${n} ${"x".repeat(2400)}`,
    }));
    const prompt = handoffPrompt(messages);
    expect(prompt).not.toContain("message 0");
    expect(prompt.indexOf("message 1")).toBeLessThan(prompt.indexOf("message 6"));
    expect(prompt.length).toBeLessThan(HANDOFF_LIMIT);
  });
});
