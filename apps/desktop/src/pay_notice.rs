//! What the chat composer says about paying, before a send is refused: Nexus UI's `plan-notice.tsx` and
//! `@nexus/kit`'s `payNoticeOf`, in the same words. `GET /v1/spend`'s `nexusPays` says whether Nexus can pay the next
//! call and why not; while the free start pays and its end is near, one quiet line says so. Brand v5's billing voice:
//! what stops, until when, and what keeps working. It never blocks sending: a call on your own key still runs.

use chrono::DateTime;
use serde_json::Value;

const DAY_MS: i64 = 86_400_000;
/// The line says when the free start ends from this many days before.
const EXPIRY_NOTICE_DAYS: i64 = 7;
/// The line says how much of the month's usage limit the card paid from this share, in percent (kit's).
const LIMIT_NEAR_PERCENT: i64 = 80;
/// The billing glossary's name for the card's monthly limit (kit's `BILLING_TERMS.usageLimit`).
const USAGE_LIMIT: &str = "Usage limit";
const PAUSED: &str = "Models Nexus runs are paused";
/// The composer notice's actions, as kit's `BILLING_TERMS` names them (`PAY_NOTICE_ACTION`). The tests check
/// each reason's title and action against `kit/src/fixtures/billing-words.json`.
const SET_UP_BILLING: &str = "Set up billing";
const BUY_CREDITS: &str = "Buy credits";
const CHANGE_LIMIT: &str = "Change limit";
const FIX_PAYMENT: &str = "Fix payment";
const CONFIRM_PAYMENT: &str = "Confirm payment";
const ADD_ANOTHER_CARD: &str = "Add another card";
const PAY_INVOICE: &str = "Pay invoice";
const OPEN_BILLING: &str = "Open billing";
/// The model menu's line while the free start pays: which models it pays for (Nexus UI's `FREE_START_NOTE`).
pub(crate) const FREE_START_NOTE: &str =
    "Your free start pays for Auto and the models marked Free start. Other models need pay as you go.";
const KEEPS: &str = "Your own keys keep working. Nothing is lost.";
/// A charge for usage that is still going through, in the billing glossary's words (kit's
/// `CHARGE_PROCESSING`, checked against `kit/src/fixtures/billing-words.json`).
const CHARGING_TITLE: &str = "A payment is still going through";
const CHARGING_BODY: &str = "Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work.";

/// Above the chat composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PayNotice {
    /// Nexus cannot pay the next call: a card with a title, a body and one action that opens Billing.
    Refused {
        title: String,
        body: String,
        action: &'static str,
    },
    /// The free start pays and its end is near: "Free $1 · 85% used · expires in 3 days". Or the card pays and has
    /// paid 80% of this month's usage limit: "Usage limit · 85% used · resets on 1 Dec, 00:00 UTC".
    Line(String),
}

/// A decimal dollar string as micro-dollars (`"0.400000"` is 400 000); `None` for anything else.
fn micros(value: &Value) -> Option<i64> {
    let text = value.as_str()?.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let fraction: i64 = format!("{fraction:0<6}").parse().ok()?;
    whole.checked_mul(1_000_000)?.checked_add(fraction)
}

/// Whole dollars without cents (`"$1"`), else dollars and cents (`"$1.50"`): Nexus UI's `budgetText`.
fn dollars(micros: i64) -> String {
    let cents = (micros + 5_000) / 10_000;
    if cents % 100 == 0 {
        format!("${}", cents / 100)
    } else {
        format!("${}.{:02}", cents / 100, cents % 100)
    }
}

/// When a block lifts or the free start ends, as the billing glossary writes a date that decides something:
/// "1 Dec, 00:00 UTC" (kit's `billingCutoff`).
fn day(at: &str) -> Option<String> {
    Some(
        DateTime::parse_from_rfc3339(at)
            .ok()?
            .to_utc()
            .format("%-d %b, %H:%M UTC")
            .to_string(),
    )
}

/// The first instant of the UTC month after `now_ms`, when the usage limit resets (kit's `usageLimitResetsAt`).
fn next_month(now_ms: i64) -> Option<String> {
    use chrono::Datelike;
    let now = DateTime::from_timestamp_millis(now_ms)?;
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    Some(format!("{year:04}-{month:02}-01T00:00:00Z"))
}

fn ms(at: &str) -> Option<i64> {
    Some(DateTime::parse_from_rfc3339(at).ok()?.timestamp_millis())
}

/// Whether the free start pays the next call (`nexusPays.payer`): the model menu marks the models it pays for then.
pub(crate) fn free_start_pays(spend: &Value) -> bool {
    spend["nexusPays"]["payer"].as_str() == Some("free_start")
}

/// The free start's grant, from `subsidy.grantedUsdDecimal`; `None` without one.
fn granted(spend: &Value) -> Option<i64> {
    micros(&spend["subsidy"]["grantedUsdDecimal"]).filter(|granted| *granted > 0)
}

/// What the composer says at `now_ms`, from `GET /v1/spend`; nothing while Nexus pays and nothing is near its end, or
/// from a Gateway that does not send `nexusPays`.
pub(crate) fn notice(spend: &Value, now_ms: i64) -> Option<PayNotice> {
    let pays = spend.get("nexusPays")?;
    if pays["pays"].as_bool() == Some(false) {
        let reason = pays["reason"].as_str()?;
        return Some(refused(reason, granted(spend), pays["changesAt"].as_str()));
    }
    // The card pays: from 80% of this month's usage limit, one quiet line says how much, and when it resets.
    if pays["payer"].as_str() == Some("card") {
        let used = pays["limitUsedPercent"]
            .as_i64()
            .filter(|used| *used >= LIMIT_NEAR_PERCENT)?;
        return Some(PayNotice::Line(format!(
            "{USAGE_LIMIT} · {used}% used · resets on {}",
            day(&next_month(now_ms)?)?
        )));
    }
    if !free_start_pays(spend) {
        return None;
    }
    let granted = granted(spend)?;
    let left = micros(&spend["subsidy"]["remainingUsdDecimal"])
        .unwrap_or(0)
        .clamp(0, granted);
    let used = granted - left;
    let mut parts = vec![format!("Free {}", dollars(granted))];
    let mut near = false;
    if used * 5 >= granted * 4 {
        parts.push(format!("{}% used", used * 100 / granted));
        near = true;
    }
    if let Some(ends) = pays["changesAt"].as_str().and_then(ms)
        && ends - now_ms <= EXPIRY_NOTICE_DAYS * DAY_MS
    {
        let days = ((ends - now_ms).max(0) + DAY_MS - 1) / DAY_MS;
        parts.push(match days {
            0 => "expires today".to_string(),
            1 => "expires in 1 day".to_string(),
            days => format!("expires in {days} days"),
        });
        near = true;
    }
    near.then(|| PayNotice::Line(parts.join(" · ")))
}

/// The share the quiet line shows on a meter, in percent from 0 to 100: of this month's usage limit while the card pays,
/// or of the free start while it pays; `None` when the line says no share (kit's `payLineParts`' `usedPercent`).
pub(crate) fn line_share(spend: &Value) -> Option<i64> {
    let pays = spend.get("nexusPays")?;
    if pays["pays"].as_bool() == Some(false) {
        return None;
    }
    if pays["payer"].as_str() == Some("card") {
        return pays["limitUsedPercent"]
            .as_i64()
            .filter(|used| *used >= LIMIT_NEAR_PERCENT)
            .map(|used| used.min(100));
    }
    if !free_start_pays(spend) {
        return None;
    }
    let granted = granted(spend)?;
    let left = micros(&spend["subsidy"]["remainingUsdDecimal"])
        .unwrap_or(0)
        .clamp(0, granted);
    let used = granted - left;
    (used * 5 >= granted * 4).then(|| used * 100 / granted)
}

/// One notice per reason: `@nexus/kit`'s `payNoticeTitle`, and Nexus UI's `payNoticeWords` and `payNoticeAction`.
fn refused(reason: &str, granted: Option<i64>, changes_at: Option<&str>) -> PayNotice {
    let free = granted.map_or_else(
        || "free start".to_string(),
        |granted| format!("free {}", dollars(granted)),
    );
    let (title, body, action) = match reason {
        "free_start_used_up" => (
            format!("Your {free} is used up"),
            format!("{PAUSED}. {SET_UP_BILLING} to keep going. {KEEPS}"),
            SET_UP_BILLING,
        ),
        "free_start_expired" => (
            format!("Your {free} expired"),
            format!("{PAUSED}. {SET_UP_BILLING} to keep going. {KEEPS}"),
            SET_UP_BILLING,
        ),
        "credits_out" => (
            // The billing glossary's title (kit's `usedUpTitle`), as on the Billing page and in every app.
            "Your Nexus credits are used up".to_string(),
            format!("{PAUSED}. {BUY_CREDITS}, or turn on auto top-up or paying after the fact. {KEEPS}"),
            BUY_CREDITS,
        ),
        "usage_limit_reached" => {
            let until = match changes_at.and_then(day) {
                Some(day) => format!(" until {day}, or until an owner raises the limit"),
                None => " until an owner raises the limit".to_string(),
            };
            (
                "This month's usage limit is used up".to_string(),
                format!("{PAUSED}{until}. {KEEPS}"),
                CHANGE_LIMIT,
            )
        }
        "charge_failed" => (
            "A payment didn't go through".to_string(),
            format!("{PAUSED} until a payment goes through. {KEEPS}"),
            FIX_PAYMENT,
        ),
        "card_needs_confirming" => (
            "The bank wants you to confirm a payment".to_string(),
            format!("{PAUSED} until you confirm it. Nothing is charged until then. {KEEPS}"),
            CONFIRM_PAYMENT,
        ),
        "card_disputed" => (
            "A payment on the card was disputed".to_string(),
            format!("{PAUSED} until an owner adds another card. {KEEPS}"),
            ADD_ANOTHER_CARD,
        ),
        "past_due" => (
            "A payment is past due".to_string(),
            format!("{PAUSED} until it's paid. {KEEPS}"),
            PAY_INVOICE,
        ),
        "billing_off" => (
            "Pay as you go isn't set up".to_string(),
            format!("{SET_UP_BILLING} to use models Nexus runs. {KEEPS}"),
            SET_UP_BILLING,
        ),
        "charge_processing" => (CHARGING_TITLE.to_string(), CHARGING_BODY.to_string(), BUY_CREDITS),
        // A reason this build does not know yet: say what stops, and open Billing (kit's `PAY_NOTICE_FALLBACK_TITLE`).
        _ => (
            "Nexus can't pay for model calls now".to_string(),
            format!("{PAUSED}. {KEEPS}"),
            OPEN_BILLING,
        ),
    };
    PayNotice::Refused { title, body, action }
}

/// A chat refusal in plain words: Gateway's billing codes say what stops and what keeps working, never
/// the CLI's commands; anything else as Gateway said it. A usage-limit refusal names what the call may cost and what is
/// left, as Nexus UI does: Gateway refuses a call that MAY cost more than what is left.
pub(crate) fn refusal(error: &nexus_client::Error) -> String {
    let nexus_client::Error::Core(core) = error else {
        return error.to_string();
    };
    let details = core.details.as_ref();
    let detail = |key: &str| details.and_then(|details| details.get(key)).and_then(micros);
    // A budget's refusal names its level and when calls resume: the chat shows it as a limit's setup line.
    if core.code.as_deref() == Some("budget_exceeded")
        && let Some(words) = crate::limit_stop::budget_refusal(details)
    {
        return words;
    }
    match core.code.as_deref() {
        Some("usage_limit_reached") => match (detail("requestedUsd"), detail("remainingUsd")) {
            (_, Some(0)) => format!("Nothing is left of this month's usage limit. {KEEPS}"),
            (Some(cost), Some(left)) => format!(
                "This call may cost up to {}, and {} of this month's usage limit is left. {KEEPS}",
                up_to_cent(cost),
                down_to_cent(left)
            ),
            _ => format!("This month's usage limit doesn't cover this call. {KEEPS}"),
        },
        Some("subsidy_exhausted") => format!("Your free start doesn't cover this call. {KEEPS}"),
        // The chat shows it as a setup line that adds the key (`missing_key`), never as a red error.
        Some("provider_key_required") => match provider_of(&core.message) {
            Some(provider) => format!("{NO_KEY_HEAD}{provider}{NO_KEY_TAIL}"),
            None => error.to_string(),
        },
        Some(code) => {
            let reason = details
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str);
            let Some(title) = refusal_title(code, reason) else {
                return error.to_string();
            };
            // What stops and the owner's way on, after the title every app shows for the same refusal.
            let next = match (code, reason) {
                ("credit_exhausted", _) => KEEPS.to_string(),
                // A dispute needs another card, not a fix to this one.
                ("payment_failed", Some("disputed")) => {
                    format!("{PAUSED} until an owner adds another card. {KEEPS}")
                }
                // 3-D Secure: the composer's notice offers the same action.
                ("payment_failed", Some("authentication_required")) => format!(
                    "{PAUSED} until an owner confirms it: {CONFIRM_PAYMENT} under Billing. Nothing is charged twice. {KEEPS}"
                ),
                ("payment_failed", _) => format!("{PAUSED} until one does. {KEEPS}"),
                ("payment_past_due", _) => format!("{PAUSED} until it's paid. {KEEPS}"),
                ("charge_processing", _) => CHARGING_BODY.to_string(),
                _ => return error.to_string(),
            };
            format!("{title}. {next}")
        }
        None => error.to_string(),
    }
}

/// A missing-key refusal's words (`provider_key_required`): "No Anthropic key for this organization yet."
const NO_KEY_HEAD: &str = "No ";
const NO_KEY_TAIL: &str = " key for this organization yet.";

/// The provider Gateway names in a missing-key refusal: "no Anthropic key for this organization: …", or "…: model
/// calls need your own Anthropic key; …" (`nexus-routing`'s funding).
fn provider_of(message: &str) -> Option<&str> {
    let after = message
        .split_once("your own ")
        .map(|(_, rest)| rest)
        .or_else(|| message.strip_prefix("no "))?;
    let (label, _) = after.split_once(" key")?;
    (!label.is_empty() && label.len() <= 40 && !label.contains([':', ';'])).then_some(label)
}

/// The provider a chat refusal says has no key, for the setup line that adds it; `None` for any other refusal.
pub(crate) fn missing_key(refusal: &str) -> Option<&str> {
    refusal
        .strip_prefix(NO_KEY_HEAD)?
        .strip_suffix(NO_KEY_TAIL)
        .filter(|provider| !provider.is_empty() && !provider.contains('.'))
}

/// The title of a 402 refusal of a call Nexus pays, from its code and `details.reason`, as every app says it (kit's
/// `paymentRefusalTitle`); `None` for a code with words of its own. The tests check it against the glossary's
/// `refusals`.
fn refusal_title(code: &str, reason: Option<&str>) -> Option<&'static str> {
    Some(match (code, reason) {
        ("credit_exhausted", _) => "Your Nexus credits don't cover this call",
        ("payment_failed", Some("disputed")) => "A payment on the card was disputed",
        ("payment_failed", Some("authentication_required")) => "The bank wants you to confirm a payment",
        ("payment_failed", _) => "A payment didn't go through",
        ("payment_past_due", _) => "A payment is past due",
        ("charge_processing", _) => CHARGING_TITLE,
        _ => return None,
    })
}

/// A decimal dollar string as micro-dollars, for another page's words.
pub(crate) fn micros_text(text: &str) -> Option<i64> {
    micros(&Value::String(text.into()))
}

/// A cost to the cent, rounded up.
pub(crate) fn up_to_cent(micros: i64) -> String {
    let cents = (micros + 9_999) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// What is left to the cent, rounded down; under a cent says so.
pub(crate) fn down_to_cent(micros: i64) -> String {
    if micros > 0 && micros < 10_000 {
        return "under $0.01".into();
    }
    let cents = micros / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_794_312_000_000; // 2026-11-10T12:00:00Z

    /// The dates are the billing glossary's (`kit/src/fixtures/billing-words.json`).
    #[test]
    fn dates_match_the_kit_glossary() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        for row in fixture["days"].as_array().expect("days") {
            let iso = row["iso"].as_str().expect("iso");
            assert_eq!(day(iso).as_deref(), row["cutoff"].as_str(), "{iso}");
        }
        assert_eq!(fixture["terms"]["usageLimit"].as_str(), Some(USAGE_LIMIT));
    }

    fn spend(pays: Value, left: &str) -> Value {
        json!({ "nexusPays": pays, "subsidy": { "grantedUsdDecimal": "2.000000", "remainingUsdDecimal": left } })
    }

    #[test]
    fn each_reason_has_its_own_title_and_the_amount_comes_from_gateway() {
        let expired = notice(
            &spend(
                json!({ "pays": false, "payer": null, "reason": "free_start_expired", "changesAt": null }),
                "0",
            ),
            NOW,
        );
        let Some(PayNotice::Refused { title, body, action }) = expired else {
            panic!("{expired:?}")
        };
        assert_eq!(title, "Your free $2 expired");
        assert_eq!(
            body,
            "Models Nexus runs are paused. Set up billing to keep going. Your own keys keep working. Nothing is lost."
        );
        assert_eq!(action, "Set up billing");
        let reasons = [
            "free_start_used_up",
            "free_start_expired",
            "credits_out",
            "usage_limit_reached",
            "charge_failed",
            "card_needs_confirming",
            "card_disputed",
            "past_due",
            "billing_off",
        ];
        let titles: std::collections::HashSet<String> = reasons
            .iter()
            .map(|reason| match refused(reason, None, None) {
                PayNotice::Refused { title, .. } => title,
                PayNotice::Line(_) => unreachable!(),
            })
            .collect();
        assert_eq!(titles.len(), reasons.len());
    }

    /// Each reason's title and action are the glossary's (`payNotices`), as on the web and the phone.
    #[test]
    fn each_reason_says_the_glossary_title_and_action() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        let notices = fixture["payNotices"].as_object().expect("payNotices");
        assert_eq!(notices.len(), 10);
        for (reason, words) in notices {
            let PayNotice::Refused { title, action, .. } = refused(reason, None, None) else {
                unreachable!()
            };
            assert_eq!(words["title"].as_str(), Some(title.as_str()), "{reason}");
            assert_eq!(words["action"].as_str(), Some(action), "{reason}");
            assert_eq!(
                fixture["terms"]
                    .as_object()
                    .expect("terms")
                    .values()
                    .filter(|term| term.as_str() == Some(action))
                    .count(),
                1,
                "{reason}"
            );
        }
    }

    #[test]
    fn the_usage_limit_says_until_when() {
        let PayNotice::Refused { body, .. } = refused("usage_limit_reached", None, Some("2026-12-01T00:00:00.000Z"))
        else {
            unreachable!()
        };
        assert_eq!(
            body,
            "Models Nexus runs are paused until 1 Dec, 00:00 UTC, or until an owner raises the limit. Your own keys keep working. Nothing is lost."
        );
    }

    /// A charge still going through says the glossary's words, in the composer and in a refused answer.
    #[test]
    fn a_payment_still_going_through_says_the_glossary_words() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        assert_eq!(fixture["chargeProcessing"]["title"], CHARGING_TITLE);
        assert_eq!(fixture["chargeProcessing"]["body"], CHARGING_BODY);
        assert_eq!(
            refused("charge_processing", None, None),
            PayNotice::Refused {
                title: "A payment is still going through".into(),
                body: "Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work.".into(),
                action: "Buy credits",
            }
        );
        let error = nexus_client::Error::from(nexus_client::error::CoreError {
            status: 402,
            message: "a charge of $10.80 for usage is still going through".into(),
            code: Some("charge_processing".into()),
            interval: None,
            details: None,
        });
        assert_eq!(
            refusal(&error),
            "A payment is still going through. Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work."
        );
    }

    #[test]
    fn a_quiet_line_once_the_card_paid_80_percent_of_the_months_usage_limit() {
        let card = |percent: Value| json!({ "nexusPays": { "pays": true, "payer": "card", "reason": null, "changesAt": null, "limitUsedPercent": percent } });
        assert_eq!(
            notice(&card(json!(85)), NOW),
            Some(PayNotice::Line(
                "Usage limit · 85% used · resets on 1 Dec, 00:00 UTC".into()
            ))
        );
        assert_eq!(
            notice(&card(json!(80)), NOW),
            Some(PayNotice::Line(
                "Usage limit · 80% used · resets on 1 Dec, 00:00 UTC".into()
            ))
        );
        assert_eq!(notice(&card(json!(79)), NOW), None);
        assert_eq!(notice(&card(Value::Null), NOW), None);
        // December resets on the first of January.
        let december = 1_796_904_000_000; // 2026-12-10T12:00:00Z
        assert_eq!(
            notice(&card(json!(90)), december),
            Some(PayNotice::Line(
                "Usage limit · 90% used · resets on 1 Jan, 00:00 UTC".into()
            ))
        );
    }

    #[test]
    fn the_line_shows_its_share_on_a_meter_only_when_it_says_one() {
        let card =
            |percent: i64| json!({ "nexusPays": { "pays": true, "payer": "card", "limitUsedPercent": percent } });
        assert_eq!(line_share(&card(85)), Some(85));
        assert_eq!(line_share(&card(79)), None);
        assert_eq!(line_share(&card(140)), Some(100));
        let free = |left: &str| spend(json!({ "pays": true, "payer": "free_start", "changesAt": null }), left);
        assert_eq!(line_share(&free("0.300000")), Some(85));
        assert_eq!(line_share(&free("1.000000")), None, "it only ends soon");
        assert_eq!(
            line_share(&json!({ "nexusPays": { "pays": false, "reason": "credits_out" } })),
            None
        );
    }

    #[test]
    fn a_quiet_line_from_80_percent_used_or_a_week_before_the_end() {
        let soon = "2026-11-13T11:59:59.000Z";
        let pays = json!({ "pays": true, "payer": "free_start", "reason": null, "changesAt": soon });
        assert_eq!(
            notice(&spend(pays.clone(), "0.300000"), NOW),
            Some(PayNotice::Line("Free $2 · 85% used · expires in 3 days".into()))
        );
        assert_eq!(
            notice(&spend(pays, "1.000000"), NOW),
            Some(PayNotice::Line("Free $2 · expires in 3 days".into()))
        );
        let later =
            json!({ "pays": true, "payer": "free_start", "reason": null, "changesAt": "2026-11-30T00:00:00.000Z" });
        assert_eq!(notice(&spend(later, "1.000000"), NOW), None);
        let credits = json!({ "pays": true, "payer": "credits", "reason": null, "changesAt": null });
        assert_eq!(notice(&spend(credits, "0.100000"), NOW), None);
        // An older Gateway says nothing.
        assert_eq!(notice(&json!({ "subsidy": {} }), NOW), None);
    }

    #[test]
    fn a_usage_limit_refusal_never_says_used_up_while_money_is_left() {
        let error = |remaining: &str| {
            nexus_client::Error::from(nexus_client::error::CoreError {
                status: 402,
                message: "this call may cost up to $0.35 …".into(),
                code: Some("usage_limit_reached".into()),
                interval: None,
                details: Some(json!({ "requestedUsd": "0.345001", "remainingUsd": remaining })),
            })
        };
        assert_eq!(
            refusal(&error("0.200000")),
            "This call may cost up to $0.35, and $0.20 of this month's usage limit is left. Your own keys keep working. Nothing is lost."
        );
        assert!(refusal(&error("0.000000")).starts_with("Nothing is left of this month's usage limit."));
    }

    /// A missing key reads as the setup line's words, with Gateway's provider; limits and billing never do.
    #[test]
    fn a_missing_key_names_its_provider_for_the_setup_line() {
        let refused = |message: &str, code: &str| {
            refusal(&nexus_client::Error::from(nexus_client::error::CoreError {
                status: 403,
                message: message.into(),
                code: Some(code.into()),
                interval: None,
                details: None,
            }))
        };
        let plain = refused(
            "no Anthropic key for this organization: an owner or admin adds one with `nexus keys add anthropic <key>`",
            "provider_key_required",
        );
        assert_eq!(plain, "No Anthropic key for this organization yet.");
        assert_eq!(missing_key(&plain), Some("Anthropic"));
        let credit = refused(
            "this organization's Nexus credit is used up: model calls need your own OpenAI key; an owner or admin adds one with `nexus keys add openai <key>`",
            "provider_key_required",
        );
        assert_eq!(missing_key(&credit), Some("OpenAI"));
        // Words Gateway did not write this way stay as it said them.
        assert_eq!(missing_key(&refused("refused", "provider_key_required")), None);
        assert_eq!(missing_key(&refused("over the limit", "usage_limit_reached")), None);
    }

    /// A dispute needs another card, so it is not said as a failed charge.
    #[test]
    fn a_disputed_payment_is_not_a_failed_charge() {
        let failed = |reason: &str| {
            nexus_client::Error::from(nexus_client::error::CoreError {
                status: 402,
                message: "calls the card pays stopped".into(),
                code: Some("payment_failed".into()),
                interval: None,
                details: Some(json!({ "reason": reason })),
            })
        };
        assert_eq!(
            refusal(&failed("disputed")),
            "A payment on the card was disputed. Models Nexus runs are paused until an owner adds another card. Your own keys keep working. Nothing is lost."
        );
        assert!(refusal(&failed("declined")).starts_with("A payment didn't go through."));
    }

    /// 3-D Secure asks to confirm, with the composer notice's action, not "A payment didn't go through".
    #[test]
    fn a_payment_the_bank_wants_confirmed_asks_to_confirm_it() {
        let error = nexus_client::Error::from(nexus_client::error::CoreError {
            status: 402,
            message: "calls the card pays stopped".into(),
            code: Some("payment_failed".into()),
            interval: None,
            details: Some(json!({ "reason": "authentication_required", "unpaidUsd": "12.000000" })),
        });
        assert_eq!(
            refusal(&error),
            "The bank wants you to confirm a payment. Models Nexus runs are paused until an owner confirms it: Confirm payment under Billing. Nothing is charged twice. Your own keys keep working. Nothing is lost."
        );
        let PayNotice::Refused { action, .. } = refused("card_needs_confirming", None, None) else {
            unreachable!()
        };
        assert!(refusal(&error).contains(action));
    }

    /// Each 402 refusal's title is the glossary's (`refusals`), the same as on the web; the unknown reason's notice too.
    #[test]
    fn each_refusal_says_the_glossary_title() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        let refusals = fixture["refusals"].as_array().expect("refusals");
        assert_eq!(refusals.len(), 7);
        for row in refusals {
            let code = row["code"].as_str().expect("code");
            let reason = row["reason"].as_str();
            let title = row["title"].as_str().expect("title");
            assert_eq!(refusal_title(code, reason), Some(title), "{code} {reason:?}");
            let error = nexus_client::Error::from(nexus_client::error::CoreError {
                status: 402,
                message: "refused".into(),
                code: Some(code.into()),
                interval: None,
                details: reason.map(|reason| json!({ "reason": reason })),
            });
            assert!(refusal(&error).starts_with(&format!("{title}. ")), "{code} {reason:?}");
        }
        let fallback = &fixture["payNoticeFallback"];
        let PayNotice::Refused { title, action, .. } =
            refused(fallback["reason"].as_str().expect("reason"), None, None)
        else {
            unreachable!()
        };
        assert_eq!(fallback["title"].as_str(), Some(title.as_str()));
        assert_eq!(fallback["action"].as_str(), Some(action));
    }
}
