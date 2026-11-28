//! Default-off switch for the canon's new keys: Ask about Nexus (Cmd+Shift+/), and Ctrl+Cmd+N for a new window on a Mac.
//! With it off, the app binds and lists the keys it had.

use std::sync::OnceLock;

/// `on` turns the canon's new keys on. Unset or anything else: off.
pub const FLAG: &str = "NEXUS_DESKTOP_CANON_KEYS";

/// Whether [`FLAG`] is on in this process.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var(FLAG).ok().as_deref() == Some("on"))
}
