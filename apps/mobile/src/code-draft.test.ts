import { randomBytes, randomUUID } from "node:crypto";
import { FIXTURE, fixtureDeviceKey } from "@nexus/kit/synced-fixture";
import { beforeEach, expect, it, vi } from "vitest";

const disk = vi.hoisted(() => new Map<string, string>());
vi.mock("expo-crypto", () => ({ randomUUID: () => randomUUID(), getRandomBytes: (length: number) => new Uint8Array(randomBytes(length)) }));
vi.mock("./store", () => ({ Storage: { getItemAsync: async (key: string) => disk.get(key), setItemAsync: async (key: string, value: string) => { disk.set(key, value); }, removeItemAsync: async (key: string) => disk.delete(key) } }));
const { codeDraftStore, codeWaitStore, restoredDraft } = await import("./code-draft");
beforeEach(() => disk.clear());

const scope = { gateway: "https://gateway.test", tenantId: "tenant", personId: "person" };

it("keeps a session's draft as ciphertext, per thread and organization, until it is removed", async () => {
  const reader = { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() };
  const store = codeDraftStore(scope, "thread-a", reader);
  await store.save({ text: "Private message to the agent" });
  expect([...disk.values()].join()).not.toContain("Private message");
  expect(await codeDraftStore(scope, "thread-a", reader).load()).toEqual({ text: "Private message to the agent" });
  expect(await codeDraftStore(scope, "thread-b", reader).load()).toBeUndefined();
  expect(await codeDraftStore({ ...scope, tenantId: "another-org" }, "thread-a", reader).load()).toBeUndefined();
  await store.remove();
  expect(await store.load()).toBeUndefined();
});

it("never shows one person's draft to another person on the same phone, organization and thread", async () => {
  const reader = { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() };
  const mine = codeDraftStore(scope, "shared-thread", reader);
  const theirs = codeDraftStore({ ...scope, personId: "teammate" }, "shared-thread", reader);
  await mine.save({ text: "Only mine" });
  expect(await theirs.load()).toBeUndefined();
  await theirs.save({ text: "Only theirs" });
  expect(await mine.load()).toEqual({ text: "Only mine" });
  expect(await theirs.load()).toEqual({ text: "Only theirs" });
  await theirs.remove();
  expect(await mine.load()).toEqual({ text: "Only mine" });
});

it("keeps a waiting message as ciphertext, per person and thread, and refuses a malformed one", async () => {
  const reader = { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() };
  const envelope = { id: "0193f2a4-7c1e-7b3a-9d2e-5f6a7b8c9e10", senderDeviceId: "dev_phone", runnerDeviceId: "dev_runner", expiresAt: 42, lockedKey: "locked", sealed: "sealed", signature: "signature", waits: true };
  const wait = { envelope, text: "Run the slow suite", waitsFor: "Studio Mac" };
  await codeWaitStore(scope, "thread-a", reader).save(wait);
  expect([...disk.values()].join()).not.toContain("slow suite");
  expect(await codeWaitStore(scope, "thread-a", reader).load()).toEqual(wait);
  expect(await codeWaitStore(scope, "thread-b", reader).load()).toBeUndefined();
  expect(await codeWaitStore({ ...scope, personId: "teammate" }, "thread-a", reader).load()).toBeUndefined();
  expect(await codeDraftStore(scope, "thread-a", reader).load()).toBeUndefined();
  // A cancel the person started survives a restart, so the phone never sends that command again.
  await codeWaitStore(scope, "thread-d", reader).save({ ...wait, cancelling: true });
  expect(await codeWaitStore(scope, "thread-d", reader).load()).toEqual({ ...wait, cancelling: true });
  await codeWaitStore(scope, "thread-c", reader).save({ ...wait, envelope: { ...envelope, waits: false } });
  // A copy that does not parse fails its load; the session then drops it.
  await expect(codeWaitStore(scope, "thread-c", reader).load()).rejects.toThrow(/invalid/);
});

it("restores a saved draft only when the person typed nothing and the computer did not already confirm it", () => {
  expect(restoredDraft({ text: "Saved" }, "", undefined)).toBe("Saved");
  expect(restoredDraft({ text: "Saved" }, "Typed now", undefined)).toBeUndefined();
  expect(restoredDraft({ text: "Already sent " }, "", "Already sent")).toBeUndefined();
  expect(restoredDraft({ text: "   " }, "", undefined)).toBeUndefined();
  expect(restoredDraft(undefined, "", undefined)).toBeUndefined();
});
