import type { CurrentTenantResponse } from "@nexus/kit";

// Copies of Nexus UI's team helpers (`shared.ts`'s `flattenTeams`, `teams/page.tsx`'s slug rule).

/** The tree as one list, each team before the teams under it. */
export function flattenTeams<T extends { children: T[] }>(nodes: T[]): T[] {
  return nodes.flatMap((node) => [node, ...flattenTeams(node.children)]);
}

/** A team's slug: lower-case letters, digits and dashes, starting with a letter or digit (Gateway's rule). */
export const SLUG = /^[a-z0-9][a-z0-9-]{0,126}$/;

/** The slug a name suggests: `Platform Core` → `platform-core`. */
export const suggestSlug = (name: string) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 127);

/** What the composer's team picker reads from `me()`: the teams you are in, and your default. */
export type MyTeams = Pick<CurrentTenantResponse, "teams" | "team">;

/**
 * The composer's team picker, as Nexus UI's `TeamPicker`: the teams you are in, your default marked. It is on the
 * chat's team or the one you picked, else on your default, where Gateway sends the spend when the app names none.
 * Null when you are in no team: there is nothing to pick.
 */
export function teamPicker(mine: MyTeams, picked?: string) {
  const teams = mine.teams ?? [];
  if (!teams.length) return null;
  const selected = picked ?? mine.team?.slug ?? "";
  return {
    options: teams.map((t) => ({ value: t.slug, label: t.name, detail: t.slug === mine.team?.slug ? "Your default" : undefined })),
    selected,
    /** A team you have left keeps its slug. */
    name: teams.find((t) => t.slug === selected)?.name ?? (selected || "Team"),
  };
}
