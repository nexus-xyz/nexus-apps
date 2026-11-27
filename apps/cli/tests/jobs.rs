//! `nexus jobs` against a stand-in Gateway: what each command sends, and what it prints.

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
const JOB: &str = "job_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    idempotency_key: Option<String>,
}

fn job(state: &str) -> Value {
    json!({
        "id": JOB, "module": "robustness/review", "verb": "run", "state": state, "teamId": "team_web",
        "createdAt": "2026-10-29T00:00:00.000Z", "updatedAt": "2026-10-29T00:00:00.000Z",
    })
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str) -> Response {
    let missing = || {
        answer(
            404,
            json!({ "error": { "message": "no such job", "code": "not_found" } }),
        )
    };
    match (method, path) {
        ("POST", "/v1/jobs") => answer(202, job("accepted")),
        ("GET", "/v1/jobs/job_missing") | ("POST", "/v1/jobs/job_missing/cancel") => missing(),
        ("GET", "/v1/jobs/job_done/result") => answer(
            200,
            json!({ "id": "job_done", "state": "done", "result": { "pages": 3 } }),
        ),
        ("POST", "/v1/jobs/job_running/cancel") => answer(200, job("running")),
        ("POST", _) if path.ends_with("/cancel") => answer(200, job("cancelled")),
        ("GET", _) if path.ends_with("/result") => answer(200, json!({ "id": JOB, "state": "accepted" })),
        ("GET", _) if path.starts_with("/v1/jobs/") => answer(200, job("accepted")),
        _ => missing(),
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
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
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
        "nexus-jobs-{}-{}",
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
fn submit_sends_the_module_verb_input_and_team_with_a_key() {
    let submitted = run(&[
        "jobs",
        "submit",
        "robustness/review",
        "run",
        "--input",
        r#"{"repo":"https://github.com/a/b"}"#,
        "--team",
        "web",
    ]);
    assert_eq!(submitted.code, 0, "{}", submitted.err);
    assert_eq!(submitted.seen.len(), 1);
    let sent = &submitted.seen[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/jobs"));
    assert_eq!(
        sent.body,
        json!({ "module": "robustness/review", "verb": "run", "input": { "repo": "https://github.com/a/b" }, "team": "web" })
    );
    assert!(sent.idempotency_key.is_some());
    assert!(
        submitted.text().contains(&format!("Submitted job {JOB} (accepted)")),
        "{}",
        submitted.text()
    );

    let bare = run(&["jobs", "submit", "robustness/review", "run", "--json"]);
    assert_eq!(bare.code, 0, "{}", bare.err);
    assert_eq!(
        bare.seen[0].body,
        json!({ "module": "robustness/review", "verb": "run", "input": {} })
    );
    let printed: Value = serde_json::from_str(&bare.text()).expect("json");
    assert_eq!(printed, job("accepted"));
}

#[test]
fn submit_refuses_a_missing_verb_and_an_input_that_is_not_an_object() {
    for argv in [
        &["jobs", "submit", "robustness/review"][..],
        &["jobs", "submit", "robustness/review", "run", "--input", "[1]"][..],
        &["jobs", "submit", "robustness/review", "run", "--input", "{oops"][..],
        &["jobs", "get"][..],
        &["jobs", "wait", JOB][..],
    ] {
        let refused = run(argv);
        assert_eq!(refused.code, 2, "{argv:?}: {}", refused.err);
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn get_cancel_and_result_name_the_job() {
    let read = run(&["jobs", "get", JOB]);
    assert_eq!(read.code, 0, "{}", read.err);
    assert_eq!(read.seen[0].path, format!("/v1/jobs/{JOB}"));
    assert!(read.text().contains("robustness/review run"), "{}", read.text());
    assert!(read.text().contains("accepted"), "{}", read.text());
    assert!(read.text().contains("team_web"), "{}", read.text());

    let cancelled = run(&["jobs", "cancel", JOB]);
    assert_eq!(cancelled.code, 0, "{}", cancelled.err);
    assert_eq!(
        (cancelled.seen[0].method.as_str(), cancelled.seen[0].path.as_str()),
        ("POST", format!("/v1/jobs/{JOB}/cancel").as_str())
    );
    assert!(cancelled.text().contains(&format!("Cancelled {JOB}.")));
    let running = run(&["jobs", "cancel", "job_running"]);
    assert_eq!(running.code, 0, "{}", running.err);
    assert!(running.text().contains("job_running is running"), "{}", running.text());

    let done = run(&["jobs", "result", "job_done"]);
    assert_eq!(done.code, 0, "{}", done.err);
    let result: Value = serde_json::from_str(&done.text()).expect("the result as JSON");
    assert_eq!(result, json!({ "pages": 3 }));
    let pending = run(&["jobs", "result", JOB]);
    assert!(
        pending.text().contains("is accepted, with no result"),
        "{}",
        pending.text()
    );
}

#[test]
fn a_missing_job_prints_gateways_words() {
    let missing = run(&["jobs", "get", "job_missing"]);
    assert_eq!(missing.code, 1);
    assert!(missing.err.contains("No such job"), "{}", missing.err);
}
