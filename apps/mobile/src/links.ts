/**
 * Nexus UI's address: the origin of the approval page Gateway named at sign-in (`verification_uri`), else the Gateway's
 * own (Nexus UI under `/app` there). A regex, not `URL`: React Native's URL lacks `origin`.
 */
export const uiOrigin = (gateway: string, verificationUri?: string) => /^(https?:\/\/[^/?#]+)/i.exec(verificationUri ?? "")?.[1] ?? gateway;

/** A Nexus UI page of the organization: `/app/<org><path>`. */
export const uiPage = (ui: string, org: string, path: string) => `${ui}/app/${encodeURIComponent(org)}${path}`;

/** Production Nexus Gateway. */
export const DEFAULT_GATEWAY = "https://api.nexus.inc";

/**
 * Host names the build gives the app (`app.config.ts`, read by `build-hosts.ts`), without scheme: the retired
 * Gateway and its Nexus UI, and the dev Gateway. Each is unset when the build names none.
 */
export type BuildHosts = { legacyUi?: string; legacyGateway?: string; devGateway?: string };

/**
 * A sign-in saved on a retired Gateway host (the legacy Gateway, a `*.nexus.xyz` name but the dev Gateway's), moved to
 * production's with its Nexus UI kept; `undefined` when it stays. A regex, not `URL`, as in `uiOrigin`. A sign-in saved
 * on the legacy Gateway before `ui` was kept finds its Nexus UI on the legacy UI host.
 */
export function movedSignIn<T extends { gateway: string; ui?: string }>(saved: T, hosts: BuildHosts): T | undefined {
  const { legacyUi, legacyGateway, devGateway } = hosts;
  const host = /^https?:\/\/([^/?#:]+)/i.exec(saved.gateway)?.[1]?.toLowerCase() ?? "";
  const retired = (!!legacyGateway && host === legacyGateway) || (host.endsWith(".nexus.xyz") && host !== devGateway);
  if (!retired) return undefined;
  const legacyUiOf = legacyGateway && legacyUi && saved.gateway === `https://${legacyGateway}` ? `https://${legacyUi}` : undefined;
  return { ...saved, gateway: DEFAULT_GATEWAY, ui: saved.ui ?? legacyUiOf };
}

/**
 * The site's support page and legal pages, which Settings and sign-in open and App Store Connect lists.
 * They are the site's, not the signed-in Gateway's Nexus UI, so a sign-in to a dev Gateway opens the same pages. The
 * legal paths are Nexus UI's `LEGAL_URLS`. `terms` is the Customer Terms, which govern the apps; the site's
 * `/legal/terms` is the Terms of Use, which govern only the website.
 */
export const SITE_PAGES = {
  support: "https://nexus.inc/support",
  privacyPolicy: "https://nexus.inc/legal/privacy",
  terms: "https://nexus.inc/legal/customer-terms",
} as const;
