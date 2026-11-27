//! Interruptible folder transitions use the shared Nexus durations and ease-out curve.

use std::time::{Duration, Instant};

use gpui_kit::{Animation, AnimationExt as _, AnyElement, Div, IntoElement, SharedString};

use crate::motion;

#[derive(Clone, Copy)]
pub(super) struct Transition {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    generation: usize,
}

impl Default for Transition {
    fn default() -> Self {
        Self::settled(0.)
    }
}

impl Transition {
    pub(super) fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            started: Instant::now(),
            duration: motion::UI,
            generation: 0,
        }
    }

    fn value(self, now: Instant) -> f32 {
        let progress = now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.to - self.from) * motion::ease_out(progress)
    }

    pub(super) fn retarget(&mut self, to: f32, duration: Duration) {
        self.retarget_at(to, duration, Instant::now());
    }

    fn retarget_at(&mut self, to: f32, duration: Duration, now: Instant) {
        if self.to == to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.started = now;
        self.duration = duration;
        self.generation += 1;
    }

    pub(super) fn visible(self) -> bool {
        self.to > 0. || self.value(Instant::now()) > 0.
    }

    /// Whether the value is still on its way: [`draw`](Self::draw) animates it.
    pub(super) fn moving(self) -> bool {
        self.moving_at(Instant::now())
    }

    fn moving_at(self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.started) < self.duration
    }

    pub(super) fn draw(self, element: Div, id: String, apply: impl Fn(Div, f32) -> Div + 'static) -> AnyElement {
        if !self.moving() {
            return apply(element, self.to).into_any_element();
        }
        // Read the same clock when reversing; a new animation starts at the current position.
        element
            .with_animation(
                (SharedString::from(id), self.generation),
                Animation::new(self.duration),
                move |element, delta| {
                    // GPUI sends the endpoint immediately when the person requests reduced motion.
                    apply(
                        element,
                        if delta >= 1. {
                            self.to
                        } else {
                            self.value(Instant::now())
                        },
                    )
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_preserves_position_and_finishes_at_the_new_target() {
        let mut transition = Transition::settled(0.);
        let start = Instant::now();
        transition.retarget_at(1., motion::UI, start);
        let reverse = start + motion::UI / 2;
        let before = transition.value(reverse);
        assert!(before > 0. && before < 1.);
        transition.retarget_at(0., motion::UI, reverse);
        assert!((transition.value(reverse) - before).abs() < 0.0001);
        assert_eq!(transition.value(reverse + motion::UI), 0.);
    }

    #[test]
    fn a_transition_moves_only_until_its_duration_ends() {
        let mut transition = Transition::settled(1.);
        let start = Instant::now();
        assert!(!transition.moving_at(start));
        transition.retarget_at(0., motion::UI_LONG, start);
        assert!(transition.moving_at(start + motion::UI_LONG / 2));
        assert!(!transition.moving_at(start + motion::UI_LONG));
    }

    #[test]
    fn repeated_target_does_not_restart_motion() {
        let mut transition = Transition::settled(1.);
        let start = Instant::now();
        transition.retarget_at(0., motion::UI_LONG, start);
        transition.retarget_at(0., motion::UI_LONG, start + motion::UI_LONG / 2);
        assert_eq!(transition.started, start);
        assert_eq!(transition.generation, 1);
        assert_eq!(transition.value(start + motion::UI_LONG), 0.);
    }
}
