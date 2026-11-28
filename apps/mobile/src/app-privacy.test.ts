import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const { expo } = JSON.parse(readFileSync(new URL("../app.json", import.meta.url), "utf8"));
const manifest = expo.ios.privacyManifests;

// Apple's names for a collected data type and a purpose, from "Describing data use in privacy manifests". iOS ignores a
// name it does not know, so a misspelt one would drop that answer from the app without an error.
const TYPES = [
  "Name", "EmailAddress", "PhoneNumber", "PhysicalAddress", "OtherUserContactInfo", "Health", "Fitness", "PaymentInfo",
  "CreditInfo", "OtherFinancialInfo", "PreciseLocation", "CoarseLocation", "SensitiveInfo", "Contacts",
  "EmailsOrTextMessages", "PhotosorVideos", "AudioData", "GameplayContent", "CustomerSupport", "OtherUserContent",
  "BrowsingHistory", "SearchHistory", "UserID", "DeviceID", "PurchaseHistory", "ProductInteraction", "AdvertisingData",
  "OtherUsageData", "CrashData", "PerformanceData", "OtherDiagnosticData", "EnvironmentScanning", "Hands", "Head",
  "OtherDataTypes",
].map((name) => `NSPrivacyCollectedDataType${name}`);
const PURPOSES = [
  "ThirdPartyAdvertising", "DeveloperAdvertising", "Analytics", "ProductPersonalization", "AppFunctionality", "Other",
].map((name) => `NSPrivacyCollectedDataTypePurpose${name}`);

describe("App Privacy answers (README, App Privacy)", () => {
  it("say the app does no tracking", () => {
    expect(manifest.NSPrivacyTracking).toBe(false);
    expect(manifest.NSPrivacyTrackingDomains ?? []).toEqual([]);
  });

  it("declare the required reason for reading managed app configuration", () => {
    expect(manifest.NSPrivacyAccessedAPITypes).toContainEqual(expect.objectContaining({
      NSPrivacyAccessedAPIType: "NSPrivacyAccessedAPICategoryUserDefaults",
      NSPrivacyAccessedAPITypeReasons: expect.arrayContaining(["AC6B.1"]),
    }));
  });

  it("use Apple's names, each data type once, linked to the person and not for tracking", () => {
    const collected: Record<string, unknown>[] = manifest.NSPrivacyCollectedDataTypes;
    const names = collected.map((entry) => entry.NSPrivacyCollectedDataType);
    expect(names.filter((name) => !TYPES.includes(name as string))).toEqual([]);
    expect(new Set(names).size).toBe(names.length);
    for (const entry of collected) {
      const purposes = entry.NSPrivacyCollectedDataTypePurposes as string[];
      expect(purposes.length, String(entry.NSPrivacyCollectedDataType)).toBeGreaterThan(0);
      expect(purposes.filter((purpose) => !PURPOSES.includes(purpose))).toEqual([]);
      expect(entry.NSPrivacyCollectedDataTypeLinked).toBe(true);
      expect(entry.NSPrivacyCollectedDataTypeTracking).toBe(false);
    }
  });
});
