import type { AgentWriteSetting, ChatWriteSetting, Connection, ConnectorAvailability, ConnectorSummary } from "@nexus/kit";

// Integrations on the phone (brand v5's P11·C): each connector's line, who may use it, and what it waits on from you.

/**
 * Who may use a connector, as P11·C writes it: "On for all of Acme", "Only Neuro platform", "People connect their own"
 * or "Off". A member, who cannot read the organization's choices, sees whether it is on for them; nothing when that is
 * not known either.
 */
export function whoLine(
  availability: Pick<ConnectorAvailability, "availability" | "teams"> | undefined,
  usable: boolean | null | undefined,
  org: string,
  teamName: (slug: string) => string = (slug) => slug,
): string | undefined {
  if (availability) {
    if (availability.availability === "disabled") return "Off";
    if (availability.teams?.length) return `Only ${availability.teams.map(teamName).join(", ")}`;
    return availability.availability === "enabled_for_all" ? `On for all of ${org || "the organization"}` : "People connect their own";
  }
  return usable === true ? "On for you" : usable === false ? "Off for you" : undefined;
}

/**
 * What a connector waits on from you, on the right of its row: "Sign in again" when a connection it holds lost its
 * access, "Connect" when you may use it, connect your own, and have none; nothing otherwise.
 */
export function connectorAction(
  connector: Pick<ConnectorSummary, "name" | "usable" | "connects" | "oauth" | "form">,
  connections: Pick<Connection, "connector" | "status">[],
): "Sign in again" | "Connect" | undefined {
  const held = connections.filter((c) => c.connector === connector.name);
  if (held.some((c) => c.status === "needs_reconnect")) return "Sign in again";
  if (connector.usable === false || held.length) return undefined;
  return connector.connects?.includes("user") && (connector.oauth || connector.form) ? "Connect" : undefined;
}

/** What the phone says of a connector this Gateway does not offer, in Nexus UI's words. */
export const PREVIEW_NOTE = "This Gateway does not offer these yet. Their accounts are kept, and no call uses them.";

/**
 * The connectors this Gateway offers, and those in preview, which Nexus UI and desktop list under "In preview".
 * `offered` absent reads as offered: a Gateway older than the `offered` field offered every one.
 */
export function byOffer<C extends Pick<ConnectorSummary, "offered">>(connectors: C[]): { offered: C[]; preview: C[] } {
  return {
    offered: connectors.filter((c) => c.offered !== false),
    preview: connectors.filter((c) => c.offered === false),
  };
}

/** A connection's status, as the connector's page lists it. */
export const connectionStatus = (connection: Pick<Connection, "status" | "suspended">) =>
  connection.suspended ? "Kept, not used" : connection.status === "needs_reconnect" ? "Sign in again" : "Connected";

/** Assistant board A6 on the phone: the build turns it on with `EXPO_PUBLIC_NEXUS_INTEGRATIONS=on`. */
export const integrationsOn = (flag: string | undefined) => flag?.trim().toLowerCase() === "on";

/** What Nexus may do with an account, as Nexus UI says it. `unknown`: a write setting did not load. */
export type CanDo = "reads" | "writes-ask" | "writes" | "unknown";

/**
 * Reads, always, on an account you may use. Writes, only those the organization allows while its agent writes
 * are on; each asks first unless you turned on your own setting.
 */
export function canDo(connector: string, chat: ChatWriteSetting | undefined, mine: AgentWriteSetting | undefined): CanDo {
  if (!chat) return "unknown";
  if (!chat.enabled || !chat.operations.some((op) => op.connector === connector)) return "reads";
  if (!mine) return "unknown";
  return mine.enabled ? "writes" : "writes-ask";
}

/** `canDo` in words; `org` is "Personal" in your own account, where the writes are yours to allow. */
export function canDoLine(can: CanDo, org: string): string {
  const allows = org === "Personal" || !org ? "you allow" : `${org} allows`;
  switch (can) {
    case "reads":
      return "Reads only";
    case "writes-ask":
      return "Reads · writes after you approve";
    case "writes":
      return `Reads · makes the writes ${allows}`;
    case "unknown":
      return "Reads · could not check its writes";
  }
}

/** A connection Nexus cannot use until you act: it lost its access. A suspended one is kept on purpose. */
const needsYou = (connection: Pick<Connection, "status" | "suspended">) => connection.status === "needs_reconnect" && !connection.suspended;

/** The board's count line: "5 accounts · 1 needs you". */
export function accountsLine(connections: Pick<Connection, "status" | "suspended">[]): string {
  const needs = connections.filter(needsYou).length;
  const count = `${connections.length} ${connections.length === 1 ? "account" : "accounts"}`;
  return needs ? `${count} · ${needs} ${needs === 1 ? "needs" : "need"} you` : count;
}

/** One row per service you hold an account at, as the board draws it: one that needs you first, then by `label`. */
export function accountServices<C extends Pick<Connection, "connector" | "status" | "suspended">>(
  connections: C[],
  label: (connector: string) => string = (connector) => connector,
): { connector: string; count: number; value?: "Sign in again" | `${number} accounts` }[] {
  const names = [...new Set(connections.map((c) => c.connector))];
  return names
    .map((connector) => {
      const held = connections.filter((c) => c.connector === connector);
      const value = held.some(needsYou) ? ("Sign in again" as const) : held.length > 1 ? (`${held.length} accounts` as const) : undefined;
      return { connector, count: held.length, ...(value ? { value } : {}) };
    })
    .sort((a, b) => Number(b.value === "Sign in again") - Number(a.value === "Sign in again") || label(a.connector).localeCompare(label(b.connector)));
}
