//! Reordering a Code session's queue by drag and drop. Each "On queue" row has a grip; drag it onto another queued
//! row and the message moves there. The session engine owns the order (`Workbench::queue_move`): the dock redraws when
//! its `QueueMove` event comes back, so every window and a remote host's clients see the same order.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, glyph};
use crate::palette::Palette;

/// The queued message being dragged: its session, its `seq` and its first words for the preview.
#[derive(Clone)]
pub(super) struct QueuedDrag {
    thread: String,
    seq: i64,
    line: SharedString,
    palette: Palette,
}

impl QueuedDrag {
    pub(super) fn new(thread: String, seq: i64, line: SharedString, palette: Palette) -> Self {
        Self {
            thread,
            seq,
            line,
            palette,
        }
    }

    pub(super) fn seq(&self) -> i64 {
        self.seq
    }
}

/// The row under the pointer while it moves: a raised copy of the dragged row.
impl Render for QueuedDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        h_flex()
            .w(px(320.))
            .h(px(32.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .shadow_md()
            .text_size(px(13.5))
            .text_color(palette.foreground)
            .child(glyph(Lucide::GripVertical, palette.faint))
            .child(div().flex_1().min_w(px(0.)).truncate().child(self.line.clone()))
    }
}

/// Where a queued message dropped on another lands, as `queue_move`'s `before`. Dragged down, it goes just after the
/// row it was dropped on; dragged up, just before it. None: it stays where it is.
pub(super) fn drop_before(order: &[i64], dragged: i64, target: i64) -> Option<Option<i64>> {
    let from = order.iter().position(|&seq| seq == dragged)?;
    let to = order.iter().position(|&seq| seq == target)?;
    match from.cmp(&to) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Less => Some(order.get(to + 1).copied()),
        std::cmp::Ordering::Greater => Some(Some(target)),
    }
}

/// The grip at the start of a queued row: the one place a drag starts, so a click on the row's buttons never moves it.
pub(super) fn grip(id: impl Into<ElementId>, drag: QueuedDrag, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_grab()
        .hover(|style| style.bg(palette.hover))
        .child(glyph(Lucide::GripVertical, palette.faint))
        .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
}

impl Workspace {
    /// `drag` was dropped on the queued row `target` of the open session: the engine moves it there.
    pub(super) fn drop_queued(&mut self, drag: &QueuedDrag, target: i64, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            return;
        };
        if view.id != drag.thread || view.dock.acting.contains(&drag.seq) {
            return;
        }
        let order: Vec<i64> = view
            .blocks
            .iter()
            .filter_map(|block| match block {
                crate::code::Block::Queued { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        let Some(before) = drop_before(&order, drag.seq, target) else {
            return;
        };
        if let Some(runs) = self.runs(&drag.thread) {
            runs.queue_move(drag.thread.clone(), drag.seq, before);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::drop_before;

    #[test]
    fn a_row_dragged_down_lands_after_its_target_and_one_dragged_up_before_it() {
        let order = [2, 3, 4, 5];
        assert_eq!(drop_before(&order, 2, 4), Some(Some(5)));
        assert_eq!(drop_before(&order, 2, 5), Some(None), "onto the last row: it goes last");
        assert_eq!(drop_before(&order, 5, 3), Some(Some(3)));
        assert_eq!(
            drop_before(&order, 5, 2),
            Some(Some(2)),
            "onto the first row: it goes first"
        );
        assert_eq!(drop_before(&order, 3, 3), None, "onto itself: nothing moves");
        assert_eq!(drop_before(&order, 9, 3), None, "no longer queued: nothing moves");
        assert_eq!(drop_before(&order, 3, 9), None);
    }
}
