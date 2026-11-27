use super::*;
use core::prelude::v1::test;
use serde_json::json;

fn refused(status: u16, code: &str, details: Option<Value>) -> String {
    crate::pay_notice::refusal(&nexus_client::Error::from(nexus_client::error::CoreError {
        status,
        message: "refused".into(),
        code: Some(code.into()),
        interval: None,
        details,
    }))
}

/// Gateway's budget refusal reads as the line's stop, with its level and when calls resume.
#[test]
fn a_budget_refusal_names_its_level_and_when_calls_resume() {
    let at = "2026-12-01T12:00:00Z";
    let day = resume_day(at).expect("a day");
    let person = refused(
        402,
        "budget_exceeded",
        Some(json!({ "level": "person", "resetsAt": at, "remainingUsd": 0, "remainingUsdDecimal": "0.000000" })),
    );
    assert_eq!(person, format!("Your monthly limit is used up. Calls resume {day}."));
    assert_eq!(
        stop_of(&person),
        Some(Stop::Budget {
            level: Level::Person,
            used_up: true,
            resumes: Some(day.clone())
        })
    );
    let team = refused(
        402,
        "budget_exceeded",
        Some(json!({ "level": "team", "remainingUsdDecimal": "0.200000" })),
    );
    assert_eq!(team, "Your team's budget doesn't cover this call.");
    assert_eq!(
        stop_of(&team),
        Some(Stop::Budget {
            level: Level::Team,
            used_up: false,
            resumes: None
        })
    );
    // A product budget keeps Gateway's own words.
    assert_eq!(
        refused(402, "budget_exceeded", Some(json!({ "level": "product" }))),
        "refused"
    );
}

/// Each billing refusal `pay_notice::refusal` writes reads as its stop; anything else is no stop.
#[test]
fn each_billing_refusal_reads_as_its_stop() {
    let cases = [
        (
            "usage_limit_reached",
            json!({ "remainingUsd": "0.000000" }),
            Stop::UsageLimit { used_up: true },
        ),
        (
            "usage_limit_reached",
            json!({ "requestedUsd": "0.400000", "remainingUsd": "0.100000" }),
            Stop::UsageLimit { used_up: false },
        ),
        ("usage_limit_reached", json!({}), Stop::UsageLimit { used_up: false }),
        ("subsidy_exhausted", json!({}), Stop::FreeStart),
        ("credit_exhausted", json!({}), Stop::CreditsOut),
        ("payment_failed", json!({ "reason": "declined" }), Stop::PaymentFailed),
        ("payment_failed", json!({ "reason": "disputed" }), Stop::PaymentFailed),
        (
            "payment_failed",
            json!({ "reason": "authentication_required" }),
            Stop::ConfirmPayment,
        ),
        ("payment_past_due", json!({}), Stop::PastDue),
        ("charge_processing", json!({}), Stop::Processing),
    ];
    for (code, details, stop) in cases {
        let words = refused(402, code, Some(details));
        assert_eq!(stop_of(&words), Some(stop), "{code}: {words}");
    }
    assert_eq!(stop_of("The model is overloaded."), None);
    assert_eq!(stop_of(&refused(403, "provider_key_required", None)), None);
}

/// A limit takes the amber dot and names the account; Raise limit opens its row for who may manage budgets.
#[test]
fn a_budget_line_names_the_account_and_raises_its_own_row() {
    let words = words(
        &Stop::Budget {
            level: Level::Organization,
            used_up: true,
            resumes: Some("Dec 1".into()),
        },
        "Acme",
    );
    assert_eq!(words.what, "Acme's budget is used up.");
    assert_eq!(words.why.as_deref(), Some("Calls resume Dec 1."));
    assert_eq!(words.tone, Tone::Limit);
    assert_eq!(words.billing, None, "a budget asks by message, not through billing");
    let Some(("Raise limit", Fix::Open { page, focus, needs })) = words.fix else {
        panic!("Raise limit opens a page");
    };
    assert_eq!((page, focus, needs), (PageId::Usage, Some(ORG_ROW), Some(BUDGETS)));

    let person = super::words(
        &Stop::Budget {
            level: Level::Person,
            used_up: false,
            resumes: None,
        },
        "",
    );
    assert_eq!(
        person.what,
        "Your monthly limit in Your organization doesn't cover this call."
    );
    assert!(matches!(
        person.fix,
        Some((
            _,
            Fix::Open {
                focus: Some(PERSON_ROW),
                ..
            }
        ))
    ));
}

/// Billing is never red: only a charge that couldn't be collected takes the ✕, and every billing ask goes to the owners.
#[test]
fn billing_stops_ask_the_owners_and_only_a_failed_charge_is_red() {
    for stop in [
        Stop::UsageLimit { used_up: true },
        Stop::FreeStart,
        Stop::CreditsOut,
        Stop::PaymentFailed,
        Stop::ConfirmPayment,
        Stop::PastDue,
    ] {
        let words = words(&stop, "Acme");
        assert!(words.billing.is_some(), "{stop:?} asks the owners' inboxes");
        assert_eq!(words.fix.as_ref().and_then(|(_, fix)| fix.required()), Some(BILLING));
        assert_eq!(words.tone == Tone::Failed, stop == Stop::PaymentFailed, "{stop:?}");
        assert!(words.what.contains("Acme"), "{}", words.what);
    }
    let waiting = words(&Stop::Processing, "Acme");
    assert_eq!(waiting.tone, Tone::Info);
    assert!(waiting.fix.is_none());
}

#[test]
fn limits_rows_open_the_limits_tab() {
    for row in [ORG_ROW, TEAM_ROW, PERSON_ROW, USAGE_ROW] {
        assert!(is_limit_row(row));
    }
    assert!(!is_limit_row("usage-limits"));
}
