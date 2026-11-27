use super::*;

impl BillingPage {
    // -------------------------------------------------------------- Changes

    /// The switch: on asks first (`Panel::ConfirmTopup`), because it charges the card; off charges nothing.
    pub(super) fn switch_topup(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            self.open_panel(Panel::ConfirmTopup, window, cx);
        } else {
            self.set_topup(false, cx);
        }
    }

    /// The switch: each way asks first, on with what the card is charged, off with the final charge.
    pub(super) fn switch_after(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_panel(if on { Panel::ConfirmAfter } else { Panel::StopAfter }, window, cx);
    }

    pub(super) fn set_topup(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(billing) = self.billing() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        let body = if !on {
            json!({ "enabled": false })
        } else if billing.topup.is_some() {
            json!({ "enabled": true })
        } else {
            json!({ "enabled": true, "belowUsd": TOPUP_DEFAULT.0, "addUsd": TOPUP_DEFAULT.1 })
        };
        self.busy = Some(Busy::Topup);
        self.pays_note = None;
        self.steps(
            vec![(Verb::Put, "/v1/billing/auto-topup".into(), body)],
            cx,
            |this, failure, cx| {
                this.busy = None;
                match failure {
                    Some(error) => {
                        let text = format!("Auto top-up didn't change. Nothing was charged. {}", plain(&error));
                        if this.panel == Some(Panel::ConfirmTopup) {
                            this.panel_note = failed(&text);
                        } else {
                            this.pays_note = failed(&text);
                        }
                    }
                    None if this.panel == Some(Panel::ConfirmTopup) => this.panel = None,
                    None => {}
                }
                this.reload(cx);
            },
        );
        cx.notify();
    }

    pub(super) fn set_after(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(billing) = self.billing() else {
            return;
        };
        if on && billing.method.is_none() {
            // The card first: once Stripe's card shows, paying after the fact turns on.
            self.panel = None;
            self.card_setup(
                Deferred {
                    pay_after: true,
                    topup: None,
                },
                cx,
            );
            return;
        }
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::After);
        self.pays_note = None;
        self.steps(
            vec![(Verb::Put, "/v1/billing/after-the-fact".into(), json!({ "enabled": on }))],
            cx,
            |this, failure, cx| {
                this.busy = None;
                let confirming = matches!(this.panel, Some(Panel::ConfirmAfter | Panel::StopAfter | Panel::Setup));
                match failure {
                    Some(error) => {
                        let text = format!("Paying after the fact didn't change. {}", plain(&error));
                        if confirming {
                            this.panel_note = failed(&text);
                        } else {
                            this.pays_note = failed(&text);
                        }
                    }
                    None if confirming => this.panel = None,
                    None => {}
                }
                this.reload(cx);
            },
        );
        cx.notify();
    }

    pub(super) fn open_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Some(panel);
        self.panel_note = None;
        if let Some(billing) = self.billing().cloned() {
            match panel {
                Panel::Setup => {
                    self.setup_credits = true;
                    self.setup_after = false;
                }
                Panel::Credits(_) => {
                    self.choice = "50";
                    self.auto = billing.topup.as_ref().is_some_and(|topup| topup.enabled);
                    let (below, add) = billing
                        .topup
                        .as_ref()
                        .map(|topup| (plain_number(&topup.below), plain_number(&topup.add)))
                        .unwrap_or((TOPUP_DEFAULT.0.into(), TOPUP_DEFAULT.1.into()));
                    self.other.update(cx, |state, cx| state.set_value("", window, cx));
                    self.below.update(cx, |state, cx| state.set_value(below, window, cx));
                    self.add.update(cx, |state, cx| state.set_value(add, window, cx));
                }
                Panel::Limit => {
                    let limit = billing
                        .after
                        .as_ref()
                        .map(|after| after.limit.clone())
                        .unwrap_or(billing.pricing.default_limit.clone());
                    let limit = plain_number(&limit);
                    self.limit.update(cx, |state, cx| {
                        state.set_value(limit, window, cx);
                        state.focus(window, cx);
                    });
                }
                Panel::ConfirmTopup | Panel::ConfirmAfter | Panel::StopAfter => {}
            }
        }
        cx.notify();
    }

    pub(super) fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        self.panel_note = None;
        cx.notify();
    }

    pub(super) fn continue_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(billing) = self.billing() else {
            return;
        };
        let has_method = billing.method.is_some();
        let (credits, after) = (self.setup_credits, self.setup_after);
        if credits {
            // Paying after the fact goes with the purchase: it turns on once the purchase is paid, never before. Auto
            // top-up starts on, as the setup offered.
            self.open_panel(Panel::Credits(after), window, cx);
            self.auto = true;
            cx.notify();
        } else if has_method {
            // The setup panel said what it charges; its button says it turns on.
            self.set_after(true, cx);
        } else {
            self.panel = None;
            self.card_setup(
                Deferred {
                    pay_after: after,
                    topup: None,
                },
                cx,
            );
        }
    }

    /// The auto top-up as the credit form holds it, and whether it differs from what is saved.
    pub(super) fn auto_form(&self, cx: &App) -> (bool, String, String, bool) {
        let below = usd_input(&self.below.read(cx).value());
        let add = usd_input(&self.add.read(cx).value());
        let saved = self.billing().and_then(|billing| billing.topup.clone());
        let saved_on = saved.as_ref().is_some_and(|topup| topup.enabled);
        let changed = self.auto != saved_on
            || (self.auto
                && saved
                    .as_ref()
                    .is_none_or(|topup| number(&below) != number(&topup.below) || number(&add) != number(&topup.add)));
        (self.auto, below, add, changed)
    }

    pub(super) fn amount(&self, cx: &App) -> String {
        if self.choice == "other" {
            usd_input(&self.other.read(cx).value())
        } else {
            self.choice.to_string()
        }
    }

    pub(super) fn auto_step(&self, cx: &App) -> Option<(Verb, String, Value)> {
        let (auto, below, add, changed) = self.auto_form(cx);
        changed.then(|| {
            let body = if auto {
                json!({ "enabled": true, "belowUsd": below, "addUsd": add })
            } else {
                json!({ "enabled": false })
            };
            (Verb::Put, "/v1/billing/auto-topup".to_string(), body)
        })
    }

    pub(super) fn buy(&mut self, cx: &mut Context<Self>) {
        let Some(Panel::Credits(pay_after)) = self.panel else {
            return;
        };
        let Some(billing) = self.billing().cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        let min = number(&billing.pricing.min_purchase);
        let typed = self.amount(cx);
        let (auto, below, add, auto_changed) = self.auto_form(cx);
        if purchase_problem(&typed, min).is_some() || (auto && topup_problem(&below, &add, min).is_some()) {
            return;
        }
        let mut after_steps: Vec<(Verb, String, Value)> = self.auto_step(cx).into_iter().collect();
        let after_on = billing.after.as_ref().is_some_and(|after| after.enabled);
        if pay_after && !after_on {
            after_steps.push((
                Verb::Put,
                "/v1/billing/after-the-fact".into(),
                json!({ "enabled": true }),
            ));
        }
        let deferred = Deferred {
            pay_after,
            topup: (auto && auto_changed).then(|| (below.clone(), add.clone())),
        };
        let before = watched_key(WaitKind::Credits, &billing);
        self.busy = Some(Busy::Buy);
        self.panel_note = None;
        cx.notify();
        fetch(
            cx,
            self.ctx.gateway.request(
                Verb::Post,
                "/v1/billing/credits".into(),
                Some(json!({ "amountUsd": typed })),
            ),
            move |this, answer, cx| {
                this.busy = None;
                let answer = match answer {
                    Ok(answer) => answer,
                    Err(error) => {
                        this.panel_note = failed(&format!(
                            "The purchase didn't go through. Nothing was charged. {}",
                            plain(&error)
                        ));
                        return;
                    }
                };
                match ui::text(&answer, "status") {
                    "credited" => {
                        // Auto top-up and paying after the fact need the card the purchase used: it is on file now.
                        this.panel = None;
                        this.top_note = done(format!("{} of credits added.", cents(&amount(&answer, "amountUsd"))));
                        this.steps(after_steps, cx, |this, failure, cx| {
                            if let Some(error) = failure {
                                this.pays_note = failed(&plain(&error));
                            }
                            this.reload(cx);
                        });
                    }
                    "processing" => {
                        // What was chosen with the purchase waits for it to clear, as after Stripe's page.
                        this.panel = None;
                        this.watch(WaitKind::Credits, before, deferred, cx);
                        this.top_note = done(format!(
                            "Stripe is taking the payment of {}. The credits show here once it clears.",
                            cents(&amount(&answer, "amountUsd"))
                        ));
                    }
                    _ => match answer.get("url").and_then(Value::as_str) {
                        Some(url) => {
                            crate::external_link::open(url, cx);
                            this.panel = None;
                            this.watch(WaitKind::Credits, before, deferred, cx);
                        }
                        None => this.panel_note = failed("Nexus's answer had no Stripe page to pay on."),
                    },
                }
            },
        );
    }

    pub(super) fn save_auto_only(&mut self, cx: &mut Context<Self>) {
        let Some(billing) = self.billing() else {
            return;
        };
        let (auto, below, add, _) = self.auto_form(cx);
        if self.busy.is_some() || (auto && topup_problem(&below, &add, number(&billing.pricing.min_purchase)).is_some())
        {
            return;
        }
        let Some(step) = self.auto_step(cx) else {
            return;
        };
        self.busy = Some(Busy::Topup);
        self.panel_note = None;
        self.steps(vec![step], cx, |this, failure, cx| {
            this.busy = None;
            match failure {
                Some(error) => this.panel_note = failed(&format!("Auto top-up didn't change. {}", plain(&error))),
                None => this.panel = None,
            }
            this.reload(cx);
        });
        cx.notify();
    }

    pub(super) fn save_limit(&mut self, cx: &mut Context<Self>) {
        let Some(billing) = self.billing() else {
            return;
        };
        if self.panel != Some(Panel::Limit) || self.busy.is_some() {
            return;
        }
        let limit = usd_input(&self.limit.read(cx).value());
        if limit_problem(&limit, billing.pricing.max_limit.as_deref()).is_some() {
            return;
        }
        // The limit alone changes: the setting stays as it is. Turning it on is the switch's (`set_after`), which
        // asks first; with no setting yet, saving a limit never turns it on.
        let enabled = billing.after.as_ref().is_some_and(|after| after.enabled);
        self.busy = Some(Busy::Limit);
        self.panel_note = None;
        self.steps(
            vec![(
                Verb::Put,
                "/v1/billing/after-the-fact".into(),
                json!({ "enabled": enabled, "limitUsd": limit }),
            )],
            cx,
            |this, failure, cx| {
                this.busy = None;
                match failure {
                    Some(error) => this.panel_note = failed(&format!("The limit didn't change. {}", plain(&error))),
                    None => {
                        this.panel = None;
                        this.pays_note = done("Usage limit saved.");
                    }
                }
                this.reload(cx);
            },
        );
        cx.notify();
    }
}
