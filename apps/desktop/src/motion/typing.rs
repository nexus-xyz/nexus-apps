//! The typing bubble's three dots, as brand v5 draws them: three 6 px dots, 4 px apart, each on the board's
//! `nx-pulse` (2.4 s, ease-in-out, down to 45% and 80% size at the middle and back), 0.2 s after the one before. It is
//! its own small view, so its frames redraw only the dots and not the thread around them (AGENTS.md render rule 5),
//! at 30 frames a second. Under less motion the dots stand still and full, as the board's reduced motion leaves them.

use std::time::Duration;

use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Animation, AnimationExt as _, Context, Div, Hsla, IntoElement, Render, Window, div, px};

/// One cycle of a dot (`nx-pulse`).
pub const CYCLE: Duration = Duration::from_millis(2400);
/// How far apart the dots start: 0.2 s of the cycle.
const STAGGER: f32 = 0.2 / 2.4;
/// A dot's diameter, and the space between dots.
pub const DOT: f32 = 6.;
const GAP: f32 = 4.;
/// The middle of a pulse: its opacity and its size.
const DIM: f32 = 0.45;
const SMALL: f32 = 0.8;

/// CSS `ease-in-out`.
fn in_out(t: f32) -> f32 {
    super::cubic_bezier(0.42, 0., 0.58, 1., t)
}

/// How far dot `dot` (0 to 2) is into its pulse at `phase` (0 to 1) of the cycle: 0 at rest, 1 at the middle.
pub fn depth(dot: usize, phase: f32) -> f32 {
    let own = (phase - dot as f32 * STAGGER).rem_euclid(1.);
    if own < 0.5 {
        in_out(own * 2.)
    } else {
        1. - in_out(own * 2. - 1.)
    }
}

/// A dot `depth` into its pulse, in a 6 px slot so the row does not move as it shrinks.
fn dot(color: Hsla, depth: f32) -> Div {
    let size = DOT * (1. - (1. - SMALL) * depth);
    div().size(px(DOT)).flex().items_center().justify_center().child(
        div()
            .size(px(size))
            .rounded_full()
            .bg(color)
            .opacity(1. - (1. - DIM) * depth),
    )
}

/// The dots, in the board's quiet ink (`pages::agent::quiet_ink`).
#[derive(Default)]
pub struct TypingDots;

impl Render for TypingDots {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = cx
            .try_global::<crate::pages::Theme>()
            .map_or(gpui_kit::hsla(0., 0., 0.5, 1.), |theme| {
                crate::pages::agent::quiet_ink(theme.0)
            });
        let row = h_flex().gap(px(GAP));
        if cx.reduce_motion() {
            return row.children((0..3).map(|_| dot(color, 0.))).into_any_element();
        }
        // One clock for the three dots: one frame request each frame, not three.
        row.with_animation(
            "typing-dots",
            Animation::new(CYCLE).repeat().with_max_fps(super::LOOP_FPS),
            move |row, phase| row.children((0..3).map(|at| dot(color, depth(at, phase)))),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn each_dot_pulses_in_turn_two_tenths_of_a_second_apart() {
        assert!(depth(0, 0.).abs() < 1e-3, "the first dot starts at rest");
        // The first dot is deepest at the middle of its cycle; the next ones 0.2 s and 0.4 s later.
        assert!((depth(0, 0.5) - 1.).abs() < 1e-3);
        assert!((depth(1, 0.5 + STAGGER) - 1.).abs() < 1e-3);
        assert!((depth(2, 0.5 + 2. * STAGGER) - 1.).abs() < 1e-3);
        assert!(depth(1, 0.5) < 1. && depth(2, 0.5) < depth(1, 0.5));
        // The cycle loops without a jump.
        for at in 0..3 {
            assert!((depth(at, 0.) - depth(at, 0.9999)).abs() < 0.01);
        }
    }

    fn frames(reduced: bool, cx: &mut gpui_kit::TestAppContext) -> usize {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(reduced));
        let (dots, cx) = cx.add_window_view(|_, _| TypingDots);
        let asks = Rc::new(Cell::new(0));
        let count = asks.clone();
        cx.update(|_, cx| cx.observe(&dots, move |_, _| count.set(count.get() + 1)).detach());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = asks.get();
        for _ in 0..1000 {
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        asks.get() - before
    }

    #[gpui_kit::test]
    fn the_dots_redraw_at_thirty_frames_a_second(cx: &mut gpui_kit::TestAppContext) {
        let asked = frames(false, cx);
        assert!((25..=32).contains(&asked), "{asked} frames in a second");
    }

    #[gpui_kit::test]
    fn under_less_motion_the_dots_stand_still(cx: &mut gpui_kit::TestAppContext) {
        assert_eq!(frames(true, cx), 0);
    }
}
