//! `nexus notifications` against a stand-in Gateway: the query each list sends, the marks, and the catalogue.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    idempotent: bool,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn item(id: &str, extra: Value) -> Value {
    let mut item = json!({
        "id": id, "kind": "budget.hard_stop", "group": "usage_and_billing",
        "subjectText": "A hard stop refused a model call", "subject": { "type": "team", "id": "team_web" },
        "sources": [{ "type": "spend", "spendId": "sp_1" }], "count": 1,
        "createdAt": "2026-10-28T00:00:00.000Z", "updatedAt": "2026-10-28T00:00:00.000Z",
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        item[key] = value;
    }
    item
}

fn route(method: &str, path: &str, query: &str) -> Response {
    let read = json!({ "readAt": "2026-10-28T02:00:00.000Z" });
    match (method, path) {
        ("GET", "/v1/notifications") if query.contains("group=my_sessions") => answer(200, json!({ "items": [] })),
        ("GET", "/v1/notifications") => answer(
            200,
            json!({ "items": [item("ntf_1", json!({ "count": 3 })), item("ntf_2", read)], "nextCursor": "c2" }),
        ),
        ("GET", "/v1/notifications/kinds") => answer(
            200,
            json!({ "kinds": [
                { "name": "budget.hard_stop", "group": "usage_and_billing", "subjectText": "A hard stop refused a model call", "required": true, "defaults": ["inbox", "email"] },
                { "name": "artifact.access_requested", "group": "requests_and_approvals", "subjectText": "Someone asked for access", "required": false, "defaults": [] },
            ] }),
        ),
        ("POST", "/v1/notifications/ntf_1/read") => answer(200, item("ntf_1", read)),
        ("POST", "/v1/notifications/ntf_1/done") => answer(200, item("ntf_1", read)),
        _ => answer(404, json!({ "error": { "message": "Not found", "code": "not_found" } })),
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
                    let query = request.uri().query().unwrap_or("").to_string();
                    let idempotent = request.headers().contains_key("idempotency-key");
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query: query.clone(),
                        idempotent,
                    });
                    route(&method, &path, &query)
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
        "nexus-notifications-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": "nx_test", "kind": "api", "org": "acme" }).to_string(),
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
    let out = capture.0.out.borrow().join("\n");
    let err = capture.0.err.borrow().join("\n");
    Run { code, out, err, seen }
}

#[test]
fn lists_the_inbox_with_its_status_and_the_next_page() {
    let listed = run(&["notifications"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    let call = listed.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), call.query.as_str()),
        ("GET", "/v1/notifications", "")
    );
    for text in [
        "A hard stop refused a model call (3)",
        "Usage and billing",
        "More: nexus notifications list --cursor c2",
    ] {
        assert!(listed.out.contains(text), "{text}: {}", listed.out);
    }
    let line = |id: &str| {
        listed
            .out
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line("ntf_1").contains("unread"), "{}", listed.out);
    assert!(!line("ntf_2").contains("unread"), "{}", listed.out);
}

#[test]
fn sends_the_filters_as_the_query_and_says_when_nothing_matches() {
    let empty = run(&[
        "notifications",
        "list",
        "--unread",
        "--group",
        "my_sessions",
        "--cursor",
        "c2",
    ]);
    assert_eq!(empty.code, 0, "{}", empty.err);
    assert_eq!(
        empty.seen.last().expect("a call").query,
        "unread=true&group=my_sessions&cursor=c2"
    );
    assert_eq!(empty.out, "Nothing unread in your inbox.");

    let json_out = run(&["notifications", "--json"]);
    let printed: Value = serde_json::from_str(&json_out.out).expect("json");
    assert_eq!(printed["nextCursor"], "c2");
    assert_eq!(printed["items"][1]["id"], "ntf_2");
}

#[test]
fn marks_an_item_read_or_done_with_an_idempotency_key() {
    let read = run(&["notifications", "read", "ntf_1"]);
    assert_eq!(read.code, 0, "{}", read.err);
    let call = read.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), call.idempotent),
        ("POST", "/v1/notifications/ntf_1/read", true)
    );
    assert!(read.out.ends_with(" Marked ntf_1 read."), "{}", read.out);

    let done = run(&["notifications", "done", "ntf_1"]);
    assert_eq!(done.code, 0, "{}", done.err);
    assert_eq!(done.seen.last().expect("a call").path, "/v1/notifications/ntf_1/done");
    assert!(
        done.out.ends_with(" Marked ntf_1 done: it left your inbox."),
        "{}",
        done.out
    );
}

#[test]
fn lists_the_catalogue_of_kinds() {
    let kinds = run(&["notifications", "kinds"]);
    assert_eq!(kinds.code, 0, "{}", kinds.err);
    let line = |name: &str| {
        kinds
            .out
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line("budget.hard_stop").contains("inbox, email"), "{}", kinds.out);
    assert!(line("artifact.access_requested").contains('—'), "{}", kinds.out);
}

#[test]
fn refuses_a_missing_id_an_unknown_verb_and_an_item_that_is_not_yours() {
    let missing = run(&["notifications", "done"]);
    assert_eq!(missing.code, 2);
    assert!(
        missing.err.contains("Usage: nexus notifications done <id>"),
        "{}",
        missing.err
    );
    assert_eq!(run(&["notifications", "archive"]).code, 2);
    let other = run(&["notifications", "done", "ntf_9"]);
    assert_eq!(other.code, 1);
    assert!(other.err.contains("Not found"), "{}", other.err);
}
