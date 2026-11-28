// @vitest-environment happy-dom
import { act, createElement, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { approvalMessage, b64u, fromB64u, generateDeviceKey, sign, verify } from "@nexus/kit/e2e";
import { phonePairingDecisionMessage } from "@nexus/kit/phone-link";
import type { PhonePairingDecision } from "@nexus/kit/computer-types";
import { deviceFingerprint, formatCode, pairingQrText } from "@nexus/kit/pairing";
import { memoryDeviceTrust, setDeviceTrustStore } from "@nexus/kit/device-trust";
import type { CoreClient, E2eDevice } from "@nexus/kit";
import { loadDeviceKey } from "./device-key";
import { haptic } from "./haptics";
import Connect from "./app/connect";
import Index from "./app/index";
import { redirectSystemPath } from "./app/+native-intent";
import { connectionIntent, forgetConnectionIntent, keepConnectionIntent, keepRunOnContinuation } from "./connection-intent";

const fixture = vi.hoisted(() => ({
  signedIn: true,
  connectionCore: undefined as object | undefined, scan: "", from: undefined as string | undefined, approve: undefined as string | undefined, computers: vi.fn(), replace: vi.fn(), back: vi.fn(), chosen: { save: vi.fn() }, reader: vi.fn(), paired: vi.fn(),
  core: { approveE2eDevice: vi.fn<CoreClient["approveE2eDevice"]>(), e2eDevices: vi.fn(), phonePairingRequests: vi.fn(), phonePairingReceipt: vi.fn() },
  invitation: { version: 1, scope: { gateway: "https://gateway.test" }, inviteId: "invite_one", inviterDeviceId: "dev_computer", computerName: "My computer", computerId: "computer_selected", expiresAt: Number.MAX_SAFE_INTEGER },
}));
vi.mock("react-native", () => {
  const view = ({ children }: { children?: ReactNode }) => createElement("div", null, children);
  return {
    ActivityIndicator: () => createElement("progress"), View: view, ScrollView: view, KeyboardAvoidingView: view, Text: view,
    Pressable: ({ children, onPress, disabled, accessibilityLabel }: { children?: ReactNode; onPress: () => void; disabled?: boolean; accessibilityLabel?: string }) =>
      createElement("button", { onClick: onPress, disabled, "aria-label": accessibilityLabel }, children),
    TextInput: ({ value, onChangeText, accessibilityLabel }: { value: string; onChangeText: (text: string) => void; accessibilityLabel: string }) => createElement("input", { value, "aria-label": accessibilityLabel, onChange: (event) => onChangeText(event.currentTarget.value) }),
    StyleSheet: { create: (value: unknown) => value }, Platform: { OS: "ios" },
    AppState: { currentState: "active", addEventListener: () => ({ remove() {} }) },
  };
});
vi.mock("expo-camera", () => ({
  CameraView: ({ onBarcodeScanned }: { onBarcodeScanned: (scan: { data: string }) => void }) =>
    createElement("button", { onClick: () => onBarcodeScanned({ data: fixture.scan }) }, "Scan QR"),
  useCameraPermissions: () => [{ granted: true }, vi.fn(), vi.fn()],
}));
vi.mock("expo-clipboard", () => ({ getStringAsync: vi.fn() }));
vi.mock("expo-linking", () => ({ useLinkingURL: () => undefined }));
vi.mock("expo-router", () => {
  return { Redirect: ({ href }: { href: string }) => createElement("a", { href }, href), router: { replace: fixture.replace, back: fixture.back, canGoBack: () => true }, useLocalSearchParams: () => ({ from: fixture.from, approve: fixture.approve }), useFocusEffect: (effect: () => void | (() => void)) => useEffect(effect, [effect]) };
});
vi.mock("react-native-safe-area-context", () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
vi.mock("@nexus/kit/phone-link", async (importOriginal) => {
  const kit = await importOriginal<typeof import("@nexus/kit/phone-link")>();
  return { ...kit, parsePhoneInvitation: (value: string) => value === "nexus://connect#invite=one" ? fixture.invitation : kit.parsePhoneInvitation(value) };
});
vi.mock("./device-key", () => ({ loadDeviceKey: vi.fn() }));
vi.mock("./auth", () => ({ useAuth: () => ({ core: fixture.signedIn ? fixture.connectionCore ?? fixture.core : null, gateway: "https://gateway.test" }), useCore: () => fixture.connectionCore ?? fixture.core, useStored: () => fixture.chosen }));
vi.mock("./computers", () => ({ useComputers: fixture.computers }));
vi.mock("./code-reader", () => ({ useReader: fixture.reader }));
vi.mock("./code-pairing", () => ({ Pairing: () => null }));
vi.mock("./phone-connect", () => ({ connectPhone: fixture.paired, PhoneConnectionScopeError: class extends Error {} }));
vi.mock("./haptics", () => ({ haptic: vi.fn() }));
vi.mock("./icon", () => ({ Icon: () => null }));
vi.mock("./app-sign-in", () => ({ isSignInReturn: (url: string) => url.startsWith("nexus://auth/callback") }));
vi.mock("./theme", () => ({ font: {}, mono: "mono", text: {}, useTheme: () => ({}) }));

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  setDeviceTrustStore(memoryDeviceTrust());
  vi.clearAllMocks();
  fixture.signedIn = true;
  fixture.core.approveE2eDevice.mockReset();
  fixture.from = undefined;
  fixture.approve = undefined;
  fixture.invitation.version = 1;
  fixture.connectionCore = undefined;
  fixture.computers.mockReturnValue({
    computers: [{ computerId: "computer_selected", runnerDeviceId: "dev_computer", enabled: true }],
    awake: () => true,
  });
  fixture.reader.mockReturnValue({ state: { kind: "loading" }, busy: false, pair: vi.fn(), pairAgain: vi.fn(), refresh: vi.fn() });
  forgetConnectionIntent();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); forgetConnectionIntent(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it.each(["legacy", "valid", "expired after submission", "different keys"])(
  "uses the computer's approval and the keys in the scanned invitation (%s)",
  async (scenario) => {
    const computer = await generateDeviceKey();
    const phone = await generateDeviceKey();
    const listed = scenario === "different keys" ? await generateDeviceKey() : computer;
    const invitation = fixture.invitation;
    const scope = { gateway: "https://gateway.test", personId: "person", tenantId: "tenant" };
    Object.assign(invitation, {
      version: scenario === "legacy" ? 1 : 2, scope,
      encryptionKey: b64u(computer.encryptionKey), signingKey: b64u(computer.signingKey),
    });
    const base = { name: "Device", kind: "device", state: "trusted", createdAt: new Date().toISOString(), approvedAt: new Date().toISOString(), revokedAt: null } satisfies Partial<E2eDevice>;
    const approver: E2eDevice = { ...base, id: "dev_computer", encryptionKey: b64u(listed.encryptionKey), signingKey: b64u(listed.signingKey), approvedBy: null, approvalSignature: null };
    const self: E2eDevice = { ...base, id: "dev_phone", encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey), approvedBy: approver.id, approvalSignature: b64u(await sign(listed, approvalMessage("dev_phone", phone.encryptionKey, phone.signingKey))) };
    fixture.core.e2eDevices.mockResolvedValue({ devices: [approver, self] });
    if (scenario === "expired after submission") {
      fixture.core.e2eDevices.mockResolvedValueOnce({ devices: [approver, { ...self, state: "pending" }] });
    }
    const envelope = { id: "request_one", senderDeviceId: self.id };
    const decision: PhonePairingDecision = {
      scope, state: "approved", requestId: envelope.id, senderDeviceId: self.id, signature: "",
      computerId: invitation.computerId, inviteId: invitation.inviteId, inviterDeviceId: invitation.inviterDeviceId,
      expiresAt: invitation.expiresAt,
    };
    decision.signature = b64u(await sign(computer, phonePairingDecisionMessage(decision)));
    fixture.core.phonePairingRequests.mockResolvedValue({ requests: [] });
    fixture.core.phonePairingReceipt.mockImplementation(async () => {
      if (scenario === "expired after submission") vi.spyOn(Date, "now").mockReturnValue(invitation.expiresAt);
      return { state: "finished", ...(scenario === "legacy" ? {} : { decision }) };
    });
    fixture.paired.mockResolvedValue({ deviceId: self.id, holder: phone, computerId: invitation.computerId, envelope, code: "phone_code" });
    keepConnectionIntent("nexus://connect#invite=one");
    await act(async () => { root.render(createElement(Connect)); });

    if (scenario === "legacy") {
      await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Confirm the approving device"); });
      expect(host.textContent).not.toContain("Start a session");
      return;
    }

    if (scenario === "different keys") {
      await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("approval does not verify"); });
      expect(host.textContent).not.toContain("Computer connected");
      expect(fixture.replace).not.toHaveBeenCalled();
      return;
    }

    await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Computer connected"); });
    expect(host.querySelector("input")).toBeNull();
    expect(vi.mocked(haptic).mock.calls.filter(([effect]) => effect === "success")).toHaveLength(1);
    const open = [...host.querySelectorAll("button")].find((button) => button.textContent === "Start a session");
    expect(open).toBeDefined();
    await act(async () => open?.click());
    expect(fixture.replace).toHaveBeenCalledWith({ pathname: "/session/[id]", params: { id: "new", computer: "computer_selected" } });
    expect(connectionIntent()).toBeUndefined();
    expect(fixture.chosen.save).toHaveBeenLastCalledWith("computer_selected");
  },
);

it.each(["pending", "approved", "cancelled", "different request"])(
  "uses the computer’s signed decision for a new invitation (%s)",
  async (scenario) => {
    const mac = await generateDeviceKey();
    const phone = await generateDeviceKey();
    const scope = { gateway: "https://gateway.test", personId: "person", tenantId: "tenant" };
    const invitation = fixture.invitation;
    Object.assign(invitation, { version: 2, scope, signingKey: b64u(mac.signingKey), encryptionKey: b64u(mac.encryptionKey) });
    const self: E2eDevice = {
      id: "dev_phone", name: "Phone", kind: "device", state: "trusted",
      encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey),
      createdAt: new Date().toISOString(), approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
    };
    const envelope = { id: "request_one", senderDeviceId: self.id };
    fixture.core.e2eDevices.mockResolvedValue({ devices: [self] });
    fixture.paired.mockResolvedValue({ deviceId: self.id, holder: phone, computerId: invitation.computerId, envelope, code: "phone_code" });

    const common = {
      scope, computerId: invitation.computerId, inviteId: invitation.inviteId, inviterDeviceId: invitation.inviterDeviceId,
      expiresAt: invitation.expiresAt, signature: "",
    };
    const decision: PhonePairingDecision = scenario === "cancelled"
      ? { ...common, state: "cancelled" }
      : { ...common, state: "approved", requestId: scenario === "different request" ? "request_two" : envelope.id, senderDeviceId: self.id };
    decision.signature = b64u(await sign(mac, await phonePairingDecisionMessage(decision)));
    fixture.core.phonePairingReceipt.mockResolvedValue({
      state: "finished",
      ...(scenario === "pending" ? {} : { decision }),
    });
    keepConnectionIntent("nexus://connect#invite=one");
    await act(async () => { root.render(createElement(Connect)); });

    const expected = scenario === "approved" ? "Start a session" :
      scenario === "cancelled" ? "The computer cancelled" :
      scenario === "different request" ? "response could not be verified" :
      "Approve on your computer";
    await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain(expected); });
    if (scenario !== "approved") expect(host.textContent).not.toContain("Start a session");
  },
);

it("keeps waiting when the request is absent from the inbox but the exact receipt is pending", async () => {
  const phone = await generateDeviceKey();
  const self: E2eDevice = {
    id: "dev_phone", name: "Phone", kind: "device", state: "trusted",
    encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey),
    createdAt: new Date().toISOString(), approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  fixture.core.e2eDevices.mockResolvedValue({ devices: [self] });
  fixture.core.phonePairingRequests.mockResolvedValue({ requests: [] });
  fixture.core.phonePairingReceipt.mockResolvedValue({ state: "pending" });
  fixture.paired.mockResolvedValue({
    deviceId: self.id, holder: phone, computerId: "computer_selected", envelope: { id: "request_one" }, code: "phone_code",
  });
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });

  expect(fixture.core.phonePairingReceipt).toHaveBeenCalledWith({ id: "request_one" }, expect.any(AbortSignal));
  expect(host.textContent).toContain("Approve on your computer");
  expect(host.textContent).not.toContain("Computer connected");
});

it("shows the computer’s control setting before offering to open it", async () => {
  const phone = await generateDeviceKey();
  const self: E2eDevice = {
    id: "dev_phone", name: "Phone", kind: "device", state: "trusted",
    encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey),
    createdAt: new Date().toISOString(), approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  fixture.core.e2eDevices.mockResolvedValue({ devices: [self] });
  fixture.core.phonePairingRequests.mockResolvedValue({ requests: [] });
  fixture.core.phonePairingReceipt.mockResolvedValue({ state: "finished" });
  fixture.paired.mockResolvedValue({
    deviceId: self.id, holder: phone, computerId: "computer_selected", envelope: { id: "request_one" }, code: "phone_code",
  });
  fixture.computers.mockReturnValue({
    computers: [{ computerId: "computer_selected", runnerDeviceId: "dev_computer", enabled: false }],
    awake: () => false,
  });
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).toContain("Turn on Remote control");
  expect(host.textContent).not.toContain("Start a session");

  fixture.computers.mockReturnValue({
    computers: [{ computerId: "computer_selected", runnerDeviceId: "dev_computer", enabled: true }],
    awake: () => true,
  });
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).toContain("Start a session");
});

it("discards the previous connection when the active account changes", async () => {
  const phone = await generateDeviceKey();
  const self: E2eDevice = {
    id: "dev_phone", name: "Phone", kind: "device", state: "trusted",
    encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey),
    createdAt: new Date().toISOString(), approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  fixture.core.e2eDevices.mockResolvedValue({ devices: [self] });
  fixture.core.phonePairingRequests.mockResolvedValue({ requests: [] });
  fixture.core.phonePairingReceipt.mockResolvedValue({ state: "finished" });
  fixture.paired.mockResolvedValueOnce({
    deviceId: self.id, holder: phone, computerId: "computer_selected", envelope: { id: "request_one" }, code: "phone_code",
  }).mockRejectedValueOnce(new Error("Use the same Nexus account as the computer, then scan again."));
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).toContain("Computer connected");

  fixture.connectionCore = { ...fixture.core };
  fixture.computers.mockReturnValue({ computers: [], awake: () => false });
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).not.toContain("Computer connected");
  expect(host.textContent).toContain("Use the same Nexus account");
});

it("clears a pending invitation and returns to the previous screen when cancelled", async () => {
  fixture.paired.mockImplementation(() => new Promise(() => {}));
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });

  const back = [...host.querySelectorAll("button")].find((button) => button.getAttribute("aria-label") === "Back")!;
  expect(back).toBeDefined();
  await act(async () => back.click());

  expect(fixture.back).toHaveBeenCalledOnce();
  expect(connectionIntent()).toBeUndefined();
});

it("shows recovery and gives feedback once for a QR code it cannot use", async () => {
  fixture.scan = "not a Nexus pairing code";
  await act(async () => { root.render(createElement(Connect)); });

  const scan = [...host.querySelectorAll("button")].find((button) => button.textContent === "Scan QR");
  expect(scan).toBeDefined();
  await act(async () => scan?.click());

  expect(host.textContent).toContain("Couldn’t connect");
  expect(host.textContent).not.toContain("Scan QR");
  expect(vi.mocked(haptic).mock.calls).toEqual([["error"]]);
  expect(fixture.paired).not.toHaveBeenCalled();

  const retry = [...host.querySelectorAll("button")].find((button) => button.textContent === "Scan a new QR code");
  expect(retry).toBeDefined();
  await act(async () => retry?.click());
  expect(host.textContent).toContain("Connect a computer");
  expect(host.textContent).toContain("Scan QR");
  expect(vi.mocked(haptic).mock.calls).toEqual([["error"], ["tap"]]);
});

it("offers no QR code, link or code when Gateway vouches for the person's devices", async () => {
  const refresh = vi.fn();
  fixture.reader.mockReturnValue({ state: { kind: "ready" }, trust: "gateway", busy: false, pair: vi.fn(), pairAgain: vi.fn(), refresh });
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).toContain("This phone is set up");
  expect(host.textContent).not.toContain("Scan QR");
  expect(host.textContent).not.toContain("connection link");
  fixture.reader.mockReturnValue({ state: { kind: "error", message: "Gateway refused" }, trust: "gateway", busy: false, pair: vi.fn(), pairAgain: vi.fn(), refresh });
  await act(async () => { root.render(createElement(Connect)); });
  expect(host.textContent).toContain("Gateway refused");
  await act(async () => [...host.querySelectorAll("button")].find((button) => button.textContent === "Try again")?.click());
  expect(refresh).toHaveBeenCalled();
});

it("lets the phone retry the same invitation after a failed submission", async () => {
  fixture.paired.mockRejectedValueOnce(new TypeError("Network request failed"))
    .mockImplementation(() => new Promise(() => {}));
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });

  const retry = [...host.querySelectorAll("button")].find((button) => button.textContent === "Try again")!;
  expect(retry).toBeDefined();
  await act(async () => retry.click());

  expect(fixture.paired).toHaveBeenCalledTimes(2);
});


it("opens Approve computer from Computers' Waiting for approval row, with no scan", async () => {
  const [computerKey, phoneKey] = await Promise.all([generateDeviceKey(), generateDeviceKey()]);
  const createdAt = new Date().toISOString();
  const computer: E2eDevice = {
    id: "dev_laptop", name: "Laptop", kind: "device", state: "pending", createdAt,
    encryptionKey: b64u(computerKey.encryptionKey), signingKey: b64u(computerKey.signingKey),
    approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  const phone: E2eDevice = {
    ...computer, id: "dev_iphone", name: "iPhone", state: "trusted", approvedAt: createdAt,
    encryptionKey: b64u(phoneKey.encryptionKey), signingKey: b64u(phoneKey.signingKey),
  };
  const { fingerprint, code } = await deviceFingerprint(computer);
  fixture.approve = pairingQrText(computer.id, fingerprint);
  vi.mocked(loadDeviceKey).mockResolvedValue(phoneKey);
  fixture.core.e2eDevices.mockResolvedValue({ devices: [phone, computer] });

  await act(async () => { root.render(createElement(Connect)); });
  await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Trust Laptop?"); });
  // The person still compares the code with the computer's screen, and approves by hand.
  expect(host.textContent).toContain(formatCode(code));
  expect(fixture.core.approveE2eDevice).not.toHaveBeenCalled();
});

it.each(["unchanged", "lost response", "changed keys"])("checks the computer approval QR and requires an explicit approval (%s)", async (scenario) => {
  const computerKey = await generateDeviceKey();
  const phoneKey = await generateDeviceKey();
  const createdAt = new Date().toISOString();
  const computer: E2eDevice = {
    id: "dev_laptop", name: "Laptop", kind: "device", state: "pending", createdAt,
    encryptionKey: b64u(computerKey.encryptionKey), signingKey: b64u(computerKey.signingKey),
    approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  const phone: E2eDevice = {
    ...computer, id: "dev_iphone", name: "iPhone", state: "trusted", approvedAt: createdAt,
    encryptionKey: b64u(phoneKey.encryptionKey), signingKey: b64u(phoneKey.signingKey),
  };
  const { fingerprint } = await deviceFingerprint(computer);
  fixture.scan = pairingQrText(computer.id, fingerprint);
  vi.mocked(loadDeviceKey).mockResolvedValue(phoneKey);
  fixture.core.e2eDevices.mockResolvedValue({ devices: [phone, computer] });
  fixture.core.approveE2eDevice.mockImplementationOnce(async (id, body) => {
    const signature = fromB64u(body.signature);
    expect(id).toBe(computer.id);
    expect(body.approverDeviceId).toBe(phone.id);
    expect(body.handover).toBeDefined();
    expect(signature && await verify(phoneKey.signingKey, approvalMessage(id, computerKey.encryptionKey, computerKey.signingKey), signature)).toBe(true);
    const trusted: E2eDevice = {
      ...computer, state: "trusted", approvedAt: createdAt, approvedBy: phone.id, approvalSignature: body.signature,
    };
    fixture.core.e2eDevices.mockResolvedValue({ devices: [phone, trusted] });
    if (scenario === "lost response") throw new TypeError("Network request failed");
    return trusted;
  });

  const click = async (label: string) => {
    const button = [...host.querySelectorAll("button")].find((button) => button.textContent === label);
    expect(button).toBeDefined();
    await act(async () => button?.click());
  };
  await act(async () => { root.render(createElement(Connect)); });
  await click("Scan QR");
  await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Trust Laptop?"); });
  expect(fixture.core.approveE2eDevice).not.toHaveBeenCalled();
  expect(fixture.paired).not.toHaveBeenCalled();

  if (scenario === "changed keys") {
    fixture.core.e2eDevices.mockResolvedValue({
      devices: [phone, { ...computer, signingKey: b64u((await generateDeviceKey()).signingKey) }],
    });
    await click("Approve computer");
    await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("No device is waiting with that code"); });
    expect(fixture.core.approveE2eDevice).not.toHaveBeenCalled();
    return;
  }

  await click("Approve computer");
  if (scenario === "lost response") {
    await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Network request failed"); });
    await click("Approve computer");
  }
  await vi.waitFor(async () => { await act(async () => {}); expect(host.textContent).toContain("Computer approved"); });
  expect(fixture.core.approveE2eDevice).toHaveBeenCalledOnce();
  expect(host.textContent).toContain("On Laptop, match this fingerprint under This device, then click Confirm.");
  expect(host.textContent).toContain(formatCode((await deviceFingerprint(phone)).code));
  expect(host.textContent).not.toContain("Computer connected");
  expect(host.textContent).toContain("Waiting for Laptop to appear");
  expect(fixture.paired).not.toHaveBeenCalled();
});

it("resumes the Run on action only when the person opens the approved computer", async () => {
  const phone = await generateDeviceKey();
  const self: E2eDevice = {
    id: "dev_phone", name: "Phone", kind: "device", state: "trusted",
    encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey),
    createdAt: new Date().toISOString(), approvedAt: null, approvedBy: null, approvalSignature: null, revokedAt: null,
  };
  const resume = vi.fn();
  fixture.from = "run-on";
  keepRunOnContinuation(fixture.core as unknown as CoreClient, resume);
  fixture.core.e2eDevices.mockResolvedValue({ devices: [self] });
  fixture.core.phonePairingRequests.mockResolvedValue({ requests: [] });
  fixture.core.phonePairingReceipt.mockResolvedValue({ state: "finished" });
  fixture.paired.mockResolvedValue({
    deviceId: self.id, holder: phone, computerId: "computer_selected", envelope: { id: "request_one" }, code: "phone_code",
  });
  keepConnectionIntent("nexus://connect#invite=one");
  await act(async () => { root.render(createElement(Connect)); });

  const open = [...host.querySelectorAll("button")].find((button) => button.textContent === "Start a session");
  expect(open).toBeDefined();
  expect(resume).not.toHaveBeenCalled();
  await act(async () => open?.click());
  expect(resume).toHaveBeenCalledOnce();
  expect(fixture.replace).toHaveBeenCalledWith({ pathname: "/session/[id]", params: { id: "new", computer: "computer_selected" } });
});

it("keeps a pending invitation when the index sends a signed-out user to sign-in", async () => {
  const invitation = "xyz.nexus.app://connect#invite=one";
  redirectSystemPath({ path: invitation, initial: true });
  fixture.signedIn = false;

  await act(async () => { root.render(createElement(Index)); });
  expect(host.querySelector("a")?.getAttribute("href")).toBe("/sign-in");
  expect(connectionIntent()).toBe(invitation);

  fixture.signedIn = true;
  await act(async () => { root.render(createElement(Index)); });
  expect(host.querySelector("a")?.getAttribute("href")).toBe("/connect");
});

it("opens a system camera link without putting its invitation in navigation state", () => {
  const path = "xyz.nexus.app://connect#invite=one";
  expect(redirectSystemPath({ path, initial: true })).toBe("/connect");
  expect(connectionIntent()).toBe(path);
  expect(redirectSystemPath({ path: "nexus://auth/callback?code=one", initial: false })).toBeNull();
});

it("opens Expo preview connection links through the same route", () => {
  const path = "exp://192.0.2.10:8081/--/connect#invite=one";
  expect(redirectSystemPath({ path, initial: false })).toBe("/connect");
  expect(connectionIntent()).toBe(path);
});
