import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

it("sets a module's write grants with PUT on its product and capability", async () => {
  const seen: { url: string; method?: string; body?: string }[] = [];
  const client = createCoreClient({
    baseUrl: "http://gateway.invalid",
    fetch: async (url, init) => {
      seen.push({ url: String(url), method: init?.method, body: typeof init?.body === "string" ? init.body : undefined });
      return new Response("{}", { status: 200, headers: { "content-type": "application/json" } });
    },
  });
  await client.setWriteGrants("robustness", "review", { connector: "github", actions: ["pull_review", "check_run"] });
  expect(seen[0]?.url).toBe("http://gateway.invalid/v1/policies/grants/robustness/review");
  expect(seen[0]?.method).toBe("PUT");
  expect(JSON.parse(seen[0]?.body ?? "{}")).toEqual({ connector: "github", actions: ["pull_review", "check_run"] });
});
