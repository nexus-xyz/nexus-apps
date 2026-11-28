import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  approvalMessage,
  derivationInfo,
  openRecoveryChallenge,
  RECOVER_CHALLENGE_LABEL,
  recoverChallengeInfo,
  formatRecoveryCode,
  generateRecoveryCode,
  HANDOVER_LABEL,
  handoverInfo,
  handoverMessage,
  openHandover,
  sealHandover,
  holderApprovalMessage,
  lockContentKey,
  lockContentKeyWithIkm,
  parseRecoveryCode,
  PASSKEY_PRF_LABEL,
  PASSKEY_SALT,
  passkeyPrfInput,
  passkeySecret,
  RECOVERY_CODE_SALT,
  recoveryCodeSecret,
  unlockContentKeyBytes,
  b64u,
  type DeviceKey,
  DEVICE_SECRET_LEN,
  E2eError,
  e2eSupported,
  epochOf,
  EVENT_AAD_LABEL,
  eventAad,
  FINGERPRINT_LABEL,
  fingerprint,
  fromB64u,
  generateDeviceKey,
  importContentKey,
  importDeviceSecret,
  importHolderSecret,
  keyVersion,
  LOCK_INFO_LABEL,
  lockInfo,
  openEvent,
  revocationMessage,
  shortCode,
  sign,
  threadKeyMessage,
  unlockContentKey,
  verify,
} from "./e2e";
import { type DeviceKeyStore, loadOrCreateDeviceKey, sealDeviceKey, unsealDeviceKey } from "./e2e-store";
import type { HolderKind } from "./api";

// The Rust core's vectors, read in place: both sides check the one file. Its events and locks are the
// bytes nexus-client seals and locks, byte for byte (`events_seal_and_open_byte_for_byte_as_the_vectors`).
const vectorsPath = fileURLToPath(new URL("../../../crates/nexus-client/tests/e2e-vectors.json", import.meta.url));
const v = JSON.parse(readFileSync(vectorsPath, "utf8")) as {
  labels: { event_aad: string; lock_info: string; fingerprint: string; recover_challenge: string; handover: string };
  hpke: { mode: number; kem_id: number; kdf_id: number; aead_id: number };
  events: { content_key: string; epoch: number; thread_id: string; seq: number; nonce: string; plaintext: string; aad: string; sealed: string }[];
  locks: { holder_secret_key: string; holder_key: string; holder_id: string; thread_id: string; epoch: number; ikm_e: string; content_key: string; info: string; locked: string }[];
  devices: { secret: string; encryption_key: string; signing_key: string; encryption_key_b64u: string; signing_key_b64u: string; fingerprint: string; short_code: string }[];
  recovery_keys: { secret: string; encryption_key: string; fingerprint: string; short_code: string }[];
  signer_secret: string;
  approvals: { device_id: string; encryption_key: string; signing_key: string; message: string; signature: string }[];
  thread_keys: { thread_id: string; epoch: number; key_version: number; device_id: string; locked: string; message: string; signature: string }[];
  revocations: { device_id: string; message: string; signature: string }[];
  salts: { recovery_code: string; passkey: string };
  passkey_prf_label: string;
  passkey_prf_input: string;
  recovery_codes: { bytes: string; text: string; person_id: string; info: string; secret: string; encryption_key: string; encryption_key_b64u: string; fingerprint: string }[];
  recovery_code_typos: string[];
  passkeys: { prf_output: string; person_id: string; credential_id: string; info: string; secret: string; encryption_key: string; encryption_key_b64u: string; fingerprint: string }[];
  holder_approvals: { holder_id: string; kind: string; encryption_key: string; message: string; signature: string }[];
  recover_challenges: { holder_secret_key: string; holder_key: string; person_id: string; holder_id: string; challenge: string; info: string; sealed: string }[];
  handovers: { sealed_by: string; device_index: number; device_id: string; approver_id: string; payload: string; info: string; message: string; sealed: string }[];
};

const THREAD = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506";

function bytes(text: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));
}

function hex(data: Uint8Array): string {
  return Array.from(data, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function flipped(data: Uint8Array, bit: number): Uint8Array {
  const out = new Uint8Array(data);
  out[bit >> 3]! ^= 1 << (bit & 7);
  return out;
}

async function fails(promise: Promise<unknown>, kind: E2eError["kind"]): Promise<void> {
  await expect(promise).rejects.toSatisfy((error) => error instanceof E2eError && error.kind === kind);
}

describe("e2e: the shared vectors", () => {
  it("name this suite and these labels", () => {
    expect(v.labels).toEqual({ event_aad: EVENT_AAD_LABEL, lock_info: LOCK_INFO_LABEL, fingerprint: FINGERPRINT_LABEL, recover_challenge: RECOVER_CHALLENGE_LABEL, handover: HANDOVER_LABEL });
    expect(v.hpke).toEqual({ mode: 0, kem_id: 0x20, kdf_id: 1, aead_id: 2 });
  });

  it("events: the associated data byte for byte, and each sealed event opens", async () => {
    expect(v.events.length).toBeGreaterThan(0);
    for (const e of v.events) {
      expect(hex(eventAad(e.thread_id, e.epoch, e.seq))).toBe(e.aad);
      expect(e.sealed.startsWith(e.nonce)).toBe(true);
      const key = await importContentKey(bytes(e.content_key), e.epoch);
      expect(hex(await openEvent(key, e.thread_id, e.seq, bytes(e.sealed)))).toBe(e.plaintext);
    }
  });

  it("locks: the HPKE info byte for byte, and each locked key unlocks on its holder", async () => {
    expect(v.locks.length).toBeGreaterThan(0);
    for (const l of v.locks) {
      expect(hex(lockInfo(l.thread_id, l.epoch, l.holder_id))).toBe(l.info);
      const holder = await importHolderSecret(bytes(l.holder_secret_key));
      expect(hex(holder.encryptionKey)).toBe(l.holder_key);
      const content = await unlockContentKey(holder, l.thread_id, l.epoch, l.holder_id, bytes(l.locked));
      expect(content.epoch).toBe(l.epoch);
      // The unlocked key opens the vectors' events sealed under the same key and epoch: a Rust lock, then a Rust seal.
      for (const e of v.events.filter((e) => e.content_key === l.content_key && e.epoch === l.epoch)) {
        expect(hex(await openEvent(content, e.thread_id, e.seq, bytes(e.sealed)))).toBe(e.plaintext);
      }
    }
    expect(v.locks.some((l) => v.events.some((e) => e.content_key === l.content_key && e.epoch === l.epoch))).toBe(true);
  });

  it("devices: keys, base64url, fingerprints and short codes", async () => {
    for (const d of v.devices) {
      const device = await importDeviceSecret(bytes(d.secret));
      expect(hex(device.encryptionKey)).toBe(d.encryption_key);
      expect(hex(device.signingKey)).toBe(d.signing_key);
      expect(b64u(device.encryptionKey)).toBe(d.encryption_key_b64u);
      expect(b64u(device.signingKey)).toBe(d.signing_key_b64u);
      expect(hex(fromB64u(d.signing_key_b64u)!)).toBe(d.signing_key);
      const print = await fingerprint(device.encryptionKey, device.signingKey);
      expect(hex(print)).toBe(d.fingerprint);
      expect(shortCode(print)).toBe(d.short_code);
    }
    for (const r of v.recovery_keys) {
      const holder = await importHolderSecret(bytes(r.secret));
      expect(hex(holder.encryptionKey)).toBe(r.encryption_key);
      expect(hex(await fingerprint(holder.encryptionKey))).toBe(r.fingerprint);
      expect(shortCode(bytes(r.fingerprint))).toBe(r.short_code);
    }
  });

  it("signatures: Gateway's messages, signed and verified byte for byte", async () => {
    const signer = await importDeviceSecret(bytes(v.signer_secret));
    const signed = async (message: string, signature: string) => {
      expect(hex(await sign(signer, message))).toBe(signature);
      expect(await verify(signer.signingKey, message, bytes(signature))).toBe(true);
    };
    for (const a of v.approvals) {
      expect(approvalMessage(a.device_id, bytes(a.encryption_key), bytes(a.signing_key))).toBe(a.message);
      await signed(a.message, a.signature);
    }
    for (const t of v.thread_keys) {
      expect(keyVersion(t.epoch)).toBe(t.key_version);
      expect(epochOf(t.key_version)).toBe(t.epoch);
      expect(threadKeyMessage(t.thread_id, t.epoch, t.device_id, bytes(t.locked))).toBe(t.message);
      await signed(t.message, t.signature);
    }
    for (const r of v.revocations) {
      expect(revocationMessage(r.device_id)).toBe(r.message);
      await signed(r.message, r.signature);
    }
  });
});

describe("e2e: handovers", () => {
  it("open as the shared vectors, whichever side sealed them", async () => {
    const signer = await importDeviceSecret(bytes(v.signer_secret));
    expect(v.handovers.map((h) => h.sealed_by)).toEqual(["nexus-client", "kit"]);
    for (const h of v.handovers) {
      const device = await importDeviceSecret(bytes(v.devices[h.device_index]!.secret));
      expect(hex(handoverInfo(h.device_id, h.approver_id))).toBe(h.info);
      const payload = new TextEncoder().encode(h.payload);
      expect(handoverMessage(h.device_id, h.approver_id, payload)).toBe(h.message);
      const opened = await openHandover(device, h.device_id, h.approver_id, bytes(h.sealed));
      expect(new TextDecoder().decode(opened.payload)).toBe(h.payload);
      expect(await verify(signer.signingKey, h.message, opened.signature)).toBe(true);
    }
  });

  it("open only for their device and approver, and anyone can seal one", async () => {
    const [approver, device, gateway] = [await generateDeviceKey(), await generateDeviceKey(), await generateDeviceKey()];
    const payload = new TextEncoder().encode('{"version":1,"holders":[],"revoked":[]}');
    const signature = await sign(approver, handoverMessage("dev_c", "dev_b", payload));
    const sealed = await sealHandover("dev_c", "dev_b", device.encryptionKey, signature, payload);
    const opened = await openHandover(device, "dev_c", "dev_b", sealed);
    expect(await verify(approver.signingKey, handoverMessage("dev_c", "dev_b", opened.payload), opened.signature)).toBe(true);
    await fails(openHandover(device, "dev_x", "dev_b", sealed), "open");
    await fails(openHandover(device, "dev_c", "dev_x", sealed), "open");
    await fails(openHandover(approver, "dev_c", "dev_b", sealed), "open");
    await fails(openHandover(device, "dev_c", "dev_b", flipped(sealed, 400)), "open");
    // Gateway knows the device's public key: its list opens, and its signature is not the approver's.
    const forged = await sealHandover("dev_c", "dev_b", device.encryptionKey, await sign(gateway, handoverMessage("dev_c", "dev_b", payload)), payload);
    const read = await openHandover(device, "dev_c", "dev_b", forged);
    expect(await verify(approver.signingKey, handoverMessage("dev_c", "dev_b", read.payload), read.signature)).toBe(false);
  });
});

describe("e2e: recovery keys, the shared vectors", () => {
  it("recovery codes: the printed text, the HKDF info and secret, and the holder's keys", async () => {
    expect(v.salts.recovery_code).toBe(RECOVERY_CODE_SALT);
    expect(v.recovery_codes.length).toBeGreaterThan(0);
    for (const r of v.recovery_codes) {
      expect(formatRecoveryCode(bytes(r.bytes))).toBe(r.text);
      expect(hex(parseRecoveryCode(r.text))).toBe(r.bytes);
      expect(hex(parseRecoveryCode(r.text.toLowerCase().replace(/-/g, " ")))).toBe(r.bytes);
      expect(hex(derivationInfo(r.person_id))).toBe(r.info);
      const secret = await recoveryCodeSecret(bytes(r.bytes), r.person_id);
      expect(hex(secret)).toBe(r.secret);
      const holder = await importHolderSecret(secret);
      expect(hex(holder.encryptionKey)).toBe(r.encryption_key);
      expect(b64u(holder.encryptionKey)).toBe(r.encryption_key_b64u);
      expect(hex(await fingerprint(holder.encryptionKey))).toBe(r.fingerprint);
    }
    for (const typo of v.recovery_code_typos) expect(() => parseRecoveryCode(typo), typo).toThrow(E2eError);
  });

  it("passkeys: the PRF input, the HKDF info and secret, and the holder's keys", async () => {
    expect(v.salts.passkey).toBe(PASSKEY_SALT);
    expect(v.passkey_prf_label).toBe(PASSKEY_PRF_LABEL);
    expect(hex(await passkeyPrfInput())).toBe(v.passkey_prf_input);
    for (const p of v.passkeys) {
      expect(hex(derivationInfo(p.person_id, bytes(p.credential_id)))).toBe(p.info);
      const secret = await passkeySecret(bytes(p.prf_output), p.person_id, bytes(p.credential_id));
      expect(hex(secret)).toBe(p.secret);
      const holder = await importHolderSecret(secret);
      expect(hex(holder.encryptionKey)).toBe(p.encryption_key);
      expect(b64u(holder.encryptionKey)).toBe(p.encryption_key_b64u);
      expect(hex(await fingerprint(holder.encryptionKey))).toBe(p.fingerprint);
    }
  });

  it("recovery challenges: the HPKE info byte for byte, and each opens only for its person and holder", async () => {
    expect(v.recover_challenges).toHaveLength(2);
    for (const c of v.recover_challenges) {
      const holder = await importHolderSecret(bytes(c.holder_secret_key));
      expect(hex(holder.encryptionKey)).toBe(c.holder_key);
      expect(hex(recoverChallengeInfo(c.person_id, c.holder_id))).toBe(c.info);
      expect(hex(await openRecoveryChallenge(holder, c.person_id, c.holder_id, bytes(c.sealed)))).toBe(c.challenge);
      await fails(openRecoveryChallenge(holder, "user_other", c.holder_id, bytes(c.sealed)), "open");
      await fails(openRecoveryChallenge(holder, c.person_id, "dev_other", bytes(c.sealed)), "open");
    }
  });

  it("holder approvals: Gateway's v2 message, signed byte for byte", async () => {
    const signer = await importDeviceSecret(bytes(v.signer_secret));
    for (const a of v.holder_approvals) {
      const message = holderApprovalMessage(a.holder_id, a.kind as Exclude<HolderKind, "device">, bytes(a.encryption_key));
      expect(message).toBe(a.message);
      expect(hex(await sign(signer, message))).toBe(a.signature);
    }
  });

  it("locks: a content key locked here is nexus-client's bytes, and opens raw on its holder", async () => {
    for (const l of v.locks) {
      const locked = await lockContentKeyWithIkm(bytes(l.content_key), l.thread_id, l.epoch, l.holder_id, bytes(l.holder_key), bytes(l.ikm_e));
      expect(hex(locked)).toBe(l.locked);
      const holder = await importHolderSecret(bytes(l.holder_secret_key));
      const fresh = await lockContentKey(bytes(l.content_key), l.thread_id, l.epoch, l.holder_id, bytes(l.holder_key));
      expect(hex(await unlockContentKeyBytes(holder, l.thread_id, l.epoch, l.holder_id, fresh))).toBe(l.content_key);
      await fails(unlockContentKeyBytes(holder, l.thread_id, l.epoch + 1, l.holder_id, fresh), "open");
    }
    await fails(lockContentKey(new Uint8Array(32), THREAD, 0, "dev_x", new Uint8Array(32)), "key");
  });

  it("a recovery code round-trips and refuses what is not one", async () => {
    for (let i = 0; i < 64; i++) {
      const code = generateRecoveryCode();
      const text = formatRecoveryCode(code);
      expect(text).toMatch(/^[0-9A-Z*~$=]{5}-[0-9A-Z*~$=]{5}-[0-9A-Z*~$=]{5}-[0-9A-Z*~$=]{4}-[0-9A-Z*~$=]{4}-[0-9A-Z*~$=]{4}$/);
      expect(hex(parseRecoveryCode(text))).toBe(hex(code));
    }
    expect(hex(parseRecoveryCode("OOOOO-OOOOO-OOOOO-oooo-0000-0000"))).toBe("00".repeat(16));
    for (const bad of ["", "U0000-00000-00000-0000-0000-0000", "00000-00000-00000-0000-0000-000!", "00000"]) {
      expect(() => parseRecoveryCode(bad), bad).toThrow(E2eError);
    }
    await expect(recoveryCodeSecret(generateRecoveryCode(), "")).rejects.toThrow(E2eError);
  });
});

describe("e2e: what does not open or verify", () => {
  const e = v.events[0]!;
  const l = v.locks[0]!;

  it("an event opens only with its key, thread, epoch and number", async () => {
    const key = await importContentKey(bytes(e.content_key), e.epoch);
    const sealed = bytes(e.sealed);
    await fails(openEvent(key, e.thread_id, e.seq + 1, sealed), "open");
    await fails(openEvent(key, "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f507", e.seq, sealed), "open");
    await fails(openEvent(await importContentKey(bytes(e.content_key), e.epoch + 1), e.thread_id, e.seq, sealed), "open");
    await fails(openEvent(await importContentKey(new Uint8Array(32), e.epoch), e.thread_id, e.seq, sealed), "open");
  });

  it("every flipped bit or cut byte fails to open an event", async () => {
    const key = await importContentKey(bytes(e.content_key), e.epoch);
    const sealed = bytes(e.sealed);
    for (let bit = 0; bit < sealed.length * 8; bit++) await fails(openEvent(key, e.thread_id, e.seq, flipped(sealed, bit)), "open");
    for (let len = 0; len < sealed.length; len++) await fails(openEvent(key, e.thread_id, e.seq, sealed.slice(0, len)), "open");
  });

  it("a locked key opens only on its holder, for its thread, epoch and id", async () => {
    const holder = await importHolderSecret(bytes(l.holder_secret_key));
    const other = await importHolderSecret(bytes(v.locks[1]!.holder_secret_key));
    const locked = bytes(l.locked);
    await fails(unlockContentKey(other, l.thread_id, l.epoch, l.holder_id, locked), "open");
    await fails(unlockContentKey(holder, l.thread_id, l.epoch + 1, l.holder_id, locked), "open");
    await fails(unlockContentKey(holder, "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f507", l.epoch, l.holder_id, locked), "open");
    await fails(unlockContentKey(holder, l.thread_id, l.epoch, `${l.holder_id}x`, locked), "open");
    await fails(unlockContentKey(holder, l.thread_id, l.epoch, l.holder_id, locked.slice(1)), "open");
    // One flipped bit in each byte, across enc, the ciphertext and the tag.
    for (let at = 0; at < locked.length; at++) {
      await fails(unlockContentKey(holder, l.thread_id, l.epoch, l.holder_id, flipped(locked, at * 8 + (at % 8))), "open");
    }
  });

  it("a low-order ephemeral key does not open", async () => {
    const holder = await importHolderSecret(bytes(l.holder_secret_key));
    const locked = bytes(l.locked);
    locked.fill(0, 0, 32);
    await fails(unlockContentKey(holder, l.thread_id, l.epoch, l.holder_id, locked), "open");
  });

  it("a signature verifies only for its key and message", async () => {
    const a = v.approvals[0]!;
    const signer = await importDeviceSecret(bytes(v.signer_secret));
    const other = await importDeviceSecret(bytes(v.devices[1]!.secret));
    const signature = bytes(a.signature);
    expect(await verify(other.signingKey, a.message, signature)).toBe(false);
    expect(await verify(signer.signingKey, `${a.message}x`, signature)).toBe(false);
    expect(await verify(signer.signingKey, a.message, flipped(signature, 3))).toBe(false);
    expect(await verify(signer.signingKey, a.message, signature.slice(1))).toBe(false);
    expect(await verify(signer.signingKey.slice(1), a.message, signature)).toBe(false);
  });
});

describe("e2e: fields and encodings", () => {
  it("a thread id is a lowercase hyphenated UUID and a holder id has no colon", async () => {
    for (const bad of [THREAD.toUpperCase(), THREAD.replaceAll("-", ""), `{${THREAD}}`, `urn:uuid:${THREAD}`, ""]) {
      expect(() => eventAad(bad, 0, 0)).toThrow(E2eError);
      expect(() => lockInfo(bad, 0, "dev_a")).toThrow(E2eError);
    }
    for (const bad of ["", "dev:a", "dev a", "dév", "a".repeat(129)]) {
      expect(() => lockInfo(THREAD, 0, bad)).toThrow(E2eError);
      expect(() => revocationMessage(bad)).toThrow(E2eError);
    }
    expect(() => lockInfo(THREAD, 0, "a".repeat(128))).not.toThrow();
    for (const bad of [-1, 0.5, Number.NaN, 2 ** 53]) expect(() => eventAad(THREAD, bad, 0)).toThrow(E2eError);
    await fails(importContentKey(new Uint8Array(32), -1), "field");
  });

  it("key versions are epochs plus one, within Gateway's 32 bits", () => {
    expect(keyVersion(0)).toBe(1);
    expect(keyVersion(2 ** 31 - 2)).toBe(2 ** 31 - 1);
    expect(() => keyVersion(2 ** 31 - 1)).toThrow(E2eError);
    expect(epochOf(1)).toBe(0);
    for (const bad of [0, -1, 1.5, 2 ** 31]) expect(epochOf(bad)).toBeUndefined();
  });

  it("base64url is canonical and unpadded", () => {
    for (let len = 0; len < 8; len++) {
      const data = Uint8Array.from({ length: len }, (_, i) => 251 - i * 37);
      const text = b64u(data);
      expect(text).toBe(Buffer.from(data).toString("base64url"));
      expect(fromB64u(text)).toEqual(data);
    }
    for (const bad of ["A", "AB==", "AB+/", "AB", "AAB", "A A="]) expect(fromB64u(bad)).toBeUndefined();
    expect(fromB64u("AA")).toEqual(Uint8Array.of(0));
  });

  it("a short code is sixteen base32 characters of the first ten bytes", () => {
    expect(shortCode(new Uint8Array(32))).toBe("AAAAAAAAAAAAAAAA");
    expect(shortCode(new Uint8Array(32).fill(0xff))).toBe("7777777777777777");
    const tail = new Uint8Array(32);
    tail.fill(0xff, 10);
    expect(shortCode(tail)).toBe("AAAAAAAAAAAAAAAA");
  });
});

describe("e2e: a device made here", () => {
  it("WebCrypto has the curves", async () => {
    expect(await e2eSupported()).toBe(true);
  });

  it("signs and verifies, and its private keys cannot be exported", async () => {
    const device = await generateDeviceKey();
    expect(device.encryptionKey).toHaveLength(32);
    expect(device.signingKey).toHaveLength(32);
    const message = revocationMessage("dev_self");
    expect(await verify(device.signingKey, message, await sign(device, message))).toBe(true);
    for (const key of [device.x25519, device.ed25519]) {
      expect(key.extractable).toBe(false);
      await expect(crypto.subtle.exportKey("pkcs8", key)).rejects.toThrow();
    }
    const imported = await importDeviceSecret(bytes(v.devices[0]!.secret));
    expect(imported.x25519.extractable).toBe(false);
    expect(imported.ed25519.extractable).toBe(false);
    await expect(importDeviceSecret(new Uint8Array(DEVICE_SECRET_LEN - 1))).rejects.toThrow(RangeError);
  });

  it("is made once and then loaded from its store, even when two loads race", async () => {
    let stored: DeviceKey | undefined;
    const store: DeviceKeyStore = {
      load: async () => stored,
      saveIfAbsent: async (device) => (stored ??= device),
      clear: async () => {
        stored = undefined;
      },
    };
    const [first, second] = await Promise.all([loadOrCreateDeviceKey(store), loadOrCreateDeviceKey(store)]);
    expect(second).toBe(first);
    expect(await loadOrCreateDeviceKey(store)).toBe(first);
    await store.clear();
    expect(await loadOrCreateDeviceKey(store)).not.toBe(first);
  });

  // The browser's store only: the phone's WebCrypto subset (mobile/src/subtle.ts) runs this suite too, and has no AES-KW.
  it.skipIf(typeof crypto.subtle.wrapKey !== "function")("keeps no X25519 CryptoKey in the browser's record, because WebKit reads one back as null", async () => {
    const device = await generateDeviceKey(true);
    const record = await sealDeviceKey(device);
    expect(Object.values(record).some((value) => value instanceof CryptoKey && value.algorithm.name === "X25519")).toBe(false);
    const opened = await unsealDeviceKey(record);
    expect(opened.x25519.extractable).toBe(false);
    // The same key: it derives the same secret with another device.
    const peer = await crypto.subtle.importKey("raw", (await generateDeviceKey()).encryptionKey, { name: "X25519" }, true, []);
    const derive = async (key: CryptoKey) => new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: peer }, key, 256));
    expect(await derive(opened.x25519)).toEqual(await derive(device.x25519));
    // A record from before, which Chrome and Firefox read back, still opens.
    expect(await unsealDeviceKey(device)).toBe(device);
  });
});
