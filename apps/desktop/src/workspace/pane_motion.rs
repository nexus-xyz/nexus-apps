//! The Workbench's motion. Opening and closing match side-pane.css: the pane slides in from the window's edge at its
//! full width, so its tools never reflow, and the session gives way to it frame by frame. Maximizing and restoring
//! grow and shrink the pane over the session along the same curves, instead of switching layouts in one frame.
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Kind {
    #[default]
    Docked,
    Drawer,
    Expanded,
}

/// What moves. `Slide`: the pane enters or leaves, 0 is off screen and 1 on it. `Grow`: the pane maximizes or
/// restores, 0 is its width beside the session and 1 the whole content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Slide,
    Grow,
}

#[derive(Clone, Copy)]
struct Transition {
    at: Instant,
    from: f32,
    to: f32,
    mode: Mode,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Motion {
    initialized: bool,
    shown: bool,
    kind: Kind,
    /// Where the pane sits when it is not maximized: beside the session or over it as a drawer.
    rest: Kind,
    epoch: usize,
    transition: Option<Transition>,
}

#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub(super) kind: Kind,
    pub(super) rest: Kind,
    pub(super) mode: Mode,
    epoch: usize,
    from: f32,
    to: f32,
    pub(super) moving: bool,
}

fn easing(time: f32, closing: bool) -> f32 {
    if closing {
        crate::motion::cubic_bezier(0.65, 0., 0.35, 1., time)
    } else {
        crate::motion::ease_out(time)
    }
}

impl Motion {
    fn position(&self, now: Instant, mode: Mode) -> Option<f32> {
        let step = self.transition.filter(|step| step.mode == mode)?;
        let time = now.saturating_duration_since(step.at).as_secs_f32() / crate::motion::PANEL.as_secs_f32();
        Some(step.from + (step.to - step.from) * easing(time.min(1.), step.to == 0.))
    }

    /// Animation frames can reuse an element tree; the owner timer removes an exited pane explicitly.
    pub(super) fn finish(&mut self, epoch: usize) {
        if self.epoch == epoch {
            self.transition = None;
        }
    }

    fn start(&mut self, now: Instant, from: f32, to: f32, mode: Mode) {
        self.epoch += 1;
        self.transition = Some(Transition {
            at: now,
            from,
            to,
            mode,
        });
    }

    /// Observe visibility and layout centrally: shortcuts, drawer close, and tool actions share one transition.
    pub(super) fn update(&mut self, shown: bool, kind: Kind, now: Instant, reduced: bool) -> Option<Frame> {
        if !self.initialized || reduced {
            self.initialized = true;
            self.shown = shown;
            self.kind = kind;
            if kind != Kind::Expanded {
                self.rest = kind;
            }
            self.transition = None;
        } else if shown != self.shown {
            let from = self
                .position(now, Mode::Slide)
                .unwrap_or(if self.shown { 1. } else { 0. });
            self.shown = shown;
            // A pane that leaves keeps the layout it leaves from: hiding a maximized Workbench also restores it, and
            // it should slide away at the width it had, not jump to its docked width first.
            if shown {
                self.kind = kind;
                if kind != Kind::Expanded {
                    self.rest = kind;
                }
            }
            self.start(now, from, if shown { 1. } else { 0. }, Mode::Slide);
        } else if shown && kind != self.kind {
            if (kind == Kind::Expanded) != (self.kind == Kind::Expanded) {
                let to = if kind == Kind::Expanded { 1. } else { 0. };
                let from = self.position(now, Mode::Grow).unwrap_or(1. - to);
                self.rest = if kind == Kind::Expanded { self.kind } else { kind };
                self.kind = kind;
                self.start(now, from, to, Mode::Grow);
            } else {
                // Docked and drawer swap when the window crosses the drawer width: no motion for a resize.
                self.kind = kind;
                self.rest = kind;
                self.transition = None;
            }
        }
        if self
            .transition
            .is_some_and(|step| now.saturating_duration_since(step.at) >= crate::motion::PANEL)
        {
            self.transition = None;
        }
        if !self.shown && self.transition.is_none() {
            return None;
        }
        let (from, to, mode) = self
            .transition
            .map_or((1., 1., Mode::Slide), |step| (step.from, step.to, step.mode));
        Some(Frame {
            kind: self.kind,
            rest: self.rest,
            mode,
            epoch: self.epoch,
            from,
            to,
            moving: self.transition.is_some(),
        })
    }
}

impl Frame {
    pub(super) fn wake_epoch(self) -> Option<usize> {
        self.moving.then_some(self.epoch)
    }

    /// Maximized and still: the pane takes the content area in the session's place.
    pub(super) fn expanded_at_rest(self) -> bool {
        self.kind == Kind::Expanded && !self.moving
    }

    /// Growing or shrinking: the pane fills a frame whose width moves.
    pub(super) fn growing(self) -> bool {
        self.moving && self.mode == Mode::Grow
    }

    /// Whether the pane's head sits in the window's title bar, as its column's head: docked or maximized, and while it
    /// slides or grows between those. A drawer floats over the session, under the bar, so it keeps its head.
    pub(super) fn head_in_title(self) -> bool {
        self.kind != Kind::Drawer && self.rest != Kind::Drawer
    }

    /// The pane's own width while it moves, when it is fixed: a slide keeps the width it rests at.
    pub(super) fn slide_width(self, rest: f32, full: f32) -> Option<f32> {
        (self.moving && self.mode == Mode::Slide).then_some(if self.kind == Kind::Expanded { full } else { rest })
    }
}

/// `rest`: the pane's width beside the session (or as a drawer); `full`: the whole content area's.
pub(super) fn render(pane: AnyElement, rest: f32, full: f32, state: Frame) -> AnyElement {
    if !state.moving {
        return pane;
    }
    let closing = state.to == 0.;
    let (from, to) = (state.from, state.to);
    let blocked = |frame: Stateful<Div>| {
        frame.capture_key_down(|_, _, cx| cx.stop_propagation()).child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
        )
    };
    if state.mode == Mode::Grow {
        // The pane lays out at each width, as when its edge is dragged; the session gives way beside it.
        let span = (full - rest).max(0.);
        return div()
            .id("side-pane-grow")
            .relative()
            .h_full()
            .flex()
            .flex_none()
            .overflow_hidden()
            .child(pane)
            .when(state.rest == Kind::Drawer, |frame| {
                frame.absolute().top_0().right_0().bottom_0()
            })
            .with_animation(
                ("side-pane-grow", state.epoch),
                Animation::new(crate::motion::PANEL).with_easing(move |time| easing(time, closing)),
                move |frame, progress| frame.w(px(rest + span * (from + (to - from) * progress))),
            )
            .into_any_element();
    }
    let width = if state.kind == Kind::Expanded { full } else { rest };
    let drawer = state.kind == Kind::Drawer;
    let frame = div()
        .id("side-pane-motion")
        .relative()
        .h_full()
        .flex_none()
        .overflow_hidden()
        // The tool's width stays fixed while the conversation gives it room.
        .child(div().relative().flex().w(px(width)).h_full().flex_none().child(pane))
        .when(drawer, |frame| frame.absolute().top_0().right_0().bottom_0())
        .when(closing, blocked);
    let style = move |frame: Stateful<Div>, fraction: f32| {
        if drawer {
            frame.w(px(width)).right(px(-16. * (1. - fraction))).opacity(fraction)
        } else {
            frame.w(px(width * fraction))
        }
    };
    frame
        .with_animation(
            ("side-pane-slide", state.epoch),
            Animation::new(crate::motion::PANEL).with_easing(move |time| easing(time, closing)),
            move |frame, progress| style(frame, from + (to - from) * progress),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Kind, Mode, Motion};
    use std::time::{Duration, Instant};

    /// A docked Workbench's head is its column's head in the title bar, also maximized and on the way between, and
    /// while it slides out. A drawer, and a drawer maximized, keep their head over the session.
    #[test]
    fn docked_and_maximized_heads_sit_in_the_title_bar_and_a_drawers_does_not() {
        let now = Instant::now();
        let mut motion = Motion::default();
        assert!(
            motion
                .update(true, Kind::Docked, now, false)
                .expect("docked")
                .head_in_title()
        );
        let growing = motion.update(true, Kind::Expanded, now, false).expect("growing");
        assert!(growing.moving && growing.head_in_title());
        let maximized = motion
            .update(true, Kind::Expanded, now + Duration::from_millis(280), false)
            .expect("maximized");
        assert!(maximized.expanded_at_rest() && maximized.head_in_title());
        let leaving = motion
            .update(false, Kind::Expanded, now + Duration::from_millis(300), false)
            .expect("leaving");
        assert!(leaving.moving && leaving.head_in_title());

        let mut drawer = Motion::default();
        assert!(
            !drawer
                .update(true, Kind::Drawer, now, false)
                .expect("drawer")
                .head_in_title()
        );
        let drawer_grows = drawer.update(true, Kind::Expanded, now, false).expect("drawer grows");
        assert_eq!(drawer_grows.rest, Kind::Drawer);
        assert!(!drawer_grows.head_in_title());
    }

    #[test]
    fn first_paint_is_still_and_closing_retains_the_pane_for_280_ms() {
        let now = Instant::now();
        let mut motion = Motion::default();
        assert!(!motion.update(true, Kind::Docked, now, false).expect("visible").moving);
        let closing = motion.update(false, Kind::Docked, now, false).expect("exit frame");
        assert_eq!((closing.from, closing.to), (1., 0.));
        assert!(
            motion
                .update(false, Kind::Docked, now + Duration::from_millis(279), false)
                .is_some()
        );
        assert!(
            motion
                .update(false, Kind::Docked, now + Duration::from_millis(280), false)
                .is_none()
        );
    }

    #[test]
    fn reversing_a_slide_starts_at_its_current_position() {
        let now = Instant::now();
        let mut motion = Motion::default();
        assert!(motion.update(false, Kind::Docked, now, false).is_none());
        let enter = motion.update(true, Kind::Docked, now, false).expect("entry");
        assert_eq!((enter.from, enter.to), (0., 1.));
        let halfway = now + Duration::from_millis(140);
        let position = motion.position(halfway, Mode::Slide).expect("sliding");
        let reverse = motion.update(false, Kind::Docked, halfway, false).expect("exit");
        assert!((reverse.from - position).abs() < 0.0001);
        assert!(reverse.from > 0. && reverse.from < 1.);
        assert!(reverse.epoch > enter.epoch);
    }

    #[test]
    fn stale_completion_cannot_remove_a_reversed_pane() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(false, Kind::Docked, now, false);
        let opening = motion.update(true, Kind::Docked, now, false).expect("entry");
        let closing = motion
            .update(false, Kind::Docked, now + Duration::from_millis(40), false)
            .expect("exit");
        motion.finish(opening.epoch);
        assert!(motion.transition.is_some());
        motion.finish(closing.epoch);
        assert!(
            motion
                .update(false, Kind::Docked, now + Duration::from_millis(40), false)
                .is_none()
        );
    }

    #[test]
    fn reduced_motion_and_drawer_swaps_cancel_a_transition() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(false, Kind::Docked, now, false);
        motion.update(true, Kind::Docked, now, false);
        assert!(!motion.update(true, Kind::Drawer, now, false).expect("drawer").moving);
        assert!(motion.update(false, Kind::Drawer, now, true).is_none());
        assert!(!motion.update(true, Kind::Expanded, now, true).expect("expanded").moving);
        assert!(motion.update(false, Kind::Expanded, now, true).is_none());
    }

    #[test]
    fn maximizing_and_restoring_grow_and_shrink_the_pane() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(true, Kind::Docked, now, false);
        let grow = motion.update(true, Kind::Expanded, now, false).expect("grow");
        assert_eq!(
            (grow.mode, grow.rest, grow.from, grow.to),
            (Mode::Grow, Kind::Docked, 0., 1.)
        );
        assert!(grow.growing() && !grow.expanded_at_rest());
        // Restored half way: it shrinks back from where it got to.
        let halfway = now + Duration::from_millis(140);
        let position = motion.position(halfway, Mode::Grow).expect("growing");
        let shrink = motion.update(true, Kind::Docked, halfway, false).expect("shrink");
        assert!((shrink.from - position).abs() < 0.0001 && shrink.to == 0.);
        let later = halfway + crate::motion::PANEL;
        assert!(!motion.update(true, Kind::Docked, later, false).expect("docked").moving);
        motion.update(true, Kind::Expanded, later, false);
        let done = motion
            .update(true, Kind::Expanded, later + crate::motion::PANEL, false)
            .expect("maximized");
        assert!(done.expanded_at_rest());
    }

    #[test]
    fn a_maximized_pane_slides_away_at_its_full_width() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.update(true, Kind::Expanded, now, false);
        // Hiding also restores it: the kind asked for is docked, the pane still leaves maximized.
        let leaving = motion.update(false, Kind::Docked, now, false).expect("exit");
        assert_eq!(
            (leaving.kind, leaving.mode, leaving.to),
            (Kind::Expanded, Mode::Slide, 0.)
        );
        assert_eq!(leaving.slide_width(400., 1200.), Some(1200.));
        assert!(!leaving.expanded_at_rest());
        // Opening straight into it slides in at full width too.
        let later = now + crate::motion::PANEL;
        assert!(motion.update(false, Kind::Docked, later, false).is_none());
        let opening = motion.update(true, Kind::Expanded, later, false).expect("entry");
        assert_eq!(
            (opening.kind, opening.mode, opening.from),
            (Kind::Expanded, Mode::Slide, 0.)
        );
    }
}
