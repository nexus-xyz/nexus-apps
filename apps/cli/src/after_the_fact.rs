//! Paying after the fact's charge phase and its words, as kit's `after-the-fact.ts` says them (`GET /v1/billing`'s
//! `afterTheFact`). Gateway names the phase; an older Gateway does not, and then `phase_of` works it out from the other
//! fields. The tests check every phase's words against `kit/src/fixtures/billing-words.json`, which the TypeScript CLI
//! reads through the kit.

use serde_json::Value;

use crate::json::s;

/// The charge phase, in Gateway's order of precedence (kit's `AfterTheFactPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Closed,
    Disputed,
    NeedsConfirmation,
    Retrying,
    Failed,
    Charging,
    On,
    OffOwing,
    Off,
}

impl Phase {
    /// Gateway's name; `None` for a phase this build does not know.
    pub fn parse(name: &str) -> Option<Phase> {
        Some(match name {
            "closed" => Phase::Closed,
            "disputed" => Phase::Disputed,
            "needs_confirmation" => Phase::NeedsConfirmation,
            "retrying" => Phase::Retrying,
            "failed" => Phase::Failed,
            "charging" => Phase::Charging,
            "on" => Phase::On,
            "off_owing" => Phase::OffOwing,
            "off" => Phase::Off,
            _ => return None,
        })
    }

    /// A charge failed or waits for the cardholder: an owner must act (kit's `chargeProblem`).
    pub fn charge_problem(self) -> bool {
        matches!(
            self,
            Phase::Failed | Phase::Retrying | Phase::NeedsConfirmation | Phase::Disputed
        )
    }
}

/// The owner's way on for a charge problem: a key of the glossary's `terms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    FixPayment,
    ConfirmPayment,
    AddAnotherCard,
}

impl Action {
    /// The glossary's name for it (kit's `BILLING_TERMS`).
    pub fn label(self) -> &'static str {
        match self {
            Action::FixPayment => "Fix payment",
            Action::ConfirmPayment => "Confirm payment",
            Action::AddAnotherCard => "Add another card",
        }
    }

    /// The command that does it.
    pub fn command(self) -> &'static str {
        match self {
            Action::ConfirmPayment => "nexus billing confirm",
            Action::FixPayment | Action::AddAnotherCard => "nexus billing card add",
        }
    }
}

/// What a surface says beyond its usual line (kit's `AfterTheFactNotice`). `title` and `body` have no final period.
/// `stops_calls`: Gateway refuses calls the card pays. With the setting off, nothing runs on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub phase: Phase,
    pub title: String,
    pub body: Option<String>,
    pub action: Option<Action>,
    pub stops_calls: bool,
}

/// Why a charge for usage after the fact failed (kit's `CHARGE_FAILED_REASON`); `None` for a reason this build does
/// not know, which is then not said.
pub fn charge_failed_reason(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "declined" => "the card was declined",
        "authentication_required" => "the bank wants the cardholder to confirm it",
        "payment_failed" => "the payment failed",
        "no_payment_method" => "no payment method is on file",
        "disputed" => "a payment on the card was disputed",
        _ => return None,
    })
}

/// The usage the card has not paid, a running charge included: `unpaidUsd`, else (an older Gateway) `owedUsd`.
pub fn unpaid_usd(after: &Value) -> &str {
    s(after, "unpaidUsd").or_else(|| s(after, "owedUsd")).unwrap_or("0")
}

fn failed_reason(after: &Value) -> Option<&str> {
    after
        .get("failed")
        .filter(|failed| failed.is_object())
        .map(|failed| s(failed, "reason").unwrap_or(""))
}

fn flag(after: &Value, key: &str) -> bool {
    after.get(key).and_then(Value::as_bool) == Some(true)
}

/// The charge phase: Gateway's `phase`, else the same order from the other fields (kit's `afterTheFactPhaseOf`).
pub fn phase_of(after: &Value) -> Phase {
    if let Some(phase) = s(after, "phase").and_then(Phase::parse) {
        return phase;
    }
    match failed_reason(after) {
        Some("disputed") => Phase::Disputed,
        Some("authentication_required") => Phase::NeedsConfirmation,
        Some(_) if flag(after, "pending") => Phase::Retrying,
        Some(_) => Phase::Failed,
        None if flag(after, "pending") => Phase::Charging,
        None if flag(after, "enabled") => Phase::On,
        None if micros(unpaid_usd(after)).unwrap_or(0) > 0 => Phase::OffOwing,
        None => Phase::Off,
    }
}

const KEEPS: &str = "Credits and your own keys still work";
const NOTHING_RUNS: &str = "Nothing else runs on the card";

/// The notice for `after`, or `None` while nothing needs saying (on, off, closed). `min_final_charge`: Gateway's
/// `pricing.minFinalChargeUsd`. The amount named is the unpaid usage, never `owedUsd`, which reads "$0.00" in a retry.
pub fn notice(after: &Value, min_final_charge: &str) -> Option<Notice> {
    let phase = phase_of(after);
    let last = !flag(after, "enabled");
    let unpaid = unpaid_usd(after);
    let stops_calls = !last && phase.charge_problem();
    let made = |title: String, body: String, action: Action| Notice {
        phase,
        title,
        body: Some(body),
        action: Some(action),
        stops_calls,
    };
    Some(match phase {
        Phase::Failed => made(
            failed_text(failed_reason(after), unpaid, last),
            if last {
                format!("{NOTHING_RUNS}. An owner fixes the card so the charge goes through")
            } else {
                format!("Nexus-run AI the card pays is paused until a charge goes through. {KEEPS}")
            },
            Action::FixPayment,
        ),
        Phase::Retrying => made(
            format!(
                "{} didn't go through. Nexus is trying it again now",
                last_charge(unpaid, last)
            ),
            if last {
                NOTHING_RUNS.to_string()
            } else {
                format!("Nexus-run AI the card pays stays paused until it's paid. {KEEPS}")
            },
            Action::FixPayment,
        ),
        Phase::NeedsConfirmation => made(
            format!(
                "The {}{} needs the cardholder's confirmation",
                if last { "final charge" } else { "charge" },
                of_amount(Some(unpaid))
            ),
            if last {
                format!("{NOTHING_RUNS}. An owner confirms the charge on the bank's page")
            } else {
                format!("Until then, Nexus-run AI the card pays is paused. {KEEPS}")
            },
            Action::ConfirmPayment,
        ),
        Phase::Disputed => made(
            "A payment on the card was disputed".to_string(),
            if last {
                format!("{NOTHING_RUNS}. An owner adds another card so the charge goes through")
            } else {
                format!("Nexus-run AI the card pays is paused until an owner adds another card. {KEEPS}")
            },
            Action::AddAnotherCard,
        ),
        Phase::Charging => Notice {
            phase,
            title: format!(
                "{}{} is going through",
                if last { "The final charge" } else { "A charge" },
                of_amount(s(after, "chargeUsd"))
            ),
            body: None,
            action: None,
            stops_calls: false,
        },
        Phase::OffOwing => Notice {
            phase,
            title: final_charge_text(unpaid, min_final_charge),
            body: None,
            action: None,
            stops_calls: false,
        },
        Phase::On | Phase::Off | Phase::Closed => return None,
    })
}

/// "The last charge of $12.00 didn't go through: the card was declined". An unknown reason is left out.
pub fn failed_text(reason: Option<&str>, unpaid: &str, last: bool) -> String {
    let why = reason
        .and_then(charge_failed_reason)
        .map(|why| format!(": {why}"))
        .unwrap_or_default();
    format!("{} didn't go through{why}", last_charge(unpaid, last))
}

fn last_charge(amount: &str, last: bool) -> String {
    format!(
        "The {} charge{}",
        if last { "final" } else { "last" },
        of_amount(Some(amount))
    )
}

/// " of $12.00", or nothing for no amount or one under a cent: no text says "$0.00".
fn of_amount(amount: Option<&str>) -> String {
    match amount {
        Some(amount) if micros(amount).is_some_and(|micros| micros >= 10_000) => {
            format!(" of {}", billing_amount(amount))
        }
        _ => String::new(),
    }
}

/// The final charge once paying after the fact is off (kit's `finalChargeText`): it runs from `minimum`; less stays
/// unpaid until the setting is on again.
fn final_charge_text(unpaid: &str, minimum: &str) -> String {
    let minimum = if micros(minimum).is_some() { minimum } else { "0.50" };
    if micros(unpaid).unwrap_or(0) >= micros(minimum).unwrap_or(500_000) {
        format!(
            "A final charge of {} runs once open calls settle",
            billing_amount(unpaid)
        )
    } else {
        format!(
            "Less than {} of usage is unpaid. It stays unpaid until paying after the fact is on again",
            billing_setting(minimum)
        )
    }
}

/// A decimal dollar string as micro-dollars; `None` for anything else, a negative amount included.
fn micros(text: &str) -> Option<i64> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let fraction: i64 = format!("{:0<6}", &fraction[..fraction.len().min(6)]).parse().ok()?;
    whole.parse::<i64>().ok()?.checked_mul(1_000_000)?.checked_add(fraction)
}

/// A decimal string's sign, whole digits and fraction digits.
fn decimal(text: &str) -> Option<(&'static str, &str, &str)> {
    let text = text.trim();
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    // "12." has a point and no fraction: not a decimal.
    (!whole.is_empty() && digits(whole) && digits(fraction) && (fraction.is_empty() != rest.contains('.')))
        .then_some((sign, whole, fraction))
}

fn thousands(whole: &str) -> String {
    let mut out = String::new();
    for (ix, digit) in whole.chars().enumerate() {
        if ix > 0 && (whole.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// An amount to the cent, sub-cent digits cut: "$1,180.00" (kit's `billingAmount`).
fn billing_amount(text: &str) -> String {
    let Some((sign, whole, fraction)) = decimal(text) else {
        return text.to_string();
    };
    let fraction = format!("{fraction:0<2}");
    format!("{sign}${}.{}", thousands(whole), &fraction[..2])
}

/// A setting with cents only when there are some: "$50", "$184.20" (kit's `billingSetting`).
fn billing_setting(text: &str) -> String {
    let Some((sign, whole, fraction)) = decimal(text) else {
        return text.to_string();
    };
    let fraction = fraction.trim_end_matches('0');
    let fraction = if fraction.is_empty() {
        String::new()
    } else {
        format!(".{fraction:0<2}")
    };
    format!("{sign}${}{fraction}", thousands(whole))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json")).expect("fixture")
    }

    fn phase_name(phase: Phase) -> &'static str {
        match phase {
            Phase::Closed => "closed",
            Phase::Disputed => "disputed",
            Phase::NeedsConfirmation => "needs_confirmation",
            Phase::Retrying => "retrying",
            Phase::Failed => "failed",
            Phase::Charging => "charging",
            Phase::On => "on",
            Phase::OffOwing => "off_owing",
            Phase::Off => "off",
        }
    }

    fn action_key(action: Action) -> &'static str {
        match action {
            Action::FixPayment => "fixPayment",
            Action::ConfirmPayment => "confirmPayment",
            Action::AddAnotherCard => "addAnotherCard",
        }
    }

    /// Each charge phase's example in the glossary: its phase, and the notice every surface shows for it.
    #[test]
    fn each_phase_says_the_glossary_words() {
        let fixture = fixture();
        let examples = fixture["afterTheFact"].as_array().expect("afterTheFact");
        assert!(examples.len() >= 15);
        for example in examples {
            let case = example["case"].as_str().expect("case");
            let mut after = fixture["afterTheFactBase"].clone();
            for (key, value) in example["patch"].as_object().expect("patch") {
                after[key] = value.clone();
            }
            assert_eq!(Some(phase_name(phase_of(&after))), example["phase"].as_str(), "{case}");
            let said = notice(&after, "0.50").map(|notice| {
                let mut said = serde_json::json!({
                    "phase": phase_name(notice.phase),
                    "title": notice.title,
                    "stopsCalls": notice.stops_calls,
                });
                if let Some(body) = notice.body {
                    said["body"] = body.into();
                }
                if let Some(action) = notice.action {
                    said["action"] = action_key(action).into();
                }
                said
            });
            assert_eq!(said.unwrap_or(Value::Null), example["notice"], "{case}");
        }
    }

    #[test]
    fn the_reasons_and_the_actions_are_the_glossary_s() {
        let fixture = fixture();
        let reasons = fixture["chargeFailedReasons"].as_object().expect("chargeFailedReasons");
        assert_eq!(reasons.len(), 5);
        for (reason, words) in reasons {
            assert_eq!(charge_failed_reason(reason), words.as_str(), "{reason}");
        }
        assert_eq!(charge_failed_reason("card_expired"), None);
        for action in [Action::FixPayment, Action::ConfirmPayment, Action::AddAnotherCard] {
            assert_eq!(fixture["terms"][action_key(action)].as_str(), Some(action.label()));
        }
        for row in fixture["money"].as_array().expect("money") {
            let decimal = row["decimal"].as_str().expect("decimal");
            assert_eq!(
                Some(billing_amount(decimal).as_str()),
                row["amount"].as_str(),
                "{decimal}"
            );
            assert_eq!(
                Some(billing_setting(decimal).as_str()),
                row["setting"].as_str(),
                "{decimal}"
            );
        }
    }
}
