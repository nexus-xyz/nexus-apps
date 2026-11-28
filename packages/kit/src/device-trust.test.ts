import { describe, expect, it } from "vitest";
import type { E2eDevice } from "./api";
import { DeviceTrustError, confirmDeviceApprover, deviceTrustMode, handoverOf, memoryDeviceTrust, rememberDeviceTrust, trustedDevices } from "./device-trust";
import { approvalMessage, b64u, type DeviceKey, generateDeviceKey, handoverMessage, sealHandover, sign } from "./e2e";
import { deviceFingerprint } from "./pairing";
import { type Handover, handoverPayload, readHandoverPayload, sealHandoverFor } from "./handover";

/** A listed device, approved by `approver` over its keys, or a root with none. The runner's tests (`trust.rs`) mirror these. */
async function row(id: string, key: DeviceKey, approver?: { id: string; key: DeviceKey }, state: E2eDevice["state"] = "trusted"): Promise<E2eDevice> {
  const signature = approver && (await sign(approver.key, approvalMessage(id, key.encryptionKey, key.signingKey)));
  return {
    id, name: id, kind: "device", state, encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey),
    approvedBy: approver?.id ?? null, approvalSignature: signature ? b64u(signature) : null,
    createdAt: "2026-11-01T00:00:00Z", approvedAt: approver ? "2026-11-01T00:01:00Z" : null, revokedAt: state === "revoked" ? "2026-11-02T00:00:00Z" : null,
  };
}

const keys = async () => Promise.all([1, 2, 3, 4].map(() => generateDeviceKey()));
const me = (id: string, key: DeviceKey) => ({ deviceId: id, encryptionKey: key.encryptionKey, signingKey: key.signingKey });
const ids = (set: Set<string>) => [...set].sort();

describe("device trust: the runner's check, in the kit", () => {
  it("does not trust an approver or its ancestors without local confirmation", async () => {
    const [f, d, p] = await keys();
    const list = [await row("dev_f", f!), await row("dev_d", d!, { id: "dev_f", key: f! }), await row("dev_p", p!, { id: "dev_d", key: d! })];
    const trust = await trustedDevices(list, me("dev_p", p!), memoryDeviceTrust());
    expect(ids(trust.trusted)).toEqual(["dev_p"]);
    expect(handoverOf(list, trust)?.holders.map((holder) => holder.id)).toEqual(undefined);
  });

  it("confirms only D and takes ancestors solely from D's signed handover", async () => {
    const [a, d, p, f] = await keys();
    const approver = await row("dev_d", d!, { id: "dev_f", key: f! });
    const phone = await row("dev_p", p!, { id: "dev_d", key: d! });
    const list = [await row("dev_a", a!), approver, phone, await row("dev_f", f!)];
    const code = (await deviceFingerprint(approver)).code;
    const store = memoryDeviceTrust();
    await expect(confirmDeviceApprover(list, reader("dev_p", p!), (await deviceFingerprint(list[3]!)).code, store)).rejects.toThrow();
    const trust = await confirmDeviceApprover(list, reader("dev_p", p!), code, store);
    expect(ids(trust.trusted)).toEqual(["dev_d", "dev_p"]);
    expect(handoverOf(list, trust)?.holders.map((holder) => holder.id)).toEqual(["dev_d", "dev_p"]);
    const signed = await handed(phone, "dev_d", d!, { holders: [holder("dev_a", a!)], revoked: [] });
    expect(ids((await trustedDevices(list.map((row) => row.id === phone.id ? signed : row), reader("dev_p", p!), store)).trusted)).toEqual(["dev_a", "dev_d", "dev_p"]);
  });

  it("refuses duplicate ids and discards legacy automatic trust while preserving revocations", async () => {
    const [a, b, f] = await keys();
    const list = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! })];
    const store = memoryDeviceTrust();
    await store.remember(JSON.stringify(["dev_b", b64u(b!.signingKey)]), { verified: { dev_a: holder("dev_a", a!) }, revoked: ["dev_x"] });
    const clean = await trustedDevices(list, me("dev_b", b!), store);
    expect(ids(clean.trusted)).toEqual(["dev_b"]);
    expect([...clean.revoked]).toEqual(["dev_x"]);
    await expect(trustedDevices([await row("dev_a", f!), ...list], me("dev_b", b!), store)).rejects.toThrow(DeviceTrustError);
  });

  it("trusts a root and the devices it approved, from the root and from the newest device", async () => {
    const [a, b, c] = await keys();
    const list = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_c", c!, { id: "dev_b", key: b! })];
    expect(ids((await trustedDevices(list, me("dev_a", a!), memoryDeviceTrust())).trusted)).toEqual(["dev_a", "dev_b", "dev_c"]);
    const store = memoryDeviceTrust();
    await confirmDeviceApprover(list, me("dev_c", c!), (await deviceFingerprint(list[1]!)).code, store);
    expect(ids((await trustedDevices(list, me("dev_c", c!), store)).trusted)).toEqual(["dev_b", "dev_c"]);
  });

  it("refuses this device when Gateway does not list it trusted with its keys, or its own chain is forged", async () => {
    const [a, b, mallory] = await keys();
    const refused = (kind: DeviceTrustError["kind"]) => (error: unknown) => error instanceof DeviceTrustError && error.kind === kind;
    const store = memoryDeviceTrust();
    await expect(trustedDevices([await row("dev_a", a!)], me("dev_b", b!), store)).rejects.toSatisfy(refused("not-trusted"));
    await expect(trustedDevices([await row("dev_a", a!), await row("dev_b", b!, undefined, "pending")], me("dev_b", b!), store)).rejects.toSatisfy(refused("not-trusted"));
    await expect(trustedDevices([await row("dev_a", a!), await row("dev_b", mallory!, { id: "dev_a", key: a! })], me("dev_b", b!), store)).rejects.toSatisfy(refused("not-trusted"));
    const forged = { ...(await row("dev_b", b!, { id: "dev_m", key: mallory! })), approvedBy: "dev_a" };
    await expect(confirmDeviceApprover([await row("dev_a", a!), forged], me("dev_b", b!), (await deviceFingerprint(await row("dev_a", a!))).code, store)).rejects.toSatisfy(refused("chain-broken"));
  });

  it("anchors only on the first check: a later approver Gateway names for this device is not followed", async () => {
    const [a, b, f, r] = await keys();
    const store = memoryDeviceTrust();
    await confirmDeviceApprover([await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! })], me("dev_b", b!), (await deviceFingerprint(await row("dev_a", a!))).code, store);
    const rewritten = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_f", key: f! }), await row("dev_f", f!), await row("dev_r", r!, { id: "dev_f", key: f! })];
    expect(ids((await trustedDevices(rewritten, me("dev_b", b!), store)).trusted)).toEqual(["dev_a", "dev_b"]);
  });

  it("keeps an approval after its approver is revoked, for a device verified before", async () => {
    const [a, b, c] = await keys();
    const store = memoryDeviceTrust();
    const list = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_c", c!, { id: "dev_b", key: b! })];
    await trustedDevices(list, me("dev_a", a!), store);
    const revoked = [list[0]!, await row("dev_b", b!, { id: "dev_a", key: a! }, "revoked"), list[2]!];
    const trust = await trustedDevices(revoked, me("dev_a", a!), store);
    expect(ids(trust.trusted)).toEqual(["dev_a", "dev_c"]);
    expect(ids(trust.verified)).toEqual(["dev_a", "dev_b", "dev_c"]);
  });

  it("keeps the keys a device was first verified with, and every revocation", async () => {
    const [a, b, c] = await keys();
    const store = memoryDeviceTrust();
    await confirmDeviceApprover([await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! })], me("dev_b", b!), (await deviceFingerprint(await row("dev_a", a!))).code, store);
    // The approver's id listed with other keys, which signed for a new device: neither is trusted.
    const swapped = [await row("dev_a", c!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_c", c!, { id: "dev_a", key: c! })];
    expect(ids((await trustedDevices(swapped, me("dev_b", b!), store)).trusted)).toEqual(["dev_b"]);
    const merged = rememberDeviceTrust({ revoked: ["dev_x"], verified: { dev_b: { encryptionKey: "old", signingKey: "old" } } }, { revoked: ["dev_y"], verified: { dev_b: { encryptionKey: "new", signingKey: "new" } } });
    expect(merged).toEqual({ revoked: ["dev_x", "dev_y"], verified: { dev_b: { encryptionKey: "old", signingKey: "old" } } });
  });
});

/** `device` with a handover signed by `signer` as `approverId`, sealed to it: what Gateway lists after the approval. */
async function handed(device: E2eDevice, approverId: string, signer: DeviceKey, handover: Handover): Promise<E2eDevice> {
  const sealed = await sealHandoverFor({ signHandover: (d, a, p) => sign(signer, handoverMessage(d, a, p)) }, approverId, device, handover);
  return { ...device, handover: sealed ?? null };
}

const holder = (id: string, key: DeviceKey) => ({ id, encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey) });
const reader = (id: string, key: DeviceKey) => ({ ...me(id, key), holder: key });

describe("device trust: the approver's handover", () => {
  it("rejects a legacy handover even after its signer is confirmed locally", async () => {
    const [f, d, p] = await keys();
    const approver = await row("dev_d", d!, { id: "dev_f", key: f! });
    const phone = await row("dev_p", p!, { id: "dev_d", key: d! });
    const payload = new TextEncoder().encode(JSON.stringify({ version: 1, holders: [holder("dev_f", f!)], revoked: [] }));
    const signature = await sign(d!, handoverMessage(phone.id, approver.id, payload));
    const sealed = await sealHandover(phone.id, approver.id, p!.encryptionKey, signature, payload);
    const list = [await row("dev_f", f!), approver, { ...phone, handover: b64u(sealed) }];
    const trust = await confirmDeviceApprover(list, reader(phone.id, p!), (await deviceFingerprint(approver)).code, memoryDeviceTrust());
    expect(ids(trust.trusted)).toEqual(["dev_d", "dev_p"]);
    expect(handoverOf(list, trust)?.holders.map(({ id }) => id).sort()).toEqual(["dev_d", "dev_p"]);
  });

  it("discards verified holders imported before handover v2, while retaining their revocations", async () => {
    const [f, d, p] = await keys();
    const store = memoryDeviceTrust();
    const self = reader("dev_p", p!);
    await store.remember(JSON.stringify(["confirmed-approver-v2", self.deviceId, b64u(self.signingKey)]), {
      verified: { dev_f: holder("dev_f", f!), dev_d: holder("dev_d", d!) }, revoked: ["dev_old"],
    });
    const list = [await row("dev_f", f!), await row("dev_d", d!, { id: "dev_f", key: f! }), await row("dev_p", p!, { id: "dev_d", key: d! })];
    const trust = await trustedDevices(list, self, store);
    expect(ids(trust.trusted)).toEqual(["dev_p"]);
    expect(trust.revoked.has("dev_old")).toBe(true);
    expect(handoverOf(list, trust)).toBeUndefined();
  });

  it("brings the approver's revoked ids and the holders it verified before", async () => {
    // A approved B, X and D. X was revoked; B verified D before that. B approves C and hands over its list.
    // Gateway lists X as trusted again: without the list, C would trust it through A.
    const [a, b, c, x] = await keys();
    const d = (await keys())[0]!;
    const list = [
      await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_x", x!, { id: "dev_a", key: a! }),
      await row("dev_d", d, { id: "dev_x", key: x! }),
    ];
    const plain = await row("dev_c", c!, { id: "dev_b", key: b! });
    const withList = await handed(plain, "dev_b", b!, { holders: [holder("dev_a", a!), holder("dev_b", b!), holder("dev_d", d)], revoked: ["dev_x"] });
    expect(ids((await trustedDevices([...list, plain], reader("dev_c", c!), memoryDeviceTrust())).trusted)).toEqual(["dev_c"]);
    const store = memoryDeviceTrust();
    const trust = await confirmDeviceApprover([...list, withList], reader("dev_c", c!), (await deviceFingerprint(list[1]!)).code, store);
    expect(ids(trust.trusted)).toEqual(["dev_a", "dev_b", "dev_c", "dev_d"]);
    expect(trust.revoked.has("dev_x")).toBe(true);
    // Remembered: a later check without the holder still refuses X.
    expect(ids((await trustedDevices([...list, plain], me("dev_c", c!), store)).trusted)).toEqual(["dev_a", "dev_b", "dev_c", "dev_d"]);
  });

  it("refuses a list Gateway made up, and a list another of the person's devices signed", async () => {
    const [a, b, c, f] = await keys();
    const list = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_f", f!)];
    const plain = await row("dev_c", c!, { id: "dev_b", key: b! });
    const lying = { holders: [holder("dev_f", f!)], revoked: ["dev_a"] };
    // Gateway knows C's public key, so it can seal; it signs with a key of its own, or with the root's real signature.
    for (const signer of [f!, a!]) {
      const trust = await confirmDeviceApprover([...list, await handed(plain, "dev_b", signer, lying)], reader("dev_c", c!), (await deviceFingerprint(list[1]!)).code, memoryDeviceTrust());
      expect(ids(trust.trusted)).toEqual(["dev_b", "dev_c"]);
      expect(trust.revoked.has("dev_a")).toBe(false);
    }
  });

  it("refuses a list from an approver this device does not trust: Gateway rewrote its approval to name F", async () => {
    const [a, b, c, f] = await keys();
    const store = memoryDeviceTrust();
    await confirmDeviceApprover([await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_c", c!, { id: "dev_b", key: b! })], reader("dev_c", c!), (await deviceFingerprint(await row("dev_b", b!))).code, store);
    // F's approval of C verifies, and F signed the list; C trusts B, not F.
    const rewritten = await handed(await row("dev_c", c!, { id: "dev_f", key: f! }), "dev_f", f!, { holders: [holder("dev_f", f!)], revoked: ["dev_b"] });
    const trust = await trustedDevices([await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_f", f!), rewritten], reader("dev_c", c!), store);
    expect(ids(trust.trusted)).toEqual(["dev_b", "dev_c"]);
    expect(trust.revoked.has("dev_b")).toBe(false);
  });

  it("hands over what this device trusts and every revoked id it knows", async () => {
    const [a, b, x] = await keys();
    const list = [await row("dev_a", a!), await row("dev_b", b!, { id: "dev_a", key: a! }), await row("dev_x", x!, { id: "dev_a", key: a! }, "revoked")];
    const trust = await trustedDevices(list, me("dev_a", a!), memoryDeviceTrust());
    expect(handoverOf(list, trust)).toEqual({ holders: [holder("dev_a", a!), holder("dev_b", b!)], revoked: ["dev_x"] });
  });
});

describe("device trust: Gateway vouches (trust gateway)", () => {
  /** A device Gateway registered as trusted at once: no approver and no signature, with `approvedAt` set. */
  const vouched = async (id: string, key: DeviceKey, state: E2eDevice["state"] = "trusted") => ({ ...(await row(id, key, undefined, state)), approvedAt: "2026-11-01T00:01:00Z" });
  const gateway = (id: string, key: DeviceKey) => ({ ...me(id, key), trust: "gateway" as const });

  it("reads the mode from the list: absent or unknown is approvals", () => {
    expect(deviceTrustMode({ trust: "gateway" })).toBe("gateway");
    expect(deviceTrustMode({ trust: "approvals" })).toBe("approvals");
    expect(deviceTrustMode({})).toBe("approvals");
    expect(deviceTrustMode({ trust: "other" })).toBe("approvals");
    expect(deviceTrustMode(undefined)).toBe("approvals");
  });

  it("trusts an unapproved device Gateway lists as trusted; approvals mode refuses it", async () => {
    const [a, b] = await keys();
    const list = [await vouched("dev_a", a!), await vouched("dev_b", b!)];
    expect(ids((await trustedDevices(list, gateway("dev_b", b!), memoryDeviceTrust())).trusted)).toEqual(["dev_a", "dev_b"]);
    expect(ids((await trustedDevices(list, me("dev_b", b!), memoryDeviceTrust())).trusted)).toEqual(["dev_b"]);
  });

  it("never makes a recovery key a holder, and skips pending devices", async () => {
    const [a, b, r, p] = await keys();
    const recovery: E2eDevice = { ...(await row("dev_code", r!, { id: "dev_a", key: a! })), kind: "recovery_code", signingKey: null };
    const list = [await vouched("dev_a", a!), await vouched("dev_b", b!), recovery, await row("dev_p", p!, undefined, "pending")];
    const trust = await trustedDevices(list, gateway("dev_a", a!), memoryDeviceTrust());
    expect(ids(trust.trusted)).toEqual(["dev_a", "dev_b"]);
    expect(trust.verified.has("dev_code")).toBe(false);
  });

  it("refuses this device unless Gateway lists it trusted with its keys", async () => {
    const [a, b, mallory] = await keys();
    const refused = (error: unknown) => error instanceof DeviceTrustError && error.kind === "not-trusted";
    const store = memoryDeviceTrust();
    await expect(trustedDevices([await vouched("dev_a", a!)], gateway("dev_b", b!), store)).rejects.toSatisfy(refused);
    await expect(trustedDevices([await vouched("dev_a", a!), await row("dev_b", b!, undefined, "pending")], gateway("dev_b", b!), store)).rejects.toSatisfy(refused);
    await expect(trustedDevices([await vouched("dev_a", a!), await vouched("dev_b", mallory!)], gateway("dev_b", b!), store)).rejects.toSatisfy(refused);
  });

  it("keeps every revocation it saw, and remembers no holder as verified", async () => {
    const [a, b] = await keys();
    const store = memoryDeviceTrust();
    const revoked = [await vouched("dev_a", a!), await vouched("dev_b", b!, "revoked")];
    const trust = await trustedDevices(revoked, gateway("dev_a", a!), store);
    expect(ids(trust.trusted)).toEqual(["dev_a"]);
    expect(ids(trust.verified)).toEqual(["dev_a", "dev_b"]);
    // Gateway lists B as trusted again: it stays refused.
    const again = [await vouched("dev_a", a!), await vouched("dev_b", b!)];
    expect(ids((await trustedDevices(again, gateway("dev_a", a!), store)).trusted)).toEqual(["dev_a"]);
    // Back in approvals mode, nothing Gateway vouched for counts as verified.
    expect(ids((await trustedDevices(again, me("dev_a", a!), store)).trusted)).toEqual(["dev_a"]);
  });
});

describe("handover payload", () => {
  it("reads back what it writes and refuses anything else", async () => {
    const [a] = await keys();
    const handover = { holders: [holder("dev_a", a!), { id: "dev_code", encryptionKey: b64u(a!.encryptionKey), signingKey: null }], revoked: ["dev_x"] };
    expect(readHandoverPayload(handoverPayload(handover))).toEqual(handover);
    const text = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
    for (const bad of [
      { version: 1, holders: [], revoked: [] },
      { version: 2, holders: [{ id: "dev:a", encryptionKey: b64u(a!.encryptionKey), signingKey: null }], revoked: [] },
      { version: 2, holders: [{ id: "dev_a", encryptionKey: "short", signingKey: null }], revoked: [] },
      { version: 2, holders: [], revoked: [7] },
      { version: 2, holders: {}, revoked: [] },
    ]) {
      expect(readHandoverPayload(text(bad))).toBeUndefined();
    }
    expect(readHandoverPayload(Uint8Array.of(0xff))).toBeUndefined();
  });
});
