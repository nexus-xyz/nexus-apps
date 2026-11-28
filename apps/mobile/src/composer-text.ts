/**
 * What the composer works out from its text (brand v5's P65 and P98): how many lines it holds, the editor's count, a
 * long paste turned into a file, and the `/` being typed. No React Native here, so the repository's vitest runs it.
 */

/** The composer's line height (P65·H: 16/22 in the box). */
export const LINE = 22;
/** The composer grows a line at a time to five, then scrolls. */
export const MAX_LINES = 5;
/** From this many lines, ⤢ opens the editor. */
export const EXPAND_FROM = 3;
/** A paste longer than this becomes a file, with Paste as text instead (P65·A). */
export const LONG_PASTE = 2000;
/** From this many tokens the count is an estimate, not words (P65·H). */
const ESTIMATE_FROM = 1000;

/** The lines a multiline field's content holds, from its height (without its padding). */
export const linesOf = (contentHeight: number) => Math.max(1, Math.round(contentHeight / LINE));

/** About how many tokens a text costs: four characters each, as the providers' rule of thumb. */
export const tokensOf = (text: string, extraBytes = 0) => Math.ceil((text.length + extraBytes) / 4);

/**
 * Under the editor's title (P65·H): words while short, then "~2.1k tokens". With a file attached the estimate includes
 * it, since a file is what makes a draft long.
 */
export function draftCount(text: string, attachedBytes = 0): string {
  const tokens = tokensOf(text, attachedBytes);
  if (tokens >= ESTIMATE_FROM) return `~${(tokens / 1000).toFixed(1).replace(/\.0$/, "")}k tokens`;
  const words = text.trim() ? text.trim().split(/\s+/).length : 0;
  return `${words.toLocaleString("en-US")} ${words === 1 ? "word" : "words"}`;
}

/**
 * What one change of the field inserted and where, when it inserted one run of text and removed nothing (a paste);
 * else null.
 */
export function insertedRun(before: string, after: string): { start: number; text: string } | null {
  if (after.length <= before.length) return null;
  let start = 0;
  while (start < before.length && before[start] === after[start]) start++;
  let end = 0;
  while (end < before.length - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  // Everything outside the run must be the text as it was.
  if (start + end !== before.length) return null;
  return { start, text: after.slice(start, after.length - end) };
}

/** A long paste as the file it becomes: Markdown, named as P65 shows it. */
export function pastedFile(text: string): { name: string; data: string; bytes: number } {
  const bytes = new TextEncoder().encode(text);
  return { name: "pasted-text.md", data: `data:text/markdown;base64,${base64(bytes)}`, bytes: bytes.length };
}

/** Bytes as standard base64, padded, as a data URL takes them (Hermes may lack `btoa` for bytes). */
export function base64(bytes: Uint8Array): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const [a, b = 0, c = 0] = [bytes[i]!, bytes[i + 1], bytes[i + 2]];
    const n = (a << 16) | (b << 8) | c;
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]!;
    out += i + 1 < bytes.length ? alphabet[(n >> 6) & 63]! : "=";
    out += i + 2 < bytes.length ? alphabet[n & 63]! : "=";
  }
  return out;
}

/** The `/` word the caret is in (P98): what follows the slash, when the word starts the text or follows a space. */
export function slashQuery(text: string, caret: number): string | null {
  const before = text.slice(0, caret);
  const match = /(?:^|\s)\/([\w-]*)$/.exec(before);
  return match ? match[1]! : null;
}

/** The `/` word at the caret replaced by `/name ` (a skill picked), and where the caret goes after it. */
export function insertSlash(text: string, caret: number, name: string): { text: string; caret: number } {
  const query = slashQuery(text, caret) ?? "";
  const start = caret - query.length - 1;
  const inserted = `/${name} `;
  const after = text.slice(caret).replace(/^\S*\s?/, "");
  return { text: text.slice(0, start) + inserted + after, caret: start + inserted.length };
}

/** The entries whose name starts with the query, then those that have it elsewhere, in their order. */
export function matchSlash<T extends { name: string }>(entries: T[], query: string): T[] {
  const q = query.toLowerCase();
  const starts = entries.filter((e) => e.name.toLowerCase().startsWith(q));
  return [...starts, ...entries.filter((e) => !starts.includes(e) && e.name.toLowerCase().includes(q))];
}

/** An @ word at the caret, except inside email addresses or other words. */
export function referenceToken(text: string, caret: number): { start: number; end: number; query: string } | undefined {
  if (caret < 0 || caret > text.length) return;
  const before = text.slice(0, caret);
  const match = /(?:^|\s)@([^\s@]*)$/.exec(before);
  if (!match || match[1] === undefined) return;
  const query = match[1];
  const rest = /^\S*/.exec(text.slice(caret))?.[0] ?? "";
  return { start: caret - query.length - 1, end: caret + rest.length, query };
}
