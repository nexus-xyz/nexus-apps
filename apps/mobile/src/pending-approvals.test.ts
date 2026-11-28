import type { E2eDevice } from "@nexus/kit";
import { b64u, generateDeviceKey, type DeviceKey } from "@nexus/kit/e2e";
import { deviceFingerprint, PAIRING_TTL_MS, readPairingInput } from "@nexus/kit/pairing";
import { expect, it } from "vitest";
import { pendingApprovals } from "./pending-approvals";

// A trusted phone lists the computers it may approve now.

const NOW = Date.parse("2026-11-25T12:00:00Z");
const at = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000).toISOString();

function row(id: string, key: DeviceKey, extra: Partial<E2eDevice> = {}): E2eDevice {
  return {
    id, name: id, kind: "device", state: "pending", createdAt: at(1),
    encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey),
    approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
    ...extra,
  };
}

it("lists each waiting computer, newest first, with the text that opens its approval", async () => {
  const [phoneKey, laptopKey, studioKey, oldKey, recoveryKey] = await Promise.all(
    Array.from({ length: 5 }, () => generateDeviceKey()),
  );
  const phone = row("dev_phone", phoneKey, { state: "trusted", approvedAt: at(600) });
  const laptop = row("dev_laptop", laptopKey, { name: "MacBook-Pro", createdAt: at(5) });
  const studio = row("dev_studio", studioKey, { name: "MacBook-Pro", createdAt: at(1) });
  const expired = row("dev_old", oldKey, { createdAt: new Date(NOW - PAIRING_TTL_MS).toISOString() });
  const recovery = row("rk_code", recoveryKey, { kind: "recovery_code" });

  const listed = await pendingApprovals([phone, laptop, expired, studio, recovery], phoneKey, NOW);

  // Two computers may share a name: each is its own row, and its code tells them apart.
  expect(listed.map((device) => device.id)).toEqual(["dev_studio", "dev_laptop"]);
  const input = readPairingInput(listed[1]!.approval);
  expect(input.deviceId).toBe("dev_laptop");
  expect(input.code).toBe((await deviceFingerprint(laptop)).code);
});

it("lists nothing on a phone that is not trusted, or has no key", async () => {
  const [phoneKey, laptopKey] = await Promise.all([generateDeviceKey(), generateDeviceKey()]);
  const laptop = row("dev_laptop", laptopKey);
  expect(await pendingApprovals([row("dev_phone", phoneKey), laptop], phoneKey, NOW)).toEqual([]);
  expect(await pendingApprovals([laptop], null, NOW)).toEqual([]);
});

it("lists nothing when Gateway vouches for the person's devices", async () => {
  const [phoneKey, laptopKey] = await Promise.all([generateDeviceKey(), generateDeviceKey()]);
  const devices = [row("dev_phone", phoneKey, { state: "trusted", approvedAt: at(600) }), row("dev_laptop", laptopKey)];
  expect(await pendingApprovals(devices, phoneKey, NOW, "gateway")).toEqual([]);
  expect((await pendingApprovals(devices, phoneKey, NOW, "approvals")).map((device) => device.id)).toEqual(["dev_laptop"]);
});
