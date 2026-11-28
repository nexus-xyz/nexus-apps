import type { E2eDevice } from "@nexus/kit";
import type { DeviceTrustMode } from "@nexus/kit/device-trust";
import { deviceFingerprint, deviceStatus, holderKind, pairingQrText, thisDevice } from "@nexus/kit/pairing";

/** A computer this phone may approve now, and the text that opens Approve computer for it. */
export type PendingApproval = { id: string; name: string; createdAt: string; approval: string };

/**
 * The devices this phone may approve now: waiting within the pairing window, a device and not a recovery
 * key, and not this phone. Newest first. None unless this phone is trusted, because only a trusted device approves.
 *
 * The approval text carries the keys Gateway lists, so it proves nothing by itself: Approve computer shows the
 * device's code, and the person compares it with the computer's screen before they approve.
 *
 * None with `trust` `gateway`: Gateway trusts each device at once, and nothing waits for an approval.
 */
export async function pendingApprovals(
  devices: E2eDevice[],
  holder: Parameters<typeof thisDevice>[1] | null | undefined,
  now = Date.now(),
  trust: DeviceTrustMode = "approvals",
): Promise<PendingApproval[]> {
  const phone = holder ? thisDevice(devices, holder) : undefined;
  if (trust === "gateway" || phone?.state !== "trusted") return [];
  const waiting = devices
    .filter((device) => device.id !== phone.id && holderKind(device) === "device" && deviceStatus(device, now) === "pending")
    .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  return Promise.all(waiting.map(async (device) => ({
    id: device.id,
    name: device.name,
    createdAt: device.createdAt,
    approval: pairingQrText(device.id, (await deviceFingerprint(device)).fingerprint),
  })));
}
