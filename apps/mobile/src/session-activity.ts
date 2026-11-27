/**
 * What a Code session does now, in the words the desktop puts over its composer (GPUI `composer.rs`, `code_dock.rs`):
 * "Working 1m 20s" or "Waiting for you", and the plan's progress, "2 of 5 done". Pure: no React Native, so vitest runs
 * it.
 */
import type { SessionEvent } from "@nexus/kit";
import type { OpenedEvent } from "@nexus/kit/synced-sessions";
import { duration } from "./code-timeline";

export interface PlanProgress {
  done: number;
  total: number;
  /** The step in progress, as the agent words it ("Running the decoder tests"); null when none is. */
  current: string | null;
}

/** The plan as the agent last wrote it (its latest top-level TodoWrite), or null before it wrote one. */
export function planProgress(events: readonly SessionEvent[]): PlanProgress | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.kind !== "tool" || event.name !== "TodoWrite" || event.parent) continue;
    const input = event.input && typeof event.input === "object" ? (event.input as { todos?: unknown }) : {};
    const todos = Array.isArray(input.todos) ? (input.todos as Record<string, unknown>[]) : [];
    if (!todos.length) return null;
    const active = todos.find((todo) => todo.status === "in_progress");
    const words = active ? (typeof active.activeForm === "string" && active.activeForm.trim() ? active.activeForm : typeof active.content === "string" ? active.content : "") : "";
    return { done: todos.filter((todo) => todo.status === "completed").length, total: todos.length, current: words.trim() || null };
  }
  return null;
}

/** When the current turn started, in milliseconds since 1970: its `turnStart` event's time. Null before one. */
export function turnStartedAt(events: readonly OpenedEvent[]): number | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.state === "open" && (event.event.kind === "turnStart" || event.event.kind === "user")) {
      return event.event.kind === "turnStart" ? event.at : null;
    }
  }
  return null;
}

/**
 * The activity line: "Waiting for you" while an approval or a question is open, "Working" with the turn's elapsed
 * time while the agent runs, and nothing at rest. Waiting outranks working, as on the desktop.
 */
export function activityText({ running, waiting, since, now }: { running: boolean; waiting: boolean; since: number | null; now: number }): string | null {
  if (waiting) return "Waiting for you";
  if (!running) return null;
  return since === null || now < since ? "Working" : `Working ${duration(now - since)}`;
}
