import type { createCoreClient } from "@nexus/kit";
import { compareSessions, createUnifiedHistory, originLabel, sessionKey, type UnifiedSession } from "@nexus/kit/session-history";
import { useCallback, useMemo, useRef, useState } from "react";
import { useHeldThreads, useReader, useTitles } from "./code-reader";
import { accessOf, STATUS_WORDS, titleText, type TitleState } from "./code-sessions";

export { sessionKey };
export type { UnifiedSession };
type Core = ReturnType<typeof createCoreClient>;
export function sessionHref(session: Pick<UnifiedSession, "id" | "source">) {
  return { pathname: "/session/[id]" as const, params: { id: session.id, source: session.source } };
}
export function sessionTitle(session: UnifiedSession, titles: Record<string, TitleState>): string {
  if (session.source === "chat") return session.chat.title || "Untitled";
  return titleText(session.thread.lastSeq === null ? "empty" : session.thread.keyVersion === null ? "local" : "sealed", titles[session.id]);
}
/** Where a session runs: "Chat", or its computer or cloud machine. */
export function sessionPlace(session: UnifiedSession, titles: Record<string, TitleState> = {}): string {
  if (session.source === "chat") return "Chat";
  const title = titles[session.id];
  return originLabel(title?.state === "open" ? title.title.origin ?? session.origin : session.origin);
}

/** Where a session runs and its state in words: "MacBook · Running". */
export function sessionDetail(session: UnifiedSession, titles: Record<string, TitleState> = {}): string {
  if (session.source === "chat") return "Chat";
  return [sessionPlace(session, titles), session.thread.status ? STATUS_WORDS[session.thread.status] : null].filter(Boolean).join(" · ");
}

/** Each consumer owns its pager. Decrypted native titles stay in this client’s memory only. */
export function useSessionHistory(core: Core, limit = 50, includeTeam = false) {
  const pager = useMemo(() => createUnifiedHistory(core, includeTeam), [core, includeTeam]);
  const [state, setState] = useState<{ owner: typeof pager; sessions: UnifiedSession[]; more: boolean; error?: string }>({ owner: pager, sessions: [], more: true });
  const active = state.owner === pager ? state : { owner: pager, sessions: [], more: true };
  const [loading, setLoading] = useState(false);
  const owner = useMemo(() => ({ pager, revision: 0, pending: false }), [pager]);
  const current = useRef(owner);
  current.current = owner;
  const load = useCallback(async (reset: boolean, quiet = false) => {
    if (current.current !== owner || (owner.pending && (!reset || quiet))) return;
    const generation = reset ? ++owner.revision : owner.revision;
    if (reset) pager.reset();
    owner.pending = true;
    setLoading(true);
    try {
      const page = await pager.next({ limit });
      if (current.current !== owner || generation !== owner.revision) return;
      setState((previous) => {
        const failed = new Set(page.errors.map(({ source }) => source));
        const retained = previous.owner !== pager ? [] : reset ? previous.sessions.filter(({ source }) => !includeTeam && failed.has(source)) : previous.sessions;
        const sessions = [...new Map([...retained, ...page.sessions].map((session) => [sessionKey(session), session])).values()].sort(compareSessions);
        return { owner: pager, sessions, more: page.more, error: page.errors.map((error) => error.message).join("\n") || undefined };
      });
    } catch (e) {
      if (current.current === owner && generation === owner.revision) setState((previous) => ({ ...(previous.owner === pager ? previous : { owner: pager, sessions: [], more: true }), error: e instanceof Error ? e.message : String(e) }));
    } finally {
      if (current.current === owner && generation === owner.revision) { owner.pending = false; setLoading(false); }
    }
  }, [pager, owner, limit, includeTeam]);
  const titles = useHistoryTitles(core, active.sessions);
  const remove = (session: Pick<UnifiedSession, "source" | "id">) => setState((previous) => previous.owner === pager ? { ...previous, sessions: previous.sessions.filter((row) => sessionKey(row) !== sessionKey(session)) } : previous);
  return { ...active, loading, titles, remove, refresh: useCallback(() => { void load(true); }, [load]), refreshQuiet: useCallback(() => { void load(true, true); }, [load]), loadMore: () => active.more && void load(false) };
}

/** Opens titles through the current account’s existing key cache. */
export function useHistoryTitles(core: Core, sessions: readonly UnifiedSession[]) {
  const reader = useReader(core);
  const ready = reader.state.kind === "ready" ? reader.state : undefined;
  const threads = useMemo(() => sessions.flatMap((s) => s.source === "synced" ? [s.thread] : []), [sessions]);
  const held = useHeldThreads(core, ready?.reader, threads);
  return useTitles(core, ready, threads, (thread) => accessOf(thread, held) === "sealed");
}
