import { connectorName, type ChatSessionMessage, type ChatWebSource } from "@nexus/kit";

type WebSource = ChatWebSource & { host: string };

/**
 * What an answer did besides write, as brand v5's P38 shows it on the phone: one chip per kind of call, in the order
 * the answer ran them, each a tap to its detail. Skills it loaded, the connectors it read (one chip per connector, with
 * how many reads), its web sources, then the page it wrote.
 */
export type Chip =
  | { key: string; kind: "skill"; label: string; skill: NonNullable<ChatSessionMessage["skills"]>[number] }
  | { key: string; kind: "connector"; label: string; connector: string; reads: NonNullable<ChatSessionMessage["connectors"]> }
  | { key: string; kind: "web"; label: string; sources: WebSource[] }
  | { key: string; kind: "page"; label: string; page: NonNullable<ChatSessionMessage["page"]> };

/** A connector's name as a person reads it, as Nexus UI says it: `google-calendar` is Google Calendar. */
export const connectorLabel = connectorName;

const times = (n: number) => (n === 1 ? "" : n === 2 ? " twice" : ` ${n} times`);

export function activityChips(message: Partial<Pick<ChatSessionMessage, "skills" | "connectors" | "sources" | "page">>): Chip[] {
  const chips: Chip[] = (message.skills ?? []).map((skill, n) => ({ key: `skill-${n}`, kind: "skill", label: `Loaded ${skill.name}`, skill }));
  const byConnector = new Map<string, NonNullable<ChatSessionMessage["connectors"]>>();
  for (const read of message.connectors ?? []) byConnector.set(read.connector, [...(byConnector.get(read.connector) ?? []), read]);
  for (const [connector, reads] of byConnector) {
    chips.push({ key: `connector-${connector}`, kind: "connector", label: `Read ${connectorLabel(connector)}${times(reads.length)}`, connector, reads });
  }
  const sources = (message.sources ?? []).flatMap(webSource);
  if (sources.length) chips.push({ key: "web", kind: "web", label: `${sources.length} web ${sources.length === 1 ? "source" : "sources"}`, sources });
  if (message.page) chips.push({ key: "page", kind: "page", label: `Wrote ${message.page.title || "a page"}`, page: message.page });
  return chips;
}

function webSource(source: ChatWebSource): WebSource[] {
  try {
    const url = new URL(source.url);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) return [];
    return [{ ...source, url: url.href, host: url.hostname.replace(/^www\./, "") }];
  } catch {
    return [];
  }
}
