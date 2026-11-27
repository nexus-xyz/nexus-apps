//! Dragging a project in the sidebar to give the list the person's own order. A project's head lifts into a raised
//! copy under the pointer, and its place dims. Over another project, a line shows where it lands: above it with the
//! pointer on its head's upper half, below it anywhere else. Pinned projects stay above the others, so a project
//! lands only among the ones pinned as it is. The order keeps (`project_groups.rs`) and `project_list::arrange` reads
//! it.
//!
//! What the drag is over is [`ProjectDragState`], a plain state with no UI. This file draws it and tells it what the
//! pointer does. A session drag (`session_drag.rs`) and a project drag never go on at once: each one's lift ends the
//! other's.

use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::folder_motion::Transition;
use super::sidebar::{ROW, ROW_TEXT};
use super::{Workspace, glyph};
use crate::motion;
use crate::palette::Palette;

/// How far the dragged project's place dims: still there, so the list does not jump while the drag goes on.
const LIFTED: f32 = 0.4;

/// Where a project sits in the list, for a project dropped on it.
#[derive(Clone)]
pub(super) struct ProjectSlot {
    /// The project's key (`project_list::Entry::key`), and whether it is pinned.
    pub key: String,
    pub pinned: bool,
    /// Every project's key, in the order on screen.
    pub listed: Rc<[String]>,
}

/// Where a dropped project lands: before the project `key`, or `after` it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Spot {
    pub key: String,
    pub after: bool,
}

/// The project being dragged.
#[derive(Debug, Clone, PartialEq)]
struct Lifted {
    key: String,
    pinned: bool,
}

/// The sidebar's project drag: the lifted project, the spot it would land at, and its place's dimming.
#[derive(Default)]
pub(super) struct ProjectDragState {
    lifted: Option<Lifted>,
    over: Option<Spot>,
    lift: Transition,
}

impl ProjectDragState {
    /// Project `key` was lifted.
    fn lift(&mut self, key: String, pinned: bool) {
        self.lifted = Some(Lifted { key, pinned });
        self.over = None;
        self.lift = Transition::settled(0.);
        self.lift.retarget(1., motion::UI);
    }

    /// The pointer moved, `inside` the project `slot` or out of it, on the half that lands `after` it or before it.
    /// Only a project pinned as the lifted one is takes it, and never the lifted one itself. True when the spot
    /// changed. GPUI tells every project of each move, so leaving one never clears another's spot.
    fn moved(&mut self, slot: &ProjectSlot, inside: bool, after: bool) -> bool {
        let Some(lifted) = &self.lifted else {
            return false;
        };
        let takes = inside && slot.pinned == lifted.pinned && slot.key != lifted.key;
        let next = if takes {
            Some(Spot {
                key: slot.key.clone(),
                after,
            })
        } else if self.over.as_ref().is_some_and(|spot| spot.key == slot.key) {
            None
        } else {
            return false;
        };
        if next == self.over {
            return false;
        }
        self.over = next;
        true
    }

    /// The project was let go on `slot`: the list's new order, or None when it lands where it is or not there.
    fn dropped(&mut self, slot: &ProjectSlot) -> Option<Vec<String>> {
        let lifted = self.lifted.take()?;
        let spot = self.over.take().filter(|spot| spot.key == slot.key)?;
        let listed: Vec<&str> = slot.listed.iter().map(String::as_str).collect();
        super::session_order::moved_all(&listed, &[lifted.key.as_str()], &spot.key, spot.after)
    }

    /// Ends the drag with nothing landed. True when one went on.
    fn cancel(&mut self) -> bool {
        self.over = None;
        self.lifted.take().is_some()
    }

    /// The dragged project's key, while GPUI still has the drag. A drag let go outside every project ends with no
    /// event: GPUI only stops it.
    fn dragging(&self, cx: &App) -> Option<&str> {
        self.lifted
            .as_ref()
            .filter(|_| cx.has_active_drag())
            .map(|lifted| lifted.key.as_str())
    }

    /// Where project `key` draws the line: over its top edge (false), under its bottom one (true), or not at all.
    fn line(&self, key: &str, cx: &App) -> Option<bool> {
        self.dragging(cx)?;
        self.over.as_ref().filter(|spot| spot.key == key).map(|spot| spot.after)
    }
}

/// The raised copy of a project's head drawn under the pointer.
#[derive(Clone)]
pub(super) struct ProjectDrag {
    key: String,
    pinned: bool,
    name: SharedString,
    palette: Palette,
}

impl Render for ProjectDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        h_flex()
            .id("project-drag-ghost")
            .w(px(248.))
            .h(px(ROW))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .shadow_md()
            .text_size(px(ROW_TEXT))
            .text_color(palette.foreground)
            .child(glyph(Lucide::GripVertical, palette.faint).size(px(14.)))
            .child(div().flex_1().min_w(px(0.)).truncate().child(self.name.clone()))
            .when(self.pinned, |ghost| ghost.child(glyph(Lucide::Pin, palette.faint)))
            // It lifts off the list as a session's row does (`session_drag.rs`).
            .with_animation(
                "project-drag-lift",
                Animation::new(motion::UI).with_easing(motion::ease_out),
                |ghost, delta| ghost.opacity(0.7 + 0.3 * delta).mt(px(2. * (1. - delta))),
            )
    }
}

impl Workspace {
    /// `head`, a project's head, made draggable: pressed and moved, it lifts the project named `name`.
    pub(super) fn project_drag_source(
        &self,
        head: AnyElement,
        slot: &ProjectSlot,
        name: &str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drag = ProjectDrag {
            key: slot.key.clone(),
            pinned: slot.pinned,
            name: SharedString::from(name.to_string()),
            palette,
        };
        let workspace = cx.entity().downgrade();
        let selector = format!("project-drag-{}", slot.key);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .child(head)
            .on_drag(drag, move |drag, _, _, cx| {
                let begun = workspace.update(cx, |this, cx| {
                    this.peek.dismiss();
                    // A session drag GPUI let go of outside every target left its state: end it.
                    this.session_drag.stop_assists();
                    this.drag_step(super::session_drag::Event::Cancel, cx);
                    this.project_drag.lift(drag.key.clone(), drag.pinned);
                    cx.notify();
                });
                if let Err(error) = begun {
                    eprintln!("nexus-desktop-gpui: a project drag had no window: {error}");
                }
                cx.new(|_| drag.clone())
            })
            .into_any_element()
    }

    /// `block`, a project's head and its sessions, as a target for a dragged project: the line shows where it lands,
    /// and the dragged project's own block dims.
    pub(super) fn project_drop_target(
        &self,
        block: AnyElement,
        slot: ProjectSlot,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = slot.key.clone();
        let line = self.project_drag.line(&key, cx);
        let dragged = self.project_drag.dragging(cx) == Some(key.as_str());
        let dropped = slot.clone();
        let selector = format!("project-drop-{key}");
        let frame = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .relative()
            .child(block)
            .when_some(line, |frame, after| frame.child(drop_line(&key, after, palette)))
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                let position = event.event.position;
                // Above the project only from its head's upper half: below it anywhere else, as a session's family.
                let after = position.y > event.bounds.top() + px(ROW / 2.);
                if this.project_drag.moved(&slot, event.bounds.contains(&position), after) {
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(move |this, _: &ProjectDrag, _, cx| {
                if let Some(order) = this.project_drag.dropped(&dropped) {
                    this.groups.groups.set_project_order(order);
                    this.save_groups();
                }
                cx.notify();
            }));
        if dragged {
            self.project_drag
                .lift
                .draw(div().child(frame), format!("project-lifted-{key}"), |frame, value| {
                    frame.opacity(1. - (1. - LIFTED) * value)
                })
        } else {
            frame.into_any_element()
        }
    }

    /// Esc during a project drag: GPUI lets go of it, and nothing lands. False when no drag goes on.
    pub(super) fn cancel_project_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.project_drag.dragging(cx).is_none() {
            return false;
        }
        cx.stop_active_drag(window);
        self.project_drag.cancel();
        cx.notify();
        true
    }

    /// A session row was lifted: a project drag GPUI let go of outside every project ends.
    pub(super) fn end_project_drag(&mut self) {
        self.project_drag.cancel();
    }
}

/// The line where a dropped project lands: over the block's top edge, or under its bottom one.
fn drop_line(key: &str, after: bool, palette: Palette) -> impl IntoElement {
    div()
        .absolute()
        .left(px(8.))
        .right(px(8.))
        .h(px(2.))
        .when(after, |line| line.bottom(px(-1.)))
        .when(!after, |line| line.top(px(-1.)))
        .rounded_full()
        .bg(palette.primary)
        .with_animation(
            SharedString::from(format!("project-drop-line-{key}-{after}")),
            Animation::new(motion::UI).with_easing(motion::ease_out),
            |line, delta| line.opacity(delta).mr(relative(1. - delta)),
        )
}

#[cfg(test)]
#[path = "project_drag_tests.rs"]
mod tests;
