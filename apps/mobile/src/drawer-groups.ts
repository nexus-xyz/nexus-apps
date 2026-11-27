/**
 * The drawer's sessions as the desktop's sidebar groups them (GPUI `sidebar_groups.rs`): Projects, then
 * Recent. A Code session whose title this phone opened sits under its project; a chat, and a Code session whose title
 * is locked or still opening, sit in Recent. A session is in one place, never two. No React Native, so vitest runs it.
 */
import { originLabel, type UnifiedSession } from "@nexus/kit/session-history";
import { agentName, type TitleState } from "./code-sessions";

/**
 * A session's state as its mark shows it (GPUI `sidebar.rs` `status_mark`). `working`: the agent runs. `needsYou`: it
 * waits on an approval or a question. `failed`: it ended in an error. `idle`: anything else, a chat included.
 */
export type RowState = "working" | "needsYou" | "failed" | "idle";

export function rowState(session: UnifiedSession, titles: Record<string, TitleState>): RowState {
  if (session.source === "chat") return "idle";
  const status = session.thread.status;
  if (status === "error") return "failed";
  if (status !== "running") return "idle";
  const title = titles[session.id];
  return title?.state === "open" && title.waiting ? "needsYou" : "working";
}

/** What a row says in place of its age while its session works or waits on you. None at rest or failed. */
export function stateWord(state: RowState): "Working" | "Needs you" | null {
  return state === "working" ? "Working" : state === "needsYou" ? "Needs you" : null;
}

/** All Sessions puts pending decisions first, then sorts completed and other inactive work by day. */
export function sessionSections(sessions: UnifiedSession[], titles: Record<string, TitleState>, now = new Date()) {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime();
  const groups: Record<"Needs you" | "Running" | "Done today" | "Yesterday" | "Earlier", UnifiedSession[]> = {
    "Needs you": [], Running: [], "Done today": [], Yesterday: [], Earlier: [],
  };
  for (const session of sessions) {
    const state = rowState(session, titles);
    const at = Date.parse(session.updatedAt);
    const title = state === "needsYou" ? "Needs you" : state === "working" ? "Running"
      : at >= today ? "Done today" : at >= yesterday ? "Yesterday" : "Earlier";
    groups[title].push(session);
  }
  return Object.entries(groups).flatMap(([title, data]) => data.length ? [{ key: title, title, data }] : []);
}

/** The strongest state among sessions a folded project hides: Needs you outranks Working. None when neither shows. */
export function loudest(states: Iterable<RowState>): "working" | "needsYou" | null {
  let found: "working" | "needsYou" | null = null;
  for (const state of states) {
    if (state === "needsYou") return "needsYou";
    if (state === "working") found = "working";
  }
  return found;
}

export interface ProjectGroup {
  key?: string;
  /** The project's name, as the session's opened title carries it. */
  name: string;
  /** Newest first. */
  sessions: UnifiedSession[];
}

/** How many sessions Recent shows before See all, as the desktop's sidebar does. */
export const RECENT_ROWS = 8;

/**
 * `sessions` (newest first) as Projects and Recent. Projects are in the order of their newest session; Recent keeps
 * the first `recentRows`, and `more` says whether it held more.
 */
export function drawerGroups(
  sessions: readonly UnifiedSession[],
  titles: Record<string, TitleState>,
  recentRows = RECENT_ROWS,
): { projects: ProjectGroup[]; recent: UnifiedSession[]; more: boolean } {
  const projects = new Map<string, ProjectGroup>();
  const recent: UnifiedSession[] = [];
  for (const session of sessions) {
    const name = projectOf(session, titles);
    if (!name) {
      recent.push(session);
      continue;
    }
    const group = projects.get(name);
    if (group) group.sessions.push(session);
    else projects.set(name, { name, sessions: [session] });
  }
  return { projects: [...projects.values()], recent: recent.slice(0, recentRows), more: recent.length > recentRows };
}

function projectOf(session: UnifiedSession, titles: Record<string, TitleState>): string | null {
  if (session.source !== "synced") return null;
  const title = titles[session.id];
  return title?.state === "open" ? title.title.projectName.trim() || null : null;
}

export function projectComputer(group: ProjectGroup, titles: Record<string, TitleState>): string | undefined {
  const origins = group.sessions.flatMap((session) => {
    const title = titles[session.id];
    return title?.state === "open" && title.title.origin ? [title.title.origin] : [];
  });
  const first = origins[0];
  if (first?.kind === "computer" && origins.every((origin) => origin.kind === "computer" && origin.computerId === first.computerId)) return first.computerId;
}

export const DRAWER_GROUPINGS = [
  { key: "folder", label: "Folder" },
  { key: "machine", label: "Machine" },
  { key: "status", label: "Status" },
  { key: "none", label: "None" },
] as const;
export type DrawerGrouping = (typeof DRAWER_GROUPINGS)[number]["key"];

export function groupDrawerSessions(
  sessions: readonly UnifiedSession[],
  titles: Record<string, TitleState>,
  grouping: DrawerGrouping,
): { projects: ProjectGroup[]; recent: UnifiedSession[]; more: boolean } {
  switch (grouping) {
    case "folder":
      return drawerGroups(sessions, titles);
    case "none":
      return { projects: [], recent: [...sessions], more: false };
    case "status": {
      const sections = new Map<string, UnifiedSession[]>([["Waiting on you", []], ["Working", []], ["Idle and done", []]]);
      for (const session of sessions) {
        const state = rowState(session, titles);
        const name = state === "needsYou" ? "Waiting on you" : state === "working" ? "Working" : "Idle and done";
        sections.get(name)?.push(session);
      }
      return { projects: [...sections].flatMap(([name, rows]) => rows.length ? [{ name, sessions: rows }] : []), recent: [], more: false };
    }
    case "machine": {
      const machines = new Map<string, ProjectGroup>();
      for (const session of sessions) {
        const decrypted = titles[session.id];
        const origin = session.source === "synced" && decrypted?.state === "open" ? decrypted.title.origin ?? session.origin : session.origin;
        const name = originLabel(origin);
        const key = origin.kind === "computer" ? `computer:${origin.computerId}`
          : origin.kind === "cloud" ? `cloud:${origin.machineId}` : origin.kind;
        const group = machines.get(key);
        if (group) group.sessions.push(session);
        else machines.set(key, { key, name, sessions: [session] });
      }
      return { projects: [...machines.values()], recent: [], more: false };
    }
    default: {
      const exhaustive: never = grouping;
      return exhaustive;
    }
  }
}

const filterName = (name: string) => name.trim().toLowerCase().replace(/[ -]+/g, "-");
const sessionAgent = (session: UnifiedSession) => session.source === "synced" ? (session.thread.agent ?? null) : "nexus";

export function drawerAgents(sessions: readonly UnifiedSession[]): { id: string; label: string }[] {
  const names = new Map<string, string>();
  for (const session of sessions) {
    const id = sessionAgent(session);
    if (id && !names.has(id)) names.set(id, agentName(id) ?? id);
  }
  return [...names].map(([id, label]) => ({ id, label })).sort((a, b) => a.label.localeCompare(b.label));
}

export function filterDrawerSessions(sessions: readonly UnifiedSession[], query: string): readonly UnifiedSession[] {
  const name = filterName(query.replace(/^agent:\s*/i, ""));
  if (!name) return sessions;
  return sessions.filter((session) => {
    const id = sessionAgent(session);
    return id !== null && [id, agentName(id) ?? id].some((value) => filterName(value) === name);
  });
}
