const { withAndroidManifest } = require("expo/config-plugins");

const portraitFeature = "android.hardware.screen.portrait";

module.exports = function withOptionalPortraitFeature(config) {
  return withAndroidManifest(config, (androidConfig) => {
    const manifest = androidConfig.modResults.manifest;
    const features = (manifest["uses-feature"] ??= []);
    const portrait = features.find((feature) => feature.$["android:name"] === portraitFeature);

    // Play otherwise infers a hardware requirement from the activity's portrait orientation.
    if (portrait) {
      portrait.$["android:required"] = "false";
    } else {
      features.push({ $: { "android:name": portraitFeature, "android:required": "false" } });
    }

    return androidConfig;
  });
};
