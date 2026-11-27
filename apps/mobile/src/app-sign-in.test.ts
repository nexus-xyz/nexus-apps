import { createHash, randomBytes } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  accountOf,
  APP_REDIRECT,
  authorizeUrl,
  base64Url,
  bytesToBase64Url,
  isSignInReturn,
  NO_ANSWER,
  readCallback,
  SIGN_IN_STEP_MS,
  withDeadline,
} from "./app-sign-in";

describe("PKCE encoding", () => {
  it("makes RFC 7636's verifier and challenge (appendix B)", () => {
    const octets = [
      116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
    ];
    const verifier = bytesToBase64Url(new Uint8Array(octets));
    expect(verifier).toBe("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    expect(base64Url(createHash("sha256").update(verifier).digest("base64"))).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  });

  it("encodes any length as Node's base64url does", () => {
    for (const length of [0, 1, 2, 3, 16, 31, 32]) {
      const bytes = randomBytes(length);
      expect(bytesToBase64Url(new Uint8Array(bytes))).toBe(bytes.toString("base64url"));
    }
  });
});

describe("authorizeUrl", () => {
  it("names the app's redirect, an S256 challenge, the state and the provider", () => {
    const url = new URL(authorizeUrl("https://gateway.test", { client: "Nexus for iPhone", redirect: APP_REDIRECT, challenge: "c", state: "s", provider: "apple" }));
    expect(url.origin + url.pathname).toBe("https://gateway.test/authorize");
    expect(Object.fromEntries(url.searchParams)).toEqual({
      client: "Nexus for iPhone",
      client_id: "nxc_mobile",
      redirect_uri: APP_REDIRECT,
      code_challenge: "c",
      code_challenge_method: "S256",
      state: "s",
      provider: "apple",
    });
  });
});

describe("readCallback", () => {
  it.each(["#", "#_=_", "#?state=other"])("ignores the callback's URI fragment %s when checking state", (fragment) => {
    expect(readCallback(`xyz.nexus.app:/auth?code=nxa_1&state=s${fragment}`, "s")).toBe("nxa_1");
    expect(() => readCallback(`xyz.nexus.app:/auth?code=nxa_1&state=other${fragment}`, "s")).toThrow("did not start here");
  });

  it("takes the code of this sign-in only", () => {
    expect(readCallback("xyz.nexus.app:/auth?code=nxa_1&state=s", "s")).toBe("nxa_1");
    expect(() => readCallback("xyz.nexus.app:/author?code=nxa_1&state=s", "s")).toThrow("unexpected sign-in link");
    expect(() => readCallback("xyz.nexus.app:/auth?code=nxa_1&state=other", "s")).toThrow("did not start here");
    expect(() => readCallback("xyz.nexus.app:/auth?code=nxa_1", "s")).toThrow("did not start here");
    expect(() => readCallback("xyz.nexus.app:/auth?state=s", "s")).toThrow("didn't send a sign-in code");
    expect(() => readCallback("xyz.nexus.app:/auth?error=access_denied&state=s", "s")).toThrow("The sign-in was refused (access_denied)");
    expect(() => readCallback("xyz.nexus.app:/auth?error=access_denied&error_description=Denied+here&state=s", "s")).toThrow("Denied here");
  });
});

describe("isSignInReturn", () => {
  it("knows the sheet's return and leaves other links alone", () => {
    for (const path of ["xyz.nexus.app:/auth?code=nxa_1&state=s", "xyz.nexus.app:/auth", "/auth?code=x", "/auth/"]) expect(isSignInReturn(path)).toBe(true);
    for (const path of ["nexus://a/key", "nexus://session/1", "/authors", "xyz.nexus.app:/author", "xyz.nexus.app:/", "/"]) expect(isSignInReturn(path)).toBe(false);
  });
});

describe("accountOf", () => {
  const tenant = { id: "t_1", name: "Acme", createdAt: "2026-11-05T00:00:00Z" };

  it("names the organization and the person, as the sign-in keeps them", () => {
    expect(accountOf({ tenant: { ...tenant, slug: "acme", kind: "company" }, user: { id: "u_1", name: "Ada Lovelace" } })).toEqual({
      org: "acme",
      orgName: "Acme",
      name: "Ada Lovelace",
    });
  });

  it("calls a personal organization Personal, uses the id without a slug, and keeps no name as empty", () => {
    expect(accountOf({ tenant: { ...tenant, kind: "personal" } })).toEqual({ org: "t_1", orgName: "Personal", name: "" });
  });
});

describe("withDeadline", () => {
  afterEach(() => vi.useRealTimers());

  it("answers with the work when it settles in time", async () => {
    await expect(withDeadline(Promise.resolve("token"), SIGN_IN_STEP_MS, NO_ANSWER)).resolves.toBe("token");
    await expect(withDeadline(Promise.reject(new Error("refused")), SIGN_IN_STEP_MS, NO_ANSWER)).rejects.toThrow("refused");
  });

  it("rejects with the message when Gateway never answers, so the spinner ends", async () => {
    vi.useFakeTimers();
    const never = new Promise<string>(() => {});
    const result = withDeadline(never, SIGN_IN_STEP_MS, NO_ANSWER);
    const settled = expect(result).rejects.toThrow(NO_ANSWER);
    await vi.advanceTimersByTimeAsync(SIGN_IN_STEP_MS);
    await settled;
  });

  it("stops its timer once the work settles", async () => {
    vi.useFakeTimers();
    await withDeadline(Promise.resolve(1), SIGN_IN_STEP_MS, NO_ANSWER);
    expect(vi.getTimerCount()).toBe(0);
  });
});
