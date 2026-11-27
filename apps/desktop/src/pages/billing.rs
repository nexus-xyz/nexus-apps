//! Settings › Billing (the web app's billing page): the organization's balance,
//! how it pays, its receipts and its payment method. Owners only (`org.billing`). A free account spends its one-time
//! free start, then pays as it goes: prepaid credits (with auto top-up), paying after the fact on the card up to a
//! monthly usage limit, or both. Cards and billing details live on Stripe's pages (Checkout, the customer portal), which
//! open in the browser; the card or the payment reaches Gateway through Stripe's webhook, so the page asks again every
//! few seconds until it shows, as the desktop app's web page does. Plans are dormant on the web too, so none show here.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{Ctx, Load, PageId, Verb, fetch, palette, ui};
use crate::palette::Palette;

#[path = "billing_after.rs"]
mod charge;
#[path = "billing_usage_limit.rs"]
mod usage_limit;
pub(super) use usage_limit::build as build_usage_limit;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| BillingPage::new(ctx, window, cx));
    Some(view.into())
}

/// How often the page asks for billing while Stripe's webhook is on its way.
const POLL: Duration = Duration::from_secs(2);
/// The browser stays open as long as a person takes on Stripe's form: wait that long.
const HANDOFF_WAIT: Duration = Duration::from_secs(10 * 60);
/// The amounts the credit form offers, the same on every surface (kit's `CREDIT_AMOUNTS`); Other takes any amount
/// from the minimum.
const AMOUNTS: [&str; 3] = ["10", "25", "50"];

// ------------------------------------------------------------------ The billing glossary

/// One name for each billing concept, as kit's `BILLING_TERMS`. The tests check these against
/// `kit/src/fixtures/billing-words.json`.
mod words {
    /// The page writes the free start in sentences ("of your free $1 left"); the test still checks the name.
    #[allow(dead_code)]
    pub const FREE_START: &str = "Free start";
    pub const PROMOTIONAL_CREDIT: &str = "Promotional credit";
    pub const CREDITS: &str = "Nexus credits";
    pub const PAY_AFTER_THE_FACT: &str = "Pay after the fact";
    pub const USAGE_LIMIT: &str = "Usage limit";
    pub const CHANGE_LIMIT: &str = "Change limit";
    pub const AUTO_TOPUP: &str = "Auto top-up";
    pub const PAY_AS_YOU_GO: &str = "Pay as you go";
    pub const DATES_IN_UTC: &str = "Billing dates are in UTC.";
    pub const PAY_INVOICE: &str = "Pay invoice";
    pub const UPDATE_CARD: &str = "Update card";
    pub const FIX_PAYMENT: &str = "Fix payment";
    pub const ADD_ANOTHER_CARD: &str = "Add another card";
    pub const CONFIRM_PAYMENT: &str = "Confirm payment";
    pub const SET_UP_BILLING: &str = "Set up billing";
    pub const BUY_CREDITS: &str = "Buy credits";
    /// The action beside a billing error.
    pub const TRY_AGAIN: &str = "Try again";
}

mod actions;
mod model;
mod panel;
mod state;
#[cfg(test)]
mod tests;
mod view;
use model::Deferred;
use model::*;
use state::*;
pub(super) use view::build_locked;
use view::*;
