import { createCoreClient } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { restoreQueuedChat } from "./queued-unarchive";

const archived = { id: "chat-1", updatedAt: "2026-11-24T20:00:00.000Z", archived: true };

function gateway(current = archived, patchStatus = 200) {
  const writes: Array<{ body: unknown; version: string | null }> = [];
  const core = createCoreClient({
    baseUrl: "https://gateway.test",
    ifMatch: true,
    fetch: async (input, request) => {
      expect(new URL(String(input)).pathname).toBe("/v1/chat/sessions/chat-1");
      if (request?.method === "GET") return Response.json(current, { headers: { etag: '"7"' } });
      expect(request?.method).toBe("PATCH");
      writes.push({ body: JSON.parse(String(request?.body)), version: new Headers(request?.headers).get("if-match") });
      if (patchStatus === 412) return Response.json({ error: { code: "version_mismatch" } }, { status: 412 });
      return Response.json({ ...current, archived: false }, { headers: { etag: '"8"' } });
    },
  });
  return { core, writes };
}

describe("replaying an offline unarchive", () => {
  it("uses the current authorized chat version for the unarchive write", async () => {
    const { core, writes } = gateway();
    await expect(restoreQueuedChat(core, archived, new AbortController().signal)).resolves.toBe("restored");
    expect(writes).toEqual([{ body: { archived: false }, version: '"7"' }]);
  });

  it("clears an action already completed by another device or a lost response", async () => {
    const { core, writes } = gateway({ ...archived, archived: false });
    await expect(restoreQueuedChat(core, archived, new AbortController().signal)).resolves.toBe("restored");
    expect(writes).toHaveLength(0);
  });

  it("keeps a newer archive and refuses a race after the read", async () => {
    const old = gateway({ ...archived, updatedAt: "2026-11-24T21:00:00.000Z" });
    await expect(restoreQueuedChat(old.core, archived, new AbortController().signal)).resolves.toBe("changed");
    expect(old.writes).toHaveLength(0);

    const race = gateway(archived, 412);
    await expect(restoreQueuedChat(race.core, archived, new AbortController().signal)).resolves.toBe("changed");
    expect(race.writes).toEqual([{ body: { archived: false }, version: '"7"' }]);
  });
});
