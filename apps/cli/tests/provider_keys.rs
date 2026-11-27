//! `nexus keys` for a stored provider key against a stand-in Gateway (`provider-keys.test.ts`): the list's
//! last use and state, what replace, disable, enable and remove send (a dry run says so), and a key's versions. A
//! team's or a person's key with `--team` or `--user`.

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

const KEY: &str = "nx_keys";
/// A caller who may read a team's and a person's keys: the list adds them.
const SCOPED: &str = "nx_keys_scoped";
const ADDED: &str = "2026-10-20T00:00:00.000Z";

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

fn route(method: &str, path: &str, body: &Value, scoped: bool) -> Response {
    let verb = path
        .strip_prefix("/v1/keys/providers/anthropic/")
        .filter(|verb| method == "POST" && ["replace", "disable", "enable", "remove"].contains(verb));
    if let Some(verb) = verb {
        let last4 = body["key"]
            .as_str()
            .map_or("0002".to_string(), |key| key[key.len() - 4..].to_string());
        let state = match verb {
            "replace" => "valid",
            "disable" => "disabled",
            "enable" => "unreachable",
            _ => "removed",
        };
        return answer(
            200,
            json!({
                "dryRun": body["dryRun"] == true, "changed": true,
                "providerKey": { "provider": "anthropic", "last4": last4, "createdAt": ADDED, "state": state },
            }),
        );
    }
    match (method, path) {
        ("GET", "/v1/keys/providers") => {
            let mut listed = json!({ "providers": [
                { "provider": "anthropic", "last4": "0002", "createdAt": ADDED, "state": "disabled", "unused": true, "notice": "disabled: no call uses it until `nexus keys enable anthropic`", "lastUsedOn": "2026-10-22" },
                { "provider": "openai", "last4": "0003", "createdAt": ADDED, "state": "valid" },
            ] });
            if scoped {
                listed["scopedKeys"] = json!([
                    { "provider": "anthropic", "scope": "team", "scopeId": "tm_web", "last4": "0004", "createdAt": ADDED, "state": "valid" },
                    { "provider": "openai", "scope": "user", "scopeId": "usr_bo", "last4": "0005", "createdAt": ADDED, "state": "valid" },
                ]);
            }
            answer(200, listed)
        }
        ("POST", "/v1/keys/providers") => {
            let key = body["key"].as_str().unwrap_or("0000");
            answer(
                200,
                json!({ "provider": body["provider"], "last4": &key[key.len() - 4..], "createdAt": ADDED, "state": "valid" }),
            )
        }
        ("GET", "/v1/keys/providers/anthropic") => answer(
            200,
            json!({
                "provider": "anthropic", "last4": "0002", "createdAt": ADDED, "state": "rejected",
                "versions": [
                    { "version": 1, "last4": "0001", "addedAt": ADDED },
                    { "version": 2, "last4": "0002", "addedAt": ADDED, "addedBy": "usr_ada" },
                ],
            }),
        ),
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
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_string();
                    let query = request
                        .uri()
                        .query()
                        .map(|query| format!("?{query}"))
                        .unwrap_or_default();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query,
                        body: body.clone(),
                    });
                    let auth = headers.get("authorization").and_then(|v| v.to_str().ok());
                    let scoped = auth == Some(&format!("Bearer {SCOPED}"));
                    if !scoped && auth != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&method, &path, &body, scoped)
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
    seen: Vec<Seen>,
}

fn run(argv: &[&str]) -> Run {
    run_as(KEY, argv)
}

/// [`run`] signed in with `key`.
fn run_as(key: &str, argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-provider-keys-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": key, "org": "acme" }).to_string(),
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
        seen,
    }
}

fn lines(run: &Run) -> Vec<&str> {
    run.out.lines().filter(|line| !line.trim().is_empty()).collect()
}

#[test]
fn lists_each_key_s_last_use_and_state() {
    let listed = run(&["keys", "list"]);
    assert_eq!(listed.code, 0, "{}", listed.out);
    let lines = lines(&listed);
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["Provider", "Key", "Added", "Last", "used", "Status"]
    );
    assert!(
        lines[1].contains("2026-10-22") && lines[1].ends_with("! disabled: no call uses it"),
        "{}",
        listed.out
    );
    assert!(
        lines[2].contains("never") && lines[2].ends_with("✓ in use"),
        "{}",
        listed.out
    );
}

#[test]
fn sends_each_change_with_its_dry_run_and_says_what_it_did() {
    let replacement = ["sk-ant-", "test-", "9999"].concat();
    let cases = [
        (
            vec!["keys", "disable", "anthropic", "--dry-run"],
            "disable",
            json!({ "dryRun": true }),
            "! Would disable your anthropic key ••••0002. Run without --dry-run to disable it.",
        ),
        (
            vec!["keys", "replace", "anthropic", replacement.as_str()],
            "replace",
            json!({ "key": replacement, "dryRun": false }),
            "✓ Replaced your anthropic key ••••9999 (checked).",
        ),
        (
            vec!["keys", "enable", "anthropic"],
            "enable",
            json!({ "dryRun": false }),
            "✓ Enabled your anthropic key ••••0002 (not checked by the provider).",
        ),
        (
            vec!["keys", "remove", "anthropic"],
            "remove",
            json!({ "dryRun": false }),
            "✓ Removed your anthropic key ••••0002. Its versions stay: nexus keys history anthropic",
        ),
    ];
    for (argv, verb, body, line) in cases {
        let changed = run(&argv);
        assert_eq!(changed.code, 0, "{}", changed.out);
        let post = changed.seen.iter().find(|call| call.method == "POST").expect("post");
        assert_eq!(post.path, format!("/v1/keys/providers/anthropic/{verb}"));
        assert_eq!(post.body, body);
        assert_eq!(changed.out, line);
    }
}

#[test]
fn shows_a_key_s_versions_never_its_value() {
    let history = run(&["keys", "history", "anthropic"]);
    assert_eq!(history.code, 0, "{}", history.out);
    let lines = lines(&history);
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["Version", "Key", "Added", "By"]
    );
    assert!(
        lines[1].starts_with("· 1") && lines[1].contains("••••0001") && lines[1].ends_with('–'),
        "{}",
        history.out
    );
    assert!(
        lines[2].starts_with("· 2") && lines[2].ends_with("usr_ada"),
        "{}",
        history.out
    );
    assert_eq!(
        lines[3],
        "Now: ! rejected by the provider: replace it, or enable it to check it again"
    );
}

#[test]
fn names_a_team_s_or_a_person_s_key_with_team_or_user() {
    let added = ["sk-ant-", "test-", "4444"].concat();
    let saved = run(&["keys", "add", "anthropic", added.as_str(), "--team", "web"]);
    assert_eq!(saved.code, 0, "{}", saved.out);
    let post = saved.seen.iter().find(|call| call.method == "POST").expect("post");
    assert_eq!(
        (post.path.as_str(), &post.body),
        (
            "/v1/keys/providers",
            &json!({ "provider": "anthropic", "key": added, "team": "web" })
        )
    );
    assert_eq!(
        saved.out,
        "✓ Saved your anthropic key ••••4444 for team web. Encrypted at rest; calls for team web use it before the organization's key."
    );
    let disabled = run(&["keys", "disable", "anthropic", "--user", "me"]);
    let post = disabled.seen.iter().find(|call| call.method == "POST").expect("post");
    assert_eq!(post.body, json!({ "dryRun": false, "user": "me" }));
    assert_eq!(disabled.out, "✓ Disabled your anthropic key ••••0002 for you.");
    let removed = run(&["keys", "remove", "anthropic", "--team", "web"]);
    assert_eq!(
        removed.out,
        "✓ Removed your anthropic key ••••0002 for team web. Its versions stay: nexus keys history anthropic --team web"
    );
    let history = run(&["keys", "history", "anthropic", "--team", "web"]);
    let read = history.seen.iter().find(|call| call.method == "GET").expect("get");
    assert_eq!(
        (read.path.as_str(), read.query.as_str()),
        ("/v1/keys/providers/anthropic", "?team=web")
    );

    // A caller who may read scoped keys sees each key's holder.
    let listed = run_as(SCOPED, &["keys", "list"]);
    let lines = lines(&listed);
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["Provider", "Holder", "Key", "Added", "Last", "used", "Status"]
    );
    assert!(
        lines[1].contains("organization") && lines[1].contains("••••0002"),
        "{}",
        listed.out
    );
    assert!(
        lines[3].contains("team tm_web") && lines[3].contains("••••0004"),
        "{}",
        listed.out
    );
    assert!(
        lines[4].contains("usr_bo") && lines[4].contains("••••0005"),
        "{}",
        listed.out
    );
    let json = run_as(SCOPED, &["keys", "list", "--json"]);
    let printed: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(printed["scopedKeys"].as_array().map(Vec::len), Some(2));
    let plain = run(&["keys", "list", "--json"]);
    let printed: Value = serde_json::from_str(&plain.out).expect("json");
    assert!(printed.get("scopedKeys").is_none(), "{printed}");

    // Both names two holders: refused before any request.
    for argv in [
        vec!["keys", "disable", "anthropic", "--team", "web", "--user", "me"],
        vec![
            "keys",
            "add",
            "anthropic",
            added.as_str(),
            "--team",
            "web",
            "--user",
            "me",
        ],
        vec!["keys", "history", "anthropic", "--team", "web", "--user", "me"],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}: {}", refused.out);
        assert!(refused.seen.is_empty(), "{argv:?}: {:?}", refused.seen);
    }
}
