import { existsSync, readFileSync } from "node:fs";
import type { ExpoConfig } from "expo/config";
import { describe, expect, it } from "vitest";
import { storeEnv, withHosts, withoutMissingImages, withStoreIdentity } from "../app.config";
import { movedSignIn } from "./links";

// app.config.ts gives the build its hosts from the environment (OS-8); app.json names only production's.
const { expo } = JSON.parse(readFileSync(new URL("../app.json", import.meta.url), "utf8")) as { expo: ExpoConfig };

// Hosts as Nexus's release workflow sets them, from its repository variables.
const RELEASE = {
  NEXUS_MOBILE_LEGACY_UI_HOST: "ui.legacy.example.test",
  NEXUS_MOBILE_LEGACY_GATEWAY_HOST: "gateway.legacy.example.test",
  NEXUS_MOBILE_DEV_GATEWAY_HOST: "gateway.dev.example.test",
};

describe("the build's hosts", () => {
  it("names no legacy host with no environment", () => {
    const none = withHosts(expo, {});
    expect(none.ios?.associatedDomains).toEqual(["webcredentials:nexus.inc", "applinks:nexus.inc"]);
    expect(none.extra?.hosts).toEqual({});
  });

  it("keeps the production sign-in and legacy domains in store builds", () => {
    const before = structuredClone(expo);
    before.ios = {
      ...before.ios,
      associatedDomains: ["webcredentials:nexus.inc", "applinks:nexus.inc", "applinks:ui.legacy.example.test", "webcredentials:ui.legacy.example.test"],
    };
    before.android!.intentFilters![0]!.data = [
      { scheme: "https", host: "nexus.inc", pathPrefix: "/a/" },
      { scheme: "https", host: "ui.legacy.example.test", pathPrefix: "/a/" },
    ];
    // The one addition: the hosts src/links.ts named before, which it now reads from the build.
    before.extra = {
      ...before.extra,
      hosts: { legacyUi: "ui.legacy.example.test", legacyGateway: "gateway.legacy.example.test", devGateway: "gateway.dev.example.test" },
    };
    expect(withHosts(expo, RELEASE)).toEqual(before);
  });

  it("name only production's domain when a build sets them empty", () => {
    const none = withHosts(expo, { NEXUS_MOBILE_LEGACY_UI_HOST: "", NEXUS_MOBILE_LEGACY_GATEWAY_HOST: "", NEXUS_MOBILE_DEV_GATEWAY_HOST: " " });
    expect(none.ios?.associatedDomains).toEqual(["webcredentials:nexus.inc", "applinks:nexus.inc"]);
    expect(none.android?.intentFilters?.[0]?.data).toEqual([{ scheme: "https", host: "nexus.inc", pathPrefix: "/a/" }]);
    expect(none.extra?.hosts).toEqual({});
    expect(JSON.stringify(none)).not.toMatch(/legacy\.example|dev\.example/);
  });

  it("are a fork's own when it sets them", () => {
    const fork = withHosts(expo, { NEXUS_MOBILE_LEGACY_UI_HOST: "ui.example.test", NEXUS_MOBILE_LEGACY_GATEWAY_HOST: "old.example.test", NEXUS_MOBILE_DEV_GATEWAY_HOST: "dev.example.test" });
    expect(fork.ios?.associatedDomains).toEqual(["webcredentials:nexus.inc", "applinks:nexus.inc", "applinks:ui.example.test", "webcredentials:ui.example.test"]);
    expect(fork.android?.intentFilters?.[0]?.data).toContainEqual({ scheme: "https", host: "ui.example.test", pathPrefix: "/a/" });
    expect(fork.extra?.hosts).toEqual({ legacyUi: "ui.example.test", legacyGateway: "old.example.test", devGateway: "dev.example.test" });
  });

  it("move a sign-in saved on the legacy Gateway as before, with the release's hosts", () => {
    const hosts = withHosts(expo, RELEASE).extra?.hosts;
    expect(movedSignIn({ gateway: `https://${hosts.legacyGateway}`, token: "t" }, hosts)).toEqual({
      gateway: "https://api.nexus.inc",
      token: "t",
      ui: `https://${hosts.legacyUi}`,
    });
    expect(movedSignIn({ gateway: `https://${hosts.devGateway}`, token: "t" }, hosts)).toBeUndefined();
  });
});

// B12: app.json names no store account; app.config.ts takes them from the environment, or from eas.json's production
// profile, which stays private.
describe("the build's store accounts", () => {
  const nexus = {
    NEXUS_MOBILE_EAS_PROJECT_ID: "project",
    NEXUS_MOBILE_EAS_OWNER: "owner",
    NEXUS_MOBILE_IOS_BUNDLE_ID: "test.app.ios",
    NEXUS_MOBILE_APPLE_TEAM_ID: "TEAM",
    NEXUS_MOBILE_ANDROID_PACKAGE: "test.app.android",
  };

  it("are not in app.json", () => {
    expect(JSON.stringify(expo)).not.toMatch(/"(ascAppId|appleTeamId|projectId|owner|bundleIdentifier|package)"\s*:/);
  });

  it("come from the environment", () => {
    const config = withStoreIdentity(expo, nexus);
    expect(config.owner).toBe("owner");
    expect(config.extra?.eas).toEqual({ projectId: "project" });
    expect(config.ios?.bundleIdentifier).toBe("test.app.ios");
    expect(config.ios?.appleTeamId).toBe("TEAM");
    expect(config.android?.package).toBe("test.app.android");
    expect(config.ios?.privacyManifests).toEqual(expo.ios?.privacyManifests);
  });

  it("are left out when a fork sets none", () => {
    expect(withStoreIdentity(expo, {})).toEqual(expo);
    expect(withStoreIdentity(expo, { NEXUS_MOBILE_EAS_OWNER: " " }).owner).toBeUndefined();
  });

  it("fall back to eas.json's production profile, under the environment", () => {
    const eas = JSON.stringify({ build: { production: { env: { ...nexus, EXPO_PUBLIC_X: "1" } } } });
    expect(storeEnv({}, eas)).toEqual(nexus);
    expect(storeEnv({ NEXUS_MOBILE_EAS_OWNER: "fork", PATH: "/bin" }, eas)).toEqual({ ...nexus, NEXUS_MOBILE_EAS_OWNER: "fork" });
    expect(storeEnv({ NEXUS_MOBILE_EAS_OWNER: "fork" }, null)).toEqual({ NEXUS_MOBILE_EAS_OWNER: "fork" });
  });

  it.skipIf(!existsSync(new URL("../eas.json", import.meta.url)))("are Nexus's where eas.json is", () => {
    const eas = readFileSync(new URL("../eas.json", import.meta.url), "utf8");
    const config = withStoreIdentity(expo, storeEnv({}, eas));
    expect(config.owner).toBeTruthy();
    expect(config.extra?.eas?.projectId).toBeTruthy();
    expect(config.ios?.bundleIdentifier).toBeTruthy();
    expect(config.android?.package).toBeTruthy();
  });
});

// B2: Nexus's icons are its marks, which the open-source repo leaves out.
describe("the build's icons", () => {
  it("stay as app.json names them when the files are there", () => {
    expect(withoutMissingImages(expo, () => true)).toEqual(expo);
  });

  it("are left out when the files are not", () => {
    const config = withoutMissingImages(expo, () => false);
    expect(config.icon).toBeUndefined();
    expect(config.ios?.icon).toBeUndefined();
    expect(config.android?.icon).toBeUndefined();
    expect(config.android?.adaptiveIcon).toEqual({ backgroundColor: expo.android?.adaptiveIcon?.backgroundColor });
    const splash = config.plugins?.find((plugin) => Array.isArray(plugin) && plugin[0] === "expo-splash-screen") as [string, Record<string, unknown>];
    expect(splash[1].image).toBeUndefined();
    expect(splash[1].dark).toEqual({ backgroundColor: "#18181A" });
    expect(JSON.stringify(config)).not.toMatch(/icon[^"]*\.png/);
  });
});
