//! `nexus budget … org` against a stand-in Gateway (see `budget.test.ts`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, body: &Value) -> Response {
    match (method, path) {
        // $14 stands in for a member, whom the Gateway refuses.
        ("POST", "/v1/orgs/budget") if body["budgetUsd"] == 14.0 => answer(
            403,
            json!({ "error": { "message": "only owners or admins can manage budgets", "code": "role_required" } }),
        ),
        // $13 stands in for a Gateway that ignores `dryRun`: it sets the budget.
        ("POST", "/v1/orgs/budget") if body["budgetUsd"] == 13.0 => {
            answer(200, json!({ "budgetUsd": body["budgetUsd"], "period": body["period"] }))
        }
        ("POST", "/v1/orgs/budget") if body["dryRun"] == true => answer(
            200,
            json!({
                "dryRun": true,
                "before": { "budgetUsd": 2000.0, "period": "month" },
                "after": { "budgetUsd": body["budgetUsd"], "period": body["period"] },
            }),
        ),
        ("POST", "/v1/orgs/budget") => answer(200, json!({ "budgetUsd": body["budgetUsd"], "period": body["period"] })),
        ("GET", "/v1/spend") => answer(
            200,
            json!({
                "budgetUsd": 50.0, "spentUsd": 900.0, "reservedUsd": 1.5, "budgetPeriod": "week",
                "periodSpentUsd": 12.0, "remainingUsd": 36.5, "resetsAt": "2026-11-02T00:00:00.000Z",
                "byProduct": [], "recent": [],
            }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

fn gateway() -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        body: body.clone(),
                    });
                    route(&method, &path, &body)
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

struct Run {
    code: i32,
    out: String,
    err: String,
    seen: Vec<Seen>,
}

fn run(argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-budget-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": "nx_0123456789abcdef", "kind": "api" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
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
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        seen,
    }
}

fn posts(run: &Run) -> Vec<&Seen> {
    run.seen.iter().filter(|call| call.method == "POST").collect()
}

#[test]
fn set_org_sends_the_amount_and_the_period() {
    let set = run(&["budget", "set", "org", "50/week"]);
    assert_eq!(set.code, 0, "{}", set.out);
    let sent = posts(&set);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].path, "/v1/orgs/budget");
    assert_eq!(sent[0].body, json!({ "budgetUsd": 50.0, "period": "week" }));
    assert_eq!(set.out, "✓ The organization is capped at $50.00 a week.");
}

#[test]
fn a_dry_run_of_set_org_sends_dry_run_and_shows_before_and_after() {
    let dry = run(&["budget", "set", "org", "$50/week", "--dry-run"]);
    assert_eq!(dry.code, 0, "{}", dry.out);
    assert_eq!(
        posts(&dry)[0].body,
        json!({ "budgetUsd": 50.0, "period": "week", "dryRun": true })
    );
    assert_eq!(
        dry.out,
        "Dry run: the organization would be capped at $50.00 a week (now: $2000.00 a month). Nothing changed."
    );
}

#[test]
fn a_dry_run_on_a_gateway_that_ignores_it_is_an_error() {
    let dry = run(&["budget", "set", "org", "13/week", "--dry-run"]);
    assert_eq!(dry.code, 1, "{}", dry.out);
    assert!(dry.err.contains("may have set it"), "{}", dry.err);
    assert!(dry.err.contains("nexus budget show org"), "{}", dry.err);
}

#[test]
fn set_org_names_the_fix_for_a_member() {
    let member = run(&["budget", "set", "org", "14/week"]);
    assert_eq!(member.code, 1, "{}", member.out);
    assert!(
        member.err.contains("Ask an owner or admin of your organization."),
        "{}",
        member.err
    );
}

#[test]
fn show_org_gives_an_owner_the_budget_this_period() {
    let shown = run(&["budget", "show", "org"]);
    assert_eq!(shown.code, 0, "{}", shown.out);
    assert!(
        shown.out.contains("Organization · $50.00 a week, hard stop"),
        "{}",
        shown.out
    );
    assert!(shown.out.contains("Spent     $12.00"), "{}", shown.out);
    assert!(shown.out.contains("Left      $36.50"), "{}", shown.out);
    assert!(shown.out.contains("Resets    2026-11-02 (UTC)"), "{}", shown.out);
    let json = run(&["budget", "show", "org", "--json"]);
    // Printed as the TypeScript CLI prints it: whole numbers without a fraction.
    let printed: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(
        printed,
        json!({
            "budgetUsd": 50, "budgetPeriod": "week", "periodSpentUsd": 12, "reservedUsd": 1.5,
            "remainingUsd": 36.5, "resetsAt": "2026-11-02T00:00:00.000Z",
        })
    );
}

#[test]
fn set_org_checks_the_amount_and_the_flags_before_it_sends_anything() {
    for argv in [
        &["budget", "set", "org", "50"][..],
        &["budget", "set", "org", "50/year"],
        &["budget", "set", "org", "50/week", "--mode", "hard_stop"],
        &["budget", "set", "org", "50/week", "--warn-at", "80"],
    ] {
        let refused = run(argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(posts(&refused).is_empty(), "{argv:?}");
    }
}

#[test]
fn set_team_refuses_the_modes_the_gateway_does_not_enforce_before_it_sends_anything() {
    for argv in [
        &["budget", "set", "team/web", "500/month", "--mode", "warn"][..],
        &["budget", "set", "team/web", "500/month", "--mode", "approval"],
        &["budget", "set", "team/web", "500/month", "--mode", "approve"],
        &["budget", "set", "team/web", "500/month", "--warn-at", "80"],
    ] {
        let refused = run(argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}
