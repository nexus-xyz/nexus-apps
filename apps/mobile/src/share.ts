// `sharingSettings`, `parseEmails` and `expiryChoices` copy Nexus UI's (`chat/external-sharing.tsx`,
// `artifacts.ts`), which sit beside Next-only imports; the audience rules are its share dialog's (`chat/share-dialog.tsx`,
// `pages/page-share.tsx`).
import {
  ARTIFACT_SETTING_DEFAULTS,
  externalShareAllowed,
  type Artifact,
  type ArtifactAccess,
  type ArtifactAccessPerson,
  type ArtifactAudience,
  type ArtifactAudienceKind,
  type ArtifactSharingSettings,
  type ArtifactsClient,
  type ShareChatSessionRequest,
} from "@nexus/kit";

/** A link the Gateway gives, absolute: it answers a path (`/app/<org>/…`) when it does not know where Nexus UI is. */
export const absoluteLink = (urlOrPath: string, ui: string) => (/^https?:\/\//i.test(urlOrPath) ? urlOrPath : `${ui.replace(/\/$/, "")}${urlOrPath.startsWith("/") ? "" : "/"}${urlOrPath}`);

/** Who an artifact is shared with, as Nexus UI's share menu says it. */
export function audienceLabel(audience: Pick<ArtifactAudience, "kind" | "teams" | "people">): string {
  switch (audience.kind) {
    case "org":
      return "Everyone in the organization";
    case "public":
      return "Anyone with the link";
    case "teams":
      return audience.teams.length === 1 ? `Team ${audience.teams[0]}` : `${audience.teams.length} teams`;
    case "people":
      return audience.people.length === 1 ? "1 person" : `${audience.people.length} people`;
    case "external":
      return audience.people.length === 1 ? "1 person outside" : `${audience.people.length} people outside`;
  }
}

/** What the share sheet shares with a chosen audience: a chat of yours (its session) or a page of yours. */
export type Shareable = { kind: "chat" | "page"; id: string };

/** The organization's sharing settings as Gateway answers them, with the defaults for any it leaves out. */
export function sharingSettings(answer: Partial<ArtifactSharingSettings> | undefined): ArtifactSharingSettings {
  const given = Object.entries(answer ?? {}).filter(([key, value]) => key.startsWith("artifacts.") && value !== undefined && value !== null);
  return { ...ARTIFACT_SETTING_DEFAULTS, ...Object.fromEntries(given) } as ArtifactSharingSettings;
}

/** An audience as the sheet lists it; `outside` ones go under their own heading, after the organization's. */
export type AudienceChoice = { kind: ArtifactAudienceKind; label: string; detail: string; outside: boolean; refusal?: string };

/**
 * The audiences the sheet lists, in Nexus UI's order: inside the organization, then outside it. An audience the
 * organization's settings forbid stays listed with the reason (`refusal`), as Nexus UI greys it out. A personal
 * organization has no "Everyone in it": that is only you.
 */
export function audienceChoices({ policy, role, personal, page, orgName }: { policy: ArtifactSharingSettings; role?: string; personal?: boolean; page?: boolean; orgName?: string }): AudienceChoice[] {
  const all: AudienceChoice[] = [
    { kind: "org", label: `Everyone in ${orgName ?? "the organization"}`, detail: "Members sign in to open it.", outside: false },
    { kind: "teams", label: "Teams", detail: "Choose teams. Sub-teams get it too.", outside: false },
    { kind: "people", label: "People", detail: "Choose members of your organization.", outside: false },
    { kind: "external", label: "People outside", detail: "Named people, by email.", outside: true },
    { kind: "public", label: "Anyone with the link", detail: "No sign-in. Search engines do not list it.", outside: true },
  ];
  return all
    .filter((choice) => !(personal && choice.kind === "org"))
    .map((choice) => {
      if (page && choice.kind === "public" && !policy["artifacts.pages.public"]) return { ...choice, refusal: "Your organization has turned off public links for pages." };
      const verdict = externalShareAllowed(choice.kind, policy, role);
      return verdict.allowed ? choice : { ...choice, refusal: verdict.reason };
    });
}

/**
 * Teams as a tree reads: each before the teams under it, so an indented team sits under its own parent. A team the
 * walk from the top never reaches (a cycle, which Gateway does not allow) still comes last rather than being lost.
 */
export function inTreeOrder<T extends { slug: string; parent?: string }>(teams: T[]): T[] {
  const slugs = new Set(teams.map((t) => t.slug));
  const under = (parent: string | undefined): T[] =>
    teams.filter((t) => (t.parent && slugs.has(t.parent) ? t.parent : undefined) === parent).flatMap((t) => [t, ...under(t.slug)]);
  const tree = under(undefined);
  return [...tree, ...teams.filter((t) => !tree.includes(t))];
}

/** Emails typed into the "People outside" box: split on commas, spaces and new lines; lower-cased, unique. */
export function parseEmails(text: string): { emails: string[]; invalid: string[] } {
  const parts = text
    .split(/[\s,;]+/)
    .map((part) => part.trim().toLowerCase())
    .filter(Boolean);
  const emails: string[] = [];
  const invalid: string[] = [];
  for (const part of parts) {
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(part)) invalid.push(part);
    else if (!emails.includes(part)) emails.push(part);
  }
  return { emails, invalid };
}

/** The choices for a public link's expiry, within the organization's maximum (and without "never" when one is required). */
export function expiryChoices(rule: { maxDays: number | null; required: boolean } | undefined): { value: string; label: string }[] {
  const all = [
    { days: 1, label: "1 day" },
    { days: 7, label: "7 days" },
    { days: 30, label: "30 days" },
    { days: 90, label: "90 days" },
    { days: 365, label: "1 year" },
  ];
  const max = rule?.maxDays ?? undefined;
  const choices = all.filter((choice) => max === undefined || choice.days <= max).map((choice) => ({ value: `${choice.days}d`, label: choice.label }));
  if (max !== undefined && !choices.some((choice) => choice.value === `${max}d`)) choices.push({ value: `${max}d`, label: `${max} days` });
  if (!rule?.required && max === undefined) choices.unshift({ value: "", label: "Never" });
  return choices;
}

/** The picks a step has made: teams, members, people outside, and the text still in "Add an email" (`draft`). */
export type Picks = { teams: string[]; people: string[]; outside: string[]; draft?: string; expiresIn?: string };

/** Emails typed or pasted into "Add an email", added to `list` (lower-cased, without repeats), and what is not one. */
export function addEmails(list: string[], text: string): { list: string[]; invalid: string[] } {
  const typed = parseEmails(text);
  return { list: [...list, ...typed.emails.filter((email) => !list.includes(email))], invalid: typed.invalid };
}

/** The people outside a share goes to: those added, and a whole address still in the field; `invalid` when it is not. */
const outsiders = (picks: Picks) => addEmails(picks.outside, picks.draft ?? "");

/**
 * What Share sends for `kind`: nothing until a team, a person or an email is chosen, or while the field holds what is
 * not an address. A public link carries its expiry (`""`: never).
 */
export function shareRequest(kind: ArtifactAudienceKind, picks: Picks): ShareChatSessionRequest | undefined {
  switch (kind) {
    case "org":
      return { audience: kind };
    case "public":
      return { audience: kind, ...(picks.expiresIn ? { expiresIn: picks.expiresIn } : {}) };
    case "teams":
      return picks.teams.length ? { audience: kind, teams: picks.teams } : undefined;
    case "people":
      return picks.people.length ? { audience: kind, people: picks.people } : undefined;
    case "external": {
      const { list, invalid } = outsiders(picks);
      return list.length && !invalid.length ? { audience: kind, people: list } : undefined;
    }
  }
}

/**
 * What the Share button says for the picks: whom it shares with (by `name` when it is one), or what is still missing
 * (`ready` false), so the button explains itself instead of a hint beside it.
 */
export function shareButton(kind: ArtifactAudienceKind, picks: Picks, name: (value: string) => string): { label: string; ready: boolean } {
  const whom = (list: string[], one: string, many: string, missing: string) =>
    list.length === 0 ? { label: missing, ready: false } : { label: `Share with ${list.length === 1 ? name(list[0]) || one : `${list.length} ${many}`}`, ready: true };
  switch (kind) {
    case "org":
    case "public":
      return { label: kind === "public" ? "Create the link" : "Share", ready: true };
    case "teams":
      return whom(picks.teams, "a team", "teams", "Choose a team");
    case "people":
      return whom(picks.people, "a person", "people", "Choose a person");
    case "external": {
      const { list, invalid } = outsiders(picks);
      return invalid.length ? { label: "Finish the address", ready: false } : whom(list, "a person", "people", "Add an email address");
    }
  }
}

/**
 * Shares `target` with `request`, replacing what was shared before, so one audience stays open (Nexus UI's levels).
 * Gateway replaces a chat's earlier versions in the same step; a page's earlier shares (`active`) are revoked after.
 */
export async function shareAs(
  client: Pick<ArtifactsClient, "shareChatSession" | "sharePage" | "revokeArtifact">,
  target: Shareable,
  request: ShareChatSessionRequest,
  active: Pick<Artifact, "id">[],
): Promise<Artifact> {
  if (target.kind === "chat") return client.shareChatSession(target.id, { ...request, replace: true });
  const { audience, teams, people, expiresIn } = request;
  const made = await client.sharePage(target.id, { audience, ...(teams ? { teams } : {}), ...(people ? { people } : {}), ...(expiresIn ? { expiresIn } : {}) });
  for (const share of active) if (share.id !== made.id) await client.revokeArtifact(share.id);
  return made;
}

/**
 * Who has access: the active shares of yours from the chat or the page, newest first, each with its own audience.
 * Gateway files a page's shares under the page's id as their session, so one filter reads both (as Nexus UI's page menu).
 */
export async function whoHasAccess(client: Pick<ArtifactsClient, "artifacts">, target: Shareable): Promise<Artifact[]> {
  const page = await client.artifacts({ scope: "shared_by_me", session: target.id, status: "active", limit: 50 });
  return [...page.artifacts].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

const times = (n: number) => (n === 1 ? "once" : `${n} times`);

/** How often a share outside was opened, and whether anyone reported it. */
export function accessLine(access: Pick<ArtifactAccess, "audience" | "opens" | "reports">): string {
  return [access.opens ? `Opened ${times(access.opens)}` : "Not opened yet", access.reports > 0 ? `${access.reports} ${access.reports === 1 ? "report" : "reports"}` : ""].filter(Boolean).join(" · ");
}

/** A person on a share outside: asking, denied, or how often they opened it. */
export function personState(person: Pick<ArtifactAccessPerson, "status" | "opens">): string {
  if (person.status === "requested") return "Asked for access";
  if (person.status === "denied") return "Denied";
  return person.opens ? `Opened ${times(person.opens)}` : "Not opened yet";
}
