import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

function client(seen: string[], body: unknown = { runs: [] }) {
  return createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (url) => {
    seen.push(String(url));
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  } });
}

it("asks for the first page of a report with no query string", async () => {
  const seen: string[] = [];
  await client(seen).moduleReports("intelligence", "review");
  expect(seen).toEqual(["http://gateway.invalid/v1/intelligence/review/report"]);
});

it("sends the page size, the cursor and the statuses of a paged report", async () => {
  const seen: string[] = [];
  await client(seen).moduleReports("intelligence", "review", { limit: 50, after: "run_42", status: ["failed", "paused"] });
  const url = new URL(seen[0] ?? "");
  expect(url.pathname).toBe("/v1/intelligence/review/report");
  expect(url.searchParams.get("limit")).toBe("50");
  expect(url.searchParams.get("after")).toBe("run_42");
  expect(url.searchParams.get("status")).toBe("failed,paused");
});

it("passes the next cursor of a page through", async () => {
  const page = await client([], { runs: [], next_cursor: "run_9" }).moduleReports("intelligence", "review", { limit: 50 });
  expect(page.next_cursor).toBe("run_9");
});
