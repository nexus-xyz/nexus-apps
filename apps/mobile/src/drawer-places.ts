import type { MobilePlace, Role } from "@nexus/kit";

/**
 * The drawer's destinations. More holds Archived, Inbox, and help.
 *
 * The Assistant shows only where its screen works: in a build with the agent API (`useAuth().agent`, the screen's own
 * gate). Desktop shows it only with an agent API, and Nexus UI only when the agent is ready. Its icon is the Nexus
 * mark, not a symbol (`NavIcon`'s "mark").
 */

const ASSISTANT = { id: "assistant", label: "Assistant", icon: "mark", href: "/agent" } as const;
const ARTIFACTS = { id: "artifacts", label: "Artifacts", icon: "artifacts", href: "/pages" } as const;
const CUSTOMIZE = { id: "customize", label: "Customize", icon: "connector", href: { pathname: "/integrations", params: { customize: "1" } } } as const;
const MORE = { id: "more", label: "More", icon: "more", href: "/settings-group/more" } as const;

export function mobilePlaceVisibility(id: MobilePlace | "more", hidden: readonly MobilePlace[], role?: Role) {
  if (id === "more" || !hidden.includes(id)) return "visible";
  return role === "owner" ? "owner" : "hidden";
}

export type DrawerPlace = typeof ASSISTANT | typeof ARTIFACTS | typeof CUSTOMIZE | typeof MORE;

/**
 * Whether a link opens a page of the drawer itself, beside Chat (a session, the Assistant), rather than a page in the
 * sheet: the sidebar then closes and the page takes the chat's place. Search's Ask Nexus is the Assistant with words.
 * An errand's thread is a page of the drawer too (A12).
 */
export function opensInDrawer(href: unknown): boolean {
  if (href === ASSISTANT.href) return true;
  const pathname = typeof href === "object" && href !== null ? (href as { pathname?: unknown }).pathname : undefined;
  return pathname === ASSISTANT.href || pathname === "/session/[id]" || pathname === "/errand/[id]";
}

/** An errand's thread, as a link of the drawer. */
export const errandHref = (id: string) => ({ pathname: "/errand/[id]" as const, params: { id } });

/** The drawer's first row, above New session. `agent`: the sign-in's agent client, or null when this build has no agent API. */
export function assistantPlace(agent: unknown) {
  return agent ? ASSISTANT : null;
}

/** Pages under the drawer's New and Search actions, in order. */
export function drawerPlaces(): readonly DrawerPlace[] {
  return [ARTIFACTS, CUSTOMIZE, MORE];
}
