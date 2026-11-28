import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

it("bounds a stalled run lookup even while its caller's cancellation signal stays active", async () => {
  const focus = new AbortController();
  const client = createCoreClient({
    baseUrl: "https://gateway.test",
    timeoutMs: 20,
    fetch: async (_input, init) => {
      const signal = init?.signal;
      if (!signal) throw new Error("Missing request signal");
      signal.throwIfAborted();
      return new Promise<Response>((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason), { once: true });
      });
    },
  });
  await expect(client.agentSession("stalled", focus.signal)).rejects.toMatchObject({ name: "TimeoutError" });
  expect(focus.signal.aborted).toBe(false);
});
