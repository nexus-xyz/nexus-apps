//! `nexus org domains` and `nexus org subscription` against a stand-in Gateway (`org.test.ts`): what each command
//! shows and sends, that a dry run writes nothing, and usage errors (exit 2) before anything is written.

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

const KEY: &str = "nx_domains";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

fn domains() -> Value {
    json!([{ "id": "orgdmn_1", "domain": "acme.test", "enrollmentMode": "automatic_invitation", "verified": true, "pendingInvitations": 2 }])
}

fn subscription() -> Value {
    json!({
        "plan": "team", "payerUserId": "usr_ada", "billingContactUserId": "usr_gone", "periodStart": "2026-11-01",
        "periodEnd": null, "status": "past_due", "statusEffectiveAt": "2026-10-20T00:00:00.000Z",
        "effectiveAt": "2026-10-20T00:00:00.000Z", "updatedAt": "2026-10-20T00:00:00.000Z", "seats": 4,
    })
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, body: &Value) -> Response {
    match (method, path) {
        ("GET", "/v1/orgs/domains") => answer(200, json!({ "domains": domains() })),
        ("POST", "/v1/orgs/domains") => answer(
            200,
            json!({
                "id": "orgdmn_2", "domain": body["domain"], "enrollmentMode": body["enrollmentMode"],
                "verified": true, "pendingInvitations": 0,
            }),
        ),
        ("DELETE", "/v1/orgs/domains/orgdmn_1") => answer(200, json!({ "removed": true })),
        ("GET", "/v1/orgs/subscription") => answer(200, subscription()),
        ("GET", "/v1/orgs/members") => answer(
            200,
            json!({ "members": [{ "userId": "usr_ada", "email": "ada@acme.test" }, { "userId": "usr_bob", "email": "bob@acme.test" }] }),
        ),
        ("PATCH", "/v1/orgs/subscription") => {
            let changed: Vec<&String> = body
                .as_object()
                .map(|fields| {
                    fields
                        .keys()
                        .filter(|key| *key != "effectiveAt" && *key != "dryRun")
                        .collect()
                })
                .unwrap_or_default();
            let mut after = subscription();
            after["effectiveAt"] = json!("2026-10-23T12:00:00.000Z");
            answer(
                200,
                json!({ "subscription": after, "changed": changed, "dryRun": body["dryRun"] == true }),
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
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
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
                        body: body.clone(),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    }
                    route(&method, &path, &body)
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
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-org-domains-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway();
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "org": "acme" }).to_string(),
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

fn writes(run: &Run) -> usize {
    run.seen
        .iter()
        .filter(|call| matches!(call.method.as_str(), "POST" | "DELETE" | "PATCH"))
        .count()
}

#[test]
fn lists_the_domains_and_how_people_on_them_join() {
    let listed = run(&["org", "domains"]);
    assert_eq!(listed.code, 0);
    assert!(listed.out.contains("acme.test"), "{}", listed.out);
    assert!(listed.out.contains("invited automatically"));
    assert!(listed.out.contains("orgdmn_1"));
    let json = run(&["org", "domains", "list", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.out).expect("json"),
        json!({ "domains": domains() })
    );
}

#[test]
fn adds_a_domain_and_a_dry_run_sends_nothing() {
    let dry = run(&["org", "domains", "add", "Acme.io", "--dry-run", "--json"]);
    assert_eq!(dry.code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&dry.out).expect("json"),
        json!({ "dryRun": true, "domain": "acme.io", "enrollmentMode": "automatic_invitation", "alreadyAdded": false })
    );
    assert_eq!(writes(&dry), 0);
    let added = run(&[
        "org",
        "domains",
        "add",
        "acme.io",
        "--enrollment",
        "automatic_suggestion",
    ]);
    assert_eq!(added.code, 0, "{}", added.out);
    let post = added.seen.iter().find(|call| call.method == "POST").expect("post");
    assert_eq!(post.path, "/v1/orgs/domains");
    assert_eq!(
        post.body,
        json!({ "domain": "acme.io", "enrollmentMode": "automatic_suggestion" })
    );
    assert!(added.out.contains("suggested; an admin accepts each"));
}

#[test]
fn removes_a_domain_by_name_and_refuses_before_writing() {
    let removed = run(&["org", "domains", "remove", "ACME.test", "--json"]);
    assert_eq!(removed.code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&removed.out).expect("json"),
        json!({ "removed": true, "domain": "acme.test", "id": "orgdmn_1" })
    );
    assert!(
        removed
            .seen
            .iter()
            .any(|call| call.method == "DELETE" && call.path == "/v1/orgs/domains/orgdmn_1")
    );
    for (argv, code) in [
        (vec!["org", "domains", "add"], 2),
        (vec!["org", "domains", "add", "   "], 2),
        (
            vec!["org", "domains", "add", "acme.io", "--enrollment", "enterprise_sso"],
            2,
        ),
        (vec!["org", "domains", "remove", "globex.test"], 1),
        (vec!["org", "domains", "remove", "acme.test", "--dry-run"], 0),
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, code, "{argv:?}");
        assert_eq!(writes(&refused), 0, "{argv:?}");
    }
}

#[test]
fn shows_the_subscription_with_its_people_by_email() {
    let shown = run(&["org", "subscription"]);
    assert_eq!(shown.code, 0, "{}", shown.out);
    for expected in [
        "team",
        "past due since 2026-10-20T00:00:00.000Z",
        "ada@acme.test",
        "usr_gone",
        "2026-11-01 to open",
    ] {
        assert!(shown.out.contains(expected), "{expected}: {}", shown.out);
    }
    let seats = shown.out.lines().any(|line| line.split_whitespace().eq(["Seats", "4"]));
    assert!(seats, "{}", shown.out);
    let json = run(&["org", "subscription", "show", "--json"]);
    assert_eq!(serde_json::from_str::<Value>(&json.out).expect("json"), subscription());
    assert_eq!(writes(&json), 0);
}

#[test]
fn sets_the_subscription_by_email_and_asks_gateway_for_a_dry_run() {
    let dry = run(&[
        "org",
        "subscription",
        "set",
        "--payer",
        "Ada@Acme.test",
        "--billing-contact",
        "none",
        "--dry-run",
    ]);
    assert_eq!(dry.code, 0, "{}", dry.out);
    let patch = dry.seen.iter().find(|call| call.method == "PATCH").expect("patch");
    assert_eq!(patch.path, "/v1/orgs/subscription");
    assert_eq!(
        patch.body,
        json!({ "payerUserId": "usr_ada", "billingContactUserId": null, "dryRun": true })
    );
    assert!(
        dry.out
            .contains("Dry run: would change the payer, the billing contact. Nothing changed."),
        "{}",
        dry.out
    );
    let changed = run(&[
        "org",
        "subscription",
        "set",
        "--billing-contact",
        "usr_bob",
        "--effective-at",
        "2026-10-23T00:00:00Z",
    ]);
    assert_eq!(changed.code, 0, "{}", changed.out);
    let patch = changed.seen.iter().find(|call| call.method == "PATCH").expect("patch");
    assert_eq!(
        patch.body,
        json!({ "billingContactUserId": "usr_bob", "effectiveAt": "2026-10-23T00:00:00Z" })
    );
    assert!(
        changed
            .out
            .contains("Changed the billing contact, effective 2026-10-23T12:00:00.000Z."),
        "{}",
        changed.out
    );
    for (argv, code) in [
        (vec!["org", "subscription", "set"], 2),
        (
            vec!["org", "subscription", "set", "--effective-at", "2026-10-23T00:00:00Z"],
            2,
        ),
        (vec!["org", "subscription", "set", "--payer", "nobody@acme.test"], 1),
        (vec!["org", "subscription", "cancel"], 2),
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, code, "{argv:?}");
        assert_eq!(writes(&refused), 0, "{argv:?}");
    }
}
