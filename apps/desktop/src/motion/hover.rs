//! A hover fill that fades in and out over [`super::HOVER`] instead of switching, as Zeron's rows do (`HOVER_FADE`,
//! `ui/src/motion.rs`). GPUI's `.hover()` style changes in the frame the pointer crosses the edge, so a pointer that
//! runs down a list flashes each row in turn. Put [`hover_fill`] first in a `.relative()` row, in place of the row's
//! `.hover(|style| style.bg(..))`: it paints under the row's content, and only the fill fades; the text still
//! changes at once.

use std::time::Instant;

use gpui_kit::{
    App, Bounds, Corners, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseMoveEvent, Pixels, Position, Style, Window, fill, px,
};

use super::tween::{Course, Tween};
use super::{HOVER, standard};

/// The fill of a row under the pointer, `color` with corners of `radius`, fading in and out.
pub fn hover_fill(id: impl Into<ElementId>, color: Hsla, radius: f32) -> HoverFill {
    HoverFill {
        id: id.into(),
        color,
        radius: px(radius),
    }
}

pub struct HoverFill {
    id: ElementId,
    color: Hsla,
    radius: Pixels,
}

const TWEEN: Tween = Tween::new(HOVER, standard);

impl IntoElement for HoverFill {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HoverFill {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.inset.bottom = px(0.).into();
        style.inset.right = px(0.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        // A plain hitbox: it takes no clicks from the row, and the row's content over it does not hide it.
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let now = Instant::now();
        let hovered = hitbox.is_hovered(window);
        let reduced = cx.reduce_motion();
        let (value, moving) = window.with_element_state(id.unwrap(), |course: Option<Course>, _| {
            let target = if hovered { 1. } else { 0. };
            let mut course = course.unwrap_or_else(|| Course::new(target, now));
            if reduced {
                course = Course::new(target, now);
            }
            course.retarget(target, now, &TWEEN);
            (course.at(now, &TWEEN), course)
        });
        if value > 0. {
            let color = Hsla {
                a: self.color.a * value,
                ..self.color
            };
            window.paint_quad(fill(bounds, color).corner_radii(Corners::all(self.radius)));
        }
        if moving {
            window.request_animation_frame();
        }
        // Draw again when the pointer crosses the edge, as `.hover()` does.
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) != hovered {
                window.refresh();
            }
        });
    }
}
