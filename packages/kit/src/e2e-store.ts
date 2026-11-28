/**
 * Where this device's keys live (eng-architecture.md §4.3). In the browser, IndexedDB keeps the `CryptoKey`s, so the
 * private keys stay unexportable at rest. WebKit stores an X25519 `CryptoKey` but reads it back as `null`, and the
 * whole record with it, so Safari lost its key at every load after the first. The record therefore keeps the X25519
 * key wrapped by an AES-KW key, which every engine reads back; WebCrypto unwraps it without the bytes reaching script.
 * The store is an interface, so tests and other platforms supply their own.
 */
import type { DeviceTrustMemory, DeviceTrustStore } from "./device-trust";
import { type Bytes, type DeviceKey, generateDeviceKey } from "./e2e";

export interface DeviceKeyStore {
  load(): Promise<DeviceKey | undefined>;
  /** Stores `device` unless a device is stored already, and returns the stored one. The browser's store wraps `device.x25519`, so it must be extractable. */
  saveIfAbsent(device: DeviceKey): Promise<DeviceKey>;
  clear(): Promise<void>;
}

/** This device's keys: the stored ones, or new ones made and stored now. Two tabs that race end with one device. */
export async function loadOrCreateDeviceKey(store: DeviceKeyStore): Promise<DeviceKey> {
  return (await store.load()) ?? store.saveIfAbsent(await generateDeviceKey(true));
}

const STORE = "device";
const SELF = "self";

/** The record: the X25519 key wrapped, the rest as it is. A record without `wrapKey` holds the X25519 `CryptoKey` itself, as before. */
type Stored = DeviceKey | { encryptionKey: Bytes; signingKey: Bytes; ed25519: CryptoKey; wrapKey: CryptoKey; x25519Wrapped: Bytes };

/** The record for `device`: its X25519 key wrapped by a new AES-KW key that cannot be exported. */
export async function sealDeviceKey(device: DeviceKey): Promise<Stored> {
  const subtle = globalThis.crypto.subtle;
  const wrapKey = await subtle.generateKey({ name: "AES-KW", length: 256 }, false, ["wrapKey", "unwrapKey"]);
  const x25519Wrapped = new Uint8Array(await subtle.wrapKey("pkcs8", device.x25519, wrapKey, "AES-KW"));
  return { encryptionKey: device.encryptionKey, signingKey: device.signingKey, ed25519: device.ed25519, wrapKey, x25519Wrapped };
}

/** The key in a record: a wrapped X25519 key unwrapped into one that cannot be exported, an older record as it is. */
export async function unsealDeviceKey(stored: Stored): Promise<DeviceKey> {
  if (!("wrapKey" in stored)) return stored;
  const x25519 = await globalThis.crypto.subtle.unwrapKey("pkcs8", stored.x25519Wrapped, stored.wrapKey, "AES-KW", { name: "X25519" }, false, ["deriveBits"]);
  return { encryptionKey: stored.encryptionKey, x25519, signingKey: stored.signingKey, ed25519: stored.ed25519 };
}

/** The browser's store: one record in IndexedDB database `name`. */
export function indexedDbDeviceKeyStore(name = "nexus-e2e"): DeviceKeyStore {
  return {
    async load() {
      // `null` is a record WebKit cannot read back: no key.
      const stored = await transact<Stored | null | undefined>(name, "readonly", (store, done) => {
        const get = store.get(SELF);
        get.onsuccess = () => done(get.result as Stored | null | undefined);
      });
      return stored ? unsealDeviceKey(stored) : undefined;
    },
    async saveIfAbsent(device) {
      const sealed = await sealDeviceKey(device);
      const stored = await transact<Stored>(name, "readwrite", (store, done) => {
        const get = store.get(SELF);
        get.onsuccess = () => {
          if (get.result) return done(get.result as Stored);
          // `put`, not `add`: a record WebKit cannot read back holds a lost key, and the new one replaces it.
          store.put(sealed, SELF);
          done(sealed);
        };
      });
      return unsealDeviceKey(stored);
    },
    clear: () => transact<void>(name, "readwrite", (store, done) => {
      store.delete(SELF);
      done();
    }),
  };
}

/**
 * Adds what a trust check saw to what this device remembers. A revocation is never forgotten. A verified
 * holder keeps the keys it was first verified with, so a later listing with other keys does not replace them.
 */
export function rememberDeviceTrust(previous: DeviceTrustMemory | undefined, seen: DeviceTrustMemory): DeviceTrustMemory {
  return {
    revoked: [...new Set([...(previous?.revoked ?? []), ...seen.revoked])],
    verified: { ...seen.verified, ...previous?.verified },
  };
}

/**
 * The browser's trust memory, next to the device key: one record per device in IndexedDB database `name`. It holds
 * ids and public keys only. Each call merges in one transaction, so two tabs never lose a revocation.
 */
export function indexedDbDeviceTrust(name = "nexus-device-trust"): DeviceTrustStore {
  return {
    remember: (scope, seen) =>
      transact<DeviceTrustMemory>(name, "readwrite", (store, done) => {
        const get = store.get(scope);
        get.onsuccess = () => {
          const next = rememberDeviceTrust(get.result as DeviceTrustMemory | undefined, seen);
          store.put(next, scope);
          done(next);
        };
      }),
  };
}

/** Runs `body` in one transaction and resolves with what it passed to `done` once the transaction commits. */
function transact<T>(
  name: string,
  mode: IDBTransactionMode,
  body: (store: IDBObjectStore, done: (value: T) => void) => void,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const open = globalThis.indexedDB.open(name, 1);
    open.onupgradeneeded = () => open.result.createObjectStore(STORE);
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const db = open.result;
      const tx = db.transaction(STORE, mode);
      let value: T;
      tx.oncomplete = () => {
        db.close();
        resolve(value);
      };
      // A failed request aborts the transaction. Only `abort` has `tx.error` set; in `error` it is still null.
      tx.onabort = () => {
        db.close();
        reject(tx.error ?? new Error("IndexedDB aborted the transaction"));
      };
      body(tx.objectStore(STORE), (result) => {
        value = result;
      });
    };
  });
}
