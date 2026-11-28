import type { CloudHistoryEvent, ModelEntry, Session } from "./api";
import type { OpenedEvent } from "./synced-sessions";

/** Coding agents that can run a headless cloud session. */
export type CloudSessionAgent = "opencode" | "claude" | "codex";

export interface CloudModelGroup {
  agent: CloudSessionAgent;
  agentLabel: string;
  groupLabel: string;
  models: ModelEntry[];
}

export const CLOUD_AGENT_LABELS: Record<CloudSessionAgent, string> = {
  opencode: "OpenCode",
  claude: "Claude Code",
  codex: "Codex",
};

const GROUP_META: readonly { agent: CloudSessionAgent; groupLabel: string }[] = [
  { agent: "opencode", groupLabel: "OpenCode · Open weights" },
  { agent: "claude", groupLabel: "Claude Code · Anthropic" },
  { agent: "codex", groupLabel: "Codex · OpenAI" },
];

const PREFERRED_DEFAULT_MODELS: readonly string[] = ["zai/glm-5.3", "zai/glm-5.3-flash"];

/**
 * Maps a Gateway catalog model (`GET /v1/models`) to the cloud coding agent that runs it,
 * matching `nexus_auto_router::cloud_session::choose_agent`:
 * - open-weight models supporting `chat` -> `opencode`
 * - Anthropic models supporting `anthropic_messages` -> `claude`
 * - OpenAI models supporting `openai_responses` -> `codex`
 * Disallowed or non-supported models return `null`.
 */
export function cloudAgentForModel(
  entry: Pick<ModelEntry, "id" | "owned_by" | "surfaces" | "openWeights" | "allowed" | "status">,
): CloudSessionAgent | null {
  if (entry.status !== "supported" || entry.allowed === false) return null;
  if (entry.openWeights === true && entry.surfaces.includes("chat")) return "opencode";
  if (entry.owned_by === "anthropic" && entry.surfaces.includes("anthropic_messages")) return "claude";
  if (entry.owned_by === "openai" && entry.surfaces.includes("openai_responses")) return "codex";
  return null;
}

/** Groups eligible cloud models by agent in display order (OpenCode, Claude Code, Codex). */
export function groupCloudModels(models: readonly ModelEntry[]): CloudModelGroup[] {
  const buckets: Record<CloudSessionAgent, ModelEntry[]> = { opencode: [], claude: [], codex: [] };
  for (const entry of models) {
    const agent = cloudAgentForModel(entry);
    if (agent) buckets[agent].push(entry);
  }
  const sortModels = (list: ModelEntry[], agent: CloudSessionAgent) =>
    [...list].sort((a, b) => {
      const aDefault = isDefaultFor(a, agent) ? 0 : 1;
      const bDefault = isDefaultFor(b, agent) ? 0 : 1;
      if (aDefault !== bDefault) return aDefault - bDefault;
      return a.id.localeCompare(b.id);
    });
  return GROUP_META.map(({ agent, groupLabel }) => ({
    agent,
    agentLabel: CLOUD_AGENT_LABELS[agent],
    groupLabel,
    models: sortModels(buckets[agent], agent),
  })).filter((group) => group.models.length > 0);
}

function isDefaultFor(entry: ModelEntry, agent: CloudSessionAgent): boolean {
  if (agent === "opencode") return PREFERRED_DEFAULT_MODELS.includes(entry.id) || Boolean(entry.default_for?.includes("chat"));
  if (agent === "claude") return Boolean(entry.default_for?.includes("claude"));
  return Boolean(entry.default_for?.includes("codex"));
}

/** Chooses a sensible initial model from grouped cloud models (prefers an open-weight OpenCode model). */
export function defaultCloudModel(groups: readonly CloudModelGroup[]): ModelEntry | null {
  const all = groups.flatMap((group) => group.models);
  for (const preferred of PREFERRED_DEFAULT_MODELS) {
    const match = all.find((entry) => entry.id === preferred);
    if (match) return match;
  }
  return groups[0]?.models[0] ?? null;
}

/**
 * Normalizes a GitHub repository input (`https://github.com/owner/name`, `git@github.com:owner/name.git`,
 * or `owner/name`) into `owner/name`. Returns the trimmed input if it does not match a GitHub URL so
 * validation can report a clear message.
 */
export function normalizeRepoInput(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  const https = trimmed.match(/^https?:\/\/(?:www\.)?github\.com\/([^/\s]+\/[^/\s?#]+?)(?:\.git)?\/?(?:[?#].*)?$/i);
  if (https) return https[1]!;
  const ssh = trimmed.match(/^git@github\.com:([^/\s]+\/[^/\s]+?)(?:\.git)?\/?$/i);
  if (ssh) return ssh[1]!;
  const stripped = trimmed.replace(/^github\.com\//i, "").replace(/\.git$/i, "").replace(/\/+$/, "");
  return stripped;
}

const REPO_SEGMENT = /^[A-Za-z0-9._-]+$/;

/** Whether `repo` matches Gateway's GitHub `owner/name` shape. */
export function isValidRepo(repo: string): boolean {
  const parts = repo.split("/");
  if (parts.length !== 2) return false;
  return parts.every((part) => part.length > 0 && part !== "." && part !== ".." && REPO_SEGMENT.test(part));
}

/** Merges incremental cloud history pages by `seq`, ascending, with later copies replacing earlier ones. */
export function mergeCloudHistoryEvents(
  held: readonly CloudHistoryEvent[],
  fresh: readonly CloudHistoryEvent[],
): CloudHistoryEvent[] {
  const bySeq = new Map<number, CloudHistoryEvent>();
  for (const event of held) bySeq.set(event.seq, event);
  for (const event of fresh) bySeq.set(event.seq, event);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/**
 * Converts Gateway cloud machine history events (`GET /v1/cloud/machines/{id}/history/{threadId}/events`)
 * into `OpenedEvent[]` that `@nexus/kit/synced-timeline`'s `syncedTimeline` folds into `SessionTimeline`.
 * Cloud history sequences start at 1 while `syncedTimeline` indexes `queueCancel`/`queueEdit` against 0-based seq.
 */
export function cloudEventsToOpened(
  events: readonly CloudHistoryEvent[],
  meta: { threadId?: string; title?: string | null; repo?: string } = {},
): { opened: OpenedEvent[]; omitted: number; unreadable: number } {
  const opened: OpenedEvent[] = [];
  let omitted = 0;
  let unreadable = 0;
  const thread = {
    localId: meta.threadId ?? "",
    title: meta.title ?? meta.repo ?? "Cloud session",
    projectName: meta.repo ?? "",
    projectPath: "",
  };
  for (const entry of events) {
    const seq = Math.max(0, entry.seq - 1);
    if (entry.body === null) {
      omitted += 1;
      continue;
    }
    try {
      const parsed = JSON.parse(entry.body) as unknown;
      if (parsed && typeof parsed === "object" && typeof (parsed as { kind?: unknown }).kind === "string") {
        opened.push({
          state: "open",
          seq,
          at: entry.at,
          thread,
          event: parsed as { kind: string } & Record<string, unknown>,
        });
      } else {
        unreadable += 1;
      }
    } catch {
      unreadable += 1;
    }
  }
  return { opened, omitted, unreadable };
}

/** Short display title for a cloud session row or header. */
export function cloudSessionTitle(session: Pick<Session, "title" | "repo" | "id">): string {
  const title = session.title?.trim();
  if (title) return title;
  if (session.repo) return session.repo;
  return session.id;
}
