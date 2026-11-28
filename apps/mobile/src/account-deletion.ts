import { CoreError, type AccountDeleted, type createCoreClient, type OrgSummary } from "@nexus/kit";
import { usd } from "./billing";
import { uiPage } from "./links";

type Core = ReturnType<typeof createCoreClient>;

export type DeletionPreview =
  | { kind: "ready"; plan: AccountDeleted }
  | { kind: "ownership"; organizations: OrgSummary[] }
  | { kind: "failed"; message: string };

export function accountDeletionFailure(error: unknown): string {
  if (error instanceof CoreError && error.code === "owns_organization")
    return "Transfer ownership of your company organizations before deleting your account.";
  if (error instanceof CoreError && error.code === "recent_sign_in_required")
    return "Sign out and sign in again, then return here within ten minutes.";
  if (error instanceof CoreError && error.code === "usage_settling")
    return "Calls that your card pays for, or a charge for your usage, are still running. Paying after the fact is now off. Try again in 10 minutes.";
  return error instanceof Error ? error.message : "The request did not finish. Try again.";
}

/** What the deletion does with money, one line each; empty when nothing is owed or lost. */
export function deletionMoney(plan: AccountDeleted): string[] {
  const lines: string[] = [];
  if (plan.unpaidUsageUsd) lines.push(`You owe ${usd(plan.unpaidUsageUsd)} for usage. Nexus charges it to your card on file before it deletes your account, and writes off what the card does not pay.`);
  if (plan.creditsLostUsd) lines.push(`Your ${usd(plan.creditsLostUsd)} of Nexus credits is lost. Nothing refunds it.`);
  return lines;
}

export async function previewAccountDeletion(core: Pick<Core, "deleteAccount" | "orgs">): Promise<DeletionPreview> {
  try {
    return { kind: "ready", plan: await core.deleteAccount({ dryRun: true }) };
  } catch (error) {
    if (!(error instanceof CoreError) || error.code !== "owns_organization")
      return { kind: "failed", message: accountDeletionFailure(error) };
    try {
      const { orgs } = await core.orgs();
      const organizations = orgs.filter((org) => org.kind === "company" && org.role === "owner");
      if (organizations.length) return { kind: "ownership", organizations };
      return { kind: "failed", message: "Your organization ownership changed. Check again before deleting your account." };
    } catch {
      return { kind: "failed", message: "Organization ownership could not be loaded. Try again." };
    }
  }
}

export function ownershipTransferUrl({ ui, gateway, org }: {
  ui?: string;
  gateway?: string;
  org: Pick<OrgSummary, "id" | "slug" | "clerkOrgId">;
}): string | undefined {
  const origin = ui ?? gateway;
  return origin ? uiPage(origin, org.slug || org.clerkOrgId || org.id, "/settings/members") : undefined;
}

export async function deleteConfirmedAccount({ core, confirmation, signOut }: {
  core: Pick<Core, "deleteAccount">;
  confirmation: string;
  signOut: () => Promise<void>;
}): Promise<void> {
  if (confirmation !== "DELETE") throw new Error("Type DELETE to confirm.");
  const result = await core.deleteAccount({ confirm: "DELETE" });
  if (!result.deleted) throw new Error("The account was not deleted. Try again.");
  await signOut();
}
