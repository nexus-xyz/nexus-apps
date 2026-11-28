/**
 * A synced coding session folded into what the phone's timeline draws, as the desktop's Code timeline and
 * Nexus UI's (`ui/…/session-timeline/fold.ts`, `tools.ts`) fold the same `SessionEvent`s: the person's prompts, each
 * turn's line (model, cost, how it ended), the agent's text, runs of work (each tool call with its result and the
 * approval it needed, a sub-agent's calls counted under its Task), the agent's questions, notices and errors.
 *
 * The phone draws it as brand v5's P57·C ("Watching"): a run of work is one card of rows, "Edited decoder.py +24 −9",
 * "Running bench.py --rig a". The verbs and summaries are the desktop's, so a session reads the same on both.
 *
 * Pure: no React Native, so vitest runs it. Moving `fold.ts` and `tools.ts` into the kit, so the two share one copy,
 * waits on a change that rewrites them.
 */
import type { SessionEvent } from "@nexus/kit";
import { isFileEdit, testCommand } from "./turn-output";

export const TRANSCRIPT_VIEWS = [
  { id: "normal", label: "Normal", detail: "Keep steps folded" },
  { id: "thinking", label: "Thinking", detail: "Open the agent's thinking" },
  { id: "verbose", label: "Verbose", detail: "Open each step and preview its output" },
] as const;
export type TranscriptView = (typeof TRANSCRIPT_VIEWS)[number]["id"];

export type StepState = "done" | "running" | "failed" | "asking" | "denied" | "idle";

/** One row of a run of work: a tool call, or the agent's thinking. */
export interface Step {
  key: string;
  kind: "tool" | "thinking";
  /** "Edited", "Running", "Wants to run", or the tool's own name. */
  verb: string;
  /** The command, path, pattern or URL; mono when `code`. */
  detail: string;
  code: boolean;
  /** A short fact at the row's right: "+24 −9", "12 lines", "3 steps". */
  extra?: string;
  state: StepState;
  /** What opening the row shows: the output, or an edit's lines. */
  body?: { kind: "output"; text: string; error: boolean; truncated: boolean } | { kind: "diff"; rows: DiffRow[] } | { kind: "text"; text: string };
}

export type StepGroup = { kind: "routine"; key: string; steps: Step[] } | { kind: "step"; key: string; step: Step };

export function groupTranscriptSteps(steps: readonly Step[], view: TranscriptView): StepGroup[] {
  const groups: StepGroup[] = [];
  for (const step of steps) {
    if (view !== "verbose" && step.kind === "tool" && step.state === "done") {
      const previous = groups.at(-1);
      if (previous?.kind === "routine") previous.steps.push(step);
      else groups.push({ kind: "routine", key: `routine:${step.key}`, steps: [step] });
    } else groups.push({ kind: "step", key: step.key, step });
  }
  return groups;
}

export type DiffRow = { sign: " " | "-" | "+"; text: string };

export interface TurnOutput {
  kind: "output";
  key: string;
  status: "completed" | "interrupted" | "error";
  durationMs: number | null;
  edits: Step[];
  tests: Step[];
}

export type TimelineItem =
  | { kind: "user"; key: string; text: string; images: string[] }
  | { kind: "turn"; key: string; model: string | null; costUsd: number | null; tokens: number; status: "running" | "completed" | "interrupted" | "error"; durationMs: number | null; error: string | null }
  | { kind: "text"; key: string; text: string; answerSeq?: number }
  | { kind: "work"; key: string; steps: Step[] }
  | { kind: "question"; key: string; text: string; answered: boolean }
  | { kind: "approval"; key: string; text: string }
  | { kind: "notice"; key: string; text: string }
  | { kind: "error"; key: string; text: string; detail: string | null }
  | TurnOutput;

type Turn = Extract<TimelineItem, { kind: "turn" }>;

interface Call {
  key: string;
  id: string;
  name: string;
  input: unknown;
  result: { output: string; isError: boolean; truncated: boolean } | null;
  approval: "pending" | "allow" | "allowSession" | "deny" | "expired" | null;
  /** Calls a sub-agent made under this one. */
  children: number;
}

const QUESTION_TOOL = "AskUserQuestion";
const MAX_BODY_LINES = 60;
const MAX_OUTPUT_CHARS = 64_000;

/**
 * The timeline of `events`, in order. `live`: the session still runs, so a call with no result is running, not
 * stopped. Thinking shows only when the session synced it.
 */
export function foldTimeline(events: readonly SessionEvent[], live: boolean, turnEnds?: ReadonlyMap<number, number>): TimelineItem[] {
  const items: TimelineItem[] = [];
  const calls = new Map<string, Call>();
  // Each run of work's entries, by its item's key. A call is filled in place as its result and approval arrive.
  const runs = new Map<string, Entry[]>();
  const approvals = new Map<string, { tool: string; input: unknown; call?: Call }>();
  const questions = new Map<string, number>();
  let turn: Turn | undefined;
  let answer: Extract<TimelineItem, { kind: "text" }> | undefined;
  let turnCalls: Call[] = [];

  // The run of work at the end of the timeline, or a new one.
  const run = (): Entry[] => {
    const tail = items.at(-1);
    if (tail?.kind === "work") return runs.get(tail.key)!;
    const key = `w${items.length}`;
    items.push({ kind: "work", key, steps: [] });
    runs.set(key, []);
    return runs.get(key)!;
  };

  events.forEach((event, index) => {
    const key = String(index);
    switch (event.kind) {
      case "user":
        items.push({ kind: "user", key, text: event.text, images: event.images ?? [] });
        turn = undefined;
        answer = undefined;
        turnCalls = [];
        break;
      case "turnStart":
        answer = undefined;
        turnCalls = [];
        turn = { kind: "turn", key, model: null, costUsd: null, tokens: 0, status: "running", durationMs: null, error: null };
        items.push(turn);
        break;
      case "text":
        if (!event.parent && event.text.trim()) {
          answer = { kind: "text", key, text: event.text };
          items.push(answer);
        }
        break;
      case "thinking":
        if (!event.parent && event.text.trim()) run().push({ key, text: event.text });
        break;
      case "tool": {
        const call: Call = { key, id: event.id, name: event.name, input: event.input, result: null, approval: null, children: 0 };
        calls.set(event.id, call);
        if (turn && !event.parent) turnCalls.push(call);
        const owner = event.parent ? calls.get(event.parent) : undefined;
        if (owner) owner.children += 1;
        else if (!event.parent && event.name !== QUESTION_TOOL) run().push(call);
        break;
      }
      case "toolResult": {
        const call = calls.get(event.id);
        if (call) call.result = { output: event.output, isError: event.isError ?? false, truncated: event.truncated ?? false };
        break;
      }
      case "approval": {
        const call = event.toolUseId ? calls.get(event.toolUseId) : undefined;
        if (call) call.approval = "pending";
        approvals.set(event.requestId, { tool: event.tool, input: event.input, call });
        if (event.tool === QUESTION_TOOL && !questions.has(event.requestId)) {
          questions.set(event.requestId, items.length);
          items.push({ kind: "question", key, text: `Asked: ${firstQuestion(event.input)}`, answered: false });
        }
        break;
      }
      case "approvalResolved": {
        const asked = approvals.get(event.requestId);
        if (asked?.call) asked.call.approval = event.decision;
        const at = questions.get(event.requestId);
        const question = at === undefined ? undefined : items[at];
        if (at !== undefined && question?.kind === "question") {
          items[at] = { ...question, text: `${question.text}${event.message ? ` (${event.message})` : ""}`, answered: true };
        } else if (asked && !asked.call && asked.tool !== QUESTION_TOOL) {
          items.push({ kind: "approval", key, text: approvalText(asked.tool, asked.input, event.decision, event.message ?? null) });
        }
        break;
      }
      case "usage":
        if (event.parent || !turn) break;
        turn.model = event.model;
        turn.costUsd = (turn.costUsd ?? 0) + event.costUsd;
        turn.tokens += event.inputTokens + event.outputTokens;
        break;
      case "turnEnd": {
        if (answer && event.status === "completed" && turnEnds?.has(index)) answer.answerSeq = turnEnds.get(index);
        answer = undefined;
        if (!turn) break;
        turn.status = event.status;
        turn.durationMs = event.durationMs ?? null;
        turn.costUsd = event.costUsd ?? turn.costUsd;
        turn.error = event.error ?? null;
        const edits: Step[] = [];
        const tests: Step[] = [];
        for (const call of turnCalls) {
          if (!call.result || call.approval === "deny" || call.approval === "expired") continue;
          if (isFileEdit(call.name) && !call.result.isError) {
            edits.push(stepOf(call, false));
          } else if (call.name === "Bash") {
            const command = testCommand(text(record(call.input).command));
            if (command) tests.push({ ...stepOf(call, false), detail: firstLine(command, 240), extra: call.result.isError ? "Failed" : "Completed" });
          }
        }
        if (edits.length || tests.length) items.push({
          kind: "output", key: `output:${key}`, status: event.status, durationMs: turn.durationMs, edits, tests,
        });
        turn = undefined;
        turnCalls = [];
        break;
      }
      case "notice":
        items.push({ kind: "notice", key, text: event.text });
        break;
      case "error":
        items.push({ kind: "error", key, text: event.text, detail: event.detail ?? null });
        break;
      case "task":
        break;
    }
  });

  // A run of work can still run only while the session is live, its turn has not ended, and the person has not
  // written since: whatever result never came, nothing before that is running.
  const runningRuns = new Set<string>();
  const lastUser = items.findLastIndex((item) => item.kind === "user");
  let current: Turn | undefined;
  items.forEach((item, index) => {
    if (item.kind === "user") current = undefined;
    if (item.kind === "turn") current = item;
    if (item.kind === "work" && live && index > lastUser && (!current || current.status === "running")) runningRuns.add(item.key);
  });
  return items.map((item) =>
    item.kind === "work"
      ? { ...item, steps: runs.get(item.key)!.map((entry) => ("id" in entry ? stepOf(entry, runningRuns.has(item.key)) : thinkingStep(entry))) }
      : item,
  );
}

type Entry = Call | { key: string; text: string };

function thinkingStep(entry: { key: string; text: string }): Step {
  return { key: entry.key, kind: "thinking", verb: "Thought", detail: firstLine(entry.text, 120), code: false, state: "done", body: { kind: "text", text: clip(entry.text) } };
}

function stepOf(call: Call, live: boolean): Step {
  const summary = summarize(call.name, call.input);
  const state: StepState =
    call.approval === "pending" && !call.result
      ? live
        ? "asking"
        : "idle"
      : call.approval === "deny" || call.approval === "expired"
        ? "denied"
        : call.result
          ? call.result.isError
            ? "failed"
            : "done"
          : live
            ? "running"
            : "idle";
  const tense = state === "done" || state === "failed" ? "done" : state === "running" ? "doing" : state === "asking" ? "asking" : "plain";
  const extra = call.children ? `${call.children} ${call.children === 1 ? "step" : "steps"}` : (summary.extra ?? resultMeta(call));
  return { key: call.key, kind: "tool", verb: verbFor(summary.verb, tense), detail: summary.detail, code: summary.code, extra, state, body: bodyOf(call) };
}

function bodyOf(call: Call): Step["body"] {
  const input = record(call.input);
  if (call.name === "Edit" && !call.result?.isError) return { kind: "diff", rows: diffRows(text(input.old_string), text(input.new_string)) };
  if (call.name === "Write" && !call.result?.isError) return { kind: "diff", rows: diffRows("", text(input.content)) };
  const result = call.result;
  if (result?.output.trim()) {
    return {
      kind: "output", text: result.output.slice(0, MAX_OUTPUT_CHARS).trimEnd(), error: result.isError,
      truncated: result.truncated || result.output.length > MAX_OUTPUT_CHARS,
    };
  }
  return undefined;
}

type Input = Record<string, unknown>;
const record = (value: unknown): Input => (value && typeof value === "object" ? (value as Input) : {});
const text = (value: unknown): string => (typeof value === "string" ? value : "");

export interface Summary {
  verb: string;
  detail: string;
  code: boolean;
  extra?: string;
}

/** A tool call in one line, as the desktop says it (`tools.ts` `summarize`). */
export function summarize(name: string, raw: unknown): Summary {
  const input = record(raw);
  const path = (key = "file_path") => shortPath(text(input[key]) || text(input.path) || text(input.notebook_path));
  switch (name) {
    case "Bash":
      return { verb: "Run", detail: firstLine(text(input.command)), code: true };
    case "BashOutput":
      return { verb: "Read output", detail: text(input.bash_id), code: true };
    case "KillShell":
    case "KillBash":
      return { verb: "Stop shell", detail: text(input.shell_id) || text(input.bash_id), code: true };
    case "Read":
      return { verb: "Read", detail: path(), code: true };
    case "Write":
      return { verb: "Write", detail: path(), code: true, extra: lines(text(input.content)) };
    case "Edit":
      return { verb: "Edit", detail: path(), code: true, extra: change(text(input.old_string), text(input.new_string)) };
    case "MultiEdit": {
      const edits = Array.isArray(input.edits) ? input.edits.length : 0;
      return { verb: "Edit", detail: path(), code: true, extra: `${edits} ${edits === 1 ? "change" : "changes"}` };
    }
    case "NotebookEdit":
      return { verb: "Edit notebook", detail: path("notebook_path"), code: true };
    case "Grep":
      return { verb: "Search", detail: text(input.pattern), code: true };
    case "Glob":
      return { verb: "Find files", detail: text(input.pattern), code: true };
    case "LS":
      return { verb: "List", detail: path("path"), code: true };
    case "WebFetch":
      return { verb: "Fetch", detail: shortUrl(text(input.url)), code: true };
    case "WebSearch":
      return { verb: "Search the web", detail: text(input.query), code: false };
    case "Task":
    case "Agent":
      return { verb: "Agent", detail: text(input.description) || firstLine(text(input.prompt), 80), code: false };
    case "TodoWrite": {
      const todos = Array.isArray(input.todos) ? (input.todos as Input[]) : [];
      const done = todos.filter((todo) => todo.status === "completed").length;
      return { verb: "Update the plan", detail: `${done} of ${todos.length} done`, code: false };
    }
    case "ExitPlanMode":
      return { verb: "Propose a plan", detail: firstLine(text(input.plan).replace(/^#+\s*/, ""), 100), code: false };
  }
  const mcp = /^mcp__(.+?)__(.+)$/.exec(name);
  if (mcp) return { verb: `${mcp[1]} · ${mcp[2]}`, detail: firstArgument(input), code: false };
  return { verb: name, detail: firstArgument(input), code: false };
}

/** The desktop's verbs, as a finished step says them, and a running one. */
const TENSES: Record<string, [done: string, doing: string]> = {
  Run: ["Ran", "Running"],
  "Read output": ["Read output", "Reading output"],
  "Stop shell": ["Stopped shell", "Stopping shell"],
  Read: ["Read", "Reading"],
  Write: ["Wrote", "Writing"],
  Edit: ["Edited", "Editing"],
  "Edit notebook": ["Edited notebook", "Editing notebook"],
  Search: ["Searched", "Searching"],
  "Find files": ["Found files", "Finding files"],
  List: ["Listed", "Listing"],
  Fetch: ["Fetched", "Fetching"],
  "Search the web": ["Searched the web", "Searching the web"],
  "Update the plan": ["Updated the plan", "Updating the plan"],
  "Propose a plan": ["Proposed a plan", "Proposing a plan"],
};

export function verbFor(verb: string, tense: "done" | "doing" | "asking" | "plain"): string {
  const forms = TENSES[verb];
  if (!forms || tense === "plain") return verb;
  if (tense === "asking") return `Wants to ${verb.charAt(0).toLowerCase()}${verb.slice(1)}`;
  return tense === "done" ? forms[0] : forms[1];
}

function resultMeta(call: Call): string | undefined {
  if (call.name !== "Read" || !call.result || call.result.isError) return undefined;
  return lines(call.result.output.replace(/\n+$/, ""));
}

/** An approval outside a call, answered: "Denied: run rm -rf build (“not now”)". */
function approvalText(tool: string, input: unknown, decision: string, message: string | null): string {
  const summary = summarize(tool, input);
  const word = decision === "deny" ? "Denied" : decision === "expired" ? "Expired" : decision === "allowSession" ? "Always allowed" : "Allowed";
  return `${word}: ${summary.verb.toLowerCase()} ${firstLine(summary.detail, 80)}${message ? ` (“${message}”)` : ""}`;
}

function firstQuestion(input: unknown): string {
  const questions = record(input).questions;
  return Array.isArray(questions) && typeof record(questions[0]).question === "string" ? (record(questions[0]).question as string) : "a question";
}

/** An edit's lines: those both sides share at the start and the end as context, the others removed, then added. */
export function diffRows(before: string, after: string): DiffRow[] {
  const removed = before ? before.split("\n") : [];
  const added = after ? after.split("\n") : [];
  let head = 0;
  while (head < removed.length && head < added.length && removed[head] === added[head]) head++;
  let tail = 0;
  while (tail < removed.length - head && tail < added.length - head && removed[removed.length - 1 - tail] === added[added.length - 1 - tail]) tail++;
  const rows: DiffRow[] = [
    ...removed.slice(Math.max(0, head - 2), head).map((line) => ({ sign: " " as const, text: line })),
    ...removed.slice(head, removed.length - tail).map((line) => ({ sign: "-" as const, text: line })),
    ...added.slice(head, added.length - tail).map((line) => ({ sign: "+" as const, text: line })),
    ...removed.slice(removed.length - tail, removed.length - tail + 2).map((line) => ({ sign: " " as const, text: line })),
  ];
  return rows.length > MAX_BODY_LINES ? [...rows.slice(0, MAX_BODY_LINES), { sign: " ", text: `… ${rows.length - MAX_BODY_LINES} more lines` }] : rows;
}

export function firstLine(value: string, max = 160): string {
  const line = value.trim().split("\n")[0] ?? "";
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** A long text cut to its first lines, saying how many more there were. */
export function clip(value: string): string {
  const all = value.replace(/\n+$/, "").split("\n");
  return all.length > MAX_BODY_LINES ? `${all.slice(0, MAX_BODY_LINES).join("\n")}\n… ${all.length - MAX_BODY_LINES} more lines` : all.join("\n");
}

/** A path as the phone has room for it: its last two parts. */
export function shortPath(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  return parts.length > 2 ? parts.slice(-2).join("/") : path;
}

function firstArgument(input: Input): string {
  for (const value of Object.values(input)) if (typeof value === "string" && value.trim()) return firstLine(value, 100);
  return "";
}

function shortUrl(url: string): string {
  const match = /^[a-z]+:\/\/([^/?#]+)([^?#]*)/i.exec(url);
  return match ? `${match[1]}${match[2] === "/" ? "" : match[2]}` : url;
}

function count(value: string): number {
  return value ? value.split("\n").length : 0;
}

function lines(content: string): string | undefined {
  const n = count(content);
  return n ? `${n} ${n === 1 ? "line" : "lines"}` : undefined;
}

function change(before: string, after: string): string | undefined {
  const removed = count(before);
  const added = count(after);
  return removed || added ? `+${added} −${removed}` : undefined;
}

/** How long a turn took, as the desktop's timeline says it: "41s", "1m 48s", "1h 2m". */
export function duration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** A turn's line after its model and cost: how it ended, and after how long. */
export function turnEnding(turn: { status: "running" | "completed" | "interrupted" | "error"; durationMs: number | null }): string | null {
  const after = turn.durationMs === null ? "" : duration(turn.durationMs);
  switch (turn.status) {
    case "running":
      return null;
    case "completed":
      return after ? `Done in ${after}` : "Done";
    case "interrupted":
      return after ? `Stopped after ${after}` : "Stopped";
    case "error":
      return after ? `Failed after ${after}` : "Failed";
  }
}
