// The phone's WebCrypto subset (subtle.ts) in place of Node's: the kit's own e2e, pairing and synced-session suites
// run against it, so the phone reads the Rust core's vectors byte for byte, as the browser does.
import { randomBytes } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { createSubtle } from "./subtle";

const subtle = createSubtle((length) => new Uint8Array(randomBytes(length)));
vi.stubGlobal("crypto", { subtle, getRandomValues: globalThis.crypto.getRandomValues.bind(globalThis.crypto) });

await import("../../../packages/kit/src/e2e.test");
await import("../../../packages/kit/src/pairing.test");
await import("../../../packages/kit/src/synced-sessions.test");
await import("../../../packages/kit/src/remote-control.test");

describe("the phone's WebCrypto subset", () => {
  it("keeps a private key it was told not to export", async () => {
    const pair = (await subtle.generateKey({ name: "Ed25519" }, false, ["sign"])) as CryptoKeyPair;
    await expect(subtle.exportKey("jwk", pair.privateKey)).rejects.toThrow(/not extractable/);
    expect(new Uint8Array((await subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer)).toHaveLength(32);
  });

  it("refuses an X25519 agreement with a small-order key, as WebCrypto does", async () => {
    const pair = (await subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const zero = await subtle.importKey("raw", new Uint8Array(32), { name: "X25519" }, true, []);
    await expect(subtle.deriveBits({ name: "X25519", public: zero }, pair.privateKey, 256)).rejects.toThrow();
  });

  it("refuses what the kit never asks for", async () => {
    await expect(subtle.digest("SHA-1", new Uint8Array(1))).rejects.toThrow(/not computed/);
    await expect(subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt"])).rejects.toThrow(/not generated/);
  });
});
