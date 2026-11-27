import { createHash, randomBytes } from "node:crypto";
import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  open: vi.fn(),
  save: vi.fn(),
  platform: { OS: "ios", Version: "27.0", isPad: false },
  inbox: { clear: vi.fn(async () => undefined), setAccount: vi.fn(async () => undefined) },
}));
vi.mock("expo-crypto", () => ({
  getRandomBytes: (length: number) => randomBytes(length),
  digestStringAsync: async (_algorithm: string, value: string) => createHash("sha256").update(value).digest("base64"),
  CryptoDigestAlgorithm: { SHA256: "SHA256" },
  CryptoEncoding: { BASE64: "base64" },
}));
vi.mock("expo-secure-store", () => ({ AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: "device-only", setItemAsync: native.save, deleteItemAsync: async () => undefined }));
vi.mock("expo-constants", () => ({ default: { expoConfig: { extra: {} } } }));
vi.mock("expo-web-browser", () => ({ openAuthSessionAsync: native.open }));
vi.mock("react-native", () => ({ Platform: native.platform }));
vi.mock("./managed", () => ({ useManagedConfig: () => ({}) }));
vi.mock("./store", () => ({ forgetAll: vi.fn(), forgetCaches: vi.fn(), load: vi.fn(), remove: vi.fn(), save: vi.fn() }));
vi.mock("./chat-images", () => ({ forgetImages: vi.fn() }));
vi.mock("./device-key", () => ({ forgetDeviceKey: vi.fn(async () => undefined) }));
vi.mock("./share-inbox-native", () => ({ accountKey: async () => "account-key", shareInbox: native.inbox }));

const { AuthProvider, useAuth } = await import("./auth");
let auth: ReturnType<typeof useAuth>;
const wire = vi.fn<typeof fetch>();

function Capture() {
  auth = useAuth();
  return null;
}

beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(native.platform, { OS: "ios", Version: "27.0" });
  vi.stubGlobal("fetch", wire);
  renderToString(createElement(AuthProvider, { children: createElement(Capture) }));
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it.each(["apple", "google", "email"] as const)("completes %s sign-in through the verified iOS callback and saves the exchanged account", async (provider) => {
  let challenge = "";
  native.open.mockImplementation(async (url: string, redirect: string, options: object) => {
    const request = new URL(url);
    challenge = request.searchParams.get("code_challenge") ?? "";
    expect(request.searchParams.get("provider")).toBe(provider);
    expect(request.searchParams.get("redirect_uri")).toBe(redirect);
    expect(redirect).toBe("https://nexus.inc/app/device/authorize/done");
    expect(options).toMatchObject({ preferUniversalLinks: true, preferEphemeralSession: true });
    return { type: "success", url: `${redirect}?code=nxa_returned&state=${request.searchParams.get("state")}` };
  });
  wire.mockImplementation(async (input, init) => {
    const url = String(input);
    if (url.endsWith("/v1/auth/token")) {
      const body = JSON.parse(String(init?.body));
      expect(body).toMatchObject({ grant_type: "authorization_code", code: "nxa_returned", redirect_uri: "https://nexus.inc/app/device/authorize/done" });
      expect(createHash("sha256").update(body.code_verifier).digest("base64url")).toBe(challenge);
      return Response.json({ access_token: "new-account-token", tenant: { id: "personal_new", kind: "personal" }, expires_at: "2026-12-01T00:00:00Z" });
    }
    expect(url).toBe("https://api.nexus.inc/v1/tenants/me");
    expect(new Headers(init?.headers).get("authorization")).toBe("Bearer new-account-token");
    return Response.json({ tenant: { id: "personal_new", kind: "personal" }, user: { id: "new-user", name: "New account" } });
  });
  await auth.signIn(provider);
  expect(native.save).toHaveBeenCalledOnce();
  expect(JSON.parse(native.save.mock.calls[0]![1])).toMatchObject({ token: "new-account-token", org: "personal_new", orgName: "Personal", name: "New account", user: "new-user" });
});

it("removes what was shared to Nexus, and who it was for, on sign-out", async () => {
  await auth.signOut();
  expect(native.inbox.clear).toHaveBeenCalledOnce();
  expect(native.inbox.setAccount).toHaveBeenCalledWith(null);
});

it("does not exchange or save a callback from another sign-in", async () => {
  native.open.mockResolvedValue({ type: "success", url: "https://nexus.inc/app/device/authorize/done?code=nxa_other&state=other" });
  await expect(auth.signIn("google")).rejects.toThrow("did not start here");
  expect(wire).not.toHaveBeenCalled();
  expect(native.save).not.toHaveBeenCalled();
});

it.each(["cancel", "dismiss"])("lets a person retry after leaving the browser (%s)", async (type) => {
  native.open.mockResolvedValue({ type });
  await expect(auth.signIn("google")).resolves.toBeUndefined();
  await expect(auth.signIn("email")).resolves.toBeUndefined();
  expect(native.open).toHaveBeenCalledTimes(2);
  expect(wire).not.toHaveBeenCalled();
  expect(native.save).not.toHaveBeenCalled();
});

it.each([["android", "36"], ["ios", "17.3.1"]])("keeps the app-scheme return on %s %s", async (os, version) => {
  Object.assign(native.platform, { OS: os, Version: version });
  native.open.mockResolvedValue({ type: "cancel" });
  await auth.signIn("google");
  expect(native.open).toHaveBeenCalledWith(
    expect.stringContaining("redirect_uri=xyz.nexus.app%3A%2Fauth"),
    "xyz.nexus.app:/auth",
    expect.objectContaining({ preferUniversalLinks: false }),
  );
});

it("opens one sheet when the person taps two methods before the first sheet opens", async () => {
  native.open.mockResolvedValue({ type: "cancel" });
  await Promise.all([auth.signIn("google"), auth.signIn("email")]);
  expect(native.open).toHaveBeenCalledOnce();
  expect(native.open.mock.calls[0]?.[0]).toContain("provider=google");
});

it("saves the signed-in account promptly when the profile request stalls", async () => {
  vi.useFakeTimers();
  native.open.mockImplementation(async (url: string, redirect: string) => ({
    type: "success",
    url: `${redirect}?code=nxa_returned&state=${new URL(url).searchParams.get("state")}`,
  }));
  wire.mockImplementation(async (input) => {
    if (String(input).endsWith("/v1/auth/token")) {
      return Response.json({ access_token: "new-account-token", tenant: { id: "personal_new" } });
    }
    return new Promise<Response>(() => {});
  });
  const signingIn = auth.signIn("google");
  await vi.advanceTimersByTimeAsync(2_000);
  expect(native.save).toHaveBeenCalledOnce();
  await signingIn;
});
