/**
 * This phone's device when Gateway vouches for the person's devices: the list's `trust` is `gateway`.
 * The phone registers its key right after sign-in, with no screen, and Gateway trusts it at once: no QR code, no
 * code and no approval. With `approvals`, or no `trust`, nothing here registers anything.
 *
 * No React Native here, so vitest runs it. `use-gateway-device.ts` wires the phone's key store and name to it.
 */
import type { CoreClient, DeviceList, E2eDevice } from "@nexus/kit";
import { deviceTrustMode, type DeviceTrustMode } from "@nexus/kit/device-trust";
import { b64u, type DeviceKey } from "@nexus/kit/e2e";
import { deviceStatus, thisDevice } from "@nexus/kit/pairing";

/** Where the phone keeps its device key: `device-key.ts` on the phone, a fake in tests. */
export interface PhoneKeys {
  load(): Promise<DeviceKey | undefined>;
  create(): Promise<DeviceKey>;
  forget(): Promise<void>;
}

/** The devices list as this phone read it, its mode, this phone's key, and its row. */
export interface PhoneDevice {
  list: DeviceList;
  trust: DeviceTrustMode;
  holder?: DeviceKey;
  self?: E2eDevice;
}

/**
 * Whether this phone registers now: only with `gateway`, when Gateway lists no row with its key, or a row left
 * pending from before Gateway vouched. A revoked phone waits: the person revoked it, and only they set it up again.
 */
export function needsGatewayRegistration(trust: DeviceTrustMode, self: E2eDevice | undefined): boolean {
  if (trust !== "gateway") return false;
  return !self || deviceStatus(self, Date.now(), "gateway") === "expired";
}

const running = new WeakMap<object, Promise<PhoneDevice>>();

/**
 * Reads the devices list and, with `gateway`, registers this phone when {@link needsGatewayRegistration}. One run at a
 * time for each `core`, so two screens never make two keys. A pending row's key stays pending, so a new key
 * replaces it.
 */
export function ensureGatewayDevice(
  core: Pick<CoreClient, "e2eDevices" | "registerE2eDevice">,
  keys: PhoneKeys,
  name: string,
  signal?: () => AbortSignal,
): Promise<PhoneDevice> {
  let work = running.get(core);
  if (!work) {
    work = register(core, keys, name, signal).finally(() => running.delete(core));
    running.set(core, work);
  }
  return work;
}

async function register(
  core: Pick<CoreClient, "e2eDevices" | "registerE2eDevice">,
  keys: PhoneKeys,
  name: string,
  signal?: () => AbortSignal,
): Promise<PhoneDevice> {
  const list = await core.e2eDevices(signal?.());
  const trust = deviceTrustMode(list);
  let holder = await keys.load();
  const self = holder ? thisDevice(list.devices, holder) : undefined;
  if (!needsGatewayRegistration(trust, self)) return { list, trust, ...(holder ? { holder } : {}), ...(self ? { self } : {}) };
  if (!holder || self) {
    if (self) await keys.forget();
    holder = await keys.create();
  }
  const registered = await core.registerE2eDevice(
    { name, encryptionKey: b64u(holder.encryptionKey), signingKey: b64u(holder.signingKey) },
    signal?.(),
  );
  const devices = [...list.devices.filter((device) => device.id !== registered.id), registered];
  return { list: { ...list, devices }, trust, holder, self: registered };
}
