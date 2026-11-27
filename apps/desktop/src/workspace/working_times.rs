//! Running clocks outlive the views that navigation replaces.

use std::collections::HashMap;
use std::time::Instant;

#[derive(Default)]
pub(super) struct WorkingTimes(HashMap<String, Clock>);

struct Clock {
    since: Instant,
    /// The completed turn before this work; 0 before the first turn. None while its history is unavailable.
    previous_turn: Option<i64>,
}

impl WorkingTimes {
    pub fn retain(&mut self, mut active: impl FnMut(&str) -> bool) {
        self.0.retain(|id, _| active(id));
    }

    pub fn since(&mut self, id: &str, previous_turn: Option<i64>, fallback: Instant, now: Instant) -> Instant {
        let clock = self.0.entry(id.to_string()).or_insert(Clock {
            since: fallback,
            previous_turn,
        });
        if let Some(turn) = previous_turn {
            if clock.previous_turn.is_some_and(|previous| previous != turn) {
                clock.since = now;
            }
            clock.previous_turn = Some(turn);
        }
        clock.since
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn navigation_keeps_each_sessions_elapsed_time() {
        let mut clocks = WorkingTimes::default();
        let start = Instant::now();
        assert_eq!(clocks.since("a", Some(10), start, start), start);
        let later = start + Duration::from_secs(30);
        assert_eq!(clocks.since("b", None, later, later), later);
        // The replacement view has not read its transcript yet.
        assert_eq!(clocks.since("a", None, later, later), start);
        assert_eq!(clocks.since("a", Some(10), later, later), start);
        assert_eq!(
            clocks.since(
                "b",
                None,
                later + Duration::from_secs(5),
                later + Duration::from_secs(5)
            ),
            later
        );
    }

    #[test]
    fn a_new_turn_while_away_gets_a_new_clock() {
        let mut clocks = WorkingTimes::default();
        let start = Instant::now();
        clocks.since("a", Some(10), start, start);
        let next = start + Duration::from_secs(60);
        assert_eq!(clocks.since("a", Some(20), start, next), next);
    }

    #[test]
    fn loading_history_keeps_the_clock_but_finishing_the_first_turn_resets_it() {
        let mut clocks = WorkingTimes::default();
        let start = Instant::now();
        clocks.since("a", None, start, start);
        let later = start + Duration::from_secs(30);
        assert_eq!(clocks.since("a", Some(0), later, later), start);
        assert_eq!(clocks.since("a", Some(15), start, later), later);
    }

    #[test]
    fn finished_sessions_drop_their_clocks() {
        let mut clocks = WorkingTimes::default();
        let start = Instant::now();
        clocks.since("a", None, start, start);
        clocks.since("b", None, start, start);
        clocks.retain(|id| id == "b");
        let next = start + Duration::from_secs(60);
        assert_eq!(clocks.since("a", None, next, next), next);
        assert_eq!(clocks.since("b", None, next, next), start);
    }
}
