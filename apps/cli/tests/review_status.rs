//! `nexus robustness status review`, the manifest's dotted `connect` flags and `nexus policy grants` against a
//! stand-in Gateway (the TypeScript twins: `module-status.test.ts`, `cli.test.ts`).

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
    query: String,
    body: Value,
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn manifest() -> Value {
    let input = |name: &str, verbs: &[&str]| json!({ "name": name, "label": name, "verbs": verbs });
    json!({
        "product": { "slug": "robustness", "name": "Nexus Robustness" },
        "capability": { "slug": "review", "name": "Nexus Review", "description": "" },
        "verbs": ["connect", "run", "watch", "report", "apply", "status"],
        "inputs": [
            { "name": "repo", "label": "Repository", "required": true, "verbs": ["connect", "run", "watch", "report", "apply", "status"] },
            input("pr", &["run", "apply", "status"]),
            input("effort", &["connect"]),
            input("output.incremental", &["connect"]),
            input("outcome-filter-enabled", &["run"]),
        ],
        "panel": { "kind": "pages" },
        "operations": {},
    })
}

/// A status answer in the contract's shape, trimmed to what the renderer reads.
fn status_body() -> Value {
    json!({
        "contract": 1,
        "repository": { "repo": "acme/app", "connected": true },
        "effective": {
            "readiness": "limited",
            "headline": "Review stores findings here without posting, at quick effort, on every matching pull request.",
            "automatic": { "state": "on", "summary": "Reviews pull requests on opened into main.", "reasons": [] },
            "posting": { "state": "fallback", "summary": "Runs as shadow: this deployment cannot post.", "reasons": [] },
            "effort": { "state": "fallback", "summary": "Runs as quick.", "reasons": [] },
        },
        "warnings": [{
            "code": "write_not_granted",
            "message": "The organization has not granted Review: post a review with inline comments.",
            "fix": { "who": "owner_or_admin", "action": "Grant the missing writes to Review.",
                     "command": "nexus policy grants set robustness/review --connector github --actions pull_review" },
        }],
        "run": {
            "post": { "state": "fallback", "summary": "Runs dry: Review cannot post here." },
            "skips": ["watching", "labels_and_title"],
            "applies": ["author"],
            "pr": { "number": 7, "author": "dependabot[bot]", "reviewed": false,
                    "reasons": [{ "code": "bot_author", "message": "Bot authors are skipped unless include_bots is on." }] },
            "overrides": { "available": false, "source": "REVIEW_RUN_OVERRIDES",
                           "reason": { "code": "run_overrides_off", "message": "Overrides are off." } },
            "model_override": null,
        },
    })
}

fn route(method: &str, path: &str, query: &str, body: &Value) -> Response {
    match (method, path) {
        ("GET", "/v1/modules") => answer(200, json!({ "modules": [manifest()], "unavailable": [] })),
        ("GET", "/v1/robustness/review/status") if query.contains("legacy") => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
        ("GET", "/v1/robustness/review/status") => answer(200, status_body()),
        ("POST", "/v1/robustness/review/connect") => answer(200, json!({ "ok": true, "repo": "acme/app" })),
        ("GET", "/v1/policies/grants") => answer(
            200,
            json!({
                "grants": [{ "module": "robustness/review", "connector": "github", "action": "pull_review",
                             "grantedBy": "usr_1", "at": "2026-11-01T00:00:00.000Z" }],
                "grantable": { "github": ["pull_review", "check_run"] },
            }),
        ),
        ("PUT", "/v1/policies/grants/robustness/review") => {
            let actions = body["actions"].clone();
            answer(
                200,
                json!({ "module": "robustness/review", "connector": body["connector"], "actions": actions,
                        "granted": actions, "revoked": [] }),
            )
        }
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
                    let query = request.uri().query().unwrap_or("").to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let response = route(&method, &path, &query, &body);
                    log.lock().expect("log").push(Seen {
                        method,
                        path,
                        query,
                        body,
                    });
                    response
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

impl Run {
    /// The calls past the manifest read.
    fn calls(&self) -> Vec<&Seen> {
        self.seen.iter().filter(|seen| seen.path != "/v1/modules").collect()
    }

    /// The one write among the calls. The client may read the grant list first for its `ETag`; any
    /// other call is a read.
    fn edit(&self) -> &Seen {
        let calls = self.calls();
        let (writes, reads): (Vec<&Seen>, Vec<&Seen>) = calls.iter().partition(|seen| seen.method != "GET");
        assert_eq!(
            writes.len(),
            1,
            "one write: {:?}",
            calls.iter().map(|seen| &seen.path).collect::<Vec<_>>()
        );
        assert!(reads.iter().all(|seen| seen.path == "/v1/policies/grants"));
        writes[0]
    }
}

fn run(argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-review-status-{}-{}",
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
fn status_reads_the_contract_with_only_the_status_inputs() {
    let status = run(&[
        "robustness",
        "status",
        "review",
        "--repo",
        "acme/app",
        "--pr",
        "7",
        "--effort",
        "deep",
    ]);
    assert_eq!(status.code, 0, "{}", status.err);
    let call = status.calls()[0].clone();
    assert_eq!(
        (call.method.as_str(), call.path.as_str()),
        ("GET", "/v1/robustness/review/status")
    );
    // `effort` is a connect input: it is not sent to status.
    assert_eq!(call.query, "repo=acme%2Fapp&pr=7");
    for expected in [
        "acme/app  limited",
        "Review stores findings here without posting",
        "automatic         Reviews pull requests on opened into main.",
        "posting           Runs as shadow",
        "Warnings",
        "nexus policy grants set robustness/review --connector github --actions pull_review",
        "post to GitHub  Runs dry",
        "skips    watching, labels and title",
        "applies  author",
        "#7 by dependabot[bot]: the author filter skips it",
        "bot_author",
        "overrides  off: Overrides are off.",
    ] {
        assert!(status.out.contains(expected), "{expected}\n{}", status.out);
    }
}

#[test]
fn status_json_prints_the_raw_answer() {
    let status = run(&["robustness", "status", "review", "--repo", "acme/app", "--json"]);
    assert_eq!(status.code, 0, "{}", status.err);
    let printed: Value = serde_json::from_str(&status.out).expect("json");
    assert_eq!(printed["effective"]["readiness"], "limited");
}

#[test]
fn a_404_on_status_says_the_backend_predates_it() {
    let status = run(&["robustness", "status", "review", "--repo", "legacy/app"]);
    assert_eq!(status.code, 1, "{}", status.out);
    assert!(status.err.contains("predates the status contract"), "{}", status.err);
}

#[test]
fn dotted_manifest_inputs_are_connect_flags() {
    let connected = run(&[
        "robustness",
        "connect",
        "review",
        "--repo",
        "acme/app",
        "--effort",
        "standard",
        "--output.incremental",
        "on",
    ]);
    assert_eq!(connected.code, 0, "{}", connected.err);
    let call = connected.calls()[0].clone();
    assert_eq!(call.path, "/v1/robustness/review/connect");
    assert_eq!(call.body["effort"], "standard");
    assert_eq!(call.body["output.incremental"], "on");
}

#[test]
fn grants_list_prints_each_grant_and_what_may_be_granted() {
    let listed = run(&["policy", "grants"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.calls()[0].path, "/v1/policies/grants");
    assert!(
        listed.out.contains("robustness/review       github    pull_review"),
        "{}",
        listed.out
    );
    assert!(
        listed.out.contains("Grantable  github: pull_review, check_run"),
        "{}",
        listed.out
    );
    assert_eq!(run(&["policy", "grants", "list"]).code, 0);
}

#[test]
fn grants_set_parses_the_command_the_status_contract_prints() {
    let set = run(&[
        "policy",
        "grants",
        "set",
        "robustness/review",
        "--connector",
        "github",
        "--actions",
        "pull_review,check_run",
    ]);
    assert_eq!(set.code, 0, "{}", set.err);
    let call = set.edit();
    assert_eq!(
        (call.method.as_str(), call.path.as_str()),
        ("PUT", "/v1/policies/grants/robustness/review")
    );
    assert_eq!(
        call.body,
        json!({ "connector": "github", "actions": ["pull_review", "check_run"] })
    );
    assert!(
        set.out
            .contains("robustness/review now holds github: pull_review, check_run."),
        "{}",
        set.out
    );
}

#[test]
fn grants_for_mode_sets_exactly_the_modes_minimum() {
    for (mode, actions) in [
        ("shadow", json!([])),
        ("comment", json!(["pull_review"])),
        ("comment_and_check", json!(["pull_review", "check_run"])),
    ] {
        let set = run(&["policy", "grants", "set", "robustness/review", "--for-mode", mode]);
        assert_eq!(set.code, 0, "{mode}: {}", set.err);
        assert_eq!(
            set.edit().body,
            json!({ "connector": "github", "actions": actions }),
            "{mode}"
        );
    }
}

#[test]
fn grants_set_refuses_what_it_cannot_send() {
    for argv in [
        vec![
            "policy",
            "grants",
            "set",
            "robustness/review",
            "--for-mode",
            "comment",
            "--actions",
            "x",
        ],
        vec!["policy", "grants", "set", "robustness/review", "--for-mode", "loud"],
        vec!["policy", "grants", "set", "intelligence/wiki", "--for-mode", "comment"],
        vec!["policy", "grants", "set", "robustness"],
        vec![
            "policy",
            "grants",
            "set",
            "robustness/review",
            "--actions",
            "pull_review",
        ],
        vec!["policy", "grants", "set", "robustness/review"],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}: {}", refused.out);
        assert!(refused.calls().is_empty(), "{argv:?}");
    }
}
