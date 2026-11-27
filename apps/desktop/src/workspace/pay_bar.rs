//! The chat composer's pay notice (`crate::pay_notice`), above the composer: a card when Nexus cannot pay
//! the next call, or a quiet line while the free start's end is near. Brand v5's composer notice (P73·E): radius 14,
//! padding 14, raised, a hairline edge on light, the title 16/22 in `text-1`, the body 15/20 in `text-2`, one action.
//! A payment that needs someone carries an 8 px amber dot (P73·H). It never blocks sending.

use gpui_kit::component::h_flex;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use super::settings_registry::Dest;
use crate::palette::Palette;
use crate::pay_notice::{self, PayNotice};

/// Brand v5's `--nx-status-waiting`: the amber of a payment that needs someone.
fn waiting(palette: Palette) -> Hsla {
    if palette.dark {
        rgb(0xFFD60A).into()
    } else {
        rgb(0xC79A00).into()
    }
}

impl Workspace {
    /// While the free start pays the next call, the highest output price it pays for: the model menu marks those models.
    pub(super) fn free_start_cap(&self) -> Option<f64> {
        let spend = self.spend.as_ref()?;
        pay_notice::free_start_pays(spend)
            .then_some(self.free_start_max)
            .flatten()
    }

    /// The quiet line's share, for its meter (`pay_notice::line_share`).
    fn pay_line_share(&self) -> Option<i64> {
        pay_notice::line_share(self.spend.as_ref()?)
    }

    fn pay_notice(&self) -> Option<PayNotice> {
        let now = chrono::Utc::now().timestamp_millis();
        pay_notice::notice(self.spend.as_ref()?, now)
    }

    pub(super) fn render_pay_notice(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.pay_notice()?;
        let row = match notice {
            // 80% used or ends soon: someone should act, so the line carries amber (P73·H): a 24 x 4 meter of the share
            // used where the line says one, else the 6 px dot.
            PayNotice::Line(line) => {
                let mark = match self.pay_line_share() {
                    Some(share) => div()
                        .w(px(24.))
                        .h(px(4.))
                        .flex_none()
                        .rounded(px(2.))
                        .bg(palette.border)
                        .child(
                            div()
                                .h_full()
                                .w(px(24. * share as f32 / 100.))
                                .rounded(px(2.))
                                .bg(waiting(palette)),
                        ),
                    None => div().size(px(6.)).flex_none().rounded_full().bg(waiting(palette)),
                };
                h_flex()
                    .px(px(14.))
                    .gap(px(8.))
                    .items_center()
                    .text_size(px(13.))
                    .text_color(palette.faint)
                    .child(mark)
                    .child(line)
                    .into_any_element()
            }
            PayNotice::Refused { title, body, action } => {
                // A payment that needs someone carries the dot; a limit reached as set (used up, ran out) does not.
                let dot = matches!(action, "Fix payment" | "Confirm payment" | "Pay invoice");
                h_flex()
                    .p(px(14.))
                    .gap(px(12.))
                    .items_start()
                    .rounded(px(14.))
                    .bg(palette.raised)
                    .when(!palette.dark, |card| card.border_1().border_color(palette.border))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .items_center()
                                    .when(dot, |head| {
                                        head.child(div().size(px(8.)).flex_none().rounded_full().bg(waiting(palette)))
                                    })
                                    .text_size(px(16.))
                                    .line_height(px(22.))
                                    .text_color(palette.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .line_height(px(20.))
                                    .text_color(palette.muted)
                                    .child(body),
                            ),
                    )
                    .child(
                        div()
                            .id("pay-notice-action")
                            .flex_none()
                            .cursor_pointer()
                            .rounded_full()
                            .px(px(12.))
                            .py(px(6.))
                            .text_size(px(13.))
                            .bg(palette.primary)
                            .text_color(palette.primary_foreground)
                            .hover(|style| style.opacity(0.85))
                            // Billing holds every action here, the usage limit's Change limit too (`Panel::Limit`).
                            .on_click(cx.listener(|this, _, window, cx| this.open_dest(Dest::Billing, window, cx)))
                            .child(action),
                    )
                    .into_any_element()
            }
        };
        Some(row)
    }
}
