//! `nexus agents share` against a stand-in Gateway (`agent-share.test.ts`): the snapshot it sends from
//! the fixture transcript (`fixtures/claude-code/`), what it prints, `--private` revoking every active version, and
//! usage errors (exit 2) before anything is sent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::agent_share::agent_session_key;
use nexus_cli::claude_transcript::claude_project_name;
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_agents_share";
const USER: &str = "usr_ada";
const SESSION: &str = "5f0c2b1e-0000-4000-8000-000000000001";
const TRANSCRIPT: &str = include_str!("fixtures/interfaces/claude-code/session.jsonl");
const SNAPSHOT: &str = include_str!("fixtures/interfaces/claude-code/session.snapshot.json");

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
    let key = agent_session_key(USER, "claude-code", SESSION);
    let versions = [
        json!({ "id": "art_s1", "version": 1 }),
        json!({ "id": "art_s2", "version": 2 }),
    ];
    match (method, path) {
        ("POST", "/v1/artifacts/sessions") => {
            let mut artifact = json!({
                "id": "art_s3", "version": 3, "sessionId": key, "kind": "session", "title": "Fix the failing build",
                "audience": {
                    "kind": body["audience"], "teams": body.get("teams").cloned().unwrap_or(json!([])),
                    "people": body.get("people").cloned().unwrap_or(json!([])),
                },
                "status": "active", "mine": true, "url": "http://ui.test/app/acme/artifacts/art_s3",
                "markdownUrl": "http://core.test/v1/artifacts/art_s3.md",
            });
            if body["audience"] == "public" {
                artifact["publicUrl"] = json!("http://ui.test/a/tok_s3");
                artifact["expiresAt"] = json!("2026-11-28T12:00:00.000Z");
            }
            answer(201, artifact)
        }
        ("GET", "/v1/tenants/me") => answer(
            200,
            json!({ "tenant": { "id": "tnt_acme", "name": "Acme", "createdAt": "2026-01-01T00:00:00.000Z" }, "user": { "id": USER } }),
        ),
        ("DELETE", path) if path.starts_with("/v1/artifacts/sessions/") => {
            let asked = path.trim_start_matches("/v1/artifacts/sessions/");
            // Gateway answers 404 when you have no share of the session under that key.
            if asked != key {
                return answer(
                    404,
                    json!({ "error": { "message": "no such shared session", "type": "not_found_error" } }),
                );
            }
            answer(
                200,
                json!({ "sessionId": asked, "visibility": "private", "revoked": versions }),
            )
        }
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

/// A stand-in Gateway on a free port, on its own thread; every request it saw.
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
                    let uri = request.uri().clone();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: uri.path_and_query().map(|p| p.to_string()).unwrap_or_default(),
                        body: body.clone(),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&method, uri.path(), &body)
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

/// A directory name of its own per run: tests run in parallel, so the process id and a counter.
fn uuid_like() -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

/// A run in a folder with the fixture transcript as its Claude Code session; `signed_in: false` has no API key.
fn run_with(argv: &[&str], signed_in: bool) -> Run {
    let (url, seen) = gateway();
    let dir = std::env::temp_dir().join(format!("nexus-agents-share-{}", uuid_like()));
    let cwd = dir.join("work");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let claude = dir.join("claude");
    let project = claude.join("projects").join(claude_project_name(&cwd));
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join(format!("{SESSION}.jsonl")), TRANSCRIPT).expect("transcript");
    let config_path = dir.join("config.json");
    let config = if signed_in {
        json!({ "url": url, "apiKey": KEY, "org": "acme" })
    } else {
        json!({ "url": url })
    };
    std::fs::write(&config_path, config.to_string()).expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::from([
            ("NEXUS_UI_URL".to_string(), "http://ui.test/app".to_string()),
            ("CLAUDE_CONFIG_DIR".to_string(), claude.to_string_lossy().into_owned()),
        ]),
        poll_ms: Some(0),
        hostname: None,
        cwd: PathBuf::from(&cwd),
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

fn run(argv: &[&str]) -> Run {
    run_with(argv, true)
}

#[test]
fn sends_this_folders_latest_session_fitted_to_the_organization() {
    let r = run(&["agents", "share", "latest", "--org"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].method, "POST");
    assert_eq!(r.seen[0].path, "/v1/artifacts/sessions");
    let expected: Value = serde_json::from_str(SNAPSHOT).expect("snapshot");
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "org", "replace": true, "session": expected["session"], "images": expected["images"] })
    );
    assert!(
        r.out
            .contains("✓ Shared Fix the failing build with your organization (version 3)."),
        "{}",
        r.out
    );
    assert!(r.out.contains("http://ui.test/app/acme/artifacts/art_s3"));
    assert!(r.out.contains(&format!("nexus agents share {SESSION} --private")));
    assert!(
        r.out
            .contains("3 transcript records of a kind Nexus does not read skipped")
    );
}

#[test]
fn takes_chats_audiences_and_thinking_when_asked() {
    let r = run(&[
        "agents",
        "share",
        SESSION,
        "--public",
        "--expires",
        "30d",
        "--include-thinking",
        "--json",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    let body = &r.seen[0].body;
    assert_eq!(body["audience"], "public");
    assert_eq!(body["expiresIn"], "30d");
    assert_eq!(body["replace"], true);
    assert_eq!(body["includeThinking"], true);
    let events = body["session"]["events"].as_array().expect("events");
    assert!(events.iter().any(|event| event["kind"] == "thinking"));
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(printed["artifact"]["publicUrl"], "http://ui.test/a/tok_s3");
    assert_eq!(printed["sessionId"], SESSION);
    assert_eq!(printed["skipped"], 3);
    assert_eq!(
        printed["trimmed"],
        json!({ "outputs": 0, "inputs": 0, "images": 0, "events": 0 })
    );

    let people = run(&["agents", "share", SESSION, "--to", "bo@partner.test, cy@partner.test"]);
    assert_eq!(people.code, 0, "{}", people.err);
    assert_eq!(people.seen[0].body["audience"], "external");
    assert_eq!(
        people.seen[0].body["people"],
        json!(["bo@partner.test", "cy@partner.test"])
    );
}

#[test]
fn private_revokes_every_active_version_by_its_key() {
    let key = agent_session_key(USER, "claude-code", SESSION);
    let r = run(&["agents", "share", "latest", "--private"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let calls: Vec<String> = r
        .seen
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect();
    assert_eq!(
        calls,
        vec![
            "GET /v1/tenants/me".to_string(),
            format!("DELETE /v1/artifacts/sessions/{key}"),
        ]
    );
    assert!(
        r.out
            .contains(&format!("Only you can read {SESSION} now: revoked v1, v2.")),
        "{}",
        r.out
    );

    let none = run(&["agents", "share", "other-session", "--private", "--json"]);
    assert_eq!(none.code, 0, "{}", none.err);
    assert_eq!(
        serde_json::from_str::<Value>(&none.out).expect("json"),
        json!({ "sessionId": "other-session", "visibility": "private", "revoked": [] })
    );
    let text = run(&["agents", "share", "other-session", "--private"]);
    assert_eq!(text.code, 0, "{}", text.err);
    assert!(
        text.out.contains("other-session was not shared; only you can read it."),
        "{}",
        text.out
    );
    assert_eq!(text.seen.last().map(|seen| seen.method.as_str()), Some("DELETE"));
}

#[test]
fn refuses_bad_options_before_anything_is_sent() {
    for (argv, message) in [
        (vec!["agents", "share"], "Usage: nexus agents share"),
        (vec!["agents", "share", "latest"], "Choose who may read it"),
        (
            vec!["agents", "share", "latest", "--org", "--team", "web"],
            "Choose who may read it",
        ),
        (
            vec!["agents", "share", "latest", "--org", "--allow-continue"],
            "--allow-continue is for chat sessions",
        ),
        (
            vec!["agents", "share", "latest", "--private", "--include-thinking"],
            "--private takes no other option",
        ),
        (
            vec!["agents", "share", "latest", "--org", "--expires", "30d"],
            "--expires is for --public links.",
        ),
        (
            vec!["agents", "share", "../x", "--org"],
            "is not a Claude Code session id",
        ),
        (
            vec!["agents", "share", "../x", "--private"],
            "is not a Claude Code session id",
        ),
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.err.contains(message), "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn needs_a_sign_in() {
    let r = run_with(&["agents", "share", "latest", "--org"], false);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("You are not logged in."), "{}", r.err);
    assert!(r.seen.is_empty());
}
