/**
 * Memory as files (A16), for every app: the notes of `GET /agent/memory` grouped into Markdown files, and
 * the export. A file holds one kind of note (people, preferences, places, routines), and each errand has a file of
 * its own under `errands/`. A note with no kind, such as one made before kinds were kept, goes in `notes.md`. Each line
 * of a file is one line of a note, and it keeps that note's source. Nexus UI and the phone read it alike. The wire
 * types and their strict readers are in `agent-chat.ts`.
 *
 * The export (A16·c) is one ZIP: one Markdown file per memory file, each line with where it came from, and
 * `memory.json`, the notes as the agent API sends them. It is built from the view on screen, so nothing more is read
 * or sent.
 */
import { AGENT_NOTE_KINDS, agentProblem, agentWide, noteAudience, noteSourceText, type AgentMemoryNote, type AgentMemoryView, type AgentNoteKind, type AgentProblem } from "./agent-chat";

/** A list item's marker (`- `, `* `, `+ `, `• `, `1. `, `1) `) or a heading's `#`s, at the start of a line. */
const LINE_MARKER = /^\s*(?:[-*+•]|\d{1,3}[.)]|#{1,6})\s+/;

/**
 * A note as the lines a person reads. The agent writes its notes in Markdown, often as a list: each line loses its
 * list or heading marker, `**bold**` and backticks keep only their text, and blank lines go. The lines stay text: an
 * app draws them as plain text, never as Markdown or HTML.
 */
export function noteLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) =>
      line
        .replace(LINE_MARKER, "")
        .replace(/\*\*(.+?)\*\*/g, "$1")
        .replace(/`([^`]*)`/g, "$1")
        .trim(),
    )
    .filter(Boolean);
}

/** One line of a file: its text, the note it belongs to, and its place in that note. */
export interface MemoryLine {
  text: string;
  note: AgentMemoryNote;
  /** Its index among the note's lines (`noteLines`). */
  index: number;
}

/** One memory file. `errand`: set on an errand's own file. */
export interface MemoryFile {
  /** Unique among the files: `people.md`, or `errands/<slug>.md`. */
  path: string;
  /** The name the list shows: `people.md`, or `<slug>.md` for an errand. */
  name: string;
  /** The heading of the file: "People", or the errand's goal. */
  title: string;
  lines: MemoryLine[];
  /** When a note of the file was last made or edited (ISO 8601). */
  changedAt: string;
  errand?: { id: string; goal: string };
}

const TITLES: Record<AgentNoteKind | "notes", string> = { people: "People", preferences: "Preferences", places: "Places", routines: "Routines", notes: "Notes" };

/** A file name from an errand's goal: lowercase words joined by `-`, at most 40 characters. */
export function errandSlug(goal: string): string {
  const slug = goal
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/g, "");
  return slug || "errand";
}

/**
 * The files of `notes` (newest first, as the view sends them): the kinds in their order, then `notes.md`, then one
 * file per errand, the newest errand first. A file with no lines is left out. Lines keep the notes' order.
 */
export function memoryFiles(notes: AgentMemoryNote[]): MemoryFile[] {
  const byKind = new Map<string, MemoryFile>();
  const byErrand = new Map<string, MemoryFile>();
  const slugs = new Set<string>();
  for (const note of notes) {
    const lines = noteLines(note.text).map((text, index) => ({ text, note, index }));
    if (!lines.length) continue;
    const changedAt = note.editedAt && note.editedAt > note.createdAt ? note.editedAt : note.createdAt;
    const errand = note.source?.errand;
    let file: MemoryFile | undefined;
    if (errand) {
      file = byErrand.get(errand.id);
      if (!file) {
        let slug = errandSlug(errand.goal);
        for (let n = 2; slugs.has(slug); n += 1) slug = `${errandSlug(errand.goal)}-${n}`;
        slugs.add(slug);
        file = { path: `errands/${slug}.md`, name: `${slug}.md`, title: errand.goal, lines: [], changedAt, errand: { id: errand.id, goal: errand.goal } };
        byErrand.set(errand.id, file);
      }
    } else {
      const kind = note.kind ?? "notes";
      file = byKind.get(kind);
      if (!file) {
        file = { path: `${kind}.md`, name: `${kind}.md`, title: TITLES[kind], lines: [], changedAt };
        byKind.set(kind, file);
      }
    }
    file.lines.push(...lines);
    if (changedAt > file.changedAt) file.changedAt = changedAt;
  }
  const kinds = [...AGENT_NOTE_KINDS, "notes"].flatMap((kind) => byKind.get(kind) ?? []);
  return [...kinds, ...byErrand.values()];
}

/** How many facts a list of files holds: one per line. */
export function factCount(files: MemoryFile[]): number {
  return files.reduce((sum, file) => sum + file.lines.length, 0);
}

/** "1 fact", "14 facts". */
export function factsText(count: number): string {
  return `${count} ${count === 1 ? "fact" : "facts"}`;
}

/**
 * A note's text without one of its lines, for "Delete" on that line: the other lines as a Markdown list. Undefined
 * when the note has no other line, so the whole note is forgotten instead.
 */
export function withoutLine(note: AgentMemoryNote, index: number): string | undefined {
  const rest = noteLines(note.text).filter((_, i) => i !== index);
  if (!rest.length) return undefined;
  return rest.length === 1 ? rest[0]! : rest.map((line) => `- ${line}`).join("\n");
}

/**
 * Whose memory this is (A16, the page's header): Personal for a personal account, else the organization's, kept by
 * its Assistant. Undefined until the account is known.
 */
export function memoryScope(account: { kind?: string; name: string } | undefined): { name: string; kept: string } | undefined {
  if (!account) return undefined;
  if (account.kind === "personal") return { name: "Personal", kept: "Kept by your own Assistant. Only you see it." };
  const name = account.name.trim();
  if (!name) return undefined;
  return { name, kept: `Kept by ${name}’s Assistant. Your personal memory is kept apart.` };
}

/**
 * What an app says when a read or a forget fails. A sign-in that ended, a refused credential and an agent API that is
 * not available here read the same in every view (`agentWide`); the rest names the memory, not a question.
 */
export function memoryProblem(status: number, body?: { error?: unknown; detail?: unknown }): AgentProblem {
  const detail = typeof body?.detail === "string" ? body.detail : "";
  if (agentWide(status, body)) return agentProblem(status, body, "open Memory again");
  if (status === 503) return { title: "Memory is not available now", detail: detail || "Try again in a moment." };
  return { title: "Memory did not open", detail: detail || "Nexus could not reach the Assistant. Try again in a moment." };
}

/** What an app says when an add, an edit or the learning switch fails: the agent API's own words when it sent some. */
export function memoryChangeProblem(status: number, body?: { error?: unknown; detail?: unknown }): AgentProblem {
  if (agentWide(status, body)) return agentProblem(status, body, "make the change again");
  const detail = typeof body?.detail === "string" && body.detail ? body.detail : "Nexus could not reach the Assistant. Try again in a moment.";
  return { title: "Memory did not change", detail };
}

/** The export's file name. */
export const EXPORT_FILE = "nexus-memory.zip";

/** Where a line came from, as the export writes it: its audience, its source and its dates. */
export function lineSource(note: AgentMemoryNote): string {
  return [noteAudience(note).label, noteSourceText(note), note.createdAt.slice(0, 10), note.editedAt ? `edited ${note.editedAt.slice(0, 10)}` : undefined].filter(Boolean).join(" · ");
}

/** One memory file as Markdown: its heading, then one list item per line with its source. The text is not escaped. */
export function fileMarkdown(file: MemoryFile): string {
  return [`# ${file.title}`, "", ...file.lines.map((line) => `- ${line.text} (${lineSource(line.note)})`), ""].join("\n");
}

/** The export's Markdown files, by their path in the ZIP. */
export function memoryMarkdownFiles(view: AgentMemoryView): { name: string; text: string }[] {
  const files = memoryFiles(view.notes);
  if (!files.length) return [{ name: "notes.md", text: "# Notes\n\nNexus keeps no notes about you.\n" }];
  return files.map((file) => ({ name: file.path, text: fileMarkdown(file) }));
}

/** The notes as JSON, as the agent API sends them, with the organization and the time of the export. */
export function memoryJson(view: AgentMemoryView, org: string, at: Date): string {
  return `${JSON.stringify({ organization: org, exportedAt: at.toISOString(), ...view }, null, 2)}\n`;
}

/** The export: a ZIP of one Markdown file per memory file, and `memory.json`. */
export function memoryZip(view: AgentMemoryView, org: string, at = new Date()): Uint8Array {
  return storedZip([...memoryMarkdownFiles(view), { name: "memory.json", text: memoryJson(view, org, at) }], at);
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff]! ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

/** A ZIP with each file stored, not compressed: a few kilobytes of text need no more, and every unzip reads it. */
export function storedZip(files: { name: string; text: string }[], at: Date): Uint8Array {
  const encoder = new TextEncoder();
  const time = (at.getUTCHours() << 11) | (at.getUTCMinutes() << 5) | Math.floor(at.getUTCSeconds() / 2);
  const date = ((at.getUTCFullYear() - 1980) << 9) | ((at.getUTCMonth() + 1) << 5) | at.getUTCDate();
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const file of files) {
    const name = encoder.encode(file.name);
    const data = encoder.encode(file.text);
    const crc = crc32(data);
    // Version 2.0, flag 0x0800 (UTF-8 names), method 0 (stored).
    const common = [20, 0x0800, 0, time, date] as const;
    const local = new DataView(new ArrayBuffer(30 + name.length + data.length));
    local.setUint32(0, 0x04034b50, true);
    common.forEach((value, i) => local.setUint16(4 + i * 2, value, true));
    local.setUint32(14, crc, true);
    local.setUint32(18, data.length, true);
    local.setUint32(22, data.length, true);
    local.setUint16(26, name.length, true);
    new Uint8Array(local.buffer).set(name, 30);
    new Uint8Array(local.buffer).set(data, 30 + name.length);
    const central = new DataView(new ArrayBuffer(46 + name.length));
    central.setUint32(0, 0x02014b50, true);
    central.setUint16(4, 20, true);
    common.forEach((value, i) => central.setUint16(6 + i * 2, value, true));
    central.setUint32(16, crc, true);
    central.setUint32(20, data.length, true);
    central.setUint32(24, data.length, true);
    central.setUint16(28, name.length, true);
    central.setUint32(42, offset, true);
    new Uint8Array(central.buffer).set(name, 46);
    locals.push(new Uint8Array(local.buffer));
    centrals.push(new Uint8Array(central.buffer));
    offset += local.byteLength;
  }
  const size = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, files.length, true);
  end.setUint16(10, files.length, true);
  end.setUint32(12, size, true);
  end.setUint32(16, offset, true);
  const out = new Uint8Array(offset + size + 22);
  let cursor = 0;
  for (const part of [...locals, ...centrals, new Uint8Array(end.buffer)]) {
    out.set(part, cursor);
    cursor += part.length;
  }
  return out;
}
