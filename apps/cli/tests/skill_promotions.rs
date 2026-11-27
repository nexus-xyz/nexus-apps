//! `nexus skills promote | promotions | approve | refuse | withdraw | library` and the library side of `export`, against a
//! stand-in Gateway (`skill-promotions.test.ts` holds the same cases): the requests each verb sends, the output of each,
//! the targets it takes, a refusal printed with its code and the fix for every code, the diff and the text of a request
//! printed so that nothing in another person's text reaches the terminal as a command, and paging.

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

const KEY: &str = "nx_promo";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    key: Option<String>,
}

/// What the stand-in Gateway is asked.
struct Ask<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
}

type Route = Arc<dyn Fn(&Ask) -> Response + Send + Sync>;

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn refusal(status: u16, code: &str, message: &str) -> Response {
    answer(
        status,
        json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } }),
    )
}

fn not_found() -> Response {
    answer(
        404,
        json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
    )
}

fn gateway(route: Route) -> (String, Arc<Mutex<Vec<Seen>>>) {
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
                let route = route.clone();
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
                    route(&Ask {
                        method: &method,
                        path: uri.path(),
                        query: uri.query().unwrap_or(""),
                    })
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
    dir: PathBuf,
}

fn new_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nexus-promotions-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn run(route: Route, argv: &[&str]) -> Run {
    run_in(new_dir(), route, argv)
}

fn run_in(dir: PathBuf, route: Route, argv: &[&str]) -> Run {
    let (url, seen) = gateway(route);
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
        env: HashMap::new(),
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
        seen,
        dir,
    }
}

fn requests(r: &Run) -> Vec<String> {
    r.seen
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect()
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// The words of a table row, without the mark that starts it.
fn cells(line: &str) -> Vec<&str> {
    let mut words = words(line);
    if words.first() == Some(&"·") {
        words.remove(0);
    }
    words
}

// ---------------------------------------------------------------- fixtures

fn skill(id: &str, name: &str, latest: i64) -> Value {
    json!({ "id": id, "name": name, "description": "d", "latestVersion": latest, "createdAt": "2026-10-17T12:00:00.000Z", "updatedAt": "2026-10-18T13:00:00.000Z" })
}

fn promotion(id: &str, state: &str) -> Value {
    json!({
        "id": id, "state": state, "scope": "org", "name": "changelog",
        "description": "Write release notes\n  from merged pull requests.",
        "contentSha256": "a".repeat(64), "bytes": 120,
        "sourceSkillId": "skl_1", "sourceVersion": 2, "sourceWrittenVia": "cli", "sourceWrittenAt": "2026-10-18T11:00:00.000Z",
        "requestedBy": "usr_ada", "requestedVia": "cli", "requestedAt": "2026-10-18T12:00:00.000Z",
        "selfApproved": false
    })
}

fn team_promotion(id: &str, state: &str) -> Value {
    let mut value = promotion(id, state);
    value["scope"] = json!("team");
    value["teamId"] = json!("team_web");
    value
}

const TEXT: &str =
    "---\nname: changelog\ndescription: Write release notes.\n---\n\n# Changelog\n\nGroup the changes by area.\n";

fn library_entry(id: &str, name: &str, scope: &str, team: Option<&str>, version: i64, marked: bool) -> Value {
    let mut entry = json!({ "id": id, "name": name, "description": format!("The {name} skill.\nIn two lines."), "scope": scope, "version": version, "approvedAt": "2026-10-19T09:00:00.000Z" });
    if let Some(team) = team {
        entry["teamId"] = json!(team);
    }
    if marked {
        entry["duplicateName"] = json!(true);
    }
    entry
}

fn teams() -> Value {
    json!({ "teams": [
        { "id": "team_web", "slug": "web", "name": "Web" },
        { "id": "team_ops", "slug": "ops", "name": "Ops" }
    ], "default": null })
}

/// A route table from a closure: `GET /v1/skills` lists the person's skills unless the closure answers it.
fn route(handler: impl Fn(&Ask) -> Option<Response> + Send + Sync + 'static) -> Route {
    Arc::new(move |ask: &Ask| {
        if let Some(response) = handler(ask) {
            return response;
        }
        match (ask.method, ask.path) {
            ("GET", "/v1/skills") => answer(200, json!({ "skills": [skill("skl_1", "changelog", 2)] })),
            ("GET", "/v1/teams") => answer(200, teams()),
            _ => not_found(),
        }
    })
}

fn none() -> Route {
    route(|_| None)
}

// ---------------------------------------------------------------- promote

#[test]
fn promote_asks_for_the_organization_and_says_who_is_asked() {
    let r = run(
        route(|ask| {
            (ask.method == "POST" && ask.path == "/v1/skills/skl_1/versions/latest/promotions")
                .then(|| answer(201, promotion("spr_1", "held")))
        }),
        &[
            "skills",
            "promote",
            "Changelog",
            "--to",
            "org",
            "--note",
            "  For the whole team  ",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        ["GET /v1/skills", "POST /v1/skills/skl_1/versions/latest/promotions"]
    );
    assert_eq!(r.seen[1].body, json!({ "scope": "org", "note": "For the whole team" }));
    assert!(
        r.seen[1].key.as_deref().is_some_and(|key| !key.is_empty()),
        "an Idempotency-Key"
    );
    assert_eq!(
        r.out,
        [
            "✓ Requested promotion of changelog, version 2, to the organization.",
            "  Request  spr_1",
            "  State    held",
            "  Asked    the organization's other owners and admins",
            "  Next     nexus skills promotions spr_1",
        ]
        .join("\n")
    );
}

#[test]
fn promote_asks_for_a_team_at_a_version_by_id() {
    let r = run(
        route(|ask| {
            (ask.path == "/v1/skills/skl_1/versions/2/promotions").then(|| answer(201, team_promotion("spr_2", "held")))
        }),
        &["skills", "promote", "skl_1", "2", "--to=team/web"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[1].body, json!({ "scope": "team", "team": "web" }));
    assert_eq!(
        r.out,
        [
            "✓ Requested promotion of changelog, version 2, to team web.",
            "  Request  spr_2",
            "  State    held",
            "  Asked    the organization's other owners and admins, and the other leads of team web and of the teams above it",
            "  Next     nexus skills promotions spr_2",
        ]
        .join("\n")
    );
    // The version `latest` is spelt out the same way as leaving it out.
    let r = run(
        route(|ask| ask.method.eq("POST").then(|| answer(201, promotion("spr_3", "held")))),
        &["skills", "promote", "changelog", "latest", "--to", "org"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[1].path, "/v1/skills/skl_1/versions/latest/promotions");
}

#[test]
fn a_target_is_read_in_any_case_and_the_slug_stays_as_typed() {
    let post = || route(|ask| ask.method.eq("POST").then(|| answer(201, promotion("spr_1", "held"))));
    for (to, body) in [
        ("ORG", json!({ "scope": "org" })),
        ("Org", json!({ "scope": "org" })),
        ("Team/Web", json!({ "scope": "team", "team": "Web" })),
        ("TEAM/web", json!({ "scope": "team", "team": "web" })),
    ] {
        let r = run(post(), &["skills", "promote", "changelog", "--to", to]);
        assert_eq!(r.code, 0, "{to}: {}", r.err);
        assert_eq!(r.seen[1].body, body, "{to}");
    }
    let r = run(
        list_route(json!({ "promotions": [] })),
        &["skills", "promotions", "--to", "TEAM/web"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions?scope=team&team=web"]);
    let r = run(library_route(vec![]), &["skills", "library", "--to", "ORG"]);
    assert_eq!(r.code, 0, "{}", r.err);
}

#[test]
fn promote_with_json_prints_the_request_unchanged() {
    let r = run(
        route(|ask| ask.method.eq("POST").then(|| answer(201, promotion("spr_1", "held")))),
        &["skills", "promote", "changelog", "--to", "org", "--json"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).expect("json"),
        promotion("spr_1", "held")
    );
}

#[test]
fn promote_reads_its_arguments_before_it_sends_anything() {
    let long_note = "x".repeat(501);
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["skills", "promote"],
            "Usage: nexus skills promote <name|id> [<version>] --to org|team/<slug> [--note TEXT]",
        ),
        (vec!["skills", "promote", "changelog"], "Usage: nexus skills promote"),
        (
            vec!["skills", "promote", "changelog", "--to"],
            "--to needs a target: org or team/<slug>.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "everyone"],
            "\"everyone\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "team/"],
            "\"team/\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "team/a/b"],
            "\"team/a/b\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "person/me"],
            "\"person/me\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "org/"],
            "\"org/\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "orgs"],
            "\"orgs\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "TEAM/"],
            "\"TEAM/\" is not a target.",
        ),
        (
            vec!["skills", "promote", "changelog", "0", "--to", "org"],
            "The version must be a version number (1 or more), or latest.",
        ),
        (
            vec!["skills", "promote", "changelog", "two", "--to", "org"],
            "The version must be a version number",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "org", "--note"],
            "--note needs text.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "org", "--note", "  "],
            "--note needs text.",
        ),
        (
            vec!["skills", "promote", "changelog", "--to", "org", "--note", &long_note],
            "--note is at most 500 characters; this one is 501.",
        ),
    ];
    for (argv, message) in cases {
        let r = run(none(), &argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.err.contains(message), "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?} sent {:?}", requests(&r));
    }
    // 500 characters of two bytes each are 500 characters.
    let fits = "é".repeat(500);
    let r = run(
        route(|ask| ask.method.eq("POST").then(|| answer(201, promotion("spr_1", "held")))),
        &["skills", "promote", "changelog", "--to", "org", "--note", &fits],
    );
    assert_eq!(r.code, 0, "{}", r.err);
}

#[test]
fn promote_names_a_skill_you_do_not_have_and_prints_what_gateway_refuses() {
    let r = run(none(), &["skills", "promote", "nope", "--to", "org"]);
    assert_eq!(r.code, 1);
    assert_eq!(requests(&r), ["GET /v1/skills"]);
    assert!(
        r.err.contains("You have no skill named nope (skill_not_found)"),
        "{}",
        r.err
    );
    assert!(r.err.contains("`nexus skills` lists your skills."), "{}", r.err);
}

// ---------------------------------------------------------------- promotions: the list

fn list_route(page: Value) -> Route {
    route(move |ask| (ask.method == "GET" && ask.path == "/v1/skill-promotions").then(|| answer(200, page.clone())))
}

#[test]
fn promotions_lists_the_requests_you_can_see() {
    let mut approved = promotion("spr_2", "approved");
    approved["libraryVersion"] = json!(3);
    let r = run(
        list_route(json!({ "promotions": [promotion("spr_1", "held"), approved, team_promotion("spr_3", "refused")] })),
        &["skills", "promotions"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions", "GET /v1/teams"]);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(words(lines[0]), ["Request", "Skill", "Version", "To", "State", "Asked"]);
    assert_eq!(
        cells(lines[1]),
        [
            "spr_1",
            "changelog",
            "v2",
            "organization",
            "held",
            "2026-10-18",
            "12:00:00"
        ]
    );
    assert_eq!(
        cells(lines[2]),
        [
            "spr_2",
            "changelog",
            "v2",
            "organization",
            "approved,",
            "library",
            "v3",
            "2026-10-18",
            "12:00:00"
        ]
    );
    assert_eq!(
        cells(lines[3]),
        [
            "spr_3",
            "changelog",
            "v2",
            "team",
            "web",
            "refused",
            "2026-10-18",
            "12:00:00"
        ]
    );
    assert_eq!(lines.len(), 4);
}

#[test]
fn promotions_sends_each_filter_and_a_cursor() {
    let r = run(
        list_route(json!({ "promotions": [promotion("spr_1", "held")], "nextCursor": "c2" })),
        &[
            "skills",
            "promotions",
            "--state",
            "held",
            "--to",
            "team/web",
            "--mine",
            "--limit",
            "5",
            "--cursor",
            "c1",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        ["GET /v1/skill-promotions?state=held&scope=team&team=web&mine=true&limit=5&cursor=c1"]
    );
    assert!(
        r.out.ends_with("\n\nMore: add --cursor c2 to the same command."),
        "{}",
        r.out
    );
    let r = run(
        list_route(json!({ "promotions": [] })),
        &["skills", "promotions", "--to", "org"],
    );
    assert_eq!(requests(&r), ["GET /v1/skill-promotions?scope=org"]);
    assert_eq!(r.out, "No promotion requests match.");
    let r = run(list_route(json!({ "promotions": [] })), &["skills", "promotions"]);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions"]);
    assert_eq!(
        r.out,
        "No promotion requests yet. Ask for one: nexus skills promote <name> --to org"
    );
}

#[test]
fn promotions_pages_with_the_cursor_it_printed() {
    let pages = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_queries = pages.clone();
    let route = route(move |ask| {
        if ask.path != "/v1/skill-promotions" {
            return None;
        }
        seen_queries.lock().expect("queries").push(ask.query.to_string());
        Some(if ask.query.contains("cursor=c1") {
            answer(200, json!({ "promotions": [promotion("spr_2", "held")] }))
        } else {
            answer(
                200,
                json!({ "promotions": [promotion("spr_1", "held")], "nextCursor": "c1" }),
            )
        })
    });
    let first = run(route.clone(), &["skills", "promotions", "--limit", "1"]);
    assert!(
        first.out.contains("spr_1") && first.out.contains("--cursor c1"),
        "{}",
        first.out
    );
    let second = run(route, &["skills", "promotions", "--limit", "1", "--cursor", "c1"]);
    assert!(
        second.out.contains("spr_2") && !second.out.contains("More:"),
        "{}",
        second.out
    );
    assert_eq!(*pages.lock().expect("queries"), ["limit=1", "limit=1&cursor=c1"]);
}

#[test]
fn promotions_checks_its_filters_before_it_sends_anything() {
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["skills", "promotions", "--state", "pending"],
            "\"pending\" is not a state.",
        ),
        (
            vec!["skills", "promotions", "--state"],
            "--state needs a state: held, approved, refused or withdrawn.",
        ),
        (vec!["skills", "promotions", "--to", "web"], "\"web\" is not a target."),
        (
            vec!["skills", "promotions", "--to"],
            "--to needs a target: org or team/<slug>.",
        ),
        (
            vec!["skills", "promotions", "--limit", "0"],
            "--limit must be a whole number from 1 to 200.",
        ),
        (
            vec!["skills", "promotions", "--limit", "201"],
            "--limit must be a whole number from 1 to 200.",
        ),
        (
            vec!["skills", "promotions", "--limit", "1.5"],
            "--limit must be a whole number from 1 to 200.",
        ),
        (
            vec!["skills", "promotions", "--limit", "many"],
            "--limit must be a whole number from 1 to 200.",
        ),
        (
            vec!["skills", "promotions", "--cursor"],
            "--cursor needs the cursor the last page printed.",
        ),
    ];
    for (argv, message) in cases {
        let r = run(none(), &argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.err.contains(message), "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn promotions_with_json_prints_the_page_unchanged() {
    let page = json!({ "promotions": [promotion("spr_1", "held")], "nextCursor": "c1" });
    let r = run(list_route(page.clone()), &["skills", "promotions", "--json"]);
    assert_eq!(serde_json::from_str::<Value>(&r.out).expect("json"), page);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions"]);
}

// ---------------------------------------------------------------- promotions <id>

fn detail(promotion: Value, content: &str, previous: Value) -> Value {
    let mut value = promotion;
    value["content"] = json!(content);
    value["previous"] = previous;
    value
}

fn previous(content: &str) -> Value {
    json!({ "id": "spr_0", "name": "changelog", "description": "d", "contentSha256": "b".repeat(64), "libraryVersion": 1, "decidedAt": "2026-10-10T08:00:00.000Z", "content": content })
}

fn detail_route(value: Value) -> Route {
    route(move |ask| (ask.path == "/v1/skill-promotions/spr_1").then(|| answer(200, value.clone())))
}

#[test]
fn a_request_shows_provenance_a_diff_from_the_last_approved_version_and_the_text() {
    let old =
        "---\nname: changelog\ndescription: Write release notes.\n---\n\n# Changelog\n\nGroup the changes by date.\n";
    let mut held = promotion("spr_1", "held");
    held["note"] = json!("Used by the whole team.");
    let r = run(
        detail_route(detail(held, TEXT, previous(old))),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions/spr_1"]);
    assert_eq!(
        r.out,
        [
            "changelog · version 2 · held",
            "spr_1",
            "",
            "  To           the organization",
            "  Description  Write release notes from merged pull requests.",
            "  Asked by     usr_ada via cli, 2026-10-18T12:00:00.000Z",
            "  Written      skl_1 version 2, via cli, 2026-10-18T11:00:00.000Z",
            "  Note         Used by the whole team.",
            &format!("  SHA-256      {}", "a".repeat(64)),
            "  Size         120 bytes",
            "",
            "Changes since the last approved version",
            "Library version 1 of changelog, approved 2026-10-10T08:00:00.000Z.",
            "--- library version 1",
            "+++ promotion request spr_1",
            "@@ -5,4 +5,4 @@",
            " ",
            " # Changelog",
            " ",
            "-Group the changes by date.",
            "+Group the changes by area.",
            "1 added, 1 removed.",
            "",
            "Text (SKILL.md, 120 bytes)",
            "  | ---",
            "  | name: changelog",
            "  | description: Write release notes.",
            "  | ---",
            "  |",
            "  | # Changelog",
            "  |",
            "  | Group the changes by area.",
        ]
        .join("\n")
    );
}

#[test]
fn a_request_with_no_earlier_version_says_so_and_one_with_the_same_text_says_that() {
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), TEXT, Value::Null)),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out.contains(
            "Changes since the last approved version\nNone to show. There is no earlier approved version of this skill, or you do not load its library.\n\nText (SKILL.md, 120 bytes)"
        ),
        "{}",
        r.out
    );
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), TEXT, previous(TEXT))),
        &["skills", "promotions", "spr_1"],
    );
    assert!(
        r.out.contains("No changes: the text is the same as library version 1."),
        "{}",
        r.out
    );
}

#[test]
fn a_request_that_is_decided_or_withdrawn_shows_who_and_why() {
    let mut value = team_promotion("spr_1", "withdrawn");
    value["decidedBy"] = json!("usr_bob");
    value["decidedAt"] = json!("2026-10-19T09:00:00.000Z");
    value["decisionReason"] = json!("Good\nwork");
    value["librarySkillId"] = json!("skl_9");
    value["libraryVersion"] = json!(3);
    value["withdrawnBy"] = json!("usr_cy");
    value["withdrawnAt"] = json!("2026-10-20T09:00:00.000Z");
    value["withdrawnReason"] = json!("Out of date");
    let r = run(
        detail_route(detail(value, TEXT, Value::Null)),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skill-promotions/spr_1", "GET /v1/teams"]);
    // The labels are as wide as the longest, `Withdrawal reason`.
    for (label, value) in [
        ("To", "team web"),
        ("Decided", "usr_bob, 2026-10-19T09:00:00.000Z"),
        ("Decision reason", "Good work"),
        ("Library", "skl_9, version 3"),
        ("Withdrawn", "usr_cy, 2026-10-20T09:00:00.000Z"),
        ("Withdrawal reason", "Out of date"),
    ] {
        let line = format!("  {label:<17}  {value}");
        assert!(r.out.lines().any(|shown| shown == line), "{line:?}\n{}", r.out);
    }
}

#[test]
fn a_team_the_list_does_not_name_is_shown_by_its_id() {
    let r = run(
        route(|ask| match ask.path {
            "/v1/skill-promotions/spr_1" => {
                Some(answer(200, detail(team_promotion("spr_1", "held"), TEXT, Value::Null)))
            }
            "/v1/teams" => Some(refusal(500, "internal_error", "boom")),
            _ => None,
        }),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("  To           team team_web"), "{}", r.out);
}

#[test]
fn text_that_another_person_wrote_never_reaches_the_terminal_as_a_command() {
    let hostile = "---\nname: changelog\ndescription: A\u{1b}[2J\u{1b}]0;owned\u{7}B\u{202e}C\u{200b}D\u{9b}31mE\n---\n\nbackspace\u{8}x\rcarriage\u{7f}del\n\u{feff}bom\u{e0041}tag\n";
    let mut held = promotion("spr_1", "held");
    held["description"] = json!("Looks fine\u{1b}[31m red \u{202e}gnp.exe");
    held["note"] = json!("Please\u{1b}[2Japprove\u{9d}0;t\u{9c}");
    let r = run(
        detail_route(detail(held, hostile, previous("---\nname: changelog\n---\n"))),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    for bad in [
        '\u{1b}',
        '\u{7}',
        '\u{8}',
        '\r',
        '\u{7f}',
        '\u{9b}',
        '\u{9d}',
        '\u{9c}',
        '\u{202e}',
        '\u{200b}',
        '\u{feff}',
        '\u{e0041}',
    ] {
        assert!(!r.out.contains(bad), "{bad:?} reached the output:\n{}", r.out);
    }
    for shown in [
        "A[U+001B][2J[U+001B]]0;owned[U+0007]B[U+202E]C[U+200B]D[U+009B]31mE",
        "backspace[U+0008]x[U+000D]carriage[U+007F]del",
        "[U+FEFF]bom[U+E0041]tag",
        "Looks fine[U+001B][31m red [U+202E]gnp.exe",
        "Please[U+001B][2Japprove[U+009D]0;t[U+009C]",
    ] {
        assert!(r.out.contains(shown), "{shown}\n{}", r.out);
    }
}

#[test]
fn json_output_escapes_what_a_terminal_could_run_and_means_the_same() {
    let value = detail(
        promotion("spr_1", "held"),
        "a\u{9b}31m\u{202e}b\u{7f}c\u{1b}[2J",
        Value::Null,
    );
    let r = run(
        detail_route(value.clone()),
        &["skills", "promotions", "spr_1", "--json"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    for bad in ['\u{9b}', '\u{202e}', '\u{7f}', '\u{1b}'] {
        assert!(!r.out.contains(bad), "{bad:?}");
    }
    assert!(r.out.contains("a\\u009b31m\\u202eb\\u007fc\\u001b[2J"), "{}", r.out);
    assert_eq!(serde_json::from_str::<Value>(&r.out).expect("json"), value);
}

#[test]
fn a_very_long_line_of_text_is_printed_whole() {
    let long = format!("{}\u{1b}[2J{}", "x".repeat(300_000), "y".repeat(300_000));
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), &format!("{long}\n"), Value::Null)),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let shown = r
        .out
        .lines()
        .find(|line| line.starts_with("  | xxx"))
        .expect("the long line");
    assert_eq!(shown.len(), "  | ".len() + 600_000 + "[U+001B]".len() + "[2J".len());
    assert!(shown.contains("[U+001B][2J"));
}

#[test]
fn a_crlf_line_end_is_not_a_code_and_a_carriage_return_inside_a_line_still_is() {
    let windows = "---\r\nname: changelog\r\ndescription: Write release notes.\r\n---\r\n\r\n# Changelog\r\n";
    let mut held = promotion("spr_1", "held");
    held["description"] = json!("Write release notes\r\n  from merged pull requests.");
    let r = run(
        detail_route(detail(held, windows, Value::Null)),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(!r.out.contains("[U+000D]") && !r.out.contains('\r'), "{}", r.out);
    assert!(
        r.out
            .contains("  Description  Write release notes from merged pull requests.\n"),
        "{}",
        r.out
    );
    assert!(
        r.out.ends_with(
            "Text (SKILL.md, 120 bytes)\nLine ends: CRLF. A carriage return that ends a line is not shown.\n  | ---\n  | name: changelog\n  | description: Write release notes.\n  | ---\n  |\n  | # Changelog"
        ),
        "{}",
        r.out
    );
    // Mixed line ends. A carriage return inside a line, and one that ends the text, stay shown.
    let r = run(
        detail_route(detail(
            promotion("spr_1", "held"),
            "a\r\nb\nx\ry\r\nlast\r",
            Value::Null,
        )),
        &["skills", "promotions", "spr_1"],
    );
    assert!(
        r.out.ends_with(
            "Line ends: mixed. A carriage return that ends a line is not shown.\n  | a\n  | b\n  | x[U+000D]y\n  | last[U+000D]"
        ),
        "{}",
        r.out
    );
}

#[test]
fn the_diff_hides_a_crlf_and_says_when_the_line_ends_changed() {
    // The same lines with CRLF for LF: every line differs, and the note says why.
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), "a\r\nb\r\n", previous("a\nb\n"))),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out
            .contains("@@ -1,2 +1,2 @@\n-a\n-b\n+a\n+b\n2 added, 2 removed.\nLine ends changed from LF to CRLF.\n"),
        "{}",
        r.out
    );
    assert!(!r.out.contains("[U+000D]"), "{}", r.out);
    // A last line with no line end keeps a carriage return as a code, before the marker line.
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), "a\r", previous("a\n"))),
        &["skills", "promotions", "spr_1"],
    );
    assert!(
        r.out.contains("-a\n+a[U+000D]\n\\ No newline at end of file\n"),
        "{}",
        r.out
    );
    // No note when both texts end their lines the same way.
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), "a\r\nc\r\n", previous("a\r\nb\r\n"))),
        &["skills", "promotions", "spr_1"],
    );
    assert!(r.out.contains("-b\n+c\n1 added, 1 removed.\n\nText"), "{}", r.out);
}

#[test]
fn a_long_file_with_two_distant_edits_shows_two_edits() {
    let numbered = |name: &str| -> String { (0..3000).map(|n| format!("{name} {n}\n")).collect() };
    let old = numbered("line");
    let new = old
        .replacen("line 1\n", "line one\n", 1)
        .replacen("line 2998\n", "line 2998 changed\n", 1);
    let r = run(
        detail_route(detail(promotion("spr_1", "held"), &new, previous(&old))),
        &["skills", "promotions", "spr_1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("\n2 added, 2 removed.\n"), "{}", r.out);
    assert!(!r.out.contains("too large to line up"), "{}", r.out);
    // Texts with nothing in common cannot be lined up, and the note says so.
    let r = run(
        detail_route(detail(
            promotion("spr_1", "held"),
            &numbered("new"),
            previous(&numbered("old")),
        )),
        &["skills", "promotions", "spr_1"],
    );
    assert!(
        r.out.contains(
            "\n3000 added, 3000 removed.\nPart of the change was too large to line up: those lines show as removed, then as added.\n"
        ),
        "{}",
        r.out
    );
}

#[test]
fn a_refusal_with_no_skill_code_cannot_reach_the_terminal_as_a_command() {
    for (status, code) in [
        (500, Some("internal_error")),
        (502, None),
        (403, Some("person_required")),
    ] {
        let r = run(
            route(move |ask| {
                ask.path.starts_with("/v1/skill-").then(|| match code {
                    Some(code) => refusal(status, code, "no \u{1b}[2J\u{202e}skill\u{9b}"),
                    None => answer(
                        status,
                        json!({ "error": { "message": "no \u{1b}[2J\u{202e}skill\u{9b}", "type": "server_error" } }),
                    ),
                })
            }),
            &["skills", "promotions"],
        );
        assert_eq!(r.code, 1, "{status}: {}", r.err);
        assert!(
            r.err.contains("no [U+001B][2J[U+202E]skill[U+009B]"),
            "{status}: {}",
            r.err
        );
        assert!(!r.err.contains(['\u{1b}', '\u{202e}', '\u{9b}']), "{status}: {}", r.err);
    }
}

#[test]
fn a_request_nobody_can_see_is_promotion_not_found() {
    let r = run(
        route(|ask| {
            ask.path
                .starts_with("/v1/skill-promotions/")
                .then(|| refusal(404, "promotion_not_found", "no skill promotion spr_9"))
        }),
        &["skills", "promotions", "spr_9"],
    );
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("No skill promotion spr_9 (promotion_not_found)"),
        "{}",
        r.err
    );
    assert!(
        r.err
            .contains("`nexus skills promotions` lists the requests you can see."),
        "{}",
        r.err
    );
}

// ---------------------------------------------------------------- approve, refuse, withdraw

fn decision_route(response: Value) -> Route {
    route(move |ask| {
        (ask.method == "POST" && ask.path == "/v1/skill-promotions/spr_1/decision")
            .then(|| answer(200, response.clone()))
    })
}

#[test]
fn approve_sends_the_decision_and_says_what_joined_the_library() {
    let mut approved = promotion("spr_1", "approved");
    approved["libraryVersion"] = json!(3);
    let r = run(decision_route(approved.clone()), &["skills", "approve", "spr_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["POST /v1/skill-promotions/spr_1/decision"]);
    assert_eq!(r.seen[0].body, json!({ "decision": "approved" }));
    assert!(r.seen[0].key.as_deref().is_some_and(|key| !key.is_empty()));
    assert_eq!(
        r.out,
        [
            "✓ Approved changelog, version 2.",
            "  Request  spr_1",
            "  Library  version 3 of changelog, for the organization",
        ]
        .join("\n")
    );
    let mut team = team_promotion("spr_1", "approved");
    team["libraryVersion"] = json!(1);
    let r = run(
        decision_route(team),
        &["skills", "approve", "spr_1", "--reason", "Looks good"],
    );
    assert_eq!(
        r.seen[0].body,
        json!({ "decision": "approved", "reason": "Looks good" })
    );
    assert_eq!(
        requests(&r),
        ["POST /v1/skill-promotions/spr_1/decision", "GET /v1/teams"]
    );
    assert_eq!(
        r.out,
        [
            "✓ Approved changelog, version 2.",
            "  Request  spr_1",
            "  Library  version 1 of changelog, for team web",
            "  Reason   Looks good",
        ]
        .join("\n")
    );
}

#[test]
fn refuse_needs_a_reason_and_says_so_before_it_sends_anything() {
    for argv in [
        vec!["skills", "refuse", "spr_1"],
        vec!["skills", "refuse", "spr_1", "--reason"],
        vec!["skills", "refuse", "spr_1", "--reason", "   "],
    ] {
        let r = run(none(), &argv);
        assert_eq!(r.code, 2, "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}");
    }
    let r = run(none(), &["skills", "refuse", "spr_1"]);
    assert!(
        r.err
            .contains("A refusal needs a reason, so the author knows what to change."),
        "{}",
        r.err
    );
    assert!(r.err.contains("nexus skills refuse <id> --reason TEXT"), "{}", r.err);
    let r = run(none(), &["skills", "refuse"]);
    assert_eq!(r.code, 2);
    assert!(
        r.err.contains("Usage: nexus skills refuse <id> --reason TEXT"),
        "{}",
        r.err
    );
}

#[test]
fn refuse_sends_the_reason() {
    let r = run(
        decision_route(promotion("spr_1", "refused")),
        &["skills", "refuse", "spr_1", "--reason", "Say when to use it"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen[0].body,
        json!({ "decision": "refused", "reason": "Say when to use it" })
    );
    assert_eq!(
        r.out,
        [
            "✓ Refused changelog, version 2.",
            "  Request  spr_1",
            "  Reason   Say when to use it"
        ]
        .join("\n")
    );
}

fn withdraw_route(response: Value) -> Route {
    route(move |ask| {
        (ask.method == "POST" && ask.path == "/v1/skill-promotions/spr_1/withdraw")
            .then(|| answer(200, response.clone()))
    })
}

#[test]
fn withdraw_a_held_request_an_approved_version_and_all_versions() {
    let mut held = promotion("spr_1", "withdrawn");
    held["withdrawnIds"] = json!(["spr_1"]);
    let r = run(withdraw_route(held), &["skills", "withdraw", "spr_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body, json!({}));
    assert_eq!(
        r.out,
        [
            "✓ Withdrew the request to promote changelog, version 2.",
            "  Request  spr_1"
        ]
        .join("\n")
    );

    let mut approved = promotion("spr_1", "withdrawn");
    approved["libraryVersion"] = json!(3);
    approved["withdrawnIds"] = json!(["spr_1"]);
    let r = run(
        withdraw_route(approved),
        &["skills", "withdraw", "spr_1", "--reason", "Out of date"],
    );
    assert_eq!(r.seen[0].body, json!({ "reason": "Out of date" }));
    assert_eq!(
        r.out,
        [
            "✓ Withdrew changelog, library version 3.",
            "  Request  spr_1",
            "  Reason   Out of date",
            "  Effect   New sessions and chat turns load the highest approved version that is left, if any. Add --all-versions to stop the whole skill. A session that already loaded it keeps the text.",
        ]
        .join("\n")
    );

    // One approved version withdrawn with --all-versions: the skill stops, whatever the number of versions.
    let mut only = promotion("spr_1", "withdrawn");
    only["libraryVersion"] = json!(1);
    only["withdrawnIds"] = json!(["spr_1"]);
    let r = run(withdraw_route(only), &["skills", "withdraw", "spr_1", "--all-versions"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out.ends_with(
            "  Effect   New sessions and chat turns stop loading it. A session that already loaded it keeps the text."
        ),
        "{}",
        r.out
    );

    let mut many = promotion("spr_1", "withdrawn");
    many["libraryVersion"] = json!(3);
    many["withdrawnIds"] = json!(["spr_0", "spr_1"]);
    // The switch can come before the id: it takes no value.
    let r = run(withdraw_route(many), &["skills", "withdraw", "--all-versions", "spr_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].body, json!({ "allVersions": true }));
    assert!(
        r.out
            .starts_with("✓ Withdrew 2 versions of changelog.\n  Requests  spr_0, spr_1\n"),
        "{}",
        r.out
    );
    assert!(
        r.out.contains("Effect    New sessions and chat turns stop loading it."),
        "{}",
        r.out
    );
}

#[test]
fn withdraw_needs_an_id_and_text_for_a_reason() {
    let r = run(none(), &["skills", "withdraw"]);
    assert_eq!(r.code, 2);
    assert!(
        r.err
            .contains("Usage: nexus skills withdraw <id> [--reason TEXT] [--all-versions]"),
        "{}",
        r.err
    );
    let r = run(none(), &["skills", "withdraw", "spr_1", "--reason"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--reason needs text."), "{}", r.err);
    assert!(r.seen.is_empty());
    let r = run(none(), &["skills", "approve"]);
    assert_eq!(r.code, 2);
    assert!(
        r.err.contains("Usage: nexus skills approve <id> [--reason TEXT]"),
        "{}",
        r.err
    );
}

#[test]
fn a_saved_api_key_is_told_to_sign_in() {
    let r = run(
        route(|ask| {
            ask.path.ends_with("/decision").then(|| {
                refusal(
                    403,
                    "person_session_required",
                    "deciding on a skill promotion needs your Nexus UI session or a CLI sign-in, not an API key: run `nexus auth login`",
                )
            })
        }),
        &["skills", "approve", "spr_1"],
    );
    assert_eq!(r.code, 1);
    assert_eq!(
        r.err,
        [
            "✕ Error: Deciding on a skill promotion needs your Nexus UI session or a CLI sign-in, not an API key: run `nexus auth login` (person_session_required)",
            "  → Run `nexus auth login` to sign in as yourself. A saved API key cannot decide or withdraw an approved version.",
        ]
        .join("\n")
    );
}

/// Each code Gateway gives for a promotion request or the library, with the verb that can meet it: the line the person
/// reads carries Gateway's sentence, the code, and the fix.
#[test]
fn every_typed_error_prints_the_sentence_the_code_and_the_fix() {
    let cases: [(&str, u16, &[&str], &str); 18] = [
        (
            "self_approval",
            403,
            &["skills", "approve", "spr_1"],
            "Ask another owner or admin to decide it.",
        ),
        (
            "person_session_required",
            403,
            &["skills", "approve", "spr_1"],
            "A saved API key cannot decide or withdraw an approved version.",
        ),
        (
            "promotion_not_found",
            404,
            &["skills", "approve", "spr_1"],
            "`nexus skills promotions` lists the requests you can see.",
        ),
        (
            "promotion_decided",
            409,
            &["skills", "approve", "spr_1"],
            "`nexus skills promotions <id>` shows its state.",
        ),
        (
            "requester_left",
            409,
            &["skills", "approve", "spr_1"],
            "Withdraw it with `nexus skills withdraw <id>`.",
        ),
        ("name_taken", 409, &["skills", "approve", "spr_1"], "rename the skill."),
        (
            "library_limit",
            409,
            &["skills", "approve", "spr_1"],
            "Withdraw a library skill you no longer need",
        ),
        (
            "promotion_limit",
            409,
            &["skills", "promote", "changelog", "--to", "org"],
            "Withdraw a held request with `nexus skills withdraw <id>`",
        ),
        (
            "promotion_exists",
            409,
            &["skills", "promote", "changelog", "--to", "org"],
            "This version is already requested or approved.",
        ),
        (
            "not_in_team",
            403,
            &["skills", "promote", "changelog", "--to", "team/ops"],
            "or to the organization with `--to org`.",
        ),
        (
            "unknown_team",
            404,
            &["skills", "promote", "changelog", "--to", "team/zzz"],
            "`nexus team list` lists the teams.",
        ),
        (
            "personal_org",
            409,
            &["skills", "promote", "changelog", "--to", "org"],
            "`nexus org switch <org>`",
        ),
        (
            "team_deleted",
            409,
            &["skills", "approve", "spr_1"],
            "Refuse it with `nexus skills refuse <id> --reason TEXT`",
        ),
        (
            "invalid_promotion",
            400,
            &["skills", "withdraw", "spr_1", "--all-versions"],
            "Use `--all-versions` with an approved request",
        ),
        (
            "invalid_cursor",
            400,
            &["skills", "promotions", "--cursor", "zzz"],
            "or leave --cursor out.",
        ),
        (
            "role_required",
            403,
            &["skills", "withdraw", "spr_1"],
            "Members ask; owners, admins and team leads decide.",
        ),
        (
            "skill_not_found",
            404,
            &["skills", "library"],
            "`nexus skills` lists your skills. `nexus skills library` lists the library's.",
        ),
        (
            "skill_version_not_found",
            404,
            &["skills", "promote", "changelog", "9", "--to", "org"],
            "`nexus skills library` lists the library's.",
        ),
    ];
    for (code, status, argv, fix) in cases {
        let message = format!("gateway sentence for {code}");
        let r = run(
            route(move |ask| {
                (ask.method == "POST" || ask.path.starts_with("/v1/skill-")).then(|| refusal(status, code, &message))
            }),
            argv,
        );
        assert_eq!(r.code, 1, "{code}: {}", r.err);
        let shown = format!("Gateway sentence for {code} ({code})");
        assert!(r.err.contains(&shown), "{code}: {}", r.err);
        assert!(r.err.contains(fix), "{code}: {}", r.err);
        // The sentence, then one line that says the fix.
        assert_eq!(r.err.lines().count(), 2, "{code}: {}", r.err);
    }
}

// ---------------------------------------------------------------- library

fn library_route(entries: Vec<Value>) -> Route {
    route(move |ask| {
        (ask.method == "GET" && ask.path == "/v1/skill-library")
            .then(|| answer(200, json!({ "skills": entries.clone() })))
    })
}

#[test]
fn the_library_lists_what_you_may_load_and_marks_a_shared_name() {
    let r = run(
        library_route(vec![
            library_entry("skl_9", "changelog", "org", None, 3, false),
            library_entry("skl_8", "lint", "team", Some("team_web"), 1, false),
            library_entry("skl_7", "release", "org", None, 2, true),
            library_entry("skl_6", "release", "team", Some("team_ops"), 4, true),
        ]),
        &["skills", "library"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(requests(&r), ["GET /v1/skill-library", "GET /v1/teams"]);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(words(lines[0]), ["Skill", "Scope", "Version", "Description"]);
    assert_eq!(
        cells(lines[1]),
        [
            "changelog",
            "organization",
            "v3",
            "The",
            "changelog",
            "skill.",
            "In",
            "two",
            "lines."
        ]
    );
    assert_eq!(cells(lines[2])[..4], ["lint", "team", "web", "v1"]);
    assert_eq!(
        cells(lines[3])[..4],
        ["release", "(duplicate_name)", "organization", "v2"]
    );
    assert_eq!(
        cells(lines[4])[..5],
        ["release", "(duplicate_name)", "team", "ops", "v4"]
    );
    assert_eq!(
        lines[5..],
        [
            "",
            "! release (duplicate_name): another skill has this name, so none of them loads.",
            "  → Rename or delete your own skill, or ask an approver to withdraw all but one library skill of the name.",
        ]
    );
}

#[test]
fn the_library_filters_by_target() {
    let entries = vec![
        library_entry("skl_9", "changelog", "org", None, 3, false),
        library_entry("skl_8", "lint", "team", Some("team_web"), 1, false),
        library_entry("skl_5", "deploy", "team", Some("team_ops"), 1, false),
    ];
    let r = run(library_route(entries.clone()), &["skills", "library", "--to", "org"]);
    assert_eq!(requests(&r), ["GET /v1/skill-library"]);
    assert_eq!(r.out.lines().count(), 2);
    assert!(r.out.contains("changelog") && !r.out.contains("lint"), "{}", r.out);
    let r = run(
        library_route(entries.clone()),
        &["skills", "library", "--to", "team/web"],
    );
    assert_eq!(requests(&r), ["GET /v1/skill-library", "GET /v1/teams"]);
    assert!(
        r.out.contains("lint") && !r.out.contains("deploy") && !r.out.contains("changelog"),
        "{}",
        r.out
    );
    let r = run(
        library_route(entries.clone()),
        &["skills", "library", "--to", "team/zzz"],
    );
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("No team zzz in this organization; see `nexus team list` (unknown_team)"),
        "{}",
        r.err
    );
    let r = run(library_route(vec![]), &["skills", "library", "--to", "team/ops"]);
    assert_eq!(r.out, "No library skills for that target.");
    let r = run(library_route(vec![]), &["skills", "library"]);
    assert_eq!(
        r.out,
        "The library is empty. Promote a skill: nexus skills promote <name> --to org"
    );
    let r = run(none(), &["skills", "library", "--to", "nowhere"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("\"nowhere\" is not a target."), "{}", r.err);
    assert!(r.seen.is_empty());
}

#[test]
fn the_library_with_json_prints_the_list_and_a_filtered_list() {
    let entries = vec![
        library_entry("skl_9", "changelog", "org", None, 3, false),
        library_entry("skl_8", "lint", "team", Some("team_web"), 1, false),
    ];
    let r = run(library_route(entries.clone()), &["skills", "library", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).expect("json"),
        json!({ "skills": entries })
    );
    let r = run(
        library_route(entries.clone()),
        &["skills", "library", "--to", "org", "--json"],
    );
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).expect("json"),
        json!({ "skills": [entries[0].clone()] })
    );
}

#[test]
fn a_library_description_cannot_reach_the_terminal_as_a_command() {
    let mut entry = library_entry("skl_9", "changelog", "org", None, 3, false);
    entry["description"] = json!("Notes\u{1b}[2J\u{202e}gnp\u{9b}31m");
    let r = run(library_route(vec![entry]), &["skills", "library"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("Notes[U+001B][2J[U+202E]gnp[U+009B]31m"), "{}", r.out);
    assert!(!r.out.contains('\u{1b}') && !r.out.contains('\u{202e}') && !r.out.contains('\u{9b}'));
}

// ---------------------------------------------------------------- export

fn export_route(entries: Vec<Value>, library_status: Option<u16>) -> Route {
    route(move |ask| match (ask.method, ask.path) {
        ("GET", "/v1/skills") => Some(answer(
            200,
            json!({ "skills": [skill("skl_1", "changelog", 2), skill("skl_2", "pdf-processing", 1)] }),
        )),
        ("GET", "/v1/skill-library") => Some(match library_status {
            Some(status) => refusal(status, "internal_error", "the library is down"),
            None => answer(200, json!({ "skills": entries.clone() })),
        }),
        ("GET", path) if path.ends_with("/export") => Some(
            (
                [("content-type", "text/markdown; charset=utf-8")],
                format!("exported {path}").into_bytes(),
            )
                .into_response(),
        ),
        _ => None,
    })
}

#[test]
fn export_resolves_a_name_a_library_skill_has_and_says_which_skill_it_exported() {
    // An unmarked library skill of the name serves, even when the person has a skill of that name: it has the same bytes.
    let r = run(
        export_route(vec![library_entry("skl_9", "changelog", "org", None, 3, false)], None),
        &["skills", "export", "changelog"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    // The latest version is the one the list showed, so a version approved since is not written under that number.
    assert_eq!(
        requests(&r),
        [
            "GET /v1/skills",
            "GET /v1/skill-library",
            "GET /v1/skills/skl_9/versions/3/export"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(r.dir.join("changelog/SKILL.md")).expect("file"),
        "exported /v1/skills/skl_9/versions/3/export"
    );
    assert_eq!(
        r.out,
        [
            "✓ Wrote changelog/SKILL.md (version 3, 43 bytes).",
            "  Exported the organization's library skill changelog (skl_9), approved version 3."
        ]
        .join("\n")
    );
}

#[test]
fn export_names_a_team_library_skill_and_gives_json_its_scope() {
    let entries = vec![library_entry("skl_8", "lint", "team", Some("team_web"), 2, false)];
    let r = run(export_route(entries.clone(), None), &["skills", "export", "lint", "1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        [
            "GET /v1/skills",
            "GET /v1/skill-library",
            "GET /v1/skills/skl_8/versions/1/export",
            "GET /v1/teams"
        ]
    );
    // The second line names the version that was written, not the latest one.
    assert_eq!(
        r.out,
        [
            "✓ Wrote lint/SKILL.md (version 1, 43 bytes).",
            "  Exported team web's library skill lint (skl_8), approved version 1."
        ]
        .join("\n")
    );
    let r = run(export_route(entries, None), &["skills", "export", "skl_8", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).expect("json"),
        json!({ "id": "skl_8", "name": "lint", "version": 2, "path": "lint/SKILL.md", "bytes": 43, "scope": "team", "teamId": "team_web" })
    );
}

#[test]
fn a_version_number_names_your_own_skill_when_you_have_one_of_that_name() {
    // You have `changelog` (versions 1 and 2), and the library has `changelog` with the same latest bytes (version 3). A
    // library skill numbers its versions on its own, so `1` is your version 1, and not the library's.
    let entries = vec![library_entry("skl_9", "changelog", "org", None, 3, false)];
    let r = run(
        export_route(entries.clone(), None),
        &["skills", "export", "changelog", "1"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        [
            "GET /v1/skills",
            "GET /v1/skill-library",
            "GET /v1/skills/skl_1/versions/1/export"
        ]
    );
    assert_eq!(r.out, "✓ Wrote changelog/SKILL.md (version 1, 43 bytes).");
    // With `latest`, or no version, the library skill has the same bytes and serves.
    let r = run(
        export_route(entries.clone(), None),
        &["skills", "export", "changelog", "latest"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[2].path, "/v1/skills/skl_9/versions/3/export");
    // The library skill's own versions are named by its id.
    let r = run(export_route(entries, None), &["skills", "export", "skl_9", "2"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[2].path, "/v1/skills/skl_9/versions/2/export");
    assert!(
        r.out
            .ends_with("  Exported the organization's library skill changelog (skl_9), approved version 2."),
        "{}",
        r.out
    );
    // A version of your own skill that does not exist is Gateway's refusal, and the fix names both lists.
    let r = run(
        route(|ask| {
            ask.path
                .ends_with("/versions/9/export")
                .then(|| refusal(404, "skill_version_not_found", "skill skl_1 has no version 9"))
        }),
        &["skills", "export", "skl_1", "9"],
    );
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("`nexus skills library` lists the library's."),
        "{}",
        r.err
    );
}

#[test]
fn an_id_of_your_own_skill_exports_without_the_library() {
    // A library route that fails cannot stop the export of a skill you name by its id.
    let r = run(export_route(vec![], Some(500)), &["skills", "export", "skl_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        requests(&r),
        ["GET /v1/skills", "GET /v1/skills/skl_1/versions/latest/export"]
    );
    // A name needs the library, to know whether a library skill has it.
    let r = run(export_route(vec![], Some(500)), &["skills", "export", "changelog"]);
    assert_eq!(r.code, 1);
    assert_eq!(requests(&r), ["GET /v1/skills", "GET /v1/skill-library"]);
}

#[test]
fn export_stops_at_a_shared_name_and_names_the_scopes() {
    let shared = vec![
        library_entry("skl_9", "changelog", "org", None, 3, true),
        library_entry("skl_8", "changelog", "team", Some("team_web"), 1, true),
    ];
    let r = run(export_route(shared, None), &["skills", "export", "changelog"]);
    assert_eq!(r.code, 1);
    assert_eq!(
        r.err,
        [
            "✕ Error: 3 skills are named changelog: your own skills, the organization's library and team web's library (duplicate_name)",
            "  → None of them loads while the name is shared. Export one by its id (skl_1, skl_9, skl_8), or rename or delete your own skill.",
        ]
        .join("\n")
    );
    assert!(!requests(&r).iter().any(|request| request.ends_with("/export")));
    assert!(!r.dir.join("changelog").exists());
    // Without a skill of the person's own, the fix is the approver's.
    let two = vec![
        library_entry("skl_7", "lint", "org", None, 1, true),
        library_entry("skl_6", "lint", "team", Some("team_ops"), 1, true),
    ];
    let r = run(export_route(two, None), &["skills", "export", "lint"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("2 skills are named lint: the organization's library and team ops's library (duplicate_name)"),
        "{}",
        r.err
    );
    assert!(
        r.err.contains("or ask an approver to withdraw all but one."),
        "{}",
        r.err
    );
    // A marked skill of the person's own name collides too, and its id still exports.
    let marked = vec![library_entry("skl_9", "changelog", "org", None, 3, true)];
    let r = run(export_route(marked.clone(), None), &["skills", "export", "changelog"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("2 skills are named changelog: your own skills and the organization's library"),
        "{}",
        r.err
    );
    let r = run(export_route(marked, None), &["skills", "export", "skl_1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "✓ Wrote changelog/SKILL.md (version 2, 48 bytes).");
}

#[test]
fn export_of_your_own_skill_is_as_it_was() {
    // No library skill of the name: the person's skill, and one line, as before.
    let r = run(
        export_route(vec![library_entry("skl_9", "lint", "org", None, 3, false)], None),
        &["skills", "export", "pdf-processing", "--out", "skills"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "✓ Wrote skills/pdf-processing/SKILL.md (version 1, 48 bytes).");
    // A Gateway without the library route, and a key that may not hold skills, have no library.
    for status in [404, 403] {
        let r = run(
            export_route(vec![], Some(status)),
            &["skills", "export", "pdf-processing"],
        );
        assert_eq!(r.code, 0, "{status}: {}", r.err);
        assert_eq!(r.out, "✓ Wrote pdf-processing/SKILL.md (version 1, 48 bytes).");
    }
    // A library nobody could read is not "no library".
    let r = run(export_route(vec![], Some(500)), &["skills", "export", "pdf-processing"]);
    assert_eq!(r.code, 1);
    assert!(!requests(&r).iter().any(|request| request.ends_with("/export")));
    let r = run(export_route(vec![], None), &["skills", "export", "nope"]);
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("You have no skill named nope (skill_not_found)"),
        "{}",
        r.err
    );
}
