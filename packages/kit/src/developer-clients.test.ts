import { expect, it } from "vitest";
import type { DeveloperClient } from "./api";
import { CoreError, createCoreClient } from "./core-client";

const fork: DeveloperClient = {
  id: "nxc_Qm3xR8tLp2Ve",
  name: "Ada's fork",
  tier: "registered_fork",
  allowedScopes: ["models.use", "chat.ask"],
  createdAt: "2026-11-24T13:00:00.000Z",
  updatedAt: "2026-11-24T13:00:00.000Z",
};

type Seen = { method: string; url: string; body?: unknown };

function client(seen: Seen[], status: number, body: unknown) {
  return createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      seen.push({ method: init?.method ?? "GET", url: String(url), ...(init?.body ? { body: JSON.parse(String(init.body)) } : {}) });
      return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    },
  });
}

it("lists your forks and the grantable scopes with GET /v1/developer/clients", async () => {
  const seen: Seen[] = [];
  const list = await client(seen, 200, { clients: [fork], grantableScopes: ["models.use", "chat.ask"] }).listDeveloperClients();
  expect(seen).toEqual([{ method: "GET", url: "http://gateway.invalid/v1/developer/clients" }]);
  expect(list.clients).toEqual([fork]);
  expect(list.grantableScopes).toEqual(["models.use", "chat.ask"]);
});

it("registers a fork with its name and scopes, and returns its client_id", async () => {
  const seen: Seen[] = [];
  const created = await client(seen, 201, fork).registerDeveloperClient({ name: "Ada's fork", allowedScopes: ["models.use", "chat.ask"] });
  expect(seen).toEqual([{ method: "POST", url: "http://gateway.invalid/v1/developer/clients", body: { name: "Ada's fork", allowedScopes: ["models.use", "chat.ask"] } }]);
  expect(created.id).toBe("nxc_Qm3xR8tLp2Ve");
});

it("revokes a fork by its id, encoded, with an empty body by default", async () => {
  const seen: Seen[] = [];
  const revoked = await client(seen, 200, { ...fork, revokedAt: "2026-11-25T09:00:00.000Z", revokedReason: "revoked by owner" }).revokeDeveloperClient("nxc_a/b");
  expect(seen).toEqual([{ method: "POST", url: "http://gateway.invalid/v1/developer/clients/nxc_a%2Fb/revoke", body: {} }]);
  expect(revoked.revokedReason).toBe("revoked by owner");
});

it("throws the switch's 404 with its code, so a page can say the routes are off", async () => {
  const refusal = { error: { message: "OAuth clients are off on this Gateway", type: "not_found", code: "oauth_client_tiers_off" } };
  const error = await client([], 404, refusal).listDeveloperClients().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(CoreError);
  expect(error).toMatchObject({ status: 404, code: "oauth_client_tiers_off" });
});
