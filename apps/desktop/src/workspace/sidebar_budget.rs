//! The budget card at the sidebar's foot, as Nexus UI's `SidebarBudget` (`shell.tsx`) draws it: always the person's
//! own figures, never the organization's budget, which owners and admins read in Usage. A member sees what is left for
//! them (`GET /v1/spend`). An owner or an admin sees their own month (`GET /v1/people/me/budget`): what is left of
//! their limit with a meter, or what they spent without one. A click opens Budgets.

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::Workspace;
use super::settings_registry::Dest;
use crate::palette::Palette;
use crate::pay_notice::micros_text;

/// What the card shows, in micro-dollars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Card {
    /// A member: what is left for them, and the day the cap that binds it resets.
    LeftForYou { left: i64, resets: Option<String> },
    /// An owner or an admin: their own month. `limit` and `left` are `None` without a limit of their own.
    Own {
        limit: Option<i64>,
        spent: i64,
        left: Option<i64>,
    },
}

/// A money field in micro-dollars: its decimal twin first, else the number. Money travels as a
/// decimal string, so it stays exact.
fn money(value: &Value, key: &str) -> Option<i64> {
    value[format!("{key}Decimal")]
        .as_str()
        .and_then(micros_text)
        .or_else(|| value[key].as_f64().map(|usd| (usd * 1_000_000.).round() as i64))
}

/// The card from `GET /v1/spend` and, for an owner or an admin, `GET /v1/people/me/budget`. `None` until what it needs
/// is read: an owner's card never falls back to the organization's budget.
pub(super) fn card(spend: &Value, own: Option<&Value>) -> Option<Card> {
    spend.as_object()?;
    let org = !spend["budgetUsd"].is_null() || !spend["budgetUsdDecimal"].is_null();
    if !org {
        return Some(Card::LeftForYou {
            left: money(spend, "remainingUsd").unwrap_or(0).max(0),
            resets: spend["resetsAt"].as_str().map(String::from),
        });
    }
    let own = own?;
    own.as_object()?;
    let limit = money(own, "limitUsd");
    Some(Card::Own {
        limit,
        spent: money(own, "spentUsd").unwrap_or(0),
        left: limit.map(|_| money(own, "remainingUsd").unwrap_or(0).max(0)),
    })
}

/// A whole number grouped by thousands: "1,234".
fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// To the cent, rounded half up: "$9.03", "$1,234.00".
fn cents(micros: i64) -> String {
    let cents = (micros + 5_000).div_euclid(10_000);
    format!("${}.{:02}", count((cents / 100) as u64), cents % 100)
}

/// Whole dollars without cents ("$30"), else as [`cents`]: Nexus UI's `budgetText`.
fn budget_text(micros: i64) -> String {
    let rounded = (micros + 5_000).div_euclid(10_000);
    if rounded % 100 == 0 {
        format!("${}", count((rounded / 100) as u64))
    } else {
        cents(micros)
    }
}

/// "Dec 1": the day an RFC 3339 time falls on, in UTC.
fn short_day(at: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|time| time.with_timezone(&chrono::Utc).format("%b %-d").to_string())
}

/// When person limits reset: the 1st of next month, 00:00 UTC.
fn next_month_start(now: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Datelike as _;
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    format!("{year:04}-{month:02}-01T00:00:00Z")
}

/// The card's words: the figure in bold, the rest of its line, the meter's share left, and the caption.
#[derive(Debug, PartialEq)]
pub(super) struct Words {
    pub figure: String,
    pub rest: String,
    pub left_share: Option<f32>,
    pub caption: String,
}

pub(super) fn words(card: &Card, now: chrono::DateTime<chrono::Utc>) -> Words {
    match card {
        Card::LeftForYou { left, resets } => {
            let name = "Your budget";
            Words {
                figure: budget_text(*left),
                rest: "left for you".into(),
                left_share: None,
                caption: match resets.as_deref().and_then(short_day) {
                    Some(day) => format!("{name} · resets {day}"),
                    None => name.into(),
                },
            }
        }
        Card::Own { limit, spent, left } => {
            let resets = short_day(&next_month_start(now))
                .map(|day| format!(" · resets {day}"))
                .unwrap_or_default();
            match (limit, left) {
                (Some(limit), Some(left)) => Words {
                    figure: cents(*left),
                    rest: format!("left of {}", budget_text(*limit)),
                    left_share: Some(if *limit > 0 {
                        (*left as f64 / *limit as f64).clamp(0., 1.) as f32
                    } else {
                        0.
                    }),
                    caption: format!("Your budget{resets}"),
                },
                _ => Words {
                    figure: cents(*spent),
                    rest: "spent this month".into(),
                    left_share: None,
                    caption: format!("No limit of your own{resets}"),
                },
            }
        }
    }
}

impl Workspace {
    /// The card above the person row; nothing until its figures are read.
    pub(super) fn render_sidebar_budget(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = card(self.spend.as_ref()?, self.own_budget.as_ref())?;
        let words = words(&card, chrono::Utc::now());
        Some(
            div()
                .px(px(8.))
                .pb(px(4.))
                .child(
                    v_flex()
                        .id("sidebar-budget")
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(12.))
                        .bg(palette.hover)
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.selected))
                        .on_click(cx.listener(|this, _, window, cx| this.open_dest(Dest::Budgets, window, cx)))
                        .child(
                            div().truncate().text_size(px(12.)).text_color(palette.muted).child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.foreground)
                                            .child(words.figure),
                                    )
                                    .child(words.rest),
                            ),
                        )
                        .children(words.left_share.map(|share| {
                            div()
                                .w_full()
                                .h(px(3.))
                                .rounded(px(2.))
                                .bg(palette.border)
                                .child(div().h_full().rounded(px(2.)).w(relative(share)).bg(palette.foreground))
                        }))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(words.caption),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    const ORG: &str =
        r#"{"budgetUsd":30,"budgetUsdDecimal":"30.000000","remainingUsd":9.03,"remainingUsdDecimal":"9.030000"}"#;

    #[test]
    fn an_owner_sees_their_own_month_never_the_organizations_budget() {
        let spend: Value = serde_json::from_str(ORG).unwrap();
        let own =
            json!({"limitUsdDecimal": "50.000000", "spentUsdDecimal": "12.500000", "remainingUsdDecimal": "37.500000"});
        let card = card(&spend, Some(&own)).unwrap();
        assert_eq!(
            card,
            Card::Own {
                limit: Some(50_000_000),
                spent: 12_500_000,
                left: Some(37_500_000)
            }
        );
        let said = words(&card, at("2026-11-12T10:00:00Z"));
        assert_eq!(said.figure, "$37.50");
        assert_eq!(said.rest, "left of $50");
        assert_eq!(said.caption, "Your budget · resets Dec 1");
        assert_eq!(said.left_share, Some(0.75));
        assert!(!format!("{said:?}").contains("$30"));
    }

    #[test]
    fn an_owner_without_a_limit_sees_what_they_spent() {
        let spend: Value = serde_json::from_str(ORG).unwrap();
        let own = json!({"limitUsd": null, "spentUsd": 20.97, "remainingUsd": null});
        let card = card(&spend, Some(&own)).unwrap();
        let said = words(&card, at("2026-12-31T23:59:00Z"));
        assert_eq!(
            (said.figure.as_str(), said.rest.as_str()),
            ("$20.97", "spent this month")
        );
        assert_eq!(said.caption, "No limit of your own · resets Jan 1");
        assert_eq!(said.left_share, None);
    }

    #[test]
    fn an_owner_card_waits_for_their_own_month() {
        let spend: Value = serde_json::from_str(ORG).unwrap();
        assert_eq!(card(&spend, None), None);
    }

    #[test]
    fn a_member_sees_what_is_left_for_them() {
        let spend = json!({"remainingUsdDecimal": "1.250000", "resetsAt": "2026-12-01T00:00:00Z", "scope": "own"});
        let card = card(&spend, None).unwrap();
        let said = words(&card, at("2026-11-12T10:00:00Z"));
        assert_eq!((said.figure.as_str(), said.rest.as_str()), ("$1.25", "left for you"));
        assert_eq!(said.caption, "Your budget · resets Dec 1");
    }
}
