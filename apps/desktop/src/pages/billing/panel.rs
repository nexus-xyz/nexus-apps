use super::*;

impl BillingPage {
    pub(super) fn render_panel(
        &self,
        billing: &Billing,
        org: &str,
        panel: Panel,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let pricing = &billing.pricing;
        let method_note = billing
            .method
            .as_ref()
            .map(|method| format!("Charged to {}.", method.sentence()));
        let close = ui::button("billing-panel-close", "Close", palette)
            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx)));
        let frame = |title: &'static str| {
            ui::card(palette).mt(px(14.)).gap(px(10.)).child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child(title),
                    )
                    .child(close),
            )
        };
        let busy = self.busy.is_some();
        let cancel = || {
            ui::ghost_button("billing-panel-cancel", "Cancel", palette)
                .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx)))
        };
        let body = match panel {
            Panel::Setup => {
                let option = |id: &'static str, picked: bool, name: &'static str, note: String| {
                    h_flex()
                        .id(id)
                        .gap(px(10.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(if picked { ui::thumb(palette) } else { palette.hover })
                        .cursor_pointer()
                        .child(ui::switch(
                            SharedString::from(format!("{id}-mark")),
                            picked,
                            false,
                            palette,
                        ))
                        .child(ui::two_lines(name, note, palette).flex_1())
                };
                let has_method = billing.method.is_some();
                let ready = self.setup_credits || self.setup_after;
                let offered = billing
                    .topup
                    .as_ref()
                    .map(|topup| (topup.below.clone(), topup.add.clone()))
                    .unwrap_or((TOPUP_DEFAULT.0.into(), TOPUP_DEFAULT.1.into()));
                let limit = billing
                    .after
                    .as_ref()
                    .map(|after| after.limit.clone())
                    .unwrap_or(pricing.default_limit.clone());
                let go = if self.setup_credits {
                    "Continue"
                } else if has_method {
                    "Turn on pay after the fact"
                } else {
                    "Continue to Stripe"
                };
                frame(words::SET_UP_BILLING)
                    .child(ui::note(
                        format!(
                            "Choose one or both. AI that Nexus runs costs the provider's price + {}%.",
                            pricing.markup_bps as f64 / 100.
                        ),
                        palette,
                    ))
                    .child(
                        option(
                            "billing-setup-credits",
                            self.setup_credits,
                            words::CREDITS,
                            format!(
                                "Prepay from {}. Auto top-up adds {} when the balance drops under {}; change or turn it off on the next step.",
                                money(&pricing.min_purchase),
                                money(&offered.1),
                                money(&offered.0)
                            ),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.setup_credits = !this.setup_credits;
                            cx.notify();
                        })),
                    )
                    .child(
                        option(
                            "billing-setup-after",
                            self.setup_after,
                            words::PAY_AFTER_THE_FACT,
                            format!(
                                "Charged every {} of unpaid usage; less carries over. {} {} a month; the owner can raise it.{}",
                                money(&pricing.threshold),
                                words::USAGE_LIMIT,
                                money(&limit),
                                if self.setup_credits && self.setup_after {
                                    " It turns on once the credits are paid."
                                } else {
                                    ""
                                }
                            ),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.setup_after = !this.setup_after;
                            cx.notify();
                        })),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(
                                ui::note(
                                    method_note.clone().unwrap_or_else(|| {
                                        "Your card is entered on Stripe's page in your browser. Nexus never sees it.".into()
                                    }),
                                    palette,
                                )
                                .flex_1(),
                            )
                            .child(if !ready || busy {
                                ui::disabled_button("billing-setup-go", go, palette)
                            } else {
                                ui::primary_button("billing-setup-go", go, palette)
                                    .on_click(cx.listener(|this, _, window, cx| this.continue_setup(window, cx)))
                            }),
                    )
            }
            Panel::Credits(pay_after) => {
                let after_hint = (pay_after && !billing.after.as_ref().is_some_and(|after| after.enabled)).then(|| {
                    format!(
                        "Pay after the fact turns on once this purchase is paid. Then the card is charged every {} of unpaid usage, up to {} a month.",
                        money(&pricing.threshold),
                        money(
                            &billing
                                .after
                                .as_ref()
                                .map(|after| after.limit.clone())
                                .unwrap_or(pricing.default_limit.clone())
                        )
                    )
                });
                let min = number(&pricing.min_purchase);
                let amount = self.amount(cx);
                let problem = purchase_problem(&amount, min);
                let (auto, below, add, auto_changed) = self.auto_form(cx);
                let auto_problem = if auto { topup_problem(&below, &add, min) } else { None };
                let show_problem = problem.is_some() && (self.choice != "other" || !amount.is_empty());
                let left = billing
                    .credits
                    .as_ref()
                    .map(|(_, left)| money(left))
                    .unwrap_or_else(|| "$0".into());
                let chips = h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(AMOUNTS.into_iter().map(|value| {
                        let id = SharedString::from(format!("billing-amount-{value}"));
                        if number(value) < min {
                            ui::disabled_button(id, money(value), palette)
                        } else {
                            ui::chip(id, money(value), self.choice == value, palette).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.choice = value;
                                    cx.notify();
                                },
                            ))
                        }
                    }))
                    .child(
                        ui::chip("billing-amount-other", "Other", self.choice == "other", palette).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.choice = "other";
                                this.other.update(cx, |state, cx| state.focus(window, cx));
                                cx.notify();
                            }),
                        ),
                    )
                    .when(self.choice == "other", |row| {
                        row.child(
                            h_flex()
                                .gap(px(4.))
                                .text_size(px(13.))
                                .text_color(palette.faint)
                                .child("$")
                                .child(field(&self.other, 96., false, palette)),
                        )
                    });
                let auto_fields = h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Add $")
                    .child(field(&self.add, 72., !auto, palette))
                    .child("when the balance falls under $")
                    .child(field(&self.below, 72., !auto, palette));
                let buying = self.busy == Some(Busy::Buy);
                let buy_label = if buying {
                    "Buying…".to_string()
                } else if problem.is_some() {
                    words::BUY_CREDITS.to_string()
                } else {
                    format!("Buy {} of credits", money(&amount))
                };
                let off_reason = billing
                    .topup
                    .as_ref()
                    .and_then(|topup| topup.off_reason.clone())
                    .filter(|_| !auto);
                frame(words::BUY_CREDITS)
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_end()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.strong)
                                    .child(left),
                            )
                            .child(ui::note("left", palette)),
                    )
                    .child(chips)
                    .when(show_problem, |column| {
                        column.child(ui::error_line(problem.clone().unwrap_or_default(), palette))
                    })
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .pt(px(6.))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(6.))
                                    .child(div().text_size(px(13.)).text_color(palette.strong).child("Auto top-up"))
                                    .child(auto_fields)
                                    .child(ui::label(
                                        auto_problem.clone().unwrap_or_else(|| TOPUP_CAPS.into()),
                                        palette,
                                    ))
                                    .children(
                                        off_reason
                                            .map(|reason| ui::label(format!("{}.", topup_off_text(&reason)), palette)),
                                    ),
                            )
                            .child(ui::switch("billing-auto", auto, false, palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.auto = !this.auto;
                                    cx.notify();
                                },
                            ))),
                    )
                    .children(after_hint.map(|hint| ui::label(hint, palette)))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .pt(px(4.))
                            .child(
                                ui::note(
                                    method_note.clone().unwrap_or_else(|| {
                                        "You pay on Stripe's page in your browser; it saves the card.".into()
                                    }),
                                    palette,
                                )
                                .flex_1(),
                            )
                            .when(auto_changed && billing.method.is_some(), |row| {
                                row.child(if busy || auto_problem.is_some() {
                                    ui::disabled_button("billing-save-auto", "Save auto top-up", palette)
                                } else {
                                    ui::button("billing-save-auto", "Save auto top-up", palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.save_auto_only(cx)))
                                })
                            })
                            .child(if busy || problem.is_some() || auto_problem.is_some() {
                                ui::disabled_button("billing-buy-go", buy_label, palette)
                            } else {
                                ui::primary_button("billing-buy-go", buy_label, palette)
                                    .on_click(cx.listener(|this, _, _, cx| this.buy(cx)))
                            }),
                    )
            }
            Panel::Limit => {
                let after = billing.after.as_ref();
                let limit = usd_input(&self.limit.read(cx).value());
                let problem = limit_problem(&limit, pricing.max_limit.as_deref());
                let hint = problem.clone().unwrap_or_else(|| {
                    format!(
                        "{}{}",
                        limit_hint(pricing),
                        after
                            .map(|after| format!(" {} used this month.", cents(&after.month_spent)))
                            .unwrap_or_default()
                    )
                });
                let saving = self.busy == Some(Busy::Limit);
                frame(words::USAGE_LIMIT)
                    .child(ui::note(
                        format!(
                            "After the fact, Nexus charges the card every {} of unpaid usage; less carries over. Above the limit, Nexus-run AI pauses until the limit resets or goes up. {}.",
                            money(&pricing.threshold),
                            limit_resets(today())
                        ),
                        palette,
                    ))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(ui::label("Limit a month", palette))
                            .child(div().text_size(px(13.)).text_color(palette.faint).child("$"))
                            .child(field(&self.limit, 120., false, palette))
                            .child(div().flex_1())
                            .child(if busy || problem.is_some() {
                                ui::disabled_button("billing-limit-save", if saving { "Saving…" } else { "Save limit" }, palette)
                            } else {
                                ui::primary_button("billing-limit-save", "Save limit", palette)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_limit(cx)))
                            }),
                    )
                    .child(if problem.is_some() {
                        ui::error_line(hint, palette)
                    } else {
                        ui::label(hint, palette)
                    })
            }
            Panel::ConfirmTopup => {
                let (below, add) = billing
                    .topup
                    .as_ref()
                    .map(|topup| (topup.below.clone(), topup.add.clone()))
                    .unwrap_or((TOPUP_DEFAULT.0.into(), TOPUP_DEFAULT.1.into()));
                let card = billing
                    .method
                    .as_ref()
                    .map(Method::sentence)
                    .unwrap_or_else(|| "the card on file".into());
                let saving = self.busy == Some(Busy::Topup);
                frame("Turn on auto top-up?")
                    .child(ui::note(
                        format!(
                            "Each time {org}'s credits fall under {}, Nexus charges {card} {} and adds {} of credits.{}",
                            money(&below),
                            money(&add),
                            money(&add),
                            if billing.credits.as_ref().map_or(0., |(_, left)| number(left)) < number(&below) {
                                format!(
                                    " The credits are under {} now, so the first {} is charged right away.",
                                    money(&below),
                                    money(&add)
                                )
                            } else {
                                String::new()
                            }
                        ),
                        palette,
                    ))
                    .child(facts(
                        vec![("When credits fall under", cents(&below)), ("Nexus charges", cents(&add))],
                        palette,
                    ))
                    .child(ui::label(
                        format!(
                            "It can charge more than once a month. {TOPUP_CAPS} Change the amounts under Buy credits, and turn it off here at any time."
                        ),
                        palette,
                    ))
                    .child(confirm_row(
                        cancel(),
                        if busy {
                            ui::disabled_button(
                                "billing-topup-on",
                                if saving { "Turning on…".to_string() } else { format!("Turn on, {} each time", money(&add)) },
                                palette,
                            )
                        } else {
                            ui::primary_button("billing-topup-on", format!("Turn on, {} each time", money(&add)), palette)
                                .on_click(cx.listener(|this, _, _, cx| this.set_topup(true, cx)))
                        },
                    ))
            }
            Panel::ConfirmAfter => {
                let has_method = billing.method.is_some();
                let card = billing
                    .method
                    .as_ref()
                    .map(Method::sentence)
                    .unwrap_or_else(|| "the card you save on Stripe's page".into());
                let limit = billing
                    .after
                    .as_ref()
                    .map(|after| after.limit.clone())
                    .unwrap_or(pricing.default_limit.clone());
                let label = if self.busy == Some(Busy::After) {
                    "Turning on…".to_string()
                } else if self.busy == Some(Busy::Stripe) {
                    "Opening Stripe…".to_string()
                } else if has_method {
                    format!("Turn on, up to {}", money(&limit))
                } else {
                    "Continue to Stripe".to_string()
                };
                frame("Turn on pay after the fact?")
                    .child(ui::note(
                        format!(
                            "Nexus charges {card} each time {org}'s unpaid usage reaches {}. Less than that carries over to the next charge.",
                            money(&pricing.threshold)
                        ),
                        palette,
                    ))
                    .child(facts(
                        vec![
                            // Settings, as the billing glossary writes them, and the reset's day.
                            ("Charged at every", money(&pricing.threshold)),
                            (words::USAGE_LIMIT, format!("{} a month", money(&limit))),
                            ("Resets", limit_reset_day(today())),
                        ],
                        palette,
                    ))
                    .child(ui::label(
                        format!(
                            "Credits pay first. At the limit, models Nexus runs stop until the limit resets or an owner raises it.{}",
                            if has_method { "" } else { " It turns on once Stripe saves the card." }
                        ),
                        palette,
                    ))
                    .child(confirm_row(cancel(), if busy {
                        ui::disabled_button("billing-after-on", label, palette)
                    } else {
                        ui::primary_button("billing-after-on", label, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.set_after(true, cx)))
                    }))
            }
            Panel::StopAfter => {
                let note = billing
                    .after
                    .as_ref()
                    .map(|after| stop_after_note(after, pricing))
                    .unwrap_or_else(|| "No additional unpaid usage is currently due.".into());
                let label = if self.busy == Some(Busy::After) {
                    "Turning off…"
                } else {
                    "Turn off and settle usage"
                };
                frame("Turn off paying after the fact?")
                    .child(ui::note(
                        "New calls will stop using the card. Credits and your own keys can still pay for calls.",
                        palette,
                    ))
                    .child(ui::note(note, palette))
                    .children(method_note.clone().map(|note| ui::label(note, palette)))
                    .child(confirm_row(
                        cancel(),
                        if busy {
                            ui::disabled_button("billing-after-off", label, palette)
                        } else {
                            ui::primary_button("billing-after-off", label, palette)
                                .on_click(cx.listener(|this, _, _, cx| this.set_after(false, cx)))
                        },
                    ))
            }
        };
        body.children(outcome(&self.panel_note, palette))
    }

    pub(super) fn render_receipts(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let rows = match &self.receipts {
            Load::Ready(rows) => Some(rows),
            _ => None,
        };
        let meta = rows.filter(|rows| !rows.is_empty()).map(|rows| {
            let csv = receipts_csv(rows);
            h_flex()
                .mt(px(14.))
                .gap(px(10.))
                .child(ui::note(
                    format!(
                        "{} {}",
                        rows.len(),
                        if rows.len() == 1 { "payment" } else { "payments" }
                    ),
                    palette,
                ))
                .child(
                    ui::button(
                        "billing-csv",
                        if self.copied { "Copied" } else { "Copy as CSV" },
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(csv.clone()));
                        this.copied = true;
                        cx.notify();
                    })),
                )
        });
        let body: AnyElement = match &self.receipts {
            Load::Loading => ui::skeleton(3, palette).into_any_element(),
            Load::Failed(error) => v_flex()
                .gap(px(8.))
                .child(ui::error_line(format!("Receipts didn't load: {error}"), palette))
                .child(
                    ui::button("billing-receipts-retry", "Try again", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.read_receipts(cx))),
                )
                .into_any_element(),
            Load::Ready(rows) if rows.is_empty() => {
                let after = match &self.billing {
                    Load::Ready(Some(billing)) => billing.after.as_ref(),
                    _ => None,
                };
                ui::note(charge::receipts_empty_text(after), palette).into_any_element()
            }
            Load::Ready(rows) => v_flex()
                .child(
                    ui::head_row(palette)
                        .child(div().w(px(90.)).child("Receipt"))
                        .child(div().w(px(96.)).child("Date"))
                        .child(div().flex_1().child("For"))
                        .child(div().w(px(90.)).flex().justify_end().child("Amount"))
                        .child(div().w(px(64.)).child("Status"))
                        .child(div().w(px(84.))),
                )
                .children(rows.iter().map(|row| {
                    let opening = self.busy == Some(Busy::Receipt(row.id.clone()));
                    let link = if row.url.is_some() {
                        Some(if row.status == "paid" || row.status == "void" {
                            "Invoice ↗"
                        } else {
                            "Pay ↗"
                        })
                    } else if row.receipt {
                        Some(if opening { "Opening…" } else { "Receipt ↗" })
                    } else {
                        None
                    };
                    let receipt = row.clone();
                    ui::row(palette)
                        .min_h(px(40.))
                        .text_size(px(13.))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .text_color(palette.faint)
                                .truncate()
                                .child(row.label()),
                        )
                        .child(
                            div()
                                .w(px(96.))
                                .flex_none()
                                .text_color(palette.foreground)
                                .child(day_label(&row.at)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(palette.foreground)
                                .child(row.description.clone()),
                        )
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .flex()
                                .justify_end()
                                .text_color(palette.strong)
                                .child(cents(&row.amount)),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .flex_none()
                                .text_color(if row.status == "failed" {
                                    palette.destructive
                                } else {
                                    palette.muted
                                })
                                .child(status_word(&row.status).to_string()),
                        )
                        .child(
                            div().w(px(84.)).flex_none().flex().justify_end().child(match link {
                                Some(label) => div()
                                    .id(SharedString::from(format!("billing-receipt-{}", row.id)))
                                    .text_color(palette.muted)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(palette.strong))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_receipt(&receipt, cx)))
                                    .into_any_element(),
                                None => div().text_color(palette.faint).child("–").into_any_element(),
                            }),
                        )
                }))
                .into_any_element(),
        };
        v_flex()
            .child(
                h_flex()
                    .justify_between()
                    .child(ui::section_head("Receipts", palette))
                    .children(meta),
            )
            .child(body)
            .children(outcome(&self.receipts_note, palette))
    }
}
