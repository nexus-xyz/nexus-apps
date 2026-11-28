import { type DeviceTrustMemory, type DeviceTrustStore, rememberDeviceTrust, setDeviceTrustStore } from "@nexus/kit/device-trust";
import { Storage } from "./store";

/**
 * What this phone remembers of its trusted devices (`@nexus/kit/device-trust`): every revoked id it saw,
 * and every device it verified, with its public keys. One key per device key, `device-trust:<scope>`, in the app's
 * store. It holds no secret. It outlives an expired sign-in, as the device key does (`forgetAll` keeps the prefix),
 * and goes with the device key (`forgetDeviceKey`).
 */
const PREFIX = "device-trust:";

export const phoneDeviceTrust: DeviceTrustStore = {
  // Synchronous read and write: no other check can run between them, so none loses a revocation.
  async remember(scope, seen) {
    const key = PREFIX + scope;
    const raw = Storage.getItemSync(key);
    const next = rememberDeviceTrust(raw ? (JSON.parse(raw) as DeviceTrustMemory) : undefined, seen);
    Storage.setItemSync(key, JSON.stringify(next));
    return next;
  },
};

/** Makes the kit's trust checks use this phone's store. The root layout calls it once, before any screen. */
export function installDeviceTrust(): void {
  setDeviceTrustStore(phoneDeviceTrust);
}

/** Removes what this phone remembers, with its device key: a new key starts with no memory. */
export function forgetDeviceTrust(): void {
  try {
    for (const key of Storage.getAllKeysSync()) if (key.startsWith(PREFIX)) Storage.removeItemSync(key);
  } catch {
    // A memory that stays is the old key's: a new key's scope never reads it.
  }
}
