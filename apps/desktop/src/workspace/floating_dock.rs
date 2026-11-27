//! The composer floats over the transcript's foot, as Zeron's does: the transcript runs on under it, and the
//! composer's glass (`glass.rs`) blurs it as it scrolls past. Below and beside the box, a fade into the window's
//! color takes the lines out before they reach the window's edge.
//!
//! The dock's height changes with what it holds (a longer prompt, queued messages, an approval card), so it is
//! measured each frame. The list's last item (its tail) leaves that much room under the last message, so the end of
//! the transcript still scrolls clear of the dock.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    Context, IntoElement, ParentElement, Pixels, Styled, canvas, div, linear_color_stop, linear_gradient, px,
};

use super::Workspace;
use crate::palette::Palette;

/// The dock's height at the last frame, in logical pixels.
#[derive(Clone, Default)]
pub(super) struct DockHeight(Rc<Cell<f32>>);

impl Workspace {
    /// The room the list's tail leaves for the dock over it.
    pub(super) fn dock_room(&self) -> Pixels {
        px(self.dock_height.0.get())
    }

    /// `dock`, over the foot of the transcript area (a `.relative()` box), with the fade under it.
    pub(super) fn floating_dock(
        &self,
        dock: impl IntoElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        floating_dock(dock, self.dock_height.clone(), palette, cx, |this, _| {
            this.remeasure_tails()
        })
    }

    /// The tails of the chat's and the Code session's lists: the item after the last.
    fn remeasure_tails(&mut self) {
        let tail = self.entries.len();
        self.chat_list.list.remeasure_items(tail..tail + 1);
        if let super::View::Code(view) = &self.view {
            let tail = view.blocks.len();
            view.list.remeasure_items(tail..tail + 1);
        }
    }
}

/// The same measured overlay for sessions and sidechats.
pub(super) fn floating_dock<T: 'static>(
    dock: impl IntoElement,
    height: DockHeight,
    palette: Palette,
    cx: &mut Context<T>,
    resized: fn(&mut T, &mut Context<T>),
) -> gpui_kit::Div {
    let workspace = cx.weak_entity();
    let measure = canvas(
        move |bounds, _, cx| {
            let next = f32::from(bounds.size.height).round();
            if height.0.get() != next {
                height.0.set(next);
                let workspace = workspace.clone();
                // The tails' heights changed: measure them again, and draw again with the new room.
                cx.defer(move |cx| {
                    let _ = workspace.update(cx, |this, cx| {
                        resized(this, cx);
                        cx.notify();
                    });
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    let ground = crate::frost::Look::now(&palette).veil;
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .child(measure)
        // From clear at the box's middle to the window's color (its veil, `frost.rs`) at its foot.
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(relative_half())
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(ground.opacity(0.), 0.),
                    linear_color_stop(ground, 0.7),
                )),
        )
        .child(dock)
}
impl DockHeight {
    pub(super) fn from_room(room: f32) -> Self {
        Self(Rc::new(Cell::new(room)))
    }
    pub(super) fn room(&self) -> Pixels {
        px(self.0.get())
    }
}

fn relative_half() -> gpui_kit::DefiniteLength {
    gpui_kit::relative(0.5)
}
