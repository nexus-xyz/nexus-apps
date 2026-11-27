//! The welcome screen's mark. It spins in: one turn that slows
//! to rest on its drawn angle over [`SPIN_IN`] on the ease-out curve, fading in and growing from 60 %. Then it turns
//! once in [`TURN`], linear, while the screen waits for the person.
//!
//! The brand keeps the mark still at rest and turns it only while Nexus works; the welcome screen is the one exception
//! the design asks for. The mark is its own small view, so its frames (30 a second) redraw only the mark, never the
//! workspace (the render rules in `AGENTS.md`). Under less motion it stands still at its drawn angle and asks for no
//! frames.
//!
//! A new arrival (a sign-out, a cold start signed out) spins it in again: the workspace bumps the shared [`Arrival`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    Animation, AnimationExt as _, Context, Hsla, IntoElement, ParentElement as _, Render, Styled as _, Transformation,
    Window, div, px, radians, size, svg,
};

/// The spin-in: one turn to the drawn angle.
pub const SPIN_IN: Duration = Duration::from_millis(1200);
/// One slow turn after the spin-in (`motion.turn`).
pub const TURN: Duration = Duration::from_secs(14);
/// How far back the spin-in starts, in degrees. The mark repeats every 60°, so 300° lands on the drawn angle.
const SPIN_FROM: f32 = 300.;
/// The mark's size when the spin-in starts, against its own.
const FROM_SCALE: f32 = 0.6;

/// When the mark last arrived, and which arrival it is: the workspace restarts it without a context.
#[derive(Clone)]
pub struct Arrival(Rc<Cell<(Instant, usize)>>);

impl Arrival {
    pub fn new() -> Arrival {
        Arrival(Rc::new(Cell::new((Instant::now(), 0))))
    }

    /// The mark arrives again: it spins in once more.
    pub fn again(&self) {
        let (_, run) = self.0.get();
        self.0.set((Instant::now(), run + 1));
    }

    fn get(&self) -> (Instant, usize) {
        self.0.get()
    }
}

/// The mark's angle (degrees), scale and light, `elapsed` after it arrived.
pub fn pose(elapsed: Duration) -> (f32, f32, f32) {
    if elapsed < SPIN_IN {
        let t = super::ease_out(elapsed.as_secs_f32() / SPIN_IN.as_secs_f32());
        let light = (elapsed.as_secs_f32() / SPIN_IN.as_secs_f32() * 2.5).min(1.);
        (-SPIN_FROM * (1. - t), FROM_SCALE + (1. - FROM_SCALE) * t, light)
    } else {
        let turned = (elapsed - SPIN_IN).as_secs_f32() / TURN.as_secs_f32();
        (turned.fract() * 360., 1., 1.)
    }
}

/// The spinning mark, `side` pixels, in the window's foreground ink.
pub struct SpinningMark {
    side: f32,
    arrival: Arrival,
}

impl SpinningMark {
    pub fn new(side: f32, arrival: Arrival) -> SpinningMark {
        SpinningMark { side, arrival }
    }
}

/// The mark's layers, turned `angle` degrees and scaled, in `color` at `light`.
fn layers(side: f32, color: Hsla, angle: f32, scale: f32, light: f32) -> gpui_kit::Div {
    let count = crate::assets::mark_layers().len();
    div()
        .relative()
        .size(px(side))
        .flex_none()
        .children((0..count).map(|layer| {
            svg()
                .path(crate::assets::mark_layer(layer))
                .absolute()
                .top_0()
                .left_0()
                .size(px(side))
                .text_color(color.opacity(light))
                .with_transformation(
                    Transformation::rotate(radians(angle.to_radians())).with_scaling(size(scale, scale)),
                )
        }))
}

impl Render for SpinningMark {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = cx
            .try_global::<crate::pages::Theme>()
            .map(|theme| theme.0.foreground)
            .unwrap_or_else(gpui_kit::white);
        let side = self.side;
        if cx.reduce_motion() {
            return layers(side, color, 0., 1., 1.).into_any_element();
        }
        let (since, run) = self.arrival.get();
        div()
            .size(px(side))
            .flex_none()
            .with_animation(
                ("welcome-spin", run),
                Animation::new(TURN).repeat().with_max_fps(super::LOOP_FPS),
                move |frame, _| {
                    let (angle, scale, light) = pose(since.elapsed());
                    frame.child(layers(side, color, angle, scale, light))
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn the_spin_in_starts_small_and_back_and_lands_on_the_drawn_angle() {
        let (angle, scale, light) = pose(Duration::ZERO);
        assert_eq!(angle, -SPIN_FROM);
        assert_eq!(scale, FROM_SCALE);
        assert_eq!(light, 0.);
        let (angle, scale, light) = pose(SPIN_IN);
        assert!(angle.abs() < 1e-3);
        assert_eq!((scale, light), (1., 1.));
    }

    #[test]
    fn after_the_spin_in_it_turns_once_in_fourteen_seconds() {
        let (half, _, _) = pose(SPIN_IN + TURN / 2);
        assert!((half - 180.).abs() < 0.1, "{half}");
        let (whole, _, _) = pose(SPIN_IN + TURN);
        assert!(whole.abs() < 0.1 || (whole - 360.).abs() < 0.1, "{whole}");
    }

    /// A window with the mark, and a count of the times it asked to draw again.
    fn mark(cx: &mut gpui_kit::TestAppContext) -> (Rc<Cell<usize>>, &mut gpui_kit::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_, _| SpinningMark::new(24., Arrival::new()));
        let asks = Rc::new(Cell::new(0));
        let count = asks.clone();
        cx.update(|_, cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)).detach());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (asks, cx)
    }

    fn asks_over(asks: &Cell<usize>, span: Duration, cx: &mut gpui_kit::VisualTestContext) -> usize {
        let before = asks.get();
        let step = Duration::from_millis(1);
        let mut elapsed = Duration::ZERO;
        while elapsed < span {
            cx.executor().advance_clock(step);
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            elapsed += step;
        }
        asks.get() - before
    }

    #[gpui_kit::test]
    fn the_mark_redraws_at_thirty_frames_a_second(cx: &mut gpui_kit::TestAppContext) {
        let (asks, cx) = mark(cx);
        let frames = asks_over(&asks, Duration::from_secs(1), cx);
        assert!((27..=31).contains(&frames), "{frames} frames in a second");
    }

    #[gpui_kit::test]
    fn under_less_motion_the_mark_stands_still(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (asks, cx) = mark(cx);
        assert_eq!(asks_over(&asks, Duration::from_secs(1), cx), 0);
    }
}
