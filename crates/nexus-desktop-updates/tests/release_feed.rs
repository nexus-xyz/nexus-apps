//! The release's feed script and the app agree on `latest.json`. The sample manifest is the one the script's own
//! test pins (apps/desktop/packaging/test_feed.py), so a change to either side fails one of the two tests.
use nexus_desktop_updates::Channel;
use nexus_desktop_updates::manifest::{Manifest, package_target};

const SAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/desktop/packaging/testdata/latest.json"
));

/// The package keys the release publishes: one per platform it builds.
const PACKAGES: [&str; 3] = ["linux-x86_64-tarball", "darwin-aarch64-dmg", "windows-x86_64-exe"];

#[test]
fn the_app_reads_the_manifest_the_release_writes() {
    let manifest = Manifest::parse(SAMPLE).expect("the sample manifest parses");
    assert_eq!(manifest.version, "1.2.3-beta.4");
    let channel = Channel::of(&manifest.version).expect("a beta version has a channel");
    assert_eq!(channel, Channel::Beta);
    // Each file sits in its version's folder of the channel, beside that version's copy of the manifest.
    let folder = format!("/{}/{}/", channel.folder(), manifest.version);
    for key in PACKAGES {
        let package = manifest
            .platform(key)
            .unwrap_or_else(|| panic!("no {key} in the manifest"));
        assert!(package.url.contains(&folder), "{} is outside {folder}", package.url);
        assert!(!package.signature.is_empty());
    }
    // This computer's own key is one of them, on every platform the release builds.
    if let Some(target) = package_target().filter(|target| PACKAGES.contains(&target.as_str())) {
        assert!(manifest.platform(&target).is_some());
    }
    assert_eq!(
        manifest.platforms.len(),
        PACKAGES.len(),
        "a key no build of the app asks for"
    );
}

#[test]
fn the_manifest_lists_a_signed_host_for_each_linux_arch() {
    // The app reads these entries as plain JSON (nexus-remote's install.rs): `sha256` and `signature` are required
    // there, and the file's name is `nexus-host_<version>_linux-<arch>.gz`, beside the manifest.
    let manifest: serde_json::Value = serde_json::from_str(SAMPLE).expect("json");
    let version = manifest["version"].as_str().expect("version");
    for arch in ["x86_64", "aarch64"] {
        let host = &manifest["downloads"][format!("linux-{arch}-nexus-host")];
        let url = host["url"].as_str().unwrap_or_else(|| panic!("no host for {arch}"));
        assert!(
            url.ends_with(&format!("/{version}/nexus-host_{version}_linux-{arch}.gz")),
            "{url}"
        );
        assert_eq!(host["sha256"].as_str().map(str::len), Some(64));
        assert!(
            host["signature"]
                .as_str()
                .is_some_and(|signature| !signature.is_empty())
        );
    }
}
