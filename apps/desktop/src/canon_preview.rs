//! Default-off switches for canon transcript capabilities still gated for production cutover.

use std::sync::OnceLock;

/// `on` enables canon plan answers that set permission modes and "Ask the agent to fix it" on failed steps.
pub const TRANSCRIPT_ACTIONS: &str = "NEXUS_DESKTOP_CANON_TRANSCRIPT_ACTIONS";

/// Whether [`TRANSCRIPT_ACTIONS`] is on in this process.
pub fn transcript_actions() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var(TRANSCRIPT_ACTIONS).ok().as_deref() == Some("on"))
}
