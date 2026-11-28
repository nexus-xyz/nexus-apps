import { expect, it } from "vitest";
import type { AgentLink } from "./api";
import { CoreError, createCoreClient } from "./core-client";

const link: AgentLink = {
  id: "agl_1",
  userId: "usr_1",
  agent: "slack",
  label: "Slack",
  grant: ["models.use"],
  prefix: "nxo_ab12",
  createdAt: "2026-10-20T10:00:00.000Z",
  expiresAt: "2027-01-18T10:00:00.000Z",
  lastUsedAt: "2026-10-27T09:00:00.000Z",
};

function client(seen: { method: string; url: string }[], status: number, body: unknown) {
  return createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      seen.push({ method: init?.method ?? "GET", url: String(url) });
      return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    },
  });
}

it("lists your agent links with GET /v1/agent-links", async () => {
  const seen: { method: string; url: string }[] = [];
  const list = await client(seen, 200, { agentLinks: [link] }).listAgentLinks();
  expect(seen).toEqual([{ method: "GET", url: "http://gateway.invalid/v1/agent-links" }]);
  expect(list.agentLinks).toEqual([link]);
});

it("unlinks one agent link by its id, encoded, with DELETE", async () => {
  const seen: { method: string; url: string }[] = [];
  const ended = await client(seen, 200, { ...link, revokedAt: "2026-10-28T00:00:00.000Z" }).unlinkAgentLink("agl_1/x");
  expect(seen).toEqual([{ method: "DELETE", url: "http://gateway.invalid/v1/agent-links/agl_1%2Fx" }]);
  expect(ended.revokedAt).toBe("2026-10-28T00:00:00.000Z");
});

it("throws the Gateway's refusal, with its status and code", async () => {
  const refusal = { error: { message: "no agent link agl_9", code: "not_found" } };
  const error = await client([], 404, refusal).unlinkAgentLink("agl_9").catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(CoreError);
  expect(error).toMatchObject({ status: 404, code: "not_found", message: "no agent link agl_9" });
});
