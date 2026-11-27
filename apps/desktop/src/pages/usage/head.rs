//! The page's head: the title and its line, the call search on Calls, the range and Export.

use super::*;

pub(super) fn range_buttons(palette: Palette, mut live: Option<(Range, &mut Context<UsagePage>)>) -> Div {
    ui::tabs(palette).children(Range::ALL.into_iter().enumerate().map(|(ix, range)| {
        let id = ("usage-range", ix);
        let Some((picked, cx)) = live.as_mut() else {
            return ui::disabled_button(id, range.label(), palette);
        };
        let tip = range.tip();
        ui::tab(id, range.label(), *picked == range, palette)
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
    }))
}

impl UsagePage {
    /// The title and its line, with the call search (on Calls), the range and Export at the right.
    pub(super) fn render_head(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let span = self.span();
        let org = if self.ctx.org_name.is_empty() {
            "Your organization".to_string()
        } else {
            self.ctx.org_name.clone()
        };
        let subtitle = match (self.tab, &self.filters.day) {
            (Tab::Limits, _) => {
                let resets = match &self.budgets {
                    Load::Ready(budgets) => short_date(&budgets.resets_at),
                    _ => short_date(&next_month()),
                };
                format!("{org} · {}, UTC · resets {resets}", month_name(&iso_day(today())))
            }
            (Tab::Calls, Some(day)) => format!("{org} · {}, UTC", short_date(day)),
            _ => format!("{org} · {} – {}, UTC", short_date(&span.from), short_date(&span.to)),
        };
        h_flex()
            .items_end()
            .gap(px(12.))
            .child(parts::title(subtitle, palette).flex_1().min_w(px(0.)))
            .when(self.tab == Tab::Calls && self.can("spend.read_all"), |head| {
                head.child(
                    h_flex()
                        .w(px(220.))
                        .h(px(ui::CONTROL_HEIGHT))
                        .px(px(10.))
                        .rounded(px(8.))
                        .bg(palette.hover)
                        .text_size(px(13.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(Input::new(&self.call_id).appearance(false)),
                        ),
                )
            })
            .when(self.tab != Tab::Limits, |head| {
                head.child(range_buttons(palette, Some((self.range, cx))))
            })
            .when(
                matches!(self.tab, Tab::Overview | Tab::Breakdown | Tab::Calls)
                    && self.can("spend.read_all")
                    && self.can("spend.export"),
                |head| head.child(self.render_export(palette, cx)),
            )
    }
}
