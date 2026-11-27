//! The Budgets card reuses Billing's wire parser, dollar validation and language. Payment setup stays in Billing.
use super::super::{read, usage::Me};
use super::{
    Billing, Ctx, Load, PageId, Verb, billing_off, cents, fetch, limit_problem, limit_resets, money, palette,
    parse_billing, plain_number, today, ui, usd_input,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;
use std::{cell::Cell, rc::Rc};

pub(crate) fn build(ctx: Ctx, mutation: Rc<Cell<bool>>, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| UsageLimit::new(ctx, mutation, window, cx)).into()
}
struct UsageLimit {
    ctx: Ctx,
    marker: Option<String>,
    mutation: Rc<Cell<bool>>,
    me: Load<Me>,
    billing: Load<Option<Billing>>,
    epoch: u64,
    account_epoch: u64,
    editing: bool,
    busy: bool,
    amount: Entity<InputState>,
    error: Option<SharedString>,
    _subscription: Subscription,
}
impl UsageLimit {
    fn new(ctx: Ctx, mutation: Rc<Cell<bool>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let amount = cx.new(|cx| InputState::new(window, cx).placeholder("Limit a month"));
        let subscription = cx.subscribe_in(&amount, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save(cx);
            }
        });
        let mut page = Self {
            marker: ctx.gateway.marker(),
            ctx,
            mutation,
            me: Load::Loading,
            billing: Load::Loading,
            epoch: 0,
            account_epoch: 0,
            editing: false,
            busy: false,
            amount,
            error: None,
            _subscription: subscription,
        };
        page.load_account(cx);
        page
    }
    fn current(&self) -> bool {
        self.marker.is_some() && self.marker == self.ctx.gateway.marker()
    }
    fn allowed(&self) -> bool {
        self.current() && matches!(&self.me, Load::Ready(me) if me.can("org.billing"))
    }
    fn load_account(&mut self, cx: &mut Context<Self>) {
        self.account_epoch += 1;
        let epoch = self.account_epoch;
        self.me = Load::Loading;
        read(
            cx,
            self.ctx.gateway.query("/v1/tenants/me".into()),
            move |this, answer, cx| {
                if !this.current() || this.account_epoch != epoch {
                    return;
                }
                this.me = super::super::usage::load_of(answer, Me::parse);
                if this.allowed() {
                    this.reload(cx);
                }
            },
        );
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.allowed() || self.busy {
            return;
        }
        self.epoch += 1;
        let epoch = self.epoch;
        self.billing = Load::Loading;
        read(
            cx,
            self.ctx.gateway.query("/v1/billing".into()),
            move |this, answer, _| {
                if !this.allowed() || this.epoch != epoch {
                    return;
                }
                this.billing = match answer {
                    Ok(value) => parse_billing(&value)
                        .map(|value| Load::Ready(Some(value)))
                        .unwrap_or_else(|| Load::Failed("Nexus sent billing this app cannot read.".into())),
                    Err(error) if billing_off(&error) => Load::Ready(None),
                    Err(error) => Load::Failed(error.into()),
                };
            },
        );
    }
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.allowed() || self.mutation.get() {
            return;
        }
        let Load::Ready(Some(billing)) = &self.billing else {
            return;
        };
        // A limit changes on or off (Nexus UI's `BillingUsageLimit`); with no setting yet, Billing turns it on first.
        let Some(after) = billing.after.as_ref() else {
            return;
        };
        let text = plain_number(&after.limit);
        self.amount.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        self.editing = true;
        self.error = None;
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.allowed() || !self.editing || self.mutation.get() {
            return;
        }
        let Load::Ready(Some(billing)) = &self.billing else {
            return;
        };
        let Some(after) = billing.after.as_ref() else {
            return;
        };
        let limit = usd_input(&self.amount.read(cx).value());
        if let Some(problem) = limit_problem(&limit, billing.pricing.max_limit.as_deref()) {
            self.error = Some(problem.into());
            cx.notify();
            return;
        }
        // The limit alone changes: the setting keeps its value, so saving never turns paying after the fact on.
        let body = json!({"enabled": after.enabled, "limitUsd": limit});
        self.epoch += 1;
        self.mutation.set(true);
        self.busy = true;
        self.error = None;
        fetch(
            cx,
            self.ctx
                .gateway
                .request(Verb::Put, "/v1/billing/after-the-fact".into(), Some(body)),
            |this, answer, cx| {
                if !this.current() {
                    return;
                }
                this.busy = false;
                this.mutation.set(false);
                match answer {
                    Ok(_) => {
                        this.editing = false;
                        this.reload(cx);
                        // A refused answer that waited on the usage limit is asked again (`limit_stop.rs`).
                        crate::limit_stop::raised(cx);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
            },
        );
        cx.notify();
    }
}
impl Render for UsageLimit {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        if !self.current() {
            return v_flex();
        }
        let mut card = ui::card(palette)
            .child(ui::label("Usage limit", palette))
            .child(ui::note("The most your card pays in a month.", palette));
        let retry = |cx: &mut Context<Self>| {
            ui::button("budget-card-limit-retry", "Retry", palette).on_click(cx.listener(|this, _, _, cx| {
                if matches!(this.me, Load::Failed(_)) {
                    this.load_account(cx);
                } else {
                    this.reload(cx);
                }
            }))
        };
        match (&self.me, &self.billing) {
            (Load::Failed(error), _) | (_, Load::Failed(error)) => {
                return v_flex().child(card.child(ui::error_line(error.clone(), palette)).child(retry(cx)));
            }
            (Load::Ready(_), _) if !self.allowed() => return v_flex(),
            (_, Load::Ready(None)) => {
                return v_flex().child(card.child(ui::note("Billing is not enabled on this Gateway.", palette)));
            }
            (_, Load::Ready(Some(_))) => {}
            _ => return v_flex().child(card.child(ui::skeleton(2, palette))),
        }
        let Load::Ready(Some(billing)) = &self.billing else {
            return v_flex();
        };
        let after = billing.after.as_ref();
        let limit = after
            .map(|after| &after.limit)
            .unwrap_or(&billing.pricing.default_limit);
        card = card.child(ui::label(format!("{} a month", money(limit)), palette));
        if let Some(after) = after.filter(|after| after.enabled) {
            card = card.child(super::super::usage::meter(
                400.,
                super::number(&after.month_spent) / super::number(&after.limit).max(0.01),
                0.,
                palette.strong,
                palette,
            ));
            card = card.child(ui::note(
                format!("{} used · {}", cents(&after.month_spent), limit_resets(today())),
                palette,
            ));
        } else if after.is_some() {
            card = card.child(ui::note("Pay after the fact is off.", palette));
        }
        if after.is_some() {
            if !self.editing {
                card = card.child(
                    ui::button("budget-card-limit-change", "Change limit", palette)
                        .on_click(cx.listener(|this, _, window, cx| this.edit(window, cx))),
                );
            }
        } else {
            let navigate = self.ctx.navigate.clone();
            card = card.child(ui::note("Pay after the fact is off.", palette)).child(
                ui::button("budget-payment-settings", "Payment settings", palette)
                    .on_click(move |_, window, cx| navigate(PageId::Billing, window, cx)),
            );
        }
        if self.editing {
            card = card
                .child(ui::note(
                    "Above the limit, calls your card pays pause until the limit resets or goes up.",
                    palette,
                ))
                .child(Input::new(&self.amount))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(if self.busy {
                            ui::disabled_button("budget-card-limit-save", "Saving…", palette)
                        } else {
                            ui::primary_button("budget-card-limit-save", "Save limit", palette)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                        })
                        .child(
                            ui::button("budget-card-limit-cancel", "Cancel", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    if !this.busy {
                                        this.editing = false;
                                        this.error = None;
                                        cx.notify();
                                    }
                                },
                            )),
                        ),
                );
        }
        if let Some(error) = &self.error {
            card = card.child(ui::error_line(error.clone(), palette));
        }
        v_flex().child(card)
    }
}

#[cfg(test)]
#[path = "billing_usage_limit_tests.rs"]
mod tests;
