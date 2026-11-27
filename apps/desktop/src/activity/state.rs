//! Hover reveals temporarily. A click pins. Escape always retracts.
use std::time::{Duration, Instant};

pub const PEEK: Duration = Duration::from_millis(160);
pub const RETRACT: Duration = Duration::from_millis(450);

#[derive(Default, Debug)]
pub struct Reveal {
    pub open: bool,
    pinned: bool,
    pending: Option<(Instant, bool)>,
}

impl Reveal {
    pub fn hover(&mut self, hovered: bool, now: Instant) {
        self.pending = if hovered {
            Some((now + PEEK, true))
        } else if self.pinned {
            None
        } else {
            Some((now + RETRACT, false))
        };
    }

    pub fn toggle(&mut self) {
        self.pinned = !self.pinned;
        self.open = self.pinned;
        self.pending = None;
    }

    pub fn retract(&mut self) {
        self.pinned = false;
        self.open = false;
        self.pending = None;
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let Some((at, open)) = self.pending else { return false };
        if now < at {
            return false;
        }
        self.pending = None;
        let changed = self.open != open;
        self.open = open;
        changed
    }
}
