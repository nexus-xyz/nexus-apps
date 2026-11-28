/**
 * "Ask an owner", shared by Nexus UI and the phone. A refusal or a used-up notice only an owner can fix
 * sends an owner to Billing. A member who is not an owner sees who the owners are and asks them in one tap; the owners get an inbox item
 * that names the person and the block. Names follow brand v5: "Maya C.", the first name and the last initial.
 */
import type { MemberSummary, NexusPaysReason } from "./api";
import type { BillingBlock } from "./notifications";
import type { Plan } from "./plan";

/** The block a Gateway refusal code is, for a question to the owners; undefined for a refusal an owner cannot fix. */
export function blockOf(code: string | undefined, plan?: Plan): BillingBlock | undefined {
  if (code === "usage_limit_reached") return "usage_limit";
  if (code === "credit_exhausted" || code === "subsidy_exhausted") return plan?.kind === "free" ? "free_start_used" : "credits_out";
  if (code === "payment_failed") return "payment_failed";
  if (code === "payment_past_due") return "past_due";
  if (code === "charge_processing") return "billing";
  return undefined;
}

/** The block a composer pay notice is (`nexusPays.reason`), for a question to the owners. */
export function blockOfReason(reason: NexusPaysReason): BillingBlock {
  switch (reason) {
    case "free_start_used_up":
    case "free_start_expired":
      return "free_start_used";
    case "credits_out":
      return "credits_out";
    case "usage_limit_reached":
      return "usage_limit";
    case "charge_failed":
    case "card_needs_confirming":
    case "card_disputed":
      return "payment_failed";
    case "past_due":
      return "past_due";
    case "billing_off":
    case "charge_processing":
      return "billing";
    default:
      // A reason a newer Gateway sends and this build does not know.
      return "billing";
  }
}

/** A person as an owner's list shows them: "Maya C." from "Maya Costa", the name alone when it is one word. */
export function shortName(member: Pick<MemberSummary, "name" | "email">): string {
  const words = (member.name ?? "").trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return member.email?.split("@")[0] ?? "An owner";
  if (words.length === 1) return words[0]!;
  return `${words[0]} ${words[words.length - 1]![0]!.toUpperCase()}.`;
}

/** The organization's owners, but not `me`, by name. */
export function ownersOf(members: MemberSummary[], me?: string): string[] {
  return members
    .filter((member) => member.role === "owner" && member.userId !== me)
    .map(shortName)
    .sort((a, b) => a.localeCompare(b));
}

/** The line that names the owners: "Owner: Maya C.", "Owners: Maya C. and Ana S.", "Owners: Ana S., Maya C. and 2 more". */
export function ownersLine(owners: string[]): string {
  if (owners.length === 0) return "";
  if (owners.length === 1) return `Owner: ${owners[0]}`;
  if (owners.length === 2) return `Owners: ${owners[0]} and ${owners[1]}`;
  return `Owners: ${owners[0]}, ${owners[1]} and ${owners.length - 2} more`;
}

/** What the person reads after they ask. */
export function askedLine(status: "sent" | "already_asked"): string {
  return status === "sent" ? "Asked. The owners see it in their inbox." : "You asked today. The owners have it in their inbox.";
}
