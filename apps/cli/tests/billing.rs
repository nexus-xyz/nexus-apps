//! `nexus billing` against a stand-in Gateway (`billing.test.ts`): the card and an unpaid month, money digit for
//! digit, `card add` waiting for the card Stripe saves, the free start, Nexus credits, buying them and their auto
//! top-up, paying after the fact, the receipts, billing off or missing on a Gateway as a note, usage errors before
//! anything is sent, and who can pay when a model call is refused for an unpaid month.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, LinkOpener, link_opener, openable, run_cli};
use serde_json::{Value, json};

pub(crate) const KEY: &str = "nx_billing";
pub(crate) const CHECKOUT: &str = "https://checkout.stripe.test/c/pay/cs_test_1";
const PORTAL: &str = "https://billing.stripe.test/p/session/bps_1";
const PAY: &str = "https://checkout.stripe.test/c/pay/cs_test_credits";
const RECEIPT: &str = "https://pay.stripe.test/receipts/payment/rcpt_1";

pub(crate) fn visa() -> Value {
    json!({ "brand": "visa", "last4": "4242", "expMonth": 4, "expYear": 2099 })
}

fn amex() -> Value {
    json!({ "brand": "amex", "last4": "0005", "expMonth": 12, "expYear": 2098 })
}

pub(crate) fn billing(card: Value, past_due: Value) -> Value {
    json!({ "mode": "test", "customer": { "state": "ready" }, "paymentMethod": card, "pastDue": past_due })
}

pub(crate) fn credits(purchased: &str, remaining: &str) -> Value {
    json!({ "purchasedUsd": purchased, "spentUsd": "0.000000", "reservedUsd": "0.000000", "remainingUsd": remaining, "markupBps": 500 })
}

fn topup(enabled: bool, off_reason: Option<&str>) -> Value {
    json!({
        "enabled": enabled, "belowUsd": "10.000000", "addUsd": "25.000000", "offReason": off_reason,
        "todayCount": 1, "todayUsd": "25.000000", "pending": false,
    })
}

fn after(enabled: bool, failed: Value) -> Value {
    json!({
        "enabled": enabled, "limitUsd": "25.000000", "monthSpentUsd": "3.200000", "reservedUsd": "0.500000",
        "leftUsd": "21.300000", "owedUsd": "1.100000", "thresholdUsd": "10.000000",
        "pending": false, "failed": failed,
    })
}

fn pricing() -> Value {
    json!({ "markupBps": 500, "minPurchaseUsd": "10.000000", "thresholdUsd": "10.000000", "defaultLimitUsd": "25.000000" })
}

fn receipts() -> Value {
    json!([
        { "id": "pi_2", "at": "2026-11-02T10:00:00.000Z", "kind": "usage", "description": "Usage after the fact", "amountUsd": "10.000000", "status": "failed", "url": null, "receipt": false },
        { "id": "pi_1", "at": "2026-11-01T09:00:00.000Z", "kind": "credits", "description": "Nexus credits", "amountUsd": "10.000000", "status": "paid", "url": null, "receipt": true },
    ])
}

pub(crate) fn with_credits(mut billing: Value, credits: Value) -> Value {
    billing["credits"] = credits;
    billing
}

fn statement(period: &str) -> Value {
    json!({
        "period": period,
        "lines": [
            { "code": "gateway.calls", "unit": "call", "quantity": "1234", "rateUsd": "0.001", "amountUsd": "1.234" },
            { "code": "gateway.provider_cost", "unit": "usd", "quantity": "12.345678", "rateBps": 250, "amountUsd": "0.30864195" },
        ],
        "providerCostUsd": "12.345678",
        "carriedInUsd": "0.004",
        "totalUsd": "1.54664195",
        "amountCents": "154",
        "carriedOutUsd": "0.00664195",
        "invoice": { "state": "failed", "hostedInvoiceUrl": "https://invoice.stripe.test/i_1" },
    })
}

/// Teams pro, monthly, 3 seats, as `GET /v1/billing` shows it, with a change pending or not.
pub(crate) fn team_monthly(pending: Option<Value>) -> Value {
    json!({
        "plan": "teams_pro", "interval": "month", "status": "active", "seats": 3,
        "currentPeriodEnd": "2026-11-17T07:20:00.000Z", "cancelAtPeriodEnd": false, "includedUsd": "30.000000",
        "includedHours": 450, "hourlyUsd": "0.080000", "sizes": ["default", "large"], "pendingChange": pending,
    })
}

/// A company's catalogue: Teams pro, priced per seat, with its yearly price missing from Stripe.
pub(crate) fn company_plans() -> Value {
    json!({
        "audience": "company",
        "plans": [{
            "plan": "teams_pro", "name": "Nexus Teams Pro", "perSeat": true, "minSeats": 2,
            "includedHours": 150, "hourlyUsd": "0.080000", "sizes": ["default", "large"],
            "intervals": [
                { "interval": "month", "amountUsd": "30.000000", "includedUsd": "10.000000", "error": null },
                { "interval": "year", "amountUsd": null, "includedUsd": "10.000000", "error": "stripe_error" },
            ],
            "current": true,
        }],
        "current": { "plan": "teams_pro", "interval": "month", "seats": 3 },
    })
}

fn statements() -> Value {
    json!([
        { "period": "2026-09", "totalUsd": "1.54664195", "amountCents": "154", "state": "failed", "hostedInvoiceUrl": "https://invoice.stripe.test/i_1" },
        { "period": "2026-08", "totalUsd": "0.31", "amountCents": "31", "state": "closed", "hostedInvoiceUrl": null },
    ])
}

/// What the stand-in answers.
#[derive(Clone, Default)]
pub(crate) struct Stand {
    /// What `GET /v1/billing` answers, call by call; the last one repeats.
    pub(crate) billing: Vec<Value>,
    /// A status, code and message for every billing route instead.
    pub(crate) off: Option<(u16, Option<&'static str>, &'static str)>,
    pub(crate) statements: Option<Value>,
    /// What `POST /v1/billing/credits` answers.
    pub(crate) buy: Option<Value>,
    /// What `PUT /v1/billing/auto-topup` answers.
    pub(crate) topup: Option<Value>,
    /// What `PUT /v1/billing/after-the-fact` answers.
    pub(crate) after: Option<Value>,
    /// What `GET /v1/billing/receipts` answers.
    pub(crate) receipts: Option<Value>,
    /// What `GET /v1/billing/plans` answers.
    pub(crate) plans: Option<Value>,
    /// What `POST /v1/billing/plan/change` answers.
    pub(crate) change: Option<Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: String,
    pub(crate) body: String,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(stand: &Stand, reads: &AtomicUsize, method: &str, path: &str, query: &str) -> Response {
    if let Some((status, code, message)) = stand.off
        && path.starts_with("/v1/billing")
    {
        let mut error = json!({ "message": message, "type": "api_error" });
        if let Some(code) = code {
            error["code"] = json!(code);
        }
        return answer(status, json!({ "error": error }));
    }
    match (method, path) {
        ("GET", "/v1/billing") => {
            let answers = if stand.billing.is_empty() {
                vec![json!({ "mode": "test", "customer": null, "paymentMethod": null, "pastDue": null })]
            } else {
                stand.billing.clone()
            };
            let at = reads.fetch_add(1, Ordering::SeqCst).min(answers.len() - 1);
            answer(200, answers[at].clone())
        }
        ("POST", "/v1/billing/payment-method") | ("POST", "/v1/billing/plan") => {
            answer(200, json!({ "url": CHECKOUT, "expiresAt": "2026-10-23T19:00:00.000Z" }))
        }
        ("POST", "/v1/billing/portal") => answer(200, json!({ "url": PORTAL })),
        ("POST", "/v1/billing/credits") => answer(
            200,
            stand.buy.clone().unwrap_or_else(
                || json!({ "status": "checkout", "amountUsd": "25.000000", "url": PAY, "expiresAt": "2026-10-26T19:00:00.000Z" }),
            ),
        ),
        ("PUT", "/v1/billing/auto-topup") => answer(
            200,
            json!({ "autoTopup": stand.topup.clone().unwrap_or_else(|| topup(true, None)) }),
        ),
        ("POST", "/v1/billing/plan/change") => answer(200, stand.change.clone().unwrap_or(Value::Null)),
        ("DELETE", "/v1/billing/plan/change") => answer(200, json!({ "plan": team_monthly(None) })),
        ("GET", "/v1/billing/plans") => answer(200, stand.plans.clone().unwrap_or_else(company_plans)),
        ("PUT", "/v1/billing/after-the-fact") => answer(
            200,
            json!({ "afterTheFact": stand.after.clone().unwrap_or_else(|| after(true, Value::Null)) }),
        ),
        ("GET", "/v1/billing/receipts") => answer(
            200,
            json!({ "receipts": stand.receipts.clone().unwrap_or_else(receipts) }),
        ),
        ("GET", "/v1/billing/receipts/pi_1") => answer(200, json!({ "url": RECEIPT })),
        ("GET", "/v1/billing/receipts/pi_2") => answer(
            409,
            json!({ "error": { "message": "this payment has no receipt", "type": "invalid_request_error", "code": "no_receipt" } }),
        ),
        ("GET", "/v1/billing/statement") => {
            let period = query.strip_prefix("period=").unwrap_or("2026-10");
            answer(200, statement(period))
        }
        ("GET", "/v1/billing/statements") => answer(
            200,
            json!({ "statements": stand.statements.clone().unwrap_or_else(statements) }),
        ),
        ("POST", "/v1/chat/sessions") => answer(
            200,
            json!({ "id": "chs_1", "title": null, "mode": "chat", "model": "nexus/auto", "createdAt": "2026-10-22T00:00:00.000Z", "updatedAt": "2026-10-22T00:00:00.000Z" }),
        ),
        ("POST", "/v1/chat/sessions/chs_1/messages") => answer(
            402,
            json!({ "error": {
                "message": "this organization has an unpaid Nexus invoice for 2026-09; an owner can pay it in billing",
                "type": "billing_error",
                "code": "payment_past_due",
                "nexus": { "period": "2026-09", "since": "2026-10-01T01:00:00.000Z", "session": "chs_1", "message": "chm_1" },
            } }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

fn gateway(stand: Stand) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let stand = Arc::new(stand);
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let (log, reads, stand) = (log.clone(), reads.clone(), stand.clone());
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let query = request.uri().query().unwrap_or("").to_string();
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default();
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query: query.clone(),
                        body,
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&stand, &reads, &method, &path, &query)
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            ready.send(listener.local_addr().expect("addr").port()).expect("ready");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    let port = port.recv().expect("port");
    (format!("http://127.0.0.1:{port}"), seen)
}

pub(crate) struct Run {
    pub(crate) code: i32,
    pub(crate) out: Vec<String>,
    pub(crate) err: Vec<String>,
    pub(crate) opened: Vec<String>,
    pub(crate) seen: Vec<Seen>,
}

impl Run {
    pub(crate) fn calls(&self, path: &str) -> usize {
        self.seen.iter().filter(|call| call.path == path).count()
    }
}

pub(crate) fn run_with(stand: Stand, can_open: bool, argv: &[&str]) -> Run {
    run_with_input(stand, can_open, argv, "")
}

fn run_with_input(stand: Stand, can_open: bool, argv: &[&str], stdin: &str) -> Run {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-billing-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let (url, seen) = gateway(stand);
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "kind": "cli", "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        can_open,
        stdin: stdin.into(),
        ..Captured::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir,
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let seen = seen.lock().expect("seen").clone();
    Run {
        code,
        out: capture.0.out.borrow().clone(),
        err: capture.0.err.borrow().clone(),
        opened: capture.0.opened.borrow().clone(),
        seen,
    }
}

pub(crate) fn run(stand: Stand, argv: &[&str]) -> Run {
    run_with(stand, false, argv)
}

#[test]
fn delegated_credits_use_stdin_without_a_card_or_browser_and_keep_the_token_out_of_output() {
    let bought =
        json!({ "status": "credited", "amountUsd": "10.000000", "credits": credits("10.000000", "10.000000") });
    let result = run_with_input(
        Stand {
            billing: vec![
                json!({ "mode": "test", "sharedPaymentTokens": true, "customer": null, "paymentMethod": null }),
            ],
            buy: Some(bought.clone()),
            ..Stand::default()
        },
        true,
        &["billing", "credits", "buy", "10", "--payment-token-stdin", "--json"],
        "spt_agent_wallet\n",
    );
    assert_eq!(result.code, 0, "{:?}", result.err);
    assert_eq!(
        serde_json::from_str::<Value>(&result.out.join("\n")).expect("JSON"),
        bought
    );
    let sent = result.seen.iter().find(|call| call.method == "POST").expect("purchase");
    assert_eq!(
        serde_json::from_str::<Value>(&sent.body).expect("body"),
        json!({ "amountUsd": "10", "paymentToken": "spt_agent_wallet" })
    );
    assert!(result.opened.is_empty());
    assert!(!result.out.join("\n").contains("spt_agent_wallet"));
    assert!(!result.err.join("\n").contains("spt_agent_wallet"));
}

#[test]
fn delegated_credits_refuse_an_older_gateway_before_it_can_charge_another_card() {
    let result = run_with_input(
        Stand {
            billing: vec![billing(visa(), Value::Null)],
            ..Stand::default()
        },
        true,
        &["billing", "credits", "buy", "10", "--payment-token-stdin", "--json"],
        "spt_agent_wallet",
    );
    assert_eq!(result.code, 1);
    assert!(result.seen.iter().all(|call| call.method == "GET"));
    assert!(result.opened.is_empty());
}

pub(crate) fn parsed(lines: &[String]) -> Value {
    serde_json::from_str(&lines.join("\n")).expect("json")
}

#[test]
fn shows_the_mode_and_the_card_and_passes_the_answer_through() {
    let stand = Stand {
        billing: vec![billing(visa(), Value::Null)],
        ..Stand::default()
    };
    let shown = run(stand.clone(), &["billing"]);
    assert_eq!(shown.code, 0);
    assert_eq!(
        shown.out,
        [
            "Billing · Stripe test mode: no real charges",
            "",
            "  Card  Visa ending in 4242, expires 04/2099",
            "",
            "Replace the card: nexus billing card add. This month so far: nexus billing statement. Receipts: nexus billing receipts.",
        ]
    );
    let json = run(stand, &["billing", "show", "--json"]);
    assert_eq!(parsed(&json.out), billing(visa(), Value::Null));
}

#[test]
fn shows_both_plan_allowances_in_the_billing_summary() {
    let mut answer = billing(visa(), Value::Null);
    answer["planAllowance"] = json!({
        "resetAt": "2026-12-01T00:00:00Z",
        "ai": { "includedUsd": "10", "usedUsd": "3", "leftUsd": "7" },
        "cloud": { "includedHours": 150, "usedHours": 12, "leftHours": 138 }
    });
    let shown = run(
        Stand {
            billing: vec![answer],
            ..Stand::default()
        },
        &["billing", "show"],
    );
    assert_eq!(shown.code, 0);
    let output = shown.out.join("\n");
    assert!(output.contains("12.00 h used · 138.00 h left"), "{output}");
    assert!(output.contains("$3.00 used · $7.00 left"), "{output}");
    assert!(output.contains("Allowance resets"), "{output}");
}

#[test]
fn names_an_unpaid_month_when_model_calls_stop_and_its_invoice() {
    let mut stopped = billing(
        Value::Null,
        json!({ "since": "2026-10-01T01:00:00.000Z", "period": "2026-09", "blockedFrom": "2026-10-08T01:00:00.000Z" }),
    );
    stopped["mode"] = json!("live");
    let shown = run(
        Stand {
            billing: vec![stopped],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.code, 0);
    assert_eq!(
        shown.out[..4],
        [
            "Billing · Stripe live mode",
            "",
            "✕ The invoice for September 2026 is unpaid. Model calls stopped on 2026-10-08 01:00 UTC and start again once it is paid.",
            "  → Pay the invoice on Stripe: https://invoice.stripe.test/i_1",
        ]
    );
    assert!(
        shown
            .out
            .contains(&"  Card  none: add one to buy Nexus credits or to pay after the fact".to_string())
    );
    let later = run(
        Stand {
            billing: vec![billing(
                visa(),
                json!({ "since": "2099-01-01T00:00:00.000Z", "period": "2098-12", "blockedFrom": "2099-01-08T00:00:00.000Z" }),
            )],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(
        later.out[2],
        "! The invoice for December 2098 is unpaid. Model calls stop on 2099-01-08 00:00 UTC unless it is paid."
    );
    assert_eq!(later.out[3], "  → Its invoice: nexus billing statements");
}

#[test]
fn says_plainly_when_billing_is_off_or_missing_and_refuses_a_card_there() {
    const MESSAGE: &str = "Stripe needs STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET on Gateway";
    let off = Stand {
        off: Some((503, Some("not_configured"), MESSAGE)),
        ..Stand::default()
    };
    let shown = run(off.clone(), &["billing"]);
    assert_eq!(shown.code, 0);
    assert_eq!(
        shown.out,
        [
            "! Billing is off on this Gateway, so it charges nothing.".to_string(),
            format!("  {MESSAGE}"),
        ]
    );
    let json = run(off.clone(), &["billing", "statements", "--json"]);
    assert_eq!(json.code, 0);
    assert_eq!(
        parsed(&json.out),
        json!({ "enabled": false, "reason": "not_configured", "message": MESSAGE })
    );
    let missing = run(
        Stand {
            off: Some((404, None, "no such endpoint")),
            ..Stand::default()
        },
        &["billing", "statement"],
    );
    assert_eq!(missing.code, 0);
    assert_eq!(
        missing.out,
        ["! This Gateway has no billing API (the TypeScript gateway, or an older Rust one), so it charges nothing."]
    );
    let card = run(off, &["billing", "card", "add"]);
    assert_eq!(card.code, 1);
    assert!(
        card.err
            .join("\n")
            .contains("Billing is off on this Gateway, so it takes no card.")
    );
    assert!(card.seen.iter().all(|call| call.method != "POST"));
    let member = run(
        Stand {
            off: Some((403, None, "a member cannot see the organization's billing")),
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(member.code, 1);
    assert!(member.err.join("\n").contains("Billing is for owners"));
}

#[test]
fn opens_stripes_portal_or_only_prints_it_and_refuses_it_where_billing_is_off() {
    let opened = run_with(Stand::default(), true, &["billing", "portal"]);
    assert_eq!(opened.code, 0, "{:?}", opened.err);
    assert_eq!(opened.opened, [PORTAL]);
    assert_eq!(
        opened.out,
        [
            format!("› Stripe's billing portal: {PORTAL}"),
            "  Opened your browser. The link expires within minutes.".to_string(),
        ]
    );
    let json = run_with(Stand::default(), true, &["billing", "portal", "--json", "--no-browser"]);
    assert_eq!(json.code, 0);
    assert!(json.opened.is_empty());
    assert_eq!(parsed(&json.out), json!({ "url": PORTAL }));
    assert_eq!(
        json.err,
        [
            format!("› Stripe's billing portal: {PORTAL}"),
            "  Open the link in a browser now: it expires within minutes.".to_string(),
        ]
    );
    let off = run(
        Stand {
            off: Some((503, Some("not_configured"), "Stripe needs STRIPE_SECRET_KEY")),
            ..Stand::default()
        },
        &["billing", "portal"],
    );
    assert_eq!(off.code, 1);
    assert!(
        off.err
            .join("\n")
            .contains("Billing is off on this Gateway, so it has no portal.")
    );
}

#[test]
fn opens_stripe_checkout_and_waits_until_stripe_saves_another_card() {
    let before = billing(visa(), Value::Null);
    let after = billing(amex(), Value::Null);
    let added = run_with(
        Stand {
            billing: vec![before.clone(), before.clone(), before.clone(), after.clone()],
            ..Stand::default()
        },
        true,
        &["billing", "card", "add"],
    );
    assert_eq!(added.code, 0, "{:?}", added.err);
    assert_eq!(added.opened, [CHECKOUT]);
    assert_eq!(
        added.out,
        [
            format!("› Replace the card on Stripe's page: {CHECKOUT}"),
            "  Opened your browser. Waiting for Stripe to save the card…".to_string(),
            "✓ Saved: American Express ending in 0005, expires 12/2098.".to_string(),
        ]
    );
    assert_eq!(added.calls("/v1/billing"), 4);
    let json = run_with(
        Stand {
            billing: vec![billing(Value::Null, Value::Null), after.clone()],
            ..Stand::default()
        },
        true,
        &["billing", "card", "add", "--json", "--no-browser"],
    );
    assert_eq!(json.code, 0);
    assert!(json.opened.is_empty());
    assert_eq!(parsed(&json.out), after);
    assert_eq!(json.err[0], format!("› Add a card on Stripe's page: {CHECKOUT}"));
}

#[test]
fn returns_at_once_with_no_wait_and_gives_up_after_ten_minutes_of_polls() {
    let now = run(Stand::default(), &["billing", "card", "add", "--no-wait", "--json"]);
    assert_eq!(now.code, 0);
    assert_eq!(
        parsed(&now.out),
        json!({ "url": CHECKOUT, "expiresAt": "2026-10-23T19:00:00.000Z" })
    );
    assert_eq!(
        now.err[1],
        "  Open the link before 2026-10-23 19:00 UTC; nexus billing shows the card once Stripe saves it."
    );
    let late = run(Stand::default(), &["billing", "card", "add"]);
    assert_eq!(late.code, 1);
    assert_eq!(late.calls("/v1/billing"), 301);
    assert!(late.err.join("\n").contains("Stripe has not saved the card yet."));
}

#[test]
fn shows_the_nexus_credits_left_and_how_to_buy_some() {
    let bought = with_credits(billing(visa(), Value::Null), credits("40.000000", "31.500000"));
    let shown = run(
        Stand {
            billing: vec![bought.clone()],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.out[3], "  Nexus credits  $31.50 left of $40.00 bought");
    let detail = run(
        Stand {
            billing: vec![bought.clone()],
            ..Stand::default()
        },
        &["billing", "credits"],
    );
    assert_eq!(detail.code, 0, "{:?}", detail.err);
    assert_eq!(
        detail.out,
        [
            "Nexus credits · the provider's price plus 5%",
            "",
            "  Left    $31.50",
            "  Bought  $40.00",
            "  Spent   $0.00",
            "  Held    $0.00  calls running now",
            "",
            "Credits never expire. Buy more: nexus billing credits buy <usd> (from $25.00).",
        ]
    );
    let json = run(
        Stand {
            billing: vec![bought],
            ..Stand::default()
        },
        &["billing", "credits", "--json"],
    );
    assert_eq!(
        parsed(&json.out),
        json!({ "credits": credits("40.000000", "31.500000") })
    );
    let none = run(Stand::default(), &["billing", "credits"]);
    assert_eq!(
        none.out[0],
        "No Nexus credits yet. Nexus-run AI spends them after the free start, at the provider's price plus Nexus's rate."
    );
    let mut priced = billing(visa(), Value::Null);
    priced["pricing"] = pricing();
    let rated = run(
        Stand {
            billing: vec![priced],
            ..Stand::default()
        },
        &["billing", "credits"],
    );
    assert_eq!(
        rated.out[0],
        "No Nexus credits yet. Nexus-run AI spends them after the free start, at the provider's price plus 5%."
    );
}

#[test]
fn buys_credits_with_the_payment_method_on_file_at_once() {
    let mut on_file = billing(visa(), Value::Null);
    on_file["defaultPaymentMethod"] = json!({ "type": "card", "brand": "visa", "last4": "4242" });
    let stand = Stand {
        billing: vec![on_file],
        buy: Some(
            json!({ "status": "credited", "amountUsd": "25.000000", "credits": credits("25.000000", "25.000000") }),
        ),
        ..Stand::default()
    };
    let dry = run(stand.clone(), &["billing", "credits", "buy", "25", "--dry-run"]);
    assert_eq!(dry.code, 0);
    assert_eq!(
        dry.out,
        [
            "› Would buy $25.00 of Nexus credits, charged to Visa ending in 4242.",
            "  Nothing is charged without dropping --dry-run.",
        ]
    );
    assert_eq!(dry.calls("/v1/billing/credits"), 0);
    let bought = run(stand.clone(), &["billing", "credits", "buy", "25"]);
    assert_eq!(bought.code, 0, "{:?}", bought.err);
    assert_eq!(bought.out, ["✓ Bought $25.00 of Nexus credits. $25.00 left."]);
    let post = bought
        .seen
        .iter()
        .find(|call| call.path == "/v1/billing/credits")
        .expect("a purchase");
    assert_eq!(
        serde_json::from_str::<Value>(&post.body).expect("body"),
        json!({ "amountUsd": "25" })
    );
    let json = run(stand, &["billing", "credits", "buy", "25", "--json"]);
    assert_eq!(parsed(&json.out)["status"], "credited");
}

#[test]
fn buys_credits_on_stripe_checkout_and_waits_until_stripe_credits_them() {
    let before = billing(Value::Null, Value::Null);
    let after = with_credits(before.clone(), credits("25.000000", "25.000000"));
    let bought = run_with(
        Stand {
            billing: vec![before.clone(), before.clone(), after.clone()],
            ..Stand::default()
        },
        true,
        &["billing", "credits", "buy", "25"],
    );
    assert_eq!(bought.code, 0, "{:?}", bought.err);
    assert_eq!(bought.opened, [PAY]);
    assert_eq!(
        bought.out,
        [
            format!("› Pay $25.00 for Nexus credits on Stripe's page: {PAY}"),
            "  Opened your browser. Waiting for Stripe to take the payment…".to_string(),
            "✓ Bought $25.00 of Nexus credits. $25.00 left.".to_string(),
        ]
    );
    assert_eq!(bought.calls("/v1/billing"), 3);
    let now = run(
        Stand::default(),
        &["billing", "credits", "buy", "25", "--no-wait", "--json"],
    );
    assert_eq!(now.code, 0);
    assert_eq!(parsed(&now.out)["url"], PAY);
    assert_eq!(
        now.err[1],
        "  Open the link before 2026-10-26 19:00 UTC; nexus billing credits shows the credits once Stripe takes the payment."
    );
    let late = run(Stand::default(), &["billing", "credits", "buy", "25"]);
    assert_eq!(late.code, 1);
    assert!(late.err.join("\n").contains("Stripe has not sent the payment yet."));
    let off = run(
        Stand {
            off: Some((503, Some("not_configured"), "Stripe needs STRIPE_SECRET_KEY")),
            ..Stand::default()
        },
        &["billing", "credits", "buy", "25"],
    );
    assert_eq!(off.code, 1);
    assert!(
        off.err
            .join("\n")
            .contains("Billing is off on this Gateway, so it sells no credits.")
    );
}

#[test]
fn shows_sets_and_turns_off_the_auto_top_up() {
    let mut on = billing(visa(), Value::Null);
    on["autoTopup"] = topup(true, None);
    let shown = run(
        Stand {
            billing: vec![on.clone()],
            ..Stand::default()
        },
        &["billing"],
    );
    assert!(
        shown
            .out
            .contains(&"  Auto top-up  on: adds $25.00 when credits fall below $10.00".to_string()),
        "{:?}",
        shown.out
    );
    let mut declined = billing(visa(), Value::Null);
    declined["autoTopup"] = topup(false, Some("declined"));
    let off = run(
        Stand {
            billing: vec![declined],
            ..Stand::default()
        },
        &["billing", "auto-topup"],
    );
    assert_eq!(off.code, 0, "{:?}", off.err);
    assert_eq!(
        off.out,
        [
            "Auto top-up · off",
            "",
            "! Auto top-up is off: the card was declined.",
            "",
            "  Below  $10.00",
            "  Adds   $25.00  charged to the payment method on file",
            "  Today  1 of 3 top-ups, $25.00 of $500.00",
            "",
            "Turn it on: nexus billing auto-topup set --below <usd> --add <usd>",
        ]
    );
    let set = run(
        Stand::default(),
        &["billing", "auto-topup", "set", "--below", "10", "--add", "25"],
    );
    assert_eq!(set.code, 0, "{:?}", set.err);
    assert_eq!(
        set.out,
        ["✓ Auto top-up is on: Nexus adds $25.00 when credits fall below $10.00."]
    );
    let put = set.seen.iter().find(|call| call.method == "PUT").expect("a PUT");
    assert_eq!(put.path, "/v1/billing/auto-topup");
    assert_eq!(
        serde_json::from_str::<Value>(&put.body).expect("body"),
        json!({ "enabled": true, "belowUsd": "10", "addUsd": "25" })
    );
    let turned_off = run(
        Stand {
            topup: Some(topup(false, None)),
            ..Stand::default()
        },
        &["billing", "auto-topup", "off"],
    );
    assert_eq!(turned_off.out, ["✓ Auto top-up is off."]);
    let dry = run(Stand::default(), &["billing", "auto-topup", "off", "--dry-run"]);
    assert_eq!(dry.out[0], "› Would turn auto top-up off.");
    assert!(dry.seen.iter().all(|call| call.method != "PUT"));
    let none = run(Stand::default(), &["billing", "auto-topup", "--json"]);
    assert_eq!(parsed(&none.out), json!({ "autoTopup": null }));
}

#[test]
fn prints_a_months_statement_and_the_closed_months() {
    let month = run(Stand::default(), &["billing", "statement", "--period", "2026-09"]);
    assert_eq!(month.code, 0);
    assert_eq!(
        month.seen.last().map(|call| call.query.as_str()),
        Some("period=2026-09")
    );
    assert_eq!(
        month.out,
        [
            "Statement · September 2026 (UTC) · Payment failed",
            "",
            "  Charge                   Quantity     Rate        Amount",
            "· Gateway calls                1234   $0.001        $1.234",
            "· Fee on provider cost   $12.345678     2.5%   $0.30864195",
            "",
            "  Carried in     $0.004",
            "  Total          $1.54664195",
            "  To charge      $1.54",
            "  Carries over   $0.00664195",
            "  Provider cost  $12.345678  billed by the providers, not by Nexus",
            "  Invoice        https://invoice.stripe.test/i_1",
            "",
            "Under $0.50 is not invoiced: it carries over to the next month, with what is below a cent.",
        ]
    );
    let months = run(Stand::default(), &["billing", "statements"]);
    assert_eq!(
        months.out,
        [
            "  Month                  Total   State            Invoice",
            "· September 2026   $1.54664195   Payment failed   https://invoice.stripe.test/i_1",
            "· August 2026            $0.31   Carried over",
            "",
            "One month in full: nexus billing statement --period YYYY-MM",
        ]
    );
    let none = run(
        Stand {
            statements: Some(json!([])),
            ..Stand::default()
        },
        &["billing", "statements"],
    );
    assert_eq!(
        none.out,
        [
            "No month has closed yet. On a contract, a month closes an hour after it ends (UTC); then Stripe charges the card on file."
        ]
    );
}

#[test]
fn refuses_bad_usage_before_anything_is_sent() {
    for argv in [
        vec!["billing", "statement", "--period", "2026-13"],
        vec!["billing", "statement", "--period", "September"],
        vec!["billing", "card"],
        vec!["billing", "plan"],
        vec!["billing", "nope"],
        vec!["billing", "credits", "sell"],
        vec!["billing", "credits", "buy"],
        vec!["billing", "credits", "buy", "25.005"],
        vec!["billing", "credits", "buy", "$25"],
        vec!["billing", "auto-topup", "set", "--below", "ten", "--add", "25"],
        vec!["billing", "auto-topup", "nope"],
        vec!["billing", "after-the-fact", "nope"],
        vec!["billing", "after-the-fact", "on", "--limit", "ten"],
        vec!["billing", "after-the-fact", "on", "--limit"],
        vec!["billing", "after-the-fact", "limit"],
        vec!["billing", "after-the-fact", "limit", "25.005"],
        vec!["billing", "receipts", "nope"],
        vec!["billing", "receipts", "open"],
    ] {
        let refused = run(Stand::default(), &argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn names_who_can_pay_when_a_model_call_is_refused_for_an_unpaid_month() {
    let refused = run(Stand::default(), &["chat", "hello", "--new", "--json"]);
    assert_eq!(refused.code, 1);
    assert_eq!(
        parsed(&refused.err),
        json!({ "error": {
            "message": "this organization has an unpaid Nexus invoice for 2026-09; an owner can pay it in billing",
            "hint": "Only an owner can pay it: `nexus billing` shows the unpaid month and its invoice.",
            "code": "payment_past_due",
        } })
    );
}

pub(crate) fn with_v2(after_the_fact: Value) -> Value {
    let mut answer = with_credits(billing(visa(), Value::Null), credits("40.000000", "31.500000"));
    answer["freeStart"] = json!({ "grantedUsd": "1.000000", "remainingUsd": "0.400000" });
    answer["afterTheFact"] = after_the_fact;
    answer["pricing"] = pricing();
    answer
}

/// Until when the free start lasts and which models it pays for, and once it expired: as the TypeScript CLI.
#[test]
fn shows_until_when_the_free_start_lasts_and_once_it_expired() {
    let dated = |remaining: &str, expired: Value| {
        let mut answer = with_v2(Value::Null);
        answer["freeStart"] = json!({
            "grantedUsd": "1.000000", "remainingUsd": remaining, "expiresAt": "2026-12-03T09:30:00.000Z", "expiredUsd": expired,
        });
        answer["pricing"]["freeStartMaxOutputUsd"] = json!("5.000000");
        answer
    };
    for (answer, row) in [
        (
            dated("0.400000", Value::Null),
            "  Free start      $0.40 left of $1.00 until 2026-12-03; cheaper models only (up to $5.00 per million output tokens)",
        ),
        (
            dated("0.000000", json!("0.400000")),
            "  Free start      $0.00 left of $1.00; the free start expired on 2026-12-03",
        ),
    ] {
        let shown = run(
            Stand {
                billing: vec![answer],
                ..Stand::default()
            },
            &["billing"],
        );
        assert_eq!(shown.code, 0, "{:?}", shown.err);
        assert!(shown.out.iter().any(|line| line == row), "{:?}", shown.out);
    }
}

#[test]
fn shows_the_free_start_paying_after_the_fact_and_the_rate() {
    let failed = json!({ "at": "2026-11-02T10:00:00.000Z", "reason": "declined" });
    let shown = run(
        Stand {
            billing: vec![with_v2(after(true, failed))],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.code, 0, "{:?}", shown.err);
    assert_eq!(
        shown.out,
        [
            "Billing · Stripe test mode: no real charges",
            "",
            "  Card            Visa ending in 4242, expires 04/2099",
            "  Free start      $0.40 left of $1.00",
            "  Nexus credits   $31.50 left of $40.00 bought",
            "  After the fact  on: up to $25.00 a month; $3.20 used this month, $1.10 owed. The last charge of $1.10 didn't go through: the card was declined",
            "  Rate            the provider's price plus 5% on Nexus-run AI; your own keys are free",
            "",
            "Replace the card: nexus billing card add. This month so far: nexus billing statement. Receipts: nexus billing receipts.",
        ]
    );
    let off = run(
        Stand {
            billing: vec![with_v2(Value::Null)],
            ..Stand::default()
        },
        &["billing"],
    );
    assert!(off.out.contains(&"  After the fact  off".to_string()), "{:?}", off.out);
}

#[test]
fn shows_turns_on_sets_the_limit_of_and_turns_off_paying_after_the_fact() {
    let failed = json!({ "at": "2026-11-02T10:00:00.000Z", "reason": "declined" });
    let shown = run(
        Stand {
            billing: vec![with_v2(after(true, failed))],
            ..Stand::default()
        },
        &["billing", "after-the-fact"],
    );
    assert_eq!(shown.code, 0, "{:?}", shown.err);
    assert_eq!(
        shown.out,
        [
            "Paying after the fact · on",
            "",
            "✕ The last charge of $1.10 didn't go through: the card was declined (2026-11-02 10:00 UTC). Nexus-run AI the card pays is paused until a charge goes through. Credits and your own keys still work.",
            "Fix payment: nexus billing card add",
            "",
            "  Limit  $25.00 a month",
            "  Used   $3.20 this month",
            "  Owed   $1.10  charged when it reaches $10.00; less carries over",
            "  Held   $0.50  calls running now",
            "  Left   $21.30 this month",
            "",
            "Change the limit: nexus billing after-the-fact limit <usd>. Turn it off: nexus billing after-the-fact off",
        ]
    );
    let none = run(Stand::default(), &["billing", "after-the-fact", "--json"]);
    assert_eq!(parsed(&none.out), json!({ "afterTheFact": null }));
    let never = run(Stand::default(), &["billing", "after-the-fact"]);
    assert_eq!(
        never.out[2],
        "Turn it on: nexus billing after-the-fact on [--limit <usd>]"
    );
    let on = run(Stand::default(), &["billing", "after-the-fact", "on", "--limit", "40"]);
    assert_eq!(on.code, 0, "{:?}", on.err);
    assert_eq!(
        on.out,
        ["✓ Paying after the fact is on: the card on file pays up to $25.00 a month."]
    );
    let put = on.seen.iter().find(|call| call.method == "PUT").expect("a PUT");
    assert_eq!(put.path, "/v1/billing/after-the-fact");
    assert_eq!(
        serde_json::from_str::<Value>(&put.body).expect("body"),
        json!({ "enabled": true, "limitUsd": "40" })
    );
    let dry = run(
        Stand::default(),
        &["billing", "after-the-fact", "limit", "50", "--dry-run"],
    );
    assert_eq!(
        dry.out,
        [
            "› Would set the usage limit to $50.00 a month, and leave paying after the fact off.",
            "  Nothing changes without dropping --dry-run.",
        ]
    );
    assert!(!dry.seen.iter().any(|call| call.method == "PUT"));
    let dry_json = run(
        Stand::default(),
        &["billing", "after-the-fact", "on", "--dry-run", "--json"],
    );
    assert_eq!(
        parsed(&dry_json.out),
        json!({ "dryRun": true, "request": { "enabled": true } })
    );
    let turned_off = run(
        Stand {
            after: Some(after(false, Value::Null)),
            ..Stand::default()
        },
        &["billing", "after-the-fact", "off"],
    );
    assert_eq!(turned_off.out, ["✓ Paying after the fact is off."]);
    let put = turned_off.seen.iter().find(|call| call.method == "PUT").expect("a PUT");
    assert_eq!(
        serde_json::from_str::<Value>(&put.body).expect("body"),
        json!({ "enabled": false })
    );
    let no_card = run(
        Stand {
            off: Some((409, Some("no_payment_method"), "add a payment method first")),
            ..Stand::default()
        },
        &["billing", "after-the-fact", "on"],
    );
    assert_eq!(no_card.code, 1);
    assert!(
        no_card
            .err
            .join("\n")
            .contains("Add a card first: nexus billing card add"),
        "{:?}",
        no_card.err
    );
    let off = run(
        Stand {
            off: Some((503, Some("not_configured"), "Stripe needs STRIPE_SECRET_KEY")),
            ..Stand::default()
        },
        &["billing", "after-the-fact", "off"],
    );
    assert_eq!(off.code, 1);
    assert!(
        off.err
            .join("\n")
            .contains("Billing is off on this Gateway, so it takes no payment after the fact.")
    );
}

/// Each charge phase Gateway names, in `nexus billing`'s row and in `after-the-fact`'s line, as the TypeScript CLI says
/// it (kit's `afterTheFactNotice`). Owed is the unpaid usage: a retry never reads "$0.00".
#[test]
fn says_each_after_the_fact_charge_phase() {
    let failed = |reason: &str| json!({ "at": "2026-11-12T10:00:00.000Z", "reason": reason });
    let phase = |patch: Value| {
        let mut row = after(true, Value::Null);
        for (key, value) in patch.as_object().expect("a patch") {
            row[key] = value.clone();
        }
        row
    };
    let on = "on: up to $25.00 a month; $3.20 used this month";
    let cases = [
        (
            phase(json!({ "phase": "on", "chargeUsd": null, "unpaidUsd": "1.100000" })),
            format!("{on}, $1.10 owed"),
            None,
        ),
        (
            phase(
                json!({ "phase": "charging", "pending": true, "chargeUsd": "10.400000", "unpaidUsd": "10.400000", "owedUsd": "0.000000" }),
            ),
            format!("{on}, $10.40 owed. A charge of $10.40 is going through"),
            Some("› A charge of $10.40 is going through."),
        ),
        (
            phase(
                json!({ "phase": "retrying", "pending": true, "chargeUsd": "12.000000", "unpaidUsd": "12.000000", "owedUsd": "0.000000", "failed": failed("declined") }),
            ),
            format!("{on}, $12.00 owed. The last charge of $12.00 didn't go through. Nexus is trying it again now"),
            Some(
                "✕ The last charge of $12.00 didn't go through. Nexus is trying it again now (2026-11-12 10:00 UTC). Nexus-run AI the card pays stays paused until it's paid. Credits and your own keys still work.",
            ),
        ),
        (
            phase(
                json!({ "phase": "needs_confirmation", "pending": true, "chargeUsd": "12.000000", "unpaidUsd": "12.000000", "owedUsd": "0.000000", "failed": failed("authentication_required") }),
            ),
            format!("{on}, $12.00 owed. The charge of $12.00 needs the cardholder's confirmation"),
            Some(
                "✕ The charge of $12.00 needs the cardholder's confirmation (2026-11-12 10:00 UTC). Until then, Nexus-run AI the card pays is paused. Credits and your own keys still work.",
            ),
        ),
        (
            phase(json!({ "phase": "disputed", "unpaidUsd": "12.000000", "failed": failed("disputed") })),
            format!("{on}, $12.00 owed. A payment on the card was disputed"),
            Some(
                "✕ A payment on the card was disputed (2026-11-12 10:00 UTC). Nexus-run AI the card pays is paused until an owner adds another card. Credits and your own keys still work.",
            ),
        ),
        (
            phase(json!({ "phase": "failed", "unpaidUsd": "12.000000", "failed": failed("card_expired") })),
            format!("{on}, $12.00 owed. The last charge of $12.00 didn't go through"),
            Some(
                "✕ The last charge of $12.00 didn't go through (2026-11-12 10:00 UTC). Nexus-run AI the card pays is paused until a charge goes through. Credits and your own keys still work.",
            ),
        ),
        (
            phase(
                json!({ "phase": "failed", "enabled": false, "unpaidUsd": "3.200000", "owedUsd": "3.200000", "failed": failed("no_payment_method") }),
            ),
            "off. The final charge of $3.20 didn't go through: no payment method is on file".to_string(),
            Some(
                "! The final charge of $3.20 didn't go through: no payment method is on file (2026-11-12 10:00 UTC). Nothing else runs on the card. An owner fixes the card so the charge goes through.",
            ),
        ),
        (
            phase(json!({ "phase": "off_owing", "enabled": false, "unpaidUsd": "3.200000", "owedUsd": "3.200000" })),
            "off. A final charge of $3.20 runs once open calls settle".to_string(),
            Some("› A final charge of $3.20 runs once open calls settle."),
        ),
        (
            phase(json!({ "phase": "off", "enabled": false, "unpaidUsd": "0.000000", "owedUsd": "0.000000" })),
            "off".to_string(),
            None,
        ),
        // An older Gateway sends no phase: the same phase from the other fields, and owedUsd for what is owed.
        (
            phase(json!({ "pending": true, "owedUsd": "0.000000", "failed": failed("declined") })),
            format!("{on}, $0.00 owed. The last charge didn't go through. Nexus is trying it again now"),
            Some(
                "✕ The last charge didn't go through. Nexus is trying it again now (2026-11-12 10:00 UTC). Nexus-run AI the card pays stays paused until it's paid. Credits and your own keys still work.",
            ),
        ),
    ];
    for (row, summary, line) in cases {
        let stand = Stand {
            billing: vec![with_v2(row.clone())],
            ..Stand::default()
        };
        let shown = run(stand.clone(), &["billing"]);
        assert_eq!(shown.code, 0, "{:?}", shown.err);
        let expected = format!("  After the fact  {summary}");
        assert!(shown.out.contains(&expected), "{expected}\n{:#?}", shown.out);
        let lines = run(stand, &["billing", "after-the-fact"]);
        assert_eq!(lines.code, 0, "{:?}", lines.err);
        match line {
            Some(line) => assert_eq!(lines.out[2], line, "{:#?}", lines.out),
            None => assert!(lines.out[2].starts_with("  Limit"), "{:#?}", lines.out),
        }
    }
}

/// `limit <usd>` changes only the limit: off stays off, on stays on, and no setting yet stays off. Only `on` turns it on.
#[test]
fn a_limit_change_keeps_paying_after_the_fact_on_or_off() {
    for (row, enabled, reply) in [
        (
            after(false, Value::Null),
            false,
            "✓ The usage limit is $25.00 a month. Paying after the fact stays off. Turn it on: nexus billing after-the-fact on",
        ),
        (
            after(true, Value::Null),
            true,
            "✓ Paying after the fact is on: the card on file pays up to $25.00 a month.",
        ),
        (
            Value::Null,
            false,
            "✓ The usage limit is $50.00 a month. Paying after the fact stays off. Turn it on: nexus billing after-the-fact on",
        ),
    ] {
        let changed = run(
            Stand {
                billing: vec![with_v2(row.clone())],
                after: Some(row.clone()),
                ..Stand::default()
            },
            &["billing", "after-the-fact", "limit", "50"],
        );
        assert_eq!(changed.code, 0, "{row}: {:?}", changed.err);
        let put = changed.seen.iter().find(|call| call.method == "PUT").expect("a PUT");
        assert_eq!(
            serde_json::from_str::<Value>(&put.body).expect("body"),
            json!({ "enabled": enabled, "limitUsd": "50" }),
            "{row}"
        );
        assert_eq!(changed.out, [reply], "{row}");
    }
}

/// `on`, and `limit 40` on a setting Gateway answers as on: both get the same reply.
const AFTER_ON_VERBS: [&[&str]; 2] = [
    &["billing", "after-the-fact", "on"],
    &["billing", "after-the-fact", "limit", "40"],
];

/// Gateway refuses turning on with a disputed card; a limit change alone is not refused.
#[test]
fn a_disputed_card_refusal_says_to_add_another_card() {
    let refused = run(
        Stand {
            off: Some((409, Some("card_disputed"), "a payment on the card on file was disputed")),
            ..Stand::default()
        },
        &["billing", "after-the-fact", "on"],
    );
    assert_eq!(refused.code, 1);
    let err = refused.err.join("\n");
    assert!(err.contains("a payment on the card on file was disputed"), "{err}");
    assert!(err.contains("Add another card first: nexus billing card add"), "{err}");
}

#[test]
fn a_setting_on_after_a_dispute_says_to_add_another_card() {
    for argv in AFTER_ON_VERBS {
        let failed = json!({ "at": "2026-11-02T10:00:00.000Z", "reason": "disputed" });
        let answered = run(
            Stand {
                after: Some(after(true, failed)),
                ..Stand::default()
            },
            argv,
        );
        assert_eq!(answered.code, 0, "{argv:?}: {:?}", answered.err);
        assert_eq!(
            answered.out,
            [
                "! Paying after the fact is on, up to $25.00 a month. A payment on the card on file was disputed. Calls on it stop until you add another card: nexus billing card add.",
            ],
            "{argv:?}"
        );
    }
}

#[test]
fn a_setting_on_after_a_failed_charge_never_says_the_card_pays() {
    for argv in AFTER_ON_VERBS {
        // No reason, or an empty one, says no empty ": ." after "failed".
        for (reason, said) in [
            (json!("declined"), ": the card was declined"),
            (json!(null), ""),
            (json!(""), ""),
        ] {
            let failed = json!({ "at": "2026-11-02T10:00:00.000Z", "reason": reason });
            let answered = run(
                Stand {
                    after: Some(after(true, failed)),
                    ..Stand::default()
                },
                argv,
            );
            assert_eq!(answered.code, 0, "{argv:?}: {:?}", answered.err);
            assert_eq!(
                answered.out,
                [format!(
                    "! Paying after the fact is on, up to $25.00 a month. The last charge failed{said}. Calls on the card stop until a charge goes through."
                )],
                "{argv:?} {reason}"
            );
        }
    }
}

#[test]
fn lists_the_receipts_and_opens_one_on_stripe() {
    let listed = run(Stand::default(), &["billing", "receipts"]);
    assert_eq!(listed.code, 0, "{:?}", listed.err);
    assert_eq!(
        listed.out,
        [
            "  Date         Description            Amount   Status   Payment",
            "· 2026-11-02   Usage after the fact   $10.00   Failed   pi_2",
            "· 2026-11-01   Nexus credits          $10.00   Paid     pi_1",
            "",
            "Stripe's receipt of one: nexus billing receipts open <payment>",
        ]
    );
    let json = run(Stand::default(), &["billing", "receipts", "--json"]);
    assert_eq!(parsed(&json.out), json!({ "receipts": receipts() }));
    let none = run(
        Stand {
            receipts: Some(json!([])),
            ..Stand::default()
        },
        &["billing", "receipts"],
    );
    assert_eq!(
        none.out,
        ["No payment yet. Buying credits, a top-up and a charge after the fact each leave a receipt here."]
    );
    let opened = run_with(Stand::default(), true, &["billing", "receipts", "open", "pi_1"]);
    assert_eq!(opened.code, 0, "{:?}", opened.err);
    assert_eq!(opened.opened, [RECEIPT]);
    assert_eq!(
        opened.out,
        [
            format!("› Stripe's receipt: {RECEIPT}"),
            "  Opened your browser.".to_string(),
        ]
    );
    let printed = run_with(
        Stand::default(),
        true,
        &["billing", "receipts", "open", "pi_1", "--json", "--no-browser"],
    );
    assert!(printed.opened.is_empty());
    assert_eq!(parsed(&printed.out), json!({ "url": RECEIPT }));
    assert_eq!(printed.err[1], "  Open the link in a browser.");
    let missing = run(Stand::default(), &["billing", "receipts", "open", "pi_2"]);
    assert_eq!(missing.code, 1);
    assert!(missing.err.join("\n").contains("this payment has no receipt"));
    let month = run(Stand::default(), &["billing", "receipts", "open", "month:2026-09"]);
    assert_eq!(
        month.seen.last().map(|call| call.path.as_str()),
        Some("/v1/billing/receipts/month%3A2026-09")
    );
}

#[test]
fn opens_only_https_links_and_a_local_nexus_ui() {
    assert!(openable(CHECKOUT));
    assert!(openable("http://localhost:3000/app/device?code=BCDF"));
    assert!(openable("http://127.0.0.1:3000/app"));
    assert!(openable("http://[::1]:3000/app"));
    for refused in [
        "http://checkout.stripe.test/c/pay/cs_test_1",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "ftp://checkout.stripe.test/",
        "http://localhost:1@evil.test/",
        "http://localhost.evil.test/",
        "",
        // Parsed as a browser reads it.
        "https://",
        "https://[::1/",
        "https://exa mple.test:99999/",
        "https://user:pass@checkout.stripe.test/",
        "https://checkout.stripe.test/c\n/pay",
        "https://checkout.stripe.test/\u{7}",
        "https://checkout.stripe.test/\0",
        "https:checkout.stripe.test/c/pay",
        "HTTPS://checkout.stripe.test/",
    ] {
        assert!(!openable(refused), "{refused}");
    }
}

/// A link reaches the OS opener as one argument, as it came; on Windows no cmd.exe reads its `&`,
/// `|`, `^` or `%NAME%` (`cmd /c start "" <link>` ran `calc` from `https://x.test/?a=1&calc`).
#[test]
fn a_link_reaches_the_opener_as_one_unchanged_argument() {
    let link = "https://checkout.stripe.test/c/pay?a=1&b=2|calc^x&%USERNAME%&c=%PATH%";
    assert!(openable(link));
    assert_eq!(link_opener("windows", link), LinkOpener::ShellExecute(link));
    assert_eq!(link_opener("macos", link), LinkOpener::Program("open", link));
    assert_eq!(link_opener("linux", link), LinkOpener::Program("xdg-open", link));
    for os in ["windows", "macos", "linux", "freebsd"] {
        match link_opener(os, link) {
            LinkOpener::Program(program, argument) => {
                assert!(!program.contains("cmd") && !program.ends_with("sh"), "{os}: {program}");
                assert_eq!(argument, link, "{os}");
            }
            LinkOpener::ShellExecute(argument) => assert_eq!(argument, link, "{os}"),
        }
    }
}

/// Plans are dormant (`src/plans.rs`, `--features plans`): the default CLI has no plan command, and a plan in Gateway's
/// answer shows no row while the free start keeps its name.
#[cfg(not(feature = "plans"))]
#[test]
fn has_no_plan_command_and_shows_no_plan_while_plans_are_dormant() {
    for argv in [vec!["billing", "plans"], vec!["billing", "plan", "pro"]] {
        let refused = run(Stand::default(), &argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
        assert!(refused.err.join("\n").contains("Unknown command"), "{:?}", refused.err);
    }
    let help = run(Stand::default(), &["billing", "--help"]);
    assert!(!help.out.iter().any(|line| line.contains("plan")), "{:?}", help.out);
    let mut on_plan = with_v2(Value::Null);
    on_plan["plan"] =
        json!({ "plan": "pro", "interval": "month", "status": "active", "seats": 1, "includedUsd": "10.000000" });
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
            .contains(&"  Free start      $0.40 left of $1.00".to_string()),
        "{:?}",
        planned.out
    );
    assert!(
        !planned.out.iter().any(|line| line.contains("Plan")),
        "{:?}",
        planned.out
    );
}

#[test]
fn shows_promotional_credit_separately_from_expired_welcome_credit() {
    let mut answer = with_v2(Value::Null);
    answer["freeStart"] = json!({"grantedUsd":"1", "remainingUsd":"0", "expiredUsd":"1"});
    answer["promotionalCreditUsd"] = json!("2.500001");
    let shown = run(
        Stand {
            billing: vec![answer],
            ..Stand::default()
        },
        &["billing"],
    );
    assert_eq!(shown.code, 0);
    assert!(
        shown
            .out
            .iter()
            .any(|line| line.contains("Promotional credit") && line.contains("$2.500001")),
        "{:?}",
        shown.out
    );
}

#[test]
fn omits_zero_promotional_credit_without_rounding_away_a_positive_balance() {
    for (amount, shown) in [("0", false), ("0.000000", false), ("0.000001", true)] {
        let mut answer = with_v2(Value::Null);
        answer["promotionalCreditUsd"] = json!(amount);
        let output = run(
            Stand {
                billing: vec![answer],
                ..Stand::default()
            },
            &["billing"],
        );
        assert_eq!(output.code, 0);
        assert_eq!(output.out.iter().any(|line| line.contains("Promotional credit")), shown);
    }
}
