//! Collapse trailing metadata to its action width. Measure the content at its natural width so labels, fonts and
//! pull-request badges need no fixed widths. The row's pin and title follow the changing layout together.
//!
//! [`reveal_width`] does the same for a control that comes into a row and leaves it.

use std::time::Instant;

use gpui_kit::*;

use super::tween::{Course, Tween};
use super::{HOVER, standard};

/// Keep `minimum` pixels for the actions while the metadata collapses on hover or while its menu stays open.
pub fn collapse_width(
    id: impl Into<ElementId>,
    collapsed: bool,
    minimum: f32,
    height: f32,
    content: impl IntoElement,
) -> impl IntoElement {
    CollapsingWidth {
        id: id.into(),
        collapsed,
        minimum,
        height,
        content: content.into_any_element(),
    }
}

struct CollapsingWidth {
    id: ElementId,
    collapsed: bool,
    minimum: f32,
    height: f32,
    content: AnyElement,
}

impl IntoElement for CollapsingWidth {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

const TWEEN: Tween = Tween::new(HOVER, standard);

impl Element for CollapsingWidth {
    type RequestLayoutState = Size<Pixels>;
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let content_size = self.content.layout_as_root(
            size(AvailableSpace::MaxContent, AvailableSpace::Definite(px(self.height))),
            window,
            cx,
        );
        let target = if self.collapsed { 0. } else { 1. };
        let now = Instant::now();
        let (expanded, moving) = window.with_element_state(id.unwrap(), |course: Option<Course>, _| {
            let mut course = if cx.reduce_motion() {
                Course::new(target, now)
            } else {
                course.unwrap_or_else(|| Course::new(target, now))
            };
            course.retarget(target, now, &TWEEN);
            (course.at(now, &TWEEN), course)
        });
        if moving {
            window.request_animation_frame();
        }
        let mut style = Style::default();
        style.size = size(
            px(self.minimum + (f32::from(content_size.width) - self.minimum).max(0.) * expanded).into(),
            px(self.height).into(),
        );
        style.flex_shrink = 0.;
        (window.request_layout(style, None, cx), content_size)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        content_size: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Clip both hitboxes and drawing while metadata returns, so it cannot cross the moving pin.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.content.prepaint_at(
                point(
                    bounds.right() - content_size.width,
                    bounds.top() + (bounds.size.height - content_size.height) / 2.,
                ),
                window,
                cx,
            );
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| self.content.paint(window, cx));
    }
}

/// A control that comes into a row and leaves it (Stop, Queue, the activity pill): its width eases between nothing
/// and its content's, and the content fades with it, so its neighbours slide aside instead of jumping. Draw it whether
/// or not it is `shown`, to keep its course. A first draw sits where `shown` says. `content` may be `None` once it has
/// gone: the control then closes as an empty space of the width it had.
pub fn reveal_width(id: impl Into<ElementId>, shown: bool, content: Option<AnyElement>) -> impl IntoElement {
    RevealWidth {
        id: id.into(),
        shown,
        content,
    }
}

struct RevealWidth {
    id: ElementId,
    shown: bool,
    content: Option<AnyElement>,
}

impl IntoElement for RevealWidth {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

const REVEAL: Tween = Tween::new(super::COLLAPSE, super::ease_out);

/// One control's course and its content's last measured size.
#[derive(Clone, Copy)]
struct Shown {
    course: Course,
    size: Size<Pixels>,
}

impl Element for RevealWidth {
    /// The content's size, and whether it is clipped (while it moves).
    type RequestLayoutState = (Size<Pixels>, bool);
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let target = if self.shown { 1. } else { 0. };
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let (value, moving, last) = window.with_element_state(id.unwrap(), |state: Option<Shown>, _| {
            let mut state = state.unwrap_or(Shown {
                course: Course::new(target, now),
                size: Size::default(),
            });
            if reduced {
                state.course = Course::new(target, now);
            }
            state.course.retarget(target, now, &REVEAL);
            let (value, moving) = state.course.at(now, &REVEAL);
            ((value, moving, state.size), state)
        });
        if moving {
            window.request_animation_frame();
        }
        let gone = !moving && value <= 0.;
        let content = self.content.take().filter(|_| !gone).map(|content| {
            div()
                .flex_none()
                .opacity(if moving { value.clamp(0., 1.) } else { 1. })
                .child(content)
                .into_any_element()
        });
        let size = match content {
            Some(mut content) => {
                let size =
                    content.layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MaxContent), window, cx);
                self.content = Some(content);
                // Measured while shown; a leaving control keeps the size it had.
                if self.shown {
                    window.with_element_state(id.unwrap(), |state: Option<Shown>, _| {
                        let mut state = state.expect("laid out above");
                        state.size = size;
                        ((), state)
                    });
                }
                if self.shown { size } else { last }
            }
            None => last,
        };
        let style = Style {
            size: gpui_kit::size(
                px(f32::from(size.width) * value.clamp(0., 1.)).into(),
                size.height.into(),
            ),
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(style, None, cx), (size, moving))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (_, clipped): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(content) = self.content.as_mut() else {
            return;
        };
        let mask = clipped.then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| content.prepaint_at(bounds.origin, window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (_, clipped): &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(content) = self.content.as_mut() else {
            return;
        };
        let mask = clipped.then_some(ContentMask { bounds });
        window.with_content_mask(mask, |window| content.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::component::h_flex;

    struct Row {
        collapsed: bool,
        minimum: f32,
        metadata_width: f32,
    }

    impl Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            h_flex()
                .w(px(240.))
                .h(px(32.))
                .gap(px(8.))
                .child(div().flex_1())
                .child(div().size(px(12.)).debug_selector(|| "pin".into()))
                .child(collapse_width(
                    "tail",
                    self.collapsed,
                    self.minimum,
                    32.,
                    div().w(px(self.metadata_width)).h(px(32.)),
                ))
        }
    }

    #[gpui_kit::test]
    fn hidden_metadata_releases_its_width_and_keeps_room_for_each_action_cluster(cx: &mut TestAppContext) {
        // Reduced motion exercises both endpoints without depending on wall-clock time.
        cx.update(|cx| cx.set_reduce_motion(true));
        for minimum in [22., 46.] {
            let (row, cx) = cx.add_window_view(|_, _| Row {
                collapsed: false,
                minimum,
                metadata_width: 100.,
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let expanded = cx.debug_bounds("pin").unwrap();
            row.update(cx, |row, cx| {
                row.collapsed = true;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let collapsed = cx.debug_bounds("pin").unwrap();
            assert_eq!(collapsed.left() - expanded.left(), px(100. - minimum));
            assert_eq!(collapsed.right() + px(8. + minimum), px(240.));
            row.update(cx, |row, cx| {
                row.collapsed = false;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(cx.debug_bounds("pin").unwrap(), expanded);
            row.update(cx, |row, cx| {
                // A short age must never make the action area narrower than its buttons.
                row.metadata_width = 8.;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(cx.debug_bounds("pin").unwrap(), collapsed);
        }
    }

    struct Bar {
        shown: bool,
        clicks: usize,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let button = div()
                .id("button")
                .debug_selector(|| "button".into())
                .size(px(28.))
                .on_click(cx.listener(|bar, _, _, cx| {
                    bar.clicks += 1;
                    cx.notify();
                }))
                .into_any_element();
            h_flex()
                .w(px(240.))
                .h(px(32.))
                .child(div().flex_1().debug_selector(|| "spacer".into()))
                .child(reveal_width("slot", self.shown, Some(button)))
        }
    }

    #[gpui_kit::test]
    fn a_shown_control_takes_its_width_and_its_clicks_and_a_gone_one_neither(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (bar, cx) = cx.add_window_view(|_, _| Bar { shown: true, clicks: 0 });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("spacer").unwrap().right(), px(212.));
        let button = cx.debug_bounds("button").expect("a shown control is drawn");
        assert_eq!(button.left(), px(212.));
        cx.simulate_click(button.center(), Modifiers::none());
        assert_eq!(bar.read_with(cx, |bar, _| bar.clicks), 1);
        bar.update(cx, |bar, cx| {
            bar.shown = false;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("spacer").unwrap().right(), px(240.));
        assert!(cx.debug_bounds("button").is_none(), "a gone control is not drawn");
    }
}
