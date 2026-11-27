//! `nexus chat share`, `nexus chat export` and `nexus artifacts` against a stand-in Gateway (`artifacts.test.ts`):
//! the requests they send (bodies, Idempotency-Keys), what they print, `--json` passed through, files written with
//! `--out`, and usage errors (exit 2) before anything is sent.

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

const KEY: &str = "nx_artifacts";
const MARKDOWN: &str = "---\ntitle: \"How do we deploy?\"\n---\n\n# How do we deploy?\n";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    key: Option<String>,
}

fn artifact() -> Value {
    json!({
        "id": "art_1", "version": 2, "sessionId": "chs_1", "title": "How do we deploy?", "mode": "chat",
        "model": "nexus/auto", "models": ["anthropic/claude-sonnet-5.5"],
        "author": { "email": "ada@acme.test", "name": "Ada" },
        "audience": { "kind": "teams", "teams": ["web"], "people": [] },
        "allowContinue": true, "status": "active", "mine": false, "latest": true, "messageCount": 2, "costUsd": 0.000123,
        "url": "http://ui.test/app/acme/artifacts/art_1", "path": "/app/acme/artifacts/art_1",
        "markdownUrl": "http://core.test/v1/artifacts/art_1.md", "createdAt": "2026-10-18T12:00:00.000Z",
    })
}

fn external() -> Value {
    merge(
        artifact(),
        json!({
            "id": "art_x", "audience": { "kind": "external", "teams": [], "people": ["ada@partner.test", "bo@partner.test"] },
            "external": true, "includeCode": false, "url": "http://ui.test/a/art_x", "viewerUrl": "http://ui.test/a/art_x",
        }),
    )
}

fn public() -> Value {
    merge(
        artifact(),
        json!({
            "id": "art_p", "audience": { "kind": "public", "teams": [], "people": [] }, "external": true, "includeCode": true,
            "url": "http://ui.test/a/tok_public", "publicUrl": "http://ui.test/a/tok_public", "expiresAt": "2026-11-17T12:00:00.000Z",
        }),
    )
}

fn access() -> Value {
    json!({
        "id": "art_x", "audience": "external", "status": "active",
        "people": [
            { "email": "ada@partner.test", "status": "granted", "opens": 3, "lastOpenedAt": "2026-10-18T12:00:00.000Z", "grantedAt": "2026-10-18T11:00:00.000Z" },
            { "email": "cy@partner.test", "status": "requested", "opens": 0, "requestedAt": "2026-10-18T12:00:00.000Z" },
        ],
        "opens": 3, "reports": 0,
    })
}

fn settings() -> Value {
    let values = json!({
        "chatRetentionDays": 90,
        "artifacts.external": "public",
        "artifacts.external.code": false,
        "artifacts.external.domains": { "allow": [], "deny": [] },
        "artifacts.external.expiry": { "maxDays": null, "required": false },
        "artifacts.external.continue": true,
        "artifacts.external.who": "everyone",
    });
    merge(values.clone(), json!({ "defaults": values }))
}

fn messages() -> Value {
    json!([
        { "id": "chm_1", "sessionId": "chs_1", "role": "user", "content": "How do we deploy?", "status": "complete", "createdAt": "2026-10-18T11:00:00.000Z" },
        { "id": "chm_2", "sessionId": "chs_1", "role": "assistant", "content": "With `fly deploy`.", "status": "complete", "model": "anthropic/claude-sonnet-5.5", "costUsd": 0.000123, "createdAt": "2026-10-18T11:00:01.000Z" },
    ])
}

/// A conversation with images: two named `shot.png` in the first question, one without a name in the third.
fn image_messages(session: &str) -> Value {
    json!([
        {
            "id": "chm_1", "sessionId": session, "role": "user", "content": "Why is this cut off?", "status": "complete", "createdAt": "2026-10-18T11:00:00.000Z",
            "attachments": [
                { "id": "cha_1", "type": "image", "mediaType": "image/png", "bytes": 8, "width": 1600, "height": 1000, "name": "shot.png" },
                { "id": "cha_2", "type": "image", "mediaType": "image/png", "bytes": 8, "name": "shot.png" },
            ],
        },
        { "id": "chm_2", "sessionId": session, "role": "assistant", "content": "It overflows.", "status": "complete", "createdAt": "2026-10-18T11:00:01.000Z" },
        {
            "id": "chm_3", "sessionId": session, "role": "user", "content": "", "status": "complete", "createdAt": "2026-10-18T11:00:02.000Z",
            "attachments": [{ "id": "cha_3", "type": "image", "mediaType": "image/jpeg", "bytes": 4 }],
        },
    ])
}

/// The PNG signature, and a JPEG's first bytes: what Gateway answers for each image (base64 in `data`).
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0];

/// `GET …/attachments/{attachmentId}` of a conversation with images: one image with its bytes.
fn attachment(path: &str) -> Option<Response> {
    let (_, image) = path.rsplit_once("/attachments/")?;
    let (message, data) = match image {
        "cha_1" => ("chm_1", "iVBORw0KGgo="),
        "cha_2" => ("chm_1", "iVBORw0KGgo="),
        "cha_3" => ("chm_3", "/9j/4A=="),
        _ => {
            return Some(answer(
                404,
                json!({ "error": { "message": "no such image", "type": "not_found_error" } }),
            ));
        }
    };
    Some(answer(
        200,
        json!({ "id": image, "messageId": message, "data": data, "createdAt": "2026-10-18T11:00:00.000Z" }),
    ))
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
    let markdown = || ([("content-type", "text/markdown; charset=utf-8")], MARKDOWN.to_string()).into_response();
    if method == "GET"
        && let Some(image) = attachment(path)
    {
        return image;
    }
    match (method, path) {
        ("GET", "/v1/chat/sessions/chs_img") => answer(
            200,
            json!({ "id": "chs_img", "title": "Cut off", "mode": "chat", "model": "nexus/auto", "messages": image_messages("chs_img") }),
        ),
        ("GET", "/v1/artifacts/art_img") | ("GET", "/v1/public/artifacts/tok_public") => answer(
            200,
            merge(
                public(),
                json!({ "id": "art_img", "messages": image_messages("chs_img"), "hiddenSources": 0 }),
            ),
        ),
        ("POST", "/v1/chat/sessions/chs_1/share") => answer(
            201,
            merge(
                artifact(),
                json!({ "mine": true, "allowContinue": body["allowContinue"].as_bool().unwrap_or(false) }),
            ),
        ),
        ("POST", "/v1/chat/sessions/chs_2/share") => {
            if body["audience"] == "public" {
                answer(
                    201,
                    merge(
                        public(),
                        json!({ "mine": true, "includeCode": body["includeCode"].as_bool().unwrap_or(false) }),
                    ),
                )
            } else {
                answer(
                    201,
                    merge(
                        external(),
                        json!({ "mine": true, "audience": { "kind": "external", "teams": [], "people": body["people"].clone() } }),
                    ),
                )
            }
        }
        ("POST", "/v1/chat/sessions/chs_off/share") => answer(
            403,
            json!({ "error": { "message": "your organization has turned off sharing outside it", "type": "permission_error", "code": "external_sharing_off" } }),
        ),
        ("GET", "/v1/artifacts/external") => answer(
            200,
            json!({
                "artifacts": [
                    merge(external(), json!({ "opens": 3, "readers": 2, "requests": 1, "reports": 0 })),
                    merge(public(), json!({ "opens": 12, "readers": 0, "requests": 0, "reports": 1 })),
                ],
                "totals": { "active": 2, "named": 1, "public": 1 },
            }),
        ),
        ("GET", "/v1/artifacts/art_x") => {
            let mut detail = merge(
                external(),
                json!({ "author": { "name": "Ada" }, "organization": "Acme", "variant": "external", "hiddenSources": 2, "versions": [] }),
            );
            detail.as_object_mut().expect("object").remove("costUsd");
            let mut messages = messages();
            for message in messages.as_array_mut().expect("array") {
                message.as_object_mut().expect("object").remove("costUsd");
            }
            detail["messages"] = messages;
            answer(200, detail)
        }
        ("GET", "/v1/artifacts/art_x/access") => answer(200, access()),
        ("POST", "/v1/artifacts/art_x/access") => {
            let mut granted = access();
            for person in granted["people"].as_array_mut().expect("array") {
                person["status"] = json!("granted");
            }
            answer(200, granted)
        }
        ("POST", "/v1/artifacts/art_x/access-requests") => {
            answer(202, json!({ "status": "requested", "requested": true }))
        }
        ("POST", "/v1/artifacts/revoke") => answer(
            200,
            json!({ "revoked": [{ "id": "art_1", "version": 2 }], "skipped": [{ "id": "art_gone", "reason": "not found" }] }),
        ),
        ("GET", "/v1/orgs/settings") => answer(200, settings()),
        ("PUT", "/v1/orgs/settings") => answer(200, merge(settings(), body.clone())),
        ("POST", "/v1/chat/sessions/chs_x/share") => answer(
            400,
            json!({ "error": { "message": "nobody@else.test is not in your organization", "type": "invalid_request_error", "code": "not_a_member" } }),
        ),
        ("GET", "/v1/chat/sessions/chs_1/export.md") | ("GET", "/v1/artifacts/art_1.md") => markdown(),
        ("POST", "/v1/chat/sessions") => answer(
            201,
            json!({ "id": "chs_web", "mode": "chat", "webSearch": body["webSearch"] }),
        ),
        ("GET", "/v1/chat/sessions/chs_on") => {
            answer(200, json!({ "id": "chs_on", "mode": "chat", "webSearch": true }))
        }
        ("PATCH", "/v1/chat/sessions/chs_on") => answer(
            200,
            json!({ "id": "chs_on", "mode": "chat", "webSearch": body["webSearch"] }),
        ),
        ("POST", "/v1/chat/sessions/chs_1/messages")
        | ("POST", "/v1/chat/sessions/chs_web/messages")
        | ("POST", "/v1/chat/sessions/chs_on/messages") => answer(200, json!({ "messages": [] })),
        ("GET", "/v1/chat/sessions/chs_1") => answer(
            200,
            json!({
                "id": "chs_1", "title": "How do we deploy?", "mode": "chat", "model": "nexus/auto", "archived": false,
                "messageCount": 2, "costUsd": 0.000123, "createdAt": "2026-10-18T12:00:00.000Z", "updatedAt": "2026-10-18T12:00:00.000Z",
                "messages": messages(), "visibility": "shared", "author": { "email": "ada@acme.test", "name": "Ada" },
                "sharing": { "artifactId": "art_1", "version": 2, "audience": { "kind": "teams", "teams": ["web"], "people": [] }, "allowContinue": true, "url": "http://ui.test/app/acme/artifacts/art_1", "path": "/app/acme/artifacts/art_1" },
                "artifacts": [
                    { "id": "art_1", "version": 2, "status": "active", "audience": "teams", "createdAt": "2026-10-18T12:00:00.000Z" },
                    { "id": "art_0", "version": 1, "status": "revoked", "audience": "org", "createdAt": "2026-10-17T12:00:00.000Z", "revokedAt": "2026-10-18T12:00:00.000Z" },
                ],
            }),
        ),
        ("GET", "/v1/artifacts") => answer(
            200,
            json!({ "artifacts": [artifact(), merge(artifact(), json!({ "id": "art_2", "mine": true, "version": 1, "audience": { "kind": "org", "teams": [], "people": [] } }))] }),
        ),
        ("GET", "/v1/artifacts/art_1") => answer(
            200,
            merge(
                artifact(),
                json!({ "messages": messages(), "hiddenSources": 1, "versions": [] }),
            ),
        ),
        ("GET", "/v1/artifacts/art_gone") => answer(
            404,
            json!({ "error": { "message": "no artifact art_gone", "type": "not_found_error", "code": "artifact_not_found" } }),
        ),
        ("DELETE", "/v1/artifacts/art_1") => answer(200, json!({ "revoked": true, "id": "art_1", "version": 2 })),
        ("DELETE", "/v1/chat/sessions/chs_1/share") => answer(
            200,
            json!({ "sessionId": "chs_1", "visibility": "private", "revoked": [{ "id": "art_0", "version": 1 }, { "id": "art_1", "version": 2 }] }),
        ),
        ("POST", "/v1/artifacts/art_1/continue") => answer(
            201,
            json!({
                "id": "chs_2", "title": "How do we deploy?", "mode": "chat", "model": "nexus/auto", "archived": false,
                "messageCount": 2, "costUsd": 0, "createdAt": "2026-10-18T12:00:00.000Z", "updatedAt": "2026-10-18T12:00:00.000Z",
                "visibility": "private", "forkedFrom": { "artifactId": "art_1", "version": 2, "title": "How do we deploy?" },
            }),
        ),
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
    config_path: PathBuf,
}

fn run(argv: &[&str]) -> Run {
    let dir = std::env::temp_dir().join(format!("nexus-artifacts-{}", uuid_like()));
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
        config_path: config_path.clone(),
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
        config_path,
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
fn shares_with_chosen_teams_and_says_where_it_is() {
    let r = run(&["chat", "share", "chs_1", "--team", "web,api", "--allow-continue"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].method, "POST");
    assert_eq!(r.seen[0].path, "/v1/chat/sessions/chs_1/share");
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "teams", "teams": ["web", "api"], "allowContinue": true })
    );
    assert!(r.seen[0].key.is_some());
    assert!(r.out.contains("✓ Shared How do we deploy? with team web (version 2)."));
    assert!(r.out.contains("http://ui.test/app/acme/artifacts/art_1"));
    assert!(r.out.contains("http://core.test/v1/artifacts/art_1.md"));
    assert!(r.out.contains("may continue it in a private chat of their own"));
    assert!(r.out.contains("nexus artifacts revoke art_1"));
}

#[test]
fn shares_with_the_organization_or_people_and_passes_json_through() {
    let org = run(&["chat", "share", "chs_1", "--org", "--json"]);
    assert_eq!(org.code, 0, "{}", org.err);
    assert_eq!(org.seen[0].body, json!({ "audience": "org", "allowContinue": false }));
    let printed: Value = serde_json::from_str(&org.out).expect("json");
    assert_eq!(printed["id"], "art_1");

    let people = run(&["chat", "share", "chs_1", "--people", "ada@acme.test, bo@acme.test"]);
    assert_eq!(
        people.seen[0].body,
        json!({ "audience": "people", "people": ["ada@acme.test", "bo@acme.test"], "allowContinue": false })
    );
    let refused = run(&["chat", "share", "chs_x", "--people", "nobody@else.test"]);
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("nobody@else.test is not in your organization"));
}

#[test]
fn refuses_no_audience_two_audiences_or_an_empty_list_before_calling_gateway() {
    for argv in [
        vec!["chat", "share", "chs_1"],
        vec!["chat", "share", "chs_1", "--org", "--team", "web"],
        vec!["chat", "share", "chs_1", "--team"],
        vec!["chat", "share"],
        vec!["artifacts", "--mine", "--shared-with-me"],
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 2, "{argv:?}");
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn replace_and_private_follow_the_share_menu() {
    let replaced = run(&["chat", "share", "chs_1", "--org", "--replace"]);
    assert_eq!(replaced.code, 0, "{}", replaced.err);
    assert_eq!(
        replaced.seen[0].body,
        json!({ "audience": "org", "allowContinue": false, "replace": true })
    );
    let private = run(&["chat", "share", "chs_1", "--private"]);
    assert_eq!(private.code, 0, "{}", private.err);
    assert_eq!(
        (private.seen[0].method.as_str(), private.seen[0].path.as_str()),
        ("DELETE", "/v1/chat/sessions/chs_1/share")
    );
    assert!(private.out.contains("Only you can read chs_1 now: revoked v1, v2."));
    for argv in [
        vec!["chat", "share", "chs_1", "--private", "--org"],
        vec!["chat", "share", "chs_1", "--private", "--allow-continue"],
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 2, "{argv:?}");
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn exports_a_session_as_markdown_to_stdout_or_a_file() {
    let r = run(&["chat", "export", "chs_1", "--md"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.written, MARKDOWN);

    let file = run(&["chat", "export", "chs_1", "--out", "deploy.md"]);
    assert_eq!(file.code, 0, "{}", file.err);
    assert_eq!(
        std::fs::read_to_string(file.dir.join("deploy.md")).expect("written"),
        MARKDOWN
    );
    assert!(
        file.out
            .contains(&format!("✓ Wrote deploy.md ({} bytes).", MARKDOWN.len()))
    );
}

#[test]
fn shows_a_sessions_author_visibility_audience_and_versions() {
    let r = run(&["chat", "sessions", "show", "chs_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("Ada <ada@acme.test>"));
    assert!(
        r.out
            .contains("Shared with team web (version 2, readers may continue it)")
    );
    assert!(r.out.contains("v2 art_1 (active), v1 art_0 (revoked)"));
}

#[test]
fn lists_shows_exports_continues_and_revokes_artifacts() {
    let list = run(&["artifacts"]);
    assert_eq!(list.code, 0, "{}", list.err);
    assert_eq!(list.seen[0].path, "/v1/artifacts");
    assert!(list.out.contains("Ada <ada@acme.test>"));
    assert!(
        list.out
            .lines()
            .any(|line| line.starts_with("· art_2") && line.contains("you") && line.contains("your organization"))
    );

    let mine = run(&["artifacts", "list", "--mine", "--all", "--json"]);
    assert_eq!(mine.seen[0].path, "/v1/artifacts?scope=shared_by_me&status=all");

    let show = run(&["artifacts", "show", "art_1"]);
    assert_eq!(show.code, 0, "{}", show.err);
    assert!(show.out.contains("art_1 (version 2, active)"));
    assert!(show.out.contains("With `fly deploy`."));
    assert!(
        show.out
            .contains("1 source is hidden: from repositories you cannot see.")
    );
    assert_eq!(run(&["artifacts", "show", "art_gone"]).code, 1);

    let export = run(&["artifacts", "export", "art_1", "--out", "a.md", "--json"]);
    let printed: Value = serde_json::from_str(&export.out).expect("json");
    assert_eq!(
        printed,
        json!({ "id": "art_1", "path": "a.md", "bytes": MARKDOWN.len() })
    );

    let next = run(&["artifacts", "continue", "art_1"]);
    assert_eq!(next.code, 0, "{}", next.err);
    assert_eq!(next.seen[0].path, "/v1/artifacts/art_1/continue");
    assert!(next.out.contains("http://ui.test/app/acme/sessions/chs_2"));
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&next.config_path).expect("config")).expect("json");
    assert_eq!(config["chatSession"], "chs_2");

    let gone = run(&["artifacts", "revoke", "art_1"]);
    assert_eq!(gone.seen[0].method, "DELETE");
    assert!(gone.out.contains("Revoked art_1 (version 2)"));
}

#[test]
fn shares_with_named_outsiders_by_email() {
    let r = run(&[
        "chat",
        "share",
        "chs_2",
        "--to",
        "ada@partner.test,bo@partner.test",
        "--allow-continue",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "external", "people": ["ada@partner.test", "bo@partner.test"], "allowContinue": true, "includeCode": false })
    );
    assert!(r.out.contains(
        "✓ Shared How do we deploy? outside your organization with ada@partner.test, bo@partner.test (version 2)."
    ));
    assert!(r.out.contains("http://ui.test/a/art_x"));
    assert!(r.out.contains("Code sources left out"));
    assert!(r.out.contains("nexus artifacts access art_x"));
}

#[test]
fn shares_a_public_link_with_an_expiry_and_code() {
    let r = run(&[
        "chat",
        "share",
        "chs_2",
        "--public",
        "--expires",
        "30d",
        "--include-code",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen[0].body,
        json!({ "audience": "public", "allowContinue": false, "includeCode": true, "expiresIn": "30d" })
    );
    assert!(r.out.contains("with anyone with the link (version 2)."));
    assert!(r.out.contains("http://ui.test/a/tok_public"));
    assert!(r.out.contains("2026-11-17T12:00:00.000Z"));
    let days = run(&["chat", "share", "chs_2", "--public", "--expires", "7"]);
    assert_eq!(days.seen[0].body["expiresIn"], json!(7));
    let refused = run(&["chat", "share", "chs_off", "--public"]);
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("turned off sharing outside it"));
}

#[test]
fn refuses_bad_outside_shares_and_filters_before_calling_gateway() {
    for argv in [
        vec!["chat", "share", "chs_2", "--to", "a@x", "--expires", "30d"],
        vec!["chat", "share", "chs_2", "--public", "--expires", "soon"],
        vec!["chat", "share", "chs_2", "--org", "--include-code"],
        vec!["chat", "share", "chs_2", "--to"],
        vec!["chat", "share", "chs_2", "--to", "a@x", "--public"],
        vec!["artifacts", "list", "--external", "--audience", "everyone"],
        vec!["artifacts", "list", "--external", "--mine"],
        vec!["artifacts", "list", "--audience", "public"],
        vec!["artifacts", "access", "art_x", "--deny"],
        vec!["org", "settings", "set", "artifacts.external"],
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 2, "{argv:?}");
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn lists_everything_shared_outside_with_filters() {
    let r = run(&[
        "artifacts",
        "list",
        "--external",
        "--audience",
        "public",
        "--author",
        "ada@acme.test",
        "--team",
        "web",
        "--all",
        "--search",
        "deploy",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen[0].path,
        "/v1/artifacts/external?audience=public&author=ada%40acme.test&team=web&status=all&q=deploy"
    );
    assert!(
        r.out
            .lines()
            .any(|line| line.starts_with("· art_x") && line.contains("named"))
    );
    assert!(
        r.out
            .lines()
            .any(|line| line.starts_with("· art_p") && line.contains("2026-11-17"))
    );
    assert!(r.out.contains("2 active: 1 named, 1 public."));
}

#[test]
fn shows_an_artifact_from_outside_without_cost() {
    let r = run(&["artifacts", "show", "art_x"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("Acme"));
    assert!(r.out.contains("named (2 people)"));
    assert!(!r.out.contains("Cost"));
    assert!(
        r.out
            .contains("2 sources are hidden: left out of what is shared outside the organization.")
    );
}

#[test]
fn shows_and_changes_access_asks_for_it_and_revokes_in_bulk() {
    let r = run(&["artifacts", "access", "art_x"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (r.seen[0].method.as_str(), r.seen[0].path.as_str()),
        ("GET", "/v1/artifacts/art_x/access")
    );
    assert!(r.out.contains("cy@partner.test"));
    assert!(r.out.contains("nexus artifacts access art_x --grant <email>"));

    let grant = run(&[
        "artifacts",
        "access",
        "art_x",
        "--grant",
        "cy@partner.test",
        "--remove",
        "bo@partner.test",
    ]);
    assert_eq!(grant.code, 0, "{}", grant.err);
    assert_eq!(
        grant.seen[0].body,
        json!({ "grant": ["cy@partner.test"], "remove": ["bo@partner.test"] })
    );
    assert!(
        grant
            .out
            .contains("✓ Updated access to art_x: granted 1, denied 0, removed 1.")
    );

    let ask = run(&["artifacts", "request", "art_x", "--message", "for the review"]);
    assert_eq!(ask.code, 0, "{}", ask.err);
    assert_eq!(ask.seen[0].body, json!({ "message": "for the review" }));
    assert!(ask.out.contains("Asked for access to art_x"));

    let bulk = run(&["artifacts", "revoke", "art_1", "art_gone"]);
    assert_eq!(bulk.code, 1);
    assert_eq!(bulk.seen[0].body, json!({ "ids": ["art_1", "art_gone"] }));
    assert!(bulk.out.contains("✓ Revoked art_1 (version 2)."));
    assert!(bulk.out.contains("Skipped art_gone: not found"));
}

#[test]
fn lists_gets_and_sets_organization_settings() {
    let list = run(&["org", "settings"]);
    assert_eq!(list.code, 0, "{}", list.err);
    assert!(list.out.contains("{\"allow\":[],\"deny\":[]}"));
    assert!(
        list.out
            .lines()
            .any(|line| line.starts_with("· artifacts.external ") && line.contains("public"))
    );

    let get = run(&["org", "settings", "get", "artifacts.external.who", "--json"]);
    let printed: Value = serde_json::from_str(&get.out).expect("json");
    assert_eq!(
        printed,
        json!({ "key": "artifacts.external.who", "value": "everyone", "default": "everyone" })
    );
    assert_eq!(run(&["org", "settings", "get", "nope"]).code, 2);

    let set = run(&[
        "org",
        "settings",
        "set",
        "artifacts.external.domains",
        "{\"allow\":[\"partner.test\"],\"deny\":[]}",
    ]);
    assert_eq!(set.code, 0, "{}", set.err);
    // A one-shot edit reads the settings' version first, then sends the edit.
    let edit = set.seen.iter().find(|seen| seen.method == "PUT").expect("the edit");
    assert_eq!(
        edit.body,
        json!({ "artifacts.external.domains": { "allow": ["partner.test"], "deny": [] } })
    );
    let word = run(&["org", "settings", "set", "artifacts.external", "named"]);
    assert!(word.out.contains("✓ artifacts.external is now named."));
}

#[test]
fn saves_every_image_of_a_sessions_questions_and_overwrites_only_with_force() {
    let r = run(&["chat", "sessions", "images", "chs_img", "--out", "shots"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let paths: Vec<&str> = r.seen.iter().map(|seen| seen.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/v1/chat/sessions/chs_img",
            "/v1/chat/sessions/chs_img/attachments/cha_1",
            "/v1/chat/sessions/chs_img/attachments/cha_2",
            "/v1/chat/sessions/chs_img/attachments/cha_3",
        ]
    );
    // Two images named shot.png: the second gets a number. One without a name: its id and its type's extension.
    assert_eq!(std::fs::read(r.dir.join("shots/shot.png")).expect("shot.png"), PNG);
    assert_eq!(std::fs::read(r.dir.join("shots/shot-2.png")).expect("shot-2.png"), PNG);
    assert_eq!(std::fs::read(r.dir.join("shots/cha_3.jpg")).expect("cha_3.jpg"), JPEG);
    assert_eq!(
        r.out,
        "✓ Wrote shots/shot.png (1600 × 1000, 1 KB).\n✓ Wrote shots/shot-2.png (1 KB).\n✓ Wrote shots/cha_3.jpg (1 KB)."
    );

    // Again in the same place: refused before any image is read, and nothing changes.
    std::fs::write(r.dir.join("shots/cha_3.jpg"), b"mine").expect("write");
    let again = run_in(
        r.dir.clone(),
        &["chat", "sessions", "images", "chs_img", "--out", "shots"],
    );
    assert_eq!(again.code, 1);
    assert!(again.err.contains("shots/shot.png already exists."), "{}", again.err);
    assert!(again.err.contains("--force overwrites it"), "{}", again.err);
    assert_eq!(again.seen.len(), 1);
    assert_eq!(
        std::fs::read(r.dir.join("shots/cha_3.jpg")).expect("cha_3.jpg"),
        b"mine"
    );

    let forced = run_in(
        r.dir.clone(),
        &["chat", "sessions", "images", "chs_img", "--out", "shots", "--force"],
    );
    assert_eq!(forced.code, 0, "{}", forced.err);
    assert_eq!(std::fs::read(r.dir.join("shots/cha_3.jpg")).expect("cha_3.jpg"), JPEG);
}

#[test]
fn saves_the_images_named_and_lists_what_it_wrote_as_json() {
    let r = run(&["chat", "sessions", "images", "chs_img", "cha_3", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen.len(), 2);
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(
        printed,
        json!({ "id": "chs_img", "images": [{ "id": "cha_3", "messageId": "chm_3", "path": "cha_3.jpg", "mediaType": "image/jpeg", "bytes": 4 }] })
    );
    assert_eq!(std::fs::read(r.dir.join("cha_3.jpg")).expect("cha_3.jpg"), JPEG);

    let unknown = run(&["chat", "sessions", "images", "chs_img", "cha_9"]);
    assert_eq!(unknown.code, 1);
    assert!(unknown.err.contains("No image cha_9 in chs_img."), "{}", unknown.err);

    let none = run(&["chat", "sessions", "images", "chs_1"]);
    assert_eq!(none.code, 0, "{}", none.err);
    assert_eq!(none.out, "No images in chs_1.");

    for argv in [
        vec!["chat", "sessions", "images"],
        vec!["chat", "sessions", "images", "chs_img", "--out"],
        vec!["artifacts", "images", "https://nexus.test/"],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn saves_an_artifacts_images_by_its_id_its_public_token_or_its_link() {
    let member = run(&["artifacts", "images", "art_img", "--json"]);
    assert_eq!(member.code, 0, "{}", member.err);
    assert_eq!(member.seen[0].path, "/v1/artifacts/art_img");
    assert_eq!(member.seen[3].path, "/v1/artifacts/art_img/attachments/cha_3");
    let printed: Value = serde_json::from_str(&member.out).expect("json");
    assert_eq!(printed["id"], "art_img");
    assert_eq!(printed["images"][1]["path"], "shot-2.png");

    for named in ["tok_public", "http://ui.test/a/tok_public"] {
        let public = run(&["artifacts", "images", named, "cha_1"]);
        assert_eq!(public.code, 0, "{}", public.err);
        let paths: Vec<&str> = public.seen.iter().map(|seen| seen.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/v1/public/artifacts/tok_public",
                "/v1/public/artifacts/tok_public/attachments/cha_1"
            ]
        );
        assert_eq!(public.out, "✓ Wrote shot.png (1600 × 1000, 1 KB).");
        assert_eq!(std::fs::read(public.dir.join("shot.png")).expect("shot.png"), PNG);
    }
}

/// An empty `--image` (`--image=`, or `--image "$IMG"` with the variable unset) is refused as chat.ts refuses it:
/// exit 2 before anything is sent, never a message sent without the image.
#[test]
fn an_empty_image_flag_is_refused_before_anything_is_sent() {
    for argv in [
        vec!["chat", "Why is Save cut off?", "--image="],
        vec!["chat", "Why is Save cut off?", "--image", ""],
        vec!["chat", "--image="],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}: {}", refused.err);
        assert!(
            refused.err.contains("--image needs a file."),
            "{argv:?}: {}",
            refused.err
        );
        assert!(refused.seen.is_empty(), "{argv:?} sent {:?}", refused.seen);
    }
}

#[test]
fn no_web_turns_the_sessions_web_search_switch_off() {
    // Without --no-web, a new session leaves the switch to Gateway's default (on).
    let r = run(&["chat", "news today?", "--new", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (r.seen[0].method.as_str(), r.seen[0].path.as_str()),
        ("POST", "/v1/chat/sessions")
    );
    assert_eq!(r.seen[0].body.get("webSearch"), None);

    let r = run(&["chat", "news today?", "--no-web", "--new", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body["webSearch"], json!(false));
    assert_eq!(r.seen[1].body.get("webSearch"), None, "the message carries no switch");

    // chs_1 has web search off already: --no-web changes nothing.
    let r = run(&["chat", "thanks", "--no-web", "--session", "chs_1", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.seen.iter().all(|seen| seen.method != "PATCH"));
    // chs_on has it on: --no-web turns it off.
    let r = run(&["chat", "thanks", "--no-web", "--session", "chs_on", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (r.seen[1].method.as_str(), r.seen[1].path.as_str()),
        ("PATCH", "/v1/chat/sessions/chs_on")
    );
    assert_eq!(r.seen[1].body, json!({ "webSearch": false }));
}
