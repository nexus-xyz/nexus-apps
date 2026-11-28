/**
 * The agent API's projects (A14), as every app reads them. An Assistant project is a name its errands
 * share, or a Gateway project (behind `NEXUS_PROJECTS`) that groups repositories and people. The person
 * sees only the Gateway projects they are in.
 *
 * - `GET /agent/projects` answers `AgentProjectList`: the person's projects, and whether Gateway's projects are on.
 * - `GET /agent/projects/view?id=<id>` or `?name=<name>` answers `AgentProjectView`: one project's errands, its
 *   repositories, the sessions its errands named, and what Nexus knows there.
 * - `POST /agent/projects/link` with `{ name, projectId }` binds a name-only project to the Gateway project of the
 *   same name: its errands and notes move with it.
 * - `PATCH /agent/errands/{id}` and `POST /agent/errands` take `projectId` to file an errand in a Gateway project.
 * - `POST /agent/memory/notes` takes `project` to scope a note to a project.
 *
 * Gateway's projects off, each project is a name: `gateway` says `off`, and nothing else changes. Each reader is
 * strict: a body that is not one is undefined, and a row that is not one is left out.
 */
import { agentErrandOf, errandHistoryOf, type AgentErrand } from "./agent-errands";
import { memoryNoteOf, type AgentMemoryNote } from "./agent-chat";

export const AGENT_PROJECTS_PATH = "/agent/projects";
export const AGENT_PROJECT_VIEW_PATH = "/agent/projects/view";
export const AGENT_PROJECT_LINK_PATH = "/agent/projects/link";

/** A Gateway project's id as an errand keeps it (`PROJECT_ID` in the agent core's `intents.ts`). */
export const AGENT_PROJECT_ID = /^[A-Za-z0-9_-]{1,64}$/;

/** Whether Gateway's projects answer: `on`, `off` (`NEXUS_PROJECTS` unset), or `unavailable` now. */
export type AgentProjectsGateway = "on" | "off" | "unavailable";

/** The Gateway project a name-only project may bind to: the same name, and the person is in it. */
export interface AgentProjectLinkOffer {
  id: string;
  name: string;
}

/** One project of the list. */
export interface AgentProject {
  name: string;
  /** The Gateway project's id; absent for a name-only project. */
  id?: string;
  /** Its repositories as `owner/name`; empty for a name-only project or one with no repository. */
  repos: string[];
  /** The person's role in the Gateway project. */
  role?: "member" | "admin";
  /** A personal Gateway project: its owner's alone. */
  personal?: true;
  /** How many of the person's errands are filed in it, open or closed. */
  errands: number;
  /** A name-only project with a Gateway project of the same name offers to bind to it. */
  link?: AgentProjectLinkOffer;
}

export interface AgentProjectList {
  projects: AgentProject[];
  gateway: AgentProjectsGateway;
}

/** A session one of the project's errands named: a link card the agent showed in its thread. */
export interface AgentProjectSession {
  title: string;
  state: "running" | "done" | "failed" | "stopped" | "ready";
  where: "synced" | "desktop";
  url: string;
  repo?: string;
  branch?: string;
  /** The synced thread's id. */
  id?: string;
  /** When the errand's thread showed it (ISO 8601). */
  at: string;
  /** The errand whose thread showed it. */
  errand: string;
}

/** Where a project's files come from. Gateway keeps no file per project: they come from its repositories. */
export interface AgentProjectFiles {
  source: "repositories";
  repos: { fullName: string; url: string }[];
}

/** `GET /agent/projects/view`. */
export interface AgentProjectView {
  project: AgentProject;
  gateway: AgentProjectsGateway;
  /** Its errands that are open or closed lately, as the list orders them. */
  errands: AgentErrand[];
  /** Its closed and archived errands, newest first. */
  history: AgentErrand[];
  sessions: AgentProjectSession[];
  files: AgentProjectFiles;
  /** What Nexus knows here: the person's notes scoped to it, newest first. */
  notes: AgentMemoryNote[];
}

/** A note's project scope, as `POST /agent/memory/notes` takes it and the view sends it. */
export interface AgentNoteProject {
  id?: string;
  name: string;
}

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const SESSION_STATES: readonly string[] = ["running", "done", "failed", "stopped", "ready"];

/** The longest project name (`ERRAND_PROJECT_LIMIT` in the agent core). */
const NAME_MAX = 60;
const nameOf = (value: unknown): string | undefined => (typeof value === "string" && value.trim() && value.length <= NAME_MAX ? value : undefined);

/** A link offer, read strictly. */
function linkOf(value: unknown): AgentProjectLinkOffer | undefined {
  if (!isObject(value) || typeof value.id !== "string" || !AGENT_PROJECT_ID.test(value.id)) return undefined;
  const name = nameOf(value.name);
  return name ? { id: value.id, name } : undefined;
}

/** One project row, read strictly. */
export function agentProjectOf(value: unknown): AgentProject | undefined {
  if (!isObject(value)) return undefined;
  const name = nameOf(value.name);
  if (!name) return undefined;
  if (value.id !== undefined && (typeof value.id !== "string" || !AGENT_PROJECT_ID.test(value.id))) return undefined;
  const link = value.id === undefined ? linkOf(value.link) : undefined;
  return {
    name,
    ...(typeof value.id === "string" ? { id: value.id } : {}),
    repos: Array.isArray(value.repos) ? value.repos.filter((repo): repo is string => typeof repo === "string" && /^[A-Za-z0-9-]+\/(?!\.\.?$)[\w.-]+$/.test(repo)) : [],
    ...(value.role === "member" || value.role === "admin" ? { role: value.role } : {}),
    ...(value.personal === true ? { personal: true as const } : {}),
    errands: typeof value.errands === "number" && Number.isInteger(value.errands) && value.errands >= 0 ? value.errands : 0,
    ...(link ? { link } : {}),
  };
}

const gatewayOf = (value: unknown): AgentProjectsGateway | undefined => (value === "on" || value === "off" || value === "unavailable" ? value : undefined);

/** `GET /agent/projects`'s body. */
export function agentProjectListOf(body: unknown): AgentProjectList | undefined {
  if (!isObject(body) || !Array.isArray(body.projects)) return undefined;
  const gateway = gatewayOf(body.gateway);
  if (!gateway) return undefined;
  return { projects: body.projects.flatMap((row) => agentProjectOf(row) ?? []), gateway };
}

/** One session row, read strictly: a link the app opens must be `https:` or `nexus:`. */
function projectSessionOf(value: unknown): AgentProjectSession | undefined {
  if (!isObject(value) || typeof value.title !== "string" || typeof value.url !== "string" || typeof value.at !== "string" || typeof value.errand !== "string") return undefined;
  if (!/^(https:|nexus:)/.test(value.url)) return undefined;
  if (typeof value.state !== "string" || !SESSION_STATES.includes(value.state)) return undefined;
  if (value.where !== "synced" && value.where !== "desktop") return undefined;
  return {
    title: value.title,
    state: value.state as AgentProjectSession["state"],
    where: value.where,
    url: value.url,
    ...(typeof value.repo === "string" ? { repo: value.repo } : {}),
    ...(typeof value.branch === "string" ? { branch: value.branch } : {}),
    ...(typeof value.id === "string" ? { id: value.id } : {}),
    at: value.at,
    errand: value.errand,
  };
}

/** `GET /agent/projects/view`'s body. */
export function agentProjectViewOf(body: unknown): AgentProjectView | undefined {
  if (!isObject(body)) return undefined;
  const project = agentProjectOf(body.project);
  const gateway = gatewayOf(body.gateway);
  if (!project || !gateway || !Array.isArray(body.errands) || !isObject(body.files) || body.files.source !== "repositories") return undefined;
  const repos = (Array.isArray(body.files.repos) ? body.files.repos : []).flatMap((repo): AgentProjectFiles["repos"] => {
    if (!isObject(repo) || typeof repo.fullName !== "string" || typeof repo.url !== "string" || !repo.url.startsWith("https://github.com/")) return [];
    return [{ fullName: repo.fullName, url: repo.url }];
  });
  return {
    project,
    gateway,
    errands: body.errands.flatMap((row) => agentErrandOf(row) ?? []),
    history: errandHistoryOf(body),
    sessions: (Array.isArray(body.sessions) ? body.sessions : []).flatMap((row) => projectSessionOf(row) ?? []),
    files: { source: "repositories", repos },
    notes: (Array.isArray(body.notes) ? body.notes : []).flatMap((row) => memoryNoteOf(row) ?? []),
  };
}

/** The query that reads one project's view: by its Gateway id, else by its name. */
export function projectViewQuery(project: Pick<AgentProject, "id" | "name">): string {
  return project.id ? `id=${encodeURIComponent(project.id)}` : `name=${encodeURIComponent(project.name)}`;
}

/** `POST /agent/projects/link`'s body. */
export function projectLinkBody(name: string, projectId: string): string {
  return JSON.stringify({ name, projectId });
}

/** `POST /agent/projects/link`'s answer: how many errands and notes moved, and the project as it is now. */
export interface AgentProjectLinked {
  linked: { errands: number; notes: number };
  project: AgentProject;
}

export function agentProjectLinkedOf(body: unknown): AgentProjectLinked | undefined {
  if (!isObject(body) || !isObject(body.linked)) return undefined;
  const project = agentProjectOf(body.project);
  const { errands, notes } = body.linked;
  if (!project || typeof errands !== "number" || typeof notes !== "number") return undefined;
  return { linked: { errands, notes }, project };
}

/** A project note's add body: the text, scoped to the project. */
export function projectNoteBody(text: string, project: Pick<AgentProject, "id" | "name">): string {
  return JSON.stringify({ text, project: { ...(project.id ? { id: project.id } : {}), name: project.name } });
}

/** Whether a note is scoped to `project`: by its Gateway id when it has one, else by its name. */
export function noteInProject(note: Pick<AgentMemoryNote, "project">, project: Pick<AgentProject, "id" | "name">): boolean {
  if (!note.project) return false;
  if (project.id) return note.project.id === project.id;
  return !note.project.id && note.project.name === project.name;
}

/** "In Fundraise · 2 repositories": what a project's page says under its name. */
export function projectSubtitle(project: AgentProject, gateway: AgentProjectsGateway): string {
  if (!project.id) return gateway === "on" && project.link ? `A project of your errands. ${project.link.name} is also a Gateway project: link them to use its repositories.` : "A project of your errands. No repository.";
  if (!project.repos.length) return "A Gateway project with no repository yet. Sessions, errands and what Nexus knows, together.";
  const count = project.repos.length;
  return `A Gateway project with ${count} ${count === 1 ? "repository" : "repositories"}. Its files come from them.`;
}
