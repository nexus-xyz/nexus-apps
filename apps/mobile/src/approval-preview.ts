/**
 * What an approval card shows before you answer, as the desktop's approval dock shows it (GPUI `code_dock.rs`): the
 * request in one line, then what it would do: a command in a code box, an edit or a write as its diff, a plan as its
 * text. Any other tool falls back to its input. Pure: no React Native, so vitest runs it.
 */
import { diffRows, firstLine, summarize, verbFor, type DiffRow } from "./code-timeline";

export type PreviewBody =
  | { kind: "command"; text: string }
  | { kind: "diff"; rows: DiffRow[] }
  | { kind: "plan"; text: string }
  | { kind: "input"; text: string };

export interface ApprovalPreview {
  /** "Wants to run", "Wants to edit", or the tool's own name. */
  verb: string;
  /** The command, path or URL the request acts on; mono when `code`. */
  detail: string;
  code: boolean;
  /** "+24 −9", "12 lines". */
  extra?: string;
  body?: PreviewBody;
  /** The answers the card offers. A plan has no "always": approving it starts the work. */
  choices: { allow: string; deny: string; always: string | null };
}

const PLAN_TOOL = "ExitPlanMode";
const MAX_INPUT = 4000;
/** A diff's rows before "… more lines", as the timeline cuts one edit's (`code-timeline.ts`). */
const MAX_ROWS = 60;

export function approvalPreview(tool: string, raw: unknown): ApprovalPreview {
  const input = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const text = (key: string) => (typeof input[key] === "string" ? (input[key] as string) : "");
  if (tool === PLAN_TOOL) {
    return { verb: "Ready to build", detail: "", code: false, body: text("plan").trim() ? { kind: "plan", text: text("plan") } : undefined, choices: { allow: "Implement", deny: "Keep planning", always: null } };
  }
  const summary = summarize(tool, raw);
  const head = { verb: verbFor(summary.verb, "asking"), detail: summary.detail, code: summary.code, ...(summary.extra ? { extra: summary.extra } : {}) };
  const choices = { allow: "Allow once", deny: "Deny", always: "Always allow" };
  switch (tool) {
    case "Bash":
      return { ...head, detail: firstLine(text("command"), 80), body: text("command").trim() ? { kind: "command", text: text("command").trim() } : undefined, choices };
    case "Edit":
      return { ...head, body: { kind: "diff", rows: diffRows(text("old_string"), text("new_string")) }, choices };
    case "Write":
      return { ...head, body: { kind: "diff", rows: diffRows("", text("content")) }, choices };
    case "MultiEdit": {
      const edits = Array.isArray(input.edits) ? (input.edits as Record<string, unknown>[]) : [];
      const rows = edits.flatMap((edit) => diffRows(typeof edit.old_string === "string" ? edit.old_string : "", typeof edit.new_string === "string" ? edit.new_string : ""));
      const shown = rows.length > MAX_ROWS ? [...rows.slice(0, MAX_ROWS), { sign: " " as const, text: `… ${rows.length - MAX_ROWS} more lines` }] : rows;
      return { ...head, body: { kind: "diff", rows: shown }, choices };
    }
  }
  const json = JSON.stringify(raw ?? {}, null, 2);
  const shown = json === "{}" ? null : json.length > MAX_INPUT ? `${json.slice(0, MAX_INPUT)}\n…` : json;
  return { ...head, ...(shown ? { body: { kind: "input", text: shown } } : {}), choices };
}
