//! The sidebar's session drag as a state machine, with no UI.
//!
//! ```text
//!          Lift                Move (inside, takes it)
//!   Idle ───────▶ Lifted ─────────────────────────────▶ Over { key, target }
//!    ▲              ▲  ◀─────────────────────────────────  │
//!    │              │    Move (out of `key`, or it would    │
//!    │              │    not take it there)                 │
//!    └──────────────┴──── Cancel ◀──────────────────────────┤
//!    ▲                                                      │
//!    └──────── Drop on `key`: Land(drag, target, plan) ◀────┘
//! ```
//!
//! Each target element has a `key` of its own. Several elements may offer the same target (a project's head and foot,
//! the rows of one family), and GPUI tells each of them of every move, in no set order. So only the element the
//! pointer is in may take the drag, and only that element may let it go: leaving one element never clears another's.
//! What a drop does is [`plan`]'s.

use super::session_drop::{Facts, Plan, Target, plan};

/// Where the drag is.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) enum Phase {
    #[default]
    Idle,
    /// A session is lifted, over no target that takes it.
    Lifted(Facts),
    /// Over element `key`, whose target takes the session.
    Over { drag: Facts, key: String, target: Target },
}

/// What happened to the drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Event {
    /// A session row was lifted.
    Lift(Facts),
    /// The pointer moved, `inside` element `key` or out of it, which offers `target`.
    Move { key: String, inside: bool, target: Target },
    /// The session was let go on element `key`.
    Drop { key: String },
    /// The drag ended with no drop (Esc, or let go outside every target).
    Cancel,
}

/// What a step asks of the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Step {
    /// Nothing on screen changes.
    Same,
    /// What the drag is over changed: draw again.
    Changed,
    /// The session landed: apply its plan.
    Land(Box<Landing>),
}

/// A drop that lands: the drag, the target it landed on, and what it does there.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Landing {
    pub drag: Facts,
    pub target: Target,
    pub plan: Plan,
}

impl Phase {
    /// The dragged session, while a drag goes on.
    pub fn drag(&self) -> Option<&Facts> {
        match self {
            Phase::Idle => None,
            Phase::Lifted(drag) | Phase::Over { drag, .. } => Some(drag),
        }
    }

    /// The target the drag is over, and its element's key.
    pub fn over(&self) -> Option<(&str, &Target)> {
        match self {
            Phase::Over { key, target, .. } => Some((key, target)),
            _ => None,
        }
    }

    /// Moves the machine on by `event`.
    pub fn step(&mut self, event: Event) -> Step {
        let next = match (&*self, event) {
            (_, Event::Lift(drag)) => Phase::Lifted(drag),
            (Phase::Idle, _) => return Step::Same,
            (_, Event::Cancel) => Phase::Idle,
            (phase, Event::Move { key, inside, target }) => {
                let drag = phase.drag().expect("a drag goes on").clone();
                let here = phase.over().is_some_and(|(over, _)| over == key);
                if inside && plan(&drag, &target).is_some() {
                    Phase::Over { drag, key, target }
                } else if here {
                    Phase::Lifted(drag)
                } else {
                    return Step::Same;
                }
            }
            (Phase::Over { drag, key, target }, Event::Drop { key: on }) if *key == on => {
                let landed = plan(drag, target).map(|plan| {
                    Step::Land(Box::new(Landing {
                        drag: drag.clone(),
                        target: target.clone(),
                        plan,
                    }))
                });
                *self = Phase::Idle;
                return landed.unwrap_or(Step::Changed);
            }
            (_, Event::Drop { .. }) => return Step::Same,
        };
        if next == *self {
            return Step::Same;
        }
        *self = next;
        Step::Changed
    }
}

#[cfg(test)]
#[path = "session_drag_machine_tests.rs"]
mod tests;
