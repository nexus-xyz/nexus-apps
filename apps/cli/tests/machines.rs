//! `nexus machines` against a stand-in Gateway: the list, one machine, and putting one to sleep.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

fn machine(id: &str, extra: Value) -> Value {
    let mut machine = json!({
        "id": id, "sessionId": "th_1", "title": "Fix the sidebar flicker", "name": "Fix the sidebar flicker",
        "provider": "boat", "size": "default", "state": "ready", "error": null,
        "createdAt": "2026-11-13T00:00:00.000Z", "sleepingSince": null, "stopReason": null, "queuedUntil": null,
        "hostOnline": true,
    });
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        machine[key] = value;
    }
    machine
}

fn asleep() -> Value {
    json!({ "state": "sleeping", "sleepingSince": "2026-11-13T01:00:00.000Z", "stopReason": "idle", "hostOnline": false })
}

fn route(method: &str, path: &str) -> Response {
    let mut named = asleep();
    named["title"] = Value::Null;
    named["name"] = json!("Cloud");
    match (method, path) {
        ("GET", "/v1/cloud/machines") => answer(
            200,
            json!({ "machines": [machine("cm_1", json!({})), machine("cm_2", named)] }),
        ),
        ("GET", "/v1/cloud/machines/cm_2") => answer(200, machine("cm_2", asleep())),
        ("POST", "/v1/cloud/machines/cm_1/sleep") => answer(200, machine("cm_1", asleep())),
        _ => answer(404, json!({ "error": { "message": "Not found", "code": "not_found" } })),
    }
}

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    idempotent: bool,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
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
                    let idempotent = request.headers().contains_key("idempotency-key");
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        idempotent,
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
        "nexus-machines-{}-{}",
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

fn line(out: &str, id: &str) -> String {
    out.lines()
        .find(|line| line.contains(id))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn lists_your_machines_with_their_state_and_why_one_sleeps() {
    let listed = run(&["machines"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    let call = listed.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str()),
        ("GET", "/v1/cloud/machines")
    );
    let first = line(&listed.out, "cm_1");
    assert!(
        first.contains("ready") && first.contains("Fix the sidebar flicker"),
        "{}",
        listed.out
    );
    let second = line(&listed.out, "cm_2");
    for text in ["sleeping", "idle", "Cloud"] {
        assert!(second.contains(text), "{text}: {}", listed.out);
    }

    let json_out = run(&["machines", "--json"]);
    let printed: Value = serde_json::from_str(&json_out.out).expect("json");
    assert_eq!(printed["machines"][1]["id"], "cm_2");
}

#[test]
fn shows_one_machine() {
    let shown = run(&["machines", "show", "cm_2"]);
    assert_eq!(shown.code, 0, "{}", shown.err);
    for text in ["cm_2", "sleeping", "Asleep", "idle", "offline"] {
        assert!(shown.out.contains(text), "{text}: {}", shown.out);
    }
}

#[test]
fn puts_a_machine_to_sleep_with_an_idempotency_key() {
    let slept = run(&["machines", "sleep", "cm_1"]);
    assert_eq!(slept.code, 0, "{}", slept.err);
    let call = slept.seen.last().expect("a call");
    assert_eq!(
        (call.method.as_str(), call.path.as_str(), call.idempotent),
        ("POST", "/v1/cloud/machines/cm_1/sleep", true)
    );
    assert!(
        slept
            .out
            .ends_with(" cm_1 is going to sleep: its disk is kept and its running time stops."),
        "{}",
        slept.out
    );
}

#[test]
fn refuses_a_missing_id_an_unknown_verb_and_a_machine_that_is_not_yours() {
    let missing = run(&["machines", "sleep"]);
    assert_eq!(missing.code, 2);
    assert!(
        missing.err.contains("Usage: nexus machines sleep <id>"),
        "{}",
        missing.err
    );
    assert_eq!(run(&["machines", "wake"]).code, 2);
    let other = run(&["machines", "show", "cm_9"]);
    assert_eq!(other.code, 1);
    assert!(other.err.contains("Not found"), "{}", other.err);
}
