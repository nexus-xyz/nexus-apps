import type { DeviceList, E2eDevice, RegisterDeviceRequest } from "@nexus/kit";
import { b64u, generateDeviceKey, type DeviceKey } from "@nexus/kit/e2e";
import { describe, expect, it, vi } from "vitest";
import { ensureGatewayDevice, needsGatewayRegistration, type PhoneKeys } from "./gateway-device";

// With `trust` `gateway`, the phone registers at sign-in and Gateway trusts it at once.

function row(id: string, key: DeviceKey, state: E2eDevice["state"] = "trusted"): E2eDevice {
  return {
    id, name: id, kind: "device", state, createdAt: "2026-11-25T12:00:00Z",
    encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey),
    approvedAt: state === "pending" ? null : "2026-11-25T12:00:00Z", approvedBy: null, approvalSignature: null,
    revokedAt: state === "revoked" ? "2026-11-25T13:00:00Z" : null,
  };
}

/** A key store in memory, and a Gateway whose list is `list`. Registration lists the key as trusted. */
async function phone(held: DeviceKey | undefined, list: DeviceList) {
  let key = held;
  const keys: PhoneKeys = {
    load: vi.fn(async () => key),
    create: vi.fn(async () => (key = await generateDeviceKey())),
    forget: vi.fn(async () => { key = undefined; }),
  };
  const core = {
    e2eDevices: vi.fn(async () => list),
    registerE2eDevice: vi.fn(async (body: RegisterDeviceRequest) => ({ ...row("dev_phone_new", key!), name: body.name })),
  };
  return { core, keys, key: () => key };
}

describe("this phone's device when Gateway vouches", () => {
  it("registers a new key after sign-in, trusted at once, with no screen", async () => {
    const computer = row("dev_computer", await generateDeviceKey());
    const { core, keys, key } = await phone(undefined, { devices: [computer], trust: "gateway" } as DeviceList);
    const device = await ensureGatewayDevice(core, keys, "iPhone");
    expect(core.registerE2eDevice).toHaveBeenCalledWith({ name: "iPhone", encryptionKey: b64u(key()!.encryptionKey), signingKey: b64u(key()!.signingKey) }, undefined);
    expect(device.trust).toBe("gateway");
    expect(device.self?.state).toBe("trusted");
    expect(device.list.devices.map((entry) => entry.id)).toEqual(["dev_computer", "dev_phone_new"]);
  });

  it("registers nothing in approvals mode, or when the list has no trust", async () => {
    for (const list of [{ devices: [], trust: "approvals" }, { devices: [] }] as DeviceList[]) {
      const { core, keys } = await phone(undefined, list);
      expect((await ensureGatewayDevice(core, keys, "iPhone")).trust).toBe("approvals");
      expect(core.registerE2eDevice).not.toHaveBeenCalled();
      expect(keys.create).not.toHaveBeenCalled();
    }
  });

  it("keeps a trusted phone, waits on a revoked one, and replaces a pending row's key", async () => {
    const held = await generateDeviceKey();
    for (const state of ["trusted", "revoked"] as const) {
      const { core } = await phone(held, { devices: [row("dev_phone", held, state)], trust: "gateway" } as DeviceList);
      const keys = { load: async () => held, create: vi.fn(), forget: vi.fn() };
      expect((await ensureGatewayDevice(core, keys, "iPhone")).self?.state).toBe(state);
      expect(core.registerE2eDevice).not.toHaveBeenCalled();
    }
    const { core, keys, key } = await phone(held, { devices: [row("dev_phone", held, "pending")], trust: "gateway" } as DeviceList);
    await ensureGatewayDevice(core, keys, "iPhone");
    expect(keys.forget).toHaveBeenCalled();
    expect(key()).not.toBe(held);
    expect(core.registerE2eDevice).toHaveBeenCalledTimes(1);
  });

  it("registers once when two screens ask at the same time", async () => {
    const { core, keys } = await phone(undefined, { devices: [], trust: "gateway" } as DeviceList);
    await Promise.all([ensureGatewayDevice(core, keys, "iPhone"), ensureGatewayDevice(core, keys, "iPhone")]);
    expect(keys.create).toHaveBeenCalledTimes(1);
    expect(core.registerE2eDevice).toHaveBeenCalledTimes(1);
  });

  it("needs a registration only in gateway mode, for no row or a pending one", async () => {
    const key = await generateDeviceKey();
    expect(needsGatewayRegistration("approvals", undefined)).toBe(false);
    expect(needsGatewayRegistration("gateway", undefined)).toBe(true);
    expect(needsGatewayRegistration("gateway", row("dev_p", key, "pending"))).toBe(true);
    expect(needsGatewayRegistration("gateway", row("dev_p", key, "trusted"))).toBe(false);
    expect(needsGatewayRegistration("gateway", row("dev_p", key, "revoked"))).toBe(false);
  });
});
