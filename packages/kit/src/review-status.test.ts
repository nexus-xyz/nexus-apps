import { expect, it } from "vitest";
import { CoreError, createCoreClient } from "./core-client";

function client(status: number, body: unknown = {}) {
  const seen: { url: string; method?: string; body?: string }[] = [];
  const core = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      seen.push({ url: String(url), method: init?.method, body: typeof init?.body === "string" ? init.body : undefined });
      return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    },
  });
  return { core, seen };
}

it("reads Review's status with GET, the repository and the pull request in the query", async () => {
  const { core, seen } = client(200, { contract: 1 });
  expect(await core.reviewStatus("acme/app", 42)).toEqual({ contract: 1 });
  expect(seen[0]?.method).toBe("GET");
  expect(seen[0]?.url).toBe("http://gateway.invalid/v1/robustness/review/status?repo=acme%2Fapp&pr=42");
});

it("evaluates a draft config with POST", async () => {
  const { core, seen } = client(200, { contract: 1 });
  await core.evaluateReviewStatus({ repo: "acme/app", config: { mode: "comment" } });
  expect(seen[0]?.method).toBe("POST");
  expect(seen[0]?.url).toBe("http://gateway.invalid/v1/robustness/review/status");
  expect(JSON.parse(seen[0]?.body ?? "{}")).toEqual({ repo: "acme/app", config: { mode: "comment" } });
});

it("answers null for a backend older than the status contract, and throws anything else", async () => {
  expect(await client(404, { error: { message: "no such verb" } }).core.reviewStatus("acme/app")).toBeNull();
  expect(await client(405).core.evaluateReviewStatus({ repo: "acme/app", config: {} })).toBeNull();
  await expect(client(403, { error: { message: "owners only" } }).core.evaluateReviewStatus({ repo: "acme/app", config: {} })).rejects.toBeInstanceOf(CoreError);
});
