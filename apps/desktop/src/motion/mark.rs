//! The F5 mark breathing, brand v5's thinking avatar (Nexus UI's `app/app/v5/agent-avatar.css`):
//! each of the mark's 11 layers dims to 35 % and returns, from the crossing out to the rim, 70 ms apart, while the mark
//! turns once in 14 s. When it stops it eases forward to the next sixth of a turn over 600 ms (`agent-avatar.ts`): the
//! mark is six-fold, so it rests as drawn, never turns back and travels at most a sixth. Under 24 px the layers breathe
//! as one piece. Under less motion it stands still at 60 %.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Element, ElementId, Global, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    ParentElement as _, Pixels, Styled as _, Transformation, Window, div, px, radians, svg,
};

/// One turn while breathing (`agent-avatar.css`'s `agent-avatar-turn`).
pub const TURN: Duration = Duration::from_millis(14_000);
/// The ease to rest once it stops (`agent-avatar.ts`'s `REST_MS`).
pub const REST: Duration = Duration::from_millis(600);
/// One breath of a layer (`motion.pulse`, 2.4 s), how dim it gets, and how far apart the layers start.
const BREATH: Duration = Duration::from_millis(2400);
const DIM: f32 = 0.35;
const STAGGER: Duration = Duration::from_millis(70);
/// The app mark's layers (`assets::mark_layers`), rim first.
const LAYERS: usize = 11;
/// Under this size the ripple cannot be seen, so the layers breathe as one.
const RIPPLE_FROM: f32 = 24.;
/// Under less motion the mark holds still at this opacity.
const REDUCED_OPACITY: f32 = 0.6;
/// The mark draws the same after a sixth of a turn.
const SIXTH: f32 = 1. / 6.;
/// A slow turn and a 2.4 s breath read smoothly at 30 frames a second, a fraction of the display's redraws.
const FPS: f32 = 30.;

/// `easing.settle` in `tokens.json`, `cubic-bezier(0, 0, 0.2, 1)`: the turning mark easing to rest.
fn settle(t: f32) -> f32 {
    super::cubic_bezier(0., 0., 0.2, 1., t)
}

/// CSS `ease-in-out`, `cubic-bezier(0.42, 0, 0.58, 1)` (`easing.in-out`): a breath.
fn in_out(t: f32) -> f32 {
    super::cubic_bezier(0.42, 0., 0.58, 1., t)
}

/// How lit layer `layer` (0 is the rim) is `elapsed` after the mark began to breathe, at `size` pixels: the CSS
/// keyframe `50% { opacity: 0.35 }` under `ease-in-out`, the crossing (the last layer) first and the rim last.
pub(crate) fn breath(layer: usize, elapsed: Duration, size: f32) -> f32 {
    let from_core = (LAYERS - 1).saturating_sub(layer) as u32;
    let delay = if size < RIPPLE_FROM {
        Duration::ZERO
    } else {
        STAGGER * from_core
    };
    let Some(into) = elapsed.checked_sub(delay) else {
        // Before its turn a layer is fully lit, as an animation before its delay.
        return 1.;
    };
    let phase = (into.as_secs_f32() / BREATH.as_secs_f32()).fract();
    if phase < 0.5 {
        1. - (1. - DIM) * in_out(phase * 2.)
    } else {
        DIM + (1. - DIM) * in_out(phase * 2. - 1.)
    }
}

/// Where the mark is, in turns (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Spin {
    Still(f32),
    Turning { from: f32, since: Instant },
    Resting { from: f32, to: f32, at: Instant },
}

impl Spin {
    /// The angle at `now`, and whether it still moves.
    pub(crate) fn angle(&self, now: Instant) -> (f32, bool) {
        match *self {
            Spin::Still(angle) => (angle, false),
            Spin::Turning { from, since } => {
                let turned = now.saturating_duration_since(since).as_secs_f32() / TURN.as_secs_f32();
                ((from + turned).fract(), true)
            }
            Spin::Resting { from, to, at } => {
                let t = now.saturating_duration_since(at).as_secs_f32() / REST.as_secs_f32();
                if t >= 1. {
                    (to.fract(), false)
                } else {
                    (from + (to - from) * settle(t), true)
                }
            }
        }
    }

    /// The spin at `now` when the mark should be `turning`: it starts from where it is, and stopping eases it forward to
    /// the next sixth.
    pub(crate) fn update(self, turning: bool, now: Instant) -> Spin {
        let (angle, moving) = self.angle(now);
        match (self, turning) {
            (Spin::Turning { .. }, true) => self,
            (_, true) => Spin::Turning {
                from: angle,
                since: now,
            },
            (Spin::Turning { .. }, false) => Spin::Resting {
                from: angle,
                to: next_rest(angle),
                at: now,
            },
            (Spin::Resting { .. }, false) if moving => self,
            (_, false) => Spin::Still(angle),
        }
    }
}

/// The next sixth of a turn at or ahead of `angle`.
pub(crate) fn next_rest(angle: f32) -> f32 {
    // A hair under a sixth counts as on it, so float error never sends the mark round another sixth.
    ((angle - 1e-4).max(0.) / SIXTH).ceil() * SIXTH
}

/// The F5 mark breathing and turning, `size` pixels in one ink. `id` keeps its angle and its breath across frames.
pub fn breathing_mark(id: impl Into<ElementId>, size: f32, color: Hsla) -> BreathingMark {
    BreathingMark {
        id: id.into(),
        size,
        color,
        breathing: true,
    }
}

/// The moment every breathing mark's motion counts from, the first time one is drawn in the app. A mark drawn anew (its
/// line rebuilt, a new summary) picks up the breath and the turn where they are, so the motion never restarts.
struct Epoch(Instant);

impl Global for Epoch {}

fn epoch(now: Instant, cx: &mut App) -> Instant {
    if !cx.has_global::<Epoch>() {
        cx.set_global(Epoch(now));
    }
    cx.global::<Epoch>().0
}

/// A new mark's spin: one that comes in breathing has turned since `epoch`, so it is where every breathing mark is.
pub(crate) fn first_spin(breathing: bool, epoch: Instant) -> Spin {
    if breathing {
        Spin::Turning { from: 0., since: epoch }
    } else {
        Spin::Still(0.)
    }
}

/// [`breathing_mark`]'s element. It can stand still (`breathing` false), easing to rest from where it turned to.
pub struct BreathingMark {
    id: ElementId,
    size: f32,
    color: Hsla,
    breathing: bool,
}

impl BreathingMark {
    /// Breathe (true) or stand still. A mark that stops eases forward to rest, so an avatar that is thinking only
    /// some of the time keeps one element and never jumps (the Assistant's, `pages/agent.rs` and its sidebar row).
    pub fn breathing(mut self, on: bool) -> Self {
        self.breathing = on;
        self
    }
}

/// What [`BreathingMark`] keeps across frames: its spin, when its breath began, and whether a throttled redraw is on
/// its way.
struct MarkState {
    spin: Spin,
    since: Instant,
    frame_pending: Rc<Cell<bool>>,
}

impl IntoElement for BreathingMark {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for BreathingMark {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        // The executor's clock, as GPUI's own animations read it, so a test can move time.
        let now = cx.background_executor().now();
        let breathing = self.breathing;
        let reduced = cx.reduce_motion();
        let epoch = epoch(now, cx);
        let (angle, moving, since, frame_pending) =
            window.with_element_state(id.unwrap(), |state: Option<MarkState>, _| {
                let mut state = state.unwrap_or_else(|| MarkState {
                    spin: first_spin(breathing && !reduced, epoch),
                    since: epoch,
                    frame_pending: Rc::new(Cell::new(false)),
                });
                state.spin = if reduced {
                    // Every loop stops: the mark stands as drawn.
                    Spin::Still(next_rest(state.spin.angle(now).0).fract())
                } else {
                    state.spin.update(breathing, now)
                };
                let (angle, moving) = state.spin.angle(now);
                let out = (angle, moving, state.since, state.frame_pending.clone());
                (out, state)
            });
        if moving {
            if breathing {
                // The breath and the slow turn redraw at 30 frames a second, as the shimmer does.
                if !frame_pending.get() {
                    frame_pending.set(true);
                    let view = window.current_view();
                    let interval = Duration::from_secs_f32(1. / FPS);
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(interval).await;
                            frame_pending.set(false);
                            cx.update(move |_, cx| cx.notify(view)).ok();
                        })
                        .detach();
                }
            } else {
                // The ease to rest is short and quick: every frame.
                window.request_animation_frame();
            }
        }
        let elapsed = now.saturating_duration_since(since);
        let turn = Transformation::rotate(radians(angle * std::f32::consts::TAU));
        let (size, color) = (self.size, self.color);
        // The layers stack in one frame, each lit on its own.
        let mut element = div()
            .relative()
            .size(px(size))
            .flex_none()
            .children((0..LAYERS).map(|layer| {
                let light = match (reduced, breathing) {
                    (true, true) => REDUCED_OPACITY,
                    (false, true) => breath(layer, elapsed, size),
                    (_, false) => 1.,
                };
                svg()
                    .path(crate::assets::mark_layer(layer))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size(px(size))
                    .text_color(color.opacity(light))
                    .with_transformation(turn)
            }))
            .into_any_element();
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn it_turns_once_in_fourteen_seconds() {
        let start = Instant::now();
        let spin = Spin::Still(0.).update(true, start);
        assert!(close(spin.angle(start + TURN / 4).0, 0.25));
        assert!(close(spin.angle(start + TURN + TURN / 2).0, 0.5));
        assert!(spin.angle(start).1, "a turning mark moves");
    }

    #[test]
    fn stopping_eases_forward_to_the_next_sixth() {
        let start = Instant::now();
        let spin = Spin::Still(0.).update(true, start);
        // A quarter turn is past the first sixth: it rests at the second, 120°.
        let stop = start + TURN / 4;
        let rest = spin.update(false, stop);
        let (mid, moving) = rest.angle(stop + REST / 2);
        assert!(
            moving && mid > 0.25 && mid < 2. * SIXTH,
            "it moves forward, never back: {mid}"
        );
        let done = stop + REST;
        assert_eq!(rest.angle(done), (2. * SIXTH, false));
        assert_eq!(rest.update(false, done), Spin::Still(2. * SIXTH));
    }

    #[test]
    fn a_mark_on_a_sixth_stays_there_and_a_full_turn_wraps_to_zero() {
        assert!(close(next_rest(SIXTH), SIXTH));
        assert!(close(next_rest(0.), 0.));
        assert!(close(next_rest(0.95).fract(), 0.));
    }

    #[test]
    fn turning_again_while_resting_starts_from_where_it_is() {
        let start = Instant::now();
        let stop = start + TURN / 4;
        let rest = Spin::Still(0.).update(true, start).update(false, stop);
        let again = stop + REST / 2;
        let (angle, _) = rest.angle(again);
        let turning = rest.update(true, again);
        assert!(close(turning.angle(again).0, angle));
    }

    #[test]
    fn a_mark_drawn_anew_picks_up_the_turn_where_it_is() {
        let epoch = Instant::now();
        let running = first_spin(true, epoch);
        // The line refreshes 5 s in: the new mark's state starts then, on the same clock.
        let later = epoch + Duration::from_secs(5);
        let anew = first_spin(true, epoch).update(true, later);
        assert!(close(anew.angle(later).0, running.angle(later).0));
        assert!(close(anew.angle(later).0, 5. / 14.), "it does not start again from 0");
        assert_eq!(first_spin(false, epoch), Spin::Still(0.));
    }

    #[test]
    fn a_still_mark_does_not_move() {
        let now = Instant::now();
        assert_eq!(Spin::Still(0.).update(false, now), Spin::Still(0.));
        assert_eq!(Spin::Still(0.).angle(now), (0., false));
    }

    #[test]
    fn each_layer_dims_to_35_percent_and_returns_in_a_breath() {
        let core = LAYERS - 1;
        assert!(close(breath(core, Duration::ZERO, 60.), 1.));
        assert!(close(breath(core, BREATH / 2, 60.), DIM));
        assert!(close(breath(core, BREATH, 60.), 1.));
        // Never brighter than lit, never dimmer than 35 %.
        for ms in (0..5000).step_by(37) {
            let light = breath(core, Duration::from_millis(ms), 60.);
            assert!((DIM - 1e-3..=1. + 1e-3).contains(&light), "{ms} ms: {light}");
        }
    }

    #[test]
    fn the_light_moves_from_the_crossing_out_to_the_rim() {
        // At the core's darkest, the rim, 700 ms behind, is still brighter.
        let darkest = BREATH / 2;
        assert!(breath(0, darkest, 60.) > breath(LAYERS - 1, darkest, 60.) + 0.1);
        // The rim reaches the same light 70 ms a layer later.
        assert!(close(breath(0, darkest + STAGGER * 10, 60.), DIM));
        // Under 24 px the layers breathe as one.
        assert!(close(breath(0, darkest, 16.), breath(LAYERS - 1, darkest, 16.)));
    }

    /// A view that draws one mark.
    struct Line {
        breathing: bool,
    }

    impl gpui_kit::Render for Line {
        fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
            let mut mark = breathing_mark("mark", 16., gpui_kit::black());
            mark.breathing = self.breathing;
            div().child(mark)
        }
    }

    /// A window with a [`Line`], and a count of the times the mark asked it to draw again.
    fn line(
        breathing: bool,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        gpui_kit::Entity<Line>,
        Rc<Cell<usize>>,
        &mut gpui_kit::VisualTestContext,
    ) {
        let (line, cx) = cx.add_window_view(|_, _| Line { breathing });
        let asks = Rc::new(Cell::new(0));
        let count = asks.clone();
        cx.update(|_, cx| cx.observe(&line, move |_, _| count.set(count.get() + 1)).detach());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (line, asks, cx)
    }

    /// How many times the mark asks to be drawn again over `span` of the test clock, drawing each millisecond.
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
    fn a_breathing_mark_redraws_at_thirty_frames_a_second_and_a_still_one_never(cx: &mut gpui_kit::TestAppContext) {
        let (line, asks, cx) = line(true, cx);
        // A second of breathing: about 30 frames, never the display's rate.
        let breathing = asks_over(&asks, Duration::from_secs(1), cx);
        assert!((27..=31).contains(&breathing), "{breathing} frames in a second");

        // It stops: once it has eased to rest, nothing asks for a frame.
        line.update(cx, |line, cx| {
            line.breathing = false;
            cx.notify();
        });
        asks_over(&asks, REST + Duration::from_millis(100), cx);
        assert_eq!(
            asks_over(&asks, Duration::from_secs(1), cx),
            0,
            "a mark at rest asks for no frames"
        );
    }

    #[gpui_kit::test]
    fn a_still_mark_asks_for_no_frames(cx: &mut gpui_kit::TestAppContext) {
        let (_, asks, cx) = line(false, cx);
        assert_eq!(asks_over(&asks, Duration::from_secs(1), cx), 0);
    }

    #[gpui_kit::test]
    fn under_less_motion_a_breathing_mark_stands_still(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (_, asks, cx) = line(true, cx);
        assert_eq!(asks_over(&asks, Duration::from_secs(1), cx), 0);
    }
}
