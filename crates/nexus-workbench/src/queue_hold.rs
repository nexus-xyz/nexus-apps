//! The queue holds while the person rewrites a queued message. Edit on a queued row holds it (`Workbench::queue_hold`);
//! Save (`queue_edit`) or Cancel lets it go. While any queued message is being rewritten, a turn that ends starts no
//! queued message: the run pauses with its queue intact, shown as idle, so the old text never goes out mid-edit. Once
//! no message is being rewritten, or the person sends one now, the first queued message starts as it would have at
//! the turn's end.
//!
//! A paused run has no turn under way: a follow-up sent meanwhile waits in the queue behind the others, an Interrupt
//! has nothing to stop, and a linked session's results wait as they do behind any queued message.

use std::time::Instant;

use crate::event::WbEvent;
use crate::{Live, LiveState, Result, Workbench};

/// Whether the turn that just ended leaves the queue paused: a queued message waits, and the person is rewriting one.
pub(crate) fn parks(state: &LiveState) -> bool {
    state.stop.is_none()
        && !state.done
        && state.turns.front().is_some_and(|turn| turn.held.is_some())
        && state.turns.iter().any(|turn| turn.editing)
}

/// Moves the queued turn at `at` to the front of a paused queue, its rewrite dropped: the person sent it as it is.
pub(crate) fn to_front(state: &mut LiveState, at: usize) {
    if let Some(mut turn) = state.turns.remove(at) {
        turn.editing = false;
        state.turns.push_front(turn);
    }
}

impl Workbench {
    /// A paused queue moves on: its first message starts, unless a message is still being rewritten (`force`: the
    /// person sent the first one now). With nothing left in it, the run is idle.
    pub(crate) fn resume_queue(&self, thread_id: &str, live: &Live, force: bool) -> Result<()> {
        let (turn_id, first) = {
            let mut state = live.state();
            if !state.paused || state.done || state.stop.is_some() {
                return Ok(());
            }
            if !force && state.turns.iter().any(|turn| turn.editing) {
                return Ok(());
            }
            state.paused = false;
            let Some(turn) = state.turns.front_mut() else {
                drop(state);
                return self.refresh_status(thread_id, live);
            };
            turn.started = Instant::now();
            let (turn_id, seq, held, person) = (turn.id.clone(), turn.seq, turn.held.take(), turn.person);
            // The person's message begins its turn, as at a turn's end: an Interrupt since it was sent holds no longer.
            if person {
                self.person_turn_begins(thread_id);
            }
            let first = held.is_some_and(|say| state.line_up(seq, say));
            (turn_id, first)
        };
        // Sent even when the log write fails: left at the front of the line, it would hold every later message back.
        let started = self
            .append(thread_id, WbEvent::TurnStart { turn_id })
            .and_then(|_| self.refresh_status(thread_id, live));
        let sent = if first {
            self.send_lined_up(thread_id, live)
        } else {
            Ok(())
        };
        started?;
        sent
    }
}
