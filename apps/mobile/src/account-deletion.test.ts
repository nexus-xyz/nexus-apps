import { describe, expect, it, vi } from "vitest";
import { CoreError, createCoreClient, type OrgSummary } from "@nexus/kit";
import { accountDeletionFailure, deleteConfirmedAccount, deletionMoney, ownershipTransferUrl, previewAccountDeletion } from "./account-deletion";

const deleted = { userId: "person", dryRun: false, deleted: true, left: [], planEnded: false, signInNeeded: false };

function gateway() {
  let orgs: OrgSummary[] = [
    { id: "personal", name: "Personal", kind: "personal", role: "owner" },
    { id: "company_a", slug: "acme", name: "Acme", kind: "company", role: "owner" },
    { id: "company_b", clerkOrgId: "org_beta", name: "Beta", kind: "company", role: "owner" },
    { id: "company_c", name: "Gamma", kind: "company", role: "admin" },
  ];
  let listFails = false;
  const requests: { path: string; body?: unknown }[] = [];
  const respond = (status: number, body: unknown) => new Response(JSON.stringify(body), {
    status, headers: { "Content-Type": "application/json" },
  });
  const core = createCoreClient({ baseUrl: "https://gateway.test", fetch: async (input, init) => {
    const path = new URL(input instanceof Request ? input.url : input).pathname;
    const body: unknown = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    requests.push({ path, body });
    if (path === "/v1/orgs") {
      return listFails
        ? respond(403, { error: { message: "Organization list unavailable", code: "role_required" } })
        : respond(200, { orgs });
    }
    if (path === "/v1/auth/account/delete") {
      return orgs.some((org) => org.kind === "company" && org.role === "owner")
        ? respond(409, { error: { message: "Transfer ownership first", code: "owns_organization" } })
        : respond(200, { ...deleted, dryRun: true, deleted: false });
    }
    throw new Error(`Unexpected request: ${path}`);
  } });
  return {
    core, requests,
    transfer(id: string) { orgs = orgs.map((org) => org.id === id ? { ...org, role: "admin" } : org); },
    failList() { listFails = true; },
  };
}

describe("account deletion", () => {
  it("makes no deletion request without the typed confirmation", async () => {
    const core = { deleteAccount: vi.fn().mockResolvedValue(deleted) };
    const signOut = vi.fn();
    for (const confirmation of ["", "delete", " DELETE "]) {
      await expect(deleteConfirmedAccount({ core, confirmation, signOut })).rejects.toThrow("Type DELETE");
    }
    expect(core.deleteAccount).not.toHaveBeenCalled();
    expect(signOut).not.toHaveBeenCalled();
  });

  it("keeps the person signed in until Gateway confirms deletion", async () => {
    const signOut = vi.fn();
    const core = { deleteAccount: vi.fn(async () => {
      expect(signOut).not.toHaveBeenCalled();
      return deleted;
    }) };
    await deleteConfirmedAccount({ core, confirmation: "DELETE", signOut });
    expect(core.deleteAccount).toHaveBeenCalledWith({ confirm: "DELETE" });
    expect(signOut).toHaveBeenCalledOnce();
  });

  it("keeps the sign-in when deletion fails or the response does not confirm deletion", async () => {
    const signOut = vi.fn();
    const core = { deleteAccount: vi.fn()
      .mockRejectedValueOnce(new Error("Transfer ownership first"))
      .mockResolvedValueOnce({ ...deleted, deleted: false }) };
    await expect(deleteConfirmedAccount({ core, confirmation: "DELETE", signOut })).rejects.toThrow("Transfer ownership");
    await expect(deleteConfirmedAccount({ core, confirmation: "DELETE", signOut })).rejects.toThrow("not deleted");
    expect(signOut).not.toHaveBeenCalled();
  });

  it("shows every owned company, excluding the personal account and companies owned by someone else", async () => {
    const h = gateway();
    expect(await previewAccountDeletion(h.core)).toMatchObject({
      kind: "ownership",
      organizations: [{ id: "company_a" }, { id: "company_b" }],
    });
    expect(h.requests).toEqual([
      { path: "/v1/auth/account/delete", body: { dryRun: true } },
      { path: "/v1/orgs", body: undefined },
    ]);
  });

  it("keeps deletion blocked after a cancelled or partial handoff, then requires a fresh successful dry run", async () => {
    const h = gateway();
    expect((await previewAccountDeletion(h.core)).kind).toBe("ownership");
    // Closing the browser without a transfer does not unlock deletion.
    expect((await previewAccountDeletion(h.core)).kind).toBe("ownership");
    h.transfer("company_a");
    expect(await previewAccountDeletion(h.core)).toMatchObject({
      kind: "ownership", organizations: [{ id: "company_b" }],
    });
    h.transfer("company_b");
    expect(await previewAccountDeletion(h.core)).toMatchObject({
      kind: "ready", plan: { dryRun: true, deleted: false },
    });
    expect(h.requests.filter((request) => request.path === "/v1/auth/account/delete")
      .every((request) => JSON.stringify(request.body) === JSON.stringify({ dryRun: true }))).toBe(true);
  });

  it("does not unlock deletion when the ownership list fails", async () => {
    const h = gateway();
    h.failList();
    expect(await previewAccountDeletion(h.core)).toEqual({
      kind: "failed", message: "Organization ownership could not be loaded. Try again.",
    });
  });

  it("opens the exact organization's Members page using its slug, `clerkOrgId`, or Gateway id", () => {
    const base = { ui: "https://nexus.test", gateway: "https://gateway.test" };
    expect(ownershipTransferUrl({ ...base, org: { id: "ten_a", slug: "a/b", clerkOrgId: "org_a" } }))
      .toBe("https://nexus.test/app/a%2Fb/settings/members");
    expect(ownershipTransferUrl({ ...base, org: { id: "ten_b", clerkOrgId: "org_b" } }))
      .toBe("https://nexus.test/app/org_b/settings/members");
    expect(ownershipTransferUrl({ gateway: base.gateway, org: { id: "ten_c" } }))
      .toBe("https://gateway.test/app/ten_c/settings/members");
    expect(ownershipTransferUrl({ org: { id: "ten_c" } })).toBeUndefined();
  });
});

describe("what a deletion does with money", () => {
  it("says the unpaid usage the card pays first and the Nexus credits that are lost, and nothing when none", () => {
    expect(deletionMoney({ ...deleted, dryRun: true, deleted: false, unpaidUsageUsd: "9.990000", creditsLostUsd: "15.000000" })).toEqual([
      "You owe $9.99 for usage. Nexus charges it to your card on file before it deletes your account, and writes off what the card does not pay.",
      "Your $15.00 of Nexus credits is lost. Nothing refunds it.",
    ]);
    expect(deletionMoney(deleted)).toEqual([]);
  });

  it("says sign-in is paused, with the wait, while the operator pauses it, and asks once", async () => {
    let calls = 0;
    const core = createCoreClient({ baseUrl: "https://gateway.test", idempotency: true, fetch: async () => {
      calls++;
      return new Response(JSON.stringify({ error: { message: "paused", type: "server_error", code: "cutover_paused" } }), {
        status: 503, headers: { "Content-Type": "application/json", "Retry-After": "600" },
      });
    } });
    expect(await previewAccountDeletion(core)).toEqual({
      kind: "failed", message: "Sign-in is paused for maintenance; try again in 10 minutes.",
    });
    expect(calls).toBe(1);
  });

  it("asks to wait while calls or a charge still run on the card", () => {
    const error = new CoreError(409, "calls are still running", "usage_settling");
    expect(accountDeletionFailure(error)).toContain("Try again in 10 minutes.");
  });
});
