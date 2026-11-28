import { expect, it } from "vitest";
import { CoreError, createCoreClient } from "./core-client";

const paused = (retryAfter?: string) =>
  new Response(
    JSON.stringify({ error: { message: "sign-in is paused", type: "server_error", code: "cutover_paused" } }),
    { status: 503, headers: retryAfter ? { "retry-after": retryAfter } : {} },
  );

it("says sign-in is paused, with Retry-After's wait, and asks once", async () => {
  let calls = 0;
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", idempotency: true, fetch: async () => {
    calls++;
    return paused("600");
  } });
  const error = await core.createOrg({ name: "Acme" }).catch((e: unknown) => e);
  expect(error).toBeInstanceOf(CoreError);
  expect(error).toMatchObject({
    status: 503,
    code: "cutover_paused",
    interval: 600,
    message: "Sign-in is paused for maintenance; try again in 10 minutes.",
  });
  // A keyed write is otherwise retried on 503; the pause lasts minutes, so it is not.
  expect(calls).toBe(1);
});

it("rounds the wait up to whole minutes, and says a few minutes without Retry-After", async () => {
  for (const [retryAfter, wait] of [["30", "1 minute"], ["61", "2 minutes"], [undefined, "a few minutes"]] as const) {
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async () => paused(retryAfter) });
    await expect(core.startDeviceLogin("Nexus for iPhone")).rejects.toThrow(`Sign-in is paused for maintenance; try again in ${wait}.`);
  }
});

it("reads and retries any other 503 as before", async () => {
  let calls = 0;
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", idempotency: true, fetch: async () => {
    calls++;
    return new Response(JSON.stringify({ error: { message: "draining", code: "unavailable" } }), {
      status: 503,
      headers: { "retry-after": "5" },
    });
  } });
  const error = await core.createOrg({ name: "Acme" }).catch((e: unknown) => e);
  expect(error).toMatchObject({ status: 503, code: "unavailable", interval: undefined, message: "draining" });
  expect(calls).toBe(3);
});
