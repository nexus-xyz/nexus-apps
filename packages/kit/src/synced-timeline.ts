/**
 * A synced session's opened events as the timeline a viewer draws. Each event is the desktop's stored
 * `WbEvent` JSON (`desktop/src/workbench/contract.ts`), and this reads it into the {@link SessionEvent}s that Nexus UI's
 * session timeline already folds and draws as the desktop does. It reads what the desktop's "Share as
 * artifact" (`desktop/src/workbench/session-share.ts`) keeps, the same way: prompts as edited and without those taken
 * back, answers, thinking, tool calls and results, approvals, tasks, usage, notices and errors. Bookkeeping (runs,
 * connections, commands, setup, the queue, reads) stays out.
 *
 * The JSON comes from another program's database, so every field is checked here rather than cast: a field of the
 * wrong type is left out, and an event this viewer does not know is skipped.
 */
import type { OpenedEvent } from "./synced-sessions";
import type { SessionEvent } from "./session-share";

/** An image a synced event carried inline, named by the id its {@link SessionEvent} gives it. */
export interface InlineImage {
  id: string;
  mediaType: string;
  /** base64. */
  data: string;
  name: string | null;
}

export interface SyncedTimeline {
  events: SessionEvent[];
  /** The stored sequence of each turn end, keyed by its index in events. */
  turnEnds: ReadonlyMap<number, number>;
  images: InlineImage[];
  /** The newest model a run named, for the header. */
  model: string | null;
}

type Raw = Record<string, unknown>;

const str = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);
const num = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined);
const bool = (value: unknown): boolean | undefined => (typeof value === "boolean" ? value : undefined);
const nullableStr = (value: unknown): string | null | undefined => (value === null ? null : str(value));
const oneOf = <T extends string>(value: unknown, values: readonly T[]): T | undefined => (values.includes(value as T) ? (value as T) : undefined);

const TURN_STATUSES = ["completed", "interrupted", "error"] as const;
const DECISIONS = ["allow", "allowSession", "deny", "expired"] as const;
const TASK_STATUSES = ["running", "completed", "failed", "stopped"] as const;

/** The timeline of `opened`, in number order. Events that did not open are left to the page to account for. */
export function syncedTimeline(opened: readonly OpenedEvent[]): SyncedTimeline {
  const images: InlineImage[] = [];
  const named = (list: unknown, seq: number): string[] | undefined => {
    if (!Array.isArray(list) || !list.length) return undefined;
    return list.flatMap((image: Raw, index) => {
      const data = str(image?.data);
      const mediaType = str(image?.mediaType);
      if (!data || !mediaType?.startsWith("image/")) return [];
      const id = `sync_${seq}_${index}`;
      images.push({ id, mediaType, data, name: str(image.name) ?? null });
      return [id];
    });
  };
  const open = opened.filter((entry): entry is Extract<OpenedEvent, { state: "open" }> => entry.state === "open");
  // The queue's own events name a message by its local number, which starts at 1; a synced number starts at 0.
  const cancelled = new Set<number>();
  const edited = new Map<number, string>();
  for (const { event } of open) {
    const seq = num(event.seq);
    if (seq === undefined) continue;
    if (event.kind === "queueCancel") cancelled.add(seq - 1);
    else if (event.kind === "queueEdit" && str(event.text) !== undefined) edited.set(seq - 1, str(event.text)!);
  }
  let model: string | null = null;
  const events: SessionEvent[] = [];
  const turnEnds = new Map<number, number>();
  for (const { seq, at, event: e } of open) {
    const add = (event: SessionEvent) => events.push(withoutEmpty({ ...event, at }));
    const parent = nullableStr(e.parent);
    switch (e.kind) {
      case "user": {
        // A linked session's message is not the person's own.
        if (cancelled.has(seq) || e.from) break;
        const text = edited.get(seq) ?? str(e.text);
        if (text !== undefined) add({ kind: "user", text, images: named(e.images, seq) });
        break;
      }
      case "session":
        model = str(e.model) ?? model;
        break;
      case "text":
      case "thinking": {
        const text = str(e.text);
        if (text !== undefined) add({ kind: e.kind, id: str(e.id), text, parent });
        break;
      }
      case "tool": {
        const id = str(e.id);
        const name = str(e.name);
        if (id && name) add({ kind: "tool", id, name, input: e.input, parent });
        break;
      }
      case "toolResult": {
        const id = str(e.id);
        const output = str(e.output);
        if (id && output !== undefined) add({ kind: "toolResult", id, output, isError: bool(e.isError), truncated: bool(e.truncated), images: named(e.images, seq), parent });
        break;
      }
      case "approval": {
        const requestId = str(e.requestId);
        const tool = str(e.tool);
        if (requestId && tool) add({ kind: "approval", requestId, toolUseId: nullableStr(e.toolUseId), tool, input: e.input, description: nullableStr(e.description), parent });
        break;
      }
      case "approvalResolved": {
        const requestId = str(e.requestId);
        const decision = oneOf(e.decision, DECISIONS);
        if (requestId && decision) add({ kind: "approvalResolved", requestId, decision, message: nullableStr(e.message) });
        break;
      }
      case "usage": {
        const model = str(e.model);
        const [inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, costUsd] = [e.inputTokens, e.outputTokens, e.cacheReadTokens, e.cacheWriteTokens, e.costUsd].map(num);
        if (model !== undefined && inputTokens !== undefined && outputTokens !== undefined && cacheReadTokens !== undefined && cacheWriteTokens !== undefined && costUsd !== undefined) {
          add({ kind: "usage", model, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, costUsd, parent });
        }
        break;
      }
      case "turnStart":
        add({ kind: "turnStart", turnId: str(e.turnId) });
        break;
      case "turnEnd": {
        const status = oneOf(e.status, TURN_STATUSES);
        if (status) {
          turnEnds.set(events.length, seq);
          add({ kind: "turnEnd", turnId: str(e.turnId), status, costUsd: num(e.costUsd) ?? null, durationMs: num(e.durationMs), error: nullableStr(e.error) });
        }
        break;
      }
      case "task": {
        const taskId = str(e.taskId);
        const status = oneOf(e.status, TASK_STATUSES);
        if (taskId && status) add({ kind: "task", taskId, toolUseId: nullableStr(e.toolUseId), agentType: nullableStr(e.agentType), status, description: str(e.description) });
        break;
      }
      case "notice": {
        const text = str(e.text);
        if (text !== undefined) add({ kind: "notice", text });
        break;
      }
      case "error": {
        const text = str(e.text);
        if (text !== undefined) add({ kind: "error", text, detail: nullableStr(e.detail) });
        break;
      }
      default:
        break;
    }
  }
  return { events, images, model, turnEnds };
}

/** Absent rather than undefined or null, as a shared session's events are. */
function withoutEmpty(event: SessionEvent): SessionEvent {
  return Object.fromEntries(Object.entries(event).filter(([, value]) => value !== undefined && value !== null)) as SessionEvent;
}
