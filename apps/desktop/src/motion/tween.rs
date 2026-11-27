//! A value that eases toward a target and can turn around on the way: a chevron that rotates when a group folds, a
//! fill that follows the pointer, a panel's height. GPUI's `with_animation` plays from 0 to 1 once per element id, so a
//! change of mind half way has to start over from an end. Here the element keeps where it is drawn now, and a new
//! target starts from there (Zeron's `StateMorphs`, `ui/src/motion.rs`).
//!
//! The first time an element is drawn it sits at its target (or at `from`, when given), so a row that loads with the
//! page does not move. Frames are asked for only while the value moves.

use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};

/// How a [`TweenExt::with_tween`] moves: its length and its curve.
#[derive(Clone, Copy)]
pub struct Tween {
    pub duration: Duration,
    pub easing: fn(f32) -> f32,
    /// Where a newly drawn element starts, instead of at its target: something that should arrive.
    pub from: Option<f32>,
}

impl Tween {
    pub const fn new(duration: Duration, easing: fn(f32) -> f32) -> Self {
        Self {
            duration,
            easing,
            from: None,
        }
    }

    /// Start a newly drawn element at `from`, so it moves to its target as it appears.
    pub const fn from(mut self, from: f32) -> Self {
        self.from = Some(from);
        self
    }
}

/// Where one tween is: the value it left from, the one it goes to, and when it left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Course {
    from: f32,
    to: f32,
    at: Instant,
}

impl Course {
    pub(crate) fn new(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            at: now,
        }
    }

    /// The value at `now`, and whether it still moves.
    pub(crate) fn at(&self, now: Instant, tween: &Tween) -> (f32, bool) {
        if self.from == self.to || tween.duration.is_zero() {
            return (self.to, false);
        }
        let t = now.saturating_duration_since(self.at).as_secs_f32() / tween.duration.as_secs_f32();
        if t >= 1. {
            return (self.to, false);
        }
        (self.from + (self.to - self.from) * (tween.easing)(t), true)
    }

    /// Head for `to` from where the value is at `now`. The same target again changes nothing.
    pub(crate) fn retarget(&mut self, to: f32, now: Instant, tween: &Tween) {
        if to == self.to {
            return;
        }
        let (value, _) = self.at(now, tween);
        *self = Self {
            from: value,
            to,
            at: now,
        };
    }
}

/// [`TweenExt::with_tween`]'s element.
pub struct TweenElement<E> {
    id: ElementId,
    element: Option<E>,
    target: f32,
    tween: Tween,
    animator: Option<Box<dyn FnOnce(E, f32) -> E>>,
}

pub trait TweenExt: IntoElement + Sized + 'static {
    /// Draw this element at a value easing toward `target`, kept across frames by `id`.
    fn with_tween(
        self,
        id: impl Into<ElementId>,
        target: f32,
        tween: Tween,
        animator: impl FnOnce(Self, f32) -> Self + 'static,
    ) -> TweenElement<Self> {
        TweenElement {
            id: id.into(),
            element: Some(self),
            target,
            tween,
            animator: Some(Box::new(animator)),
        }
    }
}

impl<E: IntoElement + 'static> TweenExt for E {}

impl<E: IntoElement + 'static> IntoElement for TweenElement<E> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<E: IntoElement + 'static> Element for TweenElement<E> {
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
        let now = Instant::now();
        let (target, tween) = (self.target, self.tween);
        let value = window.with_element_state(id.unwrap(), |course: Option<Course>, _| {
            let mut course = course.unwrap_or_else(|| {
                let mut course = Course::new(tween.from.unwrap_or(target), now);
                course.retarget(target, now, &tween);
                course
            });
            if cx.reduce_motion() {
                course = Course::new(target, now);
            }
            course.retarget(target, now, &tween);
            (course.at(now, &tween), course)
        });
        let (value, moving) = value;
        if moving {
            window.request_animation_frame();
        }
        let element = self.element.take().expect("laid out once");
        let animator = self.animator.take().expect("laid out once");
        let mut element = animator(element, value).into_any_element();
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

    const LINEAR: Tween = Tween::new(Duration::from_millis(100), |t| t);

    #[test]
    fn a_new_course_rests_at_its_value() {
        let now = Instant::now();
        assert_eq!(Course::new(1., now).at(now, &LINEAR), (1., false));
    }

    #[test]
    fn turning_around_starts_from_where_it_is() {
        let start = Instant::now();
        let mut course = Course::new(0., start);
        course.retarget(1., start, &LINEAR);
        let half = start + Duration::from_millis(50);
        assert!((course.at(half, &LINEAR).0 - 0.5).abs() < 1e-3);
        course.retarget(0., half, &LINEAR);
        let (value, moving) = course.at(half, &LINEAR);
        assert!((value - 0.5).abs() < 1e-3 && moving);
        assert_eq!(course.at(half + Duration::from_millis(100), &LINEAR), (0., false));
    }

    #[test]
    fn the_same_target_does_not_restart() {
        let start = Instant::now();
        let mut course = Course::new(0., start);
        course.retarget(1., start, &LINEAR);
        let later = start + Duration::from_millis(60);
        course.retarget(1., later, &LINEAR);
        assert!((course.at(later, &LINEAR).0 - 0.6).abs() < 1e-3);
    }
}
