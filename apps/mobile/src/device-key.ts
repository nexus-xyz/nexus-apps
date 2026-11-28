import { b64u, type DeviceKey, DEVICE_SECRET_LEN, fromB64u, importDeviceSecret } from "@nexus/kit/e2e";
import * as Crypto from "expo-crypto";
import * as SecureStore from "expo-secure-store";
import { forgetDeviceTrust } from "./device-trust";
import { DEVELOPMENT_NAMESPACE } from "./development";

/**
 * This phone's device key for synced sessions (eng-architecture.md §4.3): its X25519 private key and its
 * Ed25519 seed, 64 bytes, in the Keychain or the Android Keystore, as the sign-in token is: after first unlock, this
 * device only, never in a backup. It is made here and never leaves the phone; Gateway holds its public halves.
 *
 * It outlives an expired sign-in, so signing in again finds the phone still paired. Signing out removes it.
 */
const KEY = `nexus.device-key.v1${DEVELOPMENT_NAMESPACE}`;
const STORE = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY };

let loaded: Promise<DeviceKey | undefined> | undefined;

/** The stored key, read once per launch; `undefined` when this phone has none. */
export function loadDeviceKey(): Promise<DeviceKey | undefined> {
  loaded ??= SecureStore.getItemAsync(KEY, STORE).then((stored) => {
    const secret = stored ? fromB64u(stored) : undefined;
    return secret?.length === DEVICE_SECRET_LEN ? importDeviceSecret(secret) : undefined;
  });
  loaded.catch(() => (loaded = undefined));
  return loaded;
}

/** A new key, stored before it is returned. It replaces any key this phone had. */
export async function createDeviceKey(): Promise<DeviceKey> {
  const secret = Crypto.getRandomBytes(DEVICE_SECRET_LEN);
  try {
    await SecureStore.setItemAsync(KEY, b64u(secret), STORE);
    const key = await importDeviceSecret(secret);
    loaded = Promise.resolve(key);
    return key;
  } finally {
    secret.fill(0);
  }
}

/** Removes the key and what it remembered of trusted devices: the phone reads no synced session until it pairs again. */
export async function forgetDeviceKey(): Promise<void> {
  loaded = Promise.resolve(undefined);
  await SecureStore.deleteItemAsync(KEY, STORE);
  forgetDeviceTrust();
}
