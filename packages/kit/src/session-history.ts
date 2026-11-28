import type { ChatSessionSummary, SyncedThread } from "./api";
import type { CoreClient } from "./core-client";
import type { ExecutionOrigin } from "./computer-types";

export type UnifiedSession =
  | { source: "chat"; id: string; updatedAt: string; chat: ChatSessionSummary; origin: { kind: "chat" } }
  | { source: "synced"; id: string; updatedAt: string; thread: SyncedThread; origin: ExecutionOrigin };

export interface UnifiedHistoryPage {
  sessions: UnifiedSession[];
  more: boolean;
  errors: { source: HistorySource; message: string }[];
}

export const sessionKey = (session: Pick<UnifiedSession, "source" | "id">): string => `${session.source}:${session.id}`;

export function originLabel(origin: UnifiedSession["origin"], currentComputerId?: string): string {
  switch (origin.kind) {
    case "chat": return "Chat";
    case "computer": return origin.computerId === currentComputerId ? "This computer" : origin.name;
    case "cloud": return origin.name ? `Cloud · ${origin.name}` : "Cloud";
    case "unknown": return "Code";
  }
}

export function compareSessions(a: Pick<UnifiedSession, "source" | "id" | "updatedAt">, b: Pick<UnifiedSession, "source" | "id" | "updatedAt">): number {
  const moved = Date.parse(b.updatedAt) - Date.parse(a.updatedAt);
  return moved || (sessionKey(a) < sessionKey(b) ? 1 : sessionKey(a) > sessionKey(b) ? -1 : 0);
}

type HistorySource = UnifiedSession["source"] | "team";
interface Queue { cursor: string | undefined; done: boolean; rows: UnifiedSession[] }
const queue = (): Queue => ({ cursor: undefined, done: false, rows: [] });
const cancelled = () => Object.assign(new Error("Session history loading was cancelled."), { name: "AbortError" });

/** Both sources are activity ordered. Retained buffers prevent a mixed page from skipping unused source rows. */
export function createUnifiedHistory(core: Pick<CoreClient, "chatSessions" | "activeSyncedThreads">, includeTeam = false) {
  const enabled: HistorySource[] = includeTeam ? ["chat", "synced", "team"] : ["chat", "synced"];
  const empty = (): Record<HistorySource, Queue> => ({ chat: queue(), synced: queue(), team: queue() });
  let sources = empty();
  let seen = new Set<string>();
  let generation = 0;
  let pending: Promise<UnifiedHistoryPage> | undefined;

  const read = async ({ limit = 50, signal }: { limit?: number; signal?: AbortSignal } = {}): Promise<UnifiedHistoryPage> => {
    const count = Math.max(1, Math.min(100, Math.floor(limit)));
    const reading = generation;
    const current = empty();
    for (const source of enabled) current[source] = { ...sources[source], rows: [...sources[source].rows] };
    const nextSeen = new Set(seen);
    const sessions: UnifiedSession[] = [];
    const errors: UnifiedHistoryPage["errors"] = [];
    const check = () => { if (signal?.aborted || reading !== generation) throw cancelled(); };

    const fill = async (source: HistorySource): Promise<void> => {
      const state = current[source];
      if (state.rows.length || state.done) return;
      check();
      try {
        const pageSize = Math.max(50, count);
        let rows: UnifiedSession[];
        let cursor: string | undefined;
        if (source === "chat") {
          const page = await core.chatSessions({ cursor: state.cursor, limit: pageSize }, signal);
          rows = page.sessions.map((chat) => ({ source: "chat", id: chat.id, updatedAt: chat.updatedAt, chat, origin: { kind: "chat" } }));
          cursor = page.nextCursor || undefined;
        } else {
          const page = await core.activeSyncedThreads({ cursor: state.cursor, limit: pageSize, ...(source === "team" ? { scope: "team" } : {}) }, signal);
          if (page.order !== "activity") throw new Error("Update Gateway to include computer sessions in history.");
          rows = page.threads.map((thread) => ({ source: "synced", id: thread.threadId, updatedAt: thread.updatedAt, thread, origin: { kind: "unknown" } }));
          cursor = page.nextCursor || undefined;
        }
        check();
        if (rows.some((row) => !Number.isFinite(Date.parse(row.updatedAt))) || (cursor && (cursor === state.cursor || !rows.length))) throw new Error("Session history returned an invalid page.");
        state.rows = rows;
        state.cursor = cursor;
        state.done = cursor === undefined;
      } catch (reason) {
        check();
        errors.push({ source, message: reason instanceof Error ? reason.message : "Session history could not load." });
      }
    };

    while (sessions.length < count) {
      await Promise.all(enabled.map(fill));
      check();
      let head: { source: HistorySource; row: UnifiedSession } | undefined;
      for (const source of enabled) {
        const row = current[source].rows[0];
        if (row && (!head || compareSessions(row, head.row) < 0)) head = { source, row };
      }
      if (!head) break;
      current[head.source].rows.shift();
      const { row } = head;
      const key = sessionKey(row);
      if (!nextSeen.has(key)) { sessions.push(row); nextSeen.add(key); }
      // A failed source has an unknown head. Show a partial page, retaining cursors for a complete retry.
      if (errors.length) {
        const known = enabled.flatMap((source) => current[source].rows).sort(compareSessions);
        for (const each of known) {
          if (sessions.length >= count) break;
          const key = sessionKey(each);
          if (!nextSeen.has(key)) { sessions.push(each); nextSeen.add(key); }
        }
        break;
      }
    }
    check();
    if (!errors.length) { sources = current; seen = nextSeen; }
    return { sessions, errors, more: errors.length > 0 || enabled.some((source) => current[source].rows.length > 0 || !current[source].done) };
  };

  return {
    next(options: { limit?: number; signal?: AbortSignal } = {}): Promise<UnifiedHistoryPage> {
      if (!pending) {
        const tracked = read(options).finally(() => { if (pending === tracked) pending = undefined; });
        pending = tracked;
      }
      return pending;
    },
    reset(): void { generation++; sources = empty(); seen = new Set(); pending = undefined; },
  };
}
