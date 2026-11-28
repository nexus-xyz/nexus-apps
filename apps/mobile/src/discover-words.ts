import { connectorName, unlocksOf, type Connection, type ConnectorSummary, type DiscoverNewsAction } from "@nexus/kit";
import { askNexusHref } from "./assistant-handoff";

/**
 * Discover on the phone (A15), without React Native: what Nexus can do, and how to let it do more. The
 * ideas, the news, what each connector unlocks and the autonomy ladder are the kit's (`kit/src/agent-discover.ts`),
 * which Nexus UI reads too. This module keeps what the phone adds: which connectors Connect more lists, and where a
 * line of What's new goes on the phone.
 */

/** The most connectors Connect more lists, as on the web. */
export const CONNECT_MORE_MAX = 6;

/** One row of Connect more. */
export interface ConnectRow {
  connector: string;
  name: string;
  /** "Unlocks: …", or what it already does once connected. */
  line: string;
  connected: boolean;
}

/** A catalog description's first sentence, for a connector the kit names no unlock for. */
const firstSentence = (text: string) => (text.trim().match(/^[^.!?]*[.!?]?/)?.[0] ?? text).replace(/[.!?]$/, "").trim();

/**
 * Connect more: the connectors this Gateway offers that the person may connect as themselves, the unconnected first,
 * in the catalog's order. A connector off for them is left out.
 */
export function connectRows(catalog: ConnectorSummary[], mine: Pick<Connection, "connector" | "scope">[], max = CONNECT_MORE_MAX): ConnectRow[] {
  const rows = catalog
    .filter((connector) => connector.offered !== false && connector.usable !== false && (connector.connects ?? []).includes("user"))
    .map((connector): ConnectRow => {
      const unlocks = unlocksOf(connector.name, firstSentence(connector.description));
      const connected = mine.some((held) => held.connector === connector.name && held.scope === "user");
      return {
        connector: connector.name,
        name: connectorName(connector.name),
        line: connected ? `Already connected: ${unlocks.charAt(0).toLowerCase()}${unlocks.slice(1)}` : `Unlocks: ${unlocks}`,
        connected,
      };
    });
  return [...rows.filter((row) => !row.connected), ...rows.filter((row) => row.connected)].slice(0, max);
}

/** Where a line of What's new goes on the phone: a page of the app, Nexus UI's page, or a line sent to Nexus. */
export type NewsTarget = { href: string | ReturnType<typeof askNexusHref> } | { url: string } | undefined;

export function newsTarget(action: DiscoverNewsAction, uiLink?: (path: string) => string): NewsTarget {
  if (action.kind === "ask") return { href: askNexusHref(action.line) };
  if (action.view === "chat") return { href: "/agent" };
  if (action.view === "memory") return { href: "/assistant-memory" };
  if (action.view === "errands") return { href: "/errands" };
  return uiLink ? { url: uiLink("/agent?view=approvals") } : undefined;
}

/** The What's new ids this phone keeps as seen: strings only, from what the store held. */
export const seenIds = (stored: unknown): string[] => (Array.isArray(stored) ? stored.filter((id): id is string => typeof id === "string").slice(0, 200) : []);

/** What Teach it about you says of the memory: "Nexus keeps 38 notes about you." */
export const keepsLine = (assistant: string, notes: number) => `${assistant} keeps ${notes} ${notes === 1 ? "note" : "notes"} about you.`;

/** The kinds of thing to tell Nexus, with an example of each (A15·d), as the web lists them. */
export const TEACH = [
  { title: "People", line: "Ana is my manager at Acme." },
  { title: "Preferences", line: "Aisle seats. No shellfish." },
  { title: "Places", line: "My pharmacy is the Walgreens on Divisadero." },
  { title: "Routines", line: "Gym on Tuesday and Thursday at 7." },
] as const;
