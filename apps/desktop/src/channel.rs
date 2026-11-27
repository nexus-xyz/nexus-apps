//! Which builds are internal, by the Nexus app's rule (`Channel::of` in `nexus-desktop-updates`): a version whose
//! prerelease is `internal.<N>`, as `0.2.0-internal.4`. Development builds count as internal too. Internal builds show
//! what only Nexus's own people need, such as the Gateway field at sign-in.

/// This build's version: `NEXUS_DESKTOP_BUILD_VERSION` when the release set it, else Cargo.toml's.
pub const VERSION: &str = match option_env!("NEXUS_DESKTOP_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Whether `version`'s prerelease is `internal.<N>`.
pub fn is_internal(version: &str) -> bool {
    let version = version.split('+').next().unwrap_or(version);
    version
        .split_once('-')
        .and_then(|(_, pre)| pre.strip_prefix("internal."))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Whether the release pipeline built this (it sets `NEXUS_DESKTOP_BUILD_VERSION`). A build from source never sends
/// usage data (PRIVACY.md).
pub const RELEASE_BUILD: bool = option_env!("NEXUS_DESKTOP_BUILD_VERSION").is_some();

pub fn internal() -> bool {
    cfg!(debug_assertions) || is_internal(VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_is_the_prerelease() {
        assert!(is_internal("0.2.0-internal.4"));
        assert!(is_internal("0.2.0-internal.4+abc123"));
        assert!(!is_internal("0.2.0"));
        assert!(!is_internal("0.2.0-beta.1"));
        assert!(!is_internal("0.2.0-internal."));
        assert!(!is_internal("0.2.0-internal.x"));
    }
}
