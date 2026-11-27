use super::*;

impl UsagePage {
    /* -------------------------------------------------------------- breakdown */

    pub(super) fn render_breakdown(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let by = self.by;
        let mut toolbar = h_flex()
            .gap(px(8.))
            .mb(px(10.))
            .child(ui::label("Group by", palette))
            .children([By::Team, By::Person, By::Model, By::Product].into_iter().map(|each| {
                ui::chip(
                    SharedString::from(format!("usage-by-{}", each.slug())),
                    each.label(),
                    by == each,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_by(each, None, cx)))
            }));
        if let Some(under) = &self.under {
            let path: Vec<(String, String)> = match &self.breakdown {
                Load::Ready(Report { under: Some(u), .. }) => {
                    let mut path = u.path.clone();
                    path.push((u.name.clone(), u.slug.clone()));
                    path
                }
                _ => vec![(under.clone(), under.clone())],
            };
            let mut crumbs = h_flex().gap(px(4.)).ml(px(8.)).child(
                div()
                    .id("usage-crumb-all")
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(palette.strong))
                    .child("All teams")
                    .on_click(cx.listener(|this, _, _, cx| this.set_by(By::Team, None, cx))),
            );
            let last = path.len().saturating_sub(1);
            for (n, (name, slug)) in path.into_iter().enumerate() {
                crumbs = crumbs.child(ui::icon(gpui_kit::assets::IconName::ChevronRight, palette.faint));
                crumbs = crumbs.child(if n == last {
                    div()
                        .id(("usage-crumb", n))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(name)
                } else {
                    div()
                        .id(("usage-crumb", n))
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .cursor_pointer()
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_by(By::Team, Some(slug.clone()), cx)))
                });
            }
            if let (Load::Ready(report), Some(Load::Ready(whole))) = (&self.breakdown, &self.whole) {
                let org = if self.ctx.org_name.is_empty() {
                    "the organization"
                } else {
                    self.ctx.org_name.as_str()
                };
                crumbs = crumbs.child(ui::label(
                    format!(
                        "{} · {} of {org}",
                        cents(report.totals.spent),
                        percent(share(report.totals.spent, whole.totals.spent))
                    ),
                    palette,
                ));
            }
            toolbar = toolbar.child(crumbs);
        }
        if by == By::Team {
            toolbar = toolbar.child(div().flex_1()).child(
                ui::button(
                    "usage-everyone",
                    if self.everyone { "Hide people" } else { "Show people" },
                    palette,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.everyone = !this.everyone;
                    if this.everyone {
                        let slugs: Vec<String> = match &this.breakdown {
                            Load::Ready(report) => report
                                .groups
                                .iter()
                                .filter(|g| !g.direct && g.key != "unassigned" && g.key != "others")
                                .filter_map(|g| g.slug.clone())
                                .collect(),
                            _ => Vec::new(),
                        };
                        for slug in slugs {
                            if !this.open_teams.contains_key(&slug) {
                                this.load_team_people(slug, cx);
                            }
                        }
                    } else {
                        this.open_teams.clear();
                    }
                    cx.notify();
                })),
            );
        }
        let mut body = v_flex().child(toolbar);
        if let Some(state) = loading_or_failed(&self.breakdown, 6, palette) {
            return body.child(state);
        }
        let Load::Ready(report) = &self.breakdown else {
            return body;
        };
        let noun = match by {
            By::Team => "team",
            By::Person => "person",
            By::Model => "model",
            By::Product => "product",
        };
        if report.groups.is_empty() {
            return body.child(ui::empty(
                format!("No spend in these {} days", self.days()),
                format!("Calls through Nexus Gateway show here by {noun} once there are some."),
                palette,
            ));
        }
        let mut rows: Vec<&Group> = report.groups.iter().collect();
        rows.sort_by(|a, b| {
            a.direct
                .cmp(&b.direct)
                .then(b.figures.spent.total_cmp(&a.figures.spent))
        });
        let share_of = report.under.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| {
            if self.ctx.org_name.is_empty() {
                "all".into()
            } else {
                self.ctx.org_name.clone()
            }
        });
        body = body.child(
            ui::head_row(palette)
                .child(div().w(px(16.)))
                .child(div().flex_1().child(by.label()))
                .child(cell(150., format!("Share of {share_of}"), palette.faint).text_size(px(11.)))
                .child(cell(84., format!("{} days", self.days()), palette.faint).text_size(px(11.)))
                .child(right(70., "Calls", palette.faint).text_size(px(11.)))
                .child(right(110., "Tokens in / out", palette.faint).text_size(px(11.)))
                .child(right(60., "Errors", palette.faint).text_size(px(11.)))
                .child(right(NUM_W, "Spent", palette.faint).text_size(px(11.)))
                .child(div().w(px(16.))),
        );
        for (ix, group) in rows.into_iter().enumerate() {
            let expandable = by == By::Team
                && !group.direct
                && group.key != "unassigned"
                && group.key != "others"
                && group.slug.is_some();
            let open = expandable
                && group
                    .slug
                    .as_ref()
                    .map(|s| self.open_teams.contains_key(s))
                    .unwrap_or(false);
            let name = if group.direct {
                format!(
                    "{} itself",
                    report
                        .under
                        .as_ref()
                        .map(|u| u.name.clone())
                        .unwrap_or_else(|| group.label.clone())
                )
            } else if group.key == "unassigned" {
                if by == By::Team {
                    "No team".into()
                } else {
                    "No one".into()
                }
            } else {
                group.label.clone()
            };
            let sub = if group.direct {
                format!(
                    "Calls tagged to {}, not a team under it",
                    report
                        .under
                        .as_ref()
                        .and_then(|u| u.level.clone())
                        .unwrap_or_else(|| "the team".into())
                )
            } else {
                [
                    group.level.clone(),
                    group.deleted.then(|| "deleted".to_string()),
                    group.slug.clone().filter(|_| by == By::Team),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ")
            };
            let filter = calls_filter(by, group);
            body = body.child(self.breakdown_row(
                ("usage-break", ix),
                name,
                sub,
                group,
                report,
                expandable.then(|| (open, group.slug.clone().unwrap_or_default())),
                group.has_children.then(|| group.slug.clone()).flatten(),
                filter,
                palette,
                cx,
            ));
            if open {
                if let Some(slug) = &group.slug {
                    body = body.child(self.render_team_people(slug, palette, cx));
                }
            }
        }
        let total = report
            .under
            .as_ref()
            .map(|u| format!("{} total", u.name))
            .unwrap_or_else(|| "Total".into());
        body.child(
            h_flex()
                .min_h(px(40.))
                .gap(px(12.))
                .px(px(8.))
                .text_size(px(13.))
                .child(div().w(px(16.)))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(total),
                )
                .child(div().w(px(150.)))
                .child(div().w(px(84.)))
                .child(right(70., ui::count(report.totals.calls), palette.strong))
                .child(right(
                    110.,
                    format!(
                        "{} / {}",
                        compact(report.totals.input_tokens),
                        compact(report.totals.output_tokens)
                    ),
                    palette.strong,
                ))
                .child(right(60., error_text(report.totals.error_rate), palette.strong))
                .child(right(NUM_W, cents(report.totals.spent), palette.strong))
                .child(div().w(px(16.))),
        )
        .when(by == By::Team, |body| {
            let path = match &report.under {
                Some(u) => {
                    let mut slugs: Vec<String> = u.path.iter().map(|(_, s)| s.clone()).collect();
                    slugs.push(u.slug.clone());
                    format!(" --path {}", slugs.join("/"))
                }
                None => String::new(),
            };
            body.child(
                ui::note(
                    format!("Same view in the terminal: nexus analytics --by team{path}"),
                    palette,
                )
                .mt(px(12.)),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn breakdown_row(
        &self,
        id: (&'static str, usize),
        name: String,
        sub: String,
        group: &Group,
        report: &Report,
        expand: Option<(bool, String)>,
        drill: Option<String>,
        filter: Option<CallFilters>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let spark: Vec<f64> = report
            .series
            .iter()
            .map(|day| *day.groups.get(&group.key).unwrap_or(&0.))
            .collect();
        let expander: AnyElement = match expand {
            Some((open, slug)) => div()
                .id(SharedString::from(format!("{}-{}-x", id.0, id.1)))
                .w(px(16.))
                .cursor_pointer()
                .child(ui::icon(
                    if open {
                        gpui_kit::assets::IconName::ChevronDown
                    } else {
                        gpui_kit::assets::IconName::ChevronRight
                    },
                    palette.icon,
                ))
                .when(!self.everyone, |x| {
                    x.on_click(cx.listener(move |this, _, _, cx| this.toggle_team(slug.clone(), cx)))
                })
                .into_any_element(),
            None => div().w(px(16.)).into_any_element(),
        };
        let title = div()
            .id(SharedString::from(format!("{}-{}-t", id.0, id.1)))
            .text_size(px(13.))
            .text_color(palette.strong)
            .truncate()
            .child(name)
            .when_some(drill, |title, slug| {
                title
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .tooltip(|window, cx| Tooltip::new("See the teams in it").build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_by(By::Team, Some(slug.clone()), cx)))
            });
        ui::row(palette)
            .id(id)
            .child(expander)
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(title)
                    .when(!sub.is_empty(), |c| {
                        c.child(div().text_size(px(12.)).text_color(palette.faint).truncate().child(sub))
                    }),
            )
            .child(
                h_flex()
                    .w(px(150.))
                    .gap(px(8.))
                    .child(meter(90., group.share, 0., palette.muted, palette))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(percent(group.share)),
                    ),
            )
            .child(sparkline(&spark, 84., palette))
            .child(right(70., ui::count(group.figures.calls), palette.foreground))
            .child(right(
                110.,
                format!(
                    "{} / {}",
                    compact(group.figures.input_tokens),
                    compact(group.figures.output_tokens)
                ),
                palette.faint,
            ))
            .child(right(60., error_text(group.figures.error_rate), palette.faint))
            .child(right(NUM_W, cents(group.figures.spent), palette.strong))
            .child(match filter {
                Some(filter) => div()
                    .id(SharedString::from(format!("{}-{}-calls", id.0, id.1)))
                    .w(px(16.))
                    .cursor_pointer()
                    .tooltip(|window, cx| Tooltip::new("Its calls").build(window, cx))
                    .child(ui::icon(gpui_kit::assets::IconName::ArrowRight, palette.icon))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let filter = filter.clone();
                        this.tab = Tab::Calls;
                        this.set_filters(|f| *f = filter, cx);
                    }))
                    .into_any_element(),
                None => div().w(px(16.)).into_any_element(),
            })
    }

    pub(super) fn render_team_people(&self, slug: &str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let indent = |content: AnyElement| h_flex().pl(px(36.)).py(px(8.)).child(content);
        match self.open_teams.get(slug) {
            Some(Load::Ready(report)) => {
                let mut rows: Vec<&Group> = report.groups.iter().collect();
                if rows.is_empty() {
                    return indent(ui::note("No one in it spent anything in these days.", palette).into_any_element());
                }
                rows.sort_by(|a, b| b.figures.spent.total_cmp(&a.figures.spent));
                let mut list = v_flex().pl(px(20.)).bg(palette.hover.opacity(0.4));
                for (ix, person) in rows.into_iter().enumerate() {
                    let name = if person.key == "unassigned" {
                        "No one".to_string()
                    } else {
                        person.label.clone()
                    };
                    let filter = (person.key != "unassigned" && person.key != "others").then(|| CallFilters {
                        person: Some(person.key.clone()),
                        team: Some(slug.to_string()),
                        ..Default::default()
                    });
                    list = list.child(self.breakdown_row(
                        ("usage-person", ix + slug.len() * 1000),
                        name,
                        String::new(),
                        person,
                        report,
                        None,
                        None,
                        filter,
                        palette,
                        cx,
                    ));
                }
                list
            }
            Some(Load::Failed(_)) => indent(ui::note("Their people could not be loaded.", palette).into_any_element()),
            _ => indent(ui::note("Loading people…", palette).into_any_element()),
        }
    }

    /* -------------------------------------------------------------- adoption */

    pub(super) fn render_adoption(&self, palette: Palette) -> Div {
        let mut body = v_flex().child(card_head(
            "Engineer adoption",
            match &self.adoption {
                Load::Ready(a) if a.member => "UTC days · counts, not people · only you",
                _ => "UTC days · counts, not people · the whole organization",
            },
            palette,
        ));
        if let Some(state) = loading_or_failed(&self.adoption, 5, palette) {
            return body.child(state);
        }
        let Load::Ready(view) = &self.adoption else { return body };
        if view.member {
            body = body.child(
                ui::note(
                    "You see your own adoption. Owners and admins see the whole organization's.",
                    palette,
                )
                .mb(px(8.)),
            );
        }
        if !view.supported {
            return body.child(ui::empty(
                "This Gateway does not count active engineers yet",
                "Update the Gateway to a newer release to see this count.",
                palette,
            ));
        }
        if view.active_days == 0 {
            return body.child(ui::empty(
                "No active engineers in this range",
                format!(
                    "An active engineer is a person with at least one non-refused model call that day, in any product. Nobody made one on these {} days.",
                    view.rows.len()
                ),
                palette,
            ));
        }
        let col = 110.;
        let dash = |value: Option<u64>| value.map(ui::count).unwrap_or_else(|| "–".into());
        body = body.child(
            ui::head_row(palette)
                .child(div().w(px(80.)).child("Day"))
                .child(right(col, "Active engineers", palette.faint).text_size(px(11.)))
                .children(
                    view.products
                        .iter()
                        .map(|(_, label)| right(col, label.clone(), palette.faint).text_size(px(11.))),
                )
                .child(right(150., "Through Nexus Gateway", palette.faint).text_size(px(11.)))
                .child(right(150., "On their own subscription", palette.faint).text_size(px(11.))),
        );
        for row in &view.rows {
            body = body.child(
                h_flex()
                    .min_h(px(36.))
                    .gap(px(12.))
                    .px(px(8.))
                    .child(cell(80., short_date(&row.day), palette.muted))
                    .child(right(col, ui::count(row.engineers), palette.strong))
                    .children(
                        view.products
                            .iter()
                            .map(|(slug, _)| right(col, dash(row.by_product.get(slug).copied()), palette.foreground)),
                    )
                    .child(right(150., ui::count(row.gateway as u64), palette.foreground))
                    .child(right(150., ui::count(row.subscription as u64), palette.foreground)),
            );
        }
        body.child(
            ui::note(
                format!(
                    "{} of {} days with an active engineer. An active engineer is a person with at least one non-refused model call that day. A person counts once per product, so the products can add up to more than the day's total. A dash: no one was active in that product that day.",
                    view.active_days,
                    view.rows.len()
                ),
                palette,
            )
            .mt(px(12.)),
        )
    }
}
