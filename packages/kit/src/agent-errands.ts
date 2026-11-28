/**
 * The agent API's errands (A12), as every app reads them: Nexus UI, the desktop pane and the phone. The
 * routes are `GET /agent/errands`, `GET /agent/errands/{id}`, `POST /agent/errands/{id}` and, to start one from the
 * person's text, `POST /agent/errands` (A21·e; the agents service's `src/api/errands.ts`). Each errand is its own
 * thread: `PATCH /agent/errands/{id}` renames, pins, archives or marks it done, and
 * `POST /agent/errands/{id}/messages` replies about it. An errand can be filed in a project (A14): `PATCH` with
 * `project` files it, and `POST /agent/errands` with `project` starts it there. An errand can be handed off to a
 * coding session (A12·e): `errandHandoff` builds its prompt, `GET /agent/errands/{id}/handoff` answers the same, and
 * `POST /agent/errands/{id}/handoff` records the step. Each reader is strict: a body that is not one is undefined,
 * and a row that is not one is left out.
 */
import type { AgentMessage } from "./agent-chat";
import { redactShapes } from "./diagnostics";

/** `needs-you`: it waits on the person, and `need` says for what. */
export type AgentErrandStatus = "needs-you" | "running" | "done" | "failed";
/** A draft to confirm, a write card to decide, or a question the agent asked. */
export type AgentErrandNeed = "confirm" | "approval" | "question";
export type AgentErrandAction = "confirm" | "done" | "cancel";

/** One row of the list. */
export interface AgentErrand {
  /** `i` and a number, such as `i3`. */
  id: string;
  goal: string;
  status: AgentErrandStatus;
  need?: AgentErrandNeed;
  /** For an approval: the write's summary, in the agent's words. */
  detail?: string;
  /** How many steps it took. */
  steps: number;
  /** The last day, `YYYY-MM-DD`. */
  deadline?: string;
  /** When it opened, or when it closed for a closed one (ISO 8601). */
  at: string;
  /** The name the person gave it; absent, its goal names it (`errandName`). */
  title?: string;
  /** The person pinned it: it leads its group. */
  pinned?: true;
  /** The person archived it: it shows only in the history. */
  archived?: true;
  /** The project the person filed it in (A14). Absent: it is loose, under the Assistant. */
  project?: string;
  /** The Gateway project it is filed in, by id; `project` is then Gateway's name for it. */
  projectId?: string;
}

/** One turn of an errand's own thread: the person's reply about it, or the agent's, as the chat draws it. */
export interface AgentErrandTurn {
  role: "user" | "assistant";
  text: string;
  at: string;
  /** The agent's reply as the API rendered it: an answer, an approval card, a refusal. */
  messages?: AgentMessage[];
}

/** One thing the agent did for an errand. */
export interface AgentErrandStep {
  id: string;
  /** `tool`, `code` or `web`. */
  kind: string;
  text: string;
  urls: string[];
  at: string;
}

/** One piece of an outcome's evidence: a link, or a citation in the person's words. */
export type AgentErrandEvidence = { kind: "link"; url: string } | { kind: "citation"; text: string };

/** One errand, read whole. */
export interface AgentErrandView {
  errand: AgentErrand;
  constraints: string[];
  /** A decimal string of dollars. */
  budgetUsd?: string;
  /** Where it started: a surface's slug, such as `slack` or `api`. */
  surface: string;
  createdAt: string;
  steps: AgentErrandStep[];
  /** How it ended: `done`, `partly_done`, `failed` or `cancelled`. */
  outcome?: { status: string; at: string; evidence: AgentErrandEvidence[] };
  /** What the person may do now. */
  actions: AgentErrandAction[];
  /** Its own thread, oldest first; empty from an agent API that predates it. */
  thread: AgentErrandTurn[];
}

/**
 * What a `PATCH` changes: a name (`null` drops it), the pin, the archive, done, or its project (`null`: loose). At
 * least one. `projectId` files it in a Gateway project instead of `project`.
 */
export interface AgentErrandEdit {
  title?: string | null;
  pinned?: boolean;
  archived?: boolean;
  done?: true;
  project?: string | null;
  projectId?: string;
}

/** `POST /agent/errands/{id}/messages`'s body: the reply's messages, and the errand as it is now. */
export interface AgentErrandReply {
  messages: AgentMessage[];
  errand?: AgentErrandView;
}

/** The longest name an errand takes (`ERRAND_TITLE_LIMIT` in the agent core's `intents.ts`). */
export const AGENT_ERRAND_TITLE_MAX = 120;
/** The longest project name an errand is filed under (`ERRAND_PROJECT_LIMIT` in the agent core's `intents.ts`). */
export const AGENT_ERRAND_PROJECT_MAX = 60;

/** What a POST did. `discarded`: a draft went, and there is no errand to show. */
export type AgentErrandActed = { state: "confirmed" | "done" | "cancelled"; errand: AgentErrandView } | { state: "discarded" };

/** What `POST /agent/errands` did: the new errand, read whole. */
export type AgentErrandOpened = { state: "opened"; errand: AgentErrandView };

/** The longest goal `POST /agent/errands` takes (`ERRAND_GOAL_CHARS` in the agent API). */
export const AGENT_ERRAND_GOAL_CHARS = 500;

/** An errand's id as the agent API writes it. */
export const AGENT_ERRAND_ID = /^i\d{1,6}$/;

const STATUSES: readonly string[] = ["needs-you", "running", "done", "failed"];
const NEEDS: readonly string[] = ["confirm", "approval", "question"];
const ACTIONS: readonly string[] = ["confirm", "done", "cancel"];

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const strings = (value: unknown): string[] => (Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : []);

/** One row, read strictly. */
export function agentErrandOf(value: unknown): AgentErrand | undefined {
  if (!isObject(value)) return undefined;
  const { id, goal, status, need, detail, steps, deadline, at } = value;
  if (typeof id !== "string" || !AGENT_ERRAND_ID.test(id) || typeof goal !== "string" || typeof at !== "string") return undefined;
  if (typeof status !== "string" || !STATUSES.includes(status)) return undefined;
  const { title, pinned, archived, project } = value;
  return {
    id,
    goal,
    status: status as AgentErrandStatus,
    ...(typeof need === "string" && NEEDS.includes(need) ? { need: need as AgentErrandNeed } : {}),
    ...(typeof detail === "string" && detail ? { detail } : {}),
    steps: typeof steps === "number" && Number.isInteger(steps) && steps >= 0 ? steps : 0,
    ...(typeof deadline === "string" && /^\d{4}-\d{2}-\d{2}$/.test(deadline) ? { deadline } : {}),
    at,
    ...(typeof title === "string" && title.trim() ? { title } : {}),
    ...(pinned === true ? { pinned: true as const } : {}),
    ...(archived === true ? { archived: true as const } : {}),
    ...(typeof project === "string" && project.trim() ? { project } : {}),
    ...(typeof project === "string" && project.trim() && typeof value.projectId === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(value.projectId) ? { projectId: value.projectId } : {}),
  };
}

/** The name an errand goes by: the one the person gave it, else its goal. */
export function errandName(errand: Pick<AgentErrand, "goal" | "title">): string {
  return errand.title ?? errand.goal;
}

/** `GET /agent/errands`'s history: the closed and archived errands, newest first; empty from an API without one. */
export function errandHistoryOf(body: unknown): AgentErrand[] {
  if (!isObject(body) || !Array.isArray(body.history)) return [];
  return body.history.flatMap((row) => agentErrandOf(row) ?? []);
}

/** The errands that wait on the person, pinned first, for the main chat's overview ("2 errands need you"). */
export function errandsNeedingYou(errands: readonly AgentErrand[]): AgentErrand[] {
  const needs = errands.filter((errand) => errand.status === "needs-you" && !errand.archived);
  return [...needs.filter((errand) => errand.pinned), ...needs.filter((errand) => !errand.pinned)];
}

/** The history rows whose name or goal holds every word of `query`, ignoring case. An empty query holds them all. */
export function searchErrands(errands: readonly AgentErrand[], query: string): AgentErrand[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return errands.filter((errand) => {
    const text = `${errand.title ?? ""} ${errand.goal}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

const MESSAGE_KINDS: readonly string[] = ["answer", "refused", "pending_action", "notice", "memories", "forgotten", "insight", "error"];

/** One thread turn, read strictly; a message the kit does not know is left out of it. */
function errandTurnOf(value: unknown): AgentErrandTurn | undefined {
  if (!isObject(value) || (value.role !== "user" && value.role !== "assistant") || typeof value.text !== "string" || typeof value.at !== "string") return undefined;
  const messages = Array.isArray(value.messages) ? value.messages.filter((message): message is AgentMessage => isObject(message) && typeof message.kind === "string" && MESSAGE_KINDS.includes(message.kind)) : undefined;
  return { role: value.role, text: value.text, at: value.at, ...(messages?.length ? { messages } : {}) };
}

/** A `PATCH`'s body: the change alone, as the agent API reads it strictly. */
export function errandEditBody(edit: AgentErrandEdit): string {
  return JSON.stringify(edit);
}

/** A reply's body: `{ message }` alone. */
export function errandMessageBody(message: string): string {
  return JSON.stringify({ message });
}

/** `POST /agent/errands/{id}/messages`'s body; undefined when it is not one. */
export function errandReplyOf(body: unknown): AgentErrandReply | undefined {
  if (!isObject(body) || !Array.isArray(body.messages)) return undefined;
  const messages = body.messages.filter((message): message is AgentMessage => isObject(message) && typeof message.kind === "string" && MESSAGE_KINDS.includes(message.kind));
  const errand = body.errand === undefined ? undefined : errandViewOf(body.errand);
  return { messages, ...(errand ? { errand } : {}) };
}

/** `GET /agent/errands`'s body: its rows in the API's order. */
export function errandListOf(body: unknown): AgentErrand[] | undefined {
  if (!isObject(body) || !Array.isArray(body.errands)) return undefined;
  return body.errands.flatMap((row) => agentErrandOf(row) ?? []);
}

/** `GET /agent/errands/{id}`'s body. */
export function errandViewOf(body: unknown): AgentErrandView | undefined {
  if (!isObject(body)) return undefined;
  const errand = agentErrandOf(body.errand);
  if (!errand || typeof body.surface !== "string" || typeof body.createdAt !== "string" || !Array.isArray(body.steps)) return undefined;
  const steps = body.steps.flatMap((item): AgentErrandStep[] => {
    if (!isObject(item) || typeof item.id !== "string" || typeof item.text !== "string" || typeof item.at !== "string") return [];
    return [{ id: item.id, kind: typeof item.kind === "string" ? item.kind : "tool", text: item.text, urls: strings(item.urls), at: item.at }];
  });
  const outcome = isObject(body.outcome) && typeof body.outcome.status === "string" && typeof body.outcome.at === "string" ? body.outcome : undefined;
  const evidence = (Array.isArray(outcome?.evidence) ? outcome.evidence : []).flatMap((item): AgentErrandEvidence[] => {
    if (isObject(item) && item.kind === "link" && typeof item.url === "string") return [{ kind: "link", url: item.url }];
    if (isObject(item) && item.kind === "citation" && typeof item.text === "string") return [{ kind: "citation", text: item.text }];
    return [];
  });
  return {
    errand,
    constraints: strings(body.constraints),
    ...(typeof body.budgetUsd === "string" ? { budgetUsd: body.budgetUsd } : {}),
    surface: body.surface,
    createdAt: body.createdAt,
    steps,
    ...(outcome ? { outcome: { status: outcome.status as string, at: outcome.at as string, evidence } } : {}),
    actions: strings(body.actions).filter((action): action is AgentErrandAction => ACTIONS.includes(action)),
    thread: (Array.isArray(body.thread) ? body.thread : []).flatMap((turn) => errandTurnOf(turn) ?? []),
  };
}

/** `POST /agent/errands/{id}`'s body. */
export function errandActedOf(body: unknown): AgentErrandActed | undefined {
  if (!isObject(body)) return undefined;
  if (body.state === "discarded") return { state: "discarded" };
  if (body.state !== "confirmed" && body.state !== "done" && body.state !== "cancelled") return undefined;
  const errand = errandViewOf(body.errand);
  return errand ? { state: body.state, errand } : undefined;
}

/** `POST /agent/errands`'s body. */
export function errandOpenedOf(body: unknown): AgentErrandOpened | undefined {
  if (!isObject(body) || body.state !== "opened") return undefined;
  const errand = errandViewOf(body.errand);
  return errand ? { state: "opened", errand } : undefined;
}

/**
 * The body that starts an errand: the goal, trimmed to what the agent API takes, and the project it starts in (A14):
 * a Gateway project by `projectId`, else a name.
 */
export function errandGoalBody(goal: string, project?: string, projectId?: string): string {
  const said = goal.trim().slice(0, AGENT_ERRAND_GOAL_CHARS);
  if (projectId) return JSON.stringify({ goal: said, projectId });
  const named = project?.trim().slice(0, AGENT_ERRAND_PROJECT_MAX);
  return JSON.stringify({ goal: said, ...(named ? { project: named } : {}) });
}

/**
 * The projects the person's errands are filed in (A14), by name, from the list and the history. A project has no
 * record of its own yet: it is the name its errands share, in this organization only.
 */
export function errandProjects(...lists: readonly (readonly AgentErrand[])[]): string[] {
  const names = new Set(lists.flatMap((list) => list.flatMap((errand) => (errand.project ? [errand.project] : []))));
  return [...names].sort((a, b) => a.localeCompare(b));
}

/**
 * Whether a failed list read means the agent API has no errands for this person to show: errands are off there
 * (503 `errands_off`), or the API predates them (404). An app then hides its errands.
 */
export function errandsOff(status: number, body?: { error?: unknown }): boolean {
  return status === 404 || (status === 503 && body?.error === "errands_off");
}

/** The step an errand records when the person hands it off to a session (A12·e). */
export const ERRAND_HANDED_OFF = "Handed off to a session";
/** The longest hand-off prompt: the desktop's own bound on a draft link's prompt (`MAX_PROMPT` in `prompt_link.rs`). */
export const AGENT_ERRAND_HANDOFF_CHARS = 8_000;
/** The longest session link `POST /agent/errands/{id}/handoff` takes. */
export const AGENT_ERRAND_HANDOFF_URL_CHARS = 2_000;
/** The most steps and thread turns a hand-off carries, the newest kept, and the longest of each. */
const HANDOFF_STEPS = 10;
const HANDOFF_TURNS = 6;
const HANDOFF_LINE_CHARS = 500;
const HANDOFF_TURN_CHARS = 1_000;

/** An errand handed off to a session (A12·e): the prompt for the session's draft, and the repository it names. */
export interface AgentErrandHandoff {
  prompt: string;
  /** `owner/name` on GitHub, when the errand or its project names one. */
  repo?: string;
}

const REPO = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/[A-Za-z0-9._-]{1,100}$/;
const GITHUB_URL = /\bhttps?:\/\/(?:www\.)?github\.com\/([A-Za-z0-9][A-Za-z0-9-]{0,38})\/([A-Za-z0-9._-]{1,100})/i;
/** "repo owner/name" or "repository owner/name", as a person writes it. A bare `a/b` is too often not one. */
const NAMED_REPO = /\b(?:repo|repository)\s+`?([A-Za-z0-9][A-Za-z0-9-]{0,38})\/([A-Za-z0-9._-]{1,100})/i;
/** An address's local part is at most 64 characters (RFC 5321); the bound keeps the scan linear. */
const EMAIL = /[A-Za-z0-9._%+-]{1,64}@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g;
/** Control characters other than the line break, which a draft drops anyway. */
const CONTROL = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g;

/** `owner/name` from a match: no `.git`, no full stop that ends a sentence. Undefined when it is not one. */
function repoFrom(owner: string, name: string): string | undefined {
  const repo = `${owner}/${name.replace(/\.+$/, "").replace(/\.git$/i, "")}`;
  return REPO.test(repo) ? repo : undefined;
}

/** The repository a text names: a GitHub link, or "repo owner/name". */
function repoIn(text: string): string | undefined {
  const linked = GITHUB_URL.exec(text);
  if (linked) return repoFrom(linked[1]!, linked[2]!);
  const named = NAMED_REPO.exec(text);
  return named ? repoFrom(named[1]!, named[2]!) : undefined;
}

type HandoffSource = Pick<AgentErrandView, "errand" | "constraints" | "steps" | "thread">;

/**
 * The repository an errand names, for its session (A12·e): its project when that is `owner/name`, else a GitHub link
 * or "repo owner/name" in its name, goal or constraints, else in its steps' links, else in the person's own replies.
 */
export function errandHandoffRepo(view: HandoffSource): string | undefined {
  const { errand } = view;
  const project = errand.project?.trim();
  if (project && REPO.test(project)) return project;
  const texts = [errand.title ?? "", errand.goal, ...view.constraints, ...view.steps.flatMap((step) => step.urls), ...view.thread.filter((turn) => turn.role === "user").map((turn) => turn.text)];
  for (const text of texts) {
    const repo = repoIn(text);
    if (repo) return repo;
  }
  return undefined;
}

/** Text for a hand-off: no secret shapes, email addresses or control characters, and at most `limit` characters. */
function handoffText(text: string, limit: number, oneLine = false): string {
  let clean = redactShapes(text).replace(EMAIL, "[email]").replace(/\r\n?/g, "\n").replace(CONTROL, "");
  if (oneLine) clean = clean.replace(/\s+/g, " ");
  clean = clean.trim();
  return clean.length > limit ? `${clean.slice(0, limit - 1).trimEnd()}…` : clean;
}

/**
 * An errand as a coding session's prompt (A12·e): its goal, what it keeps to, its last day and project, the repository,
 * its steps so far, then the last turns of its thread. Every surface builds it here, so a hand-off reads the same
 * everywhere. Secret shapes and email addresses are taken out, and approval cards are left out, so other people's
 * details do not travel. At most `AGENT_ERRAND_HANDOFF_CHARS`: the oldest turns go first. `assistant`: the
 * Assistant's name, which the person may change; Nexus by default.
 */
export function errandHandoff(view: HandoffSource, assistant = "Nexus"): AgentErrandHandoff {
  const { errand } = view;
  const repo = errandHandoffRepo(view);
  const name = errand.title?.trim();
  const head = [
    name && name !== errand.goal ? `Continue my errand "${handoffText(name, AGENT_ERRAND_TITLE_MAX, true)}" in this session.` : "Continue my errand in this session.",
    "",
    `Goal: ${handoffText(errand.goal, HANDOFF_LINE_CHARS, true)}`,
    ...(view.constraints.length ? ["It keeps to:", ...view.constraints.map((rule) => `- ${handoffText(rule, HANDOFF_LINE_CHARS, true)}`)] : []),
    ...(errand.deadline ? [`Its last day: ${errand.deadline}`] : []),
    ...(errand.project ? [`Its project: ${handoffText(errand.project, AGENT_ERRAND_PROJECT_MAX, true)}`] : []),
    ...(repo ? [`Repository: ${repo}`] : []),
  ];
  const steps = view.steps.filter((step) => step.text !== ERRAND_HANDED_OFF).slice(-HANDOFF_STEPS);
  const done = steps.length ? ["", `What ${assistant} did so far:`, ...steps.map((step) => `- ${handoffText(step.text, HANDOFF_LINE_CHARS, true)}`)] : [];
  const turns = view.thread
    .filter((turn) => turn.text.trim())
    .slice(-HANDOFF_TURNS)
    .map((turn) => `${turn.role === "user" ? "Me" : assistant}: ${handoffText(turn.text, HANDOFF_TURN_CHARS)}`);
  const write = (kept: string[]) => [...head, ...done, ...(kept.length ? ["", "The errand's thread, newest last:", ...kept] : [])].join("\n");
  let prompt = write(turns);
  for (let from = 1; prompt.length > AGENT_ERRAND_HANDOFF_CHARS && from <= turns.length; from += 1) prompt = write(turns.slice(from));
  if (prompt.length > AGENT_ERRAND_HANDOFF_CHARS) prompt = `${prompt.slice(0, AGENT_ERRAND_HANDOFF_CHARS - 1).trimEnd()}…`;
  return { prompt, ...(repo ? { repo } : {}) };
}

/** Whether an errand can be handed off now (A12·e): it is open and confirmed, so it can record the step. */
export function errandCanHandOff(view: Pick<AgentErrandView, "actions">): boolean {
  return view.actions.includes("done");
}

/**
 * The desktop's draft link for a hand-off (`nexus-desktop-core`'s `prompt_link`): `nexus://new?prompt=…&repo=owner/name`.
 * Nexus for desktop fills a New session draft from it and never sends it. Written by hand, because React Native's
 * URLSearchParams is partial.
 */
export function errandHandoffLink(handoff: AgentErrandHandoff): string {
  const query = [`prompt=${encodeURIComponent(handoff.prompt.slice(0, AGENT_ERRAND_HANDOFF_CHARS))}`, ...(handoff.repo ? [`repo=${encodeURIComponent(handoff.repo)}`] : [])];
  return `nexus://new?${query.join("&")}`;
}

/** `GET /agent/errands/{id}/handoff`'s body, read strictly: a prompt within the bound, and a repository that is one. */
export function errandHandoffOf(body: unknown): AgentErrandHandoff | undefined {
  if (!isObject(body) || typeof body.prompt !== "string" || !body.prompt.trim() || body.prompt.length > AGENT_ERRAND_HANDOFF_CHARS) return undefined;
  const repo = typeof body.repo === "string" && REPO.test(body.repo) ? body.repo : undefined;
  return { prompt: body.prompt, ...(repo ? { repo } : {}) };
}

/** The session link a hand-off records: http(s) and within the bound, else none. */
export function errandHandoffUrl(url: unknown): string | undefined {
  return typeof url === "string" && /^https?:\/\/[^\s]+$/i.test(url) && url.length <= AGENT_ERRAND_HANDOFF_URL_CHARS ? url : undefined;
}

/** `POST /agent/errands/{id}/handoff`'s body: `{ url }` with the session's link when the app knows it, else `{}`. */
export function errandHandedOffBody(url?: string): string {
  const link = errandHandoffUrl(url);
  return JSON.stringify(link ? { url: link } : {});
}
