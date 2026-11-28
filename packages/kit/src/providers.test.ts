import { expect, it } from "vitest";
import type { ProviderView } from "./api";
import { createCoreClient } from "./core-client";

function client(seen: { method: string; url: string; body?: unknown }[], body: unknown) {
  return createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      seen.push({ method: init?.method ?? "GET", url: String(url), ...(init?.body ? { body: JSON.parse(String(init.body)) } : {}) });
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    },
  });
}

const baseten: ProviderView = { id: "baseten", name: "Baseten", country: "US", byok: true, status: "active", models: 4, key: "none", enabled: true };

it("lists the providers with GET /v1/providers", async () => {
  const seen: { method: string; url: string }[] = [];
  const list = await client(seen, { object: "list", data: [baseten] }).listProviders();
  expect(seen).toEqual([{ method: "GET", url: "http://gateway.invalid/v1/providers" }]);
  expect(list.data).toEqual([baseten]);
});

it("sends the organization policy's provider lists on PUT", async () => {
  const seen: { method: string; url: string; body?: unknown }[] = [];
  await client(seen, {}).setPolicy("organization", undefined, { providers: { deny: ["baseten"] } });
  expect(seen).toEqual([{ method: "PUT", url: "http://gateway.invalid/v1/policies/organization", body: { providers: { deny: ["baseten"] } } }]);
});
