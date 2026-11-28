import type { EnrollmentMode, MemberSummary, OrgDomain } from "@nexus/kit";

// General on the phone (brand v5's P71c·G4): the organization's name, its address, the domains people join by, and
// its owner, read only.

/** How people with an email on a domain join, as P71c names it. */
export const ENROLLMENT: Record<EnrollmentMode, string> = {
  automatic_invitation: "Automatic",
  automatic_suggestion: "Suggested",
  manual_invitation: "By invitation",
  enterprise_sso: "Single sign-on",
};

/** The organization's address in Nexus UI: `/app/<slug>`, its id when it has no slug. */
export const addressOf = (tenant: { id: string; slug?: string }) => `/app/${tenant.slug || tenant.id}`;

/** A domain's line: whether it is verified, and the invitations on it that nobody accepted yet. */
export function domainLine(domain: Pick<OrgDomain, "verified" | "pendingInvitations">): string {
  const waiting = domain.pendingInvitations;
  return [domain.verified ? "Verified" : "Not verified", waiting ? `${waiting} ${waiting === 1 ? "invitation" : "invitations"} waiting` : undefined]
    .filter(Boolean)
    .join(" · ");
}

/** The owner's name as people read it: their name, else their email; undefined when there is none in the list. */
export function ownerName(members: Pick<MemberSummary, "role" | "name" | "email" | "userId">[]): string | undefined {
  const owner = members.find((m) => m.role === "owner");
  return owner ? owner.name || owner.email || owner.userId : undefined;
}
