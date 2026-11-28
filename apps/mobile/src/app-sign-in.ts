import type { CurrentTenantResponse } from "@nexus/kit";

/**
 * The phone's sign-in (Gateway's `nexus_accounting::device::authorization`): OAuth's authorization code with PKCE.
 * The app opens Gateway's `/authorize` in the system sign-in sheet and exchanges the returned code for its token.
 */

/**
 * Android and older iOS use the app's private scheme, which requires confirmation in the browser.
 * On iOS 17.4+, the system matches the HTTPS host and path for apps associated with nexus.inc.
 */
export const APP_REDIRECT = "xyz.nexus.app:/auth";
const APP_HTTPS_REDIRECT = "https://nexus.inc/app/device/authorize/done";

export function appRedirect(platform: string, version: string | number): string {
  const [major = 0, minor = 0] = String(version).split(".").map(Number);
  return platform === "ios" && (major > 17 || (major === 17 && minor >= 4)) ? APP_HTTPS_REDIRECT : APP_REDIRECT;
}

/** The base64url form of base64 (RFC 4648, 5): `+/` as `-_`, no padding. PKCE's verifier and challenge use it. */
export const base64Url = (base64: string) => base64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** Bytes as base64url, without `btoa`, which React Native may lack. */
export function bytesToBase64Url(bytes: Uint8Array): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const [a, b = 0, c = 0] = [bytes[i]!, bytes[i + 1], bytes[i + 2]];
    const n = (a << 16) | (b << 8) | c;
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]!;
    if (i + 1 < bytes.length) out += alphabet[(n >> 6) & 63]!;
    if (i + 2 < bytes.length) out += alphabet[n & 63]!;
  }
  return out;
}

/**
 * The sign-in the person tapped: Nexus UI starts Apple (the iPhone app only) or Google in the sheet, or
 * opens Clerk's email sign-in, including passwords for accounts that have one, such as the App Review account.
 */
export type Provider = "apple" | "google" | "email";

/** Nexus Mobile's OAuth client id, as Rust's `nexus_types::oauth_clients::MOBILE`. */
export const MOBILE_CLIENT_ID = "nxc_mobile";

/**
 * Gateway's `/authorize` for this sign-in: the app's label and client id, redirect, S256 challenge, state and provider.
 * Gateway passes the query on to Nexus UI as it is, and Nexus UI sends `client_id` to `POST /v1/auth/authorize` as `clientId`.
 */
export function authorizeUrl(gateway: string, request: { client: string; redirect: string; challenge: string; state: string; provider: Provider }): string {
  const query = new URLSearchParams({
    client: request.client,
    client_id: MOBILE_CLIENT_ID,
    redirect_uri: request.redirect,
    code_challenge: request.challenge,
    code_challenge_method: "S256",
    state: request.state,
    provider: request.provider,
  });
  return `${gateway}/authorize?${query}`;
}

/**
 * The code from the sheet's return to the redirect, once its state is this sign-in's; a readable error otherwise,
 * the server's own when it sends OAuth's `error` (RFC 6749, 4.1.2.1).
 */
export function readCallback(url: string, state: string, redirect = APP_REDIRECT): string {
  const [beforeFragment = ""] = url.split("#", 1);
  const queryStart = beforeFragment.indexOf("?");
  const destination = queryStart === -1 ? beforeFragment : beforeFragment.slice(0, queryStart);
  if (destination !== redirect) throw new Error("Nexus received an unexpected sign-in link. Try again.");
  const query = new URLSearchParams(queryStart === -1 ? "" : beforeFragment.slice(queryStart + 1));
  if (query.get("state") !== state) throw new Error("This sign-in did not start here. Try again.");
  const error = query.get("error");
  if (error) throw new Error(query.get("error_description") || `The sign-in was refused (${error}). Try again.`);
  const code = query.get("code");
  if (!code) throw new Error("Nexus didn't send a sign-in code. Try again.");
  return code;
}

/** Whether a link the system hands the app is the sign-in sheet's return to `APP_REDIRECT` (Android, or elsewhere). */
export const isSignInReturn = (path: string) => {
  const destination = path.split(/[?#]/, 1)[0]?.replace(/\/$/, "");
  return destination === APP_REDIRECT || destination === "/auth" || destination === APP_HTTPS_REDIRECT ||
    destination === "/app/device/authorize/done";
};

/**
 * Whose a sign-in is, from Gateway's `me()`: the organization's slug (else id), its name ("Personal" for the person's
 * own) and the person's name ("" when the account has none).
 */
export function accountOf(me: Pick<CurrentTenantResponse, "tenant" | "user">): { org: string; orgName: string; name: string } {
  return {
    org: me.tenant.slug || me.tenant.id,
    orgName: me.tenant.kind === "personal" ? "Personal" : me.tenant.name,
    name: me.user?.name ?? "",
  };
}

/**
 * How long to wait for the sign-in exchange and a managed organization switch.
 */
export const SIGN_IN_STEP_MS = 15_000;
/** Give optional profile details a short head start before opening the app. */
export const SIGN_IN_PROFILE_MS = 1_500;
/** What the sign-in screen says when Gateway did not answer a step in time. */
export const NO_ANSWER = "Nexus didn't answer. Check your connection and sign in again.";

/** `work`, or a rejection with `message` once `ms` pass without an answer. The timer stops when `work` settles. */
export function withDeadline<T>(work: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([work, late]).finally(() => clearTimeout(timer));
}
