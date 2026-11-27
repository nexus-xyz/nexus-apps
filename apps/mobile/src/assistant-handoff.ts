import { AGENT_MAX_CHARS } from "@nexus/kit";

/**
 * Hand off from Search (A21·B e): with words typed and the Assistant in this build, Search's first row asks Nexus
 * them. The row opens the Assistant with the words in its `ask` parameter, and the screen sends them once.
 */

/** The row's words: Ask Nexus “dinner Friday”. Undefined with nothing typed. */
export function askNexusLabel(query: string): string | undefined {
  const words = query.trim();
  return words ? `Ask Nexus “${words}”` : undefined;
}

/** Where the row goes: the Assistant, with the words to send, cut to what the agent API takes. */
export const askNexusHref = (query: string) => ({ pathname: "/agent" as const, params: { ask: query.trim().slice(0, AGENT_MAX_CHARS) } });

/** The words handed to the Assistant, from its `ask` parameter. Expo Router gives a list for a repeated parameter. */
export function handedWords(ask: string | string[] | undefined): string | undefined {
  const words = (Array.isArray(ask) ? ask[0] : ask)?.trim();
  return words ? words.slice(0, AGENT_MAX_CHARS) : undefined;
}
