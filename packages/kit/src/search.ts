/**
 * `GET /v1/search` (Rust gateway only): Nexus UI's ⌘K palette and `nexus search`. One query across what the caller's
 * role may read, grouped by type and ranked. Match ranges are `[start, end)` in UTF-16 code units, so `text.slice`
 * highlights them as they are.
 */

export type SearchType = "chats" | "artifacts" | "people" | "teams" | "models";

export interface SearchQuery {
  q: string;
  /** Comma-separated types (default: all the caller may read); `actions` and `pages` are accepted and ignored. */
  types?: string;
  /** Results per group, 1 to 20 (default 5). */
  limit?: number;
}

export interface SearchSnippet {
  text: string;
  matches: [number, number][];
}

export interface SearchResult {
  type: SearchType;
  /** What it is within its type. */
  entity: "session" | "artifact" | "member" | "invitation" | "team" | "model" | "policy";
  id: string;
  title: string;
  titleMatches: [number, number][];
  subtitle?: string;
  /** Where it matched when not in the title: a chat message, an email, a team's path. */
  snippet?: SearchSnippet;
  /** Nexus UI's page: `/app/<org>/sessions/<id>`; a chat message match ends in `#m-<message id>`. */
  url: string;
  /** 0 to 1. */
  score: number;
  /** Chats: the message that matched. */
  messageId?: string;
}

export interface SearchGroup {
  type: SearchType;
  label: string;
  results: SearchResult[];
  /** More matched than `limit`. */
  more: boolean;
}

export interface SearchResponse {
  query: string;
  /** The types searched: those asked for that the caller's role may read. */
  types: SearchType[];
  /** Groups with results, best first. */
  groups: SearchGroup[];
  /** The server's time for the search. */
  tookMs: number;
}

/** A match in a result's text: `[start, end)`, as `/v1/search` answers them. */
export type SearchRange = [number, number];

/** Ranges sorted, with the overlapping and touching ones joined. */
export function mergeRanges(ranges: SearchRange[]): SearchRange[] {
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  const out: SearchRange[] = [];
  for (const [start, end] of sorted) {
    const last = out[out.length - 1];
    if (last && start <= last[1]) last[1] = Math.max(last[1], end);
    else out.push([start, end]);
  }
  return out;
}

/** `text` cut into plain and matched parts, for highlighting (Nexus UI's search palette and the phone app's Search). */
export function segments(text: string, ranges: SearchRange[] = []): { text: string; match: boolean }[] {
  const out: { text: string; match: boolean }[] = [];
  let at = 0;
  for (const [start, end] of mergeRanges(ranges.filter(([start, end]) => start >= 0 && end > start))) {
    if (start < at) continue;
    if (start > at) out.push({ text: text.slice(at, start), match: false });
    out.push({ text: text.slice(start, Math.min(end, text.length)), match: true });
    at = Math.min(end, text.length);
  }
  if (at < text.length) out.push({ text: text.slice(at), match: false });
  return out.filter((part) => part.text);
}
