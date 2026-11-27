use super::*;

impl UsagePage {
    /* -------------------------------------------------------------- calls */

    /// The calls, and one call's detail beside them while one is open.
    pub(super) fn render_calls(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let open = self.selected.is_some() && self.can("spend.read_all");
        let list = self.render_call_list(open, palette, cx);
        if !open {
            return list;
        }
        h_flex()
            .items_start()
            .gap(px(16.))
            .child(list.flex_1().min_w(px(0.)))
            .child(self.render_call(palette, cx).w(px(360.)).flex_none())
    }

    /// The filters and the calls; `narrow` drops the Source, Model and Tokens columns to make room for a call's detail.
    fn render_call_list(&self, narrow: bool, palette: Palette, cx: &mut Context<Self>) -> Div {
        if let Load::Ready(me) = &self.me {
            if !me.can("spend.read_all") {
                return v_flex().child(ui::empty(
                    "Calls are listed for owners and admins",
                    "Nexus Gateway shows the organization's calls to owners and admins. Your own spend, by day and by model, is on Overview.",
                    palette,
                ))
                .child(
                    h_flex().justify_center().child(
                        ui::button("usage-calls-overview", "See your spend by day", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Overview, cx))),
                    ),
                );
            }
        }
        let f = &self.filters;
        let status_label = f.status.as_ref().map(|s| {
            STATUSES
                .iter()
                .find(|(id, _)| id == s)
                .map(|(_, l)| l.to_string())
                .unwrap_or_else(|| s.clone())
        });
        let person_label = f
            .person
            .as_ref()
            .map(|p| self.names.get(p).cloned().unwrap_or_else(|| p.clone()));
        let chips = h_flex()
            .flex_wrap()
            .gap(px(6.))
            .child(self.filter_chip(Menu::Day, "Day", f.day.as_deref().map(short_date), palette, cx))
            .child(self.filter_chip(
                Menu::Model,
                "Model",
                f.model.as_deref().map(|m| short_model(m).to_string()),
                palette,
                cx,
            ))
            .child(self.filter_chip(Menu::Person, "Person", person_label, palette, cx))
            .child(self.filter_chip(Menu::Status, "Status", status_label, palette, cx))
            .child(self.filter_chip(Menu::Product, "Product", f.product.clone(), palette, cx))
            .when_some(f.team.clone(), |chips, team| {
                chips.child(
                    ui::chip(
                        "usage-chip-team",
                        format!("Team: {} ×", if team == "unassigned" { "No team".into() } else { team }),
                        true,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_filters(|f| f.team = None, cx))),
                )
            })
            .when_some(f.session.clone(), |chips, session| {
                chips.child(
                    ui::chip(
                        "usage-chip-session",
                        format!("Session: {} ×", session.get(..10).unwrap_or(&session)),
                        true,
                        palette,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_filters(|f| f.session = None, cx))),
                )
            });
        let mut body = v_flex().gap(px(8.)).child(chips);
        if let Some(menu) = self.menu {
            body = body.child(self.render_menu(menu, palette, cx));
        }
        if let Some(state) = loading_or_failed(&self.calls, 10, palette) {
            if let Load::Failed(error) = &self.calls {
                if is_refusal(error) {
                    // Gateway refused the list: the person's own spend is a tab away.
                    return body.child(
                        ui::empty(
                            "Calls are listed for owners and admins",
                            "Owners and admins see every call.",
                            palette,
                        )
                        .child(
                            crate::setup::quiet::link("usage-calls-your-spend", "Your spend", palette)
                                .text_size(px(13.))
                                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Overview, cx))),
                        ),
                    );
                }
            }
            return body.child(state);
        }
        let Load::Ready(page) = &self.calls else { return body };
        if page.calls.is_empty() {
            return body
                .child(ui::empty(
                    "No calls match",
                    "Nothing went through Nexus Gateway with these filters in these days.",
                    palette,
                ))
                .child(
                    h_flex().justify_center().child(
                        ui::button("usage-calls-clear", "Clear the filters", palette).on_click(
                            cx.listener(|this, _, _, cx| this.set_filters(|f| *f = CallFilters::default(), cx)),
                        ),
                    ),
                );
        }
        let wide = !narrow;
        // The open call's panel names its model, so a narrow list leaves the column out.
        let model_col = self.filters.model.is_none() && wide;
        let single_day = self.filters.day.is_some();
        let mut table = v_flex().child(
            ui::head_row(palette)
                .child(cell(110., "Time", palette.faint).text_size(px(11.)))
                .child(div().flex_1().child("Person"))
                .when(wide, |r| {
                    r.child(cell(100., "Source", palette.faint).text_size(px(11.)))
                })
                .when(model_col, |r| {
                    r.child(cell(150., "Model", palette.faint).text_size(px(11.)))
                })
                .child(cell(150., "Status", palette.faint).text_size(px(11.)))
                .when(wide, |r| {
                    r.child(right(130., "Tokens in / out", palette.faint).text_size(px(11.)))
                })
                .child(right(80., "Cost", palette.faint).text_size(px(11.))),
        );
        for (ix, call) in page.calls.iter().enumerate() {
            let time = call
                .at
                .get(11..if single_day { 19 } else { 16 })
                .unwrap_or("")
                .to_string();
            let when = if single_day {
                time
            } else {
                format!("{}, {time}", short_date(&call.at))
            };
            let tokens = format!(
                "{} / {}",
                call.input_tokens.map(ui::count).unwrap_or_else(|| "–".into()),
                call.output_tokens.map(ui::count).unwrap_or_else(|| "–".into())
            );
            let cost = if call.status == "settled" || call.cost.parse::<f64>().unwrap_or(0.) > 0. {
                usd_decimal(&call.cost)
            } else {
                "–".into()
            };
            let id = call.id.clone();
            let picked = self.selected.as_deref() == Some(call.id.as_str());
            table = table.child(
                ui::row(palette)
                    .min_h(px(40.))
                    .id(("usage-call", ix))
                    .cursor_pointer()
                    .when(picked, |row| row.bg(palette.selected))
                    .hover(|s| s.bg(palette.hover))
                    .child(cell(110., when, palette.faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(
                                call.user_id
                                    .as_ref()
                                    .map(|u| self.names.get(u).cloned().unwrap_or_else(|| u.clone()))
                                    .unwrap_or_else(|| "–".into()),
                            ),
                    )
                    .when(wide, |r| r.child(cell(100., call.source(&self.agents), palette.muted)))
                    .when(model_col, |r| {
                        r.child(cell(
                            150.,
                            call.model.as_deref().map(short_model).unwrap_or("–").to_string(),
                            palette.foreground,
                        ))
                    })
                    .child(
                        h_flex()
                            .w(px(150.))
                            .flex_none()
                            .gap(px(6.))
                            .child(parts::dot(parts::call_color(&call.status, palette)))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(palette.muted)
                                    .child(call.state()),
                            ),
                    )
                    .when(wide, |r| r.child(right(130., tokens, palette.faint)))
                    .child(right(80., cost, palette.strong))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_call(id.clone(), cx))),
            );
        }
        let start = self.cursors.len() * 50 + 1;
        let next = page.next_cursor.clone();
        table = table.child(
            h_flex()
                .py(px(10.))
                .gap(px(8.))
                .child(ui::label(
                    format!(
                        "{} to {}",
                        ui::count(start as u64),
                        ui::count((start + page.calls.len() - 1) as u64)
                    ),
                    palette,
                ))
                .child(div().flex_1())
                .child(if self.cursors.is_empty() {
                    ui::disabled_button("usage-newer", "Newer", palette)
                } else {
                    ui::button("usage-newer", "Newer", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.page_calls(None, cx)))
                })
                .child(match next {
                    Some(cursor) => ui::button("usage-older", "Older", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.page_calls(Some(cursor.clone()), cx))),
                    None => ui::disabled_button("usage-older", "Older", palette),
                }),
        );
        body.child(table)
    }

    pub(super) fn filter_chip(
        &self,
        menu: Menu,
        label: &'static str,
        value: Option<String>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = SharedString::from(format!("usage-chip-{label}"));
        match value {
            Some(value) => ui::chip(id, format!("{label}: {value} ×"), true, palette).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.set_filters(
                        |f| match menu {
                            Menu::Day => f.day = None,
                            Menu::Model => f.model = None,
                            Menu::Person => f.person = None,
                            Menu::Status => f.status = None,
                            Menu::Product => f.product = None,
                        },
                        cx,
                    )
                },
            )),
            None => ui::chip(id, format!("{label} ▾"), self.menu == Some(menu), palette).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = if this.menu == Some(menu) { None } else { Some(menu) };
                    cx.notify();
                },
            )),
        }
    }

    pub(super) fn render_menu(&self, menu: Menu, palette: Palette, cx: &mut Context<Self>) -> Div {
        let options: Vec<(String, String)> = match menu {
            Menu::Day => (0..7)
                .map(|n| {
                    let day = iso_day(today() - n);
                    let label = match n {
                        0 => "Today".to_string(),
                        1 => "Yesterday".to_string(),
                        _ => short_date(&day),
                    };
                    (day, label)
                })
                .collect(),
            Menu::Model => self.model_options.clone(),
            Menu::Person => {
                let mut people: Vec<(String, String)> =
                    self.names.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                people.sort_by(|a, b| a.1.cmp(&b.1));
                people
            }
            Menu::Status => STATUSES
                .iter()
                .map(|(id, label)| (id.to_string(), label.to_string()))
                .collect(),
            Menu::Product => self.product_options.clone(),
        };
        let mut list = h_flex()
            .flex_wrap()
            .gap(px(6.))
            .p(px(8.))
            .rounded(px(8.))
            .bg(palette.raised);
        if options.is_empty() {
            list = list.child(ui::note("Nothing to pick in these days.", palette));
        }
        for (ix, (id, label)) in options.into_iter().take(40).enumerate() {
            list = list.child(
                ui::chip(("usage-option", ix), label, false, palette).on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.set_filters(
                        |f| match menu {
                            Menu::Day => f.day = Some(id),
                            Menu::Model => f.model = Some(id),
                            Menu::Person => f.person = Some(id),
                            Menu::Status => f.status = Some(id),
                            Menu::Product => f.product = Some(id),
                        },
                        cx,
                    )
                })),
            );
        }
        list
    }

    /// One call's detail, in the panel beside the list.
    pub(super) fn render_call(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let id = self.selected.clone().unwrap_or_default();
        let copy_id = id.clone();
        let head = h_flex()
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child("Call"),
            )
            .child(
                ui::ghost_button(
                    "usage-call-copy",
                    if self.copied { "Copied" } else { "Copy id" },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_id.clone()));
                    this.copied = true;
                    cx.notify();
                })),
            )
            .child(
                ui::ghost_button("usage-call-close", "✕", palette)
                    .tooltip(|window, cx| Tooltip::new("Close").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = None;
                        this.detail = None;
                        cx.notify();
                    })),
            );
        let mut body = ui::card_frame(palette).p(px(16.)).gap(px(8.)).child(head).child(
            div()
                .truncate()
                .font_family(ui::MONO)
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(id.clone()),
        );
        let call = match &self.detail {
            Some(Load::Ready(call)) => call.clone(),
            Some(Load::Failed(error)) => return body.child(ui::failed(error.clone(), palette)),
            _ => return body.child(ui::skeleton(4, palette)),
        };
        let time = call.at.get(11..19).unwrap_or("");
        body = body.child(
            h_flex()
                .gap(px(8.))
                .child(parts::dot(parts::call_color(&call.status, palette)))
                .child(div().text_size(px(13.)).text_color(palette.foreground).child(format!(
                    "{} · {} · {time} UTC",
                    call.state(),
                    long_date(&call.at)
                ))),
        );
        let fact = |label: &str, text: String| {
            h_flex()
                .gap(px(12.))
                .py(px(6.))
                .child(cell(110., label.to_string(), palette.faint))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child(text),
                )
        };
        let mut facts = v_flex()
            .child(fact(
                "Person",
                call.user_id
                    .as_ref()
                    .map(|u| self.names.get(u).cloned().unwrap_or_else(|| u.clone()))
                    .unwrap_or_else(|| "No one".into()),
            ))
            .child(fact(
                "Team",
                call.team_id
                    .as_ref()
                    .map(|t| self.team_names.get(t).cloned().unwrap_or_else(|| t.clone()))
                    .unwrap_or_else(|| "No team".into()),
            ))
            .child(fact("Source", call.source(&self.agents)))
            .child(fact(
                "Product",
                match &call.capability {
                    Some(capability) => format!("{} › {capability}", call.product),
                    None => call.product.clone(),
                },
            ))
            .child(fact(
                "Model",
                call.model.clone().unwrap_or_else(|| "Not recorded".into()),
            ))
            .child(fact(
                "Provider",
                call.provider.clone().unwrap_or_else(|| "Not recorded".into()),
            ))
            .child(fact(
                "Paid by",
                match call.funded_by.as_deref() {
                    None => "The organization".into(),
                    Some("nexus_subsidy") => "Nexus credit".into(),
                    Some(other) => other.to_string(),
                },
            ));
        if let Some(session) = &call.agent_session_id {
            facts = facts.child(fact("Agent session", session.clone()));
        }
        if let Some(session) = &call.chat_session_id {
            facts = facts.child(fact("Chat session", session.clone()));
        }
        body = body.child(facts);

        let pricing = call.model.as_ref().and_then(|m| self.pricing.get(m)).copied();
        let token_line = |label: &str, tokens: Option<u64>, per_million: Option<f64>| {
            let count = tokens.map(ui::count).unwrap_or_else(|| "–".into());
            let (rate, price) = match (tokens, per_million) {
                (Some(tokens), Some(rate)) => (
                    format!(" × ${}/M", (rate * 100.).round() / 100.),
                    format!("${:.4}", tokens as f64 / 1_000_000. * rate),
                ),
                _ => (String::new(), String::new()),
            };
            h_flex()
                .gap(px(12.))
                .py(px(4.))
                .child(cell(110., label.to_string(), palette.faint))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .child(format!("{count}{rate}")),
                )
                .child(right(100., price, palette.foreground))
        };
        body = body
            .child(ui::section_head("Tokens and cost", palette))
            .child(token_line("In", call.input_tokens, pricing.map(|p| p.0)))
            .child(token_line("Out", call.output_tokens, pricing.map(|p| p.1)))
            .child(
                h_flex()
                    .gap(px(12.))
                    .py(px(4.))
                    .child(cell(110., "Total", palette.strong))
                    .child(div().flex_1())
                    .child(right(100., usd_decimal(&call.cost), palette.strong)),
            )
            .child(
                h_flex()
                    .gap(px(12.))
                    .py(px(4.))
                    .child(cell(110., "Reserved", palette.faint))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child("Held before it ran"),
                    )
                    .child(right(
                        100.,
                        call.reserved
                            .as_deref()
                            .map(usd_decimal)
                            .unwrap_or_else(|| "Not recorded".into()),
                        palette.foreground,
                    )),
            );
        if call.status != "settled" {
            let keys = [
                ("reason", "Reason"),
                ("overspendLevel", "Level"),
                ("overspendLimitUsd", "Limit"),
                ("overspendAmountUsd", "Asked to hold"),
                ("overspendOverUsd", "Over by"),
                ("overspendPeriod", "Period"),
            ];
            let present: Vec<(&str, String)> = keys
                .iter()
                .filter_map(|(key, label)| text_of(&call.raw, key).map(|text| (*label, text)))
                .collect();
            if !present.is_empty() {
                body = body.child(ui::section_head(
                    match call.status.as_str() {
                        "refused" => "The limit that stopped it",
                        "failed" => "Why it failed",
                        _ => "Why it was released",
                    },
                    palette,
                ));
                for (label, text) in present {
                    body = body.child(fact(label, text));
                }
            }
        }
        body = body.child(
            ui::button(
                "usage-all-fields",
                if self.all_fields {
                    "Hide all fields"
                } else {
                    "All fields"
                },
                palette,
            )
            .mt(px(8.))
            .on_click(cx.listener(|this, _, _, cx| {
                this.all_fields = !this.all_fields;
                cx.notify();
            })),
        );
        if self.all_fields {
            if let Some(map) = call.raw.as_object() {
                for (key, value) in map {
                    let text = match value {
                        Value::Null => "unknown".to_string(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    body = body.child(fact(key, text));
                }
            }
        }
        let curl = format!(
            "curl -H \"Authorization: Bearer $NEXUS_API_KEY\" {}/v1/spend/calls/{}",
            self.ctx.gateway.site_url(),
            encode(&call.id)
        );
        let mut foot = h_flex().flex_wrap().gap(px(8.)).mt(px(12.)).child(
            ui::button("usage-call-curl", "Copy as curl", palette)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(curl.clone()))),
        );
        if let Some(chat) = call.chat_session_id.clone() {
            foot = foot.child(ui::web_link("usage-call-chat", "Open chat", palette).on_click(
                cx.listener(move |this, _, _, cx| this.ctx.open_web(&format!("sessions/{}", encode(&chat)), cx)),
            ));
        }
        if let Some(session) = call.agent_session_id.clone() {
            foot = foot.child(
                ui::button("usage-call-session", "Open session's calls", palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let session = session.clone();
                        this.selected = None;
                        this.detail = None;
                        this.set_filters(|f| f.session = Some(session), cx);
                    },
                )),
            );
        }
        body.child(foot)
    }
}
