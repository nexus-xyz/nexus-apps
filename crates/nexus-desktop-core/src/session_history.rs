//! Local conversation discovery is opt-in. Check before resolving any agent home or opening a transcript.

use nexus_client::local_sessions::{self, LocalSession};
use std::path::Path;

use crate::{settings::Settings, terminal_window};

pub const DISABLED: &str = "Local conversation history is off. Enable it in Settings > Privacy.";

pub fn enabled(settings: &Settings) -> bool {
    settings.local_session_history == Some(true)
}

pub fn list(settings: &Settings, env: &dyn Fn(&str) -> Option<String>, limit: usize) -> Vec<LocalSession> {
    if !enabled(settings) {
        return Vec::new();
    }
    local_sessions::list(env, limit)
}

pub fn resume(
    settings: &Settings,
    dir: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    agent: &str,
    id: &str,
) -> Result<(), String> {
    if !enabled(settings) {
        return Err(DISABLED.into());
    }
    terminal_window::resume(dir, env, agent, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_false_and_null_never_resolve_agent_homes_or_read_conversations() {
        for json in [
            "{}",
            r#"{"localSessionHistory":false}"#,
            r#"{"localSessionHistory":null}"#,
        ] {
            let settings: Settings = serde_json::from_str(json).expect("privacy setting reads");
            let env = |_: &str| -> Option<String> { panic!("must not inspect the computer") };
            assert!(list(&settings, &env, 50).is_empty());
            assert_eq!(
                resume(&settings, Path::new("unused"), &env, "codex", "unused"),
                Err(DISABLED.into())
            );
        }
    }

    #[test]
    fn the_explicit_choice_round_trips_and_can_be_revoked() {
        let mut settings: Settings = serde_json::from_str(r#"{"localSessionHistory":true}"#).expect("opt-in reads");
        assert!(enabled(&settings));
        assert_eq!(
            serde_json::to_value(&settings).expect("settings serialize")["localSessionHistory"],
            true
        );
        settings.local_session_history = Some(false);
        assert!(!enabled(&settings));
    }
}
