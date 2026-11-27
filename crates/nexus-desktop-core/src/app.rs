//! The running app's version, set once by the shell when it starts. The crate reads it here, so it needs no build
//! script of its own.

use std::sync::OnceLock;

static VERSION: OnceLock<String> = OnceLock::new();

/// What [`version`] says before the shell set it.
pub const UNSET: &str = "unknown";

/// Sets the app's version. The first call wins; a later one is ignored.
pub fn set_version(version: &str) {
    let _ = VERSION.set(version.to_string());
}

/// The app's version, or [`UNSET`] when the shell has not set it.
pub fn version() -> &'static str {
    VERSION.get().map_or(UNSET, String::as_str)
}
