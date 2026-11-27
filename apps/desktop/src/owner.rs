//! One owner at a time: one process owns the Code sessions store and the settings
//! writes. The lock is `nexus-workbench`'s `StoreOwner` (`run/workbench.lock` in the shared data folder).
//!
//! The Nexus app takes that lock at launch and keeps it until it quits; with "Keep agents running when Nexus
//! quits" on, its background service holds it instead, until it stops. Neither can be asked to let go: a second
//! launch of that app only brings its window forward. So this build never takes the lock from it. "Take over"
//! asks the person to quit the Nexus app, then tries the lock every [`POLL`] for [`TAKE_OVER_FOR`].
//!
//! [`Owner`] is that state, and nothing else: the workspace tries the lock and reports here. [`owns_store`] is the
//! shared answer for code that writes the shared settings: write only while it is true.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often the lock is tried while taking over.
pub const POLL: Duration = Duration::from_secs(1);
/// How long taking over waits for the Nexus app to quit.
pub const TAKE_OVER_FOR: Duration = Duration::from_secs(60);

/// Why the store's lock was not taken.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The Nexus app or its background service holds it.
    Busy,
    /// The store or the lock could not be opened, for a person.
    Failed(String),
}

/// Who owns the Code sessions store, as this build sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Owner {
    /// Not tried yet: signed out, or no organization yet.
    Unknown,
    /// This build holds the lock: it runs Code sessions and may write settings.
    Here,
    /// The Nexus app holds it: this build reads only. `waited` is true after a take-over ran out of time.
    Elsewhere { waited: bool },
    /// The person asked to take over: the lock is tried until `until`.
    TakingOver { until: Instant },
    /// The store could not be opened.
    Failed(String),
}

static OWNS_STORE: AtomicBool = AtomicBool::new(false);

/// Whether this build owns the store, and so may write the shared settings. This build writes only its own settings
/// file today; the writes to the shared `settings.json` will check this first.
#[allow(dead_code, reason = "the shared settings writes are its first caller")]
pub fn owns_store() -> bool {
    OWNS_STORE.load(Ordering::Relaxed)
}

impl Owner {
    /// The state after one try of the lock, at `now`.
    pub fn tried(&self, result: Result<(), Refused>, now: Instant) -> Owner {
        let next = match (self, result) {
            (_, Ok(())) => Owner::Here,
            (Owner::TakingOver { until }, Err(Refused::Busy)) if now < *until => Owner::TakingOver { until: *until },
            (Owner::TakingOver { .. }, Err(Refused::Busy)) => Owner::Elsewhere { waited: true },
            (_, Err(Refused::Busy)) => Owner::Elsewhere { waited: false },
            (_, Err(Refused::Failed(error))) => Owner::Failed(error),
        };
        next.record();
        next
    }

    /// The person pressed "Take over" at `now`.
    pub fn take_over(&self, now: Instant) -> Owner {
        match self {
            Owner::Elsewhere { .. } => Owner::TakingOver {
                until: now + TAKE_OVER_FOR,
            },
            other => other.clone(),
        }
    }

    /// Whether the lock should be tried again on the next [`POLL`].
    pub fn polling(&self) -> bool {
        matches!(self, Owner::TakingOver { .. })
    }

    /// Whether to offer "Take over".
    pub fn can_take_over(&self) -> bool {
        matches!(self, Owner::Elsewhere { .. })
    }

    /// The line where Code sessions start, when this build does not run them.
    pub fn note(&self) -> Option<String> {
        match self {
            Owner::Here | Owner::Unknown => None,
            Owner::Elsewhere { waited: false } => {
                Some("The Nexus app is running Code sessions. They are read-only here.".into())
            }
            Owner::Elsewhere { waited: true } => Some(
                "The Nexus app still holds the Code sessions. Quit it. If it keeps agents running in the background, \
                 turn that off in its settings first."
                    .into(),
            ),
            Owner::TakingOver { .. } => {
                Some("Quit the Nexus app. Code sessions move here as soon as it closes.".into())
            }
            Owner::Failed(error) => Some(error.clone()),
        }
    }

    fn record(&self) {
        OWNS_STORE.store(matches!(self, Owner::Here), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_lock_makes_this_build_the_owner() {
        let owner = Owner::Unknown.tried(Ok(()), Instant::now());
        assert_eq!(owner, Owner::Here);
        assert_eq!(owner.note(), None);
        assert!(!owner.can_take_over());
    }

    #[test]
    fn a_held_lock_makes_this_build_read_only_and_offers_to_take_over() {
        let owner = Owner::Unknown.tried(Err(Refused::Busy), Instant::now());
        assert_eq!(owner, Owner::Elsewhere { waited: false });
        assert!(owner.can_take_over());
        assert!(!owner.polling());
        assert!(owner.note().is_some_and(|note| note.contains("read-only")));
    }

    #[test]
    fn taking_over_waits_while_held_and_owns_once_released() {
        let start = Instant::now();
        let owner = Owner::Elsewhere { waited: false }.take_over(start);
        assert!(owner.polling());
        assert!(!owner.can_take_over());
        let owner = owner.tried(Err(Refused::Busy), start + POLL);
        assert!(owner.polling(), "still waiting for the Nexus app to quit");
        let owner = owner.tried(Ok(()), start + POLL * 2);
        assert_eq!(owner, Owner::Here);
    }

    #[test]
    fn taking_over_gives_up_after_its_time_and_offers_again() {
        let start = Instant::now();
        let owner = Owner::Elsewhere { waited: false }.take_over(start);
        let owner = owner.tried(Err(Refused::Busy), start + TAKE_OVER_FOR);
        assert_eq!(owner, Owner::Elsewhere { waited: true });
        assert!(owner.can_take_over());
    }

    #[test]
    fn take_over_does_nothing_unless_another_app_owns_the_store() {
        let now = Instant::now();
        assert_eq!(Owner::Here.take_over(now), Owner::Here);
        assert_eq!(Owner::Unknown.take_over(now), Owner::Unknown);
        let failed = Owner::Failed("no folder".into());
        assert_eq!(failed.take_over(now), failed);
    }

    #[test]
    fn a_failure_is_shown_as_is() {
        let owner = Owner::Unknown.tried(Err(Refused::Failed("Could not open the store.".into())), Instant::now());
        assert_eq!(owner.note().as_deref(), Some("Could not open the store."));
        assert!(!owner.can_take_over());
    }
}
