//! The sessions put aside ("Snoozed", "Settled", "Archived"): quiet rows that close the projects, above the servers.
//! Each head shows its kind's icon at rest. Under the pointer, or while open, the icon cross-fades to the fold's
//! chevron, so the row says it folds only when that is what the person reaches for.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::folder_motion::Transition;
use super::sidebar::hint;
use super::{Workspace, disclosure, glyph};
use crate::motion;
use crate::palette::Palette;

/// A head's height: lower than a project's ROW, so the group reads as secondary, and still a full click target.
pub(super) const HEAD: f32 = 28.;
/// A head's text, one step under a project's 13.5px.
const HEAD_TEXT: f32 = 13.;
/// How far the chevron slides in from the left as it appears.
const SLIDE: f32 = 3.;

impl Workspace {
    /// The aside groups and Archived, set a little apart from the projects they follow.
    pub(super) fn render_history(&self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = crate::code::now_ms();
        v_flex()
            .id("sidebar-history")
            .mt(px(4.))
            .child(self.render_aside_groups(now, palette, window, cx))
            .child(self.render_archived(palette, window, cx))
            .into_any_element()
    }

    /// A history group's head: its icon (the chevron under the pointer or while open), its name and its count.
    /// The name keys its motion. The caller adds the click that folds it.
    pub(super) fn history_head(
        &self,
        key: &'static str,
        icon: Lucide,
        count: Option<usize>,
        open: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let reveal = if open {
            Transition::settled(1.)
        } else {
            self.history_hover_motion.get(key).copied().unwrap_or_default()
        };
        let mark = reveal.draw(
            div().relative().size(px(16.)).flex_none(),
            format!("history-mark-{key}"),
            move |slot, value| {
                slot.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(1. - value)
                        .child(glyph(icon, palette.faint).size(px(14.))),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(-SLIDE * (1. - value)))
                        .w(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(value)
                        .child(disclosure(
                            SharedString::from(format!("history-chevron-{key}")),
                            open,
                            palette.icon,
                        )),
                )
            },
        );
        h_flex()
            .id(SharedString::from(format!("history-{key}")))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.history_hover_motion
                    .entry(key)
                    .or_default()
                    .retarget(if *hovered { 1. } else { 0. }, motion::HOVER);
                cx.notify();
            }))
            .h(px(HEAD))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .text_size(px(HEAD_TEXT))
            .text_color(palette.muted)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .child(mark)
            .child(div().flex_1().min_w(px(0.)).truncate().child(key))
            .when_some(count, |head, count| head.child(hint(count.to_string(), palette)))
    }
}
