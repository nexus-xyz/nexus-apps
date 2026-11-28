import { describe, expect, it, vi } from "vitest";

const disk = new Map<string, string>();
vi.mock("./store", () => ({
  Storage: {
    getItemSync: (key: string) => disk.get(key) ?? null,
    setItemSync: (key: string, value: string) => void disk.set(key, value),
    removeItemSync: (key: string) => void disk.delete(key),
    getAllKeysSync: () => [...disk.keys()],
  },
}));

const { forgetDeviceTrust, phoneDeviceTrust } = await import("./device-trust");

describe("the phone's trust memory", () => {
  it("keeps every revocation and the first keys across checks, per device key, and goes with the key", async () => {
    disk.set("pref:appearance", "\"dark\"");
    await phoneDeviceTrust.remember("scope_a", { revoked: ["dev_x"], verified: { dev_a: { encryptionKey: "e1", signingKey: "s1" } } });
    const merged = await phoneDeviceTrust.remember("scope_a", { revoked: ["dev_y"], verified: { dev_a: { encryptionKey: "e2", signingKey: "s2" } } });
    expect(merged).toEqual({ revoked: ["dev_x", "dev_y"], verified: { dev_a: { encryptionKey: "e1", signingKey: "s1" } } });
    expect(await phoneDeviceTrust.remember("scope_b", { revoked: [], verified: {} })).toEqual({ revoked: [], verified: {} });
    forgetDeviceTrust();
    expect([...disk.keys()]).toEqual(["pref:appearance"]);
  });
});
