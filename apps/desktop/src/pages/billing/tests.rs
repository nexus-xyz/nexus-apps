use super::*;
// The glob import of gpui_kit brings its own `test` macro: name the built-in one.
use core::prelude::v1::test;

/// User copy says Nexus, never Gateway: no string literal in this page's code names it.
#[test]
fn the_page_never_names_gateway() {
    // The page's code now lives in the root and these parts (everything but the tests).
    let code = [
        include_str!("../billing.rs"),
        include_str!("model.rs"),
        include_str!("state.rs"),
        include_str!("actions.rs"),
        include_str!("view.rs"),
        include_str!("panel.rs"),
    ]
    .join("\n");
    let named: Vec<&str> = code
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| line.split('"').skip(1).step_by(2).any(|text| text.contains("Gateway")))
        .collect();
    assert!(named.is_empty(), "{named:#?}");
}

/// The billing glossary: the names, amounts, days and notes this page writes are kit's, as
/// `kit/src/fixtures/billing-words.json` holds them.
#[test]
fn billing_words_match_the_kit_glossary() {
    let fixture = value(include_str!(
        "../../../../../packages/kit/src/fixtures/billing-words.json"
    ));
    let terms = &fixture["terms"];
    for (key, ours) in [
        ("freeStart", words::FREE_START),
        ("promotionalCredit", words::PROMOTIONAL_CREDIT),
        ("credits", words::CREDITS),
        ("payAfterTheFact", words::PAY_AFTER_THE_FACT),
        ("usageLimit", words::USAGE_LIMIT),
        ("changeLimit", words::CHANGE_LIMIT),
        ("autoTopup", words::AUTO_TOPUP),
        ("payAsYouGo", words::PAY_AS_YOU_GO),
        ("datesInUtc", words::DATES_IN_UTC),
        ("payInvoice", words::PAY_INVOICE),
        ("updateCard", words::UPDATE_CARD),
        ("fixPayment", words::FIX_PAYMENT),
        ("addAnotherCard", words::ADD_ANOTHER_CARD),
        ("confirmPayment", words::CONFIRM_PAYMENT),
        ("setUpBilling", words::SET_UP_BILLING),
        ("buyCredits", words::BUY_CREDITS),
    ] {
        assert_eq!(terms[key].as_str(), Some(ours), "{key}");
    }
    // Why Nexus turned auto top-up off: one way for each reason.
    let off = fixture["topupOff"].as_object().expect("topupOff");
    assert_eq!(off.len(), 7);
    for (reason, text) in off {
        assert_eq!(text.as_str(), Some(topup_off_text(reason).as_str()), "{reason}");
    }
    let amounts: Vec<&str> = fixture["creditAmounts"]
        .as_array()
        .expect("amounts")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(amounts, AMOUNTS);
    for row in fixture["money"].as_array().expect("money") {
        let decimal = row["decimal"].as_str().expect("decimal");
        assert_eq!(Some(cents(decimal).as_str()), row["amount"].as_str(), "{decimal}");
        assert_eq!(Some(money(decimal).as_str()), row["setting"].as_str(), "{decimal}");
    }
    for row in fixture["days"].as_array().expect("days") {
        let iso = row["iso"].as_str().expect("iso");
        assert_eq!(Some(day_label(iso).as_str()), row["dayWithYear"].as_str(), "{iso}");
        assert_eq!(Some(cutoff(iso).as_str()), row["cutoff"].as_str(), "{iso}");
    }
    for row in fixture["resets"].as_array().expect("resets") {
        let now = row["now"].as_str().expect("now");
        let day = (
            now[..4].parse().expect("year"),
            now[5..7].parse().expect("month"),
            now[8..10].parse().expect("day"),
        );
        assert_eq!(Some(limit_resets(day).as_str()), row["text"].as_str(), "{now}");
    }
    let stale = &fixture["stale"];
    assert_eq!(
        Some(stale_note(stale["error"].as_str().expect("error")).as_str()),
        stale["note"].as_str()
    );
    assert_eq!(stale["action"].as_str(), Some(words::TRY_AGAIN));
}

/// A sample answer, written as Gateway sends it.
fn value(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON")
}

fn sample() -> Value {
    value(
        r#"{
        "mode": "test",
        "customer": { "state": "ready" },
        "paymentMethod": { "brand": "visa", "last4": "4242", "expMonth": 4, "expYear": 2027 },
        "defaultPaymentMethod": { "type": "card", "brand": "visa", "last4": "4242", "expMonth": 4, "expYear": 2027 },
        "freeStart": { "grantedUsd": "1.000000", "remainingUsd": "0.000000" },
        "credits": { "purchasedUsd": "50.000000", "spentUsd": "44.000000", "reservedUsd": "0", "remainingUsd": "6.000000", "markupBps": 800 },
        "autoTopup": null,
        "afterTheFact": {
            "enabled": true, "limitUsd": "25.000000", "monthSpentUsd": "3.120000", "reservedUsd": "0",
            "leftUsd": "21.880000", "owedUsd": "3.120000", "thresholdUsd": "10.000000", "pending": false, "failed": null
        },
        "pricing": { "markupBps": 800, "minPurchaseUsd": "10.000000", "thresholdUsd": "10.000000", "defaultLimitUsd": "25.000000" },
        "pastDue": {
            "since": "2026-11-01T00:00:00Z", "period": "2026-10", "blockedFrom": "2026-11-08T00:00:00Z",
            "invoiceUrl": "https://invoice.stripe.test/i/pg_1"
        }
    }"#,
    )
}

#[test]
fn the_usage_limit_never_reads_used_while_money_is_left() {
    // Gateway refused a call that may cost $0.35 with $0.20 left.
    let mut billing = parse_billing(&sample()).unwrap();
    let after = billing.after.as_mut().unwrap();
    after.left = "0.200000".into();
    after.refused = Some("0.345001".into());
    assert!(usage_limit_reached(&billing));
    let text = usage_limit_text("Acme", billing.after.as_ref().unwrap(), (2026, 11, 10));
    assert!(
        text.starts_with(
            "Acme has $0.20 left of this month's usage limit of $25, and a call that may cost $0.35 was refused."
        ),
        "{text}"
    );
    let after = billing.after.as_mut().unwrap();
    after.left = "0.004000".into();
    let used = usage_limit_text("Acme", after, (2026, 11, 10));
    assert!(
        used.starts_with("Acme has used this month's usage limit of $25."),
        "{used}"
    );
    assert!(
        used.contains("until the limit resets on 1 Dec, 00:00 UTC or goes up"),
        "{used}"
    );
}

#[test]
fn reads_billing() {
    let billing = parse_billing(&sample()).unwrap();
    assert!(billing.test_mode);
    assert_eq!(billing.card.as_ref().unwrap().expiry(), "04/27");
    assert_eq!(billing.method.as_ref().unwrap().short(), "Visa ·· 4242");
    assert_eq!(billing.credits, Some(("50.000000".into(), "6.000000".into())));
    assert!(billing.topup.is_none());
    let after = billing.after.as_ref().unwrap();
    assert!(after.enabled && after.failed.is_none());
    assert_eq!(pay_mode(&billing).0, "Pay as you go");
    assert_eq!(pay_mode(&billing).1, "Nexus credits and after the fact");
    assert!(credits_low(&billing));
    assert!(!usage_limit_reached(&billing));
    assert!(!free_start_used_up(&billing));
    assert_eq!(month_label(&billing.past_due.as_ref().unwrap().period), "October 2026");
    let due = billing.past_due.as_ref().unwrap();
    assert_eq!(
        past_due_invoice(due, &[]).as_deref(),
        Some("https://invoice.stripe.test/i/pg_1")
    );
    assert_eq!(watched_key(WaitKind::Credits, &billing), "50.000000");
    assert_eq!(Card::key(billing.card.as_ref()), "visa:4242:4/2027");
}

#[test]
fn billing_only_names_a_plan_with_a_current_allowance() {
    let mut answer = sample();
    answer["plan"] = value(r#"{"plan":"pro","status":"active"}"#);
    assert_eq!(parse_billing(&answer).unwrap().plan_name, None);

    answer["planAllowance"] =
        value(r#"{"resetAt":"2026-12-01T00:00:00Z","cloud":{"usedHours":12,"leftHours":138},"ai":null}"#);
    for (status, name) in [
        ("active", Some("Pro")),
        ("past_due", Some("Pro")),
        ("incomplete", None),
        ("canceled", None),
    ] {
        answer["plan"]["status"] = Value::String(status.into());
        let billing = parse_billing(&answer).unwrap();
        assert_eq!(billing.plan_name, name, "{status}");
        assert_eq!(billing.allowance.unwrap().cloud, Some((12., 138.)));
    }
}

#[test]
fn an_older_gateway_s_unpaid_month_is_paid_on_its_receipt_page() {
    let due = PastDue {
        period: "2026-10".into(),
        blocked_from: None,
        invoice: None,
    };
    let month = |id: &str, url: Option<&str>| Receipt {
        id: id.into(),
        at: "2026-11-01T00:00:00Z".into(),
        description: "Nexus usage".into(),
        amount: "12.000000".into(),
        status: "open".into(),
        url: url.map(String::from),
        receipt: false,
    };
    let rows = [
        month("month:2026-09", Some("https://invoice.stripe.test/i/pg_9")),
        month("month:2026-10", Some("https://invoice.stripe.test/i/pg_10")),
    ];
    assert_eq!(
        past_due_invoice(&due, &rows).as_deref(),
        Some("https://invoice.stripe.test/i/pg_10")
    );
    assert_eq!(past_due_invoice(&due, &rows[..1]), None);
}

#[test]
fn auto_top_up_that_gateway_turned_off_says_why_and_what_fixes_it() {
    let mut billing = parse_billing(&sample()).unwrap();
    let off = |reason: Option<&str>| Topup {
        enabled: false,
        below: "10.000000".into(),
        add: "25.000000".into(),
        off_reason: reason.map(String::from),
    };
    billing.credits = Some(("50.000000".into(), "40.000000".into()));
    billing.topup = Some(off(Some("declined")));
    assert_eq!(
        topup_off_notice(&billing),
        Some((
            "Auto top-up is off: the card was declined. Update the card, then turn it on again.".into(),
            TopupFix::UpdateCard
        ))
    );
    billing.topup = Some(off(Some("disputed")));
    assert_eq!(
        topup_off_notice(&billing).map(|(_, fix)| fix),
        Some(TopupFix::AddAnotherCard)
    );
    billing.topup = Some(off(Some("refunded")));
    assert_eq!(
        topup_off_notice(&billing).map(|(_, fix)| fix),
        Some(TopupFix::BuyCredits)
    );
    billing.topup = Some(off(Some("authentication_required")));
    assert_eq!(
        topup_off_notice(&billing).map(|(_, fix)| fix),
        Some(TopupFix::UpdateCard)
    );
    billing.credits = Some(("50.000000".into(), "4.000000".into()));
    billing.topup = Some(off(Some("payment_failed")));
    assert_eq!(
        topup_off_notice(&billing).map(|(text, _)| text).as_deref(),
        Some(
            "Auto top-up is off: a bank payment failed. Credits are under $10. Update the card, then turn it on again."
        )
    );
    billing.topup = Some(off(None));
    assert_eq!(topup_off_notice(&billing), None, "an owner turned it off");
    billing.topup = Some(off(Some("closed")));
    assert_eq!(topup_off_notice(&billing), None, "a closed organization");
}

#[test]
fn promotional_credit_remains_available_after_welcome_credit_is_spent() {
    for (field, available) in [(Some("1.000000"), true), (Some("0.000000"), false), (None, false)] {
        let mut answer =
            serde_json::json!({"mode":"live", "freeStart":{"grantedUsd":"1.000000","remainingUsd":"0.000000"}});
        if let Some(value) = field {
            answer["promotionalCreditUsd"] = serde_json::json!(value);
        }
        let billing = parse_billing(&answer).expect("billing");
        assert_eq!(billing.promotional_credit, field.unwrap_or("0"));
        assert_eq!(free_start_used_up(&billing), !available);
        if available {
            assert!(pay_mode(&billing).1.contains("promotional credit left"));
        }
    }
}

#[test]
fn a_free_account_with_its_start_spent() {
    let billing = parse_billing(&value(
        r#"{
        "mode": "live", "customer": null, "paymentMethod": null, "defaultPaymentMethod": null,
        "freeStart": { "grantedUsd": "1.000000", "remainingUsd": "0.000000" }, "credits": null,
        "autoTopup": null, "afterTheFact": null, "pastDue": null
    }"#,
    ))
    .unwrap();
    assert!(billing.method.is_none());
    assert_eq!(billing.pricing.markup_bps, 800);
    assert!(free_start_used_up(&billing));
    assert_eq!(pay_mode(&billing).1, "$0.00 of your free $1 left");
}

#[test]
fn reads_receipts() {
    let rows = parse_receipts(&value(r#"{ "receipts": [
        { "id": "pi_3Nabcdefgh12345678", "at": "2026-11-03T10:00:00Z", "kind": "credits", "description": "Nexus credits", "amountUsd": "50.000000", "status": "paid", "url": null, "receipt": true },
        { "id": "month:2026-10", "at": "2026-11-01T00:00:00Z", "kind": "invoice", "description": "October 2026, usage", "amountUsd": "184.2", "status": "open", "url": "https://invoice.stripe.com/i/x", "receipt": false }
    ] }"#))
    .unwrap();
    assert_eq!(rows[0].label(), "…12345678");
    assert_eq!(rows[1].label(), "2026-10");
    assert_eq!(cents(&rows[1].amount), "$184.20");
    assert_eq!(day_label(&rows[0].at), "3 Nov 2026");
    let csv = receipts_csv(&rows);
    assert!(csv.starts_with("Receipt,Date,For,Amount (USD),Status\n"));
    assert!(csv.contains("month:2026-10,2026-11-01,\"October 2026, usage\",184.2,open"));
}

#[test]
fn money_and_the_forms() {
    assert_eq!(money("1180.000000"), "$1,180");
    assert_eq!(money("184.2"), "$184.20");
    assert_eq!(money("0.000042"), "$0.000042");
    assert_eq!(cents("1234.5678"), "$1,234.56");
    assert_eq!(cents("-3"), "-$3.00");
    assert_eq!(plain_number("10.000000"), "10");
    assert_eq!(plain_number("12.50"), "12.5");
    assert_eq!(purchase_problem("50", 10.), None);
    assert!(purchase_problem("5", 10.).is_some());
    assert!(purchase_problem("40.505", 10.).is_some());
    assert_eq!(topup_problem("10", "50", 10.), None);
    assert!(topup_problem("10", "600", 10.).is_some());
    assert_eq!(limit_problem("1,000", None), None);
    assert!(limit_problem("0", None).is_some());
    assert_eq!(
        limit_problem("100001", None).as_deref(),
        Some("The limit is at most $100,000.")
    );
    // Nexus says the most a limit may be now: $100 in an organization's first week.
    assert_eq!(limit_problem("100", Some("100.000000")), None);
    assert_eq!(
        limit_problem("100.01", Some("100.000000")).as_deref(),
        Some("The limit is at most $100.")
    );
    assert_eq!(civil(0), (1970, 1, 1));
    assert_eq!(civil(20_397), (2025, 11, 5));
    assert!(billing_off(
        "Stripe needs STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET on Gateway"
    ));
    assert_eq!(
        plain("only the org's owner can manage billing (org.billing)"),
        "Only the organization's owners can change billing."
    );
}

#[test]
fn turning_paying_after_the_fact_off_says_the_final_charge_first() {
    let billing = parse_billing(&sample()).expect("billing");
    let after = billing.after.clone().expect("after the fact");
    // An older Gateway sends no minimum: $0.50, as the web assumes.
    assert_eq!(billing.pricing.min_final_charge, "0.50");
    assert_eq!(
        stop_after_note(&after, &billing.pricing),
        "Final charge for current unpaid usage: $3.12. Nexus rounds down to whole cents and waits for open calls to settle."
    );
    let small = After {
        owed: "0.300000".into(),
        reserved: "1.000000".into(),
        pending: true,
        ..after.clone()
    };
    assert_eq!(
        stop_after_note(&small, &billing.pricing),
        "A charge is already in progress and will continue. Current unpaid usage: $0.30. Whether a final charge is taken depends on the settled total reaching $0.50. Up to $1 of calls are still in flight; the final amount is calculated after they settle."
    );
    let none = After {
        owed: "0".into(),
        failed: Some("declined".into()),
        ..after
    };
    assert_eq!(
        stop_after_note(&none, &billing.pricing),
        "The last charge failed; unpaid usage remains due until a payment succeeds. No additional unpaid usage is currently due."
    );
}

#[test]
fn no_charge_to_confirm_saves_the_card_again() {
    let gateway = |code: &str| {
        nexus_client::Error::from(nexus_client::error::CoreError {
            status: 409,
            message: "any words".into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        })
    };
    assert!(nothing_to_confirm(&gateway("nothing_to_confirm")));
    assert!(!nothing_to_confirm(&gateway("charge_running")));
    assert!(!nothing_to_confirm(&nexus_client::Error::Transport(
        nexus_client::error::Transport::Timeout
    )));
}

#[test]
fn a_confirmed_charge_says_it_went_through_only_once_stripe_says_so() {
    assert_eq!(
        confirmed_note(&json!({ "status": "succeeded" })),
        "The payment went through."
    );
    for answer in [
        json!({ "status": "processing" }),
        json!({ "status": "redirect", "url": null }),
    ] {
        assert_eq!(
            confirmed_note(&answer),
            "Stripe is taking the payment. It shows here once Stripe confirms it."
        );
    }
}

/// `sample()` with its after-the-fact fields patched, as a newer or an older Gateway sends them.
fn with_after(patch: Value) -> Billing {
    let mut answer = sample();
    for (key, value) in patch.as_object().expect("patch") {
        answer["afterTheFact"][key] = value.clone();
    }
    parse_billing(&answer).expect("billing")
}

fn failure(reason: &str) -> Value {
    json!({ "at": "2026-11-12T10:00:00.000Z", "reason": reason })
}

/// The banner draws from the phase and names the unpaid usage: a retry or a confirmation never says "$0.00".
#[test]
fn the_charge_banner_names_the_unpaid_usage_in_each_phase() {
    let banner = |patch: Value| charge_banner(&with_after(patch));
    let retrying = banner(json!({
        "phase": "retrying", "pending": true, "chargeUsd": "12.000000", "unpaidUsd": "12.000000",
        "owedUsd": "0.000000", "failed": failure("declined")
    }))
    .expect("a banner");
    assert_eq!(
        retrying,
        (
            "The last charge of $12.00 didn't go through. Nexus is trying it again now. Nexus-run AI the card pays stays paused until it's paid. Credits and your own keys still work.".into(),
            charge::Action::FixPayment,
            true
        )
    );
    let (confirm, action, urgent) = banner(json!({
        "phase": "needs_confirmation", "pending": true, "chargeUsd": "12.000000", "unpaidUsd": "12.000000",
        "owedUsd": "0.000000", "failed": failure("authentication_required")
    }))
    .expect("a banner");
    assert!(
        confirm.starts_with("The charge of $12.00 needs the cardholder's confirmation."),
        "{confirm}"
    );
    assert!(confirm.ends_with(confirm_charge_note()), "{confirm}");
    assert_eq!((action, urgent), (charge::Action::ConfirmPayment, true));
    let disputed = banner(json!({ "phase": "disputed", "unpaidUsd": "12.000000", "failed": failure("disputed") }));
    assert_eq!(
        disputed.map(|(_, action, _)| action),
        Some(charge::Action::AddAnotherCard)
    );
    let failed = banner(json!({ "phase": "failed", "unpaidUsd": "12.000000", "failed": failure("declined") }))
        .expect("a banner");
    assert!(
        failed
            .0
            .starts_with("The last charge of $12.00 didn't go through: the card was declined.")
    );
    // Calm phases show in the row, not as a banner.
    assert_eq!(
        banner(json!({ "phase": "charging", "pending": true, "chargeUsd": "10.400000" })),
        None
    );
    assert_eq!(banner(json!({ "phase": "on" })), None);
}

/// With the setting off, a failed final charge pauses nothing: no urgent banner, no "Paused" in the row.
#[test]
fn off_and_failed_says_the_final_charge_failed() {
    let billing = with_after(json!({
        "phase": "failed", "enabled": false, "unpaidUsd": "3.200000", "owedUsd": "3.200000",
        "failed": failure("no_payment_method")
    }));
    let (text, action, urgent) = charge_banner(&billing).expect("a banner");
    assert_eq!(
        text,
        "The final charge of $3.20 didn't go through: no payment method is on file. Nothing else runs on the card. An owner fixes the card so the charge goes through."
    );
    assert_eq!((action, urgent), (charge::Action::FixPayment, false));
    let pay = charge::notice(billing.after.as_ref().unwrap(), &billing.pricing.min_final_charge).unwrap();
    assert!(!pay.stops_calls);
}

/// The summary cell: the phase decides its line, a running charge shows its amount, and the value is the unpaid usage.
#[test]
fn the_summary_cell_follows_the_phase() {
    let detail = |patch: Value| {
        let billing = with_after(patch);
        after_summary_detail(billing.after.as_ref(), &billing.pricing)
    };
    assert_eq!(
        detail(json!({ "phase": "charging", "pending": true, "chargeUsd": "10.400000", "unpaidUsd": "10.400000" })),
        "A charge of $10.40 is going through"
    );
    // A failure no longer hides a retry.
    assert_eq!(
        detail(
            json!({ "phase": "retrying", "pending": true, "unpaidUsd": "12.000000", "failed": failure("declined") })
        ),
        "The charge is being tried again"
    );
    assert_eq!(
        detail(json!({ "phase": "off_owing", "enabled": false, "unpaidUsd": "3.200000" })),
        "A final charge of $3.20 runs once open calls settle"
    );
    assert_eq!(detail(json!({ "phase": "on" })), "Charged at every $10");
    assert_eq!(detail(json!({ "phase": "off", "enabled": false })), "Off");
    assert_eq!(
        after_summary_detail(None, &parse_billing(&sample()).unwrap().pricing),
        "Off"
    );
    let retry = with_after(json!({ "phase": "retrying", "unpaidUsd": "12.000000", "owedUsd": "0.000000" }));
    assert_eq!(cents(charge::unpaid(retry.after.as_ref().unwrap())), "$12.00");
}

/// An older Gateway sends no phase, unpaidUsd or chargeUsd: the page works the phase out as before.
#[test]
fn an_older_gateway_without_a_phase_still_shows_the_problem() {
    let billing = with_after(json!({ "pending": true, "owedUsd": "0.000000", "failed": failure("declined") }));
    let after = billing.after.as_ref().unwrap();
    assert_eq!(charge::phase_of(after), charge::Phase::Retrying);
    let (text, _, urgent) = charge_banner(&billing).expect("a banner");
    assert!(
        text.starts_with("The last charge didn't go through. Nexus is trying it again now."),
        "{text}"
    );
    assert!(!text.contains("$0.00") && urgent);
    // An unknown reason is left out, never shown raw.
    let unknown = with_after(json!({ "failed": failure("card_expired") }));
    let (text, _, _) = charge_banner(&unknown).expect("a banner");
    assert!(
        text.starts_with("The last charge of $3.12 didn't go through."),
        "{text}"
    );
    assert!(!text.contains("card_expired"));
    // And the limit's hint: an older Gateway says no maximum, and its first-week cap was $100.
    assert_eq!(
        limit_hint(&billing.pricing),
        "New accounts start at $25, at most $100 in their first week."
    );
    let mut answer = sample();
    answer["pricing"]["maxLimitUsd"] = json!("100.000000");
    let newer = parse_billing(&answer).unwrap();
    assert_eq!(
        limit_hint(&newer.pricing),
        "New accounts start at $25. The limit is at most $100 now."
    );
}

#[test]
fn a_charge_the_bank_wants_confirmed_says_what_to_do_on_stripe() {
    assert_eq!(
        confirm_charge_note(),
        "Approve it on your bank's page; nothing is charged twice."
    );
}
