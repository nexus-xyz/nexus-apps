import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

it("preserves a delegated payment token and the purchase retry key together", async () => {
  const seen: { path: string; method?: string; body: unknown; key: string | null }[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (url, init) => {
    seen.push({ path: new URL(String(url)).pathname, method: init?.method, body: JSON.parse(String(init?.body)), key: new Headers(init?.headers).get("idempotency-key") });
    return new Response(JSON.stringify({ status: "pending", amountUsd: "10.00" }));
  } });

  for (let attempt = 0; attempt < 2; attempt++) {
    await core.buyCredits("10.00", { paymentToken: "spt_wallet", requestId: "purchase:1" });
  }
  expect(seen).toEqual([
    { path: "/v1/billing/credits", method: "POST", body: { amountUsd: "10.00", paymentToken: "spt_wallet" }, key: "purchase:1" },
    { path: "/v1/billing/credits", method: "POST", body: { amountUsd: "10.00", paymentToken: "spt_wallet" }, key: "purchase:1" },
  ]);
});
