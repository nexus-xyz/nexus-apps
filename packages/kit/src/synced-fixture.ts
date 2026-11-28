/**
 * Fixtures for the synced-session viewer's tests and demos, never for a real session: they seal and lock
 * the way a runner does, which the viewer itself never does (`e2e.ts` only opens).
 *
 * The device, its locked copy of the content key and that copy's signature are the Rust core's vector bytes
 * (`rs/crates/nexus-client/tests/e2e-vectors.json`, `devices[0]` and `thread_keys[0]`); `synced-sessions.test.ts`
 * checks they still match. Event bodies are nexus-workbench's `Plaintext` (`session_sync.rs`), sealed with ADR I1's
 * associated data. More threads get copies locked here with HPKE's sender half and signed with the device's key.
 */
import type { E2eDevice, SyncedEvent, ThreadKey } from "./api";
import { b64u, eventAad, importDeviceSecret, keyVersion, lockInfo, sign, threadKeyMessage, type Bytes, type DeviceKey } from "./e2e";
import type { PrivateThread } from "./synced-sessions";

const hex = (text: string): Bytes => Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));

/** The vector device and the thread it holds a Rust-locked copy for. */
export const FIXTURE = {
  deviceId: "dev_7Kx3mQ9pL2vN8rT4wY6zA1",
  deviceSecret: "c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedf404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
  encryptionKey: "3CzKMejkO72R3_fkdcyjNH60eBB9W9dlq6SuSjDDXUQ",
  signingKey: "JUO5L_EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0",
  threadId: "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506",
  contentKey: "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  locked:
    "729d6561d2f278f4d70a0410502f81249b75689a7b0dfeb4d489aed6d481e3621db6f7f21d58daf65d0a32443aa11c8ee4991a27f92758468f6fd2abe2c2fe5a12c95a342567c8085c21e70ab485865b",
  signature: "6c7d69dda7466c45147f26acfd29025e7b313836b9fc9697c93599dc4148e44c76e4cd4f01f77e105082095ad36d504552880038170f137953e492b25ca72906",
} as const;

export const fixtureDeviceKey = (): Promise<DeviceKey> => importDeviceSecret(hex(FIXTURE.deviceSecret));

/** The vector device as Gateway lists it: the person's first device, trusted with no approval. */
export function fixtureDevice(overrides: Partial<E2eDevice> = {}): E2eDevice {
  return {
    id: FIXTURE.deviceId,
    name: "Chrome on macOS",
    encryptionKey: FIXTURE.encryptionKey,
    signingKey: FIXTURE.signingKey,
    state: "trusted",
    approvedBy: null,
    approvalSignature: null,
    createdAt: "2026-11-01T09:00:00Z",
    approvedAt: null,
    revokedAt: null,
    ...overrides,
  };
}

/** The Rust-locked, Rust-signed copy of the fixture thread's key, at key version 1. */
export function fixtureKeyCopy(): ThreadKey {
  return {
    threadId: FIXTURE.threadId,
    keyVersion: 1,
    deviceId: FIXTURE.deviceId,
    lockedKey: b64u(hex(FIXTURE.locked)),
    senderDeviceId: FIXTURE.deviceId,
    signature: b64u(hex(FIXTURE.signature)),
    createdAt: "2026-11-01T09:05:00Z",
  };
}

/** A copy of `raw` (32 bytes) for `threadId` at `epoch`, locked to the fixture device and signed by it. */
export async function lockFixtureKey(threadId: string, epoch: number, raw: Uint8Array): Promise<ThreadKey> {
  const device = await fixtureDeviceKey();
  const locked = await hpkeSeal(device.encryptionKey, lockInfo(threadId, epoch, FIXTURE.deviceId), raw);
  return {
    threadId,
    keyVersion: keyVersion(epoch),
    deviceId: FIXTURE.deviceId,
    lockedKey: b64u(locked),
    senderDeviceId: FIXTURE.deviceId,
    signature: b64u(await sign(device, threadKeyMessage(threadId, epoch, FIXTURE.deviceId, locked))),
    createdAt: "2026-11-01T09:05:00Z",
  };
}

/** One stored event of a thread, sealed as the runner seals it: `Plaintext` v1 under the content key at `epoch`. */
export async function sealFixtureEvent(
  threadId: string,
  seq: number,
  at: number,
  thread: PrivateThread,
  event: Record<string, unknown>,
  options: { epoch?: number; contentKey?: Uint8Array; agent?: string; usage?: SyncedEvent["usage"]; status?: SyncedEvent["status"] } = {},
): Promise<SyncedEvent> {
  const epoch = options.epoch ?? 0;
  const plaintext = JSON.stringify({
    version: 1,
    at,
    thread: { local_id: thread.localId, title: thread.title, project_name: thread.projectName, project_path: thread.projectPath, ...(thread.origin ? { origin: thread.origin } : {}) },
    event_json: JSON.stringify(event),
  });
  const key = await subtle().importKey("raw", new Uint8Array(options.contentKey ?? hex(FIXTURE.contentKey)), "AES-GCM", false, ["encrypt"]);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const sealed = new Uint8Array(
    await subtle().encrypt({ name: "AES-GCM", iv, additionalData: eventAad(threadId, epoch, seq), tagLength: 128 }, key, new TextEncoder().encode(plaintext)),
  );
  const bytes = new Uint8Array(iv.length + sealed.length);
  bytes.set(iv);
  bytes.set(sealed, iv.length);
  return {
    seq,
    at,
    agent: options.agent ?? "claude",
    usage: options.usage ?? null,
    status: options.status ?? null,
    keyVersion: keyVersion(epoch),
    sealed: b64u(bytes),
  };
}

function subtle(): SubtleCrypto {
  return globalThis.crypto.subtle;
}

// HPKE (RFC 9180) base mode, the sender's half, single shot: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM.
const utf8 = (text: string): Bytes => new TextEncoder().encode(text) as Bytes;
const u16 = (n: number): Bytes => Uint8Array.of(n >>> 8, n & 0xff);
const KEM_SUITE = concat(utf8("KEM"), u16(0x0020));
const HPKE_SUITE = concat(utf8("HPKE"), u16(0x0020), u16(0x0001), u16(0x0002));

async function hpkeSeal(recipient: Uint8Array, info: Bytes, plaintext: Uint8Array): Promise<Bytes> {
  const ephemeral = (await subtle().generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  const enc = new Uint8Array(await subtle().exportKey("raw", ephemeral.publicKey));
  const publicKey = await subtle().importKey("raw", new Uint8Array(recipient), { name: "X25519" }, true, []);
  const dh = new Uint8Array(await subtle().deriveBits({ name: "X25519", public: publicKey }, ephemeral.privateKey, 256));
  const eaePrk = await extract(KEM_SUITE, new Uint8Array(), "eae_prk", dh);
  const shared = await expand(KEM_SUITE, eaePrk, "shared_secret", concat(enc, recipient), 32);
  const context = concat(
    Uint8Array.of(0),
    await extract(HPKE_SUITE, new Uint8Array(), "psk_id_hash", new Uint8Array()),
    await extract(HPKE_SUITE, new Uint8Array(), "info_hash", info),
  );
  const secret = await extract(HPKE_SUITE, shared, "secret", new Uint8Array());
  const key = await subtle().importKey("raw", await expand(HPKE_SUITE, secret, "key", context, 32), "AES-GCM", false, ["encrypt"]);
  const iv = await expand(HPKE_SUITE, secret, "base_nonce", context, 12);
  return concat(enc, new Uint8Array(await subtle().encrypt({ name: "AES-GCM", iv, tagLength: 128 }, key, new Uint8Array(plaintext))));
}

function extract(suite: Bytes, salt: Bytes, label: string, ikm: Bytes): Promise<Bytes> {
  return hmac(salt.length ? salt : new Uint8Array(32), concat(utf8("HPKE-v1"), suite, utf8(label), ikm));
}

async function expand(suite: Bytes, prk: Bytes, label: string, info: Bytes, length: number): Promise<Bytes> {
  const block = await hmac(prk, concat(u16(length), utf8("HPKE-v1"), suite, utf8(label), info, Uint8Array.of(1)));
  return block.slice(0, length);
}

async function hmac(key: Bytes, data: Bytes): Promise<Bytes> {
  const hmacKey = await subtle().importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await subtle().sign("HMAC", hmacKey, data));
}

function concat(...parts: Uint8Array[]): Bytes {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
