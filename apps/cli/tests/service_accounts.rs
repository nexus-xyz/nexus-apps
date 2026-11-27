//! `nexus service-accounts` against a stand-in Gateway: what each command sends, and that a token prints once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::service_accounts::account_expiry;
use nexus_cli::style::{now_ms, parse_ms};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_admin";
const TOKEN: &str = "nxsa_0123456789abcdef0123456789abcdef";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    body: Value,
    idempotency_key: Option<String>,
}

fn keys() -> Value {
    json!([
        { "id": "sak_live", "serviceAccountId": "sva_ci", "ownerUserId": "usr_ada", "scope": { "teamId": "team_web" }, "prefix": "nxsa_0123", "label": "github-actions", "createdBy": "usr_ada", "createdAt": "2026-10-01T00:00:00.000Z", "expiresAt": "2099-01-01T00:00:00.000Z", "lastUsedAt": "2026-10-02T00:00:00.000Z" },
        { "id": "sak_dead", "serviceAccountId": "sva_ci", "ownerUserId": "usr_ada", "scope": { "teamId": "team_web" }, "prefix": "nxsa_4567", "createdBy": "usr_ada", "createdAt": "2026-09-01T00:00:00.000Z", "expiresAt": "2099-01-01T00:00:00.000Z", "revokedAt": "2026-09-02T00:00:00.000Z" },
    ])
}

fn accounts() -> Value {
    json!([
        { "id": "sva_ci", "name": "ci-bot", "ownerUserId": "usr_ada", "teamId": "team_web", "createdBy": "usr_ada", "createdAt": "2026-09-01T00:00:00.000Z", "expiresAt": "2099-01-31T23:59:59.000Z", "keys": keys() },
        { "id": "sva_old", "name": "legacy", "ownerUserId": "usr_bo", "teamId": "team_web", "createdBy": "usr_ada", "createdAt": "2026-01-01T00:00:00.000Z", "expiresAt": "2099-01-01T00:00:00.000Z", "revokedAt": "2026-02-01T00:00:00.000Z", "keys": [] },
    ])
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn refusal(status: u16, message: &str, code: &str) -> Response {
    answer(status, json!({ "error": { "message": message, "code": code } }))
}

fn route(member: bool, method: &str, path: &str, query: &str, body: &Value) -> Response {
    if member {
        return refusal(
            403,
            "only the org's owner or admins can manage service accounts (service_accounts.manage)",
            "role_required",
        );
    }
    let tail = path.strip_prefix("/v1/service-accounts").unwrap_or("");
    let parts: Vec<&str> = tail.split('/').filter(|part| !part.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("GET", []) => answer(200, json!({ "serviceAccounts": accounts() })),
        ("POST", []) => {
            if body["team"] == "ghost" {
                return refusal(400, "no team ghost in this organization", "unknown_team");
            }
            let days = (parse_ms(body["expiresAt"].as_str().unwrap_or("")).unwrap_or(0.0) - now_ms()) / 86_400_000.0;
            if days > 365.0 {
                return refusal(400, "expires_at must be at most 365 days from now", "invalid_expiry");
            }
            let dry_run = body["dryRun"] == true;
            let created = json!({
                "id": if dry_run { "sva_dry" } else { "sva_new" },
                "name": body["name"],
                "ownerUserId": body.get("ownerUserId").cloned().unwrap_or(json!("usr_ada")),
                "teamId": "team_web", "createdBy": "usr_ada", "createdAt": "2026-10-26T00:00:00.000Z",
                "expiresAt": body["expiresAt"], "keys": [], "dryRun": dry_run,
            });
            answer(if dry_run { 200 } else { 201 }, created)
        }
        ("DELETE", ["sva_missing"]) => refusal(404, "no service account sva_missing", "unknown_service_account"),
        ("DELETE", ["sva_old"]) => answer(
            200,
            json!({ "id": "sva_old", "already": true, "dryRun": query == "dryRun=1", "revokedAt": "2026-02-01T00:00:00.000Z", "keysRevoked": 0 }),
        ),
        ("DELETE", [id]) => {
            let dry_run = query == "dryRun=1";
            answer(
                200,
                json!({
                    "id": id, "already": false, "dryRun": dry_run,
                    "revokedAt": if dry_run { Value::Null } else { json!("2026-10-26T00:00:00.000Z") },
                    "keysRevoked": u32::from(*id == "sva_ci"),
                }),
            )
        }
        ("POST", ["sva_old", "keys"]) => refusal(409, "the service account is revoked", "service_account_revoked"),
        ("POST", [id, "keys"]) => answer(
            201,
            json!({
                "token": TOKEN, "id": "sak_new", "serviceAccountId": id, "ownerUserId": "usr_ada", "scope": { "teamId": "team_web" },
                "prefix": "nxsa_0123", "label": body["label"], "createdBy": "usr_ada", "createdAt": "2026-10-26T00:00:00.000Z",
                "expiresAt": body.get("expiresAt").cloned().unwrap_or(json!("2027-01-24T00:00:00.000Z")),
            }),
        ),
        ("POST", [_, "keys", "sak_dead", "rotate"]) => refusal(
            409,
            "the key is revoked or expired; issue a new key instead",
            "key_not_live",
        ),
        ("POST", [id, "keys", key_id, "rotate"]) => answer(
            200,
            json!({
                "token": TOKEN, "id": "sak_next", "serviceAccountId": id, "ownerUserId": "usr_ada", "scope": { "teamId": "team_web" },
                "prefix": "nxsa_0123", "label": body.get("label").cloned().unwrap_or(json!("github-actions")),
                "createdBy": "usr_ada", "createdAt": "2026-10-26T00:00:00.000Z",
                "expiresAt": body.get("expiresAt").cloned().unwrap_or(json!("2027-01-24T00:00:00.000Z")),
                "rotatedFrom": key_id,
            }),
        ),
        ("DELETE", [id, "keys", key_id]) => {
            let mut key = keys()[0].clone();
            key["id"] = json!(key_id);
            key["serviceAccountId"] = json!(id);
            key["revokedAt"] = json!("2026-10-26T00:00:00.000Z");
            answer(200, key)
        }
        _ => refusal(404, "no such endpoint", "not_found"),
    }
}

fn gateway(member: bool) -> (String, Arc<Mutex<Vec<Seen>>>) {
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
                    let query = request.uri().query().unwrap_or("").to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push(Seen {
                        method: method.clone(),
                        path: path.clone(),
                        query: query.clone(),
                        body: body.clone(),
                        idempotency_key: headers
                            .get("idempotency-key")
                            .and_then(|value| value.to_str().ok())
                            .map(String::from),
                    });
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return refusal(401, "invalid API key", "authentication_error");
                    }
                    route(member, &method, &path, &query, &body)
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

    fn writes(&self) -> Vec<&Seen> {
        self.seen.iter().filter(|call| call.method != "GET").collect()
    }
}

fn run_as(member: bool, argv: &[&str]) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-service-accounts-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let (url, seen) = gateway(member);
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": url, "apiKey": KEY, "kind": "api", "org": "acme" }).to_string(),
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
    let out = capture.0.out.borrow().clone();
    let err = capture.0.err.borrow().join("\n");
    Run { code, out, err, seen }
}

fn run(argv: &[&str]) -> Run {
    run_as(false, argv)
}

#[test]
fn expires_is_a_day_to_the_end_of_that_utc_day_or_a_duration_from_now() {
    let now = parse_ms("2026-10-26T12:00:00.000Z").expect("now");
    for (value, expected) in [
        ("2027-01-31", "2027-01-31T23:59:59.000Z"),
        ("2028-02-29", "2028-02-29T23:59:59.000Z"),
        ("30d", "2026-11-25T12:00:00.000Z"),
        ("12h", "2026-10-27T00:00:00.000Z"),
        ("90", "2027-01-24T12:00:00.000Z"),
    ] {
        assert_eq!(account_expiry(Some(value), now).expect(value), expected);
    }
    for bad in [
        None,
        Some(""),
        Some("0d"),
        Some("-1d"),
        Some("1.5d"),
        Some("12m"),
        Some("2027-02-31"),
        Some("2027-13-01"),
        Some("2027-1-1"),
        Some("2027-01-31T10:00:00Z"),
        Some("tomorrow"),
    ] {
        assert!(account_expiry(bad, now).is_err(), "{bad:?}");
    }
}

#[test]
fn lists_the_accounts_then_their_keys_and_says_how_to_issue_one() {
    let listed = run(&["service-accounts"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(listed.seen.len(), 1);
    assert_eq!(
        (listed.seen[0].method.as_str(), listed.seen[0].path.as_str()),
        ("GET", "/v1/service-accounts")
    );
    let lines: Vec<&str> = listed
        .out
        .iter()
        .map(String::as_str)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let cells = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(cells(lines[0]), "Account Name Team Owner Keys Expires Status");
    assert_eq!(cells(lines[1]), "· sva_ci ci-bot team_web usr_ada 1 2099-01-31 active");
    assert_eq!(cells(lines[2]), "· sva_old legacy team_web usr_bo 0 2099-01-01 revoked");
    assert_eq!(
        cells(lines[3]),
        "Key Account Prefix Label Owner Team Expires Last used Status"
    );
    assert_eq!(
        cells(lines[4]),
        "· sak_live sva_ci nxsa_0123… github-actions usr_ada team_web 2099-01-01 2026-10-02 live"
    );
    assert_eq!(
        cells(lines[5]),
        "· sak_dead sva_ci nxsa_4567… – usr_ada team_web 2099-01-01 never revoked"
    );
    assert_eq!(
        lines[6],
        "Issue a key: nexus service-accounts keys create <account id> --label <label>"
    );
    assert!(!listed.text().contains(TOKEN));
    assert_eq!(run(&["service-accounts", "list"]).out, listed.out);
    let json = run(&["service-accounts", "list", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.text()).expect("json"),
        json!({ "serviceAccounts": accounts() })
    );
}

#[test]
fn refuses_a_member_with_the_servers_message_and_who_may() {
    let refused = run_as(true, &["service-accounts"]);
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("Only the org's owner or admins can manage service accounts (service_accounts.manage)"),
        "{}",
        refused.err
    );
    assert!(
        refused.err.contains("Ask an owner or admin of your organization."),
        "{}",
        refused.err
    );
    assert!(!refused.err.contains("Code:"), "{}", refused.err);
}

#[test]
fn shows_its_help_and_the_help_as_json() {
    let help = run(&["service-accounts", "--help"]);
    assert_eq!(help.code, 0);
    assert!(
        help.text().contains("  keys rotate <account id> <key id>"),
        "{}",
        help.text()
    );
    assert!(help.seen.is_empty());
    let json = run(&["service-accounts", "keys", "--help", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.text()).expect("json"),
        json!({ "usage": "nexus service-accounts <command>", "commands": ["list", "create", "revoke", "keys"] })
    );
}

fn create(extra: &[&str]) -> Run {
    let mut argv = vec![
        "service-accounts",
        "create",
        "--name",
        "ci",
        "--team",
        "web",
        "--expires",
        "2027-01-31",
    ];
    argv.extend_from_slice(extra);
    run(&argv)
}

#[test]
fn create_with_dry_run_asks_gateway_to_check_writes_nothing_and_says_how_to_create_it() {
    let dry = create(&["--owner", "usr_bo", "--dry-run"]);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(dry.seen.len(), 1);
    assert_eq!(dry.seen[0].method, "POST");
    assert_eq!(dry.seen[0].path, "/v1/service-accounts");
    assert_eq!(
        dry.seen[0].body,
        json!({ "name": "ci", "team": "web", "expiresAt": "2027-01-31T23:59:59.000Z", "ownerUserId": "usr_bo", "dryRun": true })
    );
    assert_eq!(
        dry.out,
        vec![
            "! Would create service account ci for team web: owner usr_bo, expires 2027-01-31T23:59:59.000Z. Run without --dry-run to create it."
        ]
    );
    let json = create(&["--dry-run", "--json"]);
    let answer: Value = serde_json::from_str(&json.text()).expect("json");
    assert_eq!(answer["id"], "sva_dry");
    assert_eq!(answer["dryRun"], true);
    assert_eq!(answer["ownerUserId"], "usr_ada");
}

#[test]
fn create_makes_the_account_and_points_at_its_first_key() {
    let created = create(&[]);
    assert_eq!(created.code, 0, "{}", created.err);
    let writes = created.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].body,
        json!({ "name": "ci", "team": "web", "expiresAt": "2027-01-31T23:59:59.000Z" })
    );
    assert!(writes[0].idempotency_key.is_some());
    assert_eq!(
        created.out,
        vec![
            "✓ Created service account ci",
            "  Id       sva_new",
            "  Team     web",
            "  Owner    usr_ada",
            "  Expires  2027-01-31T23:59:59.000Z",
            "Issue its first key: nexus service-accounts keys create sva_new --label <label>",
        ]
    );
}

#[test]
fn create_turns_a_duration_into_a_time_that_far_from_now() {
    let before = now_ms();
    let created = run(&[
        "service-accounts",
        "create",
        "--name",
        "ci",
        "--team",
        "web",
        "--expires",
        "30d",
    ]);
    assert_eq!(created.code, 0, "{}", created.err);
    let sent = parse_ms(created.writes()[0].body["expiresAt"].as_str().expect("expiresAt")).expect("time");
    let month = 30.0 * 86_400_000.0;
    assert!(sent - before >= month);
    assert!(sent - now_ms() <= month);
}

#[test]
fn create_refuses_a_missing_or_bad_argument_before_it_sends_anything() {
    let owner_without_value = create(&["--owner"]);
    assert_eq!(owner_without_value.code, 2);
    assert!(owner_without_value.seen.is_empty());
    for argv in [
        vec!["service-accounts", "create"],
        vec!["service-accounts", "create", "--team", "web", "--expires", "30d"],
        vec!["service-accounts", "create", "--name", "ci", "--expires", "30d"],
        vec!["service-accounts", "create", "--name", "ci", "--team", "web"],
        vec![
            "service-accounts",
            "create",
            "--name",
            "ci",
            "--team",
            "web",
            "--expires",
        ],
        vec![
            "service-accounts",
            "create",
            "--name",
            "ci",
            "--team",
            "web",
            "--expires",
            "2027-02-31",
        ],
        vec![
            "service-accounts",
            "create",
            "--name",
            "ci",
            "--team",
            "web",
            "--expires",
            "soon",
        ],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
    let usage = run(&["service-accounts", "create"]);
    assert!(
        usage.err.contains(
            "Usage: nexus service-accounts create --name <name> --team <slug> [--owner <user-id>] --expires <YYYY-MM-DD|30d> [--dry-run]"
        ),
        "{}",
        usage.err
    );
}

#[test]
fn create_prints_only_gateways_message_when_it_refuses() {
    let refused = run(&[
        "service-accounts",
        "create",
        "--name",
        "ci",
        "--team",
        "ghost",
        "--expires",
        "30d",
    ]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.err.contains("No team ghost in this organization"),
        "{}",
        refused.err
    );
}

#[test]
fn create_leaves_a_field_name_in_gateways_message_as_gateway_wrote_it() {
    let refused = create(&["--expires", "400d"]);
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("Error: expires_at must be at most 365 days from now"),
        "{}",
        refused.err
    );
}

#[test]
fn revoke_with_dry_run_sends_the_query_and_says_what_would_stop() {
    let dry = run(&["service-accounts", "revoke", "sva_ci", "--dry-run"]);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(dry.seen.len(), 1);
    assert_eq!(
        (
            dry.seen[0].method.as_str(),
            dry.seen[0].path.as_str(),
            dry.seen[0].query.as_str()
        ),
        ("DELETE", "/v1/service-accounts/sva_ci", "dryRun=1")
    );
    assert_eq!(
        dry.out,
        vec!["! Would revoke sva_ci: 1 live key would stop working at once. Run without --dry-run to revoke it."]
    );
    let none = run(&["service-accounts", "revoke", "sva_new", "--dry-run"]);
    assert_eq!(
        none.out,
        vec!["! Would revoke sva_new: it has no live keys. Run without --dry-run to revoke it."]
    );
}

#[test]
fn revoke_stops_the_keys_and_says_when_it_was_revoked_before() {
    let revoked = run(&["service-accounts", "revoke", "sva_ci"]);
    assert_eq!(revoked.code, 0, "{}", revoked.err);
    assert_eq!(
        (
            revoked.seen[0].method.as_str(),
            revoked.seen[0].path.as_str(),
            revoked.seen[0].query.as_str()
        ),
        ("DELETE", "/v1/service-accounts/sva_ci", "")
    );
    assert!(revoked.seen[0].idempotency_key.is_some());
    assert_eq!(revoked.out, vec!["✓ Revoked sva_ci: 1 live key stopped."]);
    assert_eq!(
        run(&["service-accounts", "revoke", "sva_old"]).out,
        vec!["✓ sva_old was already revoked."]
    );
    let json = run(&["service-accounts", "revoke", "sva_ci", "--dry-run", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&json.text()).expect("json"),
        json!({ "id": "sva_ci", "already": false, "dryRun": true, "revokedAt": null, "keysRevoked": 1 })
    );
}

#[test]
fn revoke_refuses_a_missing_id_before_it_sends_anything_and_prints_gateways_message_for_an_unknown_one() {
    let missing = run(&["service-accounts", "revoke"]);
    assert_eq!(missing.code, 2);
    assert!(missing.seen.is_empty());
    let unknown = run(&["service-accounts", "revoke", "sva_missing"]);
    assert_eq!(unknown.code, 1);
    assert!(
        unknown.err.contains("No service account sva_missing"),
        "{}",
        unknown.err
    );
}

#[test]
fn keys_create_prints_the_token_once_with_the_warning() {
    let issued = run(&[
        "service-accounts",
        "keys",
        "create",
        "sva_ci",
        "--label",
        "github-actions",
        "--expires",
        "2026-12-31",
    ]);
    assert_eq!(issued.code, 0, "{}", issued.err);
    assert_eq!(
        (issued.seen[0].method.as_str(), issued.seen[0].path.as_str()),
        ("POST", "/v1/service-accounts/sva_ci/keys")
    );
    assert_eq!(
        issued.seen[0].body,
        json!({ "label": "github-actions", "expiresAt": "2026-12-31T23:59:59.000Z" })
    );
    assert_eq!(
        issued.out,
        vec![
            "✓ Issued a key for sva_ci".to_string(),
            "  Key      sak_new".to_string(),
            "  Label    github-actions".to_string(),
            "  Owner    usr_ada".to_string(),
            "  Team     team_web".to_string(),
            "  Expires  2026-12-31T23:59:59.000Z".to_string(),
            format!("  Token    {TOKEN}"),
            "! Copy the token now. Nexus does not show it again.".to_string(),
        ]
    );
    assert_eq!(issued.text().matches(TOKEN).count(), 1);
}

#[test]
fn keys_create_sends_no_expiry_when_none_is_given_and_prints_the_token_in_json() {
    let issued = run(&[
        "service-accounts",
        "keys",
        "create",
        "sva_ci",
        "--label",
        "deploy",
        "--json",
    ]);
    assert_eq!(issued.code, 0, "{}", issued.err);
    assert_eq!(issued.seen[0].body, json!({ "label": "deploy" }));
    let answer: Value = serde_json::from_str(&issued.text()).expect("json");
    assert_eq!(answer["token"], TOKEN);
    assert_eq!(answer["id"], "sak_new");
    assert_eq!(answer["label"], "deploy");
}

#[test]
fn keys_rotate_prints_the_new_token_once_and_names_the_old_key() {
    let rotated = run(&["service-accounts", "keys", "rotate", "sva_ci", "sak_live"]);
    assert_eq!(rotated.code, 0, "{}", rotated.err);
    assert_eq!(
        (rotated.seen[0].method.as_str(), rotated.seen[0].path.as_str()),
        ("POST", "/v1/service-accounts/sva_ci/keys/sak_live/rotate")
    );
    assert_eq!(rotated.seen[0].body, json!({}));
    assert_eq!(
        rotated.out,
        vec![
            "✓ Rotated key sak_live of sva_ci: the old key no longer works".to_string(),
            "  Key      sak_next".to_string(),
            "  Label    github-actions".to_string(),
            "  Owner    usr_ada".to_string(),
            "  Team     team_web".to_string(),
            "  Expires  2027-01-24T00:00:00.000Z".to_string(),
            format!("  Token    {TOKEN}"),
            "! Copy the token now. Nexus does not show it again.".to_string(),
        ]
    );
    assert_eq!(rotated.text().matches(TOKEN).count(), 1);
}

#[test]
fn keys_rotate_sends_the_label_and_expiry_it_is_given_and_nothing_else() {
    let rotated = run(&[
        "service-accounts",
        "keys",
        "rotate",
        "sva_ci",
        "sak_live",
        "--label",
        "release",
        "--expires",
        "2026-12-31",
    ]);
    assert_eq!(rotated.code, 0, "{}", rotated.err);
    assert_eq!(
        rotated.seen[0].body,
        json!({ "label": "release", "expiresAt": "2026-12-31T23:59:59.000Z" })
    );
    assert!(
        rotated.out.contains(&"  Label    release".to_string()),
        "{:?}",
        rotated.out
    );
    assert!(
        rotated.out.contains(&"  Expires  2026-12-31T23:59:59.000Z".to_string()),
        "{:?}",
        rotated.out
    );
    let label_only = run(&[
        "service-accounts",
        "keys",
        "rotate",
        "sva_ci",
        "sak_live",
        "--label",
        "release",
    ]);
    assert_eq!(label_only.seen[0].body, json!({ "label": "release" }));
    let expires_only = run(&[
        "service-accounts",
        "keys",
        "rotate",
        "sva_ci",
        "sak_live",
        "--expires",
        "30d",
    ]);
    assert_eq!(expires_only.seen[0].body.as_object().map(|body| body.len()), Some(1));
    assert!(expires_only.seen[0].body.get("expiresAt").is_some());
}

#[test]
fn keys_rotate_refuses_a_flag_without_a_value_before_it_sends_anything() {
    for argv in [
        vec!["service-accounts", "keys", "rotate", "sva_ci", "sak_live", "--label"],
        vec![
            "service-accounts",
            "keys",
            "rotate",
            "sva_ci",
            "sak_live",
            "--label",
            "",
        ],
        vec!["service-accounts", "keys", "rotate", "sva_ci", "sak_live", "--expires"],
        vec![
            "service-accounts",
            "keys",
            "rotate",
            "sva_ci",
            "sak_live",
            "--expires",
            "2027-02-31",
        ],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn keys_revoke_stops_the_key_and_never_prints_a_token() {
    let revoked = run(&["service-accounts", "keys", "revoke", "sva_ci", "sak_live"]);
    assert_eq!(revoked.code, 0, "{}", revoked.err);
    assert_eq!(
        (revoked.seen[0].method.as_str(), revoked.seen[0].path.as_str()),
        ("DELETE", "/v1/service-accounts/sva_ci/keys/sak_live")
    );
    assert_eq!(
        revoked.out,
        vec!["✓ Key sak_live of sva_ci is revoked: it no longer works."]
    );
}

#[test]
fn keys_print_only_gateways_message_when_it_refuses() {
    let rotated = run(&["service-accounts", "keys", "rotate", "sva_ci", "sak_dead"]);
    assert_eq!(rotated.code, 1);
    assert!(
        rotated
            .err
            .contains("The key is revoked or expired; issue a new key instead"),
        "{}",
        rotated.err
    );
    let late = run(&["service-accounts", "keys", "create", "sva_old", "--label", "late"]);
    assert_eq!(late.code, 1);
    assert!(late.err.contains("The service account is revoked"), "{}", late.err);
}

#[test]
fn keys_refuse_a_missing_argument_or_flag_before_they_send_anything() {
    for argv in [
        vec!["service-accounts", "keys"],
        vec!["service-accounts", "keys", "mint"],
        vec!["service-accounts", "keys", "create"],
        vec!["service-accounts", "keys", "create", "sva_ci"],
        vec!["service-accounts", "keys", "create", "--label", "ci"],
        vec![
            "service-accounts",
            "keys",
            "create",
            "sva_ci",
            "--label",
            "ci",
            "--expires",
        ],
        vec![
            "service-accounts",
            "keys",
            "create",
            "sva_ci",
            "--label",
            "ci",
            "--expires",
            "2027-02-31",
        ],
        vec!["service-accounts", "keys", "rotate", "sva_ci"],
        vec!["service-accounts", "keys", "revoke", "sva_ci"],
        vec!["service-accounts", "mint"],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
    }
}

#[test]
fn keys_refuse_dry_run_because_the_api_has_none() {
    for argv in [
        vec![
            "service-accounts",
            "keys",
            "create",
            "sva_ci",
            "--label",
            "ci",
            "--dry-run",
        ],
        vec!["service-accounts", "keys", "rotate", "sva_ci", "sak_live", "--dry-run"],
        vec!["service-accounts", "keys", "revoke", "sva_ci", "sak_live", "--dry-run"],
    ] {
        let refused = run(&argv);
        assert_eq!(refused.code, 2, "{argv:?}");
        assert!(refused.seen.is_empty(), "{argv:?}");
        assert!(
            refused.err.contains("--dry-run does not apply to keys"),
            "{}",
            refused.err
        );
    }
}
