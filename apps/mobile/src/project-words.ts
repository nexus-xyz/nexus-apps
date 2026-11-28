import { noteInProject, type AgentFailure, type AgentMemoryNote, type AgentProject, type AgentProjectLinked, type AgentProjectSession, type AgentProjectsGateway } from "@nexus/kit";

/**
 * A project on the phone's Assistant (A14), without React Native: the words of the Projects list and of one
 * project's page. The wire types and the strict readers are the kit's (`agent-projects.ts`). A project is a name its
 * errands share, or a Gateway project that groups repositories. Gateway keeps no file per project, so its files are
 * the repositories it holds.
 */

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The page's address: `/assistant-project?id=…` for a Gateway project, else `?name=…`. */
export const projectHref = (project: Pick<AgentProject, "id" | "name">) => ({
  pathname: "/assistant-project" as const,
  params: project.id ? { id: project.id, name: project.name } : { name: project.name },
});

/** The project the page asks for, from its address: by Gateway id when it has one, else by name. */
export function projectOfParams(params: { id?: string; name?: string }): Pick<AgentProject, "id" | "name"> | undefined {
  const name = params.name?.trim();
  if (!name) return undefined;
  return params.id ? { id: params.id, name } : { name };
}

/** A project's row in the list: what it is, then its repositories and errands. */
export function projectLine(project: AgentProject): string {
  const kind = project.id ? (project.personal ? "Personal Gateway project" : "Gateway project") : "Your errands";
  const parts = [kind];
  if (project.repos.length) parts.push(count(project.repos.length, "repository", "repositories"));
  parts.push(count(project.errands, "errand", "errands"));
  return parts.join(" · ");
}

/** What the list says under it about Gateway's projects, when they are not simply on. */
export function gatewayNote(gateway: AgentProjectsGateway): string | undefined {
  if (gateway === "off") return "Projects here are names your errands share. Gateway projects are not on for your organization.";
  if (gateway === "unavailable") return "Gateway's projects could not be reached just now. Your named projects still show.";
  return undefined;
}

/** What the list says with no project at all. */
export const NO_PROJECTS = "No project yet. Open an errand's menu and choose Move to project to start one.";

/** The offer on a name-only project that has a Gateway project of the same name. */
export const linkLabel = (project: Pick<AgentProject, "link">): string | undefined => (project.link ? `Link to ${project.link.name}` : undefined);

/** What the screen says once the link is done: how many errands and notes moved. */
export function linkedText(linked: AgentProjectLinked): string {
  const { errands, notes } = linked.linked;
  return `Linked to ${linked.project.name}. ${count(errands, "errand", "errands")} and ${count(notes, "note", "notes")} moved with it.`;
}

/** What a failed link says. */
export function linkFailedText(failure: AgentFailure): string {
  if (failure.error === "projects_off") return "Gateway's projects are not on for your organization.";
  if (failure.status === 404) return "That Gateway project is not one you are in.";
  return failure.detail || "The link did not save. Try again in a moment.";
}

const STATE: Record<AgentProjectSession["state"], string> = { running: "Running", done: "Done", failed: "Failed", stopped: "Stopped", ready: "Ready" };

/** A session's line: its state, then its repository and branch. */
export function sessionLine(session: AgentProjectSession): string {
  return [STATE[session.state], session.repo, session.branch].filter(Boolean).join(" · ");
}

/** What a session's row does: a synced session opens its link; a session on the desktop cannot open here. */
export type SessionOpen = { kind: "link"; url: string } | { kind: "desktop"; text: string };

/** The desktop's `nexus://` link opens only in Nexus for desktop, so the phone says where to open it and never follows it. */
export function sessionOpen(session: Pick<AgentProjectSession, "url" | "where">): SessionOpen {
  if (session.where === "desktop" || session.url.startsWith("nexus:")) return { kind: "desktop", text: "Open it in Nexus for desktop" };
  return { kind: "link", url: session.url };
}

/** A repository's row in Files: its name; the line says where the files come from. */
export const FILES_FOOTER = "A project keeps no file of its own. Its files are in these repositories.";

/** The text under Files with no repository. */
export const NO_FILES = "No repository. Link a Gateway project with repositories to see its files here.";

/** What "What Nexus knows here" says about its notes. */
export const KNOWS_FOOTER = "Only an errand filed in this project reads these notes.";

/** A project note's age, in its own words; the note keeps its text plain. */
export const noteDetail = (note: AgentMemoryNote): string => (note.editedAt ? "Edited" : "Added") + (note.source?.kind === "told" ? " from a chat" : "");

/** The notes of a project, newest first, in case the view carries others. */
export const projectNotes = (notes: AgentMemoryNote[], project: Pick<AgentProject, "id" | "name">): AgentMemoryNote[] => notes.filter((note) => noteInProject(note, project));

/** What the screen says once a note changed. */
export const noteToast = (kind: "added" | "edited" | "forgot"): string =>
  kind === "added" ? "Nexus will use the note in this project." : kind === "edited" ? "Nexus uses the new note from its next message." : "Nexus forgot the note.";
