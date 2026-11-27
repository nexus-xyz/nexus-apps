//! Device-local setup hints.
use serde::Serialize;
use serde_json::Value;

pub const SESSIONS: u64 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct State {
    pub sessions: u64,
    pub dismissed: bool,
}

impl State {
    pub fn read(value: &Value) -> Self {
        Self {
            sessions: value
                .get("sessions")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n > 0.)
                .map_or(0, |n| n.floor() as u64),
            dismissed: value.get("dismissed") == Some(&Value::Bool(true)),
        }
    }

    pub fn shown(self) -> bool {
        !self.dismissed && self.sessions < SESSIONS
    }

    pub fn note_started(&mut self) {
        if self.shown() {
            self.sessions += 1;
        }
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }
}

pub fn newcomer(chat_loaded: bool, code_loaded: bool, chats: usize, code_sessions: usize) -> bool {
    chat_loaded && code_loaded && chats == 0 && code_sessions == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hint_counts_only_the_first_three_offered_starts() {
        let mut state = State::default();
        for _ in 0..SESSIONS {
            assert!(state.shown());
            state.note_started();
        }
        assert!(!state.shown());
        state.note_started();
        assert_eq!(state.sessions, SESSIONS);
    }

    #[test]
    fn dismissal_stays_dismissed_and_unreadable_storage_shows_the_hint() {
        let mut state = State::default();
        state.dismiss();
        assert!(!State::read(&serde_json::to_value(state).expect("setup-line state fixture serializes")).shown());
        for value in [
            Value::Null,
            json!({"sessions":"invalid"}),
            json!({"sessions":-1,"dismissed":"true"}),
        ] {
            assert_eq!(State::read(&value), State::default());
        }
        assert_eq!(State::read(&json!({"sessions":2.9})).sessions, 2);
    }

    #[test]
    fn starters_wait_for_both_lists_and_never_show_for_existing_sessions() {
        assert!(newcomer(true, true, 0, 0));
        assert!(!newcomer(false, true, 0, 0));
        assert!(!newcomer(true, false, 0, 0));
        assert!(!newcomer(true, true, 1, 0));
        assert!(!newcomer(true, true, 0, 1));
    }
}
