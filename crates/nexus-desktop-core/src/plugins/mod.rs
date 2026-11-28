//! Installed plugins and the person's approvals (plugin API v0). The manifest and its
//! rules are `nexus_workbench::plugins`.
//!
//! - [`store`]: the plugins folder, `<config>/plugins/<id>/`: install from a folder, list, remove.
//! - [`approval`]: what the person approved, kept in `settings.json` (`plugins`), and what a new version changes.
//! - [`session`]: the approved, enabled plugins as the sessions' MCP servers, and their secrets' keychain entry.
//!
//! All of it is off unless `NEXUS_DESKTOP_PLUGINS=1` ([`enabled`]): the app shows no plugin and starts none.

pub mod approval;
pub mod session;
pub mod store;

use std::path::{Path, PathBuf};

pub use approval::{Approval, Grant, State};
pub use store::{InstallError, Installed};

/// The switch for plugins, off by default.
pub const SWITCH: &str = "NEXUS_DESKTOP_PLUGINS";

/// Whether plugins are on: `NEXUS_DESKTOP_PLUGINS=1`.
pub fn enabled() -> bool {
    enabled_with(std::env::var(SWITCH).ok().as_deref())
}

fn enabled_with(value: Option<&str>) -> bool {
    value == Some("1")
}

/// The plugins folder inside the app's config folder.
pub fn dir(config_dir: &Path) -> PathBuf {
    config_dir.join("plugins")
}

#[cfg(test)]
mod tests {
    #[test]
    fn plugins_are_on_only_when_the_switch_says_1() {
        assert!(super::enabled_with(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("on")] {
            assert!(!super::enabled_with(value), "{value:?}");
        }
    }
}
