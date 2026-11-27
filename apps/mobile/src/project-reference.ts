import { noteInProject, type AgentClient, type AgentProject, type CoreClient, type RepoInfo } from "@nexus/kit";

const MAX_NOTES = 8;
const MAX_NOTE_CHARS = 1000;
const MAX_REPOS = 3;
const MAX_PATHS = 40;
const MAX_PATH_CHARS = 240;
const GITHUB_REPO = /^(?!\.{1,2}\/)[A-Za-z0-9_.-]{1,100}\/(?!\.{1,2}$)[A-Za-z0-9_.-]{1,100}$/;
const CONTROL = /[\u0000-\u001f\u007f]/;

type ProjectIdentity = Pick<AgentProject, "id" | "name">;

async function repoFileList(core: Pick<CoreClient, "readConnector">, repo: string): Promise<string[]> {
  const lines = [`### ${repo}`];
  try {
    const tree = await core.readConnector<RepoInfo>("github", "repo", { repo: `https://github.com/${repo}` });
    if (!tree || `${tree.owner}/${tree.name}`.toLowerCase() !== repo.toLowerCase() || !Array.isArray(tree.paths)) {
      return [...lines, "File list unavailable."];
    }
    const paths = tree.paths.filter((path) => typeof path === "string" && path.length > 0 && path.length <= MAX_PATH_CHARS && !CONTROL.test(path));
    const shown = paths.slice(0, MAX_PATHS);
    const omitted = tree.truncated || tree.paths.length > shown.length || (tree.total ?? 0) > shown.length;
    if (shown.length) lines.push(...shown.map((path) => `- ${path}`));
    else if (!omitted) lines.push("No files.");
    if (omitted) lines.push("Additional file paths omitted.");
  } catch {
    lines.push("File list unavailable.");
  }
  return lines;
}

/** Build a snapshot for the person to review before attaching it to Gateway Chat. */
export async function readProjectReference(
  agent: Pick<AgentClient, "projectView">,
  core: Pick<CoreClient, "readConnector">,
  selected: ProjectIdentity,
  signal: AbortSignal,
): Promise<string> {
  const response = await agent.projectView(selected, signal);
  if (response.kind !== "project") throw new Error("This project could not be read. Refresh the project list and try again.");
  const { project, notes } = response.view;
  if (selected.id ? project.id !== selected.id : !!project.id || project.name !== selected.name) {
    throw new Error("This project changed. Select it again to refresh the reference.");
  }
  if (signal.aborted) throw new Error("The project read was cancelled.");

  const projectNotes = notes.filter((note) => note.audience === "dm" && noteInProject(note, project) && note.text.trim());
  const repos = [...new Set(project.repos.filter((repo) => GITHUB_REPO.test(repo)))];
  const lists = await Promise.all(repos.slice(0, MAX_REPOS).map((repo) => repoFileList(core, repo)));
  if (signal.aborted) throw new Error("The project read was cancelled.");
  const lines = [`# Project reference: ${project.name}`, "Read-only snapshot of project notes and repository file paths."];
  if (projectNotes.length) {
    lines.push("## Project notes");
    for (const note of projectNotes.slice(0, MAX_NOTES)) {
      const text = note.text.trim();
      const excerpt = text.length > MAX_NOTE_CHARS ? `${text.slice(0, MAX_NOTE_CHARS).trimEnd()}…` : text;
      lines.push(excerpt.split(/\r?\n/).map((line) => `> ${line}`).join("\n"));
    }
    if (projectNotes.length > MAX_NOTES) lines.push("Additional notes omitted.");
  }
  if (lists.length) {
    lines.push("## Repository file paths", ...lists.map((list) => list.join("\n")));
    if (repos.length > MAX_REPOS) lines.push("Additional repositories omitted.");
  }
  if (!projectNotes.length && !lists.length) lines.push("No project notes or linked repositories are available.");
  return `${lines.join("\n\n")}\n`;
}
