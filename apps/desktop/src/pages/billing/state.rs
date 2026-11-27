use super::*;

pub(super) struct BillingPage {
    pub(super) ctx: Ctx,
    pub(super) me: Load<Me>,
    /// `None`: billing is off on this Gateway.
    pub(super) billing: Load<Option<Billing>>,
    pub(super) receipts: Load<Vec<Receipt>>,
    pub(super) offers_plans: bool,
    pub(super) panel: Option<Panel>,
    pub(super) setup_credits: bool,
    pub(super) setup_after: bool,
    /// The credit form's amount: one of `AMOUNTS`, or `other`.
    pub(super) choice: &'static str,
    pub(super) auto: bool,
    pub(super) other: Entity<InputState>,
    pub(super) below: Entity<InputState>,
    pub(super) add: Entity<InputState>,
    pub(super) limit: Entity<InputState>,
    pub(super) busy: Option<Busy>,
    /// Under the banners: Stripe's pages and the waits for them.
    pub(super) top_note: Option<Outcome>,
    pub(super) pays_note: Option<Outcome>,
    pub(super) panel_note: Option<Outcome>,
    pub(super) receipts_note: Option<Outcome>,
    /// A refresh after a good read failed: what shows is from before.
    pub(super) stale: Option<String>,
    pub(super) copied: bool,
    pub(super) _watch: Option<Task<()>>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl BillingPage {
    pub(super) fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> BillingPage {
        let other = cx.new(|cx| InputState::new(window, cx).placeholder("Amount"));
        let below = cx.new(|cx| InputState::new(window, cx));
        let add = cx.new(|cx| InputState::new(window, cx));
        let limit = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&other, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.buy(cx);
                }
            }),
            cx.subscribe_in(&limit, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_limit(cx);
                }
            }),
        ];
        let page = BillingPage {
            ctx,
            me: Load::Loading,
            billing: Load::Loading,
            receipts: Load::Loading,
            offers_plans: false,
            panel: None,
            setup_credits: true,
            setup_after: false,
            choice: "50",
            auto: false,
            other,
            below,
            add,
            limit,
            busy: None,
            top_note: None,
            pays_note: None,
            panel_note: None,
            receipts_note: None,
            stale: None,
            copied: false,
            _watch: None,
            _subscriptions: subscriptions,
        };
        super::super::read(
            cx,
            page.ctx.gateway.query("/v1/tenants/me".into()),
            |this, answer, cx| {
                this.me = match answer {
                    Ok(answer) => Load::Ready(parse_me(&answer)),
                    Err(error) => Load::Failed(error.into()),
                };
                // Only owners read billing: Gateway refuses everyone else.
                if matches!(&this.me, Load::Ready(me) if me.owner) {
                    this.reload(cx);
                }
            },
        );
        page
    }

    pub(super) fn billing(&self) -> Option<&Billing> {
        match &self.billing {
            Load::Ready(Some(billing)) => Some(billing),
            _ => None,
        }
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let gateway = self.ctx.gateway.clone();
        super::super::read(cx, gateway.query("/v1/billing/plans".into()), |this, answer, _| {
            this.offers_plans = answer.ok().is_some_and(|catalogue| {
                catalogue["plans"].as_array().is_some_and(|plans| {
                    plans.iter().any(|plan| {
                        matches!(plan["plan"].as_str(), Some("plus" | "pro" | "teams_plus" | "teams_pro"))
                            && plan["includedHours"].as_u64().is_some_and(|hours| hours > 0)
                    })
                })
            });
        });
        super::super::read(cx, gateway.query("/v1/billing".into()), |this, answer, cx| {
            let first = !matches!(this.billing, Load::Ready(_));
            this.stale = match &answer {
                Err(error) if !first && !billing_off(error) => Some(error.to_string()),
                _ => None,
            };
            this.billing = match answer {
                Ok(answer) => parse_billing(&answer)
                    .map(|billing| Load::Ready(Some(billing)))
                    .unwrap_or_else(|| Load::Failed("Nexus sent billing this app cannot read.".into())),
                Err(error) if billing_off(&error) => Load::Ready(None),
                // A failed read after a good one keeps what shows, under the stale note.
                Err(error) if first => Load::Failed(error.into()),
                Err(_) => {
                    cx.notify();
                    return;
                }
            };
            if first && this.billing().is_some() {
                this.read_receipts(cx);
            }
        });
        if matches!(self.billing, Load::Ready(Some(_))) {
            self.read_receipts(cx);
        }
    }

    pub(super) fn read_receipts(&mut self, cx: &mut Context<Self>) {
        super::super::read(
            cx,
            self.ctx.gateway.query("/v1/billing/receipts".into()),
            |this, answer, _| {
                this.receipts = match answer {
                    Ok(answer) => parse_receipts(&answer)
                        .map(Load::Ready)
                        .unwrap_or_else(|| Load::Failed("Nexus sent receipts this app cannot read.".into())),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
    }

    /// Run Gateway calls one after another; `then` gets the first failure, if any.
    pub(super) fn steps(
        &mut self,
        steps: Vec<(Verb, String, Value)>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Option<String>, &mut Context<Self>) + 'static,
    ) {
        let gateway = self.ctx.gateway.clone();
        cx.spawn(async move |this, cx| {
            let mut failure = None;
            for (verb, path, body) in steps {
                if let Err(error) = gateway.request(verb, path, Some(body)).await {
                    failure = Some(error);
                    break;
                }
            }
            this.update(cx, |this, cx| {
                then(this, failure, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -------------------------------------------------------------- Stripe's pages

    /// Stripe's card page, in the browser. What `deferred` holds turns on once the card reaches Gateway.
    pub(super) fn card_setup(&mut self, deferred: Deferred, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(billing) = self.billing() else {
            return;
        };
        let before = Card::key(billing.card.as_ref());
        self.busy = Some(Busy::Stripe);
        self.top_note = None;
        cx.notify();
        fetch(
            cx,
            self.ctx
                .gateway
                .request(Verb::Post, "/v1/billing/payment-method".into(), Some(json!({}))),
            move |this, answer, cx| {
                this.busy = None;
                match answer.map(|answer| ui::text(&answer, "url").to_string()) {
                    Ok(url) if !url.is_empty() => {
                        crate::external_link::open(&url, cx);
                        this.watch(WaitKind::Card, before, deferred, cx);
                    }
                    Ok(_) => this.top_note = failed("Stripe's page didn't open: Nexus sent no link."),
                    Err(error) => {
                        this.top_note = failed(&format!(
                            "Stripe's page didn't open. Nothing changed. {}",
                            plain(&error)
                        ))
                    }
                }
            },
        );
    }

    /// A charge the bank wants the cardholder to confirm (3-D Secure): Gateway confirms that same charge again on
    /// session, and the bank's page opens in the browser. With no charge to confirm, Stripe's card page instead.
    pub(super) fn confirm_charge(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Stripe);
        self.top_note = None;
        cx.notify();
        fetch(
            cx,
            // The typed call keeps Gateway's error code (`nothing_to_confirm`), not only its words, and the write guards.
            self.ctx
                .gateway
                .request_typed(Verb::Post, "/v1/billing/confirm-payment".into(), Some(json!({}))),
            move |this, answer, cx| {
                this.busy = None;
                match answer {
                    Ok(answer) if ui::text(&answer, "status") == "redirect" && !ui::text(&answer, "url").is_empty() => {
                        crate::external_link::open(ui::text(&answer, "url"), cx);
                        this.top_note = done(
                            "Approve the payment on your bank's page in your browser. It shows here once Stripe confirms it.",
                        );
                    }
                    Ok(answer) => {
                        this.top_note = done(confirmed_note(&answer));
                        this.reload(cx);
                    }
                    Err(error) if nothing_to_confirm(&error) => this.card_setup(Deferred::default(), cx),
                    Err(error) => {
                        this.top_note = failed(&format!(
                            "The bank's page didn't open. Nothing was charged. {}",
                            plain(&error.to_string())
                        ))
                    }
                }
            },
        );
    }

    /// Stripe's customer portal (cards, billing details, tax IDs), in the browser.
    pub(super) fn portal(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Stripe);
        self.top_note = None;
        cx.notify();
        fetch(
            cx,
            self.ctx
                .gateway
                .request(Verb::Post, "/v1/billing/portal".into(), Some(json!({}))),
            |this, answer, cx| {
                this.busy = None;
                match answer.map(|answer| ui::text(&answer, "url").to_string()) {
                    Ok(url) if !url.is_empty() => {
                        crate::external_link::open(&url, cx);
                        this.top_note = done("Stripe's page is open in your browser. Come back here when you're done.");
                    }
                    Ok(_) => this.top_note = failed("Stripe's page didn't open: Nexus sent no link."),
                    Err(error) => {
                        this.top_note = failed(&format!(
                            "Stripe's page didn't open. Nothing changed. {}",
                            plain(&error)
                        ))
                    }
                }
            },
        );
    }

    /// Ask for billing every few seconds until what `kind` watches changes, then turn on what was chosen with it.
    pub(super) fn watch(&mut self, kind: WaitKind, before: String, deferred: Deferred, cx: &mut Context<Self>) {
        self.top_note = done(match kind {
            WaitKind::Card => "Finish on Stripe's page in your browser. The card shows here once Stripe sends it.",
            WaitKind::Credits => {
                "Pay on Stripe's page in your browser. The credits show here once Stripe sends the payment."
            }
        });
        let gateway = self.ctx.gateway.clone();
        let started = Instant::now();
        self._watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let answer = gateway.query("/v1/billing".into()).await;
                let late = started.elapsed() > HANDOFF_WAIT;
                let keep = this.update(cx, |this, cx| {
                    let keep = this.on_watch(kind, &before, &deferred, answer, late, cx);
                    cx.notify();
                    keep
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// One answer while waiting; true to keep waiting.
    pub(super) fn on_watch(
        &mut self,
        kind: WaitKind,
        before: &str,
        deferred: &Deferred,
        answer: Result<Value, String>,
        late: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let billing = answer.ok().and_then(|answer| parse_billing(&answer));
        if let Some(billing) = billing {
            let now = watched_key(kind, &billing);
            let after_on = billing.after.as_ref().is_some_and(|after| after.enabled);
            self.billing = Load::Ready(Some(billing));
            if now != before {
                self.top_note = done(match kind {
                    WaitKind::Card => "The card is saved.",
                    WaitKind::Credits => "Your Nexus credits are added.",
                });
                let steps = deferred.steps(after_on);
                if steps.is_empty() {
                    self.read_receipts(cx);
                } else {
                    self.steps(steps, cx, |this, failure, cx| {
                        if let Some(error) = failure {
                            this.top_note = failed(&format!(
                                "The card is saved, but what you chose with it didn't turn on. Set it again under How it pays. {}",
                                plain(&error)
                            ));
                        }
                        this.reload(cx);
                    });
                }
                self._watch = None;
                return false;
            }
        }
        if late {
            self.top_note = done(match kind {
                WaitKind::Card => {
                    "Stripe hasn't sent the card yet. It shows here within a few minutes; open this page again to check."
                }
                WaitKind::Credits => {
                    "Stripe hasn't sent the payment yet. The credits show here within a few minutes; open this page again to check."
                }
            });
            self._watch = None;
            return false;
        }
        true
    }

    pub(super) fn open_receipt(&mut self, receipt: &Receipt, cx: &mut Context<Self>) {
        if let Some(url) = &receipt.url {
            crate::external_link::open(url, cx);
            return;
        }
        if !receipt.receipt || self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Receipt(receipt.id.clone()));
        self.receipts_note = None;
        cx.notify();
        let path = format!("/v1/billing/receipts/{}", encode(&receipt.id));
        fetch(
            cx,
            self.ctx.gateway.request(Verb::Get, path, None),
            |this, answer, cx| {
                this.busy = None;
                match answer.map(|answer| ui::text(&answer, "url").to_string()) {
                    Ok(url) if !url.is_empty() => crate::external_link::open(&url, cx),
                    Ok(_) => this.receipts_note = failed("Stripe's receipt didn't open: Nexus sent no link."),
                    Err(error) => {
                        this.receipts_note = failed(&format!("Stripe's receipt didn't open. {}", plain(&error)))
                    }
                }
            },
        );
    }
}
