//! `nexus members remove` against a stand-in Gateway (`members.test.ts`): it finds the person by email or user id, asks
//! before it removes, and sends no DELETE for an unknown email, a refusal or a dry run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_members";

/// Each request's method and path.
type Log = Arc<Mutex<Vec<(String, String)>>>;

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str) -> Response {
    match (method, path) {
        ("GET", "/v1/orgs/members") => answer(
            200,
            json!({ "members": [
                { "userId": "usr_ada", "email": "ada@acme.test", "role": "owner", "joinedAt": "2026-10-01T00:00:00.000Z" },
                { "userId": "usr_bob", "email": "Bob@acme.test", "role": "member", "joinedAt": "2026-10-02T00:00:00.000Z" },
                { "userId": "usr_dee", "email": "dee@acme.test", "role": "admin", "joinedAt": "2026-10-03T00:00:00.000Z" },
            ] }),
        ),
        ("DELETE", "/v1/orgs/members/usr_bob") => answer(200, json!({ "removed": true })),
        ("DELETE", "/v1/orgs/members/usr_ada") => answer(
            409,
            json!({ "error": { "message": "the owner cannot be removed; transfer ownership first", "type": "conflict", "code": "owner_role" } }),
        ),
        // The key is Dee's own.
        ("DELETE", "/v1/orgs/members/usr_dee") => answer(
            409,
            json!({ "error": { "message": "you cannot remove yourself; ask another owner or admin", "type": "conflict", "code": "self_removal" } }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

fn gateway() -> (String, Log) {
    let seen: Log = Arc::default();
    let log = seen.clone();
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    log.lock().expect("log").push((method.clone(), path.clone()));
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&method, &path)
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
    deletes: Vec<String>,
}

/// Run `argv`; `terminal` is what a person types on a terminal, `None` without one.
fn run(argv: &[&str], terminal: Option<&str>) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-members-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        stdin: terminal.unwrap_or_default().to_string(),
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
        interactive: terminal.is_some(),
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let deletes = seen
        .lock()
        .expect("seen")
        .iter()
        .filter(|(method, _)| method == "DELETE")
        .map(|(_, path)| path.clone())
        .collect();
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        deletes,
    }
}

#[test]
fn an_email_finds_the_member_and_removes_them_with_yes() {
    let removed = run(&["members", "remove", "BOB@acme.test", "--yes"], None);
    assert_eq!(removed.code, 0, "{}{}", removed.out, removed.err);
    assert_eq!(removed.deletes, ["/v1/orgs/members/usr_bob"]);
    assert!(removed.out.contains("Removed Bob@acme.test"), "{}", removed.out);
    assert!(removed.out.contains("nexus members recover usr_bob"), "{}", removed.out);
    let json = run(&["members", "remove", "usr_bob", "--yes", "--json"], None);
    assert_eq!(
        serde_json::from_str::<Value>(&json.out).expect("json"),
        json!({ "removed": true, "userId": "usr_bob", "email": "Bob@acme.test" })
    );
}

#[test]
fn an_unknown_email_errors_and_sends_no_delete() {
    let unknown = run(&["members", "remove", "cy@acme.test", "--yes"], None);
    assert_eq!(unknown.code, 1);
    assert!(unknown.err.contains("cy@acme.test is not a member"), "{}", unknown.err);
    assert!(unknown.deletes.is_empty());
}

#[test]
fn without_a_terminal_it_needs_yes_and_sends_no_delete() {
    let refused = run(&["members", "remove", "bob@acme.test"], None);
    assert_eq!(refused.code, 2);
    assert!(
        refused.err.contains("nexus members remove bob@acme.test --yes"),
        "{}",
        refused.err
    );
    assert!(refused.deletes.is_empty());
    let usage = run(&["members", "remove"], None);
    assert_eq!(usage.code, 2);
    assert!(usage.deletes.is_empty());
}

#[test]
fn on_a_terminal_it_asks_and_only_yes_removes() {
    let declined = run(&["members", "remove", "bob@acme.test"], Some("\n"));
    assert_eq!(declined.code, 1);
    assert!(declined.out.contains("[y/N]"), "{}", declined.out);
    assert!(declined.out.contains("Nothing changed."), "{}", declined.out);
    assert!(declined.deletes.is_empty());
    let confirmed = run(&["members", "remove", "bob@acme.test"], Some("y\n"));
    assert_eq!(confirmed.code, 0, "{}{}", confirmed.out, confirmed.err);
    assert_eq!(confirmed.deletes, ["/v1/orgs/members/usr_bob"]);
}

#[test]
fn a_dry_run_sends_no_delete() {
    let dry = run(&["members", "remove", "bob@acme.test", "--dry-run"], None);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert!(dry.out.contains("Dry run: nothing changed."), "{}", dry.out);
    assert!(
        dry.out.contains("Bob@acme.test (member) would be removed"),
        "{}",
        dry.out
    );
    assert!(dry.deletes.is_empty());
    let owner = run(&["members", "remove", "usr_ada", "--dry-run", "--json"], None);
    assert_eq!(
        serde_json::from_str::<Value>(&owner.out).expect("json"),
        json!({ "dryRun": true, "userId": "usr_ada", "email": "ada@acme.test", "role": "owner" })
    );
    assert!(owner.deletes.is_empty());
}

#[test]
fn gateway_refusing_the_owner_says_what_to_do() {
    let owner = run(&["members", "remove", "ada@acme.test", "--yes"], None);
    assert_eq!(owner.code, 1);
    assert!(owner.err.contains("Transfer ownership"), "{}", owner.err);
}

#[test]
fn gateway_refusing_a_self_removal_says_what_to_do() {
    let herself = run(&["members", "remove", "dee@acme.test", "--yes"], None);
    assert_eq!(herself.code, 1);
    assert!(herself.err.contains("Ask another owner or admin"), "{}", herself.err);
}
