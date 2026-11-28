import type { SessionEvent } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { diffRows, duration, foldTimeline, groupTranscriptSteps, summarize, turnEnding, verbFor } from "./code-timeline";

const turn: SessionEvent[] = [
  { kind: "user", text: "Make the GRU hold 5 ms at p99 on rig A." },
  { kind: "turnStart", turnId: "t1" },
  { kind: "thinking", text: "The decoder loads K one float at a time." },
  { kind: "tool", id: "c1", name: "Edit", input: { file_path: "/Users/maya/cortex-sim/src/decoder.py", old_string: "a\nb", new_string: "a\nc\nd" } },
  { kind: "toolResult", id: "c1", output: "ok" },
  { kind: "tool", id: "c2", name: "Bash", input: { command: "python bench.py --rig a" } },
  { kind: "usage", model: "anthropic/claude-sonnet-5-5", inputTokens: 900, outputTokens: 100, cacheReadTokens: 0, cacheWriteTokens: 0, costUsd: 0.06 },
];

describe("the phone's timeline", () => {
  it("folds a turn as P57·C draws it: the prompt, the turn's line, one card of work", () => {
    const items = foldTimeline(turn, true);
    expect(items.map((item) => item.kind)).toEqual(["user", "turn", "work"]);
    expect(items[1]).toMatchObject({ kind: "turn", model: "anthropic/claude-sonnet-5-5", costUsd: 0.06, tokens: 1000, status: "running" });
    const work = items[2]!;
    if (work.kind !== "work") throw new Error("expected work");
    expect(work.steps.map((step) => [step.verb, step.detail, step.extra, step.state])).toEqual([
      ["Thought", "The decoder loads K one float at a time.", undefined, "done"],
      ["Edited", "src/decoder.py", "+3 −2", "done"],
      ["Running", "python bench.py --rig a", undefined, "running"],
    ]);
  });

  it("uses the stored turn end for the final answer's rating", () => {
    const partial = [
      { kind: "turnStart", turnId: "run" },
      { kind: "text", text: "Progress update" },
      { kind: "text", text: "Final answer" },
    ] as const;
    const answer = (events: Parameters<typeof foldTimeline>[0]) =>
      foldTimeline(events, false, new Map([[partial.length, 17]])).filter((item) => item.kind === "text");
    expect(answer(partial).map((item) => item.answerSeq)).toEqual([undefined, undefined]);
    expect(answer([...partial, { kind: "turnEnd", status: "completed" }])
      .map((item) => item.answerSeq)).toEqual([undefined, 17]);
    expect(answer([...partial, { kind: "turnEnd", status: "interrupted" }])
      .map((item) => item.answerSeq)).toEqual([undefined, undefined]);
  });

  it("stops a step that never got its result once the turn ended, or the session is not live", () => {
    const ended = foldTimeline([...turn, { kind: "turnEnd", turnId: "t1", status: "interrupted", durationMs: 72_000 }], true);
    const work = ended.find((item) => item.kind === "work");
    expect(work?.kind === "work" && work.steps.at(-1)?.state).toBe("idle");
    expect(work?.kind === "work" && work.steps.at(-1)?.verb).toBe("Run");
    expect(ended.find((item) => item.kind === "turn")).toMatchObject({ status: "interrupted", durationMs: 72_000 });
    const stale = foldTimeline(turn, false).find((item) => item.kind === "work");
    expect(stale?.kind === "work" && stale.steps.at(-1)?.state).toBe("idle");
  });

  it("asks while an approval waits, and says denied once it is", () => {
    const asking: SessionEvent[] = [
      { kind: "turnStart" },
      { kind: "tool", id: "c1", name: "Bash", input: { command: "git push origin decoder-gru" } },
      { kind: "approval", requestId: "r1", toolUseId: "c1", tool: "Bash", input: { command: "git push origin decoder-gru" } },
    ];
    const step = (events: SessionEvent[]) => {
      const work = foldTimeline(events, true).find((item) => item.kind === "work");
      return work?.kind === "work" ? work.steps[0] : undefined;
    };
    expect(step(asking)).toMatchObject({ verb: "Wants to run", state: "asking" });
    expect(step([...asking, { kind: "approvalResolved", requestId: "r1", decision: "deny", message: "not now" }])).toMatchObject({ verb: "Run", state: "denied" });
  });

  it("counts a sub-agent's work under its row and keeps its text out of the conversation", () => {
    const items = foldTimeline(
      [
        { kind: "tool", id: "a1", name: "Task", input: { description: "Profile decode", subagent_type: "general" } },
        { kind: "tool", id: "s1", name: "Read", input: { file_path: "/x/a.py" }, parent: "a1" },
        { kind: "tool", id: "s2", name: "Read", input: { file_path: "/x/b.py" }, parent: "a1" },
        { kind: "text", text: "A sub-agent's note", parent: "a1" },
        { kind: "toolResult", id: "a1", output: "done" },
        { kind: "text", text: "It is memory-bound." },
      ],
      false,
    );
    expect(items.map((item) => item.kind)).toEqual(["work", "text"]);
    const work = items[0]!;
    expect(work.kind === "work" && work.steps.map((step) => [step.verb, step.detail, step.extra])).toEqual([["Agent", "Profile decode", "2 steps"]]);
  });

  it("shows the agent's question and how it was answered, never as a step", () => {
    const input = { questions: [{ question: "Which rig?" }] };
    const items = foldTimeline(
      [
        { kind: "tool", id: "q", name: "AskUserQuestion", input },
        { kind: "approval", requestId: "r", toolUseId: "q", tool: "AskUserQuestion", input },
        { kind: "approvalResolved", requestId: "r", decision: "allow", message: "Rig A" },
      ],
      true,
    );
    expect(items).toEqual([{ kind: "question", key: "1", text: "Asked: Which rig? (Rig A)", answered: true }]);
  });
});

describe("a step's words", () => {
  it("says tools as the desktop does", () => {
    expect(summarize("Grep", { pattern: "load_kv" })).toMatchObject({ verb: "Search", detail: "load_kv", code: true });
    expect(summarize("mcp__linear__get_issue", { id: "DEMO-1" })).toMatchObject({ verb: "linear · get_issue", detail: "DEMO-1" });
    expect(verbFor("Run", "asking")).toBe("Wants to run");
    expect(verbFor("linear · get_issue", "done")).toBe("linear · get_issue");
  });

  it("keeps two lines of context around an edit", () => {
    expect(diffRows("a\nb\nc\nd\ne", "a\nb\nc\nX\ne")).toEqual([
      { sign: " ", text: "b" },
      { sign: " ", text: "c" },
      { sign: "-", text: "d" },
      { sign: "+", text: "X" },
      { sign: " ", text: "e" },
    ]);
  });
});

describe("a turn's ending", () => {
  it("says how it ended and how long it took", () => {
    expect(turnEnding({ status: "completed", durationMs: 41_000 })).toBe("Done in 41s");
    expect(turnEnding({ status: "interrupted", durationMs: 108_000 })).toBe("Stopped after 1m 48s");
    expect(turnEnding({ status: "error", durationMs: null })).toBe("Failed");
    expect(turnEnding({ status: "running", durationMs: null })).toBeNull();
    expect(duration(3_720_000)).toBe("1h 2m");
  });
});

it("folds only contiguous completed tool steps and keeps live and failed steps visible", () => {
  const events: SessionEvent[] = [
    { kind: "tool", id: "a", name: "Read", input: { file_path: "a.ts" } },
    { kind: "toolResult", id: "a", output: "a" },
    { kind: "tool", id: "b", name: "Read", input: { file_path: "b.ts" } },
    { kind: "toolResult", id: "b", output: "b" },
    { kind: "thinking", text: "Next I will run the check." },
    { kind: "tool", id: "f", name: "Bash" },
    { kind: "toolResult", id: "f", output: "check failed", isError: true },
    { kind: "tool", id: "l", name: "Bash" },
  ];
  const work = foldTimeline(events, true).find((item) => item.kind === "work");
  if (!work) throw new Error("expected work");
  expect(groupTranscriptSteps(work.steps, "normal").map((group) => group.kind === "routine" ? group.steps.length : group.step.state))
    .toEqual([2, "done", "failed", "running"]);
  expect(groupTranscriptSteps(work.steps, "thinking")).toHaveLength(4);
  expect(groupTranscriptSteps(work.steps, "verbose")).toHaveLength(5);
});

it("keeps all available tool output beyond the four-line preview", () => {
  const lines = Array.from({ length: 214 }, (_, i) => `line ${i + 1}`).join("\n");
  const work = foldTimeline([
    { kind: "tool", id: "test", name: "Bash" },
    { kind: "toolResult", id: "test", output: lines },
  ], false).find((item) => item.kind === "work");
  expect(work?.steps[0]?.body).toMatchObject({ kind: "output", text: lines, truncated: false });
});


describe("turn output cards", () => {
  it("keeps recorded edits and direct test outcomes with the turn that ran them", () => {
    const items = foldTimeline([
      { kind: "turnStart", turnId: "one" },
      { kind: "tool", id: "edit", name: "Edit", input: { file_path: "src/clock.rs", old_string: "early", new_string: "late" } },
      { kind: "toolResult", id: "edit", output: "Saved" },
      { kind: "tool", id: "tests", name: "Bash", input: { command: "/bin/zsh -lc 'cargo test clock'" } },
      { kind: "toolResult", id: "tests", output: "assertion failed", isError: true },
      { kind: "tool", id: "script", name: "Bash", input: { command: "cargo test && git status" } },
      { kind: "toolResult", id: "script", output: "other command failed", isError: true },
      { kind: "tool", id: "unfinished", name: "Bash", input: { command: "npm test" } },
      { kind: "tool", id: "background", name: "Edit", parent: "agent", input: { file_path: "src/background.rs" } },
      { kind: "toolResult", id: "background", parent: "agent", output: "Saved" },
      { kind: "text", text: "All tests passed." },
      { kind: "turnEnd", status: "completed", durationMs: 120_000 },
      { kind: "user", text: "Thanks" },
      { kind: "turnStart", turnId: "two" },
      { kind: "toolResult", id: "unfinished", output: "Late completion" },
      { kind: "text", text: "You're welcome." },
      { kind: "turnEnd", status: "completed" },
    ], false);
    const cards = items.filter((item) => item.kind === "output");
    expect(cards).toHaveLength(1);
    expect(cards[0]).toMatchObject({
      status: "completed",
      durationMs: 120_000,
      edits: [{ detail: "src/clock.rs", state: "done", body: { kind: "diff" } }],
      tests: [{ detail: "cargo test clock", state: "failed", body: { kind: "output", text: "assertion failed" } }],
    });
    expect(items.indexOf(cards[0]!)).toBeGreaterThan(items.findIndex((item) => item.kind === "text"));
  });
});
