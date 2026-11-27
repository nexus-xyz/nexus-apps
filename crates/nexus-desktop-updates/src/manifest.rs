//! A channel's `latest.json`, as `desktop/scripts/updater-manifest.mjs` writes it: a static updater manifest
//! (`version`, `platforms` with each update file's URL and signature) plus `downloads`, every installer by kind.
//! Nexus for desktop reads it here, under its package key ([`package_target`]). Older Nexus installs read the same
//! feed under `<os>-<arch>-<installer>` and then the bare `<os>-<arch>` ([`target`]). The feed lists neither: they find
//! no update for their platform and never install a package they cannot run.
use std::collections::BTreeMap;

use semver::Version;
use serde::Deserialize;

use crate::Failure;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
    #[serde(default)]
    pub downloads: BTreeMap<String, Download>,
}

/// One platform's update file and its signature (`<file>.sig`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}

/// An installer a person downloads.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Download {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl Manifest {
    /// Parse a manifest. Its `version` is semver, with an optional leading `v` dropped.
    pub fn parse(body: &str) -> Result<Self, Failure> {
        let invalid = || {
            Failure::new(
                "release_not_found",
                "The update feed has no release for this channel, or it returned an error. Nexus will check again later.",
                false,
            )
        };
        let mut manifest: Manifest = serde_json::from_str(body).map_err(|_| invalid())?;
        manifest.version = manifest.version.trim_start_matches('v').to_string();
        Version::parse(&manifest.version).map_err(|_| invalid())?;
        Ok(manifest)
    }

    /// This computer's update file, under its target name (`target`).
    pub fn platform(&self, target: &str) -> Option<&Platform> {
        self.platforms.get(target)
    }
}

/// This computer's update file as the app reads it: `<os>-<arch>-<format>`, the platform ([`target`]) and the package
/// format the app installs, such as `darwin-aarch64-dmg`, `windows-x86_64-exe` or `linux-x86_64-tarball`.
pub fn package_target() -> Option<String> {
    package_key(target()?)
}

/// The package key for a platform `<os>-<arch>`: `.dmg` on macOS, the `.exe` installer on Windows, the `.tar.xz` folder
/// on Linux. No format is one an older Nexus install asks for (`app`, `appimage`, `deb`, `rpm`, `msi`, `nsis`): it
/// reads `<os>-<arch>-<installer>` first, so a `-nsis` key would hand this app's installer to older Windows installs.
fn package_key(target: &str) -> Option<String> {
    let format = match target.split_once('-')?.0 {
        "darwin" => "dmg",
        "windows" => "exe",
        "linux" => "tarball",
        _ => return None,
    };
    Some(format!("{target}-{format}"))
}

/// This computer's platform as the update feed names it: `<os>-<arch>`, such as `linux-x86_64` or `darwin-aarch64`.
pub fn target() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => return None,
    };
    Some(match (os, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("darwin", "x86_64") => "darwin-x86_64",
        ("darwin", "aarch64") => "darwin-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_release_scripts_manifest() {
        let body = r#"{
          "version": "v0.2.0-internal.3",
          "notes": "Nexus 0.2.0-internal.3 (internal)",
          "pub_date": "2026-11-06T00:00:00Z",
          "channel": "internal",
          "platforms": {
            "linux-x86_64-tarball": {"url": "https://downloads.nexus.inc/desktop/internal/0.2.0-internal.3/a.tar.xz", "signature": "c2ln"}
          },
          "downloads": {
            "linux-x86_64-tarball": {"url": "https://downloads.nexus.inc/desktop/internal/0.2.0-internal.3/a.tar.xz", "sha256": "00", "size": 1}
          }
        }"#;
        let manifest = Manifest::parse(body).expect("manifest");
        assert_eq!(manifest.version, "0.2.0-internal.3");
        assert_eq!(
            manifest.platform("linux-x86_64-tarball").expect("linux").signature,
            "c2ln"
        );
        // An older Nexus install asks for the bare platform and finds nothing to install.
        assert!(manifest.platform("linux-x86_64").is_none());
        assert!(manifest.platform("windows-x86_64-nsis").is_none());
        assert_eq!(manifest.downloads["linux-x86_64-tarball"].size, Some(1));
        assert_eq!(
            Manifest::parse(r#"{"version":"latest"}"#).expect_err("not semver").code,
            "release_not_found"
        );
        assert_eq!(
            Manifest::parse("<html>").expect_err("not JSON").code,
            "release_not_found"
        );
        // A channel's first manifest before any signed file: no platforms.
        assert!(
            Manifest::parse(r#"{"version":"1.0.0"}"#)
                .expect("bare")
                .platforms
                .is_empty()
        );
    }

    #[test]
    fn names_each_platforms_package() {
        assert_eq!(package_key("darwin-aarch64").as_deref(), Some("darwin-aarch64-dmg"));
        assert_eq!(package_key("windows-x86_64").as_deref(), Some("windows-x86_64-exe"));
        assert_eq!(package_key("linux-x86_64").as_deref(), Some("linux-x86_64-tarball"));
        assert_eq!(package_key("freebsd-x86_64"), None);
        assert_eq!(package_key("linux"), None);
        let this = package_target().expect("a supported platform");
        assert!(this.starts_with(&format!("{}-", target().expect("target"))));
    }

    #[test]
    fn names_this_computer_as_the_updater_does() {
        let target = target().expect("a supported platform");
        assert!(target.contains('-'));
        assert!(
            ["linux-", "darwin-", "windows-"]
                .iter()
                .any(|os| target.starts_with(os))
        );
    }
}
