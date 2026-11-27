//! Starts the web engine when the pointer rests on something that opens a page, not when it only passes over.
//! A card scrolling under a still pointer also counts as a hover, and the engine holds about 200MB. So a hover starts
//! the engine only after the pointer stays [`DWELL`], as instant.page does before it fetches a link. A click or an
//! open does not wait: it calls [`super::warm`] itself.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::*;

/// How long the pointer rests before the engine starts: instant.page's default.
pub(super) const DWELL: Duration = Duration::from_millis(65);

/// Which hover is the current one. Every change of the pointer's place makes a new one, which ends the one before.
pub(super) struct Dwell(AtomicU64);

impl Dwell {
    pub(super) const fn new() -> Dwell {
        Dwell(AtomicU64::new(0))
    }

    /// The pointer came onto a target: a new hover, which the caller starts the timer for.
    pub(super) fn enter(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The pointer left: the hover that waits does not fire.
    pub(super) fn leave(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether hover `id` is still the current one: the pointer did not leave, and none came after it.
    pub(super) fn resting(&self, id: u64) -> bool {
        self.0.load(Ordering::SeqCst) == id
    }
}

static HOVER: Dwell = Dwell::new();

/// A target's hover changed. Once the pointer has rested on it for [`DWELL`], the engine starts.
pub fn warm_on_hover(hovered: bool, cx: &mut App) {
    warm_on_hover_then(hovered, cx, || {});
}

/// [`warm_on_hover`], and once the pointer has rested, `then` runs too. A card uses it to ask Gateway where its page
/// is, in the same moment the engine starts.
pub fn warm_on_hover_then(hovered: bool, cx: &mut App, then: impl FnOnce() + 'static) {
    after_dwell(hovered, cx, move || {
        super::warm();
        then();
    });
}

/// Run only after the current hover rests; preparation shares the same cancellation as warming.
pub(super) fn after_dwell(hovered: bool, cx: &mut App, then: impl FnOnce() + 'static) {
    if !hovered {
        HOVER.leave();
        return;
    }
    let id = HOVER.enter();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(DWELL).await;
        if HOVER.resting(id) {
            then();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn a_pointer_that_rests_starts_the_engine() {
        let dwell = Dwell::new();
        let hover = dwell.enter();
        assert!(dwell.resting(hover));
    }

    #[test]
    fn a_pointer_that_leaves_before_the_dwell_does_not() {
        let dwell = Dwell::new();
        let hover = dwell.enter();
        dwell.leave();
        assert!(!dwell.resting(hover));
    }

    #[test]
    fn moving_to_the_next_target_restarts_the_dwell() {
        let dwell = Dwell::new();
        let first = dwell.enter();
        dwell.leave();
        let second = dwell.enter();
        assert!(!dwell.resting(first));
        assert!(dwell.resting(second));
        // Entering a second target with no leave in between also ends the first.
        let third = dwell.enter();
        assert!(!dwell.resting(second));
        assert!(dwell.resting(third));
    }
}
