//! The question at first start (PRIVACY.md, "Turn it on or off"): "Help improve Nexus by sending usage
//! data?", with "Send usage data" and "Don't send". Asked only by a release build, only while the person has not chosen,
//! and never while `NEXUS_TELEMETRY` or `DO_NOT_TRACK` turns usage data off. Neither button is the highlighted one: the
//! default is no. The answer is the "Send usage data" setting (Settings, Privacy).

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use crate::i18n::tr;
use crate::palette::Palette;

/// Whether the question was answered in this run, even if the answer could not be saved: it is asked again next start.
#[derive(Default)]
pub struct UsagePrompt {
    answered: bool,
}

impl Workspace {
    fn answer_usage(&mut self, on: bool, cx: &mut Context<Self>) {
        // A failed save leaves the setting unchosen, so the next start asks again.
        let _ = crate::paths::save_usage_data(on);
        self.usage_prompt.answered = true;
        cx.notify();
    }

    pub(super) fn render_usage_prompt(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.usage_prompt.answered || !crate::paths::asks_usage_data() {
            return None;
        }
        let button = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(6.))
                .rounded(px(8.))
                .text_size(px(13.))
                .border_1()
                .border_color(palette.border)
                .text_color(palette.foreground)
                .hover(|style| style.bg(palette.hover))
                .cursor_pointer()
                .child(label)
        };
        let dialog = v_flex()
            .id("usage-prompt")
            .occlude()
            .w(px(460.))
            .max_w(relative(0.94))
            .p(px(20.))
            .gap(px(12.))
            .rounded(px(14.))
            .bg(palette.pop)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.strong)
                    .child(tr("Help improve Nexus by sending usage data?")),
            )
            .child(div().text_size(px(13.)).text_color(palette.muted).child(tr(
                "Nexus would send how often it is used, its version, and errors by name. Never your prompts, code, \
                 files or keys. PRIVACY.md lists every field. Change your answer any time in Settings, Privacy.",
            )))
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        button("usage-dont-send", tr("Don't send"))
                            .on_click(cx.listener(|this, _, _, cx| this.answer_usage(false, cx))),
                    )
                    .child(
                        button("usage-send", tr("Send usage data"))
                            .on_click(cx.listener(|this, _, _, cx| this.answer_usage(true, cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("usage-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(dialog),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}
