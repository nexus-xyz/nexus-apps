/**
 * The Share extension: "Nexus" in the share sheet, for text, links and photos. `@bacons/apple-targets` makes
 * it an Xcode target at prebuild, so `ios/` stays generated. It shares the app's App Group (`app.json`), where Save keeps
 * the import for the app to read. Signing and the App Group's provisioning are EAS Build's.
 *
 * @type {import('@bacons/apple-targets/app.plugin').ConfigFunction}
 */
module.exports = (config) => ({
  type: "share",
  name: "NexusShare",
  displayName: "Nexus",
  bundleIdentifier: ".share",
  deploymentTarget: "16.4",
  frameworks: ["SwiftUI", "UniformTypeIdentifiers"],
  entitlements: {
    "com.apple.security.application-groups": config.ios.entitlements["com.apple.security.application-groups"],
  },
});
