//! `nexus auth delete-account` against a stand-in Gateway (`account.test.ts`): the person types DELETE to
//! confirm (or passes `--confirm DELETE`), nothing is deleted on a dry run, anything else typed or a refusal, and the
//! saved sign-in is forgotten only once the account is gone.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_account";
const TAKES: &str =
    "your sign-in, your email and your name, your personal organization and its plan and your place in Acme";

/// The deletions Gateway made: a body with `confirm: "DELETE"`, as Gateway asks.
type Log = Arc<Mutex<Vec<Value>>>;

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

/// `refuse`: Gateway refuses the person (they own Acme); `stale`: their sign-in is too old to delete; `money`: their
/// personal organization owes $9.99 for usage and holds $15 of Nexus credits.
fn gateway(refuse: bool, stale: bool, money: bool) -> (String, Log) {
    let seen: Log = Arc::default();
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
                    let route = (request.method().to_string(), request.uri().path().to_string());
                    if route != ("POST".to_string(), "/v1/auth/account/delete".to_string()) {
                        return answer(404, json!({ "error": { "message": "no such endpoint", "type": "not_found" } }));
                    }
                    if refuse {
                        return answer(
                            409,
                            json!({ "error": { "message": "you own Acme; transfer ownership in Nexus UI (Members) or ask support to close it, then delete your account", "type": "invalid_request_error", "code": "owns_organization" } }),
                        );
                    }
                    let bytes = to_bytes(request.into_body(), 1 << 16).await.expect("body");
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let dry_run = body["dryRun"] == true;
                    if !dry_run {
                        if body["confirm"] != "DELETE" {
                            return answer(
                                400,
                                json!({ "error": { "message": "type DELETE to confirm", "type": "invalid_request_error", "code": "confirmation_required" } }),
                            );
                        }
                        log.lock().expect("log").push(body);
                    }
                    let mut body = json!({ "userId": "usr_bob", "dryRun": dry_run, "deleted": !dry_run, "personalOrg": "ten_bob",
                                "left": [{ "id": "ten_acme", "name": "Acme" }], "planEnded": true, "signInNeeded": stale });
                    if money {
                        body["unpaidUsageUsd"] = json!("9.990000");
                        body["creditsLostUsd"] = json!("15.000000");
                    }
                    answer(200, body)
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
    deletions: usize,
    /// The saved config after the run.
    saved: Value,
    url: String,
}

/// Run `argv`; `terminal` is what a person types on a terminal, `None` without one.
fn run(argv: &[&str], terminal: Option<&str>, refuse: bool) -> Run {
    run_on(argv, terminal, refuse, false)
}

/// [`run`], against a Gateway that says the sign-in is too old when `stale`.
fn run_on(argv: &[&str], terminal: Option<&str>, refuse: bool, stale: bool) -> Run {
    run_against(argv, terminal, refuse, stale, false)
}

/// [`run_on`], against a Gateway that says what the deletion does with money when `money`.
fn run_against(argv: &[&str], terminal: Option<&str>, refuse: bool, stale: bool, money: bool) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-account-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway(refuse, stale, money);
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "kind": "cli", "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        stdin: terminal.unwrap_or_default().to_string(),
        ..Captured::default()
    }));
    let ctx = Context {
        config_path: config_path.clone(),
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir,
        interactive: terminal.is_some(),
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let saved = serde_json::from_str(&std::fs::read_to_string(&config_path).expect("config")).expect("json");
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        deletions: seen.lock().expect("seen").len(),
        saved,
        url,
    }
}

#[test]
fn a_dry_run_says_what_would_go_and_deletes_nothing() {
    let dry = run(&["auth", "delete-account", "--dry-run"], None, false);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(
        dry.out,
        format!(
            "! Dry run: nothing changed. Deleting your account would delete {TAKES}. Your organizations' spend and audit trail stay."
        )
    );
    assert_eq!(dry.deletions, 0);
    assert_eq!(dry.saved["apiKey"], KEY);
}

#[test]
fn without_a_terminal_it_needs_confirm_delete() {
    let refused = run(&["auth", "delete-account"], None, false);
    assert_eq!(refused.code, 2);
    assert!(
        refused.err.contains(&format!("This deletes {TAKES} for good.")),
        "{}",
        refused.err
    );
    assert!(refused.err.contains("--confirm DELETE"), "{}", refused.err);
    assert_eq!(refused.deletions, 0);
    // `--yes` is not a confirmation, and `--confirm` takes DELETE alone.
    for argv in [
        &["auth", "delete-account", "--yes"][..],
        &["auth", "delete-account", "--confirm", "delete"],
    ] {
        let refused = run(argv, None, false);
        assert_eq!(refused.code, 2, "{argv:?}: {}", refused.err);
        assert_eq!(refused.deletions, 0, "{argv:?}");
    }
}

#[test]
fn on_a_terminal_only_delete_typed_deletes() {
    for typed in ["n\n", "y\n", "delete\n", "\n"] {
        let declined = run(&["auth", "delete-account"], Some(typed), false);
        assert_eq!(declined.code, 1, "{typed:?}");
        assert!(
            declined
                .out
                .contains(&format!("This deletes {TAKES} for good. Type DELETE to confirm:")),
            "{}",
            declined.out
        );
        assert!(declined.out.contains("Nothing changed."), "{}", declined.out);
        assert_eq!(declined.deletions, 0, "{typed:?}");
    }
    let confirmed = run(&["auth", "delete-account"], Some(" DELETE \n"), false);
    assert_eq!(confirmed.code, 0, "{}", confirmed.err);
    assert_eq!(confirmed.deletions, 1);
    assert_eq!(confirmed.saved, json!({ "url": confirmed.url }));
}

#[test]
fn confirm_delete_deletes_the_account_and_forgets_the_sign_in() {
    let deleted = run(&["auth", "delete-account", "--confirm", "DELETE"], None, false);
    assert_eq!(deleted.code, 0, "{}", deleted.err);
    assert_eq!(
        deleted.out,
        format!("✓ Your account is deleted, and you are signed out of {}.", deleted.url)
    );
    assert_eq!(deleted.deletions, 1);
    assert_eq!(deleted.saved, json!({ "url": deleted.url }));
}

#[test]
fn a_refusal_keeps_the_sign_in() {
    let refused = run(&["auth", "delete-account", "--confirm", "DELETE"], None, true);
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("you own Acme"), "{}", refused.err);
    assert_eq!(refused.deletions, 0);
    assert_eq!(refused.saved["apiKey"], KEY);
}

#[test]
fn an_old_sign_in_is_sent_to_sign_in_again_before_it_is_asked_anything() {
    let stale = run_on(&["auth", "delete-account"], Some("DELETE\n"), false, true);
    assert_eq!(stale.code, 1);
    assert!(
        stale.err.contains("Sign in again to delete your account"),
        "{}",
        stale.err
    );
    assert!(stale.err.contains("nexus auth login"), "{}", stale.err);
    assert!(!stale.out.contains("Type DELETE"), "{}", stale.out);
    assert_eq!(stale.deletions, 0);
    assert_eq!(stale.saved["apiKey"], KEY);
}

/// The dry run and the question say the unpaid usage the card pays first and the Nexus credits that are lost,
/// in the TypeScript CLI's words.
#[test]
fn says_the_unpaid_usage_and_the_credits_that_are_lost() {
    let dry = run_against(&["auth", "delete-account", "--dry-run"], None, false, false, true);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(
        dry.out,
        format!(
            "! Dry run: nothing changed. Deleting your account would delete {TAKES}. You owe $9.99 for usage: Nexus charges it to your card on file first, and writes off what the card does not pay. Your $15.00 of Nexus credits is lost; nothing refunds it. Your organizations' spend and audit trail stay."
        )
    );
    let unconfirmed = run_against(&["auth", "delete-account"], None, false, false, true);
    assert_eq!(unconfirmed.code, 2);
    assert!(
        unconfirmed.err.contains("You owe $9.99 for usage"),
        "{}",
        unconfirmed.err
    );
    assert_eq!(unconfirmed.deletions, 0);
}
