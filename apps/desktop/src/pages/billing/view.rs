use super::*;

fn manage_billing_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::button("billing-manage", "Manage in Stripe ↗", palette)
    } else {
        ui::disabled_button("billing-manage", "Manage in Stripe ↗", palette)
    }
}

pub(in crate::pages) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .child(ui::section_head("Balance", palette))
            .child(manage_billing_button(palette, false))
            .into_any_element()
    })
}

impl BillingPage {
    // -------------------------------------------------------------- Drawing

    pub(super) fn render_banners(&self, billing: &Billing, org: &str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let now = today();
        let stripe = self.busy == Some(Busy::Stripe);
        let card_action = if stripe {
            "Opening Stripe…"
        } else {
            words::UPDATE_CARD
        };
        let mut banners: Vec<AnyElement> = Vec::new();
        if let Some((text, action, urgent)) = charge_banner(billing) {
            // 3-D Secure: Gateway confirms that same charge again, and the bank's page approves it.
            let confirm = action == charge::Action::ConfirmPayment;
            banners.push(
                banner(
                    "billing-fix",
                    urgent,
                    text,
                    if stripe { "Opening Stripe…" } else { action.label() },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if confirm {
                        this.confirm_charge(cx)
                    } else {
                        this.card_setup(Deferred::default(), cx)
                    }
                }))
                .into_any_element(),
            );
        }
        if let Some(due) = &billing.past_due {
            let (year, month, day) = now;
            let today = format!("{year:04}-{month:02}-{day:02}");
            let stop = match &due.blocked_from {
                None => "Stripe is still trying to charge the card.".to_string(),
                Some(from) if ui::day(from) <= today.as_str() => {
                    format!(
                        "Model calls stopped on {} and start again once it is paid.",
                        cutoff(from)
                    )
                }
                Some(from) => format!("Model calls stop on {} unless it is paid.", cutoff(from)),
            };
            let text = format!("The invoice for {} is unpaid. {stop}", month_label(&due.period));
            let receipts = match &self.receipts {
                Load::Ready(rows) => rows.as_slice(),
                _ => &[],
            };
            // Stripe's page pays it; another card is what Stripe charges on its next try.
            // Paying resolves it, so it leads and says it leaves for Stripe; another card is the quieter way.
            let pay = past_due_invoice(due, receipts).map(|url| {
                ui::button("billing-past-due-pay", format!("{} ↗", words::PAY_INVOICE), palette)
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
            });
            banners.push(
                banner_text(true, text, palette)
                    .child(
                        h_flex().flex_none().gap(px(8.)).children(pay).child(
                            ui::ghost_button("billing-past-due-card", card_action, palette)
                                .on_click(cx.listener(|this, _, _, cx| this.card_setup(Deferred::default(), cx))),
                        ),
                    )
                    .into_any_element(),
            );
        }
        if usage_limit_reached(billing)
            && let Some(after) = billing.after.as_ref()
        {
            banners.push(
                banner(
                    "billing-limit-reached",
                    false,
                    usage_limit_text(org, after, now),
                    words::CHANGE_LIMIT,
                    palette,
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Limit, window, cx)))
                .into_any_element(),
            );
        }
        if free_start_used_up(billing) {
            let pick_plan = self.offers_plans;
            let granted = billing
                .free_start
                .as_ref()
                .map(|(granted, _)| money(granted))
                .unwrap_or_else(|| "$1".into());
            banners.push(
                banner(
                    "billing-free-used",
                    false,
                    format!("Your free start of {granted} is used up. Set up billing to keep going."),
                    if pick_plan {
                        "Pick a plan to keep going"
                    } else {
                        words::SET_UP_BILLING
                    },
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if pick_plan {
                        this.ctx.open_web("settings/billing#plans", cx);
                    } else {
                        this.open_panel(Panel::Setup, window, cx);
                    }
                }))
                .into_any_element(),
            );
        } else if let Some((text, fix)) = topup_off_notice(billing) {
            let action = match fix {
                TopupFix::BuyCredits => words::BUY_CREDITS,
                _ if stripe => "Opening Stripe…",
                TopupFix::AddAnotherCard => words::ADD_ANOTHER_CARD,
                TopupFix::UpdateCard => words::UPDATE_CARD,
            };
            banners.push(
                banner("billing-topup-off", true, text, action, palette)
                    .on_click(cx.listener(move |this, _, window, cx| match fix {
                        TopupFix::BuyCredits => this.open_panel(Panel::Credits(false), window, cx),
                        _ => this.card_setup(Deferred::default(), cx),
                    }))
                    .into_any_element(),
            );
        } else if credits_low(billing) && !billing.after.as_ref().is_some_and(|after| after.enabled) {
            let line = billing
                .topup
                .as_ref()
                .map(|topup| money(&topup.below))
                .unwrap_or_else(|| "$10".into());
            banners.push(
                banner(
                    "billing-low",
                    true,
                    format!("{org}'s credits are under {line}. Auto top-up is off."),
                    words::BUY_CREDITS,
                    palette,
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Credits(false), window, cx)))
                .into_any_element(),
            );
        }
        if let Some(card) = &billing.card {
            let short = Method {
                kind: "card".into(),
                brand: card.brand.clone(),
                bank: String::new(),
                last4: card.last4.clone(),
            }
            .short();
            let text = if card.expired(now) {
                Some(format!("The {short} has expired. Stripe can't charge it."))
            } else if card.expires_this_month(now) {
                Some(format!("The {short} expires this month."))
            } else {
                None
            };
            if let Some(text) = text {
                banners.push(
                    banner("billing-card", true, text, card_action, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.card_setup(Deferred::default(), cx)))
                        .into_any_element(),
                );
            }
        }
        v_flex().gap(px(8.)).children(banners)
    }

    pub(super) fn render_summary(&self, billing: &Billing, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (mode, mode_detail, _) = pay_mode(billing);
        let credits = billing.credits.as_ref();
        let topup = billing.topup.as_ref();
        let after = billing.after.as_ref();
        let cell = |label: &'static str, value: String, detail: String| {
            ui::card(palette)
                .flex_1()
                .min_w(px(140.))
                .child(ui::label(label, palette))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .truncate()
                        .child(value),
                )
                .child(div().text_size(px(12.)).text_color(palette.faint).child(detail))
        };
        let credits_detail = match (credits, topup) {
            (None, _) => "None bought".to_string(),
            (Some(_), Some(topup)) if topup.enabled => {
                format!("Top up {} under {}", money(&topup.add), money(&topup.below))
            }
            _ => format!("{} off", words::AUTO_TOPUP),
        };
        let after_detail = after_summary_detail(after, &billing.pricing);
        let (payment, payment_detail) = match (&billing.method, &billing.card) {
            (_, Some(card)) => (
                billing.method.as_ref().map(Method::short).unwrap_or_default(),
                format!(
                    "{} {}",
                    if card.expired(today()) { "expired" } else { "expires" },
                    card.expiry()
                ),
            ),
            (Some(method), None) => (method.short(), method.sentence()),
            (None, None) => ("No card".into(), "Added on Stripe's page".into()),
        };
        let stripe = self.busy == Some(Busy::Stripe);
        let allowances = billing.allowance.as_ref().map(|allowance| {
            v_flex().child(ui::section_head("Monthly allowances", palette)).child(
                h_flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .children(allowance.cloud.map(|(used, left)| {
                        cell("Cloud hours", format!("{left:.2} h left"), format!("{used:.2} h used"))
                    }))
                    .children(allowance.ai.as_ref().map(|(used, left)| {
                        cell(
                            "AI allowance",
                            format!("{} left", money(left)),
                            format!("{} used", money(used)),
                        )
                    }))
                    .child(cell("Resets", cutoff(&allowance.reset_at), "Monthly".into())),
            )
        });
        v_flex()
            .child(
                h_flex()
                    .justify_between()
                    .child(ui::section_head("Balance", palette))
                    .child(
                        h_flex()
                            .mt(px(14.))
                            .gap(px(8.))
                            .when(self.offers_plans, |row| {
                                row.child(ui::ghost_button("billing-plans", "View plans ↗", palette).on_click(
                                    cx.listener(|this, _, _, cx| this.ctx.open_web("settings/billing#plans", cx)),
                                ))
                            })
                            .when(billing.test_mode, |row| {
                                row.child(ui::badge("Stripe test mode", palette))
                            })
                            .child(if stripe {
                                ui::disabled_button("billing-manage", "Opening Stripe…", palette)
                            } else {
                                manage_billing_button(palette, true)
                                    .on_click(cx.listener(|this, _, _, cx| this.portal(cx)))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(cell("Mode", billing.plan_name.unwrap_or(mode).to_string(), mode_detail))
                    .child(cell(
                        words::CREDITS,
                        credits.map(|(_, left)| money(left)).unwrap_or_else(|| "–".into()),
                        credits_detail,
                    ))
                    .when(number(&billing.promotional_credit) > 0., |row| {
                        row.child(cell(
                            words::PROMOTIONAL_CREDIT,
                            money(&billing.promotional_credit),
                            "Referral and promotional rewards".into(),
                        ))
                    })
                    .child(cell(
                        words::PAY_AFTER_THE_FACT,
                        after
                            .map(|after| cents(charge::unpaid(after)))
                            .unwrap_or_else(|| "–".into()),
                        after_detail,
                    ))
                    .child(cell("Payment", payment, payment_detail)),
            )
            .children(allowances)
    }

    pub(super) fn render_pays(&self, billing: &Billing, org: &str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let pricing = &billing.pricing;
        let has_method = billing.method.is_some();
        let topup = billing.topup.as_ref();
        let after = billing.after.as_ref();
        let payg = pay_mode(billing).2;
        let locked = self.busy.is_some();
        let credits_text = match topup {
            Some(Topup {
                off_reason: Some(reason),
                ..
            }) => format!("Prepaid. {}.", topup_off_text(reason)),
            Some(topup) if topup.enabled => format!(
                "Prepaid. Auto top-up adds {} when the balance falls under {}.",
                money(&topup.add),
                money(&topup.below)
            ),
            _ => format!(
                "Prepaid from {}, never expiring. Auto top-up is off.",
                money(&pricing.min_purchase)
            ),
        };
        let limit = after
            .map(|after| money(&after.limit))
            .unwrap_or_else(|| money(&pricing.default_limit));
        // The charge's phase first (`charge::notice`): a problem, a charge going through, or the final charge.
        // "Paused" only while calls on the card stop: with the setting off, nothing runs on the card.
        let pay = after.and_then(|after| charge::notice(after, &pricing.min_final_charge));
        let after_text = match pay {
            Some(pay) => format!("{}.{}", pay.title, if pay.stops_calls { " Paused." } else { "" }),
            None => format!(
                "Charged every {} of unpaid usage; less carries over. {} {limit} a month{}.",
                money(&pricing.threshold),
                words::USAGE_LIMIT,
                after
                    .map(|after| format!(", {} used. {}", cents(&after.month_spent), limit_resets(today())))
                    .unwrap_or_default()
            ),
        };
        let topup_on = topup.is_some_and(|topup| topup.enabled);
        let topup_locked = locked || (!topup_on && !has_method);
        let after_on = after.is_some_and(|after| after.enabled);
        let row = |switch: Option<Stateful<Div>>, name: &'static str, note: String, action: Stateful<Div>| {
            h_flex()
                .gap(px(12.))
                .py(px(12.))
                .children(switch)
                .child(ui::two_lines(name, note, palette).flex_1())
                .child(action)
        };
        let site = self.ctx.gateway.site_url();
        v_flex()
            .child(
                h_flex()
                    .justify_between()
                    .child(ui::section_head(format!("How {org} pays"), palette))
                    .child(
                        ui::web_link("billing-pricing", "Pricing", palette)
                            .mt(px(14.))
                            .on_click(move |_, _, cx| crate::external_link::open(&format!("{site}/pricing"), cx)),
                    ),
            )
            .child(row(
                Some(
                    ui::switch("billing-topup", topup_on, topup_locked, palette).when(!topup_locked, |switch| {
                        switch.on_click(cx.listener(move |this, _, window, cx| this.switch_topup(!topup_on, window, cx)))
                    }),
                ),
                words::CREDITS,
                credits_text,
                ui::button("billing-buy", words::BUY_CREDITS, palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Credits(false), window, cx))),
            ))
            .child(row(
                Some(
                    ui::switch("billing-after", after_on, locked, palette).when(!locked, |switch| {
                        switch.on_click(cx.listener(move |this, _, window, cx| this.switch_after(!after_on, window, cx)))
                    }),
                ),
                words::PAY_AFTER_THE_FACT,
                after_text,
                if after.is_some() {
                    ui::button("billing-limit", words::CHANGE_LIMIT, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Limit, window, cx)))
                } else {
                    ui::disabled_button("billing-limit", words::CHANGE_LIMIT, palette)
                },
            ))
            .when(!payg, |column| {
                column.child(row(
                    None,
                    words::PAY_AS_YOU_GO,
                    "No monthly fee. Pay only for what Nexus runs, with credits, after the fact, or both.".into(),
                    ui::primary_button("billing-setup", words::SET_UP_BILLING, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.open_panel(Panel::Setup, window, cx))),
                ))
            })
            .child(ui::note(
                format!(
                    "Linked subscriptions and your own keys are free and never draw on this balance. AI that Nexus runs costs the provider's price + {}%.",
                    pricing.markup_bps as f64 / 100.
                ),
                palette,
            ).pt(px(10.)))
            .children(outcome(&self.pays_note, palette))
    }
}

impl Render for BillingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let name = match &self.me {
            Load::Ready(me) if !me.name.is_empty() => me.name.clone(),
            _ => String::new(),
        };
        let lead = if name.is_empty() {
            "The balance, how it pays, receipts and payment.".to_string()
        } else {
            format!("{name}'s balance, how it pays, receipts and payment.")
        };
        let page = v_flex().w_full().child(ui::lead(lead, palette)).child(
            ui::settings_notice(
                "Secure payment handoff",
                "Checkout and payment details open in your browser. Return here to see the updated billing state.",
                palette,
            )
            .mb(px(12.)),
        );
        let org = if name.is_empty() {
            "This organization".to_string()
        } else {
            name
        };
        match (&self.me, &self.billing) {
            (Load::Failed(error), _) | (_, Load::Failed(error)) => {
                return page.child(v_flex().items_center().child(ui::failed(error.clone(), palette)).child(
                    ui::button("billing-retry", "Try again", palette).on_click(cx.listener(|this, _, _, cx| {
                        if matches!(this.me, Load::Failed(_)) {
                            // The page asks who you are again, then billing.
                            let ctx = this.ctx.clone();
                            this.me = Load::Loading;
                            super::super::read(cx, ctx.gateway.query("/v1/tenants/me".into()), |this, answer, cx| {
                                this.me = match answer {
                                    Ok(answer) => Load::Ready(parse_me(&answer)),
                                    Err(error) => Load::Failed(error.into()),
                                };
                                if matches!(&this.me, Load::Ready(me) if me.owner) {
                                    this.reload(cx);
                                }
                            });
                        } else {
                            this.billing = Load::Loading;
                            this.reload(cx);
                        }
                        cx.notify();
                    })),
                ));
            }
            (Load::Ready(me), _) if !me.owner => {
                let navigate = self.ctx.navigate.clone();
                return page.child(
                    ui::empty_state(
                        Lucide::Lock,
                        "Only owners see billing",
                        "Ask an owner of this organization about its balance, card and receipts.",
                        palette,
                    )
                    // 64px under the lead: the state brings its own top space, so take back the lead's margin.
                    .mt(px(-14.))
                    .child(
                        ui::button("billing-owners", "See who the owners are", palette)
                            .on_click(move |_, window, cx| navigate(PageId::Members, window, cx)),
                    ),
                );
            }
            (Load::Ready(_), Load::Ready(None)) => {
                return page.child(ui::empty(
                    "Billing isn't on yet",
                    "Nexus doesn't take payments here yet, so nothing is charged.",
                    palette,
                ));
            }
            (Load::Ready(_), Load::Ready(Some(_))) => {}
            _ => return page.child(ui::skeleton(5, palette)),
        }
        let Some(billing) = self.billing().cloned() else {
            return page;
        };
        page.children(self.stale.as_ref().map(|error| {
            h_flex()
                .gap(px(10.))
                .pb(px(8.))
                .child(ui::note(stale_note(error), palette).flex_1())
                .child(
                    ui::button("billing-stale-retry", words::TRY_AGAIN, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
        }))
        .child(self.render_banners(&billing, &org, palette, cx))
            .children(outcome(&self.top_note, palette))
            .child(self.render_summary(&billing, palette, cx))
            .child(self.render_pays(&billing, &org, palette, cx))
            .children(self.panel.map(|panel| self.render_panel(&billing, &org, panel, palette, cx)))
            .child(self.render_receipts(palette, cx))
            .child(ui::note(words::DATES_IN_UTC, palette).pt(px(8.)))
            .child(ui::section_head("Billing details", palette))
            .child(ui::setting(
                "Company, address and tax ID",
                "Printed on every receipt. Stripe keeps them, with the billing email and a PO number, and Nexus never sees a card number.",
                if self.busy == Some(Busy::Stripe) {
                    ui::disabled_button("billing-details", "Opening Stripe…", palette)
                } else {
                    ui::button("billing-details", "Edit in Stripe ↗", palette).on_click(cx.listener(|this, _, _, cx| this.portal(cx)))
                },
                palette,
            ))
    }
}

/// A confirm panel's figures: a label and its amount per line.

pub(super) fn facts(rows: Vec<(&'static str, String)>, palette: Palette) -> Div {
    v_flex().gap(px(4.)).children(rows.into_iter().map(|(label, value)| {
        h_flex()
            .justify_between()
            .text_size(px(13.))
            .child(div().text_color(palette.faint).child(label))
            .child(div().text_color(palette.strong).child(value))
    }))
}

/// A confirm panel's buttons, right-aligned as the design system's dialog has them: Cancel, then one primary.
pub(super) fn confirm_row(cancel: Stateful<Div>, primary: Stateful<Div>) -> Div {
    h_flex()
        .justify_end()
        .gap(px(8.))
        .pt(px(4.))
        .child(cancel)
        .child(primary)
}

/// A payment banner with its action. Billing is never red: `act` draws the amber status mark, only when someone
/// must act (a failed charge, past due, low credits, a card that expires). A limit reached
/// as set (the usage limit, the free start used up) carries no mark.
pub(super) fn banner(
    id: &'static str,
    act: bool,
    text: String,
    action: &'static str,
    palette: Palette,
) -> Stateful<Div> {
    // The whole banner is the button's target, so the caller's click handler is on it.
    banner_text(act, text, palette).id(id).child(ui::button(
        SharedString::from(format!("{id}-action")),
        action,
        palette,
    ))
}

pub(super) fn banner_text(act: bool, text: String, palette: Palette) -> Div {
    h_flex()
        .gap(px(10.))
        .p(px(12.))
        .rounded(px(10.))
        .bg(palette.raised)
        // The mark is 7 px on a banner.
        .when(act, |row| {
            row.child(div().size(px(7.)).flex_none().rounded(px(3.5)).bg(palette.warning))
        })
        .child(
            div()
                .flex_1()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(text),
        )
}

/// A text field in a soft box.
pub(super) fn field(input: &Entity<InputState>, width: f32, disabled: bool, palette: Palette) -> Div {
    h_flex()
        .w(px(width))
        .h(px(30.))
        .px(px(8.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(13.))
        .text_color(palette.foreground)
        .when(disabled, |field| field.opacity(0.5))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(Input::new(input).appearance(false).disabled(disabled)),
        )
}

pub(super) fn outcome(note: &Option<Outcome>, palette: Palette) -> Option<Div> {
    note.as_ref().map(|note| {
        if note.error {
            ui::error_line(note.text.clone(), palette).pt(px(8.))
        } else {
            ui::note(note.text.clone(), palette).pt(px(8.))
        }
    })
}
