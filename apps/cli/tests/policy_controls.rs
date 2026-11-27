//! `nexus policy controls|pause|clear` against a stand-in Gateway: `--team` picks the team's routes (see `cli.test.ts`).

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

fn control() -> Value {
    json!({
        "id": "ctl_1", "tenantId": "ten_1", "kind": "pause", "subjectLevel": "person", "subject": "usr_1",
        "reason": "incident", "createdAt": "2026-10-28T00:00:00.000Z", "expiresAt": "2099-01-01T00:00:00.000Z",
    })
}

fn route(method: &str, path: &str) -> Response {
    match (method, path) {
        // `ctl_denied` stands in for a control the caller may not clear.
        ("DELETE", "/v1/policies/controls/ctl_denied") => answer(
            403,
            json!({ "error": { "message": "role required", "type": "forbidden" } }),
        ),
        ("GET", "/v1/policies/controls" | "/v1/policies/teams/web/controls") => {
            answer(200, json!({ "controls": [control()] }))
        }
        ("POST", "/v1/policies/controls" | "/v1/policies/teams/web/controls") => answer(201, control()),
        ("DELETE", "/v1/policies/controls/ctl_1" | "/v1/policies/teams/web/controls/ctl_1") => answer(200, control()),
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
                        body,
                    });
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
    seen: Vec<Seen>,
}

fn run(argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-policy-controls-{}-{}",
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

#[test]
fn controls_without_team_reads_the_org_route() {
    let listed = run(&["policy", "controls"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.seen[0].method, "GET");
    assert_eq!(listed.seen[0].path, "/v1/policies/controls");
    assert!(listed.out.contains("ctl_1"), "{}", listed.out);
}

#[test]
fn controls_with_team_reads_the_team_route() {
    let listed = run(&["policy", "controls", "--team", "web"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.seen[0].method, "GET");
    assert_eq!(listed.seen[0].path, "/v1/policies/teams/web/controls");
    assert!(listed.out.contains("ctl_1"), "{}", listed.out);
}

#[test]
fn pause_with_team_posts_to_the_team_route() {
    let paused = run(&[
        "policy",
        "pause",
        "person/usr_1",
        "--reason",
        "incident",
        "--expires",
        "1d",
        "--team",
        "web",
    ]);
    assert_eq!(paused.code, 0, "{}", paused.err);
    assert_eq!(paused.seen[0].method, "POST");
    assert_eq!(paused.seen[0].path, "/v1/policies/teams/web/controls");
    assert_eq!(paused.seen[0].body["kind"], "pause");
    assert_eq!(paused.seen[0].body["level"], "person");
    assert_eq!(paused.seen[0].body["subject"], "usr_1");
}

#[test]
fn clear_with_team_deletes_on_the_team_route() {
    let cleared = run(&["policy", "clear", "ctl_1", "--team", "web"]);
    assert_eq!(cleared.code, 0, "{}", cleared.err);
    assert_eq!(cleared.seen[0].method, "DELETE");
    assert_eq!(cleared.seen[0].path, "/v1/policies/teams/web/controls/ctl_1");
}

#[test]
fn a_403_reads_as_owner_or_admin() {
    let denied = run(&["policy", "clear", "ctl_denied"]);
    assert_eq!(denied.code, 1, "{}", denied.out);
    assert!(
        denied.err.contains("Ask an owner or admin of your organization."),
        "{}",
        denied.err
    );
}
