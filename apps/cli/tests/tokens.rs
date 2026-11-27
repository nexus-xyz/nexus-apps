//! `nexus auth tokens` against a stand-in Gateway: what each command sends, and that a new key prints once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_me";
const SECRET: &str = "nx_0123456789abcdef0123456789abcdef";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    body: Value,
    idempotency_key: Option<String>,
}

fn tokens() -> Value {
    json!([
        { "id": "tok_ci", "userId": "usr_ada", "kind": "api", "prefix": "nx_0123", "label": "CI deploys", "createdAt": "2026-10-20T00:00:00.000Z" },
        { "id": "tok_cli", "userId": "usr_ada", "kind": "cli", "prefix": "nx_4567", "label": "laptop", "createdAt": "2026-10-10T00:00:00.000Z", "expiresAt": "2027-01-08T00:00:00.000Z", "current": true },
        { "id": "tok_ui", "userId": "usr_ada", "kind": "ui", "prefix": "nx_89ab", "createdAt": "2026-10-26T00:00:00.000Z", "expiresAt": "2026-10-26T12:00:00.000Z" },
    ])
}

fn everyone() -> Value {
    let mut all = tokens();
    all.as_array_mut().expect("tokens").push(
        json!({ "id": "tok_bo", "userId": "usr_bo", "kind": "api", "prefix": "nx_cdef", "label": "nightly", "createdAt": "2026-10-01T00:00:00.000Z" }),
    );
    all
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn refusal(status: u16, message: &str, code: &str) -> Response {
    answer(status, json!({ "error": { "message": message, "code": code } }))
}

fn route(member: bool, method: &str, path: &str, query: &str, body: &Value) -> Response {
    match (method, path) {
        ("GET", "/v1/auth/tokens") if query != "all=1" => answer(200, json!({ "tokens": tokens() })),
        ("GET", "/v1/auth/tokens") if member => refusal(
            403,
            "only owners and admins can list everyone's tokens",
            "role_required",
        ),
        ("GET", "/v1/auth/tokens") => answer(200, json!({ "tokens": everyone() })),
        ("POST", "/v1/auth/tokens") => answer(
            201,
            json!({
                "id": "tok_new", "userId": "usr_ada", "kind": "api", "prefix": "nx_0123", "label": body["label"],
                "createdAt": "2026-10-28T00:00:00.000Z", "token": SECRET,
            }),
        ),
        ("DELETE", "/v1/auth/tokens/tok_missing") => refusal(404, "no token tok_missing", "not_found"),
        ("DELETE", "/v1/auth/tokens/tok_bo") if member => refusal(
            403,
            "only owners and admins can revoke someone else's token",
            "role_required",
        ),
        ("DELETE", _) if path.starts_with("/v1/auth/tokens/") => answer(200, json!({ "revoked": true })),
        _ => refusal(404, "no such endpoint", "not_found"),
    }
}

fn gateway(member: bool) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
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
                    let query = request.uri().query().unwrap_or("").to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query: query.clone(),
                        body: body.clone(),
                        idempotency_key: headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .map(String::from),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return refusal(401, "invalid API key", "authentication_error");
                    }
                    route(member, &method, &path, &query, &body)
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
    out: Vec<String>,
    err: String,
    seen: Vec<Seen>,
}

impl Run {
    fn text(&self) -> String {
        self.out.join("\n")
    }
}

fn run_with(member: bool, signed_in: bool, argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-tokens-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway(member);
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    let config = if signed_in {
        json!({ "url": url, "apiKey": KEY, "kind": "cli", "org": "acme" })
    } else {
        json!({ "url": url })
    };
    std::fs::write(&config_path, config.to_string()).expect("config");
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
    let out = capture.0.out.borrow().clone();
    let err = capture.0.err.borrow().join("\n");
    Run { code, out, err, seen }
}

fn run(argv: &[&str]) -> Run {
    run_with(false, true, argv)
}

fn cells(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn lists_your_api_keys_and_cli_sign_ins_not_nexus_ui_sessions() {
    let listed = run(&["auth", "tokens"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.seen.len(), 1);
    assert_eq!(
        (
            listed.seen[0].method.as_str(),
            listed.seen[0].path.as_str(),
            listed.seen[0].query.as_str()
        ),
        ("GET", "/v1/auth/tokens", "")
    );
    let lines: Vec<&str> = listed
        .out
        .iter()
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(cells(lines[0]), "Token Name Kind Prefix Created Expires");
    assert_eq!(cells(lines[1]), "· tok_ci CI deploys API key nx_0123… 2026-10-20 –");
    assert_eq!(
        cells(lines[2]),
        "· tok_cli laptop CLI sign-in (in use) nx_4567… 2026-10-10 2027-01-08"
    );
    assert_eq!(lines[3], "Create an API key: nexus auth tokens create --label <label>");
    assert_eq!(lines.len(), 4);
    assert!(!listed.text().contains("tok_ui"));
    assert_eq!(run(&["auth", "tokens", "list"]).out, listed.out);
}

#[test]
fn lists_everyones_with_all_and_prints_gateways_answer_with_json() {
    let listed = run(&["auth", "tokens", "list", "--all"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.seen[0].query, "all=1");
    let bo = listed.out.iter().find(|line| line.contains("tok_bo")).expect("tok_bo");
    assert!(
        cells(bo).starts_with("· tok_bo nightly API key usr_bo nx_cdef…"),
        "{bo}"
    );
    let json = run(&["auth", "tokens", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.text()).expect("json"),
        json!({ "tokens": tokens() })
    );
}

#[test]
fn refuses_all_to_a_member_with_the_servers_message_and_who_may() {
    let refused = run_with(true, true, &["auth", "tokens", "--all"]);
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("Only owners and admins can list everyone's tokens"),
        "{}",
        refused.err
    );
    assert!(
        refused.err.contains("Ask an owner or admin of your organization."),
        "{}",
        refused.err
    );
}

#[test]
fn create_sends_the_label_and_prints_the_key_once() {
    let created = run(&["auth", "tokens", "create", "--label", "CI deploys"]);
    assert_eq!(created.code, 0, "{}", created.err);
    assert_eq!(created.seen.len(), 1);
    assert_eq!(
        (created.seen[0].method.as_str(), created.seen[0].path.as_str()),
        ("POST", "/v1/auth/tokens")
    );
    assert_eq!(created.seen[0].body, json!({ "label": "CI deploys" }));
    assert!(created.seen[0].idempotency_key.is_some());
    assert_eq!(created.text().matches(SECRET).count(), 1);
    assert_eq!(
        created.out,
        vec![
            "✓ Created API key CI deploys".to_string(),
            "  Id     tok_new".to_string(),
            format!("  Token  {SECRET}"),
            "! Copy the token now. Nexus does not show it again.".to_string(),
            "Sign in with it in CI: echo \"$NEXUS_API_KEY\" | nexus auth login --with-token".to_string(),
        ]
    );
    let json = run(&["auth", "tokens", "create", "--label", "CI deploys", "--json"]);
    let answer: Value = serde_json::from_str(&json.text()).expect("json");
    assert_eq!(answer["id"], "tok_new");
    assert_eq!(answer["token"], SECRET);
}

#[test]
fn create_needs_a_label_and_sends_nothing_without_one() {
    for argv in [
        vec!["auth", "tokens", "create"],
        vec!["auth", "tokens", "create", "--label"],
        vec!["auth", "tokens", "create", "--label", "  "],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(
            refused.err.contains("Usage: nexus auth tokens create --label <label>"),
            "{}",
            refused.err
        );
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn revoke_revokes_a_token_by_its_id() {
    let revoked = run(&["auth", "tokens", "revoke", "tok_ci"]);
    assert_eq!(revoked.code, 0, "{}", revoked.err);
    assert_eq!(revoked.seen.len(), 1);
    assert_eq!(
        (revoked.seen[0].method.as_str(), revoked.seen[0].path.as_str()),
        ("DELETE", "/v1/auth/tokens/tok_ci")
    );
    assert_eq!(
        revoked.out,
        vec!["✓ Revoked tok_ci: anything using it stops working now."]
    );
    let json = run(&["auth", "tokens", "revoke", "tok_ci", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.text()).expect("json"),
        json!({ "revoked": true })
    );
}

#[test]
fn revoke_says_gateways_refusal_and_needs_an_id() {
    let missing = run(&["auth", "tokens", "revoke", "tok_missing"]);
    assert_eq!(missing.code, 1);
    assert!(missing.err.contains("No token tok_missing"), "{}", missing.err);
    let member = run_with(true, true, &["auth", "tokens", "revoke", "tok_bo"]);
    assert_eq!(member.code, 1);
    assert!(
        member.err.contains("Ask an owner or admin of your organization."),
        "{}",
        member.err
    );
    let bare = run(&["auth", "tokens", "revoke"]);
    assert_eq!(bare.code, 2);
    assert!(bare.seen.is_empty());
}

#[test]
fn needs_a_sign_in_and_is_in_auths_help() {
    let signed_out = run_with(false, false, &["auth", "tokens"]);
    assert_eq!(signed_out.code, 1);
    assert!(signed_out.err.contains("You are not logged in."), "{}", signed_out.err);
    assert!(signed_out.seen.is_empty());
    let help = run_with(false, false, &["auth", "--help"]);
    assert_eq!(help.code, 0);
    assert!(
        help.text().contains("  tokens create --label <label>"),
        "{}",
        help.text()
    );
    let create_help = run_with(false, false, &["auth", "tokens", "create", "--help"]);
    assert_eq!(create_help.code, 0);
    assert!(
        create_help.text().contains("nexus auth tokens create --label <label>"),
        "{}",
        create_help.text()
    );
}
