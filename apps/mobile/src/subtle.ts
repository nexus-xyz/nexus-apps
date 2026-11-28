/**
 * The part of WebCrypto that `@nexus/kit/e2e` uses, for the phone. Hermes has no `crypto.subtle`, and the
 * kit reaches `globalThis.crypto.subtle` at call time, so the phone installs this and runs the kit's own format code:
 * the device key, pairing, unlocking a thread's key and opening its events, byte for byte as the browser and the Rust
 * core do. `subtle.test.ts` runs the kit's e2e, pairing and synced-session tests against it.
 *
 * Only what the kit calls: X25519 (generate, raw and PKCS #8 import, raw and JWK export, deriveBits), Ed25519
 * (the same, sign and verify), HMAC-SHA-256 (sign), AES-GCM (decrypt, and encrypt for its fixtures) and SHA-256. Anything else rejects, as a
 * browser without it would. The primitives are noble's audited pure-JS ones. A private key lives in the key object's
 * closure, in memory only; `device-key.ts` keeps the device's secret in the Keychain or the Android Keystore.
 *
 * Pure TypeScript with no React Native import, so vitest runs it.
 */
import { gcm } from "@noble/ciphers/aes.js";
import { ed25519, x25519 } from "@noble/curves/ed25519.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";

const KEY_LEN = 32;
// PKCS #8 wrappers of a raw 32-byte private key (RFC 8410), as the kit builds them: X25519 is OID 1.3.101.110, Ed25519
// 1.3.101.112.
const PKCS8_PREFIX = { X25519: hex("302e020100300506032b656e04220420"), Ed25519: hex("302e020100300506032b657004220420") } as const;

type Curve = "X25519" | "Ed25519";
type Name = Curve | "AES-GCM" | "HMAC";

/** A key as the kit holds it. Its bytes are reachable only through this module. */
interface Key {
  readonly type: "public" | "private" | "secret";
  readonly extractable: boolean;
  readonly algorithm: { name: Name };
  readonly usages: readonly string[];
}

const secrets = new WeakMap<Key, Uint8Array>();

function key(type: Key["type"], name: Name, bytes: Uint8Array, extractable: boolean, usages: readonly string[]): Key {
  const made: Key = Object.freeze({ type, extractable, algorithm: Object.freeze({ name }), usages: Object.freeze([...usages]) });
  secrets.set(made, bytes.slice());
  return made;
}

function bytesOf(made: unknown, name: Name, type: Key["type"], usage?: string): Uint8Array {
  const found = secrets.get(made as Key);
  const meta = made as Key;
  if (!found || meta.algorithm.name !== name || meta.type !== type) throw fail("InvalidAccessError", `the key is not a ${name} ${type} key`);
  if (usage && !meta.usages.includes(usage)) throw fail("InvalidAccessError", `the key cannot ${usage}`);
  return found;
}

function fail(name: string, message: string): Error {
  return Object.assign(new Error(message), { name });
}

function nameOf(algorithm: unknown): string {
  return typeof algorithm === "string" ? algorithm : String((algorithm as { name?: unknown })?.name ?? "");
}

/** A BufferSource's bytes, copied. */
function input(data: unknown): Uint8Array {
  if (data instanceof Uint8Array) return data.slice();
  if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
  throw fail("TypeError", "expected a BufferSource");
}

/** WebCrypto answers with a fresh ArrayBuffer. */
function output(data: Uint8Array): ArrayBuffer {
  return data.slice().buffer as ArrayBuffer;
}

function publicOf(curve: Curve, secret: Uint8Array): Uint8Array {
  return curve === "X25519" ? x25519.getPublicKey(secret) : ed25519.getPublicKey(secret);
}

function b64u(data: Uint8Array): string {
  let binary = "";
  for (const byte of data) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function hex(text: string): Uint8Array {
  return Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));
}

function same(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

type AesGcm = { name: string; iv?: unknown; additionalData?: unknown; tagLength?: number };

function aesGcm(algorithm: AesGcm, made: CryptoKey, usage: "encrypt" | "decrypt") {
  if (nameOf(algorithm) !== "AES-GCM") throw fail("NotSupportedError", `${nameOf(algorithm)} is not used here`);
  if ((algorithm.tagLength ?? 128) !== 128) throw fail("NotSupportedError", "AES-GCM here has a 128-bit tag");
  const aes = bytesOf(made, "AES-GCM", "secret", usage);
  return { aes, iv: input(algorithm.iv), aad: algorithm.additionalData === undefined ? undefined : input(algorithm.additionalData) };
}

/** WebCrypto's subset, with `random` for new keys. */
export function createSubtle(
  random: (length: number) => Uint8Array,
): Pick<SubtleCrypto, "generateKey" | "importKey" | "exportKey" | "deriveBits" | "sign" | "verify" | "decrypt" | "encrypt" | "digest"> {
  const subtle = {
    async generateKey(algorithm: unknown, extractable: boolean, usages: readonly string[]) {
      const curve = nameOf(algorithm);
      if (curve !== "X25519" && curve !== "Ed25519") throw fail("NotSupportedError", `${curve} keys are not generated here`);
      const secret = random(KEY_LEN);
      if (secret.length !== KEY_LEN) throw fail("OperationError", "the random source gave the wrong length");
      const publicUsages = curve === "Ed25519" ? ["verify"] : [];
      return { publicKey: key("public", curve, publicOf(curve, secret), true, publicUsages), privateKey: key("private", curve, secret, extractable, usages) };
    },

    async importKey(format: string, data: unknown, algorithm: unknown, extractable: boolean, usages: readonly string[]) {
      const name = nameOf(algorithm);
      const raw = input(data);
      if (format === "raw" && (name === "X25519" || name === "Ed25519")) {
        if (raw.length !== KEY_LEN) throw fail("DataError", `a ${name} public key is ${KEY_LEN} bytes`);
        return key("public", name, raw, extractable, usages);
      }
      if (format === "pkcs8" && (name === "X25519" || name === "Ed25519")) {
        const prefix = PKCS8_PREFIX[name];
        if (raw.length !== prefix.length + KEY_LEN || !same(raw.subarray(0, prefix.length), prefix)) throw fail("DataError", `not a ${name} PKCS #8 key`);
        return key("private", name, raw.subarray(prefix.length), extractable, usages);
      }
      if (format === "raw" && name === "AES-GCM") {
        if (![16, 24, 32].includes(raw.length)) throw fail("DataError", "an AES key is 16, 24 or 32 bytes");
        return key("secret", name, raw, extractable, usages);
      }
      if (format === "raw" && name === "HMAC") {
        if (nameOf((algorithm as { hash?: unknown }).hash) !== "SHA-256") throw fail("NotSupportedError", "HMAC here is HMAC-SHA-256");
        return key("secret", name, raw, extractable, usages);
      }
      throw fail("NotSupportedError", `${format} ${name} keys are not imported here`);
    },

    async exportKey(format: string, made: CryptoKey) {
      const meta = made as unknown as Key;
      const name = meta.algorithm?.name;
      if (!meta.extractable) throw fail("InvalidAccessError", "the key is not extractable");
      if (name !== "X25519" && name !== "Ed25519") throw fail("NotSupportedError", `${name} keys are not exported here`);
      const bytes = bytesOf(made, name, meta.type);
      if (format === "raw" && meta.type === "public") return output(bytes);
      if (format === "jwk") {
        const x = meta.type === "public" ? bytes : publicOf(name, bytes);
        return { kty: "OKP", crv: name, x: b64u(x), ...(meta.type === "private" ? { d: b64u(bytes) } : {}), ext: true, key_ops: [...meta.usages] };
      }
      throw fail("NotSupportedError", `${format} export of a ${meta.type} ${name} key is not done here`);
    },

    async deriveBits(algorithm: { name: string; public?: CryptoKey }, made: CryptoKey, length: number) {
      if (nameOf(algorithm) !== "X25519") throw fail("NotSupportedError", `${nameOf(algorithm)} bits are not derived here`);
      if (length !== 256) throw fail("OperationError", "X25519 derives 256 bits");
      const secret = bytesOf(made, "X25519", "private", "deriveBits");
      const peer = bytesOf(algorithm.public, "X25519", "public");
      let shared: Uint8Array;
      try {
        shared = x25519.getSharedSecret(secret, peer);
      } catch {
        throw fail("OperationError", "the X25519 key agreement failed");
      }
      // A small-order public key gives all zeros; WebCrypto refuses it, and so does this.
      if (shared.every((byte) => byte === 0)) throw fail("OperationError", "the X25519 key agreement failed");
      return output(shared);
    },

    async sign(algorithm: unknown, made: CryptoKey, data: unknown) {
      const name = nameOf(algorithm);
      if (name === "Ed25519") return output(ed25519.sign(input(data), bytesOf(made, "Ed25519", "private", "sign")));
      if (name === "HMAC") return output(hmac(sha256, bytesOf(made, "HMAC", "secret", "sign"), input(data)));
      throw fail("NotSupportedError", `${name} signatures are not made here`);
    },

    async verify(algorithm: unknown, made: CryptoKey, signature: unknown, data: unknown) {
      if (nameOf(algorithm) !== "Ed25519") throw fail("NotSupportedError", `${nameOf(algorithm)} signatures are not checked here`);
      const publicKey = bytesOf(made, "Ed25519", "public", "verify");
      try {
        // RFC 8032's strict check, as WebCrypto's: no ZIP-215 leniency.
        return ed25519.verify(input(signature), input(data), publicKey, { zip215: false });
      } catch {
        return false;
      }
    },

    async decrypt(algorithm: AesGcm, made: CryptoKey, data: unknown) {
      const { aes, iv, aad } = aesGcm(algorithm, made, "decrypt");
      try {
        return output(gcm(aes, iv, aad).decrypt(input(data)));
      } catch {
        throw fail("OperationError", "the data did not decrypt");
      }
    },

    // The kit's test fixtures seal events with it (`synced-fixture.ts`); the app only opens them.
    async encrypt(algorithm: AesGcm, made: CryptoKey, data: unknown) {
      const { aes, iv, aad } = aesGcm(algorithm, made, "encrypt");
      return output(gcm(aes, iv, aad).encrypt(input(data)));
    },

    async digest(algorithm: unknown, data: unknown) {
      if (nameOf(algorithm) !== "SHA-256") throw fail("NotSupportedError", `${nameOf(algorithm)} is not computed here`);
      return output(sha256(input(data)));
    },
  };
  return subtle as unknown as ReturnType<typeof createSubtle>;
}

/**
 * Puts the subset on `globalThis.crypto.subtle` when the runtime has none, keeping a native one (the web build). Call
 * once, before the kit's e2e code runs.
 */
export function installSubtle(random: (length: number) => Uint8Array): void {
  const scope = globalThis as { crypto?: Partial<Crypto> };
  if (scope.crypto?.subtle) return;
  const crypto = scope.crypto ?? {};
  Object.defineProperty(crypto, "subtle", { value: createSubtle(random), configurable: true });
  if (!crypto.getRandomValues) {
    Object.defineProperty(crypto, "getRandomValues", {
      value: <T extends ArrayBufferView | null>(array: T): T => {
        if (array) new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(random(array.byteLength));
        return array;
      },
      configurable: true,
    });
  }
  if (!scope.crypto) Object.defineProperty(globalThis, "crypto", { value: crypto, configurable: true });
}
