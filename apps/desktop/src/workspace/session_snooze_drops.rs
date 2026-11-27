//! The snooze presets as drop targets. When a dragged session rests over "Snoozed"
//! (`session_drag_edge::SPRING`), the snooze menu's presets open under it, each a target: a session dropped on one
//! snoozes until its time. A drop on "Snoozed" itself still snoozes until tomorrow morning. They close when the drag
//! ends.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::session_drag::Target;
use super::sidebar::{ROW, ROW_TEXT, hint};
use super::{Workspace, glyph};
use crate::motion;
use crate::palette::Palette;

impl Workspace {
    /// The presets under "Snoozed", open only while a drag rests there and could snooze.
    pub(super) fn render_snooze_drops(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let place = super::session_drag::Place::Aside(crate::code::Aside::Snoozed);
        let presets = self
            .session_drag
            .presets
            .clone()
            .filter(|_| self.drag_could_land(&place, cx));
        let open = presets.is_some();
        let rows = presets.map(|presets| {
            let now = chrono::Local::now();
            let rows = presets.into_iter().map(|(label, at)| {
                let row = h_flex()
                    .h(px(ROW))
                    .pl(px(28.))
                    .pr(px(8.))
                    .gap(px(8.))
                    .items_center()
                    .text_size(px(ROW_TEXT))
                    .text_color(palette.secondary)
                    .child(glyph(Lucide::AlarmClock, palette.faint))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                    .child(hint(crate::snooze::wake_text(at, now), palette));
                self.drop_zone(
                    row.into_any_element(),
                    format!("snooze-{label}"),
                    Target::SnoozeUntil(at),
                    palette,
                    cx,
                )
            });
            v_flex().children(rows.collect::<Vec<_>>()).into_any_element()
        });
        motion::reveal(SharedString::from("snooze-drops"), open, rows).into_any_element()
    }
}
