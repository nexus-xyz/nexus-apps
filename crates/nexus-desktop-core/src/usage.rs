//! Whether this install sends usage data: the activity heartbeat, error counts, page load failures and the plan tier
//! (PRIVACY.md, "Usage data"). Off until the person turns it on. Every send point reads [`allowed`];
//! the shell sets it at start and when the setting changes.

use std::sync::atomic::{AtomicBool, Ordering};

/// Off by default: a shell that never decides sends nothing.
static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Turns usage data off from the environment, whatever the setting says: `NEXUS_TELEMETRY=off` (any value but `on`
/// or `1`). It never turns usage data on.
pub const TELEMETRY_VAR: &str = "NEXUS_TELEMETRY";
/// The cross-tool convention (consoledonottrack.com): any value but empty or `0` turns usage data off.
pub const DO_NOT_TRACK_VAR: &str = "DO_NOT_TRACK";

pub fn allowed() -> bool {
    ALLOWED.load(Ordering::Relaxed)
}

pub fn set(on: bool) {
    ALLOWED.store(on, Ordering::Relaxed);
}

/// Whether usage data goes, from the person's choice in Settings (None: never asked), the environment (`env` reads a
/// variable) and whether this is an official release build (a build from source never sends).
pub fn decide(choice: Option<bool>, env: impl Fn(&str) -> Option<String>, release_build: bool) -> bool {
    // A variable set to an empty value counts as unset.
    let read = |name: &str| {
        env(name)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
    };
    let telemetry_off = read(TELEMETRY_VAR).is_some_and(|value| value != "on" && value != "1");
    let do_not_track = read(DO_NOT_TRACK_VAR).is_some_and(|value| value != "0");
    release_build && choice == Some(true) && !telemetry_off && !do_not_track
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn only_a_yes_on_a_release_build_sends() {
        assert!(decide(Some(true), no_env, true));
        assert!(!decide(None, no_env, true), "never asked is no");
        assert!(!decide(Some(false), no_env, true));
        assert!(!decide(Some(true), no_env, false), "a build from source never sends");
    }

    #[test]
    fn the_environment_turns_it_off_over_a_yes() {
        assert!(!decide(Some(true), env_of(&[(TELEMETRY_VAR, "off")]), true));
        assert!(!decide(Some(true), env_of(&[(TELEMETRY_VAR, "OFF")]), true));
        assert!(!decide(Some(true), env_of(&[(DO_NOT_TRACK_VAR, "1")]), true));
        assert!(!decide(Some(true), env_of(&[(DO_NOT_TRACK_VAR, "true")]), true));
    }

    #[test]
    fn the_environment_never_turns_it_on() {
        assert!(!decide(None, env_of(&[(TELEMETRY_VAR, "on")]), true));
        assert!(!decide(Some(false), env_of(&[(TELEMETRY_VAR, "on")]), true));
        assert!(decide(Some(true), env_of(&[(TELEMETRY_VAR, "on")]), true));
    }

    #[test]
    fn an_empty_or_zero_do_not_track_is_not_set() {
        assert!(decide(Some(true), env_of(&[(DO_NOT_TRACK_VAR, "")]), true));
        assert!(decide(Some(true), env_of(&[(DO_NOT_TRACK_VAR, "0")]), true));
        assert!(decide(Some(true), env_of(&[(TELEMETRY_VAR, " ")]), true));
    }

    #[test]
    fn any_other_telemetry_value_is_off() {
        for value in ["0", "false", "no", "disabled", " off "] {
            let pairs: &'static [(&'static str, &'static str)] = Box::leak(Box::new([(TELEMETRY_VAR, value)]));
            assert!(!decide(Some(true), env_of(pairs), true), "{value:?}");
        }
    }
}
