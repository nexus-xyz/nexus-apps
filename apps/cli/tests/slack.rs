//! `nexus slack` against a stand-in Gateway, as `slack.test.ts` does it: connect, confirm, status and disconnect, and
//! `nexus slack channel` (in [`channel`]): what bind, list and unbind send and print.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_admin";

/// Numbers every temp dir in this file. Both `run` helpers draw from it: with one counter each, both made
/// `nexus-slack-<pid>-0`, and two tests running at once shared a `config.json`.
static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn pending() -> Value {
    json!({ "id": "chw_pending", "platform": "slack", "teamId": "T0ACME", "teamName": "Acme", "scopes": ["chat:write"], "state": "pending", "installedBy": "usr_ada", "createdAt": "2026-10-27T10:00:00.000Z", "expiresAt": "2026-10-27T11:00:00.000Z" })
}

fn active() -> Value {
    json!({ "id": "chw_active", "platform": "slack", "teamId": "T0BETA", "teamName": "Beta", "scopes": ["chat:write"], "state": "active", "installedBy": "usr_ada", "confirmedBy": "usr_ada", "createdAt": "2026-10-26T09:00:00.000Z", "expiresAt": "2026-10-26T10:00:00.000Z", "confirmedAt": "2026-10-26T10:00:00.000Z" })
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn refusal(status: u16, message: &str, code: &str) -> Response {
    answer(status, json!({ "error": { "message": message, "code": code } }))
}

fn route(member: bool, method: &str, path: &str) -> Response {
    if member {
        return refusal(
            403,
            "only the org's owner or admins can connect a Slack workspace (chat_apps.manage)",
            "role_required",
        );
    }
    match (method, path) {
        ("POST", "/v1/slack/installs") => answer(
            201,
            json!({ "installId": "chw_new", "authorizeUrl": "https://slack.com/oauth/v2/authorize?client_id=1&state=s", "expiresAt": "2026-10-27T11:00:00.000Z" }),
        ),
        ("GET", "/v1/slack/installs/chw_waiting") => refusal(
            404,
            "no such Slack install in this organization yet: approve the app in Slack first, or start again with nexus slack connect",
            "install_not_found",
        ),
        ("GET", "/v1/slack/installs/chw_pending") => answer(
            200,
            json!({ "installId": "chw_pending", "status": "pending", "workspace": pending() }),
        ),
        ("GET", "/v1/slack/installs/chw_active") => answer(
            200,
            json!({ "installId": "chw_active", "status": "active", "workspace": active() }),
        ),
        ("POST", "/v1/slack/installs/chw_pending/confirm") => {
            let mut bound = pending();
            bound["state"] = json!("active");
            answer(
                200,
                json!({ "installId": "chw_pending", "status": "active", "workspace": bound }),
            )
        }
        ("GET", "/v1/slack/workspaces") => answer(200, json!({ "workspaces": [pending(), active()] })),
        ("DELETE", "/v1/slack/workspaces/chw_active") => answer(
            200,
            json!({ "removed": true, "uninstalled": true, "credentialRevoked": true }),
        ),
        ("DELETE", "/v1/slack/workspaces/chw_shared") => answer(
            200,
            json!({ "removed": true, "uninstalled": true, "credentialRevoked": false }),
        ),
        ("DELETE", "/v1/slack/workspaces/chw_slackdown") => {
            refusal(502, "Slack refused the call: invalid_auth", "slack_refused")
        }
        ("DELETE", "/v1/slack/workspaces/chw_unconfigured") => refusal(
            503,
            "the Nexus Slack app is not configured on this Gateway (SLACK_CLIENT_ID, SLACK_CLIENT_SECRET and NEXUS_PUBLIC_URL)",
            "not_configured",
        ),
        ("DELETE", "/v1/slack/workspaces/chw_missing") => refusal(
            404,
            "no such Slack workspace binding in this organization",
            "workspace_not_found",
        ),
        _ => refusal(404, "no such endpoint", "not_found"),
    }
}

fn gateway(member: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
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
                    log.lock().expect("log").push(format!("{method} {path}"));
                    if request.headers().get("authorization").and_then(|v| v.to_str().ok())
                        != Some(&format!("Bearer {KEY}"))
                    {
                        return refusal(401, "invalid API key", "authentication_error");
                    }
                    route(member, &method, &path)
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
    seen: Vec<String>,
}

fn run_as(member: bool, argv: &[&str]) -> Run {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-slack-{}-{}",
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
    let out = capture.0.out.borrow().join("\n");
    let err = capture.0.err.borrow().join("\n");
    Run { code, out, err, seen }
}

fn run(argv: &[&str]) -> Run {
    run_as(false, argv)
}

#[test]
fn connect_starts_an_install_and_prints_the_link_and_the_confirm_command() {
    let started = run(&["slack", "connect"]);
    assert_eq!(started.code, 0, "{}", started.err);
    assert_eq!(started.seen, ["POST /v1/slack/installs"]);
    assert!(
        started
            .out
            .contains("https://slack.com/oauth/v2/authorize?client_id=1&state=s")
    );
    assert!(started.out.contains("nexus slack connect --confirm chw_new"));
    let dry = run(&["slack", "connect", "--dry-run"]);
    assert_eq!(dry.code, 0);
    assert!(dry.seen.is_empty(), "a dry run sends nothing");
}

#[test]
fn confirm_shows_the_workspace_on_a_dry_run_then_binds_it() {
    let dry = run(&["slack", "connect", "--confirm", "chw_pending", "--dry-run"]);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(dry.seen, ["GET /v1/slack/installs/chw_pending"]);
    assert!(dry.out.contains("Would bind Slack workspace Acme (T0ACME)"));
    let bound = run(&["slack", "connect", "--confirm", "chw_pending"]);
    assert_eq!(bound.code, 0, "{}", bound.err);
    assert_eq!(
        bound.seen,
        [
            "GET /v1/slack/installs/chw_pending",
            "POST /v1/slack/installs/chw_pending/confirm"
        ]
    );
    assert!(bound.out.contains("Bound Slack workspace Acme (T0ACME)"));
}

#[test]
fn confirm_before_slacks_callback_is_refused_and_an_active_binding_answers_as_it_is() {
    let waiting = run(&["slack", "connect", "--confirm", "chw_waiting"]);
    assert_eq!(waiting.code, 1);
    assert!(
        waiting
            .err
            .contains("No such Slack install in this organization yet: approve the app in Slack first")
    );
    let already = run(&["slack", "connect", "--confirm", "chw_active"]);
    assert_eq!(already.code, 0, "{}", already.err);
    assert!(already.out.contains("already bound"));
    assert!(!already.seen.iter().any(|call| call.starts_with("POST")));
}

#[test]
fn a_member_is_refused_with_the_servers_words_and_who_may() {
    let refused = run_as(true, &["slack", "connect"]);
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("Only the org's owner or admins can connect a Slack workspace (chat_apps.manage)")
    );
    assert!(refused.err.contains("Ask an owner or admin of your organization."));
}

#[test]
fn status_lists_the_workspaces_and_json_prints_gateways_answer() {
    let listed = run(&["slack", "status"]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    let lines: Vec<&str> = listed.out.lines().collect();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("chw_pending") && line.contains("Acme") && line.contains("pending"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("chw_active") && line.contains("T0BETA") && line.contains("active"))
    );
    let json = run(&["slack", "status", "--json"]);
    let parsed: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(parsed, json!({ "workspaces": [pending(), active()] }));
}

#[test]
fn disconnect_says_what_it_would_do_then_does_it() {
    let dry = run(&["slack", "disconnect", "chw_active", "--dry-run"]);
    assert_eq!(dry.code, 0, "{}", dry.err);
    assert_eq!(dry.seen, ["GET /v1/slack/workspaces"]);
    assert!(
        dry.out
            .contains("Would disconnect Beta (T0BETA): Gateway would uninstall the app from it")
    );
    let removed = run(&["slack", "disconnect", "chw_active"]);
    assert_eq!(removed.code, 0, "{}", removed.err);
    assert_eq!(removed.seen, ["DELETE /v1/slack/workspaces/chw_active"]);
    assert!(
        !removed.out.contains("The bot token was not deleted"),
        "{}",
        removed.out
    );
    let shared = run(&["slack", "disconnect", "chw_shared"]);
    assert_eq!(shared.code, 0, "{}", shared.err);
    assert!(
        shared
            .out
            .contains("The bot token was not deleted: another binding of this organization still uses it"),
        "{}",
        shared.out
    );
    let missing = run(&["slack", "disconnect", "chw_missing"]);
    assert_eq!(missing.code, 1);
    assert!(
        missing
            .err
            .contains("No such Slack workspace binding in this organization")
    );
    assert_eq!(run(&["slack", "disconnect"]).code, 2);
}

#[test]
fn slack_failing_prints_gateways_words_and_an_unset_app_says_who_sets_it_up() {
    let down = run(&["slack", "disconnect", "chw_slackdown"]);
    assert_eq!(down.code, 1);
    assert!(
        down.err.contains("Slack refused the call: invalid_auth"),
        "{}",
        down.err
    );
    let unset = run(&["slack", "disconnect", "chw_unconfigured"]);
    assert_eq!(unset.code, 1);
    assert!(
        unset
            .err
            .contains("The Nexus Slack app is not configured on this Gateway"),
        "{}",
        unset.err
    );
    assert!(
        unset
            .err
            .contains("Ask whoever runs this Gateway to set up the Nexus Slack app."),
        "{}",
        unset.err
    );
}

#[test]
fn help_as_json_names_the_commands() {
    let text = run(&["slack", "--help"]);
    assert_eq!(text.code, 0);
    assert!(
        text.out.contains("connect your own account: nexus connect slack"),
        "{}",
        text.out
    );
    let help = run(&["slack", "--help", "--json"]);
    assert_eq!(help.code, 0);
    let parsed: Value = serde_json::from_str(&help.out).expect("json");
    assert_eq!(
        parsed,
        json!({ "usage": "nexus slack <command>", "commands": ["connect", "status", "disconnect", "channel"] })
    );
}

/// `nexus slack channel` against its own stand-in Gateway: a refusal in Gateway's words too.
mod channel {
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
        body: Value,
    }

    fn answer(status: u16, body: Value) -> Response {
        (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
    }

    fn bound() -> Value {
        json!({
            "workspaceId": "chw_acme", "channelId": "C0WEB", "teamId": "team_web", "teamSlug": "web",
            "createdBy": "usr_ada", "createdAt": "2026-10-28T00:00:00.000Z", "updatedAt": "2026-10-28T00:00:00.000Z",
        })
    }

    fn route(method: &str, path: &str, body: &Value) -> Response {
        match (method, path) {
            ("POST", "/v1/slack/workspaces/chw_acme/channels") if body["team"] == "ops" => answer(
                403,
                json!({ "error": { "message": "team ops is not one you lead", "type": "forbidden", "code": "outside_led_teams" } }),
            ),
            ("POST", "/v1/slack/workspaces/chw_acme/channels") => answer(201, bound()),
            ("GET", "/v1/slack/workspaces/chw_acme/channels") => answer(200, json!({ "channels": [bound()] })),
            ("DELETE", "/v1/slack/workspaces/chw_acme/channels/C0WEB") => answer(
                200,
                json!({ "workspaceId": "chw_acme", "channelId": "C0WEB", "teamId": "team_web", "removed": true }),
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
                let app = Router::new().fallback(move |request: Request| {
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
        err: String,
        seen: Vec<Seen>,
    }

    fn run(argv: &[&str]) -> Run {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "nexus-slack-{}-{}",
            std::process::id(),
            super::NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let (url, seen) = gateway();
        std::fs::create_dir_all(&dir).expect("dir");
        let config_path = dir.join("config.json");
        std::fs::write(
            &config_path,
            json!({ "url": url, "apiKey": "nx_slack", "org": "acme" }).to_string(),
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
    fn binds_a_channel_to_a_team_and_says_so() {
        let bound = run(&["slack", "channel", "bind", "chw_acme", "C0WEB", "--team", "web"]);
        assert_eq!(bound.code, 0, "{}", bound.err);
        let post = bound.seen.iter().find(|seen| seen.method == "POST").expect("post");
        assert_eq!(post.path, "/v1/slack/workspaces/chw_acme/channels");
        assert_eq!(post.body, json!({ "channelId": "C0WEB", "team": "web" }));
        assert_eq!(
            bound.out,
            "✓ Channel C0WEB is bound to team web: its calls go to that team, under its policy and caps."
        );
    }

    #[test]
    fn lists_the_bound_channels_and_unbinds_one() {
        let listed = run(&["slack", "channel", "list", "chw_acme"]);
        assert_eq!(listed.code, 0, "{}", listed.err);
        let lines: Vec<&str> = listed.out.lines().filter(|line| !line.trim().is_empty()).collect();
        let words = |line: &str| line.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            words(lines[0]),
            ["Channel", "Team", "Bound", "by", "Bound"],
            "{lines:?}"
        );
        assert_eq!(
            words(lines[1]),
            ["·", "C0WEB", "web", "usr_ada", "2026-10-28"],
            "{lines:?}"
        );
        let unbound = run(&["slack", "channel", "unbind", "chw_acme", "C0WEB", "--json"]);
        assert_eq!(unbound.code, 0, "{}", unbound.err);
        let delete = unbound.seen.last().expect("delete");
        assert_eq!(
            (delete.method.as_str(), delete.path.as_str()),
            ("DELETE", "/v1/slack/workspaces/chw_acme/channels/C0WEB")
        );
        let printed: Value = serde_json::from_str(&unbound.out).expect("json");
        assert_eq!(printed["removed"], true);
    }

    #[test]
    fn prints_gateways_refusal_and_a_usage_error_without_a_team() {
        let refused = run(&["slack", "channel", "bind", "chw_acme", "C0OPS", "--team", "ops"]);
        assert_eq!(refused.code, 1);
        assert!(refused.err.contains("Team ops is not one you lead"), "{}", refused.err);
        let no_team = run(&["slack", "channel", "bind", "chw_acme", "C0OPS"]);
        assert_eq!(no_team.code, 2);
    }
}
