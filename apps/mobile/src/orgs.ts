import type { OrgSummary } from "@nexus/kit";

/** An organization as Nexus UI names it: your personal one is "Personal". */
export const orgLabel = (org: Pick<OrgSummary, "kind" | "name">) => (org.kind === "personal" ? "Personal" : org.name);

/** What `POST /v1/orgs/switch` takes for it: `personal`, else its slug or id. */
export const orgSwitchKey = (org: Pick<OrgSummary, "kind" | "slug" | "id">) => (org.kind === "personal" ? "personal" : org.slug || org.id);

/** Personal first, then by name, as Nexus UI's switcher lists them. */
export const orderOrgs = <T extends Pick<OrgSummary, "kind" | "name">>(orgs: T[]) =>
  [...orgs].sort((a, b) => Number(b.kind === "personal") - Number(a.kind === "personal") || a.name.localeCompare(b.name));
