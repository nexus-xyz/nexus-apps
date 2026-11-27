use super::*;
use crate::limit_stop;

/// The People table's narrowest layout: a 140px name and its fixed columns. Narrower, it scrolls sideways.
const NAME_MIN_WIDTH: f32 = 140.;
const TABLE_MIN_WIDTH: f32 = NAME_MIN_WIDTH + 260. + 150. + 60. + 3. * 12.;

impl UsagePage {
    /* -------------------------------------------------------------- limits */

    /// The amount field with Save, `extra` and Cancel, and the error under them.
    pub(super) fn amount_editor(&self, extra: Vec<AnyElement>, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .items_end()
            .gap(px(6.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .w(px(140.))
                            .h(px(ui::CONTROL_HEIGHT))
                            .px(px(10.))
                            .gap(px(4.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette.muted)
                            .bg(ui::inset(palette))
                            .text_size(px(13.))
                            .child(div().text_color(palette.faint).child("$"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(Input::new(&self.amount).appearance(false)),
                            ),
                    )
                    .child(
                        ui::primary_button(
                            "usage-amount-save",
                            if self.saving { "Saving…" } else { "Save" },
                            palette,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save_amount(cx))),
                    )
                    .children(extra)
                    .child(
                        ui::ghost_button("usage-amount-cancel", "Cancel", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.editing = None;
                                this.edit_error = None;
                                this.confirm_clear = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when_some(self.edit_error.clone(), |c, error| {
                c.child(ui::error_line(error, palette))
            })
    }

    pub(super) fn render_limits(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut body = v_flex().gap(px(24.));
        if let Load::Failed(error) = &self.me {
            body = body.child(
                h_flex()
                    .gap(px(8.))
                    .child(ui::error_line(
                        format!("Account permissions did not load: {error}"),
                        palette,
                    ))
                    .child(
                        ui::button("usage-account-retry", "Retry account", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load_account(cx))),
                    ),
            );
        }
        body = body.child(self.render_person_limits(palette, cx));
        if self.can("org.billing") {
            let card = self.billing_limit.clone();
            body = body.child(self.ctx.settings_fields.field(limit_stop::USAGE_ROW, card, None, cx));
        }
        body.child(
            h_flex()
                .gap(px(8.))
                .child(ui::note("Product budgets are set in Nexus web.", palette))
                .child(
                    ui::web_link("usage-products-web", "Open product budgets", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.ctx.open_web("settings/budgets", cx))),
                ),
        )
    }

    pub(super) fn render_person_limits(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        if let Load::Failed(error) = &self.budgets {
            return v_flex()
                .child(ui::failed(format!("GET /v1/budgets · {error}"), palette))
                .child(
                    ui::button("usage-budgets-retry", "Retry limits", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load_money(cx))),
                );
        }
        let Load::Ready(budgets) = &self.budgets else {
            return v_flex().child(ui::skeleton(6, palette));
        };
        if budgets.scope == "own" {
            let own = self.render_own_limit(budgets, palette);
            return self.ctx.settings_fields.field(limit_stop::PERSON_ROW, own, None, cx);
        }
        let mut body = v_flex()
            .gap(px(24.))
            .child(self.render_rules(budgets, palette, cx))
            .child(self.render_people(budgets, palette, cx));
        if budgets.can_grant {
            body = body.child(self.render_managers(budgets, palette, cx));
        }
        body
    }

    /// A member's own limit this month.
    fn render_own_limit(&self, budgets: &Budgets, palette: Palette) -> Div {
        let month = month_name(&budgets.period);
        let resets = short_date(&budgets.resets_at);
        let Some(me) = budgets.people.first() else {
            return v_flex().child(ui::note("You have no limit of your own this month.", palette));
        };
        let detail = match me.limit {
            Some(limit) => format!(
                "{} spent of {} · {} left · resets {resets}",
                cents(me.spent),
                budget_text(limit),
                cents(me.remaining.unwrap_or(0.).max(0.))
            ),
            None => format!("{} spent · no limit · resets {resets}", cents(me.spent)),
        };
        let color = if me.at_limit {
            palette.destructive
        } else {
            palette.strong
        };
        ui::group(
            [Some(
                ui::card_row()
                    .child(ui::setting_lines(format!("Your limit · {month}"), detail, palette).flex_1())
                    .when(me.limit.is_some(), |row| {
                        row.child(div().w(px(220.)).flex_none().child(parts::bar(
                            8.,
                            me.used(),
                            0.,
                            None,
                            color,
                            palette,
                        )))
                    })
                    .into_any_element(),
            )],
            palette,
        )
        .child(
            ui::footnote("Only owners, and admins an owner allows, change limits.", palette)
                .px(px(16.))
                .pb(px(12.)),
        )
    }

    /// The organization's rules as settings rows: the budget, the default per person, and the team caps.
    fn render_rules(&self, budgets: &Budgets, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (budget, spent, held, remaining) = match &self.spend {
            Load::Ready(spend) => (spend.budget.unwrap_or(0.), spend.spent, spend.reserved, spend.remaining),
            _ => (0., 0., 0., 0.),
        };

        // The organization's budget.
        let budget_detail = match &self.spend {
            Load::Ready(_) if budget > 0. => "Every call stops when the month's spend reaches it.".to_string(),
            Load::Ready(_) => "No budget: calls stop only at a team's cap or a person's limit.".to_string(),
            Load::Loading => "Reading the organization's spend…".to_string(),
            Load::Failed(error) => format!("The spend did not load: {error}"),
        };
        let mut budget_row = rule_row("usage-limits-org-card", ORG_BUDGET, budget_detail, palette);
        if self.editing == Some(Edit::Budget) {
            budget_row = budget_row.child(self.amount_editor(Vec::new(), palette, cx));
        } else {
            // The bar, the value and Edit move together under the text in a narrow column.
            let mut tail = h_flex().flex_none().max_w_full().gap(px(16.));
            if budget > 0. {
                tail = tail.child(
                    v_flex()
                        .w(px(200.))
                        .flex_shrink(1.)
                        .min_w(px(0.))
                        .gap(px(6.))
                        .debug_selector(|| "usage-limits-org-meter".into())
                        .child(parts::bar(
                            8.,
                            share(spent, budget),
                            share(held, budget),
                            None,
                            if remaining <= 0. {
                                palette.destructive
                            } else {
                                palette.strong
                            },
                            palette,
                        ))
                        .child(
                            h_flex()
                                .justify_between()
                                .text_size(px(11.))
                                .text_color(palette.faint)
                                .child(format!("{} spent", cents(spent)))
                                .child(format!("{} left", cents(remaining.max(0.)))),
                        ),
                );
            }
            tail = tail.child(
                value_cell(
                    if budget > 0. {
                        budget_text(budget)
                    } else {
                        "None".into()
                    },
                    if budget > 0. { "a month" } else { "" },
                    palette,
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .debug_selector(|| "usage-limits-resets".into())
                        .child(format!("Resets {}", short_date(&budgets.resets_at))),
                ),
            );
            if self.edit_allowed(&Edit::Budget) && !self.mutating() {
                tail = tail.child(rule_edit("usage-edit-budget", true, palette).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.start_edit(Edit::Budget, (budget > 0.).then_some(budget), window, cx)
                    },
                )));
            }
            if let Load::Failed(_) = &self.spend {
                tail = tail.child(
                    ui::button("usage-spend-retry", "Retry", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load_money(cx))),
                );
            }
            budget_row = budget_row.child(tail);
        }

        // The default per person.
        let (_, _, own, by_default) = budgets.counts();
        let person_default = budgets.person_default;
        let default_detail = match person_default {
            None => format!(
                "No default: only people with their own limit have one. {own} {}.",
                if own == 1 { "does" } else { "do" }
            ),
            Some(_) => format!(
                "Applies to {by_default} of {} people.{}",
                budgets.people.len(),
                match own {
                    0 => String::new(),
                    1 => " 1 has their own limit.".into(),
                    n => format!(" {n} have their own limit."),
                }
            ),
        };
        let mut default_row = rule_row("usage-limits-default-card", DEFAULT_LIMIT, default_detail, palette);
        if self.editing == Some(Edit::Default) {
            let mut extra = Vec::new();
            if person_default.is_some() {
                extra.push(
                    ui::danger_button(
                        "usage-default-clear",
                        if self.confirm_clear {
                            "Remove it for everyone?"
                        } else {
                            "No default"
                        },
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.confirm_clear {
                            this.clear_limit(cx);
                        } else {
                            this.confirm_clear = true;
                            cx.notify();
                        }
                    }))
                    .into_any_element(),
                );
            }
            default_row = default_row.child(self.amount_editor(extra, palette, cx));
        } else {
            let mut tail = h_flex().flex_none().gap(px(16.)).child(value_cell(
                person_default.map(budget_text).unwrap_or_else(|| "None".into()),
                if person_default.is_some() { "a month each" } else { "" },
                palette,
            ));
            if self.edit_allowed(&Edit::Default) && !self.mutating() {
                tail = tail.child(rule_edit("usage-edit-default", true, palette).on_click(
                    cx.listener(move |this, _, window, cx| this.start_edit(Edit::Default, person_default, window, cx)),
                ));
            }
            default_row = default_row.child(tail);
        }

        // The team caps, read only here: a cap is set on its team's page.
        let caps_row = self.can("budgets.manage").then(|| {
            let detail = match &self.caps {
                Load::Loading => "Reading the teams' caps…".to_string(),
                Load::Failed(error) => format!("The caps did not load: {error}"),
                Load::Ready(reads) => {
                    let (caps, failed) = crate::pages::usage_caps::shown(reads, chrono::Utc::now().timestamp_millis());
                    let mut text = crate::pages::usage_caps::summary(&caps);
                    if let Some(note) = crate::pages::usage_caps::failed_text(&failed) {
                        text = format!("{text} {note}");
                    }
                    text
                }
            };
            rule_row("usage-limits-caps-card", TEAM_CAPS, detail, palette)
                .child(teams_button(true, palette).on_click(
                    cx.listener(|this, _, window, cx| (this.ctx.navigate)(crate::pages::PageId::Teams, window, cx)),
                ))
                .into_any_element()
        });

        // A setup line's Raise limit arrives on the row of the limit that stopped the call (`limit_stop.rs`).
        let fields = self.ctx.settings_fields.clone();
        let caps_row = caps_row.map(|row| fields.field(limit_stop::TEAM_ROW, row, None, cx).into_any_element());
        ui::group(
            [
                Some(
                    fields
                        .field(limit_stop::ORG_ROW, budget_row, None, cx)
                        .into_any_element(),
                ),
                Some(default_row.into_any_element()),
                caps_row,
            ],
            palette,
        )
    }

    pub(super) fn render_people(&self, budgets: &Budgets, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (at, near, own, _) = budgets.counts();
        let words = self.people_query.read(cx).value().trim().to_lowercase();
        let mut rows: Vec<&Person> = budgets
            .people
            .iter()
            .filter(|p| self.limit_filter.keeps(p))
            .filter(|p| {
                words.is_empty()
                    || [p.name.as_deref(), p.email.as_deref(), Some(p.user_id.as_str())]
                        .into_iter()
                        .flatten()
                        .any(|text| text.to_lowercase().contains(&words))
            })
            .collect();
        rows.sort_by(|a, b| b.spent.total_cmp(&a.spent).then(a.display().cmp(&b.display())));
        let total = rows.len();
        if !self.more && words.is_empty() {
            rows.truncate(7);
        }
        let filters = [
            (LimitFilter::All, format!("All {}", budgets.people.len())),
            (LimitFilter::Near, format!("Near limit {near}")),
            (LimitFilter::At, format!("At limit {at}")),
            (LimitFilter::Own, format!("Own limit {own}")),
        ];
        let mut list = v_flex().child(
            parts::section("People", "", palette)
                .flex_wrap()
                .child(
                    ui::tabs(palette).children(filters.into_iter().enumerate().map(|(ix, (filter, label))| {
                        ui::tab(("usage-limit-filter", ix), label, self.limit_filter == filter, palette).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.limit_filter = filter;
                                this.more = false;
                                cx.notify();
                            }),
                        )
                    })),
                )
                .child(
                    h_flex()
                        .debug_selector(|| "usage-limits-search".into())
                        .flex_1()
                        .min_w(px(96.))
                        .max_w(px(200.))
                        .h(px(ui::CONTROL_HEIGHT))
                        .px(px(10.))
                        .rounded(px(8.))
                        .bg(palette.hover)
                        .text_size(px(13.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(Input::new(&self.people_query).appearance(false)),
                        ),
                ),
        );
        // The table keeps its columns and scrolls sideways in a narrow column, so a name never collapses.
        let mut table = v_flex().w_full().min_w(px(TABLE_MIN_WIDTH)).child(
            ui::head_row(palette)
                .child(div().flex_1().child("Person"))
                .child(cell(260., "This month", palette.faint).text_size(px(11.)))
                .child(right(150., "Limit", palette.faint).text_size(px(11.)))
                .child(div().w(px(60.))),
        );
        for (ix, person) in rows.iter().enumerate() {
            let editing = self.editing == Some(Edit::Person(person.user_id.clone()));
            let name = h_flex()
                .flex_1()
                .min_w(px(NAME_MIN_WIDTH))
                .debug_selector(move || format!("usage-limits-name-{ix}"))
                .gap(px(10.))
                .child(parts::avatar(&person.display(), palette))
                .child(
                    ui::two_lines(
                        person.display(),
                        if person.name.is_some() {
                            person.email.clone().unwrap_or_default()
                        } else {
                            String::new()
                        },
                        palette,
                    )
                    .flex_1(),
                );
            let row = ui::row(palette)
                .min_h(px(48.))
                .mb(px(0.))
                .rounded(px(0.))
                .border_b_1()
                .border_color(palette.border)
                .id(("usage-limit-row", ix))
                .child(name);
            if editing {
                let mut extra = Vec::new();
                if person.source == "override" && budgets.person_default.is_some() {
                    extra.push(
                        ui::button("usage-use-default", "Use default", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.clear_limit(cx)))
                            .into_any_element(),
                    );
                }
                table = table.child(row.child(self.amount_editor(extra, palette, cx)));
                continue;
            }
            let state = person.state();
            let user = person.user_id.clone();
            let limit = person.limit;
            let color = match state {
                LimitState::At => palette.destructive,
                LimitState::Near => palette.warning,
                _ => palette.muted,
            };
            let used = match (state, limit) {
                (LimitState::At, _) => "at limit".to_string(),
                (_, Some(_)) => percent(person.used()),
                (_, None) => "no limit".into(),
            };
            table = table.child(
                row.child(
                    h_flex()
                        .w(px(260.))
                        .flex_none()
                        .gap(px(10.))
                        .child(div().w(px(120.)).flex_none().when(limit.is_some(), |c| {
                            c.child(parts::bar(6., person.used(), 0., None, color, palette))
                        }))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(palette.foreground)
                                .child(cents(person.spent)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(format!("· {used}")),
                        ),
                )
                .child(
                    h_flex()
                        .w(px(150.))
                        .flex_none()
                        .justify_end()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(if person.source == "override" {
                                    palette.foreground
                                } else {
                                    palette.muted
                                })
                                .child(limit.map(budget_text).unwrap_or_else(|| "–".into())),
                        )
                        .when(limit.is_some(), |c| {
                            c.child(ui::pill(
                                if person.source == "override" { "own" } else { "default" },
                                palette,
                            ))
                        }),
                )
                .child(h_flex().w(px(60.)).flex_none().justify_end().when(
                    budgets.can_edit(person) && !self.mutating(),
                    |c| {
                        c.child(
                            ui::ghost_button(("usage-limit-edit", ix), "Edit", palette).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.start_edit(Edit::Person(user.clone()), limit, window, cx)
                                },
                            )),
                        )
                    },
                )),
            );
        }
        list = list.child(
            div()
                .id("usage-limits-table")
                .debug_selector(|| "usage-limits-table".into())
                .w_full()
                .mx(px(-ui::ROW_BLEED))
                .px(px(ui::ROW_BLEED))
                .overflow_x_scroll()
                .child(table),
        );
        if rows.is_empty() {
            list = list.child(
                ui::note(
                    if !words.is_empty() {
                        "No one matches."
                    } else {
                        match self.limit_filter {
                            LimitFilter::At => "No one is at their limit.",
                            LimitFilter::Near => "No one is near their limit.",
                            LimitFilter::Own => "No one has a limit of their own.",
                            LimitFilter::All => "No one is in the organization yet.",
                        }
                    },
                    palette,
                )
                .py(px(12.)),
            );
        }
        if !self.more && words.is_empty() && total > 7 {
            list = list.child(
                ui::ghost_button("usage-limit-more", format!("Show {} more", total - 7), palette)
                    .mt(px(8.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more = true;
                        cx.notify();
                    })),
            );
        }
        if !budgets.can_manage {
            list = list.child(ui::note("Only owners, and admins an owner allows, change limits.", palette).mt(px(8.)));
        }
        // Each person's limit: where a person's Raise limit arrives.
        self.ctx.settings_fields.field(limit_stop::PERSON_ROW, list, None, cx)
    }

    /// Who may change limits: owners always; each admin when an owner turns it on.
    fn render_managers(&self, budgets: &Budgets, palette: Palette, cx: &mut Context<Self>) -> Div {
        let mut admins: Vec<&Person> = budgets.people.iter().filter(|p| p.role == "admin").collect();
        if admins.is_empty() {
            return v_flex();
        }
        admins.sort_by_key(|p| p.display());
        let mut list = v_flex().child(parts::section(
            "Who can change limits",
            "Owners always can. Turn it on for an admin.",
            palette,
        ));
        for (ix, admin) in admins.into_iter().enumerate() {
            let busy = self.mutating();
            let user = admin.user_id.clone();
            let on = admin.limit_manager;
            list = list.child(
                h_flex()
                    .min_h(px(48.))
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(palette.border)
                    .child(parts::avatar(&admin.display(), palette))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(admin.display()),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child("Admin"),
                    )
                    .child(ui::switch(("usage-manager", ix), on, busy, palette).when(!busy, |s| {
                        s.on_click(cx.listener(move |this, _, _, cx| this.set_manager(user.clone(), !on, cx)))
                    })),
            );
        }
        if let Some(error) = &self.manager_error {
            list = list.child(ui::error_line(error.clone(), palette).mt(px(8.)));
        }
        list
    }
}

const ORG_BUDGET: &str = "Organization budget";
const DEFAULT_LIMIT: &str = "Default per person";
const TEAM_CAPS: &str = "Team caps";

fn rule_edit(id: &'static str, live: bool, palette: Palette) -> Stateful<Div> {
    if live {
        ui::button(id, "Edit", palette)
    } else {
        ui::disabled_button(id, "Edit", palette)
    }
}

fn teams_button(live: bool, palette: Palette) -> Stateful<Div> {
    if live {
        ui::button("usage-open-teams", "Open Teams", palette)
    } else {
        ui::disabled_button("usage-open-teams", "Open Teams", palette)
    }
}

pub(super) fn locked_rules(palette: Palette) -> Div {
    let detail = String::new();
    ui::group(
        [
            Some(
                rule_row("usage-limits-org-card", ORG_BUDGET, detail.clone(), palette)
                    .child(
                        rule_edit("usage-edit-budget", false, palette)
                            .debug_selector(|| "usage-edit-budget-locked".into()),
                    )
                    .into_any_element(),
            ),
            Some(
                rule_row("usage-limits-default-card", DEFAULT_LIMIT, detail.clone(), palette)
                    .child(rule_edit("usage-edit-default", false, palette))
                    .into_any_element(),
            ),
            Some(
                rule_row("usage-limits-caps-card", TEAM_CAPS, detail, palette)
                    .child(teams_button(false, palette))
                    .into_any_element(),
            ),
        ],
        palette,
    )
}

/// A rule's settings row: its name over its explanation, and what follows it at the right. It wraps, so in a narrow
/// column the value and its button move under the text and stay inside the card.
fn rule_row(selector: &'static str, title: &'static str, detail: String, palette: Palette) -> Div {
    ui::card_row()
        .flex_wrap()
        .debug_selector(move || selector.into())
        .child(ui::setting_lines(title, detail, palette).flex_1().min_w(px(200.)))
}

/// A rule's value at the right of its row: "$30,000" over "a month".
fn value_cell(value: String, unit: &'static str, palette: Palette) -> Div {
    v_flex()
        .w(px(110.))
        .flex_none()
        .items_end()
        .child(div().text_size(px(15.)).text_color(palette.strong).child(value))
        .when(!unit.is_empty(), |cell| {
            cell.child(div().text_size(px(12.)).text_color(palette.faint).child(unit))
        })
}
