import { factCount, factsText, memoryFiles, memoryMarkdownFiles, noteAudience, noteLines, noteSourceText, withoutLine, type AgentMemoryView, type MemoryFile, type MemoryLine } from "@nexus/kit";
import { shortDay } from "./people-controls-words";

/**
 * The Assistant's Memory on the phone (A16), without React Native: the words of its two pages. The list
 * shows "Personal · 40 facts · learning on" and one row per file. A file's page shows each fact with where it came
 * from and when. The files, the export and the problems' words are the kit's (`kit/src/agent-memory.ts`), which Nexus
 * UI reads too.
 */

/** The list's head line: whose memory, how many facts, and whether learning is on. */
export function memoryHead(view: AgentMemoryView, scope: string | undefined): string {
  const facts = factsText(factCount(memoryFiles(view.notes)));
  const learning = view.learning ? (view.learning.paused ? "learning paused" : "learning on") : undefined;
  return [scope, facts, learning].filter(Boolean).join(" · ");
}

/** The file page's address: `/memory-file?path=people.md`. */
export const memoryFileHref = (path: string) => ({ pathname: "/memory-file" as const, params: { path } });

/** The open file, from its path; undefined once it is gone (its last line was deleted). */
export const fileAt = (view: AgentMemoryView, path: string | undefined): MemoryFile | undefined => memoryFiles(view.notes).find((file) => file.path === path);

/** Under a fact: where it came from and its day, "Contacts · 1 Oct". A note older than sources names its audience. */
export function lineSource(line: MemoryLine): string {
  return `${noteSourceText(line.note) ?? noteAudience(line.note).label} · ${shortDay(line.note.createdAt)}`;
}

/** A line's key in its file: its note and its place in that note. */
export const lineKey = (line: MemoryLine) => `${line.note.id}:${line.index}`;

/**
 * What Delete on a line does, and its confirm. A note with one line is forgotten. With the controls on, a note keeps
 * its other lines (`edit`). Without them, the whole note goes, and the confirm says so.
 */
export function lineRemoval(line: MemoryLine, controls: boolean, assistant: string): { title: string; body: string; edit?: string } {
  const rest = withoutLine(line.note, line.index);
  if (rest === undefined) return { title: "Delete this line?", body: `${assistant} forgets it on every surface.` };
  if (controls) return { title: "Delete this line?", body: `${assistant} keeps the other lines of its note.`, edit: rest };
  const count = noteLines(line.note.text).length;
  return { title: "Delete its note?", body: `The line is one of ${count} in a note. ${assistant} forgets the whole note, on every surface.` };
}

/** "Show where" (A16·a): where the line came from, when, and where Nexus uses it. */
export function learnedFrom(line: MemoryLine, assistant: string): string {
  const { note } = line;
  const source = noteSourceText(note) ?? `${assistant} did not keep where this came from. The note is older than sources`;
  const edited = note.editedAt ? ` · edited ${shortDay(note.editedAt)}` : "";
  const audience = noteAudience(note);
  return `${source} · ${shortDay(note.createdAt)}${edited}.\n\n${audience.label}. ${audience.hint}`;
}

/** The text a fact's editor starts with: the note's lines, one per line. */
export const editDraft = (line: MemoryLine) => noteLines(line.note.text).join("\n") || line.note.text;

/** What Forget all says before it runs. */
export const FORGET_ALL = { title: "Forget every note?", body: (assistant: string) => `${assistant} forgets every file, on every surface. This cannot be undone.` };

/** The export as text, for a share sheet that takes no file (Android): each Markdown file under its path. */
export function exportText(view: AgentMemoryView): string {
  return memoryMarkdownFiles(view)
    .map((file) => `<!-- ${file.name} -->\n${file.text}`)
    .join("\n");
}

/** The status after a change, as the web's view says it. */
export function forgotText(assistant: string, target: "note" | "all", count: number): string {
  return target === "note" ? `${assistant} forgot the note.` : `${assistant} forgot ${count} ${count === 1 ? "note" : "notes"}.`;
}
