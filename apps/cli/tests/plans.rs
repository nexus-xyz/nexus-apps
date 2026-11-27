//! The dormant plan commands (`src/plans.rs`, `--features plans`) against the billing stand-in of `billing.rs`:
//! `billing plans`, `billing plan <name>` waiting for the plan Stripe starts, `billing plan change`, the "Plan" row and
//! bad usage before anything is sent. Pricing v2.0 sells no plans, so the default CLI builds without them.

use serde_json::{Value, json};

use crate::billing::{
    CHECKOUT, Seen, Stand, billing, company_plans, parsed, run, run_with, team_monthly, visa, with_v2,
};

#[test]
fn refuses_bad_plan_usage_before_anything_is_sent() {
    for argv in [
        vec!["billing", "plan"],
        vec!["billing", "plan", "teams_pro", "--seats", "five"],
        vec!["billing", "plan", "teams_pro", "--seats", "0"],
        vec!["billing", "plan", "teams_pro", "--seats", "99999999999999999999"],
        vec!["billing", "plan", "teams_pro", "--seats"],
        vec!["billing", "plan", "teams_pro", "--seats", "--yearly"],
        vec!["billing", "plan", "change"],
        vec!["billing", "plan", "change", "teams_pro", "--seats", "two"],
    ] {
        let refused = run(Stand::default(), &argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn calls_the_free_starts_row_the_nexus_usage_on_a_plan() {
    let mut on_plan = with_v2(Value::Null);
    on_plan["plan"] = pro_plan("active");
    let planned = run(
        Stand {
            billing: vec![on_plan],
            ..Stand::default()
        },
        &["billing"],
    );
    assert!(
        planned
            .out
            .contains(&"  Nexus usage     $0.40 left of $1.00".to_string()),
        "{:?}",
        planned.out
    );
}

#[test]
fn lists_the_plans_priced_by_stripe_per_seat_for_a_company() {
    let listed = run(Stand::default(), &["billing", "plans"]);
    assert_eq!(listed.code, 0, "{:?}", listed.err);
    assert_eq!(
        listed.out,
        [
            "Plans · for a company, priced per seat",
            "",
            "  Plan        Name                      Monthly   Yearly   Included a month",
            "· teams_pro   Nexus Teams Pro (yours)    $30.00        –             $10.00",
            "",
            "Prices and included usage are per seat, from 2 seats.",
            "–: Stripe did not give this price. Try again in a moment.",
            "Start one: nexus billing plan <plan> [--yearly] [--seats N]",
        ]
    );
    let json = run(Stand::default(), &["billing", "plans", "--json"]);
    assert_eq!(parsed(&json.out), company_plans());
    // Pricing v2.0 sells no plans: a Gateway without NEXUS_PLANS says so, and it is not an error.
    let none = run(
        Stand {
            off: Some((503, Some("not_configured"), "plans need NEXUS_PLANS on Gateway")),
            ..Stand::default()
        },
        &["billing", "plans"],
    );
    assert_eq!(none.code, 0, "{:?}", none.err);
    assert_eq!(
        none.out,
        [
            "› This Gateway sells no plans: it bills pay as you go (`nexus billing credits`, `nexus billing after-the-fact`)."
        ]
    );
}

#[test]
fn changes_the_plan_now_or_at_the_period_end_and_undoes_a_pending_change() {
    let now = Stand {
        change: Some(json!({
            "when": "now", "at": "2026-10-26T12:00:00.000Z", "amountDueUsd": "12.340000", "dryRun": true,
            "plan": team_monthly(None),
        })),
        ..Stand::default()
    };
    let dry = run(
        now,
        &["billing", "plan", "change", "teams_pro", "--seats", "5", "--dry-run"],
    );
    assert_eq!(dry.code, 0, "{:?}", dry.err);
    assert_eq!(
        dry.out,
        [
            "› Switching to Teams Pro (monthly, 5 seats) applies now: $12.34 is charged today, prorated.",
            "  Nothing changes without dropping --dry-run.",
        ]
    );
    let post = dry.seen.iter().find(|call| call.method == "POST").expect("a change");
    assert_eq!(
        serde_json::from_str::<Value>(&post.body).expect("body"),
        json!({ "plan": "teams_pro", "interval": "month", "dryRun": true, "seats": 5 })
    );
    let pending = json!({ "plan": "teams_pro", "interval": "year", "seats": 3, "at": "2026-11-17T07:20:00.000Z" });
    let later = Stand {
        change: Some(json!({
            "when": "period_end", "at": "2026-11-17T07:20:00.000Z", "amountDueUsd": "0.000000", "dryRun": false,
            "plan": team_monthly(Some(pending.clone())),
        })),
        ..Stand::default()
    };
    let scheduled = run(
        later,
        &["billing", "plan", "change", "teams_pro", "--yearly", "--seats", "3"],
    );
    assert_eq!(
        scheduled.out,
        [
            "✓ Switching to Teams Pro (yearly, 3 seats) on 2026-11-17, when the period ends. Until then the plan stays as it is.",
            "  Cancel it: nexus billing plan change --undo",
        ]
    );
    let shown = run(
        Stand {
            billing: vec![
                json!({ "mode": "test", "customer": { "state": "ready" }, "paymentMethod": visa(), "plan": team_monthly(Some(pending)), "pastDue": null }),
            ],
            ..Stand::default()
        },
        &["billing"],
    );
    assert!(
        shown
            .out
            .iter()
            .any(|line| line.contains("(becomes Teams Pro, yearly, 3 seats on 2026-11-17)")),
        "{:?}",
        shown.out
    );
    let undone = run(Stand::default(), &["billing", "plan", "change", "--undo"]);
    assert_eq!(
        undone.out,
        ["✓ The pending change is cancelled. The plan stays: Teams Pro, monthly, 3 seats, $30.00 included each month."]
    );
    assert!(undone.seen.iter().any(|call| call.method == "DELETE"));
}

fn team_plan(status: &str, cancel: bool) -> Value {
    json!({
        "plan": "teams_pro", "interval": "year", "status": status, "seats": 5,
        "currentPeriodEnd": "2027-10-17T07:20:00.000Z", "cancelAtPeriodEnd": cancel, "includedUsd": "50.000000",
    })
}

fn with_plan(plan: Value) -> Value {
    let mut answer = billing(visa(), Value::Null);
    answer["plan"] = plan;
    answer
}

#[test]
fn shows_the_plan_above_the_card_and_none_on_the_free_plan() {
    let shown = run(
        Stand {
            billing: vec![with_plan(team_plan("active", false))],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.code, 0);
    assert_eq!(
        shown.out[2],
        "  Plan  Teams Pro, yearly, 5 seats, $50.00 included each month"
    );
    assert_eq!(shown.out[3], "  Card  Visa ending in 4242, expires 04/2099");
    let unpaid = run(
        Stand {
            billing: vec![with_plan(team_plan("past_due", true))],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(
        unpaid.out[2],
        "  Plan  Teams Pro, yearly, 5 seats, $50.00 included each month (past due) (ends 2027-10-17)"
    );
    // Stripe's Dashboard can cancel in a later period: this one renews, and the plan ends on `cancelAt`.
    let mut cancelled_later = team_plan("active", false);
    cancelled_later["cancelAt"] = json!("2028-01-05T00:00:00.000Z");
    let later = run(
        Stand {
            billing: vec![with_plan(cancelled_later)],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(
        later.out[2],
        "  Plan  Teams Pro, yearly, 5 seats, $50.00 included each month (ends 2028-01-05)"
    );
    let free = run(
        Stand {
            billing: vec![with_plan(Value::Null)],
            ..Stand::default()
        },
        &["billing"],
    );
    assert!(!free.out.iter().any(|line| line.contains("Plan")), "{:?}", free.out);
}

#[test]
fn opens_stripe_checkout_for_a_plan_and_waits_until_stripe_starts_it() {
    let free = with_plan(Value::Null);
    let started = with_plan(team_plan("active", false));
    let run_plan = run_with(
        Stand {
            billing: vec![free.clone(), free.clone(), started.clone()],
            ..Stand::default()
        },
        true,
        &["billing", "plan", "teams_pro", "--yearly", "--seats", "5"],
    );
    assert_eq!(run_plan.code, 0, "{:?}", run_plan.err);
    assert_eq!(run_plan.opened, [CHECKOUT]);
    assert_eq!(
        run_plan.out,
        [
            format!("› Start Teams Pro (yearly) on Stripe's page: {CHECKOUT}"),
            "  Opened your browser. Waiting for Stripe to start the plan…".to_string(),
            "✓ Started: Teams Pro, yearly, 5 seats, $50.00 included each month.".to_string(),
        ]
    );
    let sent: Vec<&Seen> = run_plan
        .seen
        .iter()
        .filter(|call| call.path == "/v1/billing/plan")
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&sent[0].body).expect("body"),
        json!({ "plan": "teams_pro", "interval": "year", "seats": 5 })
    );
    // Monthly and no seats unless asked; --no-wait returns the link at once.
    let now = run(Stand::default(), &["billing", "plan", "pro", "--no-wait", "--json"]);
    assert_eq!(now.code, 0);
    assert_eq!(
        parsed(&now.out),
        json!({ "url": CHECKOUT, "expiresAt": "2026-10-23T19:00:00.000Z" })
    );
    assert_eq!(
        now.err[0],
        format!("› Start Pro (monthly) on Stripe's page: {CHECKOUT}")
    );
    assert_eq!(
        now.err[1],
        "  Open the link before 2026-10-23 19:00 UTC; nexus billing shows the plan once Stripe starts it."
    );
    let body = &now
        .seen
        .iter()
        .find(|call| call.path == "/v1/billing/plan")
        .expect("sent")
        .body;
    assert_eq!(
        serde_json::from_str::<Value>(body).expect("body"),
        json!({ "plan": "pro", "interval": "month" })
    );
    let late = run(Stand::default(), &["billing", "plan", "pro"]);
    assert_eq!(late.code, 1);
    assert!(late.err.join("\n").contains("Stripe has not started the plan yet."));
}

#[test]
fn waits_for_the_plan_when_an_older_gateway_leaves_plan_out() {
    let mut absent = with_plan(Value::Null);
    absent.as_object_mut().expect("object").remove("plan");
    let started = with_plan(team_plan("active", false));
    let run_plan = run_with(
        Stand {
            billing: vec![absent.clone(), absent, started],
            ..Stand::default()
        },
        true,
        &["billing", "plan", "teams_pro", "--yearly", "--seats", "5"],
    );
    assert_eq!(run_plan.code, 0, "{:?}", run_plan.err);
    assert_eq!(
        run_plan.out.last().map(String::as_str),
        Some("✓ Started: Teams Pro, yearly, 5 seats, $50.00 included each month.")
    );
}

#[test]
fn refuses_a_plan_where_billing_is_off() {
    let off = Stand {
        off: Some((503, Some("not_configured"), "plans need NEXUS_PLANS on Gateway")),
        ..Stand::default()
    };
    let refused = run(off, &["billing", "plan", "pro"]);
    assert_eq!(refused.code, 1);
    let text = refused.err.join("\n");
    assert!(
        text.contains("Billing is off on this Gateway, so it starts no plan."),
        "{text}"
    );
    assert!(text.contains("plans need NEXUS_PLANS on Gateway"), "{text}");
}

fn pro_plan(status: &str) -> Value {
    json!({
        "plan": "pro", "interval": "month", "status": status, "seats": 1,
        "currentPeriodEnd": "2026-11-17T07:20:00.000Z", "cancelAtPeriodEnd": false, "includedUsd": "0.125000",
    })
}

#[test]
fn prints_the_included_money_digit_for_digit() {
    let shown = run(
        Stand {
            billing: vec![with_plan(pro_plan("active"))],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.out[2], "  Plan  Pro, monthly, $0.125 included each month");
}

#[test]
fn waits_past_the_old_plan_and_an_unpaid_one_until_stripe_starts_the_new_one() {
    // On Pro already: the webhook has not changed it for two polls, so Pro is not what started.
    let pro = with_plan(pro_plan("active"));
    let team = with_plan(team_plan("active", false));
    let switched = run(
        Stand {
            billing: vec![pro.clone(), pro.clone(), pro, team.clone()],
            ..Stand::default()
        },
        &["billing", "plan", "teams_pro", "--yearly", "--seats", "5"],
    );
    assert_eq!(switched.code, 0, "{:?}", switched.err);
    assert_eq!(switched.calls("/v1/billing"), 4);
    assert_eq!(
        switched.out.last().map(String::as_str),
        Some("✓ Started: Teams Pro, yearly, 5 seats, $50.00 included each month.")
    );
    // An `incomplete` subscription is not paid: the wait goes on until it is active.
    let free = with_plan(Value::Null);
    let unpaid = with_plan(team_plan("incomplete", false));
    let paid = run(
        Stand {
            billing: vec![free.clone(), unpaid.clone(), unpaid.clone(), team],
            ..Stand::default()
        },
        &["billing", "plan", "teams_pro", "--yearly", "--seats", "5"],
    );
    assert_eq!(paid.code, 0, "{:?}", paid.err);
    assert_eq!(paid.calls("/v1/billing"), 4);
    assert_eq!(
        paid.out.last().map(String::as_str),
        Some("✓ Started: Teams Pro, yearly, 5 seats, $50.00 included each month.")
    );
    let never = run(
        Stand {
            billing: vec![free, unpaid],
            ..Stand::default()
        },
        &["billing", "plan", "teams_pro"],
    );
    assert_eq!(never.code, 1);
    assert_eq!(never.calls("/v1/billing"), 301);
    assert!(never.err.join("\n").contains("Stripe has not started the plan yet."));
}
