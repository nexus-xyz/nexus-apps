//! `nexus pages` against a stand-in Gateway (`pages.test.ts`): the requests it sends (bodies, Idempotency-Keys), the
//! title and format it reads from the file, what it prints, `--json` passed through, the source to stdout or a file,
//! a stale `--base` surfacing Gateway's 409, the signed frame URL, and usage errors (exit 2) before anything is sent.

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

const KEY: &str = "nx_pages";
const SOURCE: &str = "<!doctype html><title>Q3 revenue</title><h1>Q3</h1>";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    key: Option<String>,
}

fn page() -> Value {
    json!({ "id": "pag_1", "title": "Q3 revenue", "format": "html", "latestVersion": 2, "createdAt": "2026-10-18T12:00:00.000Z", "updatedAt": "2026-10-18T13:00:00.000Z" })
}

fn v1() -> Value {
    json!({ "pageId": "pag_1", "version": 1, "title": "Q3 revenue", "contentSha256": "a".repeat(64), "bytes": 41, "createdVia": "cli", "createdAt": "2026-10-18T12:00:00.000Z" })
}

fn v2() -> Value {
    merge(
        v1(),
        json!({ "version": 2, "bytes": 52, "createdVia": "api", "createdAt": "2026-10-18T13:00:00.000Z" }),
    )
}

fn artifact() -> Value {
    json!({
        "id": "art_p", "version": 1, "sessionId": "pag_1", "title": "Q3 revenue", "mode": "page", "model": "",
        "models": [], "author": {}, "allowContinue": false, "status": "active", "mine": true, "messageCount": 0,
        "url": "https://nexus.example/app/acme/artifacts/art_p", "path": "/app/acme/artifacts/art_p",
        "markdownUrl": "", "createdAt": "2026-10-18T13:00:00.000Z", "kind": "page", "pageId": "pag_1",
        "pageFormat": "html",
    })
}

fn frame(version: i64) -> Value {
    json!({
        "url": format!("https://pages.example/v/pag_1/{version}?exp=1793000000&sig=abc"),
        "expiresAt": "2026-10-31T12:05:00.000Z", "pageId": "pag_1", "version": version, "format": "html",
        "title": "Q3 revenue",
    })
}

fn merge(base: Value, extra: Value) -> Value {
    let mut base = base;
    if let (Some(map), Value::Object(extra)) = (base.as_object_mut(), extra) {
        map.extend(extra);
    }
    base
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, body: &Value) -> Response {
    match (method, path) {
        ("POST", "/v1/pages") => answer(
            201,
            json!({
                "page": merge(page(), json!({ "id": "pag_new", "title": body["title"], "format": body["format"], "latestVersion": 1 })),
                "version": merge(v1(), json!({ "pageId": "pag_new", "title": body["title"] })),
            }),
        ),
        ("GET", "/v1/pages") => answer(200, json!({ "pages": [page()] })),
        ("GET", "/v1/pages/pag_1") => answer(200, merge(page(), json!({ "versions": [v2(), v1()] }))),
        ("GET", "/v1/pages/pag_1/versions/latest") => answer(200, v2()),
        ("GET", "/v1/pages/pag_1/versions/latest/source") | ("GET", "/v1/pages/pag_1/versions/1/source") => {
            ([("content-type", "text/plain; charset=utf-8")], SOURCE.to_string()).into_response()
        }
        (_, path) if path.starts_with("/v1/pages/pag_elsewhere/") => answer(
            404,
            json!({ "error": { "message": "no page pag_elsewhere", "type": "not_found", "code": "page_not_found" } }),
        ),
        ("POST", "/v1/pages/pag_1/versions") if body["baseVersion"] == json!(1) => answer(
            409,
            json!({ "error": { "message": "page pag_1 is at version 2, not 1: read the latest version and write on top of it", "type": "conflict_error", "code": "page_version_conflict" } }),
        ),
        ("POST", "/v1/pages/pag_1/versions") => answer(
            201,
            json!({ "page": merge(page(), json!({ "latestVersion": 3 })), "version": merge(v2(), json!({ "version": 3 })) }),
        ),
        ("POST", "/v1/pages/pag_1/restore") => answer(
            201,
            json!({
                "page": merge(page(), json!({ "latestVersion": 3 })),
                "version": merge(v1(), json!({ "version": 3, "createdVia": "restore", "restoredFrom": 1 })),
            }),
        ),
        ("GET", "/v1/pages/pag_1/versions/latest/frame") | ("GET", "/v1/artifacts/art_p/frame") => {
            answer(200, frame(2))
        }
        ("GET", "/v1/pages/pag_1/versions/1/frame") => answer(200, frame(1)),
        ("DELETE", "/v1/pages/pag_1") => answer(200, json!({ "deleted": true, "id": "pag_1", "versions": 2 })),
        ("POST", "/v1/pages/pag_1/share") => {
            let mut artifact = merge(
                artifact(),
                json!({ "audience": {
                    "kind": body["audience"],
                    "teams": body.get("teams").cloned().unwrap_or(json!([])),
                    "people": body.get("people").cloned().unwrap_or(json!([])),
                } }),
            );
            if let Some(version) = body.get("version") {
                artifact["pageVersion"] = version.clone();
            }
            if body["audience"] == "public" {
                artifact["publicUrl"] = json!("https://nexus.example/a/tok");
                artifact["expiresAt"] = json!("2026-10-25T12:00:00.000Z");
            }
            answer(201, artifact)
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
                        key: headers
                            .get("idempotency-key")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from),
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
    written: String,
    seen: Vec<Seen>,
    dir: PathBuf,
}

fn run(argv: &[&str]) -> Run {
    let dir = std::env::temp_dir().join(format!("nexus-pages-{}", uuid_like()));
    run_in(dir, argv)
}

/// A run in this working directory: what an earlier run there wrote is still there.
fn run_in(dir: PathBuf, argv: &[&str]) -> Run {
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        can_stream: true,
        ..Captured::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::from([("NEXUS_UI_URL".to_string(), "http://ui.test/app".to_string())]),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
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
        written: capture.0.written.borrow().clone(),
        seen,
        dir,
    }
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

#[test]
fn publishes_a_file_with_its_own_title_and_format() {
    let dir = std::env::temp_dir().join(format!("nexus-pages-{}", uuid_like()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("report.html"), SOURCE).expect("file");
    std::fs::write(dir.join("notes.md"), "Intro\n\n# Team notes\n").expect("file");
    let r = run_in(dir.clone(), &["pages", "publish", "report.html"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (r.seen[0].method.as_str(), r.seen[0].path.as_str()),
        ("POST", "/v1/pages")
    );
    assert_eq!(
        r.seen[0].body,
        json!({ "title": "Q3 revenue", "format": "html", "content": SOURCE })
    );
    assert!(r.seen[0].key.is_some());
    assert!(r.out.contains("✓ Published pag_new, version 1"), "{}", r.out);
    // The viewer link, so an agent that published can print it.
    assert!(
        r.out.contains("View it: ") && r.out.contains("/acme/pages/pag_new"),
        "{}",
        r.out
    );
    let r = run_in(dir, &["pages", "publish", "notes.md", "--title", "Mine"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body["title"], "Mine");
    assert_eq!(r.seen[0].body["format"], "md");
}

#[test]
fn lists_shows_a_version_and_its_versions() {
    let r = run(&["pages"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(5)
            .any(|w| w == ["pag_1", "Q3", "revenue", "html", "v2"]),
        "{}",
        r.out
    );
    let r = run(&["pages", "get", "pag_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("via api"), "{}", r.out);
    let r = run(&["pages", "versions", "pag_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(5)
            .any(|w| w == ["v1", "Q3", "revenue", "41", "cli"]),
        "{}",
        r.out
    );
    let r = run(&["pages", "list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(r.out.trim()).expect("json");
    assert_eq!(printed, json!({ "pages": [page()] }));
}

#[test]
fn prints_the_source_as_it_is_or_writes_it_to_a_file() {
    let r = run(&["pages", "get", "pag_1", "--source"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.written, format!("{SOURCE}\n"));
    let r = run(&["pages", "get", "pag_1", "1", "--out", "v1.html"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen.last().expect("seen").path, "/v1/pages/pag_1/versions/1/source");
    assert_eq!(std::fs::read_to_string(r.dir.join("v1.html")).expect("written"), SOURCE);
}

#[test]
fn names_the_gateway_and_organization_it_looked_in_when_there_is_no_page() {
    // Nexus for desktop can sign in to another Gateway than this CLI: "no page" alone reads as if the page were gone.
    let r = run(&["pages", "get", "pag_elsewhere", "1", "--source"]);
    assert_eq!(r.code, 1, "{}", r.err);
    assert!(
        r.err.contains("No page pag_elsewhere on Gateway http://127.0.0.1:") && r.err.contains("(organization acme)."),
        "{}",
        r.err
    );
    assert!(
        r.err.contains("NEXUS_GATEWAY=<its Gateway> nexus pages get"),
        "{}",
        r.err
    );
    let r = run(&["pages", "get", "pag_elsewhere", "--json"]);
    assert_eq!(r.code, 1, "{}", r.err);
    assert!(
        format!("{}{}", r.out, r.err).contains("page_not_found"),
        "{} {}",
        r.out,
        r.err
    );
}

#[test]
fn writes_a_version_on_a_base_and_surfaces_a_stale_base() {
    let dir = std::env::temp_dir().join(format!("nexus-pages-{}", uuid_like()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("report.html"), SOURCE).expect("file");
    let r = run_in(dir.clone(), &["pages", "update", "pag_1", "report.html", "--base", "2"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body, json!({ "content": SOURCE, "baseVersion": 2 }));
    assert!(r.out.contains("✓ Wrote pag_1, version 3"), "{}", r.out);
    assert!(r.out.contains("/acme/pages/pag_1"), "{}", r.out);
    let r = run_in(dir, &["pages", "update", "pag_1", "report.html", "--base", "1"]);
    assert_ne!(r.code, 0);
    assert!(r.err.contains("is at version 2, not 1"), "{}", r.err);
}

#[test]
fn restores_and_deletes_with_yes() {
    let r = run(&["pages", "restore", "pag_1", "1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].path, "/v1/pages/pag_1/restore");
    assert_eq!(r.seen[0].body, json!({ "version": 1 }));
    let r = run(&["pages", "delete", "pag_1"]);
    assert_eq!(r.code, 2);
    assert!(r.seen.is_empty());
    let r = run(&["pages", "delete", "pag_1", "--yes"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("✓ Deleted pag_1 and its 2 versions."), "{}", r.out);
}

#[test]
fn shares_a_page_with_one_audience_pinned_or_following_the_latest_version() {
    let r = run(&["pages", "share", "pag_1", "--team", "web,data", "--pin", "1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].path, "/v1/pages/pag_1/share");
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "teams", "teams": ["web", "data"], "version": 1 })
    );
    assert!(r.out.contains("v1, pinned"), "{}", r.out);
    let r = run(&["pages", "share", "pag_1", "--public", "--expires", "7d"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body, json!({ "audience": "public", "expiresIn": "7d" }));
    assert!(r.out.contains("https://nexus.example/a/tok"), "{}", r.out);
    assert!(r.out.contains("the latest, as you write new ones"), "{}", r.out);
    let r = run(&["pages", "share", "pag_1", "--to", "bo@partner.example", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "external", "people": ["bo@partner.example"] })
    );
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(printed["kind"], "page");
}

#[test]
fn prints_the_signed_frame_url_of_the_latest_version_a_named_one_or_a_shared_page() {
    let r = run(&["pages", "frame", "pag_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].path, "/v1/pages/pag_1/versions/latest/frame");
    // The URL alone, so an agent can pass it to its browser tool.
    assert_eq!(r.out, frame(2)["url"].as_str().expect("url"));
    let r = run(&["pages", "frame", "pag_1", "1", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].path, "/v1/pages/pag_1/versions/1/frame");
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(printed, frame(1));
    let r = run(&["pages", "frame", "art_p"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].path, "/v1/artifacts/art_p/frame");
}

#[test]
fn refuses_bad_usage_before_sending_anything() {
    for argv in [
        &["pages", "publish"][..],
        &["pages", "publish", "missing.html"],
        &["pages", "restore", "pag_1", "zero"],
        &["pages", "update", "pag_1", "x.html", "--base", "0"],
        &["pages", "frobnicate"],
        &["pages", "share", "pag_1"],
        &["pages", "share", "pag_1", "--org", "--public"],
        &["pages", "share", "pag_1", "--org", "--expires", "7d"],
        &["pages", "share", "pag_1", "--org", "--pin", "0"],
        &["pages", "frame"],
        &["pages", "frame", "pag_1", "0"],
        &["pages", "frame", "art_p", "2"],
    ] {
        let r = run(argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}
