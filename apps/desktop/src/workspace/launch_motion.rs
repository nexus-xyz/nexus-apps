//! Starting a session: the home screen hands over to the session on one clock that starts on the click, as Zeron's
//! composer dock does (`crates/ui/src/composer_dock.rs` in github.com/zeronsh/zeron, MIT, Copyright (c) 2026 Wing:
//! `stage`, the critically damped glide and the stage fractions are taken from it).
//!
//! Over `T` (420ms) from the click: the home's heading, where line, recents and starters fade and lift 4px (0 to
//! 0.25 T), its art dissolves (0.06 to 0.88 T), the composer glides from its home spot to the dock and never hides,
//! the toolbar trades the home's chips (out by 0.25 T) for the session's (0.25 to 0.55 T), and the first message fades
//! in with an 8px rise (0.20 to 0.65 T). The working line fades in from 300ms (500ms, expo-out, 4px). With reduced
//! motion there is no flight: everything is in place at once. Frames are asked for only while something moves.
use super::backdrop::Art;
use super::{View, Workspace};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc, time::Instant};

/// The whole handover, in seconds (Zeron's `NEW_THREAD_TRANSITION` and its dock duration).
const T: f32 = 0.420;
/// The working line starts fading in this long after the click, over `WORKING`.
const WORKING_FROM: f32 = 0.300;
const WORKING: f32 = 0.500;
/// The last piece (the working line) is in place by then.
const END: f32 = WORKING_FROM + WORKING;
/// How far the home's content lifts while it leaves, the first message rises and the working line rises.
const LIFT: f32 = 4.;
const MESSAGE_RISE: f32 = 8.;
const WORKING_RISE: f32 = 4.;

/// Smoothstep over a slice of the clock: 0 before `start`, 1 after `end` (Zeron's `stage`).
pub(super) fn stage(value: f32, start: f32, end: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far a critically damped spring starting at rest has come, `seconds` into a move of `duration` (Zeron's
/// `Glide`, omega = 12 / duration: no overshoot, within a fraction of a pixel at `duration`).
pub(super) fn glide(seconds: f32, duration: f32) -> f32 {
    if seconds >= duration {
        return 1.0;
    }
    let omega = 12.0 / duration;
    let s = seconds.max(0.0);
    1.0 - (1.0 + omega * s) * (-omega * s).exp()
}

/// Where every piece of the handover is, some time after the click.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frame {
    /// The home's heading, where line, recents and starters, and its toolbar chips: opacity.
    pub home: f32,
    /// How far up the home's content has lifted, in pixels.
    pub lift: f32,
    /// The home's art: opacity.
    pub art: f32,
    /// The composer's way from its home spot (0) to the dock (1).
    pub glide: f32,
    /// The home's toolbar chips still show (the first quarter); the session's show after.
    pub trading: bool,
    /// The session's toolbar chips: opacity.
    pub arriving: f32,
    /// The first message: opacity (it rises `MESSAGE_RISE` while it fades in).
    pub message: f32,
    /// The working line: opacity (it rises `WORKING_RISE` while it fades in).
    pub working: f32,
}

impl Frame {
    pub fn at(seconds: f32) -> Frame {
        let u = seconds / T;
        let out = stage(u, 0.0, 0.25);
        Frame {
            home: 1.0 - out,
            lift: LIFT * out,
            art: 1.0 - stage(u, 0.06, 0.88),
            glide: glide(seconds, T),
            trading: u < 0.25,
            arriving: stage(u, 0.25, 0.55),
            message: stage(u, 0.20, 0.65),
            working: crate::motion::ease_out((seconds - WORKING_FROM) / WORKING),
        }
    }
}

#[derive(Clone, Copy)]
struct Measurement {
    bounds: Bounds<Pixels>,
    viewport: Size<Pixels>,
}

#[derive(Default, Clone)]
pub(super) struct LaunchMotion {
    /// The home composer's place, measured on every home frame.
    origin: Rc<Cell<Option<Measurement>>>,
    flight: Option<Flight>,
    /// Set while the leaving home's content is built, so its chips draw frosted as they did on the art.
    ghost: Rc<Cell<bool>>,
}

#[derive(Clone, Copy)]
struct Flight {
    origin: Measurement,
    /// The click.
    at: Instant,
    /// From a new chat's screen; else from a new Code session's.
    chat: bool,
    /// The art the home showed.
    art: Option<Art>,
    /// The home showed its setup line under the where line (`start_hints.rs`).
    setup_line: bool,
}

/// The first message, the working line under it, or the session's chrome around the composer (who pays under it).
#[derive(Clone, Copy)]
pub(super) enum Part {
    Message,
    Working,
    Chrome,
}

fn launching(view: &View, empty_chat: bool) -> bool {
    matches!(view, View::NewCode) || matches!(view, View::Chat) && empty_chat
}

impl Workspace {
    /// Start the clock on the click, before the launch view is replaced. Existing conversations never get a flight.
    pub(super) fn begin_launch_motion(&mut self, cx: &Context<Self>) {
        let launch = launching(&self.view, self.entries.is_empty());
        self.launch_motion.flight = None;
        if !launch || cx.reduce_motion() {
            return;
        }
        if let Some(origin) = self.launch_motion.origin.get() {
            self.launch_motion.flight = Some(Flight {
                origin,
                at: Instant::now(),
                chat: matches!(self.view, View::Chat),
                art: self.backdrop_art(),
                setup_line: self.offer_setup_line(),
            });
        }
    }

    /// Where the handover is now, while it runs.
    pub(super) fn launch_frame(&self) -> Option<Frame> {
        let seconds = self.launch_motion.flight?.at.elapsed().as_secs_f32();
        (seconds < END).then(|| Frame::at(seconds))
    }

    /// The art the home showed, while it still dissolves under the session.
    pub(super) fn launch_art(&self) -> Option<Art> {
        self.launch_frame()
            .filter(|frame| frame.art > 0.)
            .and(self.launch_motion.flight?.art)
    }

    /// The leaving home's content is being built: its panels draw as on the art.
    pub(super) fn launch_ghost(&self) -> bool {
        self.launch_motion.ghost.get()
    }

    /// The first message or the working line of a session just started, where the clock has it: fading in as it rises.
    /// As is once the handover is over, or when there is none (reduced motion).
    pub(super) fn launch_part(&self, part: Part, element: AnyElement) -> AnyElement {
        let Some(frame) = self.launch_frame() else {
            return element;
        };
        let (opacity, rise) = match part {
            Part::Message => (frame.message, MESSAGE_RISE),
            Part::Working => (frame.working, WORKING_RISE),
            // With the session's toolbar chips: the composer lands without it, then it fades in under the box.
            Part::Chrome => (frame.arriving, 0.),
        };
        div()
            .w_full()
            .opacity(opacity)
            .relative()
            .top(px(rise * (1. - opacity)))
            .child(element)
            .into_any_element()
    }

    /// Whether a session's first message rides the start's clock (or, with reduced motion, simply lands), rather than
    /// arriving on its own.
    pub(super) fn launch_owns_arrival(&self, cx: &App) -> bool {
        self.launch_motion.flight.is_some() || cx.reduce_motion()
    }

    pub(super) fn move_launch_composer(&self, element: impl IntoElement, _cx: &Context<Self>) -> AnyElement {
        let element = div().w_full().child(element);
        if launching(&self.view, self.entries.is_empty()) {
            return measured(element, self.launch_motion.origin.clone()).into_any_element();
        }
        match self.launch_motion.flight {
            Some(flight) if flight.at.elapsed().as_secs_f32() < T => Glided {
                child: Some(element.into_any_element()),
                origin: flight.origin,
                at: flight.at,
            }
            .into_any_element(),
            _ => element.into_any_element(),
        }
    }

    /// The home leaving, under a session just started: its art dissolving, and its heading, where line, recents and
    /// starters fading where they stood, around the composer's home spot. They take no clicks. Put it first in the
    /// session's `relative` root. It also keeps the frames coming while anything of the handover moves.
    pub(super) fn render_launch_exit(
        &self,
        palette: crate::palette::Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let flight = self.launch_motion.flight?;
        let frame = self.launch_frame()?;
        let mut layer = div()
            .absolute()
            .inset_0()
            .child(
                canvas(|_, window, _| window.request_animation_frame(), |_, _, _, _| {})
                    .absolute()
                    .size_0(),
            )
            .children(self.launch_art().map(|art| {
                div()
                    .absolute()
                    .inset_0()
                    .opacity(frame.art)
                    .child(self.render_backdrop_art(Some(art), palette, cx))
            }));
        if frame.home > 0. {
            let origin = flight.origin;
            let width = origin.bounds.size.width;
            let x = origin.bounds.left();
            self.launch_motion.ghost.set(true);
            let head = self.render_launch_head(palette).into_any_element();
            let below = if flight.chat {
                // A new chat's column (`transcript.rs`): the where line 8px under the box, the starters 20px under it.
                v_flex()
                    .w_full()
                    .items_center()
                    .gap(px(20.))
                    .child(div().w_full().child(self.render_where_line(true, palette, cx)))
                    .children(self.render_starters(palette, cx))
            } else {
                // A new Code session's column (`new_code.rs`): the where line and the setup line, then the
                // recents and the starters. Home notes (a link, the owner) are not redrawn.
                let gap = px(if flight.setup_line { 32. } else { 40. });
                v_flex()
                    .w_full()
                    .child(self.render_where_line(false, palette, cx))
                    .children(
                        self.render_setup_line(palette, cx)
                            .map(|line| div().mt(px(12.)).child(line)),
                    )
                    .children(
                        self.render_recents(palette, cx)
                            .map(|recents| div().mt(gap).child(recents)),
                    )
                    .children(
                        self.render_starters(palette, cx)
                            .map(|starters| div().mt(gap).child(starters)),
                    )
            };
            self.launch_motion.ghost.set(false);
            let leaving = |child: AnyElement| div().w(width).opacity(frame.home).child(child).into_any_element();
            layer = layer
                .child(Pinned::new(
                    leaving(head),
                    point(x, origin.bounds.top() - px(20. + frame.lift)),
                    true,
                    width,
                    origin.viewport,
                ))
                .child(Pinned::new(
                    leaving(below.into_any_element()),
                    point(x, origin.bounds.bottom() + px(8. - frame.lift)),
                    false,
                    width,
                    origin.viewport,
                ))
                .child(div().absolute().inset_0().occlude());
        }
        Some(layer.into_any_element())
    }
}

fn measured(element: impl IntoElement, target: Rc<Cell<Option<Measurement>>>) -> Div {
    div().relative().w_full().child(element).child(
        canvas(
            move |bounds, window, _| {
                target.set(Some(Measurement {
                    bounds,
                    viewport: window.viewport_size(),
                }));
            },
            |_, _, _, _| {},
        )
        // Pinned to the corner: an absolute box with no inset sits where it would flow, under the composer.
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// The composer in its dock slot, drawn on its way there from the home spot (Zeron's `DockedComposer`). Its layout
/// stays in the slot; prepaint knows the slot before anything is drawn, so the first frame is already on the way, and
/// hitboxes, selection and caret move with its pixels. A window resized since the click puts it in the slot.
struct Glided {
    child: Option<AnyElement>,
    origin: Measurement,
    at: Instant,
}

impl IntoElement for Glided {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Glided {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.as_mut().expect("laid out once");
        (child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let left = if self.origin.viewport == window.viewport_size() {
            1. - glide(self.at.elapsed().as_secs_f32(), T)
        } else {
            0.
        };
        if left > 0. {
            window.request_animation_frame();
        }
        let offset = point(
            (self.origin.bounds.left() - bounds.left()) * left,
            (self.origin.bounds.top() - bounds.top()) * left,
        );
        // While it moves, drawn over the whole window: the dock it lands in clips to its own bounds (a scroll area), and
        // the home spot it leaves is outside them.
        if left > 0. {
            if let Some(child) = self.child.take() {
                let at = window.element_offset() + offset;
                window.defer_draw(child, at, 1, None);
            }
        } else if let Some(child) = self.child.as_mut() {
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = self.child.as_mut() {
            child.paint(window, cx);
        }
    }
}

/// A piece of the leaving home at its place in the window: under `at`, or over it when `above`. It takes no room in
/// its parent. Not drawn once the window was resized since the click.
struct Pinned {
    child: AnyElement,
    at: Point<Pixels>,
    above: bool,
    width: Pixels,
    viewport: Size<Pixels>,
    size: Size<Pixels>,
    shown: bool,
}

impl Pinned {
    fn new(child: AnyElement, at: Point<Pixels>, above: bool, width: Pixels, viewport: Size<Pixels>) -> Self {
        Pinned {
            child,
            at,
            above,
            width,
            viewport,
            size: Size::default(),
            shown: false,
        }
    }
}

impl IntoElement for Pinned {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Pinned {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.size = self.child.layout_as_root(
            size(AvailableSpace::Definite(self.width), AvailableSpace::MaxContent),
            window,
            cx,
        );
        let mut style = Style::default();
        style.position = Position::Absolute;
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.shown = window.viewport_size() == self.viewport;
        if self.shown {
            let top = if self.above {
                self.at.y - self.size.height
            } else {
                self.at.y
            };
            self.child.prepaint_at(point(self.at.x, top), window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.shown {
            self.child.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{END, Frame, T, glide, stage};

    fn near(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn stage_is_a_smoothstep_over_its_slice() {
        assert_eq!(stage(0.1, 0.2, 0.6), 0.0);
        assert_eq!(stage(0.7, 0.2, 0.6), 1.0);
        assert!(near(stage(0.4, 0.2, 0.6), 0.5, 1e-6));
        // Slow at both ends of the slice.
        assert!(stage(0.24, 0.2, 0.6) < 0.1);
        assert!(stage(0.56, 0.2, 0.6) > 0.9);
    }

    #[test]
    fn the_glide_starts_at_rest_never_overshoots_and_lands_at_t() {
        assert_eq!(glide(0.0, T), 0.0);
        // Critically damped: about 80% of the way at a quarter of the clock, 99% by 0.6 T.
        assert!(near(glide(0.25 * T, T), 0.80, 0.01));
        assert!(glide(0.6 * T, T) > 0.99);
        let mut last = 0.0;
        for step in 1..=420 {
            let p = glide(step as f32 / 1000.0, T);
            assert!(p >= last && p <= 1.0);
            last = p;
        }
        assert_eq!(glide(T, T), 1.0);
        // Within a fraction of a pixel of a 1000px move just before it lands.
        assert!((1.0 - glide(T - 0.001, T)) * 1000.0 < 0.5);
    }

    #[test]
    fn the_click_is_frame_zero_everything_still_home() {
        let frame = Frame::at(0.0);
        assert_eq!(frame.home, 1.0);
        assert_eq!(frame.lift, 0.0);
        assert_eq!(frame.art, 1.0);
        assert_eq!(frame.glide, 0.0);
        assert!(frame.trading);
        assert_eq!(frame.arriving, 0.0);
        assert_eq!(frame.message, 0.0);
        assert!(frame.working.abs() < 1e-3);
    }

    #[test]
    fn stages_follow_the_approved_timeline() {
        // The home is gone and lifted 4px at 0.25 T; the toolbar trades there.
        let quarter = Frame::at(0.25 * T + 1e-4);
        assert_eq!(quarter.home, 0.0);
        assert_eq!(quarter.lift, 4.0);
        assert!(!quarter.trading);
        assert!(quarter.arriving < 1e-3);
        // The art starts dissolving at 0.06 T and is gone at 0.88 T.
        assert_eq!(Frame::at(0.06 * T).art, 1.0);
        assert!(Frame::at(0.5 * T).art > 0.0 && Frame::at(0.5 * T).art < 1.0);
        assert_eq!(Frame::at(0.88 * T).art, 0.0);
        // The session's chips are in by 0.55 T, the first message from 0.20 T to 0.65 T.
        assert_eq!(Frame::at(0.55 * T).arriving, 1.0);
        assert_eq!(Frame::at(0.20 * T).message, 0.0);
        assert!(Frame::at(0.4 * T).message > 0.0);
        assert_eq!(Frame::at(0.65 * T).message, 1.0);
        // The working line waits for 300ms, then takes 500ms.
        assert!(Frame::at(0.3).working.abs() < 1e-3);
        assert!(Frame::at(0.4).working > 0.5);
        assert!(near(Frame::at(0.8).working, 1.0, 1e-3));
    }

    #[test]
    fn everything_is_in_place_when_the_clock_ends() {
        let frame = Frame::at(END);
        assert_eq!(frame.home, 0.0);
        assert_eq!(frame.art, 0.0);
        assert_eq!(frame.glide, 1.0);
        assert_eq!(frame.arriving, 1.0);
        assert_eq!(frame.message, 1.0);
        assert!(near(frame.working, 1.0, 1e-3));
        assert!(END > T);
    }
}
