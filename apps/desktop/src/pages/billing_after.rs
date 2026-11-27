//! Paying after the fact's charge phase and its words, as kit's `after-the-fact.ts` says them (`GET /v1/billing`'s
//! `afterTheFact`). Gateway names the phase; an older Gateway does not, and then `phase_of` works it out from the other
//! fields. The tests check every phase's words against `kit/src/fixtures/billing-words.json`, as Nexus UI shows them.

use super::{After, cents, money, words};

/// The charge phase, in Gateway's order of precedence (kit's `AfterTheFactPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
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
    fn parse(name: &str) -> Option<Phase> {
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
    pub(super) fn charge_problem(self) -> bool {
        matches!(
            self,
            Phase::Failed | Phase::Retrying | Phase::NeedsConfirmation | Phase::Disputed
        )
    }
}

/// The owner's way on for a charge problem, as the glossary names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    FixPayment,
    ConfirmPayment,
    AddAnotherCard,
}

impl Action {
    pub(super) fn label(self) -> &'static str {
        match self {
            Action::FixPayment => words::FIX_PAYMENT,
            Action::ConfirmPayment => words::CONFIRM_PAYMENT,
            Action::AddAnotherCard => words::ADD_ANOTHER_CARD,
        }
    }
}

/// What the page says beyond its usual line (kit's `AfterTheFactNotice`). `title` and `body` have no final period.
/// `stops_calls`: Gateway refuses calls the card pays, so the banner is urgent. With the setting off, nothing runs on
/// the card: a failed final charge stops nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Notice {
    pub(super) phase: Phase,
    pub(super) title: String,
    pub(super) body: Option<String>,
    pub(super) action: Option<Action>,
    pub(super) stops_calls: bool,
}

/// Why a charge for usage after the fact failed (kit's `CHARGE_FAILED_REASON`); `None` for a reason this build does
/// not know, which is then not said.
pub(super) fn charge_failed_reason(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "declined" => "the card was declined",
        "authentication_required" => "the bank wants the cardholder to confirm it",
        "payment_failed" => "the payment failed",
        "no_payment_method" => "no payment method is on file",
        "disputed" => "a payment on the card was disputed",
        _ => return None,
    })
}

/// The usage the card has not paid, a running charge included: `unpaidUsd`, else (an older Gateway) `owedUsd`, which
/// leaves a running charge out and reads "$0.00" during a retry.
pub(super) fn unpaid(after: &After) -> &str {
    after.unpaid.as_deref().unwrap_or(&after.owed)
}

fn micros(text: &str) -> i64 {
    crate::pay_notice::micros_text(text).unwrap_or(0)
}

/// The charge phase: Gateway's `phase`, else the same order from the other fields (kit's `afterTheFactPhaseOf`).
pub(super) fn phase_of(after: &After) -> Phase {
    if let Some(phase) = after.phase.as_deref().and_then(Phase::parse) {
        return phase;
    }
    match after.failed.as_deref() {
        Some("disputed") => Phase::Disputed,
        Some("authentication_required") => Phase::NeedsConfirmation,
        Some(_) if after.pending => Phase::Retrying,
        Some(_) => Phase::Failed,
        None if after.pending => Phase::Charging,
        None if after.enabled => Phase::On,
        None if micros(unpaid(after)) > 0 => Phase::OffOwing,
        None => Phase::Off,
    }
}

const KEEPS: &str = "Credits and your own keys still work";
const NOTHING_RUNS: &str = "Nothing else runs on the card";

/// The notice for `after`, or `None` while nothing needs saying (on, off, closed). `min_final_charge`: Gateway's
/// `pricing.minFinalChargeUsd`.
pub(super) fn notice(after: &After, min_final_charge: &str) -> Option<Notice> {
    let phase = phase_of(after);
    let last = !after.enabled;
    let amount = unpaid(after);
    let stops_calls = !last && phase.charge_problem();
    let made = |title: String, body: String, action: Action| Notice {
        phase,
        title,
        body: Some(body),
        action: Some(action),
        stops_calls,
    };
    let calm = |title: String| Notice {
        phase,
        title,
        body: None,
        action: None,
        stops_calls: false,
    };
    Some(match phase {
        Phase::Failed => made(
            failed_text(after.failed.as_deref(), amount, last),
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
                last_charge(amount, last)
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
                of_amount(Some(amount))
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
        Phase::Charging => calm(running_text(after)),
        Phase::OffOwing => calm(final_charge_text(amount, min_final_charge)),
        Phase::On | Phase::Off | Phase::Closed => return None,
    })
}

/// "A charge of $10.40 is going through": calm, nothing to do (kit's `chargeRunningText`).
pub(super) fn running_text(after: &After) -> String {
    format!(
        "{}{} is going through",
        if after.enabled { "A charge" } else { "The final charge" },
        of_amount(after.charge.as_deref())
    )
}

/// "The last charge of $12.00 didn't go through: the card was declined". An unknown reason is left out.
fn failed_text(reason: Option<&str>, amount: &str, last: bool) -> String {
    let why = reason
        .and_then(charge_failed_reason)
        .map(|why| format!(": {why}"))
        .unwrap_or_default();
    format!("{} didn't go through{why}", last_charge(amount, last))
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
        Some(amount) if micros(amount) >= 10_000 => format!(" of {}", cents(amount)),
        _ => String::new(),
    }
}

/// The final charge once paying after the fact is off (kit's `finalChargeText`): it runs from `minimum`; less stays
/// unpaid until the setting is on again.
pub(super) fn final_charge_text(amount: &str, minimum: &str) -> String {
    let least = crate::pay_notice::micros_text(minimum).unwrap_or(500_000);
    if micros(amount) >= least {
        format!("A final charge of {} runs once open calls settle", cents(amount))
    } else {
        format!(
            "Less than {} of usage is unpaid. It stays unpaid until paying after the fact is on again",
            money(minimum)
        )
    }
}

/// What an empty Receipts list says when nothing waits for a charge (kit's `NO_RECEIPTS`).
pub(super) const NO_RECEIPTS: &str = "No payments yet. Credits you buy and charges for usage show here.";

/// What an empty Receipts list says (kit's `receiptsEmptyText`). While paying after the fact is on and its
/// unpaid usage is from a cent to under the threshold, no charge has run yet: the list names that usage and when the
/// card pays it. Otherwise [`NO_RECEIPTS`]; the banner speaks for a charge.
pub(super) fn receipts_empty_text(after: Option<&After>) -> String {
    let Some(after) = after.filter(|after| phase_of(after) == Phase::On) else {
        return NO_RECEIPTS.into();
    };
    let amount = unpaid(after);
    if micros(amount) < 10_000 || micros(amount) >= micros(&after.threshold) {
        return NO_RECEIPTS.into();
    }
    format!(
        "No payments yet. {} of usage is unpaid. Nexus charges the card when unpaid usage reaches {}.",
        cents(amount),
        money(&after.threshold)
    )
}

/// The unpaid-usage cell's short line for each charge phase (Nexus UI's `UNPAID_NOTE`); `None` for a phase with none.
pub(super) fn summary_note(after: &After, min_final_charge: &str) -> Option<String> {
    Some(match phase_of(after) {
        Phase::Failed => "The last charge failed".into(),
        Phase::Retrying => "The charge is being tried again".into(),
        Phase::NeedsConfirmation => "The charge needs confirming".into(),
        Phase::Disputed => "A payment was disputed".into(),
        Phase::Charging => running_text(after),
        Phase::OffOwing => final_charge_text(unpaid(after), min_final_charge),
        Phase::On | Phase::Off | Phase::Closed => return None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::parse_billing;
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../../../packages/kit/src/fixtures/billing-words.json")).expect("fixture")
    }

    fn after_of(row: &Value) -> After {
        parse_billing(&json!({ "afterTheFact": row }))
            .and_then(|billing| billing.after)
            .expect("afterTheFact")
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

    /// Each charge phase's example in the glossary: each phase, off and failed, an unknown reason, an unknown phase
    /// and an older Gateway without `phase`. The page shows the notice Nexus UI shows for it.
    #[test]
    fn each_phase_says_the_glossary_words() {
        let fixture = fixture();
        let examples = fixture["afterTheFact"].as_array().expect("afterTheFact");
        assert!(examples.len() >= 15);
        for example in examples {
            let case = example["case"].as_str().expect("case");
            let mut row = fixture["afterTheFactBase"].clone();
            for (key, value) in example["patch"].as_object().expect("patch") {
                row[key] = value.clone();
            }
            let after = after_of(&row);
            assert_eq!(Some(phase_name(phase_of(&after))), example["phase"].as_str(), "{case}");
            let said = notice(&after, "0.50").map(|notice| {
                let mut said = json!({
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

    /// An empty Receipts list says the glossary's words: the usage waiting for its first charge, else the plain text.
    #[test]
    fn an_empty_receipts_list_says_the_glossary_words() {
        let fixture = fixture();
        let empty = &fixture["receiptsEmpty"];
        let plain = empty["plain"].as_str().expect("plain");
        assert_eq!(NO_RECEIPTS, plain);
        let examples = empty["examples"].as_array().expect("examples");
        assert!(examples.len() >= 8);
        for example in examples {
            let case = example["case"].as_str().expect("case");
            let after = example["patch"].as_object().map(|patch| {
                let mut row = fixture["afterTheFactBase"].clone();
                for (key, value) in patch {
                    row[key] = value.clone();
                }
                after_of(&row)
            });
            let expected = example["text"].as_str().unwrap_or(plain);
            assert_eq!(receipts_empty_text(after.as_ref()), expected, "{case}");
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
        // An unknown reason is left out, never shown raw.
        assert_eq!(charge_failed_reason("card_expired"), None);
        for action in [Action::FixPayment, Action::ConfirmPayment, Action::AddAnotherCard] {
            assert_eq!(fixture["terms"][action_key(action)].as_str(), Some(action.label()));
        }
    }

    /// The summary cell: a running charge shows behind no failure, and a retry names the unpaid usage, not "$0.00".
    #[test]
    fn the_summary_note_follows_the_phase() {
        let base = json!({
            "enabled": true, "limitUsd": "25.000000", "monthSpentUsd": "12.000000", "reservedUsd": "0",
            "leftUsd": "13.000000", "owedUsd": "0.000000", "thresholdUsd": "10.000000", "pending": true, "failed": null
        });
        let with = |patch: Value| {
            let mut row = base.clone();
            for (key, value) in patch.as_object().expect("patch") {
                row[key] = value.clone();
            }
            after_of(&row)
        };
        let charging = with(json!({ "phase": "charging", "chargeUsd": "10.400000", "unpaidUsd": "10.400000" }));
        assert_eq!(
            summary_note(&charging, "0.50").as_deref(),
            Some("A charge of $10.40 is going through")
        );
        let retrying = with(json!({
            "phase": "retrying", "chargeUsd": "12.000000", "unpaidUsd": "12.000000",
            "failed": { "at": "2026-11-12T10:00:00.000Z", "reason": "declined" }
        }));
        assert_eq!(unpaid(&retrying), "12.000000");
        assert_eq!(
            summary_note(&retrying, "0.50").as_deref(),
            Some("The charge is being tried again")
        );
        let owing = with(json!({ "phase": "off_owing", "enabled": false, "pending": false, "unpaidUsd": "3.200000" }));
        assert_eq!(
            summary_note(&owing, "0.50").as_deref(),
            Some("A final charge of $3.20 runs once open calls settle")
        );
        let on = with(json!({ "phase": "on", "pending": false, "unpaidUsd": "4.000000" }));
        assert_eq!(summary_note(&on, "0.50"), None);
        // An older Gateway: no phase and no unpaidUsd; owedUsd stands in.
        let older = with(json!({ "pending": false, "owedUsd": "4.000000" }));
        assert_eq!((phase_of(&older), unpaid(&older)), (Phase::On, "4.000000"));
    }
}
