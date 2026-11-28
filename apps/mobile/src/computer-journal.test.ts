import { randomBytes, randomUUID } from "node:crypto";
import { generateDeviceKey } from "@nexus/kit/e2e";
import { FIXTURE, fixtureDeviceKey } from "@nexus/kit/synced-fixture";
import { beforeEach, expect, it, vi } from "vitest";

const disk = vi.hoisted(() => new Map<string, string>());
vi.mock("expo-crypto", () => ({ randomUUID: () => randomUUID(), getRandomBytes: (length: number) => new Uint8Array(randomBytes(length)) }));
vi.mock("./store", () => ({ Storage: { getItemAsync: async (key: string) => disk.get(key), setItemAsync: async (key: string, value: string) => { disk.set(key, value); }, removeItemAsync: async (key: string) => disk.delete(key) } }));
const { computerJournal } = await import("./computer-journal");
const parse = (value: unknown) => typeof value === "string" ? value : undefined;
beforeEach(() => disk.clear());
it("keeps native drafts ciphertext-only on disk and restores them using this phone's key", async () => {
  const holder = await fixtureDeviceKey();
  const journal = computerJournal("account:computer", FIXTURE.deviceId, holder, parse);
  await journal.save("Private native project and prompt");
  const stored = [...disk.values()][0]!;
  expect(stored).not.toContain("Private native project and prompt");
  expect(await computerJournal("account:computer", FIXTURE.deviceId, holder, parse).load()).toBe("Private native project and prompt");
  const other = await generateDeviceKey();
  // Another device does not even address this phone's journal key.
  expect(await computerJournal("account:computer", FIXTURE.deviceId, other, parse).load()).toBeUndefined();
});
it("keeps the last draft when saves overlap and fails closed on a corrupt saved request", async () => {
  const journal = computerJournal("account:computer", FIXTURE.deviceId, await fixtureDeviceKey(), parse);
  await Promise.all([journal.save("First draft"), journal.save("Latest draft")]);
  expect(await journal.load()).toBe("Latest draft");
  disk.set([...disk.keys()][0]!, "{}");
  await expect(journal.load()).rejects.toThrow("Saved computer request is invalid");
});
