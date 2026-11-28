/**
 * Sharing a coding-agent session as a Nexus Artifact: the snapshot an app builds from its own log and sends
 * to `POST /v1/artifacts/sessions` (`shareAgentSession`). Nexus for desktop builds it from a Code session's event log;
 * `nexus claude` will build it from Claude Code's transcript. `fitSessionSnapshot` keeps it inside Gateway's limits, so
 * a long session is shared with shorter tool results rather than refused.
 */
import type { AgentName } from "./api";
import type { ArtifactAudienceKind } from "./artifacts";

/** Gateway's limits on a snapshot (`nexus_state::artifacts::sessions`). */
export const SESSION_SHARE_LIMITS = {
  /** The checked timeline, as JSON. */
  sessionBytes: 4 * 1024 * 1024,
  images: 32,
  /** One image, decoded. */
  oneImageBytes: 5 * 1024 * 1024,
  /** All images together, decoded. */
  imageBytes: 20 * 1024 * 1024,
  /** One tool result's text: the desktop timeline's cap. */
  toolOutputBytes: 64 * 1024,
  events: 20_000,
} as const;

/** `desktop`: a Code session in Nexus for desktop; `claude-code`: a `nexus claude` session's Claude Code transcript. */
export type SessionShareSource = "desktop" | "claude-code";

/** `at` is when it happened, in milliseconds; `parent` the sub-agent's tool call that made it (absent: the session's own agent). */
export type SessionEvent = { at?: number } & (
  | { kind: "user"; text: string; images?: string[] }
  | { kind: "text" | "thinking"; id?: string; text: string; parent?: string | null }
  | { kind: "tool"; id: string; name: string; input?: unknown; parent?: string | null }
  | { kind: "toolResult"; id: string; output: string; isError?: boolean; truncated?: boolean; images?: string[]; parent?: string | null }
  | { kind: "approval"; requestId: string; toolUseId?: string | null; tool: string; input?: unknown; description?: string | null; parent?: string | null }
  | { kind: "approvalResolved"; requestId: string; decision: "allow" | "allowSession" | "deny" | "expired"; message?: string | null }
  | {
      kind: "usage";
      model: string;
      inputTokens: number;
      outputTokens: number;
      cacheReadTokens: number;
      cacheWriteTokens: number;
      costUsd: number;
      parent?: string | null;
    }
  | { kind: "turnStart"; turnId?: string }
  | { kind: "turnEnd"; turnId?: string; status: "completed" | "interrupted" | "error"; costUsd?: number | null; durationMs?: number; error?: string | null }
  | { kind: "task"; taskId: string; toolUseId?: string | null; agentType?: string | null; status: "running" | "completed" | "failed" | "stopped"; description?: string }
  | { kind: "notice"; text: string }
  | { kind: "error"; text: string; detail?: string | null }
);

/** The snapshot as an app sends it. `id` is the app's own id for the session: sharing the same id again makes the next version. */
export interface SessionSnapshotInput {
  source: SessionShareSource;
  id: string;
  title?: string;
  agent?: AgentName;
  model?: string;
  /** The agent's own estimate. */
  costUsd?: number;
  startedAt?: number;
  events: SessionEvent[];
}

/** An image the events name by `id`: base64 (or a data: URL), PNG, JPEG, GIF or WebP. */
export interface SessionShareImage {
  id: string;
  data: string;
  name?: string | null;
}

/** `POST /v1/artifacts/sessions`: a chat share's audiences, and the snapshot. */
export interface ShareAgentSessionRequest {
  audience: ArtifactAudienceKind;
  teams?: string[];
  people?: string[];
  expiresIn?: string | number;
  expiresAt?: string;
  replace?: boolean;
  /** Keep the agent's thinking: left out by default. */
  includeThinking?: boolean;
  session: SessionSnapshotInput;
  images?: SessionShareImage[];
}

/** A shared session's timeline as Gateway stored it (`ArtifactDetail.session`). Readers outside get no cost or usage. */
export interface SessionSnapshot {
  format: number;
  source: SessionShareSource;
  agent?: AgentName;
  model?: string;
  costUsd?: number;
  startedAt?: number;
  thinking: boolean;
  events: SessionEvent[];
  images: { id: string; mediaType: string; bytes: number; width?: number; height?: number; name?: string }[];
}

/** What `fitSessionSnapshot` left out to fit: tool results cut, strings in tool inputs cut, images dropped, events dropped. */
export interface SessionTrim {
  outputs: number;
  inputs: number;
  images: number;
  events: number;
}

/** Thrown when a session does not fit even with its tool results at their shortest. */
export class SessionTooLargeError extends Error {
  constructor(readonly bytes: number) {
    super(`This session is ${(bytes / 1_048_576).toFixed(1)} MB even with tool results cut short, and a shared session may be at most 4 MB.`);
    this.name = "SessionTooLargeError";
  }
}

const encoder = new TextEncoder();
const utf8Length = (text: string) => encoder.encode(text).length;

/** `text` cut to at most `max` bytes of UTF-8, at a character boundary. */
export function cutUtf8(text: string, max: number): string {
  if (text.length <= max / 4 || utf8Length(text) <= max) return text;
  let low = 0;
  let high = Math.min(text.length, max);
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (utf8Length(text.slice(0, mid)) <= max) low = mid;
    else high = mid - 1;
  }
  // Never end on the first half of a surrogate pair.
  const code = text.charCodeAt(low - 1);
  return text.slice(0, code >= 0xd800 && code <= 0xdbff ? low - 1 : low);
}

/** What a cut string of a tool's input ends with, so a reader knows it was shortened. */
export const CUT_MARK = " … [cut to fit]";

/** Every string in a tool's input cut to `max` bytes and marked; the count of strings cut goes to `cut`. */
function cutStrings(value: unknown, max: number, cut: { count: number }): unknown {
  if (typeof value === "string") {
    const short = cutUtf8(value, max);
    if (short === value) return value;
    cut.count += 1;
    return short + CUT_MARK;
  }
  if (Array.isArray(value)) return value.map((item) => cutStrings(item, max, cut));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, cutStrings(item, max, cut)]));
  return value;
}

/** The decoded size of base64 (or a data: URL). */
const decodedBytes = (data: string) => Math.floor((data.slice(data.indexOf(",") + 1).replace(/=+$/, "").length * 3) / 4);

/**
 * The steps tried until the snapshot fits, as [tool result cap, tool input string cap]: tool results alone first, from
 * the timeline's own cap down, then the inputs too (a tool's input is what the agent wrote).
 */
const STEPS: [number, number | null][] = [
  [SESSION_SHARE_LIMITS.toolOutputBytes, null],
  [16 * 1024, null],
  [4 * 1024, null],
  [1024, null],
  [256, null],
  [256, 16 * 1024],
  [256, 4 * 1024],
  [256, 1024],
  [256, 256],
];
/**
 * Room left for what Gateway adds: it measures the checked timeline, not this body. That is the image list and the
 * format, plus up to `PER_EVENT` bytes an event for the defaults it fills in (`isError`, `truncated`, token counts).
 */
const HEADROOM = 64 * 1024;
const PER_EVENT = 64;

/**
 * The snapshot inside Gateway's limits: at most 20,000 events (the newest ones, where the session ended), the first 32 images the events name
 * within 20 MB, each at most 5 MB (the others are dropped from the events that name them), and tool results and tool
 * results cut shorter, then tool inputs (each cut string marked with `CUT_MARK`),
 * step by step, until the whole is under 4 MB. Throws `SessionTooLargeError` when even the shortest step is too large.
 */
export function fitSessionSnapshot(
  session: SessionSnapshotInput,
  images: SessionShareImage[],
): { session: SessionSnapshotInput; images: SessionShareImage[]; trimmed: SessionTrim } {
  const trimmed: SessionTrim = { outputs: 0, inputs: 0, images: 0, events: Math.max(0, session.events.length - SESSION_SHARE_LIMITS.events) };
  const events = session.events.slice(-SESSION_SHARE_LIMITS.events);
  const byId = new Map(images.map((image) => [image.id, image]));
  const kept = new Map<string, SessionShareImage>();
  let imageBytes = 0;
  for (const event of events) {
    if (!("images" in event) || !event.images) continue;
    for (const id of event.images) {
      const image = byId.get(id);
      if (!image || kept.has(id)) continue;
      const bytes = decodedBytes(image.data);
      if (bytes > SESSION_SHARE_LIMITS.oneImageBytes) continue;
      if (kept.size >= SESSION_SHARE_LIMITS.images || imageBytes + bytes > SESSION_SHARE_LIMITS.imageBytes) continue;
      kept.set(id, image);
      imageBytes += bytes;
    }
  }
  trimmed.images = new Set(events.flatMap((event) => ("images" in event && event.images ? event.images : []))).size - kept.size;
  const named = events.map((event): SessionEvent => {
    if (!("images" in event) || !event.images) return event;
    const ids = event.images.filter((id) => kept.has(id));
    const { images: _dropped, ...rest } = event;
    return (ids.length ? { ...rest, images: ids } : rest) as SessionEvent;
  });
  const budget = SESSION_SHARE_LIMITS.sessionBytes - HEADROOM - PER_EVENT * named.length;
  let bytes = 0;
  for (const [cap, inputCap] of STEPS) {
    const outputs = { count: 0 };
    const inputs = { count: 0 };
    const capped = named.map((event): SessionEvent => {
      if (event.kind === "toolResult") {
        const output = cutUtf8(event.output, cap);
        if (output === event.output) return event;
        outputs.count += 1;
        return { ...event, output, truncated: true };
      }
      if ((event.kind === "tool" || event.kind === "approval") && inputCap !== null) {
        return { ...event, input: cutStrings(event.input, inputCap, inputs) };
      }
      return event;
    });
    const fitted = { ...session, events: capped };
    bytes = utf8Length(JSON.stringify(fitted));
    if (bytes <= budget) {
      trimmed.outputs = outputs.count;
      trimmed.inputs = inputs.count;
      return { session: fitted, images: [...kept.values()], trimmed };
    }
  }
  throw new SessionTooLargeError(bytes);
}
