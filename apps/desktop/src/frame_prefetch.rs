//! Where to load a page, asked for before the click. A pointer resting on an artifact's card starts the web engine,
//! and in the same moment asks Gateway where the page lives, so the click has the answer and only loads the page.
//!
//! The answer is a signed address that serves the page for a few minutes (`PageFrame`). One answer is kept for
//! [`KEEP`], and the click that uses it takes it out: a later open asks again. Another sign-in or organization clears
//! them all ([`Prefetched::clear`]), and an answer still on its way then is dropped.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long an answer stays usable. Gateway's signed address lasts minutes; this stays well inside them.
const KEEP: Duration = Duration::from_secs(60);

/// The page a card names: its id and version.
pub type Key = (String, u32);

/// What Gateway answered: the address, the title and the `page@version` the address serves (`PageFrame`).
pub type Answer = (String, Option<String>, Option<String>);

enum Slot {
    /// Asked at this time, not answered yet.
    Asking(Instant),
    /// Answered at this time.
    Ready(Instant, Answer),
}

/// The answers asked for before a click, and which sign-in asked for them (`era`, one more each [`Prefetched::clear`]).
#[derive(Default)]
pub struct Prefetched {
    slots: HashMap<Key, Slot>,
    era: u64,
}

impl Prefetched {
    /// Whether to ask Gateway for `key` now, and the era to answer with. `None` when it is being asked or has a fresh
    /// answer already.
    pub fn ask(&mut self, key: &Key, now: Instant) -> Option<u64> {
        // An ask that never came back ends with the rest, so it cannot keep the page from being asked again.
        self.slots.retain(|_, slot| match slot {
            Slot::Asking(at) | Slot::Ready(at, _) => now.duration_since(*at) < KEEP,
        });
        if self.slots.contains_key(key) {
            return None;
        }
        self.slots.insert(key.clone(), Slot::Asking(now));
        Some(self.era)
    }

    /// Gateway answered for `key`, asked in `era`. A failed answer is dropped: the click asks for itself. So is an
    /// answer to an older sign-in's ask.
    pub fn answered(&mut self, key: &Key, era: u64, answer: Option<Answer>, now: Instant) {
        if era != self.era {
            return;
        }
        match answer {
            Some(answer) => {
                self.slots.insert(key.clone(), Slot::Ready(now, answer));
            }
            None => {
                self.slots.remove(key);
            }
        }
    }

    /// The fresh answer for `key`, taken out. `None` when none came, or it is still on its way: the click asks.
    pub fn take(&mut self, key: &Key, now: Instant) -> Option<Answer> {
        match self.slots.get(key) {
            Some(Slot::Ready(at, _)) if now.duration_since(*at) < KEEP => {}
            _ => return None,
        }
        match self.slots.remove(key) {
            Some(Slot::Ready(_, answer)) => Some(answer),
            _ => None,
        }
    }

    /// Another sign-in or organization: its pages' addresses are not the last one's. Drop every answer, and the
    /// answers still on their way.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.era += 1;
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn key() -> Key {
        ("pag_1".to_string(), 3)
    }

    fn answer() -> Answer {
        (
            "https://frames.example/p".to_string(),
            Some("Title".to_string()),
            Some("pag_1@3".to_string()),
        )
    }

    #[test]
    fn an_answer_asked_for_before_the_click_is_taken_by_it_once() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        prefetched.answered(&key(), 0, Some(answer()), now);
        assert_eq!(prefetched.take(&key(), now), Some(answer()));
        assert_eq!(prefetched.take(&key(), now), None);
    }

    #[test]
    fn a_page_is_asked_for_once_while_it_waits_or_is_fresh() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        assert_eq!(prefetched.ask(&key(), now), None);
        prefetched.answered(&key(), 0, Some(answer()), now);
        assert_eq!(prefetched.ask(&key(), now), None);
    }

    #[test]
    fn a_click_before_the_answer_asks_for_itself() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        assert_eq!(prefetched.take(&key(), now), None);
    }

    #[test]
    fn a_failed_answer_leaves_the_page_to_ask_again() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        prefetched.answered(&key(), 0, None, now);
        assert_eq!(prefetched.ask(&key(), now), Some(0));
    }

    #[test]
    fn an_old_answer_is_not_used_and_the_page_is_asked_again() {
        let now = Instant::now();
        let later = now + KEEP + Duration::from_secs(1);
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        prefetched.answered(&key(), 0, Some(answer()), now);
        assert_eq!(prefetched.take(&key(), later), None);
        assert_eq!(prefetched.ask(&key(), later), Some(0));
    }

    #[test]
    fn another_version_is_another_page() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        prefetched.answered(&key(), 0, Some(answer()), now);
        assert_eq!(prefetched.take(&("pag_1".to_string(), 4), now), None);
    }

    #[test]
    fn another_sign_in_drops_the_answers_and_those_on_their_way() {
        let now = Instant::now();
        let mut prefetched = Prefetched::default();
        assert_eq!(prefetched.ask(&key(), now), Some(0));
        prefetched.answered(&key(), 0, Some(answer()), now);
        prefetched.clear();
        assert_eq!(prefetched.take(&key(), now), None, "an answer for the last sign-in");
        let other = ("pag_2".to_string(), 1);
        assert_eq!(prefetched.ask(&other, now), Some(1));
        prefetched.clear();
        assert_eq!(prefetched.ask(&other, now), Some(2));
        prefetched.answered(&other, 1, Some(answer()), now);
        assert_eq!(
            prefetched.take(&other, now),
            None,
            "the last sign-in's ask, answered late"
        );
    }
}
