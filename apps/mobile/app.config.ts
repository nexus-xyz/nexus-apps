import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { ConfigContext, ExpoConfig } from "expo/config";

/**
 * The hosts a build names, read from the environment at build time (OS-8). `app.json` names only the
 * production domain; a build sets these to its own hosts. Unset or "" names none. Nexus's release workflow
 * (.github/workflows/mobile-release.yml) sets Nexus's own from repository variables.
 * - `NEXUS_MOBILE_LEGACY_UI_HOST`: the original Nexus UI host. Its `/a/<key>` share links open the app (iOS associated
 *   domains, Android's intent filter), and a sign-in saved on the legacy Gateway keeps it as its Nexus UI.
 * - `NEXUS_MOBILE_LEGACY_GATEWAY_HOST`: the retired Gateway host. A sign-in saved there moves to production's.
 * - `NEXUS_MOBILE_DEV_GATEWAY_HOST`: the dev Gateway host. A sign-in saved there stays.
 */
export const HOST_DEFAULTS = {
  NEXUS_MOBILE_LEGACY_UI_HOST: "",
  NEXUS_MOBILE_LEGACY_GATEWAY_HOST: "",
  NEXUS_MOBILE_DEV_GATEWAY_HOST: "",
};

type Env = Record<string, string | undefined>;

/** The app's config with the build's hosts: `extra.hosts` is what `src/build-hosts.ts` reads. */
export function withHosts(config: ExpoConfig, env: Env = process.env): ExpoConfig {
  const host = (name: keyof typeof HOST_DEFAULTS) => (env[name] ?? HOST_DEFAULTS[name]).trim() || undefined;
  const legacyUi = host("NEXUS_MOBILE_LEGACY_UI_HOST");
  const shareLinks = (data: unknown) => Array.isArray(data) && data.some((entry) => entry?.pathPrefix === "/a/");
  return {
    ...config,
    ios: {
      ...config.ios,
      ...(legacyUi && {
        associatedDomains: [...new Set([...(config.ios?.associatedDomains ?? []), `applinks:${legacyUi}`, `webcredentials:${legacyUi}`])],
      }),
    },
    android: config.android && {
      ...config.android,
      intentFilters: config.android.intentFilters?.map((filter) =>
        legacyUi && shareLinks(filter.data) ? { ...filter, data: [...[filter.data ?? []].flat(), { scheme: "https", host: legacyUi, pathPrefix: "/a/" }] } : filter,
      ),
    },
    extra: {
      ...config.extra,
      hosts: { legacyUi, legacyGateway: host("NEXUS_MOBILE_LEGACY_GATEWAY_HOST"), devGateway: host("NEXUS_MOBILE_DEV_GATEWAY_HOST") },
    },
  };
}

/**
 * The store accounts a build names, read from the environment (B12): `app.json` names none, so a fork builds under its
 * own Expo account and store ids. Unset (or empty) leaves the setting out.
 * - `NEXUS_MOBILE_EAS_PROJECT_ID`, `NEXUS_MOBILE_EAS_OWNER`: the EAS project and the Expo account that owns it.
 * - `NEXUS_MOBILE_IOS_BUNDLE_ID`, `NEXUS_MOBILE_APPLE_TEAM_ID`: the iOS bundle id and the Apple team.
 * - `NEXUS_MOBILE_ANDROID_PACKAGE`: the Android application id.
 * Nexus's own values are in the production profile's `env` of eas.json, which stays in the private release pipeline.
 * EAS gives them to its builds; anywhere else (a local build, `expo prebuild` in CI) `storeEnv` reads them from there.
 */
export const STORE_ENV = [
  "NEXUS_MOBILE_EAS_PROJECT_ID",
  "NEXUS_MOBILE_EAS_OWNER",
  "NEXUS_MOBILE_IOS_BUNDLE_ID",
  "NEXUS_MOBILE_APPLE_TEAM_ID",
  "NEXUS_MOBILE_ANDROID_PACKAGE",
] as const;

/** The environment, with eas.json's production values under the ones it does not set. No eas.json: the environment. */
export function storeEnv(env: Env = process.env, easJson: string | null = readIfThere("eas.json")): Env {
  const profile = easJson ? ((JSON.parse(easJson)?.build?.production?.env ?? {}) as Env) : {};
  return { ...Object.fromEntries(STORE_ENV.map((name) => [name, profile[name]])), ...pick(env) };
}

/** The app's config with the build's store accounts. */
export function withStoreIdentity(config: ExpoConfig, env: Env = process.env): ExpoConfig {
  const value = (name: (typeof STORE_ENV)[number]) => env[name]?.trim() || undefined;
  const projectId = value("NEXUS_MOBILE_EAS_PROJECT_ID");
  const owner = value("NEXUS_MOBILE_EAS_OWNER");
  const bundleIdentifier = value("NEXUS_MOBILE_IOS_BUNDLE_ID");
  const appleTeamId = value("NEXUS_MOBILE_APPLE_TEAM_ID");
  const androidPackage = value("NEXUS_MOBILE_ANDROID_PACKAGE");
  return {
    ...config,
    ...(owner && { owner }),
    ios: { ...config.ios, ...(bundleIdentifier && { bundleIdentifier }), ...(appleTeamId && { appleTeamId }) },
    android: config.android && { ...config.android, ...(androidPackage && { package: androidPackage }) },
    extra: projectId ? { ...config.extra, eas: { ...config.extra?.eas, projectId } } : config.extra,
  };
}

/**
 * The app's config without the icon and splash images that are not in the tree. Nexus's icons are its marks, which the
 * open-source repo leaves out (export manifest, B2): there the build takes Expo's default icon, or a fork's own.
 */
export function withoutMissingImages(config: ExpoConfig, exists: (path: string) => boolean = fileExists): ExpoConfig {
  const keep = (path: unknown) => typeof path === "string" && exists(path);
  const images = <T extends object>(object: T | undefined, keys: string[]): T | undefined =>
    object && (Object.fromEntries(Object.entries(object).filter(([key, value]) => !keys.includes(key) || keep(value))) as T);
  const ios = config.ios && { ...config.ios };
  if (ios && typeof ios.icon === "object") {
    const icon = images(ios.icon, ["light", "dark", "tinted"]);
    ios.icon = icon && Object.keys(icon).length ? icon : undefined;
  } else if (ios && ios.icon !== undefined && !keep(ios.icon)) {
    ios.icon = undefined;
  }
  const android = config.android && images(config.android, ["icon"]);
  if (android?.adaptiveIcon) {
    android.adaptiveIcon = images(android.adaptiveIcon, ["foregroundImage", "backgroundImage", "monochromeImage"]);
  }
  const plugins = config.plugins?.map((plugin): NonNullable<ExpoConfig["plugins"]>[number] => {
    if (!Array.isArray(plugin) || plugin[0] !== "expo-splash-screen") return plugin;
    const options = images<Record<string, unknown>>(plugin[1], ["image"]) ?? {};
    const dark = options.dark ? images(options.dark as Record<string, unknown>, ["image"]) : undefined;
    return [plugin[0], { ...options, ...(dark && { dark }) }];
  });
  return { ...images(config, ["icon"])!, ios, android, plugins };
}

function pick(env: Env): Env {
  return Object.fromEntries(STORE_ENV.filter((name) => env[name] !== undefined).map((name) => [name, env[name]]));
}

function here(path: string): string {
  return resolve(__dirname, path);
}

function readIfThere(path: string): string | null {
  return existsSync(here(path)) ? readFileSync(here(path), "utf8") : null;
}

function fileExists(path: string): boolean {
  return existsSync(here(path));
}

export default ({ config }: ConfigContext): ExpoConfig =>
  withHosts(withStoreIdentity(withoutMissingImages(config as ExpoConfig), storeEnv()));
