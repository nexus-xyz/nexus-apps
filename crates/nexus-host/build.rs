//! The host's version (`NEXUS_HOST_VERSION`): the desktop app's own, so the app always asks for a host of exactly its
//! version. `NEXUS_DESKTOP_BUILD_VERSION`, which the release sets for the app
//! and its host alike, else the app's `version` in `apps/desktop/Cargo.toml`, plus the git short sha as every Nexus
//! binary has it. The same rule as the app's build.rs.

// git runs at build time, not in the host: the console rule (clippy.toml) is not for it.
#![allow(clippy::disallowed_methods)]

include!("../../build/version.rs");

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    // Only for the `+<sha>` suffix; its semver is the repository's, not the app's.
    let (_, factory) = factory_version(&manifest.join("../nexus-client"));
    let sha = factory.split_once('+').map(|(_, sha)| sha.to_string());

    // The app's manifest, two folders up (`apps/desktop/Cargo.toml`): its `[package]` version.
    let file = manifest.join("../../apps/desktop/Cargo.toml");
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-env-changed=NEXUS_DESKTOP_BUILD_VERSION");
    let semver = match std::env::var("NEXUS_DESKTOP_BUILD_VERSION") {
        Ok(version) if !version.trim().is_empty() => version.trim().to_string(),
        _ => {
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("{}: {error} (the desktop app's manifest)", file.display()));
            app_version(&text).unwrap_or_else(|| panic!("{}: no [package] version", file.display()))
        }
    };
    let core = semver.split('-').next().unwrap_or_default();
    let valid = core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && semver
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !valid {
        panic!("desktop version {semver:?} is not a semver (MAJOR.MINOR.PATCH[-prerelease], no build)");
    }
    let version = match sha {
        Some(sha) => format!("{semver}+{sha}"),
        None => semver,
    };
    println!("cargo:rustc-env=NEXUS_HOST_VERSION={version}");
}

/// The `version` of a manifest's `[package]` table.
fn app_version(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && line.starts_with("version") {
            let value = line.strip_prefix("version")?.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}
