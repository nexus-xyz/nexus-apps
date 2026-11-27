//! Usage (`ui/src/app/app/[org]/usage/`, `ui/.../nexus-ui/usage/`): what the organization spent on models, who spent
//! it, and the limits around it. The layout is the desktop redesign: the
//! range (7d, this month, 30d, 90d) and Export beside the title, and five underlined tabs. Overview (one budget panel
//! with its pace and four figures, spend by day stacked by team or model with the month's estimate, the teams with
//! their caps, top movers and the models), Breakdown (by team, person, model or product, with a team's child teams and
//! people), Calls (the organization's calls with filters, and one call's detail beside them), Adoption (active engineers
//! by day) and Limits (the budget, the default and the team caps as settings rows, everyone's limit, and who may change
//! limits).
//!
//! Export saves the tab's rows as Gateway's CSV or JSON Lines file (`usage_export.rs`), and Overview shows each team
//! against its cap (`usage_caps.rs`). The figures, money and dates this page reads are shared with Overview
//! (`overview.rs`) as `pub(super)` helpers.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{Ctx, Load, Verb, fetch, palette, ui};
use super::{billing, download, read, usage_caps, usage_export};
use crate::library::chrono_lite;
use crate::palette::Palette;

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .gap(px(12.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(head::range_buttons(palette, None))
                    .child(page::export_button(false, false, palette, None)),
            )
            .child(
                parts::tab_row(palette).flex_wrap().gap(px(8.)).children(
                    TABS.iter()
                        .map(|(tab, label)| page::usage_tab(*tab, label, *tab == Tab::Limits, palette, None)),
                ),
            )
            .child(ui::group_head("Limits", palette))
            .child(limits::locked_rules(palette))
            .into_any_element()
    })
}

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| UsagePage::new(ctx, window, cx));
    Some(view.into())
}

/* ------------------------------------------------------------------ shared helpers */

mod breakdown;
mod calls;
mod format;
mod head;
mod limits;
mod model;
mod overview;
mod page;
mod parts;
#[cfg(test)]
mod tests;

use self::format::{
    NUM_W, calls_filter, cell, cents_signed, compact, day_number, error_text, is_refusal, loading_or_failed, long_date,
    members_of, number, pricing_of, refusal_text, right, sparkline, text_of,
};
pub(super) use self::format::{
    amount, budget_text, card_head, cents, encode, hue, iso_day, kpi, load_of, meter, month_name, next_month, percent,
    query, share, short_date, today,
};
#[cfg(test)]
use self::model::span;
use self::model::{
    Adoption, Budgets, By, Call, CallFilters, CallPage, Edit, Group, LimitFilter, LimitState, Menu, Person, Range,
    STATUSES, Span, Stack, TABS, Tab, axis, budget_pace, labeled_days, model_name, movers, parse_usd, ratio_text,
    rest_of_month, stack_of, usd_decimal,
};
pub(super) use self::model::{Me, Report, Spend, short_model};
use self::page::UsagePage;

#[cfg(test)]
#[path = "usage_budgets_tests.rs"]
mod budgets_tests;
