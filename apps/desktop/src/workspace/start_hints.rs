//! New-session starter prompts and the subscription setup hint.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, glyph};
use crate::palette::Palette;
use nexus_desktop_core::setup_line;

const STARTERS: [(Lucide, &str, &str); 3] = [
    (
        Lucide::FileText,
        "Turn a PDF or a link into a summary doc",
        "Turn this into a summary doc: ",
    ),
    (
        Lucide::ChartColumn,
        "Build a small dashboard from a spreadsheet",
        "Build a small dashboard from this spreadsheet: ",
    ),
    (
        Lucide::Globe,
        "Make a one-page site for a project",
        "Make a one-page site for ",
    ),
];

impl Workspace {
    pub(super) fn offer_setup_line(&self) -> bool {
        matches!(self.view, View::NewCode)
            && !self.cloud_ui.on
            && self.code_form.starting.is_none()
            && self
                .code_form
                .access
                .as_ref()
                .is_some_and(|access| access.subscriptions)
            && self.plan_waiting.is_empty()
            && matches!(&self.plan_usage, Some((_, Ok(plans))) if !plans.iter().any(|plan| plan.signed_in))
            && self.code_form.setup_line.shown()
    }

    pub(super) fn note_setup_start(&mut self) {
        if self.offer_setup_line() {
            self.code_form.setup_line.note_started();
            crate::paths::save_setup_line(self.code_form.setup_line);
        }
    }

    pub(super) fn render_setup_line(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.offer_setup_line() {
            return None;
        }
        // One quiet centred line under the where line, for people with no plan linked yet.
        Some(
            h_flex()
                .w_full()
                .h(px(20.))
                .justify_center()
                .gap(px(2.))
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(palette.muted)
                .child("Use your Claude or ChatGPT plan at no extra cost")
                .child(div().text_color(palette.faint).child(" · "))
                .child(
                    div()
                        .id("setup-plan-link")
                        .cursor_pointer()
                        .text_color(palette.foreground)
                        .hover(|style| style.underline())
                        .child("Link a plan")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_native(crate::pages::PageId::Subscriptions, window, cx);
                        })),
                )
                .child(
                    div()
                        .id("setup-line-dismiss")
                        .ml(px(2.))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(glyph(Lucide::X, palette.faint).size(px(14.)))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Hide this for good").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.code_form.setup_line.dismiss();
                            crate::paths::save_setup_line(this.code_form.setup_line);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_starters(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let code_sessions = self
            .projects
            .iter()
            .map(|project| project.sessions.len())
            .sum::<usize>()
            + self.archived_code.len();
        if !setup_line::newcomer(
            self.sessions_loaded,
            self.code_overview_loaded,
            self.sessions.len(),
            code_sessions,
        ) {
            return None;
        }
        // The recents' slot and row recipe: a quiet header, then one 32 row per example.
        Some(
            v_flex()
                .w_full()
                .child(
                    div()
                        .h(px(28.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Start from an example"),
                )
                .children(STARTERS.into_iter().enumerate().map(|(index, (icon, label, prompt))| {
                    h_flex()
                        .id(("starter", index))
                        .h(px(32.))
                        .gap(px(8.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .text_color(palette.foreground)
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child(glyph(icon, palette.icon).size(px(16.)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.composer
                                .update(cx, |state, cx| state.set_value(prompt, window, cx));
                            this.composer_empty = false;
                            this.focus_composer(window, cx);
                            cx.notify();
                        }))
                }))
                .into_any_element(),
        )
    }
}
