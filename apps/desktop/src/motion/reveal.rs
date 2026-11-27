//! An area that opens and closes in the layout: its height eases between nothing and its content's, and the content
//! fades with it (Zeron's `COLLAPSE`). Without it, a row that comes into the composer (a note, the attachments, the
//! queue) pushes the box up in one frame, and its going drops the box in one frame.
//!
//! Draw it whether or not it is open, so it keeps its place and its course across frames. Open and at rest it takes
//! its content's own height, so content that grows while open is never clipped. While it moves its height is fixed
//! and the content is clipped to it.
//!
//! The content's height is measured while the area is open, and kept while it closes: content that has already gone
//! (an `Option` the caller no longer has) closes as an empty area of the same height, never as a jump.

use std::time::Instant;

use gpui_kit::*;

use super::tween::{Course, Tween};
use super::{COLLAPSE, ease_out};

const TWEEN: Tween = Tween::new(COLLAPSE, ease_out);

/// An area of `content`, open or not. A first draw sits where `open` says, so what loads with a view does not move.
pub fn reveal(id: impl Into<ElementId>, open: bool, content: Option<AnyElement>) -> Reveal {
    Reveal {
        id: id.into(),
        open,
        content,
        child: None,
    }
}

/// [`reveal`]'s element.
pub struct Reveal {
    id: ElementId,
    open: bool,
    content: Option<AnyElement>,
    /// The content as laid out this frame, and its layout id.
    child: Option<(AnyElement, LayoutId)>,
}

/// One area's course and its content's last measured height.
#[derive(Clone, Copy)]
struct State {
    course: Course,
    height: Pixels,
}

/// How the area is drawn now: at rest open, at rest closed, or moving at `value` of its content's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Open,
    Closed,
    Moving(f32),
}

fn shape(value: f32, moving: bool) -> Shape {
    match (moving, value) {
        (false, value) if value >= 1. => Shape::Open,
        (false, _) => Shape::Closed,
        (true, value) => Shape::Moving(value),
    }
}

impl IntoElement for Reveal {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Reveal {
    type RequestLayoutState = Shape;
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
    ) -> (LayoutId, Shape) {
        let target = if self.open { 1. } else { 0. };
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let (value, moving, height) = window.with_element_state(id.unwrap(), |state: Option<State>, _| {
            let mut state = state.unwrap_or(State {
                course: Course::new(target, now),
                height: px(0.),
            });
            if reduced {
                state.course = Course::new(target, now);
            }
            state.course.retarget(target, now, &TWEEN);
            let (value, moving) = state.course.at(now, &TWEEN);
            ((value, moving, state.height), state)
        });
        if moving {
            window.request_animation_frame();
        }
        let shape = shape(value, moving);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.flex_shrink = 0.;
        let content = self.content.take().filter(|_| shape != Shape::Closed);
        let child = content.map(|content| {
            let opacity = match shape {
                Shape::Moving(value) => value,
                _ => 1.,
            };
            // The content keeps its own height inside the area, however tall the area is now.
            let mut child = div()
                .w_full()
                .flex_none()
                .opacity(opacity)
                .child(content)
                .into_any_element();
            let layout = child.request_layout(window, cx);
            (child, layout)
        });
        match shape {
            Shape::Open => {}
            Shape::Closed => style.size.height = px(0.).into(),
            Shape::Moving(value) => {
                style.size.height = (height * value.max(0.)).into();
                style.overflow = point(Overflow::Hidden, Overflow::Hidden);
            }
        }
        style.display = Display::Flex;
        style.flex_direction = FlexDirection::Column;
        let layout = window.request_layout(style, child.as_ref().map(|(_, layout)| *layout), cx);
        self.child = child;
        (layout, shape)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        shape: &mut Shape,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((child, layout)) = self.child.as_mut() else {
            return;
        };
        // Measure only while open: a closing area keeps the height it had, whatever its content has become.
        if self.open {
            let measured = window.layout_bounds(*layout).size.height;
            let changed = window.with_element_state(id.unwrap(), |state: Option<State>, _| {
                let mut state = state.expect("laid out first");
                let changed = state.height != measured;
                state.height = measured;
                (changed, state)
            });
            // Moving on last frame's height: draw the next one on this one's.
            if changed && matches!(shape, Shape::Moving(_)) {
                window.request_animation_frame();
            }
        }
        let mask = matches!(shape, Shape::Moving(_)).then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        shape: &mut Shape,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((child, _)) = self.child.as_mut() else {
            return;
        };
        let mask = matches!(shape, Shape::Moving(_)).then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::component::v_flex;

    #[test]
    fn at_rest_it_is_open_or_closed_and_moving_it_is_a_share() {
        assert_eq!(shape(1., false), Shape::Open);
        assert_eq!(shape(0., false), Shape::Closed);
        assert_eq!(shape(0.4, true), Shape::Moving(0.4));
    }

    struct Column {
        open: bool,
        tall: f32,
    }

    impl Render for Column {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let content = self.open.then(|| div().h(px(self.tall)).w_full().into_any_element());
            v_flex()
                .w(px(200.))
                .child(reveal("area", self.open, content))
                .child(div().h(px(10.)).w_full().debug_selector(|| "below".into()))
        }
    }

    #[gpui_kit::test]
    fn the_area_takes_its_contents_height_open_and_none_closed(cx: &mut TestAppContext) {
        // Reduced motion draws both ends without waiting on the clock.
        cx.update(|cx| cx.set_reduce_motion(true));
        let (column, cx) = cx.add_window_view(|_, _| Column { open: false, tall: 40. });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let closed = cx.debug_bounds("below").unwrap().top();
        column.update(cx, |column, cx| {
            column.open = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("below").unwrap().top() - closed, px(40.));
        column.update(cx, |column, cx| {
            // Content that grows while open is never clipped.
            column.tall = 70.;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("below").unwrap().top() - closed, px(70.));
        column.update(cx, |column, cx| {
            column.open = false;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("below").unwrap().top(), closed);
    }
}
