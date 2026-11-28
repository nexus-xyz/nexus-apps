import { createCoreClient, type E2eDevice } from "@nexus/kit";
import type { PhoneInvitation, PhonePairingDecision, PhonePairingRequest } from "@nexus/kit/computer-types";
import { approvalMessage, b64u, fromB64u, generateDeviceKey, lockContentKey, sign, threadKeyMessage, unlockContentKeyBytes, verify } from "@nexus/kit/e2e";
import { phoneInvitationMessage, phonePairingDecisionMessage } from "@nexus/kit/phone-link";
import { expect, it, vi } from "vitest";
import { loadDeviceKey } from "./device-key";
import { connectPhone, type PhonePairingDelivery } from "./phone-connect";

const storage = vi.hoisted(() => new Map<string, string>());
vi.mock("./store", () => ({
  Storage: { setItemSync: (key: string, value: string) => storage.set(key, value) },
  load: (key: string) => {
    const value = storage.get(key);
    return value ? JSON.parse(value) : undefined;
  },
}));

vi.mock("./device-key", () => ({ loadDeviceKey: vi.fn(), createDeviceKey: vi.fn() }));
vi.mock("./code-reader", () => ({ DEVICE_NAME: "iPhone" }));

it.each([false, true])("recovers the original approved request after losing the response (new client: %s)", async (restart) => {
  storage.clear();
  const computer = await generateDeviceKey();
  const phone = await generateDeviceKey();
  vi.mocked(loadDeviceKey).mockResolvedValue(phone);
  const scope = { gateway: "https://gateway.test", tenantId: "tenant", personId: "person" };
  const createdAt = new Date().toISOString();
  const devices: E2eDevice[] = [
    { id: "dev_computer", name: "Laptop", kind: "device", state: "trusted", createdAt, approvedAt: createdAt, approvedBy: null,
      approvalSignature: null, revokedAt: null, encryptionKey: b64u(computer.encryptionKey), signingKey: b64u(computer.signingKey) },
    { id: "dev_phone", name: "iPhone", kind: "device", state: "pending", createdAt, approvedAt: null, approvedBy: null,
      approvalSignature: null, revokedAt: null, encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey) },
  ];
  const unsigned = {
    version: 1, scope, computerName: "Laptop", inviterDeviceId: "dev_computer",
    inviteId: "d9278c13-5429-4f13-81e5-fd57a06c9f44", computerId: "e07fe98b-4dcb-480a-ad93-d4428f9414b3",
    encryptionKey: b64u(computer.encryptionKey), signingKey: b64u(computer.signingKey),
    expiresAt: Date.now() + 60_000, secret: b64u(new Uint8Array(32).fill(1)),
  } satisfies Omit<PhoneInvitation, "signature">;
  const invitation = { ...unsigned, signature: b64u(await sign(computer, await phoneInvitationMessage(unsigned))) };
  const requests: PhonePairingRequest[] = [];
  const transportKeys: Array<string | null> = [];
  const newCore = () => createCoreClient({ baseUrl: scope.gateway, apiKey: "test", fetch: async (url, init) => {
    const path = new URL(String(url)).pathname;
    if (path === "/v1/tenants/me") return Response.json({ user: { id: "person" }, tenant: { id: "tenant" } });
    if (path === "/v1/devices") return Response.json({ devices });
    if (path === "/v1/devices/dev_computer/pairing-requests") {
      if (init?.method === "POST") {
        transportKeys.push(new Headers(init.headers).get("idempotency-key"));
        const envelope: PhonePairingRequest["envelope"] = JSON.parse(String(init.body));
        const saved = requests.find((request) => request.envelope.id === envelope.id);
        if (saved) {
          expect(envelope).toEqual(saved.envelope);
          return Response.json(saved);
        }
        requests.push({ state: "finished", envelope });
        throw new TypeError("Connection lost");
      }
      return Response.json({ requests: requests.filter((request) => request.state === "pending") });
    }
    throw new Error(`Unexpected request ${path}`);
  } });

  const core = newCore();
  const delivery: PhonePairingDelivery = {};
  await expect(connectPhone(core, scope.gateway, invitation, delivery)).rejects.toThrow("Connection lost");
  const receipt = await connectPhone(restart ? newCore() : core, scope.gateway, invitation, restart ? {} : delivery);
  expect(receipt).toMatchObject({
    deviceId: "dev_phone", computerId: invitation.computerId, envelope: { id: requests[0]?.envelope.id },
  });
  await expect(core.phonePairingReceipt(receipt.envelope)).resolves.toMatchObject({ state: "finished" });
  expect(transportKeys[0]).toBe(receipt.envelope.id);
  if (restart) expect(transportKeys[1]).not.toBe(receipt.envelope.id);
  else expect(transportKeys[1]).toBe(receipt.envelope.id);
  expect(transportKeys[2]).toEqual(expect.any(String));
  expect(transportKeys[2]).not.toBe(receipt.envelope.id);
  expect(requests).toHaveLength(1);
});

it.each(["legacy", "pending", "cancelled"] as const)("handles a pending computer with a trusted phone (%s invitation)", async (state) => {
  storage.clear();
  const macKey = await generateDeviceKey();
  const phoneKey = await generateDeviceKey();
  vi.mocked(loadDeviceKey).mockResolvedValue(phoneKey);
  const scope = { gateway: "https://gateway.test", tenantId: "tenant", personId: "person" };
  const createdAt = new Date().toISOString();
  const pendingMac: E2eDevice = {
    id: "dev_computer", name: "Laptop", kind: "device", state: "pending", createdAt,
    encryptionKey: b64u(macKey.encryptionKey), signingKey: b64u(macKey.signingKey),
    approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  const phone: E2eDevice = {
    ...pendingMac, id: "dev_phone", name: "iPhone", state: "trusted", approvedAt: createdAt,
    encryptionKey: b64u(phoneKey.encryptionKey), signingKey: b64u(phoneKey.signingKey),
  };
  const unsigned = {
    version: state === "legacy" ? 1 : 2, scope, computerName: pendingMac.name, inviterDeviceId: pendingMac.id,
    inviteId: "d9278c13-5429-4f13-81e5-fd57a06c9f44", computerId: "e07fe98b-4dcb-480a-ad93-d4428f9414b3",
    encryptionKey: pendingMac.encryptionKey, signingKey: b64u(macKey.signingKey),
    expiresAt: Date.now() + 60_000, secret: b64u(new Uint8Array(32).fill(1)),
  } satisfies Omit<PhoneInvitation, "signature">;
  const invitation = { ...unsigned, signature: b64u(await sign(macKey, await phoneInvitationMessage(unsigned))) };
  let mac = pendingMac;
  const threadId = "962d7f70-35a8-4a7d-b3ec-57653372d07b";
  const historyKey = new Uint8Array(32).fill(37);
  const locked = await lockContentKey(historyKey, threadId, 0, phone.id, phoneKey.encryptionKey);
  const historySignature = b64u(await sign(phoneKey, threadKeyMessage(threadId, 0, phone.id, locked)));
  const history = vi.fn(() => Response.json({
    keys: [{
      threadId, keyVersion: 1, deviceId: phone.id, senderDeviceId: phone.id, createdAt,
      lockedKey: b64u(locked), signature: historySignature,
    }],
  }));
  const requests: PhonePairingRequest[] = [];
  const core = createCoreClient({ baseUrl: scope.gateway, apiKey: "test", fetch: async (url, init) => {
    const path = new URL(String(url)).pathname;
    if (path === "/v1/tenants/me") return Response.json({ user: { id: scope.personId }, tenant: { id: scope.tenantId } });
    if (path === "/v1/devices" && init?.method !== "POST") return Response.json({ devices: [phone, mac] });
    if (path === "/v1/devices/dev_phone/keys") return history();
    if (path === "/v1/devices/dev_computer/approve") {
      const body = JSON.parse(String(init?.body));
      const signature = fromB64u(body.signature);
      expect(body.approverDeviceId).toBe(phone.id);
      expect(body.keys).toHaveLength(1);
      const [copy] = body.keys;
      const copiedKey = fromB64u(copy.lockedKey)!;
      expect(copy).toMatchObject({ threadId, keyVersion: 1 });
      expect(await verify(phoneKey.signingKey, threadKeyMessage(threadId, 0, mac.id, copiedKey), fromB64u(copy.signature)!)).toBe(true);
      expect(await unlockContentKeyBytes(macKey, threadId, 0, mac.id, copiedKey)).toEqual(historyKey);
      expect(signature && await verify(phoneKey.signingKey, approvalMessage(mac.id, macKey.encryptionKey, macKey.signingKey), signature)).toBe(true);
      mac = { ...mac, state: "trusted", approvedAt: createdAt, approvedBy: phone.id, approvalSignature: body.signature };
      return Response.json(mac);
    }
    if (path === "/v1/devices/dev_computer/pairing-requests") {
      if (init?.method !== "POST") return Response.json({ requests });
      const envelope: PhonePairingRequest["envelope"] = JSON.parse(String(init.body));
      expect(mac.state).toBe(state === "legacy" ? "trusted" : "pending");
      expect(envelope).toMatchObject({ scope, inviteId: invitation.inviteId, inviterDeviceId: mac.id, senderDeviceId: phone.id });
      const request: PhonePairingRequest = { state: "pending", envelope };
      if (state === "cancelled") {
        const cancellation: PhonePairingDecision = {
          state: "cancelled", scope, computerId: invitation.computerId,
          inviteId: invitation.inviteId, inviterDeviceId: mac.id,
          expiresAt: invitation.expiresAt, signature: "",
        };
        cancellation.signature = b64u(await sign(macKey, phonePairingDecisionMessage(cancellation)));
        return Response.json({ ...request, decision: cancellation });
      }
      requests.push(request);
      return Response.json(request);
    }
    throw new Error(`Unexpected request ${path}`);
  } });

  if (state === "cancelled") {
    await expect(connectPhone(core, scope.gateway, invitation, {})).rejects.toThrow("The computer cancelled the connection.");
    expect(mac.state).toBe("pending");
    expect(history).not.toHaveBeenCalled();
    expect(requests).toHaveLength(0);
    return;
  }
  const receipt = await connectPhone(core, scope.gateway, invitation, {});
  expect(receipt.computerId).toBe(invitation.computerId);
  expect(requests).toHaveLength(1);
  expect(mac.approvedBy).toBe(phone.id);
});
