//! `nexus budgets managers` and the marks in `nexus budgets` against a stand-in Gateway (`budget.test.ts`): who may set
//! person limits, allowing and stopping an admin, and usage errors (exit 2) before anything is sent.

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

const OWNER: &str = "nx_owner";
const ADMIN: &str = "nx_admin";
const MEMBER: &str = "nx_member";
/// An owner on an older Gateway, which leaves out `limitManager` and `canGrant`.
const TYPESCRIPT: &str = "nx_typescript";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
}

fn members() -> Value {
    json!({
        "members": [
            { "userId": "usr_ada", "email": "ada@acme.test", "role": "owner" },
            { "userId": "usr_bo", "email": "bo@acme.test", "role": "admin" },
        ]
    })
}

fn person(user_id: &str, email: &str, role: &str, limit_manager: bool) -> Value {
    json!({
        "userId": user_id, "email": email, "role": role, "limitManager": limit_manager, "limitUsd": null,
        "source": "none", "spentUsd": 0, "reservedUsd": 0, "remainingUsd": null, "atLimit": false,
        "overspendsPrevented": 0,
    })
}

/// `GET /v1/budgets` as the key reads it: Ada owns, Bo and Cy are admins and only Bo is allowed.
fn budgets(key: &str) -> Value {
    let mut body = json!({
        "period": "2026-10", "periodStart": "2026-10-01T00:00:00.000Z", "resetsAt": "2026-11-01T00:00:00.000Z",
        "personDefault": { "monthlyUsd": null },
        "scope": if key == MEMBER { "own" } else { "org" },
        "people": [
            person("usr_ada", "ada@acme.test", "owner", false),
            person("usr_bo", "bo@acme.test", "admin", true),
            person("usr_cy", "cy@acme.test", "admin", false),
            person("usr_dee", "dee@acme.test", "member", false),
        ],
        "canManage": key != MEMBER,
        "canGrant": key == OWNER,
    });
    if key == TYPESCRIPT {
        body.as_object_mut().expect("object").remove("canGrant");
        for person in body["people"].as_array_mut().expect("people") {
            person.as_object_mut().expect("object").remove("limitManager");
        }
    }
    body
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn route(method: &str, path: &str, key: &str) -> Response {
    match (method, path) {
        ("GET", "/v1/budgets") => answer(200, budgets(key)),
        ("GET", "/v1/orgs/members") => answer(200, members()),
        ("GET", "/v1/spend") => answer(
            402,
            json!({ "error": { "message": "person limit: this call may cost up to $0.004096, and $0.00 of your $1.00 monthly limit is left (the organization's default per person)", "type": "budget_exceeded", "code": "budget_exceeded", "nexus": { "level": "person" } } }),
        ),
        ("PUT", "/v1/budgets/limit-managers/usr_bo") => {
            answer(200, json!({ "userId": "usr_bo", "limitManager": true }))
        }
        ("DELETE", "/v1/budgets/limit-managers/usr_bo") => {
            answer(200, json!({ "userId": "usr_bo", "limitManager": false }))
        }
        ("PUT", "/v1/budgets/limit-managers/usr_ada") => answer(
            409,
            json!({ "error": { "message": "only an admin can be allowed to set spend limits; owners always can", "type": "conflict", "code": "not_an_admin" } }),
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
                    let _: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                    });
                    let key = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|value| value.strip_prefix("Bearer "))
                        .filter(|key| [OWNER, ADMIN, MEMBER, TYPESCRIPT].contains(key))
                        .map(String::from);
                    let Some(key) = key else {
                        return answer(
                            401,
                            json!({ "error": { "message": "invalid API key", "type": "authentication_error" } }),
                        );
                    };
                    route(&method, &path, &key)
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

fn run_as(key: &str, argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-budgets-{}-{}",
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
        err: capture.0.err.borrow().join("\n"),
        seen,
    }
}

fn run(argv: &[&str]) -> Run {
    run_as(OWNER, argv)
}

fn writes(run: &Run) -> usize {
    run.seen
        .iter()
        .filter(|call| matches!(call.method.as_str(), "PUT" | "DELETE"))
        .count()
}

#[test]
fn lists_every_owner_then_each_admin_an_owner_allowed() {
    let owner = run(&["budgets", "managers"]);
    assert_eq!(owner.code, 0, "{}", owner.out);
    assert_eq!(
        owner.out,
        [
            "Who may set person spend limits",
            "",
            "  Person          Role",
            "· ada@acme.test   owner",
            "· bo@acme.test    admin",
            "",
            "Allow an admin: nexus budgets managers add <email|userId>. Stop one: nexus budgets managers remove <email|userId>.",
        ]
        .join("\n")
    );
    let admin = run_as(ADMIN, &["budgets", "managers", "list"]);
    assert_eq!(admin.code, 0);
    assert_eq!(admin.out.lines().count(), 5, "{}", admin.out);
    let json = run(&["budgets", "managers", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.out).expect("json"),
        json!({
            "managers": [
                { "userId": "usr_ada", "role": "owner", "email": "ada@acme.test" },
                { "userId": "usr_bo", "role": "admin", "email": "bo@acme.test" },
            ]
        })
    );
}

#[test]
fn reads_a_missing_limit_manager_and_can_grant_as_false() {
    let managers = run_as(TYPESCRIPT, &["budgets", "managers"]);
    assert_eq!(managers.code, 0, "{}", managers.out);
    assert_eq!(
        managers.out,
        [
            "Who may set person spend limits",
            "",
            "  Person          Role",
            "· ada@acme.test   owner",
        ]
        .join("\n")
    );
    let list = run_as(TYPESCRIPT, &["budgets"]);
    assert_eq!(list.code, 0, "{}", list.out);
    let roles: Vec<&str> = list
        .out
        .lines()
        .filter(|line| line.contains("@acme.test"))
        .filter_map(|line| {
            line.trim()
                .split("  ")
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .nth(1)
        })
        .collect();
    assert_eq!(roles, ["owner", "admin", "admin", "member"]);
}

#[test]
fn names_who_can_raise_a_person_limit() {
    let refused = run(&["spend"]);
    assert_eq!(refused.code, 1, "{}", refused.out);
    assert!(
        refused.err.contains(
            "Your monthly limit resets on the 1st (UTC); an owner, or an admin an owner allows, can raise it."
        ),
        "{}",
        refused.err
    );
}

#[test]
fn refuses_a_member_who_sees_only_their_own_row() {
    let member = run_as(MEMBER, &["budgets", "managers"]);
    assert_eq!(member.code, 1);
    assert!(
        member
            .err
            .contains("Only owners and admins can see who may set person spend limits."),
        "{}",
        member.err
    );
    assert!(member.err.contains("Ask an owner or admin of your organization."));
}

#[test]
fn marks_an_allowed_admin_in_the_list_of_limits() {
    let owner = run(&["budgets"]);
    assert_eq!(owner.code, 0, "{}", owner.out);
    let roles: Vec<&str> = owner
        .out
        .lines()
        .filter(|line| line.contains("@acme.test"))
        .filter_map(|line| {
            line.trim()
                .split("  ")
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .nth(1)
        })
        .collect();
    assert_eq!(roles, ["owner", "admin (sets limits)", "admin", "member"]);
}

#[test]
fn allows_and_stops_an_admin_by_email_or_by_id() {
    let allow = run(&["budgets", "managers", "add", "Bo@acme.test"]);
    assert_eq!(allow.code, 0, "{}", allow.out);
    let put = allow.seen.last().expect("seen");
    assert_eq!(
        (put.method.as_str(), put.path.as_str()),
        ("PUT", "/v1/budgets/limit-managers/usr_bo")
    );
    assert_eq!(allow.out, "✓ Bo@acme.test is now allowed to set person spend limits.");

    let stop = run(&["budgets", "managers", "remove", "usr_bo"]);
    assert_eq!(stop.code, 0, "{}", stop.out);
    assert_eq!(stop.seen.len(), 1);
    assert_eq!(
        (stop.seen[0].method.as_str(), stop.seen[0].path.as_str()),
        ("DELETE", "/v1/budgets/limit-managers/usr_bo")
    );
    assert_eq!(stop.out, "✓ usr_bo is no longer allowed to set person spend limits.");

    let json = run(&["budgets", "managers", "add", "usr_bo", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.out).expect("json"),
        json!({ "userId": "usr_bo", "limitManager": true })
    );
}

#[test]
fn refuses_before_it_writes_and_shows_the_gateways_refusal_as_it_is() {
    for (argv, code) in [
        (vec!["budgets", "managers", "add"], 2),
        (vec!["budgets", "managers", "remove", "   "], 2),
        (vec!["budgets", "managers", "grant", "usr_bo"], 2),
        (vec!["budgets", "managers", "add", "nobody@acme.test"], 1),
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, code, "{argv:?}");
        assert_eq!(writes(&refused), 0, "{argv:?}");
    }
    let not_an_admin = run(&["budgets", "managers", "add", "usr_ada"]);
    assert_eq!(not_an_admin.code, 1);
    assert!(
        not_an_admin
            .err
            .contains("only an admin can be allowed to set spend limits; owners always can"),
        "{}",
        not_an_admin.err
    );
}
