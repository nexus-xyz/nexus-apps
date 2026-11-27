//! `nexus routines` against a stand-in Gateway: what each command sends, and what it prints. The
//! TypeScript CLI's `routines.test.ts` checks the same.

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
const ID: &str = "rtn_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn routine(state: &str) -> Value {
    let mut routine = json!({
        "id": ID, "title": "Morning brief", "ownerId": "usr_me", "executor": "gateway",
        "trigger": { "kind": "schedule", "at": "08:00", "days": ["mon", "tue", "wed", "thu", "fri"], "timezone": "UTC" },
        "action": { "kind": "brief" }, "proactivity": { "notify": "always", "writes": "never" }, "caps": {},
        "reportTo": "slack", "state": state,
        "delivery": { "available": false, "code": "routine_brief_unavailable", "reason": "The morning brief is not available yet." },
        "createdVia": "cli", "version": 1, "createdAt": "2026-11-03T00:00:00.000Z", "updatedAt": "2026-11-03T00:00:00.000Z",
    });
    if state == "active" {
        routine["nextRunAt"] = json!("2026-11-04T08:00:00.000Z");
    } else {
        routine["pausedReason"] = json!("owner");
    }
    routine
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, body: &[u8]) -> Response {
    let one = format!("/v1/routines/{ID}");
    match (method, path) {
        ("POST", "/v1/routines") => answer(201, routine("active")),
        ("GET", "/v1/routines") => answer(200, json!({ "routines": [routine("active")] })),
        ("GET", path) if path == one => answer(200, routine("active")),
        ("PATCH", path) if path == one => {
            let state = serde_json::from_slice::<Value>(body).unwrap_or_default()["state"].clone();
            answer(200, routine(state.as_str().unwrap_or("active")))
        }
        ("DELETE", path) if path == one => answer(200, json!({ "id": ID, "deleted": true })),
        _ => answer(
            404,
            json!({ "error": { "message": "no such routine", "code": "not_found" } }),
        ),
    }
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    idempotency_key: Option<String>,
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
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let query = request.uri().query().map(String::from);
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: query.map_or_else(|| path.clone(), |query| format!("{path}?{query}")),
                        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                        idempotency_key: headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .map(String::from),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "code": "authentication_error" } }),
                        );
                    }
                    route(&method, &path, &bytes)
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

fn run(argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-routines-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    let config = json!({ "url": url, "apiKey": KEY, "kind": "cli", "org": "acme" });
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

#[test]
fn create_sends_a_weekday_brief_and_prints_its_next_run() {
    let created = run(&[
        "routines",
        "create",
        "--at",
        "08:00",
        "--days",
        "mon-fri",
        "--timezone",
        "America/Costa_Rica",
        "--max-runs-per-day",
        "1",
    ]);
    assert_eq!(created.code, 0, "{}", created.err);
    assert_eq!(created.seen.len(), 1);
    let sent = &created.seen[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/routines"));
    assert_eq!(
        sent.body,
        json!({
            "trigger": { "kind": "schedule", "at": "08:00", "days": ["mon", "tue", "wed", "thu", "fri"], "timezone": "America/Costa_Rica" },
            "action": { "kind": "brief" }, "caps": { "maxRunsPerDay": 1 },
        })
    );
    assert!(sent.idempotency_key.is_some());
    let text = created.text();
    assert!(
        text.contains(&format!(
            "Created routine {ID}: Morning brief, at 08:00 on mon,tue,wed,thu,fri"
        )),
        "{text}"
    );
    assert!(text.contains("Next run: 2026-11-04T08:00:00.000Z"), "{text}");
    assert!(text.contains("Not delivered yet (routine_brief_unavailable)"), "{text}");

    let every = run(&["routines", "create", "--every", "2h", "--title", "Status"]);
    assert_eq!(every.code, 0, "{}", every.err);
    assert_eq!(
        every.seen[0].body,
        json!({ "title": "Status", "trigger": { "kind": "schedule", "everySeconds": 7200 }, "action": { "kind": "brief" } })
    );
}

#[test]
fn a_trigger_it_cannot_send_sends_nothing() {
    for argv in [
        &["routines", "create"][..],
        &["routines", "create", "--at", "08:00", "--every", "1h"],
        &["routines", "create", "--every", "soon"],
        &["routines", "create", "--every", "1h", "--days", "mon"],
        &["routines", "create", "--at", "08:00", "--days", "fri-mon"],
        &["routines", "create", "--at", "08:00", "--max-runs-per-day", "0"],
        &["routines", "show"],
        &["routines", "run", ID],
    ] {
        let refused = run(argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn list_show_pause_resume_and_delete_name_the_routine() {
    let listed = run(&["routines", "list"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert!(listed.text().contains(&format!(
        "{ID}  active  at 08:00 on mon,tue,wed,thu,fri  next 2026-11-04T08:00:00.000Z  Morning brief"
    )));
    let all = run(&["routines", "list", "--all"]);
    assert_eq!(all.seen[0].path, "/v1/routines?all=true");

    let shown = run(&["routines", "show", ID]);
    assert_eq!(shown.code, 0, "{}", shown.err);
    assert!(
        shown.text().contains("not yet (routine_brief_unavailable)"),
        "{}",
        shown.text()
    );

    let paused = run(&["routines", "pause", ID]);
    assert_eq!(paused.code, 0, "{}", paused.err);
    assert_eq!(paused.seen[0].method, "PATCH");
    assert_eq!(paused.seen[0].body, json!({ "state": "paused" }));
    assert!(paused.text().contains(&format!("Paused {ID}.")));
    let resumed = run(&["routines", "resume", ID]);
    assert_eq!(resumed.seen[0].body, json!({ "state": "active" }));
    assert!(
        resumed
            .text()
            .contains(&format!("Resumed {ID}. Next run: 2026-11-04T08:00:00.000Z"))
    );

    let deleted = run(&["routines", "delete", ID]);
    assert_eq!(deleted.code, 0, "{}", deleted.err);
    assert_eq!(deleted.seen[0].method, "DELETE");
    assert!(deleted.text().contains(&format!("Deleted {ID}.")));

    let missing = run(&["routines", "show", "rtn_missing"]);
    assert_eq!(missing.code, 1);
    assert!(missing.err.contains("No such routine"), "{}", missing.err);
}
