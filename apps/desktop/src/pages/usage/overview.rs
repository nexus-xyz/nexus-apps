use super::*;
use crate::pages::usage_caps::{self, Cap, Verdict};

/// The chart's height.
const CHART_H: f32 = 220.;

fn ready<T>(load: &Load<T>) -> Option<&T> {
    match load {
        Load::Ready(value) => Some(value),
        _ => None,
    }
}

impl UsagePage {
    /* -------------------------------------------------------------- overview */

    pub(super) fn render_overview(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        if let Load::Failed(error) = &self.now {
            return v_flex().child(ui::failed(format!("GET /v1/analytics/usage · {error}"), palette));
        }
        v_flex()
            .gap(px(24.))
            .child(self.render_budget(palette, cx))
            .child(self.render_chart_card(palette, cx))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(24.))
                    .child(self.render_teams(palette, cx).flex_1().min_w(px(0.)))
                    .child(self.render_movers(palette).w(px(280.)).flex_none()),
            )
            .child(self.render_models(palette, cx))
    }

    /* -------------------------------------------------------------- the budget panel */

    /// One panel: what was spent against the budget, held, and where the month ends at this pace; four figures beside.
    fn render_budget(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let main = match &self.spend {
            Load::Ready(spend) if spend.budget.is_some_and(|b| b > 0.) => self.render_pace(spend, palette),
            _ => self.render_spent(palette, cx),
        };
        ui::card_frame(palette).child(
            h_flex()
                .items_stretch()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .justify_center()
                        .px(px(24.))
                        .py(px(20.))
                        .child(main),
                )
                .child(
                    v_flex()
                        .w(px(320.))
                        .flex_none()
                        .justify_center()
                        .py(px(6.))
                        .border_l_1()
                        .border_color(palette.border)
                        .children(self.figures(palette)),
                ),
        )
    }

    fn render_pace(&self, spend: &Spend, palette: Palette) -> Div {
        let budget = spend.budget.unwrap_or(0.);
        let resets = short_date(&spend.resets_at.clone().unwrap_or_else(next_month));
        let pace = budget_pace(spend, chrono_lite::now());
        let (tick, line, mark) = match &pace {
            Some((projected, _, verdict)) => {
                let (color, line) = match *verdict {
                    "over" => (
                        palette.destructive,
                        format!(
                            "On pace for {} by {resets}, over the {} budget. Calls stop when it runs out.",
                            cents(*projected),
                            budget_text(budget)
                        ),
                    ),
                    "close" => (
                        palette.warning,
                        format!(
                            "On pace for {} by {resets}, close to the budget. Calls stop when it runs out.",
                            cents(*projected)
                        ),
                    ),
                    _ => (
                        palette.muted,
                        format!("On pace for {} by {resets}, well within the budget.", cents(*projected)),
                    ),
                };
                (Some((share(*projected, budget).min(1.), color)), Some(line), color)
            }
            None => (None, None, palette.muted),
        };
        let out = spend.remaining <= 0.;
        v_flex()
            .child(ui::label("Spent this month", palette).text_size(px(12.)))
            .child(
                h_flex()
                    .items_baseline()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(32.))
                            .line_height(px(38.))
                            .font_weight(FontWeight::LIGHT)
                            .text_color(palette.strong)
                            .child(cents(spend.spent)),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(palette.muted)
                            .child(format!("of {} budget", budget_text(budget))),
                    ),
            )
            .child(div().mt(px(14.)).mb(px(8.)).child(parts::bar(
                10.,
                share(spend.spent, budget),
                share(spend.reserved, budget),
                tick,
                if out { palette.destructive } else { palette.strong },
                palette,
            )))
            .child(
                h_flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(short_date(&format!("{}-01", &iso_day(today())[..7])))
                    .child(if spend.reserved > 0. {
                        format!("{} held for calls in flight", cents(spend.reserved))
                    } else {
                        format!("{} left", cents(spend.remaining.max(0.)))
                    })
                    .child(format!("Resets {resets}")),
            )
            .when_some(line, |column, line| {
                column.child(
                    h_flex()
                        .mt(px(12.))
                        .gap(px(8.))
                        .child(parts::dot(mark))
                        .child(div().text_size(px(13.)).text_color(palette.foreground).child(line)),
                )
            })
    }

    /// With no monthly budget to pace: what the range spent against the range before, and what is left for a member.
    fn render_spent(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let span = self.span();
        let now = ready(&self.now);
        let before = ready(&self.before);
        let change = match (now, before) {
            (Some(now), Some(before)) => {
                let change = now.totals.spent - before.totals.spent;
                if change.abs() < 0.005 {
                    format!("Same as {}", span.versus().trim_start_matches("vs. "))
                } else {
                    format!("{} {}", cents_signed(change), span.versus())
                }
            }
            _ => "…".into(),
        };
        let mut column = v_flex()
            .child(
                ui::label(
                    format!("Spent · {} – {}", short_date(&span.from), short_date(&span.to)),
                    palette,
                )
                .text_size(px(12.)),
            )
            .child(
                div()
                    .text_size(px(32.))
                    .line_height(px(38.))
                    .font_weight(FontWeight::LIGHT)
                    .text_color(palette.strong)
                    .child(now.map(|r| cents(r.totals.spent)).unwrap_or_else(|| "…".into())),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(change),
            );
        match &self.spend {
            Load::Ready(spend) if spend.budget.is_none() => {
                let resets = short_date(&spend.resets_at.clone().unwrap_or_else(next_month));
                column = column.child(ui::note(
                    format!(
                        "{} left for you this month · resets {resets}",
                        cents(spend.remaining.max(0.))
                    ),
                    palette,
                ));
            }
            Load::Ready(_) if self.can("budgets.manage") => {
                column = column.child(
                    h_flex()
                        .mt(px(6.))
                        .gap(px(8.))
                        .child(ui::note("No monthly budget is set.", palette))
                        .child(
                            parts::action("usage-set-budget", "Set one in Limits →", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Limits, cx))),
                        ),
                );
            }
            Load::Failed(error) => column = column.child(ui::error_line(error.clone(), palette)),
            _ => {}
        }
        column
    }

    /// The four figures beside the budget: calls, who is active, who is at a limit, and what the limits stopped.
    fn figures(&self, palette: Palette) -> Vec<Div> {
        let wait = || "…".to_string();
        let now = ready(&self.now);
        let before = ready(&self.before);
        let days = self.days();
        let mut rows: Vec<(String, String, String)> = Vec::new();
        rows.push((
            "Calls".into(),
            now.map(|r| ui::count(r.totals.calls)).unwrap_or_else(wait),
            match (now, before) {
                (Some(now), Some(before)) if before.totals.calls > 0 => ratio_text(Some(
                    (now.totals.calls as f64 - before.totals.calls as f64) / before.totals.calls as f64,
                )),
                (Some(_), Some(_)) => "New".into(),
                _ => String::new(),
            },
        ));
        match ready(&self.budgets) {
            Some(b) if b.scope == "org" => {
                let active = b.people.iter().filter(|p| p.spent > 0.).count();
                let (at, near, ..) = b.counts();
                rows.push((
                    "Active people".into(),
                    format!("{} / {}", ui::count(active as u64), ui::count(b.people.len() as u64)),
                    format!("{} idle", ui::count((b.people.len() - active) as u64)),
                ));
                rows.push((
                    "At their limit".into(),
                    ui::count(at as u64),
                    if near > 0 {
                        format!("{near} near")
                    } else {
                        String::new()
                    },
                ));
            }
            Some(b) => {
                let me = b.people.first();
                rows.push((
                    "Active days".into(),
                    now.map(|r| format!("{} / {days}", r.active_days()))
                        .unwrap_or_else(wait),
                    String::new(),
                ));
                rows.push((
                    "Your limit".into(),
                    me.and_then(|p| p.limit)
                        .map(budget_text)
                        .unwrap_or_else(|| "None".into()),
                    me.map(|p| format!("{} spent", cents(p.spent))).unwrap_or_default(),
                ));
            }
            None => {
                rows.push(("Active people".into(), wait(), String::new()));
                rows.push(("At their limit".into(), wait(), String::new()));
            }
        }
        if let Some(prevented) = now.filter(|r| r.scope == "org").and_then(|r| r.prevented.as_ref()) {
            rows.push((
                "Stopped by limits".into(),
                cents(prevented.amount),
                format!(
                    "{} {}",
                    ui::count(prevented.calls),
                    if prevented.calls == 1 { "call" } else { "calls" }
                ),
            ));
        }
        rows.into_iter()
            .enumerate()
            .map(|(ix, (label, value, note))| parts::figure(label, value, note, ix == 0, palette))
            .collect()
    }

    /* -------------------------------------------------------------- the chart */

    fn render_chart_card(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let by = self.chart_by;
        let head = parts::section("Spend by day", "Click a day to see its calls", palette).child(
            ui::tabs(palette).children([By::Team, By::Model].into_iter().map(|each| {
                ui::tab(
                    SharedString::from(format!("usage-chart-{}", each.slug())),
                    each.label(),
                    by == each,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_chart_by(each, cx)))
            })),
        );
        let chart = match ready(&self.now) {
            Some(report) => {
                // This month's remaining days, at the average so far: dashed, so they read as an estimate.
                let (future, estimate) = if self.range == Range::Month {
                    (rest_of_month(today()), report.totals.spent / self.days().max(1) as f64)
                } else {
                    (Vec::new(), 0.)
                };
                self.render_chart(&stack_of(report), &future, estimate, palette, cx)
                    .into_any_element()
            }
            None => div()
                .h(px(CHART_H))
                .rounded(px(6.))
                .bg(palette.hover)
                .into_any_element(),
        };
        ui::card_frame(palette).p(px(16.)).child(head).child(chart)
    }

    pub(super) fn render_chart(
        &self,
        stack: &Stack,
        future: &[String],
        estimate: f64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let (top, ticks) = axis(stack.max.max(estimate));
        let mut days: Vec<String> = stack.columns.iter().map(|c| c.day.clone()).collect();
        days.extend(future.iter().cloned());
        let labeled = labeled_days(&days);
        let gap = px(if days.len() > 45 { 1. } else { 3. });
        let y_axis = v_flex()
            .w(px(48.))
            .h(px(CHART_H))
            .flex_none()
            .justify_between()
            .children(ticks.iter().rev().map(|tick| {
                div()
                    .text_size(px(10.))
                    .text_color(palette.faint)
                    .text_right()
                    .pr(px(6.))
                    .child(if top < 3. {
                        format!("${tick:.2}")
                    } else {
                        budget_text(*tick)
                    })
            }));
        let grid = ticks.iter().map(|tick| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(CHART_H - (*tick / top) as f32 * CHART_H))
                .h(px(1.))
                .bg(palette.border)
        });
        let bars = stack.columns.iter().enumerate().map(|(ix, column)| {
            let rows: Vec<(Hsla, String, String)> = stack
                .series
                .iter()
                .zip(column.values.iter())
                .filter(|(_, value)| **value > 0.)
                .map(|(series, value)| (hue(series.hue, palette), series.label.clone(), cents(*value)))
                .collect();
            let tip_title = format!("{} · {}", long_date(&column.day), cents(column.total));
            let tip_foot = format!("{} calls · click for its calls", ui::count(column.calls));
            let day = column.day.clone();
            let mut bar = v_flex()
                .id(("usage-bar", ix))
                .flex_1()
                .h_full()
                .justify_end()
                .cursor_pointer();
            let last = column.values.iter().rposition(|value| *value > 0.);
            for (n, value) in column.values.iter().enumerate().rev() {
                let hue_slot = stack.series.get(n).map(|s| s.hue).unwrap_or(0);
                let height = (*value / top) as f32 * CHART_H;
                if height > 0. {
                    bar = bar.child(
                        div()
                            .w_full()
                            .h(px(height.max(1.)))
                            .when(Some(n) == last, |top| top.rounded_t(px(3.)))
                            .bg(hue(hue_slot, palette)),
                    );
                }
            }
            bar.hover(|style| style.opacity(0.8))
                .tooltip(move |_, cx| {
                    let (title, rows, foot) = (tip_title.clone(), rows.clone(), tip_foot.clone());
                    cx.new(|_| parts::DayTip {
                        title,
                        rows,
                        foot,
                        palette,
                    })
                    .into()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let day = day.clone();
                    this.tab = Tab::Calls;
                    this.set_filters(|f| f.day = Some(day), cx);
                }))
                .into_any_element()
        });
        let estimate_height = (estimate / top) as f32 * CHART_H;
        let estimates = future.iter().map(|day| {
            let tip = format!("{} · about {} at this pace", long_date(day), cents(estimate));
            div()
                .id(SharedString::from(format!("usage-estimate-{day}")))
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .when(estimate_height > 0., |column| {
                    column.child(
                        div()
                            .w_full()
                            .h(px(estimate_height))
                            .rounded_t(px(3.))
                            .border_1()
                            .border_dashed()
                            .border_color(palette.faint),
                    )
                })
                .into_any_element()
        });
        let x_labels = h_flex().gap(gap).children(days.iter().enumerate().map(|(ix, day)| {
            div()
                .flex_1()
                .relative()
                .h(px(16.))
                .when(labeled.contains(&ix), |cell| {
                    cell.child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(2.))
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(palette.faint)
                            .child(short_date(day)),
                    )
                })
        }));
        let legend = h_flex()
            .flex_wrap()
            .gap(px(16.))
            .mt(px(10.))
            .children(stack.series.iter().map(|series| {
                h_flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .child(div().size(px(8.)).rounded(px(2.)).bg(hue(series.hue, palette)))
                    .child(div().text_color(palette.muted).child(series.label.clone()))
                    .child(div().text_color(palette.faint).child(cents(series.total)))
            }))
            .when(!future.is_empty(), |legend| {
                legend.child(div().flex_1()).child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Dashed: the rest of the month at this pace"),
                )
            });
        v_flex()
            .child(
                h_flex().child(y_axis).child(
                    div().flex_1().h(px(CHART_H)).relative().children(grid).child(
                        h_flex()
                            .absolute()
                            .inset_0()
                            .items_end()
                            .gap(gap)
                            .children(bars)
                            .children(estimates),
                    ),
                ),
            )
            .child(
                h_flex()
                    .child(div().w(px(48.)).flex_none())
                    .child(div().flex_1().child(x_labels)),
            )
            .child(legend)
            .when(stack.max <= 0., |chart| {
                chart.child(ui::note("No spend in these days.", palette))
            })
    }

    /* -------------------------------------------------------------- teams, movers, models */

    /// The teams' spend, each with its cap on its row when the person may see caps.
    fn render_teams(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (now, _) = self.reports_by(By::Team);
        let caps_shown = self.can("budgets.manage");
        let head = parts::section("Teams", "", palette).child(
            parts::action("usage-open-breakdown", "Open Breakdown →", palette)
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Breakdown, cx))),
        );
        let Some(report) = ready(now) else {
            return v_flex().child(head).child(ui::skeleton(4, palette));
        };
        let stack = stack_of(report);
        let (caps, failed): (Vec<Cap>, Vec<String>) = match &self.caps {
            Load::Ready(reads) => usage_caps::shown(reads, chrono::Utc::now().timestamp_millis()),
            _ => (Vec::new(), Vec::new()),
        };
        let mut rows: Vec<&Group> = report.groups.iter().collect();
        rows.sort_by(|a, b| b.figures.spent.total_cmp(&a.figures.spent));
        rows.truncate(8);
        let mut table = v_flex().child(head).child(
            ui::head_row(palette)
                .child(div().flex_1().child("Team"))
                .child(cell(120., "Share", palette.faint).text_size(px(11.)))
                .child(right(70., "Calls", palette.faint).text_size(px(11.)))
                .child(right(80., "Spent", palette.faint).text_size(px(11.)))
                .when(caps_shown, |row| {
                    row.child(cell(190., "Against cap", palette.faint).text_size(px(11.)))
                })
                .child(div().w(px(14.))),
        );
        if rows.is_empty() {
            table = table.child(ui::note("No spend in these days.", palette).py(px(12.)));
        }
        for (ix, group) in rows.into_iter().enumerate() {
            let slot = stack
                .series
                .iter()
                .find(|s| s.key == group.key)
                .map(|s| s.hue)
                .unwrap_or(0);
            let slug = group.slug.clone().filter(|_| group.has_children);
            let cap = group
                .slug
                .as_ref()
                .and_then(|slug| caps.iter().find(|cap| &cap.slug == slug));
            table = table.child(
                ui::row(palette)
                    .min_h(px(44.))
                    .mb(px(0.))
                    .border_b_1()
                    .border_color(palette.border)
                    .rounded(px(0.))
                    .id(("usage-team", ix))
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.hover))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(8.))
                            .child(div().size(px(8.)).flex_none().rounded(px(2.)).bg(hue(slot, palette)))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(palette.strong)
                                    .child(group.label.clone()),
                            ),
                    )
                    .child(share_cell(120., 70., group.share, palette))
                    .child(right(70., ui::count(group.figures.calls), palette.muted))
                    .child(right(80., cents(group.figures.spent), palette.strong))
                    .when(caps_shown, |row| row.child(self.cap_cell(ix, group, cap, palette)))
                    .child(ui::icon(gpui_kit::assets::IconName::ChevronRight, palette.icon))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = Tab::Breakdown;
                        this.set_by(By::Team, slug.clone(), cx);
                    })),
            );
        }
        if caps_shown {
            let note = match &self.caps {
                Load::Failed(error) => Some(format!("Team caps did not load: {error}")),
                _ => usage_caps::failed_text(&failed),
            };
            table = table.child(
                h_flex()
                    .mt(px(10.))
                    .gap(px(8.))
                    .child(
                        ui::note("The tick marks where a cap's window ends at this pace.", palette).text_size(px(12.)),
                    )
                    .when_some(note, |row, note| {
                        row.child(div().flex_1())
                            .child(ui::error_line(note, palette).text_size(px(12.)))
                            .child(
                                ui::web_link("usage-caps-retry", "Retry", palette)
                                    .on_click(cx.listener(|this, _, _, cx| this.load_caps(cx))),
                            )
                    }),
            );
        }
        table
    }

    /// A team's cap on its row: a small meter with the pace tick, and "$9,000 · close".
    fn cap_cell(&self, ix: usize, group: &Group, cap: Option<&Cap>, palette: Palette) -> AnyElement {
        let Some(cap) = cap else {
            let text = match (&self.caps, group.slug.is_some()) {
                (Load::Loading, true) => "…",
                (Load::Failed(_), true) => "–",
                _ => "No cap",
            };
            return cell(190., text, palette.faint).into_any_element();
        };
        let pace = &cap.pace;
        let (spent, held, at_end) = pace.shares();
        let color = match pace.verdict {
            Verdict::At | Verdict::Over => palette.destructive,
            Verdict::Near => palette.warning,
            Verdict::Within => palette.muted,
        };
        let tip = format!("{}. {}", usage_caps::figures(pace), usage_caps::note(pace));
        h_flex()
            .id(("usage-team-cap", ix))
            .w(px(190.))
            .flex_none()
            .gap(px(8.))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(div().w(px(70.)).flex_none().child(parts::bar(
                6.,
                spent,
                held,
                (pace.verdict != Verdict::At).then_some((at_end, palette.foreground)),
                color,
                palette,
            )))
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(format!("{} · {}", budget_text(pace.limit), pace.verdict.word())),
            )
            .into_any_element()
    }

    fn render_movers(&self, palette: Palette) -> Div {
        let span = self.span();
        let other = if self.chart_by == By::Team { "Model" } else { "Team" };
        let mut list = v_flex().child(parts::section("Top movers", span.versus(), palette));
        match (&self.now, &self.before, &self.now_other, &self.before_other) {
            (Load::Ready(now), Load::Ready(before), Load::Ready(now_other), Load::Ready(before_other)) => {
                let moved = movers(&[(self.chart_by.label(), now, before), (other, now_other, before_other)]);
                if moved.is_empty() {
                    list = list.child(ui::note("Nothing moved more than a cent.", palette).py(px(12.)));
                }
                for (label, kind, change, ratio) in moved.into_iter().take(5) {
                    list = list.child(
                        h_flex()
                            .min_h(px(44.))
                            .gap(px(10.))
                            .border_b_1()
                            .border_color(palette.border)
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(22.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(if palette.dark { palette.selected } else { palette.hover })
                                    .child(ui::icon(
                                        if change < 0. {
                                            gpui_kit::assets::IconName::TrendingDown
                                        } else {
                                            gpui_kit::assets::IconName::TrendingUp
                                        },
                                        palette.icon,
                                    )),
                            )
                            .child(ui::two_lines(label, kind, palette).flex_1())
                            .child(
                                v_flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.strong)
                                            .child(cents_signed(change)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(palette.faint)
                                            .child(ratio_text(ratio)),
                                    ),
                            ),
                    );
                }
            }
            (Load::Failed(error), ..) | (_, Load::Failed(error), ..) => {
                list = list.child(ui::note(error.clone(), palette))
            }
            _ => list = list.child(ui::skeleton(3, palette)),
        }
        list
    }

    /// The models' spend: a row opens the model's calls.
    fn render_models(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (now, _) = self.reports_by(By::Model);
        let head = parts::section("Models", "Click a model to see its calls", palette);
        let Some(report) = ready(now) else {
            return v_flex().child(head).child(ui::skeleton(3, palette));
        };
        let mut rows: Vec<&Group> = report.groups.iter().collect();
        rows.sort_by(|a, b| b.figures.spent.total_cmp(&a.figures.spent));
        rows.truncate(8);
        if rows.is_empty() {
            return v_flex();
        }
        let mut table = v_flex().child(head).child(
            ui::head_row(palette)
                .child(div().flex_1().child("Model"))
                .child(cell(170., "Share", palette.faint).text_size(px(11.)))
                .child(right(NUM_W, "Calls", palette.faint).text_size(px(11.)))
                .child(right(120., "Avg. cost / call", palette.faint).text_size(px(11.)))
                .child(right(NUM_W, "Spent", palette.faint).text_size(px(11.)))
                .child(div().w(px(14.))),
        );
        for (ix, group) in rows.into_iter().enumerate() {
            let key = group.key.clone();
            let average = if group.figures.calls > 0 {
                usd_decimal(&format!("{:.6}", group.figures.spent / group.figures.calls as f64))
            } else {
                "–".into()
            };
            table = table.child(
                ui::row(palette)
                    .min_h(px(44.))
                    .mb(px(0.))
                    .border_b_1()
                    .border_color(palette.border)
                    .rounded(px(0.))
                    .id(("usage-model", ix))
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(model_name(&group.label)),
                    )
                    .child(share_cell(170., 110., group.share, palette))
                    .child(right(NUM_W, ui::count(group.figures.calls), palette.muted))
                    .child(right(120., average, palette.muted))
                    .child(right(NUM_W, cents(group.figures.spent), palette.strong))
                    .child(ui::icon(gpui_kit::assets::IconName::ChevronRight, palette.icon))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let key = key.clone();
                        this.tab = Tab::Calls;
                        this.set_filters(|f| f.model = Some(key), cx);
                    })),
            );
        }
        table
    }
}

/// A share as a small grey meter and its percent.
fn share_cell(width: f32, meter_width: f32, value: f64, palette: Palette) -> Div {
    h_flex()
        .w(px(width))
        .flex_none()
        .gap(px(8.))
        .child(meter(meter_width, value, 0., palette.muted, palette))
        .child(div().text_size(px(12.)).text_color(palette.faint).child(percent(value)))
}
