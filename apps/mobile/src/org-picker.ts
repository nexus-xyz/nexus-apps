/**
 * The organization switcher's lists (`org-row.tsx`, `org-sheet.tsx`), built to stay usable with many organizations.
 * The sheet lists the few you were in last first, then every organization in a fixed order (Personal, then A–Z), so one
 * never moves; a search narrows them once there are enough to need one. No React Native, so vitest runs it.
 */
import type { OrgSummary } from "@nexus/kit";
import { orderOrgs, orgLabel } from "./orgs";

/** From this many organizations on, the sheet shows its search and its Recent list. */
export const SEARCH_FROM = 7;

/** How many organizations the app remembers you were in, newest first. */
const KEEP = 8;

/** How many of them the sheet's Recent list shows. */
const RECENT_SHOWN = 3;

export type PickerSection = { key: "recent" | "all" | "results"; title?: string; orgs: OrgSummary[] };

const fold = (value: string) => value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();

/** `recent` with `id` first: the organizations you were in, newest first, each once. */
export function rememberOrg(recent: readonly string[], id: string): string[] {
  return [id, ...recent.filter((other) => other !== id)].slice(0, KEEP);
}

/** The organizations you were in last, newest first, without the one you are in; ids no longer yours are dropped. */
function recentOthers(orgs: readonly OrgSummary[], recent: readonly string[]): OrgSummary[] {
  const byId = new Map(orgs.map((org) => [org.id, org]));
  return recent.flatMap((id) => {
    const org = byId.get(id);
    return org && !org.current ? [org] : [];
  });
}

/**
 * The sheet's sections for `query`. Few organizations: all of them, Personal first, then A–Z. Many: the ones you were
 * in last, then all of them. While searching: the matches alone, by name or slug, ignoring case and accents.
 */
export function pickerSections(orgs: readonly OrgSummary[], query = "", recent: readonly string[] = []): PickerSection[] {
  const ordered = orderOrgs([...orgs]);
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length) {
    const matches = ordered.filter((org) => {
      const haystack = fold([orgLabel(org), org.name, org.slug ?? ""].join(" "));
      return words.every((word) => haystack.includes(word));
    });
    return [{ key: "results", title: matches.length === 1 ? "1 organization" : `${matches.length} organizations`, orgs: matches }];
  }
  if (orgs.length < SEARCH_FROM) return [{ key: "all", orgs: ordered }];
  const last = recentOthers(orgs, recent).slice(0, RECENT_SHOWN);
  return [
    ...(last.length ? [{ key: "recent" as const, title: "Recent", orgs: last }] : []),
    { key: "all", title: `All ${orgs.length} organizations`, orgs: ordered },
  ];
}

/** Your role in an organization: Owner, Admin, Member, or "Just you" in Personal. */
export function roleLine(org: Pick<OrgSummary, "kind" | "role"> | undefined): string {
  if (!org) return "";
  if (org.kind === "personal") return "Just you";
  return org.role ? org.role.charAt(0).toUpperCase() + org.role.slice(1) : "";
}
