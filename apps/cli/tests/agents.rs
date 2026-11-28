//! `nexus claude`, `nexus codex` and `nexus agents` on each route, against a stand-in Gateway and a stand-in agent.
#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_agents";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

/// What the stand-in Gateway answers.
#[derive(Clone, Default)]
struct Setup {
    /// The organization turned subscription mode off.
    refuse: bool,
    /// `GET /v1/agents/access`'s answer; `None`: a Gateway without the endpoint (404).
    access: Option<Value>,
    signed_out: bool,
    /// The saved key is one Gateway does not accept (401).
    wrong_key: bool,
    /// The saved Gateway URL has nothing listening.
    unreachable: bool,
    /// What Gateway says about the person's skills; `None`: a Gateway without the skills routes (404).
    skills: Option<Skills>,
    /// `GET /v1/tenants/me` marks the person `staff`: one of Nexus's own people.
    staff: bool,
    /// `POST /v1/agents/sessions/{id}/usage` answers 503, which the client sends again.
    refuse_usage: bool,
    /// As `refuse_usage`, for a usage batch of fewer than 200 records only: a run's last one.
    refuse_last_usage_batch: bool,
    /// `POST /v1/agents/route`'s answer; `None`: 404 (older Gateway, falls back to `/v1/agents/check`).
    auto_route: Option<Value>,
    /// `GET /v1/models` catalog entries when set.
    catalog_models: Option<Vec<Value>>,
}

/// The person's skills on the stand-in Gateway (`GET /v1/skills`, `GET /v1/tenants/me`, the exports).
#[derive(Clone, Default)]
struct Skills {
    /// `GET /v1/tenants/me`'s `user.id`; `None`: a key that belongs to no person.
    user: Option<String>,
    /// `GET /v1/skills`'s list.
    list: Vec<Value>,
    /// `GET /v1/skills` answers this body instead of `{ "skills": list }`.
    list_body: Option<Value>,
    /// Each export, by skill id and version.
    files: HashMap<(String, i64), Vec<u8>>,
    /// `GET /v1/skills` answers this status and code instead of the list.
    refuse: Option<(u16, &'static str)>,
    /// `POST /v1/skill-loads` answers this status and code.
    refuse_records: Option<(u16, &'static str)>,
    /// The library (`GET /v1/skill-library`); `None`: a Gateway without the route (404).
    library: Option<Library>,
    /// Exports answer 429 `rate_limited` while this has refusals left.
    limit: Option<RateLimit>,
}

/// Gateway's rate limit on the exports: `refusals` 429s, shared by every request, each with this `Retry-After`.
#[derive(Clone)]
struct RateLimit {
    refusals: Arc<std::sync::atomic::AtomicUsize>,
    retry_after: &'static str,
}

impl RateLimit {
    fn new(refusals: usize, retry_after: &'static str) -> RateLimit {
        RateLimit {
            refusals: Arc::new(std::sync::atomic::AtomicUsize::new(refusals)),
            retry_after,
        }
    }
}

/// The library on the stand-in Gateway. Its skills are exported like any other, by id and version.
#[derive(Clone, Default)]
struct Library {
    /// `GET /v1/skill-library`'s list.
    list: Vec<Value>,
    /// `GET /v1/skill-library` answers this body instead of `{ "skills": list }`.
    list_body: Option<Value>,
    /// `GET /v1/skill-library` answers this status and code instead of the list.
    refuse: Option<(u16, &'static str)>,
}

/// A library skill as the library list gives it, with the file its export serves.
#[derive(Clone)]
struct Shared {
    id: &'static str,
    name: &'static str,
    version: i64,
    file: &'static str,
    /// The team of a team's skill; `None`: the organization's.
    team: Option<&'static str>,
    /// Gateway marked the name: another skill of it keeps this one from loading.
    duplicate: bool,
}

/// A skill of the organization's library.
fn org_skill(id: &'static str, name: &'static str, version: i64, file: &'static str) -> Shared {
    Shared {
        id,
        name,
        version,
        file,
        team: None,
        duplicate: false,
    }
}

/// A skill of a team's library.
fn team_skill(id: &'static str, name: &'static str, version: i64, file: &'static str, team: &'static str) -> Shared {
    Shared {
        team: Some(team),
        ..org_skill(id, name, version, file)
    }
}

impl Shared {
    fn marked(self) -> Shared {
        Shared {
            duplicate: true,
            ..self
        }
    }
}

impl Skills {
    /// These skills in the library, on top of whatever it holds.
    fn sharing(mut self, shared: &[Shared]) -> Skills {
        let library = self.library.get_or_insert_with(Library::default);
        for skill in shared {
            let mut entry = json!({
                "id": skill.id, "name": skill.name, "description": "d", "version": skill.version,
                "scope": if skill.team.is_some() { "team" } else { "org" },
                "approvedAt": "2026-10-28T12:00:00.000Z",
            });
            if let Some(team) = skill.team {
                entry["teamId"] = json!(team);
            }
            if skill.duplicate {
                entry["duplicateName"] = json!(true);
            }
            library.list.push(entry);
            self.files
                .insert((skill.id.to_string(), skill.version), skill.file.as_bytes().to_vec());
        }
        self
    }

    /// Ada's skills: each `(id, name, version, file)` is listed at that version and exported as `file`.
    fn of(user: &str, skills: &[(&str, &str, i64, &str)]) -> Skills {
        let mut found = Skills {
            user: Some(user.to_string()),
            ..Skills::default()
        };
        for (id, name, version, file) in skills {
            found.list.push(json!({
                "id": id, "name": name, "description": "d", "latestVersion": version,
                "createdAt": "2026-10-18T12:00:00.000Z", "updatedAt": "2026-10-18T12:00:00.000Z",
            }));
            found.files.insert((id.to_string(), *version), file.as_bytes().to_vec());
        }
        found
    }
}

fn answer_bytes(bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/markdown; charset=utf-8")],
        bytes,
    )
        .into_response()
}

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

/// Gateway's answer with the claude entry's route set (`from`: `preference`, `organization`), as the gateway builds it.
fn access(route: &str, from: &str) -> Value {
    let mut answer = nexus_client::agent_setup::offline_agent_access();
    answer["agents"][0]["route"] = json!(route);
    answer["agents"][0]["from"] = json!(from);
    if from == "organization" {
        answer["agents"][0]["preference"] = json!("subscription");
        answer["agentSubscriptions"] = json!(false);
    }
    answer
}

/// The same answer with the codex entry's route set.
fn codex_access(route: &str, from: &str) -> Value {
    let mut answer = nexus_client::agent_setup::offline_agent_access();
    let codex = answer["agents"]
        .as_array_mut()
        .and_then(|agents| agents.iter_mut().find(|entry| entry["agent"] == "codex"))
        .expect("a codex entry");
    codex["route"] = json!(route);
    codex["from"] = json!(from);
    answer
}

/// Sessions are `ags_<agent>_<via>` (`_quiet` for team `quiet`), so ending one knows what to report without state.
fn route(method: &str, path: &str, body: &Value, setup: &Setup) -> Response {
    match (method, path) {
        ("GET", "/v1/orgs/settings") => answer(200, json!({ "agentSubscriptions": !setup.refuse })),
        ("POST", "/v1/agents/route") => match &setup.auto_route {
            Some(reply) => answer(200, reply.clone()),
            None => answer(
                404,
                json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
            ),
        },
        ("GET", "/v1/models") => match &setup.catalog_models {
            Some(models) => answer(200, json!({ "data": models })),
            None => answer(200, json!({ "data": [] })),
        },
        ("POST", "/v1/agents/check") => {
            let provider_model = if body["agent"] == "codex" {
                "gpt-6-luna"
            } else {
                "claude-sonnet-5-5"
            };
            answer(
                200,
                json!({ "allowed": true, "providerModel": provider_model, "defaultFrom": { "level": "organization" } }),
            )
        }
        ("GET", "/v1/agents/access") => match &setup.access {
            Some(access) => answer(200, access.clone()),
            None => answer(
                404,
                json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
            ),
        },
        ("PUT", "/v1/agents/access") => match &setup.access {
            Some(access) => answer(200, access.clone()),
            None => answer(
                404,
                json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
            ),
        },
        ("POST", "/v1/agents/sessions") => {
            let (agent, via) = (body["agent"].as_str().unwrap_or(""), body["via"].as_str().unwrap_or(""));
            if setup.refuse && via == "subscription" {
                return answer(
                    403,
                    json!({ "error": {
                        "message": "your organization has turned off subscription mode: run the agent without --subscription (through Gateway); an owner or admin turns it back on with `nexus org settings set agentSubscriptions true`",
                        "type": "permission_error", "code": "subscriptions_disabled",
                    } }),
                );
            }
            let quiet = if body["team"] == "quiet" { "_quiet" } else { "" };
            let paid = if via == "nexus" {
                json!("byok")
            } else {
                json!("subscription")
            };
            answer(
                201,
                json!({
                    "id": format!("ags_{agent}_{via}{quiet}"), "agent": agent, "via": via, "model": body["model"],
                    "status": "running", "access": paid,
                }),
            )
        }
        ("GET", "/v1/skills") => match &setup.skills {
            None => answer(
                404,
                json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
            ),
            Some(Skills {
                refuse: Some((status, code)),
                ..
            }) => answer(
                *status,
                json!({ "error": { "message": "refused", "type": "api_error", "code": code } }),
            ),
            Some(skills) => answer(
                200,
                skills
                    .list_body
                    .clone()
                    .unwrap_or_else(|| json!({ "skills": skills.list })),
            ),
        },
        ("GET", "/v1/skill-library") => match setup.skills.as_ref().and_then(|skills| skills.library.as_ref()) {
            None => answer(
                404,
                json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
            ),
            Some(Library {
                refuse: Some((status, code)),
                ..
            }) => answer(
                *status,
                json!({ "error": { "message": "refused", "type": "api_error", "code": code } }),
            ),
            Some(library) => answer(
                200,
                library
                    .list_body
                    .clone()
                    .unwrap_or_else(|| json!({ "skills": library.list })),
            ),
        },
        ("GET", "/v1/tenants/me") => {
            let user = setup.skills.as_ref().and_then(|skills| skills.user.clone());
            let user = match (user, setup.staff) {
                (user, true) => Some(json!({ "id": user.unwrap_or_else(|| "usr_staff".into()), "mark": "staff" })),
                (user, false) => user.map(|id| json!({ "id": id })),
            };
            answer(
                200,
                json!({
                    "tenant": { "id": "ten_acme", "name": "Acme", "createdAt": "2026-10-18T12:00:00.000Z" },
                    "user": user,
                    "keyId": "key_1",
                }),
            )
        }
        ("GET", export) if export.starts_with("/v1/skills/") && export.ends_with("/export") => {
            if let Some(limit) = setup.skills.as_ref().and_then(|skills| skills.limit.as_ref()) {
                let refused = limit
                    .refusals
                    .fetch_update(
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                        |left| left.checked_sub(1),
                    )
                    .is_ok();
                if refused {
                    let mut limited = answer(
                        429,
                        json!({ "error": { "message": "more than 120 requests a minute; slow down", "type": "rate_limit_error", "code": "rate_limited" } }),
                    );
                    limited
                        .headers_mut()
                        .insert("retry-after", limit.retry_after.parse().expect("header"));
                    return limited;
                }
            }
            let parts: Vec<&str> = export.split('/').collect();
            let (id, version) = (parts[3], parts[5].parse::<i64>().unwrap_or(0));
            match setup
                .skills
                .as_ref()
                .and_then(|skills| skills.files.get(&(id.to_string(), version)))
            {
                Some(bytes) => answer_bytes(bytes.clone()),
                None => answer(
                    404,
                    json!({ "error": { "message": "no such version", "type": "not_found", "code": "skill_version_not_found" } }),
                ),
            }
        }
        ("POST", "/v1/skill-loads") => match setup.skills.as_ref().and_then(|skills| skills.refuse_records) {
            Some((status, code)) => answer(
                status,
                json!({ "error": { "message": "refused", "type": "api_error", "code": code } }),
            ),
            None => answer(201, json!({ "loads": [] })),
        },
        ("POST", beat) if beat.starts_with("/v1/agents/sessions/") && beat.ends_with("/heartbeat") => {
            answer(200, json!({ "status": "running" }))
        }
        ("POST", usage) if usage.starts_with("/v1/agents/sessions/") && usage.ends_with("/usage") => {
            let records = body["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
                .as_array()
                .map_or(0, Vec::len);
            if setup.refuse_usage || (setup.refuse_last_usage_batch && records < 200) {
                answer(
                    503,
                    json!({ "error": { "message": "try again", "type": "api_error", "code": "unavailable" } }),
                )
            } else {
                answer(200, json!({}))
            }
        }
        ("POST", end) if end.starts_with("/v1/agents/sessions/") && end.ends_with("/end") => {
            let id = end.trim_start_matches("/v1/agents/sessions/").trim_end_matches("/end");
            let usage = match id {
                "ags_claude_subscription" => json!({
                    "calls": 2, "inputTokens": 100, "cacheReadTokens": 14000, "cacheCreationTokens": 46, "outputTokens": 5,
                    "estimatedCostUsd": 0.0015, "models": ["claude-sonnet-5-5"],
                }),
                "ags_codex_subscription" => json!({
                    "calls": 2, "inputTokens": 14146, "cacheReadTokens": 0, "cacheCreationTokens": 0, "outputTokens": 5,
                    "estimatedCostUsd": 0.031059, "unpricedCalls": 1, "models": ["gpt-6-luna"],
                }),
                // What Gateway counted of the calls `nexus agy` posted: unpriced, and no model named.
                "ags_antigravity_subscription" => json!({
                    "calls": 2, "inputTokens": 18067, "cacheReadTokens": 0, "cacheCreationTokens": 0, "outputTokens": 200,
                    "estimatedCostUsd": 0, "unpricedCalls": 2, "models": [],
                }),
                _ => Value::Null,
            };
            answer(
                200,
                json!({ "id": id, "status": "failed", "exitCode": body["exitCode"], "durationMs": 1200, "usage": usage }),
            )
        }
        ("GET", "/v1/modules") => answer(200, json!({ "modules": [review_manifest()], "unavailable": [] })),
        ("POST", "/v1/robustness/review/apply") => {
            let mut bundle = json!({
                "finding": body["finding"], "repo": "acme/app", "pr": 7, "pr_url": "https://github.com/acme/app/pull/7",
                "run": "run_1", "head_sha": "abc1234", "moved": body["finding"] == "NR-0a0a0a", "path": "src/lib.rs",
                "line_start": 12, "line_end": 14, "title": "Unwrap on input", "detail": "It panics on empty input.",
                "command": "nexus robustness apply review --repo acme/app --run run_1 --finding NR-3f2a9c",
            });
            if body["finding"] == "NR-f0f0f0" {
                bundle["prompt"] = Value::Null;
                bundle["withheld"] = json!("fork");
                bundle["withheld_reason"] = json!("The pull request comes from a fork.");
            } else {
                bundle["prompt"] = json!(APPLY_PROMPT);
            }
            answer(200, bundle)
        }
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

/// What the stand-in `apply` answers as the fix prompt.
const APPLY_PROMPT: &str = "Fix one finding that Nexus Review posted on pull request #7 of acme/app.\n```text\nfinding: NR-3f2a9c\n```\n5. Do not commit or push.";

/// Nexus Review's manifest as far as the CLI reads it for `apply`.
fn review_manifest() -> Value {
    let input = |name: &str, verbs: &[&str]| json!({ "name": name, "label": name, "kind": "text", "verbs": verbs });
    json!({
        "product": { "slug": "robustness", "name": "Nexus Robustness" },
        "capability": { "slug": "review", "name": "Nexus Review", "description": "Reviews pull requests." },
        "verbs": ["connect", "run", "watch", "report", "apply"],
        "inputs": [
            merge_required(input("repo", &["connect", "run", "watch", "report", "apply"])),
            input("pr", &["run", "apply"]),
            input("finding", &["apply"]),
            input("run", &["apply"]),
        ],
        "panel": { "kind": "pages" },
        "operations": {},
    })
}

fn merge_required(mut input: Value) -> Value {
    input["required"] = json!(true);
    input
}

/// A stand-in Gateway on a free port, on its own thread; every request it saw.
fn gateway(setup: Setup) -> (String, Arc<Mutex<Vec<Seen>>>) {
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
                let setup = setup.clone();
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
                    route(&method, &path, &body, &setup)
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
    url: String,
    /// What the stand-in agent was given, one `name: value` per line; `None` when it never ran.
    agent: Option<String>,
}

impl Run {
    fn opened(&self) -> Option<&Value> {
        self.seen
            .iter()
            .find(|seen| seen.method == "POST" && seen.path == "/v1/agents/sessions")
            .map(|seen| &seen.body)
    }

    fn asked(&self, method: &str, path: &str) -> Option<&Value> {
        self.seen
            .iter()
            .find(|seen| seen.method == method && seen.path == path)
            .map(|seen| &seen.body)
    }
}

/// A run's folder: Nexus's config, the stand-in agent's report, and the agents' own folders. A test may write into it
/// before the run and read it after. It goes when the value drops.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new() -> Home {
        let dir = std::env::temp_dir().join(format!("nexus-agents-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("dir");
        Home { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(path, bytes).expect("write");
    }

    fn read(&self, relative: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(relative)).ok()
    }

    fn exists(&self, relative: &str) -> bool {
        std::fs::symlink_metadata(self.path(relative)).is_ok()
    }

    /// Nexus's own folder for the agent folder `relative` (the lock and the manifests): the first 16 characters of the
    /// SHA-256 of its path.
    fn state(&self, relative: &str) -> String {
        let key = nexus_client::base::sha256_hex(&[self.path(relative).to_string_lossy().as_bytes()]);
        format!("skills-delivered/{}", &key[..16])
    }

    /// Every file under `relative`, by its path from there.
    fn files(&self, relative: &str) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, at: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
            let Ok(read) = std::fs::read_dir(at) else { return };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, found);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    found.insert(path.strip_prefix(root).expect("inside").display().to_string(), bytes);
                }
            }
        }
        let root = self.path(relative);
        let mut found = BTreeMap::new();
        walk(&root, &root, &mut found);
        found
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One `nexus` run with the stand-in agents, and `env` on top of the user's own provider settings.
fn run(argv: &[&str], env: &[(&str, &str)], setup: Setup) -> Run {
    run_in(&Home::new(), argv, env, setup)
}

/// A run in a folder the test made. `{home}` in an `env` value is the folder.
fn run_in(home: &Home, argv: &[&str], env: &[(&str, &str)], setup: Setup) -> Run {
    stand_ins();
    let (signed_out, wrong_key, unreachable) = (setup.signed_out, setup.wrong_key, setup.unreachable);
    let (url, seen) = gateway(setup);
    let url = if unreachable {
        "http://127.0.0.1:1".to_string()
    } else {
        url
    };
    let dir = home.dir.clone();
    let config_path = dir.join("config.json");
    let config = match (signed_out, wrong_key) {
        (true, _) => json!({ "url": url }),
        (_, true) => json!({ "url": url, "apiKey": "nx_wrong" }),
        _ => json!({ "url": url, "apiKey": KEY }),
    };
    std::fs::write(&config_path, config.to_string()).expect("config");
    let report = dir.join("agent.txt");
    let mut vars: HashMap<String, String> = HashMap::from([
        ("NEXUS_URL".to_string(), "https://production.test".to_string()),
        ("NEXUS_UI_URL".to_string(), "http://ui.test/app".to_string()),
        ("ANTHROPIC_API_KEY".to_string(), "sk-ant-own".to_string()),
        ("ANTHROPIC_BASE_URL".to_string(), "https://proxy.test".to_string()),
        ("ANTHROPIC_AUTH_TOKEN".to_string(), "own-token".to_string()),
        ("ANTHROPIC_CUSTOM_HEADERS".to_string(), "x-own: 1".to_string()),
        ("OPENAI_API_KEY".to_string(), "sk-own".to_string()),
        ("OTEL_LOG_USER_PROMPTS".to_string(), "1".to_string()),
        ("AGENT_REPORT".to_string(), report.display().to_string()),
        ("STAND_IN_LOGIN".to_string(), "plan".to_string()),
    ]);
    for agent in ["claude", "codex"] {
        vars.insert(
            format!("NEXUS_{}_BIN", agent.to_uppercase()),
            stand_in().display().to_string(),
        );
    }
    vars.insert(
        "NEXUS_ANTIGRAVITY_BIN".to_string(),
        agy_stand_in().display().to_string(),
    );
    vars.extend(
        env.iter()
            .map(|(k, v)| (k.to_string(), v.replace("{home}", &dir.display().to_string()))),
    );
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: vars,
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
    let agent = std::fs::read_to_string(&report).ok();
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        seen,
        url,
        agent,
    }
}

/// The stand-in agent: its sign-in check answers `STAND_IN_LOGIN`; a run writes what it got to `$AGENT_REPORT`.
const STAND_IN: &str = r#"#!/bin/sh
[ "$1" = --version ] && echo "9.9.9 (stand-in)" && exit 0
if [ -n "$STAND_IN_MODELS" ] && [ "$1 $2 $3" = "-p --input-format stream-json" ]; then
  read _line
  printf '%s\n' "$STAND_IN_MODELS"
  exit 0
fi
if [ "$1 $2" = "auth status" ]; then
  case "$STAND_IN_LOGIN" in
    plan) echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max"}' ;;
    account) echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"ada@example.com"}' ;;
    signed_out) echo '{"loggedIn":false}' ;;
    api_key) echo '{"loggedIn":true,"authMethod":"console"}' ;;
  esac
  exit 0
fi
if [ "$1 $2" = "login status" ]; then
  case "$STAND_IN_LOGIN" in
    plan) echo "Logged in using ChatGPT" ;;
    signed_out) echo "Not logged in" ;;
    api_key) echo "Logged in using an API key" ;;
  esac
  exit 0
fi
{
  # printf, not echo: dash's echo turns the \n in a JSON argument into a line break.
  printf 'args: %s\n' "$*"
  echo "ANTHROPIC_BASE_URL: $ANTHROPIC_BASE_URL"
  echo "ANTHROPIC_AUTH_TOKEN: $ANTHROPIC_AUTH_TOKEN"
  echo "ANTHROPIC_CUSTOM_HEADERS: $ANTHROPIC_CUSTOM_HEADERS"
  echo "ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY"
  echo "ANTHROPIC_MODEL: $ANTHROPIC_MODEL"
  echo "CLAUDE_CODE_DISABLE_ARTIFACT: $CLAUDE_CODE_DISABLE_ARTIFACT"
  echo "OPENAI_API_KEY: $OPENAI_API_KEY"
  echo "NEXUS_API_KEY: $NEXUS_API_KEY"
  echo "CLAUDE_CODE_ENABLE_TELEMETRY: $CLAUDE_CODE_ENABLE_TELEMETRY"
  echo "OTEL_LOG_USER_PROMPTS: $OTEL_LOG_USER_PROMPTS"
  echo "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT: $OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"
  echo "OTEL_EXPORTER_OTLP_LOGS_HEADERS: $OTEL_EXPORTER_OTLP_LOGS_HEADERS"
  echo "NEXUS_URL: $NEXUS_URL"
  echo "NEXUS_UI_URL: $NEXUS_UI_URL"
  echo "SKILLS_SEEN: $(ls "${CLAUDE_CONFIG_DIR:-/nonexistent}/skills" 2>/dev/null | tr '\n' ' ')"
  echo "CODEX_SKILLS_SEEN: $(ls "${CODEX_HOME:-/nonexistent}/skills" 2>/dev/null | tr '\n' ' ')"
  echo "COMMIT_HOOK: $(git config --get hook.nexus-commit-trailer.event 2>/dev/null)"
  echo "COMMIT_HOOK_CALL: $(git config --get hook.nexus-commit-trailer.command 2>/dev/null | tail -n 1)"
} > "$AGENT_REPORT"
[ -n "$STAND_IN_SLEEP" ] && sleep "$STAND_IN_SLEEP"
exit 3
"#;

/// Every stand-in agent of this file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct StandIns {
    agent: PathBuf,
    agy: PathBuf,
    golden: PathBuf,
    golden_agy: PathBuf,
}

/// Writes every stand-in at once, when the first run asks for one ([`run_in`] does first): a fork while a test writes
/// an executable makes exec fail (ETXTBSY), so none is written once runs start agents.
fn stand_ins() -> &'static StandIns {
    static BINS: OnceLock<StandIns> = OnceLock::new();
    BINS.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("nexus-agents-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let usage = dir.join("usage.json");
        std::fs::write(
            &usage,
            include_str!("../../../crates/nexus-client/tests/fixtures/agy-1.5.8-usage.json"),
        )
        .expect("usage");
        let write = |name: &str, script: &str| {
            let bin = dir.join(name);
            std::fs::write(&bin, script).expect(name);
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            bin
        };
        let agy = |script: &str| format!("#!/bin/sh\nUSAGE='{}'\n{script}", usage.display());
        StandIns {
            agent: write("agent", STAND_IN),
            agy: write("agy", &agy(AGY_STAND_IN)),
            golden: write("golden-agent", GOLDEN_STAND_IN),
            golden_agy: write("golden-agy", &agy(GOLDEN_AGY_STAND_IN)),
        }
    })
}

fn stand_in() -> &'static PathBuf {
    &stand_ins().agent
}

/// The stand-in `agy`: `--version` answers `$STAND_IN_AGY_VERSION` (else 1.5.7). The sign-in check
/// (`-p /usage --output-format json`) prints agy 1.5.8's signed-in answer, or with `$STAND_IN_SIGNED_OUT` its sign-in
/// prompt, and then waits as agy does. A run writes its arguments and the variables that would bill elsewhere to
/// `$AGENT_REPORT`, then writes `$STAND_IN_CONVERSATIONS` new conversations under `$HOME`, as `agy` keeps them, or
/// copies the conversation file `$STAND_IN_COPY` there as `$STAND_IN_COPY_ID`. `$USAGE` is set as it is written.
const AGY_STAND_IN: &str = r#"[ "$1" = --version ] && echo "${STAND_IN_AGY_VERSION:-1.5.7}" && exit 0
if [ "$1 $2" = "-p /usage" ]; then
  if [ -n "$STAND_IN_SIGNED_OUT" ]; then
    echo "Authentication required. Please visit the URL to log in:" >&2
    /bin/sleep 30
    exit 1
  fi
  /bin/cat "$USAGE"
  exit 0
fi
{
  printf 'args: %s\n' "$*"
  echo "GEMINI_API_KEY: $GEMINI_API_KEY"
  echo "GOOGLE_API_KEY: $GOOGLE_API_KEY"
  echo "GOOGLE_APPLICATION_CREDENTIALS: $GOOGLE_APPLICATION_CREDENTIALS"
  echo "GOOGLE_CLOUD_PROJECT: $GOOGLE_CLOUD_PROJECT"
  echo "CLOUDSDK_CORE_PROJECT: $CLOUDSDK_CORE_PROJECT"
  echo "AGY_ADC_AUTH: $AGY_ADC_AUTH"
  echo "GOOGLE_GEMINI_BASE_URL: $GOOGLE_GEMINI_BASE_URL"
  echo "ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY"
  echo "OPENAI_API_KEY: $OPENAI_API_KEY"
  echo "NEXUS_API_KEY: $NEXUS_API_KEY"
  echo "NEXUS_URL: $NEXUS_URL"
  echo "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT: $OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"
  echo "COMMIT_HOOK: $(git config --get hook.nexus-commit-trailer.event 2>/dev/null)"
} > "$AGENT_REPORT"
# A file's time comes from the kernel's coarse clock, which may read a few milliseconds behind the run's start.
[ "${STAND_IN_CONVERSATIONS:-0}" -gt 0 ] && sleep 0.1
i=0
while [ "$i" -lt "${STAND_IN_CONVERSATIONS:-0}" ]; do
  mkdir -p "$HOME/.gemini/antigravity-cli/conversations"
  : > "$HOME/.gemini/antigravity-cli/conversations/0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6$i.db"
  i=$((i + 1))
done
if [ -n "$STAND_IN_COPY" ]; then
  sleep 0.1
  mkdir -p "$HOME/.gemini/antigravity-cli/conversations"
  cp "$STAND_IN_COPY" "$HOME/.gemini/antigravity-cli/conversations/$STAND_IN_COPY_ID.db"
fi
exit 3
"#;

fn agy_stand_in() -> &'static PathBuf {
    &stand_ins().agy
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

fn line<'a>(agent: &'a str, name: &str) -> &'a str {
    agent
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("{name} in {agent}"))
}

#[test]
fn claude_subscription_runs_on_its_own_login_and_reports_usage_to_the_session() {
    let staff = Setup {
        staff: true,
        ..Setup::default()
    };
    let run = run(&["claude", "--subscription", "-p", "hi"], &[], staff);
    assert_eq!(run.code, 3, "{}", run.err);
    let opened = run.opened().expect("a session");
    assert_eq!(opened["via"], "subscription");
    // The organization's default model is a policy lookup, not a call through Gateway, so it applies here too.
    assert_eq!(opened["model"], "claude-sonnet-5-5");
    // A flag decides without asking Gateway for the saved route.
    assert!(run.asked("GET", "/v1/agents/access").is_none());
    let agent = run.agent.as_deref().expect("the agent ran");
    for gone in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_CUSTOM_HEADERS",
        "ANTHROPIC_API_KEY",
        "OTEL_LOG_USER_PROMPTS",
        // The wrapper's Gateway: what the agent runs (tests, scripts) must not inherit it.
        "NEXUS_URL",
        "NEXUS_UI_URL",
    ] {
        assert_eq!(line(agent, gone), "", "{gone} reached Claude Code");
    }
    assert_eq!(line(agent, "ANTHROPIC_MODEL"), "claude-sonnet-5-5");
    // The plan's tier from Claude Code's own check, mapped to the contract's list.
    assert_eq!(opened["plan"], "max");
    // Pages, not Claude Code's own artifacts, on every route.
    assert_eq!(line(agent, "CLAUDE_CODE_DISABLE_ARTIFACT"), "1");
    assert!(line(agent, "args").contains("--append-system-prompt If this session exposes Nexus MCP tools:"));
    assert!(agent.contains(nexus_client::mcp::CONNECTOR_INSTRUCTIONS));
    assert!(agent.contains("\n\nNexus pages: when your response creates"));
    assert_eq!(line(agent, "CLAUDE_CODE_ENABLE_TELEMETRY"), "1");
    // The git the agent runs reads the commit trailer hook: the person is one of Nexus's own. The agent is
    // told, so it leaves the trailer alone.
    assert_eq!(line(agent, "COMMIT_HOOK"), "commit-msg");
    assert!(agent.contains("Nexus co-author: when a commit you make"), "{agent}");
    assert_eq!(
        line(agent, "COMMIT_HOOK_CALL"),
        "nexus_commit_trailer \"Co-authored-by: Nexus CLI <cli@nexus.inc>\""
    );
    assert_eq!(
        line(agent, "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"),
        format!("{}/v1/agents/sessions/ags_claude_subscription/usage", run.url)
    );
    assert_eq!(
        line(agent, "OTEL_EXPORTER_OTLP_LOGS_HEADERS"),
        format!("Authorization=Bearer {KEY}")
    );
    assert!(
        run.err
            .contains("claude on your Claude subscription · model calls go straight to Anthropic"),
        "{}",
        run.err
    );
    assert!(
        run.err.contains(
            "session ags_claude_subscription · 2 calls · 14.1K → 5 tokens (99% cached) · claude-sonnet-5-5 · ~$0.0015 list, on your subscription"
        ),
        "{}",
        run.err
    );
}

#[test]
fn a_session_sends_the_plan_tier_this_machine_last_read_and_nothing_else_of_the_account() {
    let home = Home::new();
    home.write(
        "agent-plans.json",
        br#"{"codex":{"plan":"team","at":1},"claude":{"plan":"ada@example.com","at":1}}"#,
    );
    let codex = run_in(
        &home,
        &["codex", "exec", "hi"],
        &[("NEXUS_CODEX_AUTH", "subscription")],
        Setup::default(),
    );
    assert_eq!(codex.code, 3, "{}", codex.err);
    assert_eq!(codex.opened().expect("a session")["plan"], "team");
    // A saved value outside the list is not sent: the session opens without a plan.
    let claude = run_in(&home, &["claude", "--api", "-p", "hi"], &[], Setup::default());
    assert_eq!(claude.code, 3, "{}", claude.err);
    assert!(claude.opened().expect("a session").get("plan").is_none());
    // No run sent a usage signal: only the binary sends them.
    assert!(
        codex
            .seen
            .iter()
            .chain(&claude.seen)
            .all(|seen| !seen.path.starts_with("/v1/diagnostics")),
        "{:?}",
        codex.seen.iter().map(|seen| &seen.path).collect::<Vec<_>>()
    );
}

#[test]
fn an_agent_that_does_not_start_is_queued_as_a_client_error_with_codes_only() {
    let home = Home::new();
    home.write("not-executable", b"#!/bin/sh\n");
    let run = run_in(
        &home,
        &["claude", "--api", "-p", "hi"],
        &[("NEXUS_CLAUDE_BIN", "{home}/not-executable")],
        Setup::default(),
    );
    assert!(run.err.contains("Could not start"), "{}", run.err);
    let queued: Value =
        serde_json::from_slice(&std::fs::read(home.path("usage-errors.json")).expect("queued")).expect("json");
    let entry = &queued[0];
    assert_eq!(entry["app"], "cli");
    assert_eq!(entry["kind"], "agent");
    assert_eq!(entry["component"], "agent_spawn");
    assert_eq!(entry["code"], "permission_denied");
    assert!(!queued.to_string().contains("not-executable"), "{queued}");
}

#[test]
fn each_variable_sets_its_own_agents_route() {
    let codex = run(
        &["codex", "exec", "hi"],
        &[("NEXUS_CODEX_AUTH", " Subscription ")],
        Setup {
            staff: true,
            ..Setup::default()
        },
    );
    assert_eq!(codex.code, 3, "{}", codex.err);
    assert_eq!(codex.opened().expect("a session")["via"], "subscription");
    let agent = codex.agent.as_deref().expect("the agent ran");
    let args = line(agent, "args");
    assert!(args.starts_with("-c shell_environment_policy.set.GIT_CONFIG_PARAMETERS="));
    assert!(args.contains("-c forced_login_method=\"chatgpt\" -c otel.log_user_prompt=false -c otel.exporter="));
    assert!(args.contains(&format!(
        "endpoint = \"{}/v1/agents/sessions/ags_codex_subscription/usage\"",
        codex.url
    )));
    // The organization's default for Codex, as Codex names it; no Gateway provider, catalog or key in argv.
    assert!(
        args.contains(" protocol = \"json\" } } -c features.shell_snapshot=false -c model=\"gpt-6-luna\" -c developer_instructions="),
        "{args}"
    );
    let instructions = args
        .split_once("-c developer_instructions=")
        .expect("developer instructions")
        .1
        .strip_suffix(" exec hi")
        .expect("agent arguments follow the instructions");
    let instructions: String = serde_json::from_str(instructions).expect("JSON-encoded developer instructions");
    assert!(instructions.contains(nexus_client::mcp::CONNECTOR_INSTRUCTIONS));
    assert!(instructions.contains("\n\nNexus pages: when your response creates"));
    assert!(args.ends_with(" exec hi"), "{args}");
    assert!(!args.contains(KEY) && !args.contains("model_provider") && !args.contains("model_catalog_json"));
    assert_eq!(line(agent, "OPENAI_API_KEY"), "");
    assert_eq!(line(agent, "COMMIT_HOOK"), "commit-msg");
    assert_eq!(
        line(agent, "OTEL_EXPORTER_OTLP_LOGS_HEADERS"),
        format!("authorization=Bearer {KEY}")
    );
    assert!(
        codex
            .err
            .contains("codex on your ChatGPT subscription · from NEXUS_CODEX_AUTH · model calls go straight to OpenAI")
    );
    assert!(codex.err.contains(
        "2 calls · 14.1K → 5 tokens (0% cached) · gpt-6-luna · ~$0.03 list, 1 calls unpriced, on your subscription"
    ));

    // NEXUS_CODEX_AUTH is Codex's only: Claude Code still asks Gateway for the saved route, and goes through Gateway.
    let claude = run(
        &["claude", "-p", "hi"],
        &[("NEXUS_CODEX_AUTH", "subscription")],
        Setup {
            access: Some(access("gateway", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(claude.opened().expect("a session")["via"], "nexus");
    assert!(claude.asked("GET", "/v1/agents/access").is_some());
    assert_eq!(line(claude.agent.as_deref().expect("ran"), "ANTHROPIC_AUTH_TOKEN"), KEY);
    assert!(
        claude.err.contains(
            "(reserved, metered and checked by Gateway) · paid with your organization's Anthropic key (BYOK)"
        )
    );
    assert!(claude.err.contains(
        "session ags_claude_nexus · paid with your organization's Anthropic key (BYOK) · spend: nexus spend"
    ));

    let wrong = run(
        &["claude", "-p", "hi"],
        &[("NEXUS_CLAUDE_AUTH", "nope")],
        Setup::default(),
    );
    assert_eq!(wrong.code, 1);
    assert!(
        wrong.err.contains("NEXUS_CLAUDE_AUTH is \"nope\", not a route."),
        "{}",
        wrong.err
    );
    assert!(wrong.opened().is_none());
}

#[test]
fn the_saved_route_decides_unless_a_flag_does_and_the_organization_may_override_it() {
    let saved = run(
        &["claude", "-p", "hi"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(saved.opened().expect("a session")["via"], "subscription");
    assert!(
        saved
            .err
            .contains("claude on your Claude subscription · your saved route (nexus agents) ·"),
        "{}",
        saved.err
    );

    let flag = run(
        &["claude", "--api", "-p", "hi"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(flag.opened().expect("a session")["via"], "nexus");
    assert!(flag.asked("GET", "/v1/agents/access").is_none());

    let overridden = run(
        &["claude", "-p", "hi"],
        &[],
        Setup {
            access: Some(access("gateway", "organization")),
            ..Setup::default()
        },
    );
    assert_eq!(overridden.opened().expect("a session")["via"], "nexus");
    assert!(
        overridden.err.contains(
            "! Through Gateway: paid from your organization's budget. Your organization turned subscription mode off."
        ),
        "{}",
        overridden.err
    );
    assert_eq!(overridden.err.matches("Through Gateway: paid from").count(), 1);

    let unsaved = run(
        &["claude", "-p", "hi"],
        &[],
        Setup {
            access: Some(access("gateway", "default")),
            ..Setup::default()
        },
    );
    assert_eq!(unsaved.opened().expect("a session")["via"], "nexus");
    assert!(
        unsaved.err.contains(
            "! Through Gateway: paid from your organization's budget. Your organization turned subscription mode off."
        ),
        "{}",
        unsaved.err
    );

    let saved_gateway = run(
        &["codex", "exec", "hi"],
        &[],
        Setup {
            access: Some(codex_access("gateway", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(saved_gateway.opened().expect("a session")["via"], "nexus");
    assert!(saved_gateway.err.contains(
        "! Through Gateway: paid from your organization's budget. Your saved route is gateway; to use your ChatGPT plan: nexus agents use codex subscription."
    ), "{}", saved_gateway.err);

    assert!(!saved.err.contains("Through Gateway: paid from"), "{}", saved.err);
    assert!(!flag.err.contains("Through Gateway: paid from"), "{}", flag.err);

    let json = run(
        &["claude", "--json", "-p", "hi"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    let printed: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(
        printed["route"],
        json!({ "route": "subscription", "from": "preference" })
    );
    assert_eq!(printed["session"]["usage"]["estimatedCostUsd"], 0.0015);
}

#[test]
fn the_plan_route_checks_the_sign_in_first() {
    let signed_out = run(
        &["claude", "--subscription", "-p", "hi"],
        &[("STAND_IN_LOGIN", "signed_out")],
        Setup::default(),
    );
    assert_eq!(signed_out.code, 1);
    assert!(
        signed_out
            .err
            .contains("Claude Code is not signed in to your Claude plan on this machine."),
        "{}",
        signed_out.err
    );
    assert!(
        signed_out
            .err
            .contains("Run `claude auth login` once (choose your Claude account, not Console)")
    );
    assert!(signed_out.opened().is_none() && signed_out.agent.is_none());

    let api_key = run(
        &["codex", "--subscription", "exec", "hi"],
        &[("STAND_IN_LOGIN", "api_key")],
        Setup::default(),
    );
    // An API key would bill the API, not the plan: it stops too.
    assert_eq!(api_key.code, 1, "{}", api_key.err);
    assert!(api_key.err.contains(
        "Codex is signed in with an API key, which bills the API, not your plan: sign out (codex logout), then sign in with your ChatGPT account."
    ), "{}", api_key.err);
    assert!(api_key.opened().is_none() && api_key.agent.is_none());

    // An agent that does not say (an older version) is not stopped, but the run says it could not check.
    let unknown = run(
        &["claude", "--subscription", "-p", "hi"],
        &[("STAND_IN_LOGIN", "")],
        Setup::default(),
    );
    assert_eq!(unknown.code, 3, "{}", unknown.err);
    assert!(
        unknown.err.contains("! Could not check how Claude Code is signed in"),
        "{}",
        unknown.err
    );

    // On the plan: the line before the run names the account, so a wrong profile shows at once.
    let account = run(
        &["claude", "--subscription", "-p", "hi"],
        &[("STAND_IN_LOGIN", "account")],
        Setup::default(),
    );
    assert_eq!(account.code, 3, "{}", account.err);
    assert!(
        account
            .err
            .contains("claude on your Claude subscription (ada@example.com) · "),
        "{}",
        account.err
    );
}

#[test]
fn routes_are_checked_and_subscription_mode_turned_off_stops_before_the_agent_starts() {
    let both = run(
        &["claude", "--api", "--subscription", "-p", "hi"],
        &[],
        Setup::default(),
    );
    assert_eq!(both.code, 1);
    assert!(both.err.contains("Name one route for Claude Code"), "{}", both.err);
    let wrong = run(&["codex", "--via", "nope", "exec", "hi"], &[], Setup::default());
    assert!(wrong.err.contains("\"nope\" is not a route."), "{}", wrong.err);

    let refused = run(
        &["claude", "--subscription", "-p", "hi"],
        &[],
        Setup {
            refuse: true,
            ..Setup::default()
        },
    );
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("your organization has turned off subscription mode"),
        "{}",
        refused.err
    );
    assert!(refused.agent.is_none());
    assert!(!refused.seen.iter().any(|seen| seen.path.ends_with("/end")));

    // A `--json` after the route flags still asks for a JSON error: they are Nexus's own flags.
    let json = run(
        &["codex", "--via", "subscription", "--json", "exec", "hi"],
        &[],
        Setup {
            refuse: true,
            ..Setup::default()
        },
    );
    let error: Value = serde_json::from_str(&json.err).expect("a JSON error");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("subscription mode"))
    );
}

#[test]
fn the_cli_beats_while_the_agent_runs_and_stops_before_it_ends_the_session() {
    let ran = run(
        &["claude", "-p", "hi"],
        &[("STAND_IN_SLEEP", "0.5"), ("NEXUS_SESSION_HEARTBEAT_MS", "50")],
        Setup::default(),
    );
    assert_eq!(ran.code, 3, "{}", ran.err);
    let paths: Vec<&str> = ran.seen.iter().map(|seen| seen.path.as_str()).collect();
    let opened = paths.iter().position(|p| *p == "/v1/agents/sessions").expect("opened");
    let ended = paths.iter().position(|p| p.ends_with("/end")).expect("ended");
    let beats: Vec<usize> = (0..paths.len()).filter(|i| paths[*i].ends_with("/heartbeat")).collect();
    assert!(beats.len() >= 2, "{paths:?}");
    assert!(beats.iter().all(|i| *i > opened && *i < ended), "{paths:?}");
    assert_eq!(paths[beats[0]], "/v1/agents/sessions/ags_claude_subscription/heartbeat");
}

#[test]
fn a_subscription_session_with_nothing_reported_says_so() {
    let quiet = run(
        &["claude", "--subscription", "--team", "quiet", "-p", "hi"],
        &[],
        Setup::default(),
    );
    assert_eq!(quiet.code, 3, "{}", quiet.err);
    assert!(
        quiet
            .err
            .contains("session ags_claude_subscription_quiet · no usage reported yet"),
        "{}",
        quiet.err
    );
}

#[test]
fn nexus_agents_shows_each_route_and_saves_one() {
    let list = run(
        &["agents", "--json"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(list.code, 0, "{}", list.err);
    let printed: Value = serde_json::from_str(&list.out).expect("json");
    let claude = &printed["agents"][0];
    assert_eq!(printed["signedIn"], true);
    assert_eq!(claude["local"]["version"], "9.9.9");
    assert_eq!(claude["local"]["login"], "plan");
    assert_eq!(claude["local"]["plan"], "max");
    assert_eq!(claude["ready"], true);
    let keys: Vec<&String> = claude.as_object().expect("object").keys().collect();
    assert_eq!(keys[keys.len() - 3..], ["local", "steps", "ready"]);

    let human = run(&["agents"], &[("STAND_IN_LOGIN", "signed_out")], Setup::default());
    assert!(
        human.out.contains("This Gateway does not keep agent routes yet"),
        "{}",
        human.out
    );
    assert!(
        human
            .out
            .contains("Claude Code  ✓ installed 9.9.9 · ○ not signed in to your Claude plan"),
        "{}",
        human.out
    );
    assert!(
        human
            .out
            .contains("● Your Claude plan     calls straight to Anthropic, usage reported (the default)"),
        "{}",
        human.out
    );

    let saved = run(
        &["agents", "use", "claude", "subscription"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    assert_eq!(saved.code, 0, "{}", saved.err);
    assert_eq!(
        saved.asked("PUT", "/v1/agents/access"),
        Some(&json!({ "claude": "subscription" }))
    );
    assert!(
        saved
            .out
            .contains("✓ nexus claude now runs on your Claude plan when you name no route")
    );
    let reset = run(
        &["agents", "use", "claude", "default"],
        &[],
        Setup {
            access: Some(access("gateway", "default")),
            ..Setup::default()
        },
    );
    assert_eq!(
        reset.asked("PUT", "/v1/agents/access"),
        Some(&json!({ "claude": null }))
    );

    assert_eq!(run(&["agents", "use", "claude", "nope"], &[], Setup::default()).code, 2);
    let old = run(&["agents", "use", "codex", "subscription"], &[], Setup::default());
    assert!(old.err.contains("export NEXUS_CODEX_AUTH=subscription"), "{}", old.err);
    let signed_out = run(
        &["agents"],
        &[],
        Setup {
            signed_out: true,
            ..Setup::default()
        },
    );
    assert!(
        signed_out.out.contains("Signed out: this machine only."),
        "{}",
        signed_out.out
    );
    assert!(signed_out.seen.is_empty());

    let refused = run(
        &["agents", "--json"],
        &[],
        Setup {
            wrong_key: true,
            ..Setup::default()
        },
    );
    let printed: Value = serde_json::from_str(&refused.out).expect("json");
    assert_eq!(printed["signedIn"], false);
    assert_eq!(
        printed["note"],
        "Gateway does not accept your saved sign-in: run nexus auth login."
    );
    // As the TypeScript CLI's fetch says it.
    let away = run(
        &["agents"],
        &[],
        Setup {
            unreachable: true,
            ..Setup::default()
        },
    );
    assert!(
        away.out
            .contains("Could not reach Gateway (fetch failed): this machine only."),
        "{}",
        away.out
    );
}

// ------------------------------------------------------------------------------------------------ antigravity

/// What would make `agy` bill something other than the person's Google sign-in.
const GOOGLE_BILLING: [&str; 7] = [
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "AGY_ADC_AUTH",
    "GOOGLE_GEMINI_BASE_URL",
];

/// The session `/end` closed it with, by the agent's own conversation id.
fn closed_with(run: &Run) -> &Value {
    run.seen
        .iter()
        .find(|seen| seen.path.ends_with("/end"))
        .map(|seen| &seen.body)
        .expect("the session was closed")
}

#[test]
fn antigravity_runs_agy_on_its_plan_without_the_variables_that_bill_an_api_key() {
    let home = Home::new();
    let mut env: Vec<(&str, &str)> = GOOGLE_BILLING.iter().map(|name| (*name, "set")).collect();
    env.push(("HOME", "{home}"));
    let staff = Setup {
        staff: true,
        ..Setup::default()
    };
    let run = run_in(&home, &["agy", "-p", "hi"], &env, staff);
    assert_eq!(run.code, 3, "{}", run.err);
    let agent = run.agent.as_deref().expect("agy ran");
    for gone in GOOGLE_BILLING {
        assert_eq!(line(agent, gone), "", "{gone} reached agy");
    }
    // Nothing points it at Gateway, and Nexus gives it no key: the person's environment has none, so any would be ours.
    for gone in ["NEXUS_API_KEY", "NEXUS_URL", "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"] {
        assert_eq!(line(agent, gone), "", "{gone} reached agy");
    }
    assert!(!agent.contains(KEY), "the Nexus key reached agy: {agent}");
    // No instructions: agy has no flag that adds to them for one run. So no commit trailer either.
    assert_eq!(line(agent, "args"), "-p hi");
    assert_eq!(line(agent, "COMMIT_HOOK"), "");
    let opened = run.opened().expect("a session");
    assert_eq!(
        (&opened["agent"], &opened["via"], &opened["model"]),
        (&json!("antigravity"), &json!("subscription"), &Value::Null)
    );
    // No policy applies to it, and no skills are written for it: Gateway is asked for neither.
    for path in ["/v1/agents/check", "/v1/skills", "/v1/skill-library"] {
        assert!(
            !run.seen.iter().any(|seen| seen.path == path),
            "asked {path}: {:?}",
            requests(&run)
        );
    }
    // No conversation file changed, so the session closes without an id.
    assert_eq!(closed_with(&run)["agentSessionId"], Value::Null);
    assert!(
        run.err
            .contains("agy on your Google AI subscription · model calls go straight to Google"),
        "{}",
        run.err
    );
    // `agy -p /usage` answered with the plan's quota: signed in, so no warning.
    for warning in ["Sign-in not checked", "Could not check"] {
        assert!(!run.err.contains(warning), "{}", run.err);
    }
    assert!(
        run.err
            .contains("not metered or checked by Gateway; usage reported when agy exits)"),
        "{}",
        run.err
    );
    // Without its conversation, the run's usage cannot be read: the summary says only that.
    assert!(
        run.err.contains("session ags_antigravity_subscription · usage unknown"),
        "{}",
        run.err
    );
}

#[test]
fn antigravity_does_not_start_while_its_settings_select_an_api_key() {
    let home = Home::new();
    home.write(
        ".gemini/antigravity-cli/settings.json",
        br#"{"modelProvider":"gemini"}"#,
    );
    let run = run_in(
        &home,
        &["agy", "--subscription"],
        &[("HOME", "{home}")],
        Setup::default(),
    );
    assert_eq!(run.code, 1, "{}", run.err);
    assert!(run.agent.is_none(), "agy ran");
    assert!(run.opened().is_none());
    assert!(
        run.err.contains(
            "Antigravity is set to use an API key: ~/.gemini/antigravity-cli/settings.json sets \"modelProvider\"."
        ),
        "{}",
        run.err
    );
    // `nexus agents` shows the same state, with the step that undoes it.
    let list = run_in(&home, &["agents", "--json"], &[("HOME", "{home}")], Setup::default());
    let printed: Value = serde_json::from_str(&list.out).expect("json");
    let antigravity = printed["agents"]
        .as_array()
        .and_then(|agents| agents.iter().find(|entry| entry["agent"] == "antigravity"))
        .expect("antigravity");
    assert_eq!(antigravity["local"]["login"], "api_key");
    assert!(
        antigravity["steps"]
            .as_array()
            .is_some_and(|steps| steps.iter().any(|step| step["id"] == "plan_relogin")),
        "{antigravity}"
    );
}

#[test]
fn antigravity_starts_with_a_warning_while_agy_is_signed_out() {
    let home = Home::new();
    let started = std::time::Instant::now();
    let run = run_in(
        &home,
        &["agy", "-p", "hi"],
        &[("HOME", "{home}"), ("STAND_IN_SIGNED_OUT", "1")],
        Setup::default(),
    );
    // agy shows its own sign-in prompt, so the run goes ahead after one warning line.
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(run.agent.is_some(), "agy did not run");
    assert!(run.opened().is_some());
    assert!(
        run.err
            .contains("Antigravity isn't signed in to your Google AI plan here; agy will ask you to sign in."),
        "{}",
        run.err
    );
    assert!(!run.err.contains("Antigravity is not signed in"), "{}", run.err);
    // The check stops at agy's sign-in prompt: it does not wait out agy's own minute, or the check's deadline.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );

    // An agy older than its `/usage` check: not checked, and the run goes ahead with the warning.
    let old = run_in(
        &home,
        &["agy", "-p", "hi"],
        &[
            ("HOME", "{home}"),
            ("STAND_IN_SIGNED_OUT", "1"),
            ("STAND_IN_AGY_VERSION", "1.1.10"),
        ],
        Setup::default(),
    );
    assert_eq!(old.code, 3, "{}", old.err);
    assert!(
        old.err
            .contains("Sign-in not checked: Antigravity runs on the account it is signed in to."),
        "{}",
        old.err
    );
}

/// Gateway's answer with subscription mode off: the gateway route, from the person's saved subscription route
/// (`organization`) or with none saved (`default`).
fn subscriptions_off(from: &str) -> Value {
    let mut answer = nexus_client::agent_setup::offline_agent_access();
    answer["agentSubscriptions"] = json!(false);
    let antigravity = answer["agents"]
        .as_array_mut()
        .and_then(|agents| agents.iter_mut().find(|entry| entry["agent"] == "antigravity"))
        .expect("an antigravity entry");
    antigravity["route"] = json!("gateway");
    antigravity["from"] = json!(from);
    answer
}

#[test]
fn antigravity_with_subscription_mode_off_names_the_organizations_setting() {
    for from in ["organization", "default"] {
        let off = || Setup {
            access: Some(subscriptions_off(from)),
            ..Setup::default()
        };
        let refused = run(&["agy", "-p", "hi"], &[], off());
        assert_eq!(refused.code, 1, "{from}: {}", refused.err);
        assert!(
            refused
                .err
                .contains("Antigravity is not available: subscription mode is off for your organization."),
            "{from}: {}",
            refused.err
        );
        assert!(refused.agent.is_none() && refused.opened().is_none(), "{from}");
        let json = run(&["agy", "--json"], &[], off());
        let error: Value = serde_json::from_str(&json.err).expect("a JSON error");
        assert_eq!(error["error"]["code"], "subscriptions_disabled", "{from}");
    }
}

/// The command is the program's name, `agy`; the id names the agent everywhere else.
#[test]
fn agy_is_the_command_and_antigravity_the_id() {
    let saved = run(
        &["agents", "use", "agy", "subscription"],
        &[],
        Setup {
            access: Some(nexus_client::agent_setup::offline_agent_access()),
            ..Setup::default()
        },
    );
    assert_eq!(saved.code, 0, "{}", saved.err);
    assert_eq!(
        saved.asked("PUT", "/v1/agents/access"),
        Some(&json!({ "antigravity": "subscription" }))
    );
    assert!(
        saved
            .out
            .contains("✓ nexus agy now runs on your Google AI plan when you name no route"),
        "{}",
        saved.out
    );
    let by_id = run(
        &["agents", "use", "antigravity", "subscription"],
        &[],
        Setup {
            access: Some(nexus_client::agent_setup::offline_agent_access()),
            ..Setup::default()
        },
    );
    assert_eq!(
        by_id.asked("PUT", "/v1/agents/access"),
        Some(&json!({ "antigravity": "subscription" }))
    );
    // `nexus antigravity` is not a launcher: no session opens and no agent starts.
    let not_a_launcher = run(&["antigravity", "-p", "hi"], &[], Setup::default());
    assert_ne!(not_a_launcher.code, 0);
    assert!(not_a_launcher.agent.is_none() && not_a_launcher.opened().is_none());
}

#[test]
fn antigravity_refuses_the_gateway_route_and_an_auto_model_before_anything_starts() {
    for (argv, env) in [
        (&["agy", "--api", "-p", "hi"][..], &[][..]),
        (&["agy", "--via", "gateway"][..], &[][..]),
        (&["agy", "-p", "hi"][..], &[("NEXUS_ANTIGRAVITY_AUTH", "gateway")][..]),
    ] {
        let run = run(argv, env, Setup::default());
        assert_eq!(run.code, 2, "{argv:?}: {}", run.err);
        assert!(
            run.err
                .contains("Antigravity runs only on your Google AI plan: run `nexus agy --subscription`"),
            "{argv:?}: {}",
            run.err
        );
        assert!(run.agent.is_none() && run.opened().is_none(), "{argv:?}");
    }
    let json = run(&["agy", "--api", "--json"], &[], Setup::default());
    let error: Value = serde_json::from_str(&json.err).expect("a JSON error");
    assert_eq!(error["error"]["code"], "route_unsupported");

    for model in ["nexus/auto", "auto", "default"] {
        let run = run(&["agy", "--model", model, "-p", "hi"], &[], Setup::default());
        assert_eq!(run.code, 2, "{model}: {}", run.err);
        assert!(
            run.err
                .contains(&format!("--model {model} is not available for Antigravity.")),
            "{model}: {}",
            run.err
        );
        assert!(run.asked("POST", "/v1/agents/check").is_none(), "{model}");
        assert!(run.agent.is_none() && run.opened().is_none(), "{model}");
    }
    let recording = run(&["agy", "--recording"], &[], Setup::default());
    assert_eq!(recording.code, 2, "{}", recording.err);
    assert!(recording.agent.is_none());
}

#[test]
fn antigravity_passes_its_model_and_resume_arguments_through() {
    let named = run(
        &["agy", "--model", "gemini-3.8-flash-high", "-p", "hi"],
        &[],
        Setup::default(),
    );
    assert_eq!(named.code, 3, "{}", named.err);
    assert_eq!(
        line(named.agent.as_deref().expect("agy ran"), "args"),
        "--model gemini-3.8-flash-high -p hi"
    );
    assert_eq!(named.opened().expect("a session")["model"], "gemini-3.8-flash-high");
    assert!(named.asked("POST", "/v1/agents/check").is_none());

    let id = "7f2c1d9e-3b4a-4c5d-8e6f-0a1b2c3d4e5f";
    let resumed = run(&["agy", "--", "--conversation", id], &[], Setup::default());
    assert_eq!(resumed.code, 3, "{}", resumed.err);
    assert_eq!(
        line(resumed.agent.as_deref().expect("agy ran"), "args"),
        format!("--conversation {id}")
    );
    assert_eq!(closed_with(&resumed)["agentSessionId"], id);
}

#[test]
fn antigravity_conversation_is_the_one_file_the_run_wrote() {
    let home = Home::new();
    // Older than the run: never this run's conversation.
    home.write(
        ".gemini/antigravity-cli/conversations/11111111-1111-4111-8111-111111111111.db",
        b"",
    );
    // Older by more than the slack allowed for a file clock that lags the run's start.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let one = run_in(
        &home,
        &["agy"],
        &[("HOME", "{home}"), ("STAND_IN_CONVERSATIONS", "1")],
        Setup::default(),
    );
    assert_eq!(one.code, 3, "{}", one.err);
    assert_eq!(
        closed_with(&one)["agentSessionId"],
        "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c60"
    );
    // Two changed in one run: which is this run's cannot be told.
    let two = run_in(
        &Home::new(),
        &["agy"],
        &[("HOME", "{home}"), ("STAND_IN_CONVERSATIONS", "2")],
        Setup::default(),
    );
    assert_eq!(two.code, 3, "{}", two.err);
    assert_eq!(closed_with(&two)["agentSessionId"], Value::Null);
}

/// The conversation the stand-in `agy` writes when a test hands it a file.
const AGY_CONVERSATION: &str = "7c1e2f4a-93b5-4d06-8a1f-2b3c4d5e6f70";
const AGY_USAGE: &str = "/v1/agents/sessions/ags_antigravity_subscription/usage";

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn delimited(field: u64, bytes: &[u8], out: &mut Vec<u8>) {
    varint((field << 3) | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// A `gen_metadata` row's data as agy lays it out: the call's usage at 1.4 (output split into thinking 1.4.9 and
/// response 1.4.10), the same usage under its one attempt in retry_infos (1.17.2), and the call's text beside them.
fn agy_row(input: u64, output: u64) -> Vec<u8> {
    let mut usage = Vec::new();
    for (field, value) in [
        (1u64, 312),
        (2, input),
        (3, output),
        (9, output / 2),
        (10, output - output / 2),
    ] {
        varint(field << 3, &mut usage);
        varint(value, &mut usage);
    }
    let mut copy = Vec::new();
    delimited(2, &usage, &mut copy);
    let mut step = Vec::new();
    delimited(3, b"the model's answer", &mut step);
    delimited(4, &usage, &mut step);
    delimited(17, &copy, &mut step);
    let mut data = Vec::new();
    delimited(2, b"the person's prompt", &mut data);
    delimited(1, &step, &mut data);
    data
}

/// An agy conversation file at `path` with rows of (idx, input, output).
fn agy_conversation(path: &Path, rows: &[(i64, u64, u64)]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let db = rusqlite::Connection::open(path).expect("open");
    db.execute_batch("CREATE TABLE gen_metadata (idx INTEGER, data BLOB, size INTEGER)")
        .expect("table");
    for (idx, input, output) in rows {
        let data = agy_row(*input, *output);
        db.execute(
            "INSERT INTO gen_metadata (idx, data, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![idx, data, data.len() as i64],
        )
        .expect("row");
    }
}

/// The record `nexus agy` posts for one call.
fn agy_record(idx: i64, input: u64, output: u64) -> Value {
    let attribute = |key: &str, value: Value| json!({ "key": key, "value": value });
    json!({
        "body": { "stringValue": "api_request" },
        "attributes": [
            attribute("event.name", json!({ "stringValue": "api_request" })),
            attribute("request_id", json!({ "stringValue": format!("antigravity:{AGY_CONVERSATION}:{idx}") })),
            attribute("input_tokens", json!({ "intValue": input })),
            attribute("output_tokens", json!({ "intValue": output })),
            attribute("cache_read_tokens", json!({ "intValue": 0 })),
            attribute("cache_creation_tokens", json!({ "intValue": 0 })),
        ],
    })
}

fn agy_batch(records: Vec<Value>) -> Value {
    json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": records }] }] })
}

/// Each usage request's body, and whether every one came before `/end`.
fn usage_posts(run: &Run) -> (Vec<&Value>, bool) {
    let ended = run
        .seen
        .iter()
        .position(|seen| seen.path.ends_with("/end"))
        .expect("the session was closed");
    let posts: Vec<(usize, &Value)> = run
        .seen
        .iter()
        .enumerate()
        .filter(|(_, seen)| seen.method == "POST" && seen.path == AGY_USAGE)
        .map(|(at, seen)| (at, &seen.body))
        .collect();
    let before = posts.iter().all(|(at, _)| *at < ended);
    (posts.into_iter().map(|(_, body)| body).collect(), before)
}

/// The run's environment: its own home, and the stand-in `agy` copying `staged.db` in as the conversation.
const AGY_COPIES: &[(&str, &str)] = &[
    ("HOME", "{home}"),
    ("STAND_IN_COPY", "{home}/staged.db"),
    ("STAND_IN_COPY_ID", AGY_CONVERSATION),
];

#[test]
fn antigravity_reports_each_call_of_a_new_conversation_before_it_ends_the_session() {
    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &[(0, 8882, 96), (1, 9185, 104)]);
    let run = run_in(&home, &["agy", "-p", "hi"], AGY_COPIES, Setup::default());
    assert_eq!(run.code, 3, "{}", run.err);
    let (posts, before_end) = usage_posts(&run);
    assert_eq!(
        posts,
        [&agy_batch(vec![agy_record(0, 8882, 96), agy_record(1, 9185, 104)])],
        "{:?}",
        requests(&run)
    );
    assert!(before_end, "{:?}", requests(&run));
    assert_eq!(closed_with(&run)["agentSessionId"], AGY_CONVERSATION);
    // The summary shows what Gateway counted, as it does for Claude Code and Codex.
    assert!(
        run.err.contains(
            "session ags_antigravity_subscription · 2 calls · 18.1K → 200 tokens (0% cached) · 2 calls unpriced, on your subscription"
        ),
        "{}",
        run.err
    );
}

#[test]
fn antigravity_says_only_that_its_usage_is_unknown_when_a_new_row_does_not_decode() {
    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &[(0, 8882, 96)]);
    rusqlite::Connection::open(home.path("staged.db"))
        .expect("open")
        .execute("INSERT INTO gen_metadata (idx, data, size) VALUES (1, X'FFFF', 2)", [])
        .expect("row");
    let run = run_in(&home, &["agy", "-p", "hi"], AGY_COPIES, Setup::default());
    assert_eq!(run.code, 3, "{}", run.err);
    let (posts, _) = usage_posts(&run);
    assert!(posts.is_empty(), "{:?}", requests(&run));
    // Gateway's stand-in answers counts whatever was posted; the summary does not show them, nor why it is unknown.
    assert!(
        run.err.contains("session ags_antigravity_subscription · usage unknown"),
        "{}",
        run.err
    );
    assert!(!run.err.contains(" calls · "), "{}", run.err);
    assert!(!run.err.contains("decoded"), "{}", run.err);
}

#[test]
fn antigravity_reports_only_the_calls_a_resumed_conversation_added() {
    for args in [
        &["agy", "--", "--conversation", AGY_CONVERSATION][..],
        &["agy", "--", "--continue"][..],
    ] {
        let home = Home::new();
        let resumed = format!(".gemini/antigravity-cli/conversations/{AGY_CONVERSATION}.db");
        agy_conversation(&home.path(&resumed), &[(0, 8882, 96), (1, 9185, 104)]);
        agy_conversation(
            &home.path("staged.db"),
            &[(0, 8882, 96), (1, 9185, 104), (2, 9402, 131)],
        );
        let run = run_in(&home, args, AGY_COPIES, Setup::default());
        assert_eq!(run.code, 3, "{args:?}: {}", run.err);
        let (posts, before_end) = usage_posts(&run);
        assert_eq!(
            posts,
            [&agy_batch(vec![agy_record(2, 9402, 131)])],
            "{args:?}: {:?}",
            requests(&run)
        );
        assert!(before_end, "{args:?}");
        assert_eq!(closed_with(&run)["agentSessionId"], AGY_CONVERSATION, "{args:?}");
    }
}

#[test]
fn antigravity_reports_in_batches_of_200_and_gives_up_on_a_batch_that_keeps_failing() {
    let rows: Vec<(i64, u64, u64)> = (0..450).map(|idx| (idx, 1000 + idx as u64, 10)).collect();
    let records =
        |range: std::ops::Range<i64>| agy_batch(range.map(|idx| agy_record(idx, 1000 + idx as u64, 10)).collect());

    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &rows);
    let run = run_in(&home, &["agy", "-p", "hi"], AGY_COPIES, Setup::default());
    assert_eq!(run.code, 3, "{}", run.err);
    let (posts, before_end) = usage_posts(&run);
    assert_eq!(posts, [&records(0..200), &records(200..400), &records(400..450)]);
    assert!(before_end);

    // Gateway refuses: the client sends the first batch three times in all, and the run stops reporting there.
    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &rows);
    let refused = run_in(
        &home,
        &["agy", "-p", "hi"],
        AGY_COPIES,
        Setup {
            refuse_usage: true,
            ..Setup::default()
        },
    );
    assert_eq!(refused.code, 3, "{}", refused.err);
    let first = records(0..200);
    let (posts, before_end) = usage_posts(&refused);
    assert_eq!(posts, [&first, &first, &first], "{:?}", requests(&refused));
    assert!(before_end);
    assert_eq!(closed_with(&refused)["agentSessionId"], AGY_CONVERSATION);
    assert!(
        refused
            .err
            .contains("session ags_antigravity_subscription · usage unknown"),
        "{}",
        refused.err
    );
}

#[test]
fn antigravity_usage_is_unknown_when_its_last_batch_does_not_land() {
    let rows: Vec<(i64, u64, u64)> = (0..450).map(|idx| (idx, 1000 + idx as u64, 10)).collect();
    let records =
        |range: std::ops::Range<i64>| agy_batch(range.map(|idx| agy_record(idx, 1000 + idx as u64, 10)).collect());
    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &rows);
    let run = run_in(
        &home,
        &["agy", "-p", "hi"],
        AGY_COPIES,
        Setup {
            refuse_last_usage_batch: true,
            ..Setup::default()
        },
    );
    assert_eq!(run.code, 3, "{}", run.err);
    let (posts, before_end) = usage_posts(&run);
    let last = records(400..450);
    assert_eq!(
        posts,
        [&records(0..200), &records(200..400), &last, &last, &last],
        "{:?}",
        requests(&run)
    );
    assert!(before_end);
    // Gateway holds 400 of the run's 450 calls: the summary does not show them as the run's usage.
    assert!(
        run.err.contains("session ags_antigravity_subscription · usage unknown"),
        "{}",
        run.err
    );
    assert!(!run.err.contains(" calls · "), "{}", run.err);
}

/// agy updates itself, so `nexus agy` checks no version. The stand-in answers 1.6.0 to `--version`: a version check that
/// came back would see a version other than 1.5.7 here and post nothing.
#[test]
fn antigravity_reports_the_calls_of_any_agy_version() {
    let home = Home::new();
    agy_conversation(&home.path("staged.db"), &[(0, 8882, 96)]);
    let mut env = AGY_COPIES.to_vec();
    env.push(("STAND_IN_AGY_VERSION", "1.6.0"));
    let run = run_in(&home, &["agy", "-p", "hi"], &env, Setup::default());
    assert_eq!(run.code, 3, "{}", run.err);
    let (posts, before_end) = usage_posts(&run);
    assert_eq!(
        posts,
        [&agy_batch(vec![agy_record(0, 8882, 96)])],
        "{:?}",
        requests(&run)
    );
    assert!(before_end, "{:?}", requests(&run));
    assert_eq!(closed_with(&run)["agentSessionId"], AGY_CONVERSATION);
}

#[test]
fn nexus_agents_lists_antigravity_with_its_gateway_route_unavailable() {
    let list = run(&["agents", "--json"], &[], Setup::default());
    assert_eq!(list.code, 0, "{}", list.err);
    let printed: Value = serde_json::from_str(&list.out).expect("json");
    let ids: Vec<&str> = printed["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .filter_map(|entry| entry["agent"].as_str())
        .collect();
    assert_eq!(ids, ["claude", "codex", "antigravity", "cursor", "grok"]);
    let antigravity = &printed["agents"][2];
    assert_eq!(antigravity["routes"][0]["available"], false);
    assert!(antigravity["routes"][0].get("provider").is_none());
    assert_eq!(antigravity["route"], "subscription");
    assert_eq!(antigravity["local"]["installed"], true);
    assert_eq!(antigravity["local"]["version"], "1.5.7");
    // No HOME, so no settings file to read: the sign-in is not checked.
    assert!(antigravity["local"].get("login").is_none(), "{antigravity}");

    let human = run(&["agents"], &[], Setup::default());
    assert!(
        human
            .out
            .contains("Antigravity  ✓ installed 1.5.7 · sign-in not checked"),
        "{}",
        human.out
    );
    assert!(
        human
            .out
            .contains("○ Gateway              Antigravity runs only on your Google AI plan through Nexus for now"),
        "{}",
        human.out
    );
    assert!(
        human
            .out
            .contains("● Your Google AI plan  calls straight to Google, usage reported (the default)"),
        "{}",
        human.out
    );
}

#[test]
fn cursor_opens_a_personal_plan_session_and_strips_billing_overrides() {
    let home = Home::new();
    let bin = home.path("cursor-agent");
    let script = r#"#!/bin/sh
if [ "$1" = status ]; then
    printf '%s' '{"status":"authenticated"}'
    exit 0
fi
printf '%s\n' "$*" "${CURSOR_API_KEY-unset}" "${CURSOR_AUTH_TOKEN-unset}" "${CURSOR_AUTH_TOKEN_FILE-unset}" "${CURSOR_API_ENDPOINT-unset}" > "$AGENT_REPORT"
"#;
    std::fs::write(&bin, script).expect("Cursor stand-in");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let env = [
        ("NEXUS_CURSOR_BIN", bin.to_str().expect("path")),
        ("CURSOR_API_KEY", "own"),
        ("CURSOR_AUTH_TOKEN", "own"),
        ("CURSOR_AUTH_TOKEN_FILE", "own"),
        ("CURSOR_API_ENDPOINT", "own"),
    ];
    let refused = run_in(&home, &["cursor-agent", "--api"], &env, Setup::default());
    assert_eq!(refused.code, 2, "{}", refused.err);
    assert!(refused.agent.is_none() && refused.opened().is_none());

    let id = "7c1e2f4a-93b5-4d06-8a1f-2b3c4d5e6f70";
    let started = run_in(&home, &["cursor-agent", "--resume", id], &env, Setup::default());
    assert_eq!(started.code, 0, "{}", started.err);
    assert!(started.err.contains("usage unavailable"), "{}", started.err);
    let opened = started.opened().expect("Gateway session before launch");
    assert_eq!(
        (opened["agent"].as_str(), opened["via"].as_str()),
        (Some("cursor"), Some("subscription"))
    );
    assert_eq!(
        started.agent.as_deref(),
        Some(format!("--resume {id}\nunset\nunset\nunset\nunset\n").as_str())
    );
    assert_eq!(closed_with(&started)["agentSessionId"], id);
}

#[test]
fn antigravity_help_names_only_its_plan() {
    let help = run(&["agy", "--help"], &[], Setup::default());
    assert_eq!(help.code, 0, "{}", help.err);
    assert!(
        help.out
            .contains("nexus agy [--json] [--subscription] [--team <slug>] [--] [agy args…]"),
        "{}",
        help.out
    );
    assert!(
        help.out
            .contains("--api, --via gateway, --model auto and --recording are not available."),
        "{}",
        help.out
    );
    assert!(help.out.contains("nexus agy -- --conversation <id>"), "{}", help.out);
}

#[test]
fn help_lists_the_routes() {
    let claude = run(
        &["claude", "--help"],
        &[],
        Setup {
            access: Some(access("subscription", "preference")),
            ..Setup::default()
        },
    );
    assert!(claude.out.contains(
        "nexus claude [--json] [--subscription | --api | --via <route>] [--team <slug>] [--recording [<id>|latest]] [--no-skills] [--] [claude args…]"
    ));
    assert!(claude.out.contains(
        "  Route  your Claude plan (your saved route; change it: nexus agents use claude gateway|subscription)"
    ));
    let codex = run(&["codex", "--json", "--help"], &[], Setup::default());
    let printed: Value = serde_json::from_str(&codex.out).expect("json");
    assert!(printed.get("route").is_none());
    assert!(
        printed["usage"]
            .as_str()
            .is_some_and(|usage| usage.contains("[--subscription | --api | --via <route>]"))
    );
}

// ------------------------------------------------------------------------------------------------ skills at launch

const CHANGELOG: &str = "---\nname: changelog\ndescription: Write release notes.\n---\n\nGroup the changes by area.\n";
/// As a Windows editor leaves it: a byte-order mark, CRLF line ends and no final newline.
const WINDOWS_SKILL: &str =
    "\u{feff}---\r\nname: pdf-processing\r\ndescription: Extract text.\r\n---\r\n\r\nUse pdftotext.";
const CLAUDE: &[&str] = &["claude", "--subscription", "-p", "hi"];
const CLAUDE_HOME: &[(&str, &str)] = &[("CLAUDE_CONFIG_DIR", "{home}/claude")];

fn sha(text: &str) -> String {
    nexus_client::base::sha256_hex(&[text.as_bytes()])
}

/// Ada's two skills.
fn ada() -> Skills {
    Skills::of(
        "usr_ada",
        &[
            ("skl_changelog", "changelog", 2, CHANGELOG),
            ("skl_pdf", "pdf-processing", 1, WINDOWS_SKILL),
        ],
    )
}

fn with(skills: Skills) -> Setup {
    Setup {
        skills: Some(skills),
        ..Setup::default()
    }
}

/// A manifest as Nexus writes it, for a person and the folders it lists, each with the SKILL.md Nexus wrote.
fn manifest_of(user: &str, folders: &[(&str, &str)]) -> Vec<u8> {
    let skills: serde_json::Map<String, Value> = folders
        .iter()
        .map(|(name, file)| {
            (
                name.to_string(),
                json!({ "skillId": format!("skl_{name}"), "version": 1, "sha256": sha(file) }),
            )
        })
        .collect();
    json!({ "version": 1, "userId": user, "agentFolder": "/elsewhere", "skills": skills })
        .to_string()
        .into_bytes()
}

fn requests(run: &Run) -> Vec<String> {
    run.seen
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect()
}

fn closed_with_exit_code_1(run: &Run, session: &str) -> bool {
    run.asked("POST", &format!("/v1/agents/sessions/{session}/end"))
        .is_some_and(|body| body["exitCode"] == 1)
}

#[test]
fn skills_are_written_before_the_agent_starts_and_recorded_for_the_session() {
    let home = Home::new();
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err
            .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
        "{}",
        run.err
    );
    // The agent found both folders when it started.
    let agent = run.agent.as_deref().expect("the agent ran");
    assert_eq!(line(agent, "SKILLS_SEEN").trim(), "changelog pdf-processing");
    // The bytes are Gateway's, exactly: the byte-order mark, the CRLF line ends and the missing final newline stay.
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("file"),
        CHANGELOG.as_bytes()
    );
    assert_eq!(
        home.read("claude/skills/pdf-processing/SKILL.md").expect("file"),
        WINDOWS_SKILL.as_bytes()
    );
    // No partial file stays.
    let files: Vec<String> = home.files("claude/skills").into_keys().collect();
    assert_eq!(files, ["changelog/SKILL.md", "pdf-processing/SKILL.md"]);
    // One record for each, naming the session; Gateway reads the name and the SHA-256 itself.
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records"),
        &json!({
            "surface": "nexus_claude", "sessionId": "ags_claude_subscription",
            "skills": [{ "skillId": "skl_changelog", "version": 2 }, { "skillId": "skl_pdf", "version": 1 }],
        })
    );
    // The order: the session exists, then the skills, then the records, and the session ends last.
    let order = requests(&run);
    let at = |entry: &str| {
        order
            .iter()
            .position(|seen| seen == entry)
            .unwrap_or_else(|| panic!("{entry} in {order:?}"))
    };
    assert!(at("POST /v1/agents/sessions") < at("GET /v1/skills"));
    assert!(at("GET /v1/skills") < at("GET /v1/skills/skl_pdf/versions/1/export"));
    assert!(at("GET /v1/skills/skl_pdf/versions/1/export") < at("POST /v1/skill-loads"));
    assert!(at("POST /v1/skill-loads") < at("POST /v1/agents/sessions/ags_claude_subscription/end"));
    // The manifest lists what Nexus wrote, and the lock is gone.
    let state = home.state("claude");
    let manifest: Value =
        serde_json::from_slice(&home.read(&format!("{state}/usr_ada.json")).expect("manifest")).expect("json");
    assert_eq!(manifest["userId"], "usr_ada");
    assert_eq!(
        manifest["skills"]["changelog"],
        json!({ "skillId": "skl_changelog", "version": 2, "sha256": sha(CHANGELOG) })
    );
    assert_eq!(manifest["skills"]["pdf-processing"]["sha256"], sha(WINDOWS_SKILL));
    assert!(!home.exists(&format!("{state}/lock")));
}

#[test]
fn a_skill_that_is_current_is_not_fetched_again_but_the_session_still_records_it() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(second.code, 3, "{}", second.err);
    assert!(
        second.seen.iter().all(|seen| !seen.path.ends_with("/export")),
        "{:?}",
        requests(&second)
    );
    assert!(
        second
            .err
            .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
        "{}",
        second.err
    );
    assert_eq!(
        second.asked("POST", "/v1/skill-loads").expect("records")["sessionId"],
        "ags_claude_subscription"
    );
}

#[test]
fn a_new_version_replaces_the_file_and_an_edited_file_is_written_back() {
    let home = Home::new();
    let old = "---\nname: changelog\ndescription: Old.\n---\n";
    run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills::of("usr_ada", &[("skl_changelog", "changelog", 1, old)])),
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("v1"),
        old.as_bytes()
    );
    let newer = with(Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]));
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, newer.clone());
    assert!(
        second
            .asked("GET", "/v1/skills/skl_changelog/versions/2/export")
            .is_some()
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("v2"),
        CHANGELOG.as_bytes()
    );
    let state = home.state("claude");
    let manifest: Value =
        serde_json::from_slice(&home.read(&format!("{state}/usr_ada.json")).expect("manifest")).expect("json");
    assert_eq!(manifest["skills"]["changelog"]["version"], 2);
    // The folder is Nexus's: a file edited in it is written back, because it no longer is the version.
    home.write("claude/skills/changelog/SKILL.md", b"edited by hand");
    let third = run_in(&home, CLAUDE, CLAUDE_HOME, newer);
    assert!(
        third
            .asked("GET", "/v1/skills/skl_changelog/versions/2/export")
            .is_some()
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("restored"),
        CHANGELOG.as_bytes()
    );
}

#[test]
fn a_skill_the_account_no_longer_has_goes_with_its_folder_and_nothing_else_does() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    // A folder no manifest lists stays.
    home.write("claude/skills/hand-made/SKILL.md", b"mine");
    let only_changelog = with(Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]));
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, only_changelog);
    assert!(
        second
            .err
            .contains("Skills: 1 skill delivered: changelog v2 · removed: pdf-processing"),
        "{}",
        second.err
    );
    assert!(!home.exists("claude/skills/pdf-processing"));
    assert!(home.exists("claude/skills/changelog/SKILL.md"));
    assert_eq!(home.read("claude/skills/hand-made/SKILL.md").expect("kept"), b"mine");
    // No skills left: the last folder goes, and so does the manifest.
    let none = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert!(none.err.contains("Skills: removed: changelog"), "{}", none.err);
    assert!(!home.exists("claude/skills/changelog"));
    assert!(!home.exists(&format!("{}/usr_ada.json", home.state("claude"))));
    assert_eq!(home.read("claude/skills/hand-made/SKILL.md").expect("kept"), b"mine");
    // Nothing delivered and nothing listed: no question about who the person is for skills (the commit trailer's is
    // the one), and no line.
    let nothing = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    let asked = requests(&nothing);
    assert_eq!(
        asked.iter().filter(|r| *r == "GET /v1/tenants/me").count(),
        1,
        "{asked:?}"
    );
    assert!(!nothing.err.contains("Skills:"), "{}", nothing.err);
}

#[test]
fn a_folder_no_manifest_lists_is_never_written_and_its_skill_is_refused() {
    let home = Home::new();
    home.write("claude/skills/pdf-processing/SKILL.md", b"somebody's own file");
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains(&format!(
            "Skills: 1 skill delivered: changelog v2 · not delivered: pdf-processing (duplicate_name) · move or delete a folder in {} to receive its skill: pdf-processing",
            home.path("claude/skills").display()
        )),
        "{}",
        run.err
    );
    assert_eq!(
        home.read("claude/skills/pdf-processing/SKILL.md").expect("kept"),
        b"somebody's own file"
    );
    // The record names only what is in the folder.
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_changelog", "version": 2 }])
    );
    // The refused skill is never fetched.
    assert!(run.asked("GET", "/v1/skills/skl_pdf/versions/1/export").is_none());
}

#[test]
fn another_persons_skills_go_from_the_folder_before_this_persons_are_written() {
    let home = Home::new();
    let state = home.state("claude");
    // Bo launched here before. Bo and Ada both have a skill named changelog.
    home.write("claude/skills/changelog/SKILL.md", b"bo's changelog");
    home.write("claude/skills/bo-private/SKILL.md", b"bo only");
    home.write(
        &format!("{state}/usr_bo.json"),
        &manifest_of("usr_bo", &[("bo-private", "bo only"), ("changelog", "bo's changelog")]),
    );
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains("removed bo-private, changelog from another person on this folder (give each person their own CLAUDE_CONFIG_DIR)"),
        "{}",
        run.err
    );
    assert!(!home.exists("claude/skills/bo-private"));
    assert!(!home.exists(&format!("{state}/usr_bo.json")));
    // Ada's changelog is Ada's file.
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("ada's"),
        CHANGELOG.as_bytes()
    );
    let agent = run.agent.as_deref().expect("the agent ran");
    assert_eq!(line(agent, "SKILLS_SEEN").trim(), "changelog pdf-processing");
}

#[test]
fn a_link_where_a_listed_folder_was_is_left_alone_and_never_followed() {
    let home = Home::new();
    let state = home.state("claude");
    home.write("outside/keep.txt", b"not Nexus's");
    std::fs::create_dir_all(home.path("claude/skills")).expect("skills");
    std::os::unix::fs::symlink(home.path("outside"), home.path("claude/skills/linked")).expect("link");
    home.write(
        &format!("{state}/usr_ada.json"),
        &manifest_of("usr_ada", &[("linked", "x")]),
    );
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert!(
        run.err.contains("Skills: left in place: linked (not_a_folder)"),
        "{}",
        run.err
    );
    // The link and its target are the person's: neither is removed, and Nexus stops listing the name.
    assert!(
        std::fs::symlink_metadata(home.path("claude/skills/linked"))
            .expect("link kept")
            .file_type()
            .is_symlink()
    );
    assert_eq!(home.read("outside/keep.txt").expect("target kept"), b"not Nexus's");
    assert!(!home.exists(&format!("{state}/usr_ada.json")));
}

#[test]
fn a_link_where_a_listed_folder_was_is_never_written_through() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    // The person swaps a delivered folder for a link to a folder of their own, with their own SKILL.md in it.
    std::fs::remove_dir_all(home.path("claude/skills/changelog")).expect("folder");
    home.write("their-repo/changelog/SKILL.md", b"their own, not committed yet");
    std::os::unix::fs::symlink(home.path("their-repo/changelog"), home.path("claude/skills/changelog")).expect("link");
    let newer = "---\nname: changelog\ndescription: Newer.\n---\n";
    let run = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills::of(
            "usr_ada",
            &[
                ("skl_changelog", "changelog", 3, newer),
                ("skl_pdf", "pdf-processing", 1, WINDOWS_SKILL),
            ],
        )),
    );
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err
            .contains("Skills: 1 skill delivered: pdf-processing v1 · not delivered: changelog (duplicate_name)"),
        "{}",
        run.err
    );
    assert_eq!(
        home.read("their-repo/changelog/SKILL.md").expect("target kept"),
        b"their own, not committed yet"
    );
    assert!(
        std::fs::symlink_metadata(home.path("claude/skills/changelog"))
            .expect("link kept")
            .file_type()
            .is_symlink()
    );
    // The file was never fetched, and the manifest no longer lists a folder that is not Nexus's.
    assert!(run.asked("GET", "/v1/skills/skl_changelog/versions/3/export").is_none());
    let manifest: Value = serde_json::from_slice(
        &home
            .read(&format!("{}/usr_ada.json", home.state("claude")))
            .expect("manifest"),
    )
    .expect("json");
    assert!(manifest["skills"]["changelog"].is_null(), "{manifest}");
    assert!(manifest["skills"]["pdf-processing"].is_object());
}

#[test]
fn what_nexus_did_not_write_stays_when_the_skill_is_gone() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    // A file the person added to one delivered folder, and a SKILL.md they edited in the other.
    home.write("claude/skills/pdf-processing/helper.py", b"print('mine')");
    home.write("claude/skills/changelog/SKILL.md", b"edited by hand");
    let gone = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert!(
        gone.err
            .contains("Skills: left in place: changelog (changed), pdf-processing (has_other_files)"),
        "{}",
        gone.err
    );
    // The edited file is untouched. In the other folder Nexus took its own SKILL.md and left the person's file.
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("edited kept"),
        b"edited by hand"
    );
    assert!(!home.exists("claude/skills/pdf-processing/SKILL.md"));
    assert_eq!(
        home.read("claude/skills/pdf-processing/helper.py")
            .expect("helper kept"),
        b"print('mine')"
    );
    // Both folders are the person's now: Nexus stops listing them, and does not write over them when a skill returns.
    assert!(!home.exists(&format!("{}/usr_ada.json", home.state("claude"))));
    let back = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert!(
        back.err
            .contains("Skills: not delivered: changelog (duplicate_name), pdf-processing (duplicate_name)"),
        "{}",
        back.err
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("still kept"),
        b"edited by hand"
    );
}

#[test]
fn a_partial_file_from_a_launch_that_died_goes_with_the_folder() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    home.write("claude/skills/changelog/.SKILL.md.4242.tmp", b"half a fi");
    let gone = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert!(
        gone.err.contains("Skills: removed: changelog, pdf-processing"),
        "{}",
        gone.err
    );
    assert!(!home.exists("claude/skills/changelog"));
}

#[test]
fn removed_names_are_in_byte_order_whatever_they_look_like() {
    let home = Home::new();
    let numbers = with(Skills::of(
        "usr_ada",
        &[
            ("skl_nine", "9", 1, "nine"),
            ("skl_ten", "10", 1, "ten"),
            ("skl_abc", "abc", 1, "abc"),
        ],
    ));
    run_in(&home, CLAUDE, CLAUDE_HOME, numbers);
    let gone = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert!(gone.err.contains("Skills: removed: 10, 9, abc"), "{}", gone.err);
}

#[test]
fn a_list_that_is_not_the_documented_shape_stops_the_launch_and_removes_nothing() {
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    let state = home.state("claude");
    let before = home.read(&format!("{state}/usr_ada.json")).expect("manifest");
    // The first run's report of the stand-in agent: the runs below must write their own, or none.
    std::fs::remove_file(home.path("agent.txt")).expect("report");
    // An answer with no `skills` is not "the person has none".
    let bare = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills {
            list_body: Some(json!({})),
            ..ada()
        }),
    );
    assert_eq!(bare.code, 1, "{}", bare.err);
    assert!(bare.err.contains("does not understand"), "{}", bare.err);
    assert!(bare.err.contains("--no-skills"), "{}", bare.err);
    assert!(bare.agent.is_none(), "the agent must not start");
    assert!(closed_with_exit_code_1(&bare, "ags_claude_subscription"));
    // A listed skill with no latest version is not a skill to remove either.
    let mut headless = ada();
    headless.list[0]
        .as_object_mut()
        .expect("object")
        .remove("latestVersion");
    let broken = run_in(&home, CLAUDE, CLAUDE_HOME, with(headless));
    assert_eq!(broken.code, 1, "{}", broken.err);
    assert!(
        broken.err.contains("the skill changelog has no id or no latestVersion"),
        "{}",
        broken.err
    );
    // Nothing was removed, and the record did not change.
    assert!(home.exists("claude/skills/changelog/SKILL.md"));
    assert!(home.exists("claude/skills/pdf-processing/SKILL.md"));
    assert_eq!(home.read(&format!("{state}/usr_ada.json")).expect("manifest"), before);
}

#[test]
fn a_folder_is_listed_before_it_is_written_so_a_launch_that_stops_strands_none() {
    let home = Home::new();
    // A file where the skills folder should be: the write fails, after the record is saved.
    home.write("claude/skills", b"in the way");
    let stopped = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(stopped.code, 1, "{}", stopped.err);
    assert!(stopped.err.contains("delivery_failed"), "{}", stopped.err);
    let manifest: Value = serde_json::from_slice(
        &home
            .read(&format!("{}/usr_ada.json", home.state("claude")))
            .expect("manifest"),
    )
    .expect("json");
    assert_eq!(manifest["skills"]["changelog"]["sha256"], sha(CHANGELOG));
    // The next launch finishes what the stopped one began: the listed folder is written, not refused.
    std::fs::remove_file(home.path("claude/skills")).expect("clear");
    let next = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(next.code, 3, "{}", next.err);
    assert!(
        next.err
            .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
        "{}",
        next.err
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("file"),
        CHANGELOG.as_bytes()
    );
    // A listed folder that was never written goes quietly when its skill is gone.
    let home = Home::new();
    home.write("claude/skills", b"in the way");
    run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    std::fs::remove_file(home.path("claude/skills")).expect("clear");
    let gone = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &[])));
    assert_eq!(gone.code, 3, "{}", gone.err);
    assert!(!gone.err.contains("Skills:"), "{}", gone.err);
    assert!(!home.exists(&format!("{}/usr_ada.json", home.state("claude"))));
}

#[test]
fn a_record_of_another_version_or_person_stops_the_launch() {
    let home = Home::new();
    let state = home.state("claude");
    let other_version = json!({ "version": 2, "userId": "usr_ada", "agentFolder": "/x", "skills": {} });
    home.write(&format!("{state}/usr_ada.json"), other_version.to_string().as_bytes());
    let first = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(first.code, 1, "{}", first.err);
    assert!(first.err.contains("it is not a version 1 record"), "{}", first.err);
    assert!(first.err.contains("--no-skills"), "{}", first.err);
    // A record under Ada's name that names Bo is not Ada's.
    home.write(
        &format!("{state}/usr_ada.json"),
        &manifest_of("usr_bo", &[("changelog", "x")]),
    );
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(second.code, 1, "{}", second.err);
    assert!(second.err.contains("it names another person"), "{}", second.err);
    assert!(!home.exists("claude/skills"));
}

#[test]
fn a_launch_that_stops_part_way_leaves_a_manifest_that_knows_what_it_wrote() {
    let home = Home::new();
    // The second export answers 404, after the first skill is on disk.
    let mut half = ada();
    half.files.remove(&("skl_pdf".to_string(), 1));
    let first = run_in(&home, CLAUDE, CLAUDE_HOME, with(half));
    assert_eq!(first.code, 1, "{}", first.err);
    assert!(
        first.err.contains("Could not get your skills from Nexus Gateway"),
        "{}",
        first.err
    );
    assert!(first.agent.is_none(), "the agent must not start");
    assert!(home.exists("claude/skills/changelog/SKILL.md"));
    let state = home.state("claude");
    let manifest: Value =
        serde_json::from_slice(&home.read(&format!("{state}/usr_ada.json")).expect("manifest")).expect("json");
    assert_eq!(manifest["skills"].as_object().expect("skills").len(), 1);
    assert!(manifest["skills"]["changelog"].is_object());
    // A folder that could not be filled is not left behind.
    assert!(!home.exists("claude/skills/pdf-processing"));
    // The next run knows the folder it made: it is not taken for someone else's.
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(second.code, 3, "{}", second.err);
    assert!(
        second
            .err
            .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
        "{}",
        second.err
    );
}

#[test]
fn a_second_launch_waits_for_the_lock_then_stops_before_the_agent_and_closes_the_session() {
    let home = Home::new();
    let state = home.state("claude");
    // Held by a launch that is alive: this very process.
    home.write(&format!("{state}/lock"), held_by_this_process().as_bytes());
    let run = run_in(
        &home,
        CLAUDE,
        &[
            ("CLAUDE_CONFIG_DIR", "{home}/claude"),
            ("NEXUS_SKILLS_LOCK_WAIT_MS", "150"),
        ],
        with(ada()),
    );
    assert_eq!(run.code, 1, "{}", run.err);
    assert!(run.err.contains("(delivery_locked)"), "{}", run.err);
    assert!(run.err.contains("If none is running, delete"), "{}", run.err);
    assert!(run.agent.is_none(), "the agent must not start");
    assert!(
        closed_with_exit_code_1(&run, "ags_claude_subscription"),
        "{:?}",
        requests(&run)
    );
    assert!(!home.exists("claude/skills"));
    // Another launch's lock is not ours to remove.
    assert!(home.exists(&format!("{state}/lock")));
}

/// A lock file as a launch writes it: its process id, then its token.
fn held_by_this_process() -> String {
    format!("{}\nsomeone-else\n", std::process::id())
}

/// The id of a process that has ended.
fn ended_process() -> u32 {
    let mut child = std::process::Command::new("true").spawn().expect("true");
    let pid = child.id();
    child.wait().expect("wait");
    pid
}

#[test]
fn a_lock_nobody_touched_for_a_long_time_is_taken_over_even_when_its_launch_looks_alive() {
    let home = Home::new();
    let state = home.state("claude");
    // The process is alive (this one), but a request cannot take 330 seconds, so the lock is a dead launch's.
    home.write(&format!("{state}/lock"), held_by_this_process().as_bytes());
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(home.path(&format!("{state}/lock")))
        .expect("lock");
    lock.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(400))
        .expect("old");
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(ada()));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(run.err.contains("Skills: 2 skills delivered"), "{}", run.err);
    assert!(!home.exists(&format!("{state}/lock")));
    // A lock 200 seconds old is not stale yet.
    let home = Home::new();
    let state = home.state("claude");
    home.write(&format!("{state}/lock"), held_by_this_process().as_bytes());
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(home.path(&format!("{state}/lock")))
        .expect("lock");
    lock.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(200))
        .expect("old");
    let waiting = run_in(
        &home,
        CLAUDE,
        &[
            ("CLAUDE_CONFIG_DIR", "{home}/claude"),
            ("NEXUS_SKILLS_LOCK_WAIT_MS", "100"),
        ],
        with(ada()),
    );
    assert_eq!(waiting.code, 1, "{}", waiting.err);
    assert!(waiting.err.contains("(delivery_locked)"), "{}", waiting.err);
}

#[test]
fn a_lock_whose_launch_has_ended_is_taken_over_at_once() {
    let home = Home::new();
    let state = home.state("claude");
    // A launch that was interrupted: its lock is fresh, and its process is gone.
    home.write(
        &format!("{state}/lock"),
        format!("{}\ngone\n", ended_process()).as_bytes(),
    );
    let run = run_in(
        &home,
        CLAUDE,
        &[
            ("CLAUDE_CONFIG_DIR", "{home}/claude"),
            ("NEXUS_SKILLS_LOCK_WAIT_MS", "0"),
        ],
        with(ada()),
    );
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(run.err.contains("Skills: 2 skills delivered"), "{}", run.err);
    assert!(!home.exists(&format!("{state}/lock")));
}

#[test]
fn a_gateway_with_no_skills_and_a_key_with_no_person_start_the_agent_as_before() {
    let none = run(CLAUDE, &[], Setup::default());
    assert_eq!(none.code, 3, "{}", none.err);
    assert!(!none.err.contains("Skills:"), "{}", none.err);
    assert!(none.seen.iter().all(|seen| seen.path != "/v1/skill-loads"));
    // Only the commit trailer asks who the person is: someone outside Nexus gets no hook.
    assert_eq!(requests(&none).iter().filter(|r| *r == "GET /v1/tenants/me").count(), 1);
    let agent = none.agent.as_deref().expect("the agent ran");
    assert_eq!(line(agent, "COMMIT_HOOK"), "");
    assert!(!agent.contains("Nexus co-author"), "{agent}");
    let keyless = run(
        CLAUDE,
        &[],
        with(Skills {
            refuse: Some((403, "person_required")),
            ..Skills::default()
        }),
    );
    assert_eq!(keyless.code, 3, "{}", keyless.err);
    assert!(!keyless.err.contains("Skills:"), "{}", keyless.err);
    assert!(keyless.asked("POST", "/v1/skill-loads").is_none());
    // A service account may not use skills at all: Gateway refuses the route itself.
    let service = run(
        CLAUDE,
        &[],
        with(Skills {
            refuse: Some((403, "permission_denied")),
            ..Skills::default()
        }),
    );
    assert_eq!(service.code, 3, "{}", service.err);
    assert!(!service.err.contains("Skills:"), "{}", service.err);
    // With no skills there is nothing to write, so a run that names no home folder still starts.
    let empty = run(CLAUDE, &[], with(Skills::of("usr_ada", &[])));
    assert_eq!(empty.code, 3, "{}", empty.err);
    assert!(!empty.err.contains("Skills:"), "{}", empty.err);
}

#[test]
fn a_failure_to_get_or_record_the_skills_stops_the_launch_and_says_how_to_start_without_them() {
    let home = Home::new();
    let down = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills {
            refuse: Some((500, "internal_error")),
            ..Skills::default()
        }),
    );
    assert_eq!(down.code, 1, "{}", down.err);
    assert!(
        down.err
            .contains("Could not get your skills from Nexus Gateway: refused (internal_error)"),
        "{}",
        down.err
    );
    assert!(down.err.contains("--no-skills"), "{}", down.err);
    assert!(down.agent.is_none(), "the agent must not start");
    assert!(closed_with_exit_code_1(&down, "ags_claude_subscription"));
    // A record Gateway refuses stops it the same way.
    let home = Home::new();
    let refused = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills {
            refuse_records: Some((500, "invalid_load")),
            ..ada()
        }),
    );
    assert_eq!(refused.code, 1, "{}", refused.err);
    assert!(refused.err.contains("(invalid_load)"), "{}", refused.err);
    assert!(refused.agent.is_none(), "the agent must not start");
    assert!(closed_with_exit_code_1(&refused, "ags_claude_subscription"));
    // And `--no-skills` starts the agent anyway, asking Gateway nothing about skills.
    let home = Home::new();
    let without = run_in(
        &home,
        &["claude", "--subscription", "--no-skills", "-p", "hi"],
        CLAUDE_HOME,
        with(ada()),
    );
    assert_eq!(without.code, 3, "{}", without.err);
    assert!(!without.err.contains("Skills:"), "{}", without.err);
    assert!(
        without
            .seen
            .iter()
            .all(|seen| !seen.path.starts_with("/v1/skills") && seen.path != "/v1/skill-loads"),
        "{:?}",
        requests(&without)
    );
    assert!(!home.exists("claude/skills"));
}

#[test]
fn codex_gets_its_skills_in_its_own_home_and_the_summary_says_what_was_delivered() {
    let home = Home::new();
    let run = run_in(
        &home,
        &["codex", "--json", "--subscription", "exec", "hi"],
        &[("CODEX_HOME", "{home}/codex")],
        with(ada()),
    );
    assert_eq!(run.code, 3, "{}", run.err);
    assert_eq!(
        home.read("codex/skills/changelog/SKILL.md").expect("file"),
        CHANGELOG.as_bytes()
    );
    let agent = run.agent.as_deref().expect("the agent ran");
    assert_eq!(line(agent, "CODEX_SKILLS_SEEN").trim(), "changelog pdf-processing");
    assert_eq!(line(agent, "SKILLS_SEEN").trim(), "");
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records")["surface"],
        "nexus_codex"
    );
    let summary: Value = serde_json::from_str(&run.out).expect("json");
    let delivered: Vec<&str> = summary["skills"]["delivered"]
        .as_array()
        .expect("delivered")
        .iter()
        .filter_map(|skill| skill["name"].as_str())
        .collect();
    assert_eq!(delivered, ["changelog", "pdf-processing"]);
    assert_eq!(summary["skills"]["written"], json!(["changelog", "pdf-processing"]));
}

#[test]
fn a_name_that_is_not_a_skill_name_is_refused_and_nothing_is_written_outside_the_folder() {
    let home = Home::new();
    let run = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills::of("usr_ada", &[("skl_evil", "../evil", 1, "x")])),
    );
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(run.err.contains("not delivered: ../evil (invalid_name)"), "{}", run.err);
    assert!(!home.exists("claude/evil"));
    assert!(!home.exists("evil"));
    assert!(run.asked("POST", "/v1/skill-loads").is_none());
}

#[test]
fn without_a_home_folder_the_launch_says_where_it_looked() {
    let home = Home::new();
    let run = run_in(&home, CLAUDE, &[], with(ada()));
    assert_eq!(run.code, 1, "{}", run.err);
    assert!(
        run.err.contains("neither CLAUDE_CONFIG_DIR nor HOME is set"),
        "{}",
        run.err
    );
    assert!(run.agent.is_none(), "the agent must not start");
}

// ------------------------------------------------------------------------------------------ the library at launch

const DEPLOY: &str = "---\nname: deploy\ndescription: Ship a release.\n---\n\nRun the checks first.\n";
const HANDBOOK: &str = "---\nname: handbook\ndescription: How we work.\n---\n\nRead it twice.\n";
const REVIEW: &str = "---\nname: review\ndescription: Review a change.\n---\n\nRead the diff.\n";
/// Ada's own skill named like the library's `deploy`, with other bytes.
const MY_DEPLOY: &str = "---\nname: deploy\ndescription: Mine.\n---\n\nDeploy by hand.\n";

/// A person's record of what Nexus wrote in the Claude folder.
fn manifest_for(home: &Home, user: &str) -> Value {
    let path = format!("{}/{user}.json", home.state("claude"));
    serde_json::from_slice(&home.read(&path).expect("manifest")).expect("json")
}

/// What the stand-in Gateway was asked to export.
fn exports(run: &Run) -> Vec<String> {
    requests(run)
        .into_iter()
        .filter(|seen| seen.ends_with("/export"))
        .collect()
}

#[test]
fn library_skills_are_delivered_by_their_own_ids_and_reported_with_the_others() {
    let home = Home::new();
    let skills = ada().sharing(&[
        org_skill("skl_deploy", "deploy", 3, DEPLOY),
        team_skill("skl_review", "review", 1, REVIEW, "team_platform"),
    ]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
    assert_eq!(run.code, 3, "{}", run.err);
    // Once the library takes part, each skill names where it comes from.
    assert!(
        run.err.contains(
            "Skills: 4 skills delivered: changelog v2 (personal), deploy v3 (organization), pdf-processing v1 (personal), review v1 (team team_platform)"
        ),
        "{}",
        run.err
    );
    let agent = run.agent.as_deref().expect("the agent ran");
    assert_eq!(
        line(agent, "SKILLS_SEEN").trim(),
        "changelog deploy pdf-processing review"
    );
    // The bytes are the approved version's, and the export names the library skill's id and the approved version.
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("file"),
        DEPLOY.as_bytes()
    );
    assert_eq!(
        home.read("claude/skills/review/SKILL.md").expect("file"),
        REVIEW.as_bytes()
    );
    assert!(run.asked("GET", "/v1/skills/skl_deploy/versions/3/export").is_some());
    // The manifest keeps its version 1 format: a library skill is an entry like any other.
    let manifest = manifest_for(&home, "usr_ada");
    assert_eq!(manifest["version"], 1);
    assert_eq!(
        manifest["skills"]["deploy"],
        json!({ "skillId": "skl_deploy", "version": 3, "sha256": sha(DEPLOY) })
    );
    assert_eq!(
        manifest["skills"]["review"],
        json!({ "skillId": "skl_review", "version": 1, "sha256": sha(REVIEW) })
    );
    // The loads name library ids, with the approved versions.
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records"),
        &json!({
            "surface": "nexus_claude", "sessionId": "ags_claude_subscription",
            "skills": [
                { "skillId": "skl_changelog", "version": 2 }, { "skillId": "skl_deploy", "version": 3 },
                { "skillId": "skl_pdf", "version": 1 }, { "skillId": "skl_review", "version": 1 },
            ],
        })
    );
    // The order: the person's list, the library's, the files, then the records.
    let order = requests(&run);
    let at = |entry: &str| {
        order
            .iter()
            .position(|seen| seen == entry)
            .unwrap_or_else(|| panic!("{entry} in {order:?}"))
    };
    assert!(at("GET /v1/skills") < at("GET /v1/skill-library"));
    assert!(at("GET /v1/skill-library") < at("GET /v1/skills/skl_deploy/versions/3/export"));
    assert!(at("GET /v1/skills/skl_review/versions/1/export") < at("POST /v1/skill-loads"));
}

#[test]
fn a_person_with_no_skills_of_their_own_still_gets_the_library() {
    let home = Home::new();
    let skills = Skills::of("usr_ada", &[]).sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY)]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains("Skills: 1 skill delivered: deploy v3 (organization)"),
        "{}",
        run.err
    );
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("file"),
        DEPLOY.as_bytes()
    );
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_deploy", "version": 3 }])
    );
}

#[test]
fn a_name_gateway_marks_is_delivered_by_neither_skill_and_both_are_reported() {
    let home = Home::new();
    let skills = Skills::of(
        "usr_ada",
        &[
            ("skl_changelog", "changelog", 2, CHANGELOG),
            ("skl_mine", "deploy", 1, MY_DEPLOY),
        ],
    )
    .sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY).marked()]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains(
            "Skills: 1 skill delivered: changelog v2 (personal) · not delivered: deploy (duplicate_name: organization and personal)"
        ),
        "{}",
        run.err
    );
    assert!(!home.exists("claude/skills/deploy"));
    // Neither file is fetched, and neither is reported as loaded.
    assert_eq!(exports(&run), ["GET /v1/skills/skl_changelog/versions/2/export"]);
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_changelog", "version": 2 }])
    );
    assert!(manifest_for(&home, "usr_ada")["skills"]["deploy"].is_null());
}

#[test]
fn an_unmarked_library_skill_is_served_and_a_personal_skill_of_its_name_is_dropped_without_a_word() {
    let home = Home::new();
    // Gateway marks nothing: the person's `deploy` has the library's bytes, so the library copy serves.
    let skills = Skills::of("usr_ada", &[("skl_mine", "deploy", 1, DEPLOY)]).sharing(&[org_skill(
        "skl_deploy",
        "deploy",
        3,
        DEPLOY,
    )]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains("Skills: 1 skill delivered: deploy v3 (organization)"),
        "{}",
        run.err
    );
    assert!(!run.err.contains("not delivered"), "{}", run.err);
    assert_eq!(exports(&run), ["GET /v1/skills/skl_deploy/versions/3/export"]);
    assert_eq!(
        manifest_for(&home, "usr_ada")["skills"]["deploy"]["skillId"],
        "skl_deploy"
    );
    assert_eq!(
        run.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_deploy", "version": 3 }])
    );
    // The library copy goes (withdrawn): the person's own skill is delivered again, and the line is as it was.
    let back = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills::of("usr_ada", &[("skl_mine", "deploy", 1, DEPLOY)]).sharing(&[])),
    );
    assert!(
        back.err.contains("Skills: 1 skill delivered: deploy v1"),
        "{}",
        back.err
    );
    assert!(!back.err.contains("organization"), "{}", back.err);
    assert_eq!(
        manifest_for(&home, "usr_ada")["skills"]["deploy"]["skillId"],
        "skl_mine"
    );
}

#[test]
fn two_library_skills_of_one_name_are_delivered_as_neither() {
    let home = Home::new();
    // Sibling teams, both marked by Gateway.
    let marked = Skills::of("usr_ada", &[]).sharing(&[
        team_skill("skl_a", "review", 1, REVIEW, "team_a").marked(),
        team_skill("skl_b", "review", 2, REVIEW, "team_b").marked(),
        org_skill("skl_deploy", "deploy", 3, DEPLOY),
    ]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(marked));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains(
            "Skills: 1 skill delivered: deploy v3 (organization) · not delivered: review (duplicate_name: team team_a and team team_b)"
        ),
        "{}",
        run.err
    );
    assert!(!home.exists("claude/skills/review"));
    // Two library skills of a name are two, whatever the marker says: a Gateway that omits it is not trusted to.
    let home = Home::new();
    let unmarked = Skills::of("usr_ada", &[]).sharing(&[
        team_skill("skl_a", "review", 1, REVIEW, "team_a"),
        org_skill("skl_b", "review", 2, REVIEW),
    ]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(unmarked));
    assert!(
        run.err
            .contains("Skills: not delivered: review (duplicate_name: team team_a and organization)"),
        "{}",
        run.err
    );
    assert!(!home.exists("claude/skills/review"));
    assert!(exports(&run).is_empty(), "{:?}", requests(&run));
}

#[test]
fn a_name_that_starts_to_collide_takes_back_what_nexus_wrote_while_the_file_is_untouched() {
    let home = Home::new();
    let mine = Skills::of(
        "usr_ada",
        &[
            ("skl_mine", "deploy", 1, MY_DEPLOY),
            ("skl_pdf", "pdf-processing", 1, WINDOWS_SKILL),
        ],
    );
    run_in(&home, CLAUDE, CLAUDE_HOME, with(mine.clone()));
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("mine"),
        MY_DEPLOY.as_bytes()
    );
    // The organization approves a `deploy` with other bytes: the name collides, so Nexus takes back its file.
    let colliding = mine
        .clone()
        .sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY).marked()]);
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(colliding.clone()));
    assert!(
        second.err.contains(
            "Skills: 1 skill delivered: pdf-processing v1 (personal) · not delivered: deploy (duplicate_name: organization and personal) · removed: deploy"
        ),
        "{}",
        second.err
    );
    assert!(!home.exists("claude/skills/deploy"));
    // A file the person edited is theirs: it stays, and the name stays undelivered.
    let home = Home::new();
    run_in(&home, CLAUDE, CLAUDE_HOME, with(mine));
    home.write("claude/skills/deploy/SKILL.md", b"edited by hand");
    let kept = run_in(&home, CLAUDE, CLAUDE_HOME, with(colliding));
    assert!(kept.err.contains("left in place: deploy (changed)"), "{}", kept.err);
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("kept"),
        b"edited by hand"
    );
}

#[test]
fn a_library_skill_withdrawn_between_launches_goes_while_untouched_and_stays_when_edited() {
    let home = Home::new();
    let all = Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]).sharing(&[
        org_skill("skl_deploy", "deploy", 3, DEPLOY),
        org_skill("skl_handbook", "handbook", 1, HANDBOOK),
        team_skill("skl_review", "review", 1, REVIEW, "team_platform"),
    ]);
    let first = run_in(&home, CLAUDE, CLAUDE_HOME, with(all));
    assert!(first.err.contains("4 skills delivered"), "{}", first.err);
    home.write("claude/skills/handbook/SKILL.md", b"my notes on the handbook");
    // The approver withdrew all three: the library list is empty.
    let none = Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]).sharing(&[]);
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(none));
    assert_eq!(second.code, 3, "{}", second.err);
    assert!(
        second.err.contains(
            "Skills: 1 skill delivered: changelog v2 · removed: deploy, review · left in place: handbook (changed)"
        ),
        "{}",
        second.err
    );
    assert!(!home.exists("claude/skills/deploy"));
    assert!(!home.exists("claude/skills/review"));
    assert_eq!(
        home.read("claude/skills/handbook/SKILL.md").expect("kept"),
        b"my notes on the handbook"
    );
    // The edited folder is the person's now: Nexus no longer lists it.
    let manifest = manifest_for(&home, "usr_ada");
    assert_eq!(manifest["skills"].as_object().expect("skills").len(), 1);
    assert_eq!(
        second.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_changelog", "version": 2 }])
    );
}

#[test]
fn a_library_version_that_moved_is_replaced_and_one_that_did_not_is_not_fetched_again() {
    let home = Home::new();
    let at = |version: i64, file: &'static str| {
        with(Skills::of("usr_ada", &[]).sharing(&[org_skill("skl_deploy", "deploy", version, file)]))
    };
    let newer = "---\nname: deploy\ndescription: Ship a release.\n---\n\nRun the checks, then the smoke test.\n";
    run_in(&home, CLAUDE, CLAUDE_HOME, at(3, DEPLOY));
    let same = run_in(&home, CLAUDE, CLAUDE_HOME, at(3, DEPLOY));
    assert!(exports(&same).is_empty(), "{:?}", requests(&same));
    assert!(
        same.err.contains("Skills: 1 skill delivered: deploy v3 (organization)"),
        "{}",
        same.err
    );
    assert_eq!(
        same.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([{ "skillId": "skl_deploy", "version": 3 }])
    );
    // A newer version is approved.
    let moved = run_in(&home, CLAUDE, CLAUDE_HOME, at(4, newer));
    assert_eq!(exports(&moved), ["GET /v1/skills/skl_deploy/versions/4/export"]);
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("v4"),
        newer.as_bytes()
    );
    assert_eq!(manifest_for(&home, "usr_ada")["skills"]["deploy"]["version"], 4);
    // The latest version is withdrawn: the one before loads again.
    let back = run_in(&home, CLAUDE, CLAUDE_HOME, at(3, DEPLOY));
    assert_eq!(exports(&back), ["GET /v1/skills/skl_deploy/versions/3/export"]);
    assert_eq!(
        home.read("claude/skills/deploy/SKILL.md").expect("v3"),
        DEPLOY.as_bytes()
    );
}

#[test]
fn a_launch_under_another_organization_removes_the_first_ones_library_folders_and_writes_the_seconds() {
    let home = Home::new();
    // The manifest belongs to Ada, not to the organization she is signed in to.
    let acme = Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]).sharing(&[
        org_skill("skl_acme_deploy", "deploy", 3, DEPLOY),
        org_skill("skl_acme_handbook", "handbook", 1, HANDBOOK),
    ]);
    let first = run_in(&home, CLAUDE, CLAUDE_HOME, with(acme));
    assert!(first.err.contains("3 skills delivered"), "{}", first.err);
    let other_handbook = "---\nname: handbook\ndescription: How Globex works.\n---\n";
    let globex = Skills::of("usr_ada", &[("skl_changelog", "changelog", 2, CHANGELOG)]).sharing(&[
        org_skill("skl_globex_handbook", "handbook", 7, other_handbook),
        team_skill("skl_globex_runbook", "runbook", 1, REVIEW, "team_ops"),
    ]);
    let second = run_in(&home, CLAUDE, CLAUDE_HOME, with(globex));
    assert_eq!(second.code, 3, "{}", second.err);
    assert!(
        second.err.contains(
            "Skills: 3 skills delivered: changelog v2 (personal), handbook v7 (organization), runbook v1 (team team_ops) · removed: deploy"
        ),
        "{}",
        second.err
    );
    // The first organization's folder is gone. The name both have is the second's file, from the second's id.
    assert!(!home.exists("claude/skills/deploy"));
    assert_eq!(
        home.read("claude/skills/handbook/SKILL.md").expect("handbook"),
        other_handbook.as_bytes()
    );
    assert_eq!(
        exports(&second),
        [
            "GET /v1/skills/skl_globex_handbook/versions/7/export",
            "GET /v1/skills/skl_globex_runbook/versions/1/export",
        ]
    );
    let manifest = manifest_for(&home, "usr_ada");
    let names: Vec<&String> = manifest["skills"].as_object().expect("skills").keys().collect();
    assert_eq!(names, ["changelog", "handbook", "runbook"]);
    assert_eq!(manifest["skills"]["handbook"]["skillId"], "skl_globex_handbook");
    assert_eq!(
        second.asked("POST", "/v1/skill-loads").expect("records")["skills"],
        json!([
            { "skillId": "skl_changelog", "version": 2 }, { "skillId": "skl_globex_handbook", "version": 7 },
            { "skillId": "skl_globex_runbook", "version": 1 },
        ])
    );
}

#[test]
fn a_gateway_without_the_library_delivers_the_persons_own_skills() {
    for refusal in [
        None,
        Some((404, "not_found")),
        Some((403, "person_required")),
        Some((403, "permission_denied")),
    ] {
        let home = Home::new();
        let mut skills = ada();
        skills.library = refusal.map(|refuse| Library {
            refuse: Some(refuse),
            ..Library::default()
        });
        let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
        assert_eq!(run.code, 3, "{refusal:?}: {}", run.err);
        // Exactly the line of a launch before the library: no scope, nothing else.
        assert!(
            run.err
                .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
            "{refusal:?}: {}",
            run.err
        );
        assert!(!run.err.contains("personal"), "{refusal:?}: {}", run.err);
        assert!(
            run.asked("GET", "/v1/skill-library").is_some(),
            "the CLI asks: {refusal:?}"
        );
        assert_eq!(
            run.asked("POST", "/v1/skill-loads").expect("records")["skills"],
            json!([{ "skillId": "skl_changelog", "version": 2 }, { "skillId": "skl_pdf", "version": 1 }])
        );
    }
}

#[test]
fn any_other_answer_to_the_library_list_stops_the_launch_and_removes_nothing() {
    let home = Home::new();
    run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(ada().sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY)])),
    );
    let state = home.state("claude");
    let before = home.read(&format!("{state}/usr_ada.json")).expect("manifest");
    std::fs::remove_file(home.path("agent.txt")).expect("report");
    let gone = |run: &Run| {
        assert_eq!(run.code, 1, "{}", run.err);
        assert!(run.err.contains("--no-skills"), "{}", run.err);
        assert!(run.agent.is_none(), "the agent must not start");
        assert!(closed_with_exit_code_1(run, "ags_claude_subscription"));
        // The library skill is still there: a list nobody could read is not "no library".
        assert!(home.exists("claude/skills/deploy/SKILL.md"));
        assert!(home.exists("claude/skills/changelog/SKILL.md"));
        assert_eq!(home.read(&format!("{state}/usr_ada.json")).expect("manifest"), before);
        assert!(run.asked("POST", "/v1/skill-loads").is_none());
    };
    for (status, code) in [(500, "internal_error"), (401, "invalid_api_key"), (429, "rate_limited")] {
        let mut skills = ada();
        skills.library = Some(Library {
            refuse: Some((status, code)),
            ..Library::default()
        });
        let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
        assert!(
            run.err.contains("Could not get your skills from Nexus Gateway"),
            "{status}: {}",
            run.err
        );
        gone(&run);
    }
    // A list that is not the documented shape.
    let shapes = [
        json!({}),
        json!({ "skills": [{ "id": "skl_deploy", "name": "deploy", "version": 3, "scope": "org" }, { "name": "handbook" }] }),
        json!({ "skills": [{ "id": "skl_deploy", "name": "deploy", "version": 3, "scope": "everyone" }] }),
        json!({ "skills": [{ "id": "skl_deploy", "name": "deploy", "version": 3, "scope": "team" }] }),
        json!({ "skills": [{ "id": "skl_deploy", "name": "deploy", "version": 0, "scope": "org" }] }),
        json!({ "skills": [{ "id": "skl_deploy", "name": "deploy", "version": 3, "scope": "org", "duplicateName": "yes" }] }),
    ];
    for shape in shapes {
        let mut skills = ada();
        skills.library = Some(Library {
            list_body: Some(shape.clone()),
            ..Library::default()
        });
        let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
        assert!(run.err.contains("does not understand"), "{shape}: {}", run.err);
        assert!(run.err.contains("skill library"), "{shape}: {}", run.err);
        gone(&run);
    }
}

#[test]
fn a_skills_list_that_fails_stops_the_launch_whatever_the_library_says() {
    let home = Home::new();
    let down = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(
            Skills {
                refuse: Some((500, "internal_error")),
                ..Skills::default()
            }
            .sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY)]),
        ),
    );
    assert_eq!(down.code, 1, "{}", down.err);
    assert!(
        down.err
            .contains("Could not get your skills from Nexus Gateway: refused (internal_error)"),
        "{}",
        down.err
    );
    assert!(down.agent.is_none(), "the agent must not start");
    assert!(!home.exists("claude/skills"));
    // The library is not asked for when the person's list did not answer.
    assert!(down.asked("GET", "/v1/skill-library").is_none());
    // A key that may not hold skills reaches no library either.
    let keyless = run(
        CLAUDE,
        &[],
        with(
            Skills {
                refuse: Some((403, "person_required")),
                ..Skills::default()
            }
            .sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY)]),
        ),
    );
    assert_eq!(keyless.code, 3, "{}", keyless.err);
    assert!(!keyless.err.contains("Skills:"), "{}", keyless.err);
    assert!(keyless.asked("GET", "/v1/skill-library").is_none());
}

#[test]
fn a_record_from_before_the_library_still_loads_and_the_manifest_stays_version_1() {
    let home = Home::new();
    let state = home.state("claude");
    // As the launcher wrote it before the library existed.
    home.write("claude/skills/changelog/SKILL.md", CHANGELOG.as_bytes());
    home.write(
        &format!("{state}/usr_ada.json"),
        &manifest_of("usr_ada", &[("changelog", CHANGELOG), ("old-skill", "gone")]),
    );
    home.write("claude/skills/old-skill/SKILL.md", b"gone");
    let skills = Skills::of("usr_ada", &[("skl_changelog", "changelog", 1, CHANGELOG)]).sharing(&[org_skill(
        "skl_deploy",
        "deploy",
        3,
        DEPLOY,
    )]);
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(skills));
    assert_eq!(run.code, 3, "{}", run.err);
    assert!(
        run.err.contains(
            "Skills: 2 skills delivered: changelog v1 (personal), deploy v3 (organization) · removed: old-skill"
        ),
        "{}",
        run.err
    );
    // The listed folder that is current is not fetched again.
    assert_eq!(exports(&run), ["GET /v1/skills/skl_deploy/versions/3/export"]);
    let manifest = manifest_for(&home, "usr_ada");
    let sorted = |value: &Value| {
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(sorted(&manifest), ["agentFolder", "skills", "userId", "version"]);
    assert_eq!(manifest["version"], 1);
    for (_, entry) in manifest["skills"].as_object().expect("skills") {
        assert_eq!(sorted(entry), ["sha256", "skillId", "version"]);
    }
}

#[test]
fn the_summary_names_each_skills_scope_and_each_collision() {
    let home = Home::new();
    let skills = Skills::of(
        "usr_ada",
        &[
            ("skl_changelog", "changelog", 2, CHANGELOG),
            ("skl_mine", "handbook", 1, MY_DEPLOY),
        ],
    )
    .sharing(&[
        org_skill("skl_deploy", "deploy", 3, DEPLOY),
        org_skill("skl_handbook", "handbook", 2, HANDBOOK).marked(),
        team_skill("skl_review", "review", 1, REVIEW, "team_platform"),
    ]);
    let run = run_in(
        &home,
        &["codex", "--json", "--subscription", "exec", "hi"],
        &[("CODEX_HOME", "{home}/codex")],
        with(skills),
    );
    assert_eq!(run.code, 3, "{}", run.err);
    let summary: Value = serde_json::from_str(&run.out).expect("json");
    assert_eq!(
        summary["skills"]["delivered"],
        json!([
            { "name": "changelog", "skillId": "skl_changelog", "version": 2, "scope": "personal" },
            { "name": "deploy", "skillId": "skl_deploy", "version": 3, "scope": "org" },
            { "name": "review", "skillId": "skl_review", "version": 1, "scope": "team", "teamId": "team_platform" },
        ])
    );
    assert_eq!(
        summary["skills"]["refused"],
        json!([{ "name": "handbook", "code": "duplicate_name", "scopes": [{ "scope": "org" }, { "scope": "personal" }] }])
    );
}

#[test]
fn a_skill_that_returns_to_a_folder_nexus_left_is_refused_and_the_line_says_how_to_free_the_name() {
    let home = Home::new();
    let library = Skills::of("usr_ada", &[]).sharing(&[org_skill("skl_deploy", "deploy", 3, DEPLOY)]);
    run_in(&home, CLAUDE, CLAUDE_HOME, with(library.clone()));
    // A file the person's tools left in the delivered folder (Finder writes a .DS_Store).
    home.write("claude/skills/deploy/.DS_Store", b"x");
    // The person's own `deploy` has other bytes: the name collides, Nexus takes back its file, and the folder stays.
    let colliding = Skills::of("usr_ada", &[("skl_mine", "deploy", 2, MY_DEPLOY)]).sharing(&[org_skill(
        "skl_deploy",
        "deploy",
        3,
        DEPLOY,
    )
    .marked()]);
    let left = run_in(&home, CLAUDE, CLAUDE_HOME, with(colliding));
    assert!(
        left.err.contains("left in place: deploy (has_other_files)"),
        "{}",
        left.err
    );
    assert!(!home.exists("claude/skills/deploy/SKILL.md"));
    // The collision ends. The folder is no longer Nexus's, so the skill is refused, and the line says what frees the name.
    let back = run_in(&home, CLAUDE, CLAUDE_HOME, with(library.clone()));
    assert!(
        back.err.contains(&format!(
            "not delivered: deploy (duplicate_name) · move or delete a folder in {} to receive its skill: deploy",
            home.path("claude/skills").display()
        )),
        "{}",
        back.err
    );
    assert!(!home.exists("claude/skills/deploy/SKILL.md"));
    // The person clears the folder, and the next launch delivers the skill.
    std::fs::remove_dir_all(home.path("claude/skills/deploy")).expect("cleared");
    let delivered = run_in(&home, CLAUDE, CLAUDE_HOME, with(library));
    assert!(
        delivered
            .err
            .contains("Skills: 1 skill delivered: deploy v3 (organization)"),
        "{}",
        delivered.err
    );
    assert!(!delivered.err.contains("move or delete"), "{}", delivered.err);
}

#[test]
fn a_refused_report_names_the_report_and_says_to_run_again_when_a_skill_changed() {
    // A skill changed between the list and the report: an approver withdrew a library version.
    let home = Home::new();
    let changed = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills {
            refuse_records: Some((404, "skill_version_not_found")),
            ..ada()
        }),
    );
    assert_eq!(changed.code, 1, "{}", changed.err);
    assert!(
        changed
            .err
            .contains("Could not report the delivered skills to Nexus Gateway: refused (skill_version_not_found)"),
        "{}",
        changed.err
    );
    assert!(
        changed
            .err
            .contains("A skill changed while the launch ran. Run the command again."),
        "{}",
        changed.err
    );
    assert!(changed.err.contains("--no-skills"), "{}", changed.err);
    assert!(changed.agent.is_none(), "the agent must not start");
    assert!(closed_with_exit_code_1(&changed, "ags_claude_subscription"));
    // A 404 for the session is not a changed skill: the line says only how to start without skills.
    let home = Home::new();
    let session = run_in(
        &home,
        CLAUDE,
        CLAUDE_HOME,
        with(Skills {
            refuse_records: Some((404, "session_not_found")),
            ..ada()
        }),
    );
    assert_eq!(session.code, 1, "{}", session.err);
    assert!(
        session
            .err
            .contains("Could not report the delivered skills to Nexus Gateway: refused (session_not_found)"),
        "{}",
        session.err
    );
    assert!(!session.err.contains("Run the command again"), "{}", session.err);
    assert!(session.err.contains("--no-skills"), "{}", session.err);
}

#[test]
fn a_delivery_of_more_skills_than_one_call_takes_is_reported_in_several_calls() {
    let home = Home::new();
    // Gateway's `MAX_LOADS_A_CALL` is 900: 901 skills are two calls, 900 and 1, each skill once.
    let ids: Vec<String> = (0..901).map(|n| format!("skl_s{n:04}")).collect();
    let names: Vec<String> = (0..901).map(|n| format!("s-{n:04}")).collect();
    let list: Vec<(&str, &str, i64, &str)> = ids
        .iter()
        .zip(&names)
        .map(|(id, name)| (id.as_str(), name.as_str(), 1, CHANGELOG))
        .collect();
    let run = run_in(&home, CLAUDE, CLAUDE_HOME, with(Skills::of("usr_ada", &list)));
    assert_eq!(run.code, 3, "{}", run.err);
    let calls: Vec<&Seen> = run
        .seen
        .iter()
        .filter(|seen| seen.method == "POST" && seen.path == "/v1/skill-loads")
        .collect();
    let sizes: Vec<usize> = calls
        .iter()
        .map(|seen| seen.body["skills"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, [900, 1]);
    let reported: Vec<&str> = calls
        .iter()
        .flat_map(|seen| seen.body["skills"].as_array().expect("skills"))
        .filter_map(|skill| skill["skillId"].as_str())
        .collect();
    assert_eq!(reported, ids.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(
        calls
            .iter()
            .all(|seen| seen.body["sessionId"] == "ags_claude_subscription")
    );
}

// ---------------------------------------------------------------- apply (Nexus Review's fix hand-off, S4.3)

const APPLY: [&str; 9] = [
    "robustness",
    "apply",
    "review",
    "--repo",
    "acme/app",
    "--run",
    "run_1",
    "--finding",
    "NR-3f2a9c",
];

#[test]
fn apply_prints_the_prompt_alone_so_it_pipes_into_any_agent() {
    let run = run(&APPLY, &[], Setup::default());
    assert_eq!(run.code, 0, "{}", run.err);
    assert_eq!(run.out, APPLY_PROMPT);
    let sent = run.asked("POST", "/v1/robustness/review/apply").expect("asked");
    assert_eq!(
        sent,
        &json!({ "repo": "acme/app", "run": "run_1", "finding": "NR-3f2a9c" }),
        "--agent and --team never reach the module"
    );
    assert!(run.agent.is_none(), "no agent without --agent");
}

#[test]
fn apply_with_agent_starts_it_on_the_prompt() {
    let mut argv = APPLY.to_vec();
    argv.extend(["--agent", "claude", "--team", "core"]);
    let run = run(&argv, &[], Setup::default());
    assert_eq!(run.code, 3, "the agent's exit code passes through: {}", run.err);
    let agent = run.agent.as_deref().expect("the agent ran");
    assert!(agent.contains(APPLY_PROMPT), "{agent}");
    assert!(
        run.opened().is_some(),
        "a recorded agent session, as nexus claude opens one"
    );
}

#[test]
fn apply_names_a_moved_head_on_stderr() {
    let mut argv = APPLY.to_vec();
    argv[8] = "NR-0a0a0a";
    let run = run(&argv, &[], Setup::default());
    assert_eq!(run.code, 0, "{}", run.err);
    assert_eq!(run.out, APPLY_PROMPT);
    assert!(run.err.contains("moved past abc1234"), "{}", run.err);
}

#[test]
fn a_withheld_prompt_prints_the_bundle_and_starts_no_agent() {
    let mut argv = APPLY.to_vec();
    argv[8] = "NR-f0f0f0";
    let read = run(&argv, &[], Setup::default());
    assert_eq!(read.code, 0, "{}", read.err);
    assert!(read.out.contains("comes from a fork"), "{}", read.out);
    assert!(read.out.contains("Unwrap on input") && read.out.contains("It panics on empty input."));
    argv.extend(["--agent", "codex"]);
    let refused = run(&argv, &[], Setup::default());
    assert_eq!(refused.code, 1);
    assert!(refused.err.contains("No agent started"), "{}", refused.err);
    assert!(refused.agent.is_none());
    assert!(refused.opened().is_none());
}

#[test]
fn apply_refuses_an_unknown_agent_before_asking_the_module() {
    let mut argv = APPLY.to_vec();
    argv.extend(["--agent", "devin"]);
    let run = run(&argv, &[], Setup::default());
    assert_eq!(run.code, 2);
    assert!(run.err.contains("--agent must be claude or codex"), "{}", run.err);
    assert!(run.asked("POST", "/v1/robustness/review/apply").is_none());
}

#[test]
fn a_rate_limited_export_waits_the_retry_after_and_is_fetched_again() {
    let home = Home::new();
    // Two 429s with `Retry-After: 30`: the delivery waits twice and gets both skills. The test sleeps 1 ms a wait.
    let limited = Skills {
        limit: Some(RateLimit::new(2, "30")),
        ..ada()
    };
    let env = [CLAUDE_HOME[0], ("NEXUS_SKILLS_RATE_WAIT_MS", "1")];
    let run = run_in(&home, CLAUDE, &env, with(limited));
    assert_eq!(run.code, 3, "{}", run.err);
    assert_eq!(
        run.err.matches("rate limit makes the delivery wait").count(),
        1,
        "{}",
        run.err
    );
    assert!(
        run.err
            .contains("Skills: Nexus Gateway's rate limit makes the delivery wait 30 s."),
        "{}",
        run.err
    );
    assert!(
        run.err
            .contains("Skills: 2 skills delivered: changelog v2, pdf-processing v1"),
        "{}",
        run.err
    );
    // The same export, asked again after each 429.
    assert_eq!(
        exports(&run),
        [
            "GET /v1/skills/skl_changelog/versions/2/export",
            "GET /v1/skills/skl_changelog/versions/2/export",
            "GET /v1/skills/skl_changelog/versions/2/export",
            "GET /v1/skills/skl_pdf/versions/1/export",
        ]
    );
    assert_eq!(
        home.read("claude/skills/changelog/SKILL.md").expect("file"),
        CHANGELOG.as_bytes()
    );
    assert!(run.asked("POST", "/v1/skill-loads").is_some());
}

#[test]
fn a_rate_limit_past_the_bound_stops_the_launch_with_rate_limited() {
    // Waits of 60 s come to the three minutes a launch waits at most: the fourth 429 stops it.
    let home = Home::new();
    let env = [CLAUDE_HOME[0], ("NEXUS_SKILLS_RATE_WAIT_MS", "1")];
    let always = Skills {
        limit: Some(RateLimit::new(usize::MAX, "60")),
        ..ada()
    };
    let run = run_in(&home, CLAUDE, &env, with(always));
    assert_eq!(run.code, 1, "{}", run.err);
    assert!(
        run.err.contains(
            "Could not get your skills from Nexus Gateway: more than 120 requests a minute; slow down (rate_limited)"
        ),
        "{}",
        run.err
    );
    assert!(run.err.contains("--no-skills"), "{}", run.err);
    assert_eq!(exports(&run).len(), 4, "{:?}", exports(&run));
    assert!(run.agent.is_none(), "the agent must not start");
    assert!(run.asked("POST", "/v1/skill-loads").is_none());
    assert!(closed_with_exit_code_1(&run, "ags_claude_subscription"));
    assert!(!home.exists("claude/skills/changelog"));
    // A `Retry-After` longer than a minute is not waited at all.
    let home = Home::new();
    let long = Skills {
        limit: Some(RateLimit::new(1, "61")),
        ..ada()
    };
    let run = run_in(&home, CLAUDE, &env, with(long));
    assert_eq!(run.code, 1, "{}", run.err);
    assert!(run.err.contains("(rate_limited)"), "{}", run.err);
    assert!(!run.err.contains("makes the delivery wait"), "{}", run.err);
    assert_eq!(exports(&run).len(), 1, "{:?}", exports(&run));
}

/// The stand-in of the golden launches. It answers Claude Code's and Codex's sign-in checks and Codex's
/// catalog. A run writes its arguments and its starting environment (Linux's `/proc`) beside `$AGENT_REPORT`. The run
/// has no `PATH`, so `cat` is named whole.
const GOLDEN_STAND_IN: &str = r#"#!/bin/sh
if [ "$1 $2" = "auth status" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max"}'; exit 0; fi
if [ "$1 $2" = "login status" ]; then echo "Logged in using ChatGPT"; exit 0; fi
if [ "$*" = "debug models --bundled" ]; then echo '{"models":[{"slug":"gpt-6-luna","use_responses_lite":true}]}'; exit 0; fi
printf '%s\0' "$@" > "$AGENT_REPORT.args"
/bin/cat /proc/$$/environ > "$AGENT_REPORT.env"
exit 3
"#;

/// The golden stand-in `agy`: version 1.5.7, and its sign-in check prints agy 1.5.8's signed-in answer (`$USAGE`). A run
/// writes as [`GOLDEN_STAND_IN`]'s does. The check's `PATH` holds only Nexus's no-op launchers, so `cat` is named whole.
const GOLDEN_AGY_STAND_IN: &str = r#"[ "$1" = --version ] && echo 1.5.7 && exit 0
if [ "$1 $2" = "-p /usage" ]; then /bin/cat "$USAGE"; exit 0; fi
printf '%s\0' "$@" > "$AGENT_REPORT.args"
/bin/cat /proc/$$/environ > "$AGENT_REPORT.env"
exit 3
"#;

/// Everything the run printed to stderr, with its stand-in Gateway's URL as `{url}` and the session's duration as
/// `{took}`.
#[cfg(target_os = "linux")]
fn golden_err(run: &Run) -> String {
    run.err
        .replace(&run.url, "{url}")
        .lines()
        .map(|line| match line.split_once(" after ") {
            Some((head, rest)) => {
                let (_, tail) = rest.split_once(' ').unwrap_or((rest, ""));
                format!("{head} after {{took}} {tail}")
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(target_os = "linux")]
fn golden_stand_in() -> &'static PathBuf {
    &stand_ins().golden
}

#[cfg(target_os = "linux")]
fn golden_agy_stand_in() -> &'static PathBuf {
    &stand_ins().golden_agy
}

/// One golden launch by one of Nexus's own people, with `env` on top of [`run_in`]'s: the agent's arguments, its whole
/// environment, and the run.
#[cfg(target_os = "linux")]
fn golden_launch(home: &Home, argv: &[&str], env: &[(&str, &str)]) -> (Vec<String>, BTreeMap<String, String>, Run) {
    let bin = golden_stand_in().display().to_string();
    let agy = golden_agy_stand_in().display().to_string();
    let staff = Setup {
        staff: true,
        ..Setup::default()
    };
    let mut vars = vec![
        ("NEXUS_CLAUDE_BIN", bin.as_str()),
        ("NEXUS_CODEX_BIN", bin.as_str()),
        ("NEXUS_ANTIGRAVITY_BIN", agy.as_str()),
    ];
    vars.extend_from_slice(env);
    let run = run_in(home, argv, &vars, staff);
    assert_eq!(run.code, 3, "{}", run.err);
    let fields = |name: &str| -> Vec<String> {
        std::fs::read(home.path(name))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect()
    };
    let env = fields("agent.txt.env")
        .into_iter()
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    (fields("agent.txt.args"), env, run)
}

/// The variables every golden launch starts from (see [`run_in`]), as the agent gets them unless its launch changes
/// them.
#[cfg(target_os = "linux")]
fn golden_vars(home: &Home, extra: &[(&str, String)]) -> BTreeMap<String, String> {
    let golden = golden_stand_in().display().to_string();
    let mut vars: BTreeMap<String, String> = [
        ("AGENT_REPORT", home.path("agent.txt").display().to_string()),
        ("STAND_IN_LOGIN", "plan".to_string()),
        ("NEXUS_CLAUDE_BIN", golden.clone()),
        ("NEXUS_CODEX_BIN", golden),
        ("NEXUS_ANTIGRAVITY_BIN", golden_agy_stand_in().display().to_string()),
        (
            "GIT_CONFIG_PARAMETERS",
            nexus_client::commit_trailer::parameters(None, nexus_client::commit_trailer::Surface::Cli),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    vars.extend(extra.iter().map(|(name, value)| (name.to_string(), value.clone())));
    vars
}

/// `-c` and a TOML string, as Codex takes an override.
#[cfg(target_os = "linux")]
fn codex_setting(key: &str, value: &str) -> [String; 2] {
    ["-c".to_string(), format!("{key}={}", serde_json::Value::from(value))]
}

/// The connector, pages and commit-trailer instructions one of Nexus's own people gets.
#[cfg(target_os = "linux")]
fn golden_instructions() -> String {
    format!(
        "If this session exposes Nexus MCP tools: {} If those tools are absent, use the authenticated CLI: `nexus connectors --json` lists services and operation names. For calls, start `nexus mcp` and send newline-delimited JSON-RPC over stdin: initialize, notifications/initialized, then tools/call with the tool name and arguments. First call describeConnector for the service to inspect its operation schemas. Await each request response; notifications have no response. This uses the saved Nexus sign-in and the same Gateway permissions. Do not read credentials or change global configuration. Attaching Nexus MCP to the client is an optional setup improvement, not a prerequisite for this CLI fallback.\n\n{}\n\n{}",
        nexus_client::mcp::CONNECTOR_INSTRUCTIONS,
        nexus_cli::agents::PAGE_INSTRUCTIONS,
        nexus_client::commit_trailer::INSTRUCTIONS
    )
}

/// Claude Code's arguments for `-p hi` on either route: the new conversation's id (taken from `args`, after checking it
/// is one), the instructions, then the person's arguments.
#[cfg(target_os = "linux")]
fn golden_claude_args(args: &[String]) -> Vec<String> {
    let id = args.get(1).cloned().unwrap_or_default();
    assert!(nexus_cli::agents::is_uuid(&id), "{args:?}");
    vec![
        "--session-id".to_string(),
        id,
        "--append-system-prompt".to_string(),
        golden_instructions(),
        "-p".to_string(),
        "hi".to_string(),
    ]
}

/// What the person's shell sets (see [`run_in`]) that every agent's launch keeps.
#[cfg(target_os = "linux")]
const SHELL: [(&str, &str); 4] = [
    ("ANTHROPIC_API_KEY", "sk-ant-own"),
    ("ANTHROPIC_BASE_URL", "https://proxy.test"),
    ("ANTHROPIC_AUTH_TOKEN", "own-token"),
    ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
];

/// `nexus claude` through Gateway, whole: the session's conversation id, the instructions, the artifact switch,
/// the commit hook and Gateway's variables.
#[cfg(target_os = "linux")]
#[test]
fn golden_claude_code_through_gateway() {
    let home = Home::new();
    let (args, env, run) = golden_launch(&home, &["claude", "--api", "-p", "hi"], &[]);
    assert_eq!(args, golden_claude_args(&args));
    assert_eq!(
        env,
        golden_vars(
            &home,
            &[
                ("OPENAI_API_KEY", "sk-own".into()),
                ("OTEL_LOG_USER_PROMPTS", "1".into()),
                ("ANTHROPIC_BASE_URL", format!("{}/v1/anthropic", run.url)),
                ("ANTHROPIC_AUTH_TOKEN", KEY.into()),
                ("ANTHROPIC_CUSTOM_HEADERS", "x-nexus-session: ags_claude_nexus".into()),
                ("CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK", "1".into()),
                ("ANTHROPIC_MODEL", "claude-sonnet-5-5".into()),
                ("CLAUDE_CODE_DISABLE_ARTIFACT", "1".into()),
            ]
        )
    );
    assert_eq!(
        golden_err(&run),
        [
            "› Nexus Gateway · session ags_claude_nexus · claude on claude-sonnet-5-5 · every model call through {url} (reserved, metered and checked by Gateway) · paid with your organization's Anthropic key (BYOK)",
            "✕ claude exited with code 3 after {took} · session ags_claude_nexus · paid with your organization's Anthropic key (BYOK) · spend: nexus spend",
            "  → {url}/app?next=%2Foverview%3Fsession%3Dags_claude_nexus#agent-sessions",
        ]
        .join("\n")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn golden_claude_code_on_its_plan() {
    let home = Home::new();
    let (args, env, run) = golden_launch(&home, &["claude", "--subscription", "-p", "hi"], &[]);
    assert_eq!(args, golden_claude_args(&args));
    assert_eq!(
        env,
        golden_vars(
            &home,
            &[
                ("OPENAI_API_KEY", "sk-own".into()),
                ("ANTHROPIC_MODEL", "claude-sonnet-5-5".into()),
                ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".into()),
                ("OTEL_LOGS_EXPORTER", "otlp".into()),
                ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json".into()),
                (
                    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                    format!("{}/v1/agents/sessions/ags_claude_subscription/usage", run.url)
                ),
                ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", format!("Authorization=Bearer {KEY}")),
                ("CLAUDE_CODE_DISABLE_ARTIFACT", "1".into()),
            ]
        )
    );
    assert_eq!(
        golden_err(&run),
        [
            "› Nexus Gateway · session ags_claude_subscription · claude on your Claude subscription · model calls go straight to Anthropic (not metered or checked by Gateway; Claude Code reports their usage)",
            "✕ claude exited with code 3 after {took} · session ags_claude_subscription · 2 calls · 14.1K → 5 tokens (99% cached) · claude-sonnet-5-5 · ~$0.0015 list, on your subscription",
            "  → {url}/app?next=%2Foverview%3Fsession%3Dags_claude_subscription#agent-sessions",
        ]
        .join("\n")
    );
}

/// `nexus codex` through Gateway, whole: the git setting first, then Nexus's provider, search off (the
/// stand-in Gateway has no organization settings) and the catalog Nexus wrote, the model, the instructions, and the
/// person's arguments last. No conversation id is passed.
#[cfg(target_os = "linux")]
#[test]
fn golden_codex_through_gateway() {
    let home = Home::new();
    let (args, env, run) = golden_launch(&home, &["codex", "--api", "exec", "hi"], &[]);
    let catalog = home.path("codex-models-ee1a29acdf50f1de.json");
    assert_eq!(
        std::fs::read_to_string(&catalog).ok().as_deref(),
        Some(r#"{"models":[{"slug":"gpt-6-luna","use_responses_lite":false}]}"#)
    );
    let parameters = nexus_client::commit_trailer::parameters(None, nexus_client::commit_trailer::Surface::Cli);
    let expected: Vec<String> = [
        codex_setting("shell_environment_policy.set.GIT_CONFIG_PARAMETERS", &parameters),
        codex_setting("model_provider", "nexus"),
        codex_setting("model_providers.nexus.name", "Nexus"),
        codex_setting("model_providers.nexus.base_url", &format!("{}/v1/openai/v1", run.url)),
        codex_setting("model_providers.nexus.env_key", "NEXUS_API_KEY"),
        codex_setting("model_providers.nexus.wire_api", "responses"),
        [
            "-c".to_string(),
            r#"model_providers.nexus.http_headers={ "x-nexus-session" = "ags_codex_nexus" }"#.to_string(),
        ],
        ["-c".to_string(), "features.shell_snapshot=false".to_string()],
        codex_setting("web_search", "disabled"),
        codex_setting("model_catalog_json", &catalog.display().to_string()),
        codex_setting("model", "gpt-6-luna"),
        codex_setting("developer_instructions", &golden_instructions()),
        ["exec".to_string(), "hi".to_string()],
    ]
    .concat();
    assert_eq!(args, expected);
    let mut shell: Vec<(&str, String)> = SHELL.iter().map(|(name, value)| (*name, value.to_string())).collect();
    shell.extend([
        ("OTEL_LOG_USER_PROMPTS", "1".to_string()),
        ("NEXUS_API_KEY", KEY.to_string()),
    ]);
    assert_eq!(env, golden_vars(&home, &shell));
    assert_eq!(
        golden_err(&run),
        [
            "› Nexus Gateway · session ags_codex_nexus · codex on gpt-6-luna · every model call through {url} (reserved, metered and checked by Gateway) · paid with your organization's OpenAI key (BYOK)",
            "✕ codex exited with code 3 after {took} · session ags_codex_nexus · paid with your organization's OpenAI key (BYOK) · spend: nexus spend",
            "  → {url}/app?next=%2Foverview%3Fsession%3Dags_codex_nexus#agent-sessions",
        ]
        .join("\n")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn golden_codex_on_its_plan() {
    let home = Home::new();
    let (args, env, run) = golden_launch(&home, &["codex", "--subscription", "exec", "hi"], &[]);
    let parameters = nexus_client::commit_trailer::parameters(None, nexus_client::commit_trailer::Surface::Cli);
    let expected: Vec<String> = [
        codex_setting("shell_environment_policy.set.GIT_CONFIG_PARAMETERS", &parameters),
        codex_setting("forced_login_method", "chatgpt"),
        ["-c".to_string(), "otel.log_user_prompt=false".to_string()],
        [
            "-c".to_string(),
            format!(
                r#"otel.exporter={{ otlp-http = {{ endpoint = "{}/v1/agents/sessions/ags_codex_subscription/usage", protocol = "json" }} }}"#,
                run.url
            ),
        ],
        ["-c".to_string(), "features.shell_snapshot=false".to_string()],
        codex_setting("model", "gpt-6-luna"),
        codex_setting("developer_instructions", &golden_instructions()),
        ["exec".to_string(), "hi".to_string()],
    ]
    .concat();
    assert_eq!(args, expected);
    // No catalog on the plan: Codex keeps its own provider.
    assert!(!home.exists("codex-models-ee1a29acdf50f1de.json"));
    let mut shell: Vec<(&str, String)> = SHELL.iter().map(|(name, value)| (*name, value.to_string())).collect();
    shell.extend([
        ("OTEL_LOG_USER_PROMPTS", "1".to_string()),
        ("NEXUS_API_KEY", KEY.to_string()),
        ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", format!("authorization=Bearer {KEY}")),
    ]);
    assert_eq!(env, golden_vars(&home, &shell));
    assert_eq!(
        golden_err(&run),
        [
            "› Nexus Gateway · session ags_codex_subscription · codex on your ChatGPT subscription · model calls go straight to OpenAI (not metered or checked by Gateway; Codex reports their usage)",
            "✕ codex exited with code 3 after {took} · session ags_codex_subscription · 2 calls · 14.1K → 5 tokens (0% cached) · gpt-6-luna · ~$0.03 list, 1 calls unpriced, on your subscription",
            "  → {url}/app?next=%2Foverview%3Fsession%3Dags_codex_subscription#agent-sessions",
        ]
        .join("\n")
    );
}

/// `nexus agy` on its plan, whole, for one of Nexus's own people: the person's arguments as given, with no
/// conversation id, instructions or commit hook. Its environment loses what would bill Google elsewhere and the wrapper's
/// Gateway; Nexus adds no key, no usage export and no artifact switch.
#[cfg(target_os = "linux")]
#[test]
fn golden_antigravity_on_its_plan() {
    let home = Home::new();
    let home_dir = home.dir.display().to_string();
    let mut env: Vec<(&str, &str)> = GOOGLE_BILLING.iter().map(|name| (*name, "set")).collect();
    env.push(("HOME", &home_dir));
    let (args, env, run) = golden_launch(&home, &["agy", "--model", "gemini-3.8-flash-high", "-p", "hi"], &env);
    assert_eq!(args, ["--model", "gemini-3.8-flash-high", "-p", "hi"]);
    let mut shell: Vec<(&str, String)> = SHELL.iter().map(|(name, value)| (*name, value.to_string())).collect();
    shell.extend([
        ("OPENAI_API_KEY", "sk-own".to_string()),
        ("OTEL_LOG_USER_PROMPTS", "1".to_string()),
        ("HOME", home_dir.clone()),
    ]);
    let mut expected = golden_vars(&home, &shell);
    expected.remove("GIT_CONFIG_PARAMETERS");
    assert_eq!(env, expected);
    // Signed in to the plan, so no warning; the session records the model the person named.
    assert_eq!(
        golden_err(&run),
        [
            "› Nexus Gateway · session ags_antigravity_subscription · agy on your Google AI subscription · model calls go straight to Google (not metered or checked by Gateway; usage reported when agy exits)",
            "✕ agy exited with code 3 after {took} · session ags_antigravity_subscription · usage unknown",
            "  → {url}/app?next=%2Foverview%3Fsession%3Dags_antigravity_subscription#agent-sessions",
        ]
        .join("\n")
    );
    let opened = run.opened().expect("a session");
    assert_eq!(
        (&opened["agent"], &opened["via"], &opened["model"]),
        (
            &json!("antigravity"),
            &json!("subscription"),
            &json!("gemini-3.8-flash-high")
        )
    );
}

#[cfg(feature = "staff")]
#[test]
fn nexus_triage_writes_the_playbook_and_the_context_then_starts_the_agent_on_them() {
    let home = Home::new();
    let staff = Setup {
        staff: true,
        ..Setup::default()
    };
    let run = run_in(&home, &["triage", "codex"], &[("HOME", "{home}")], staff);
    assert_eq!(run.code, 3, "{}", run.err);
    let triage = home.dir.join("triage");
    let made: Vec<_> = std::fs::read_dir(&triage)
        .expect("the triage folder")
        .flatten()
        .collect();
    assert_eq!(made.len(), 1, "one folder for the run");
    let folder = made[0].path();
    assert_eq!(
        std::fs::read_to_string(folder.join("playbook.md")).expect("playbook"),
        nexus_cli::triage::PLAYBOOK
    );
    let context = std::fs::read_to_string(folder.join("context.md")).expect("context");
    assert!(context.contains(&format!("- Gateway: {}", run.url)), "{context}");
    assert!(context.contains("- Working folder: ~"), "{context}");
    assert!(
        !context.contains(&home.dir.display().to_string()),
        "the home folder is written as ~: {context}"
    );
    let agent = run.agent.as_deref().expect("the agent ran");
    let args = line(agent, "args");
    assert!(
        args.contains(&folder.join("playbook.md").display().to_string()),
        "{args}"
    );
    assert!(
        args.contains(&folder.join("context.md").display().to_string()),
        "{args}"
    );
}

#[cfg(feature = "staff")]
#[test]
fn nexus_triage_is_for_nexus_staff_only() {
    let home = Home::new();
    let run = run_in(&home, &["triage"], &[], Setup::default());
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(
        run.err.contains("nexus triage is for Nexus's own people."),
        "{}",
        run.err
    );
    assert!(run.agent.is_none(), "no agent started");
    assert!(!home.dir.join("triage").exists());
}

#[test]
fn claude_model_auto_on_subscription_routes_through_agents_route_and_refuses_paid_fallback() {
    let init_models = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "models",
            "response": {
                "models": [
                    { "value": "opus", "displayName": "Opus 5.5", "resolvedModel": "claude-opus-5-5" }
                ]
            }
        }
    })
    .to_string();
    let catalog = vec![json!({
        "id": "anthropic/claude-opus-5.5",
        "provider_model": "claude-opus-5-5"
    })];
    let env = [("STAND_IN_LOGIN", "plan"), ("STAND_IN_MODELS", init_models.as_str())];

    // 1. Plan has room: runs the plan model without calling /v1/agents/check.
    let ok_setup = Setup {
        auto_route: Some(json!({
            "result": "use_plan",
            "model": "anthropic/claude-opus-5.5",
            "providerModel": "claude-opus-5-5"
        })),
        catalog_models: Some(catalog.clone()),
        ..Setup::default()
    };
    let ok_run = run(
        &[
            "claude",
            "--subscription",
            "--team",
            "eng",
            "--model",
            "auto",
            "-p",
            "hi",
        ],
        &env,
        ok_setup,
    );
    assert_eq!(ok_run.code, 3, "{}", ok_run.err);
    let routed = ok_run
        .asked("POST", "/v1/agents/route")
        .expect("called /v1/agents/route");
    assert_eq!(routed["agent"], "claude");
    assert_eq!(routed["team"], "eng");
    assert_eq!(routed["plans"][0]["models"][0]["id"], "claude-opus-5-5");
    assert!(ok_run.asked("POST", "/v1/agents/check").is_none(), "no fallback check");
    let agent = ok_run.agent.as_deref().expect("claude ran");
    assert!(agent.contains("--model claude-opus-5-5"), "{agent}");

    // 2. Plan is used up: refuses paid fallback on subscription with a clear CLI hint.
    let ask_setup = Setup {
        auto_route: Some(json!({
            "result": "ask_for_paid_fallback",
            "model": "anthropic/claude-sonnet-5.5",
            "providerModel": "claude-sonnet-5-5",
            "access": "byok"
        })),
        catalog_models: Some(catalog.clone()),
        ..Setup::default()
    };
    let refused = run(
        &["claude", "--subscription", "--model", "auto", "-p", "hi"],
        &env,
        ask_setup.clone(),
    );
    assert_ne!(refused.code, 0);
    assert!(refused.agent.is_none(), "must not start agent on paid fallback");
    assert!(
        refused.err.contains("Your Claude plan has no room left for this task")
            && refused.err.contains("--via gateway --model auto"),
        "{}",
        refused.err
    );

    // 3. Same answer on --via gateway runs the paid model directly.
    let gw_run = run(
        &["claude", "--via", "gateway", "--model", "auto", "-p", "hi"],
        &[],
        ask_setup.clone(),
    );
    assert_eq!(gw_run.code, 3, "{}", gw_run.err);
    let gw_routed = gw_run.asked("POST", "/v1/agents/route").expect("gateway routed");
    assert!(gw_routed.get("plans").is_none(), "gateway sends no plans");
    let gw_agent = gw_run.agent.as_deref().expect("claude ran on gateway");
    assert!(gw_agent.contains("--model claude-sonnet-5-5"), "{gw_agent}");

    // 4. Org disabled subscriptions: surfaces org refusal instead of blaming plan room.
    let disabled = run(
        &["claude", "--subscription", "--model", "auto", "-p", "hi"],
        &env,
        Setup {
            refuse: true,
            ..ask_setup
        },
    );
    assert_ne!(disabled.code, 0);
    assert!(disabled.agent.is_none());
    assert!(
        disabled
            .err
            .contains("does not allow sessions on a personal subscription"),
        "{}",
        disabled.err
    );
}

#[test]
fn codex_model_auto_on_gateway_routes_through_agents_route() {
    let setup = Setup {
        auto_route: Some(json!({
            "result": "ask_for_paid_fallback",
            "model": "openai/gpt-6.1-sol",
            "providerModel": "gpt-6.1-sol",
            "access": "nexus"
        })),
        ..Setup::default()
    };
    let res = run(&["codex", "--via", "gateway", "--model", "auto", "hi"], &[], setup);
    assert_eq!(res.code, 3, "{}", res.err);
    let routed = res.asked("POST", "/v1/agents/route").expect("codex routed");
    assert_eq!(routed["agent"], "codex");
    assert!(res.asked("POST", "/v1/agents/check").is_none());
    let agent = res.agent.as_deref().expect("codex ran");
    assert!(agent.contains("--model gpt-6.1-sol"), "{agent}");
}

#[test]
fn grok_checks_the_effective_config_before_launch_with_or_without_a_nexus_account() {
    let home = Home::new();
    let bin = home.path("grok");
    let config = home.path("requirements.toml");
    let script = r#"#!/bin/sh
case "$1" in
  --version) echo "grok 1.0.92";;
  inspect) printf '{"loginPolicy":{"apiKeyAuthDisabled":%s},"configSources":{"layers":[{"path":"%s"}]}}' "$GROK_DISABLE_API_KEY_AUTH" "$TEST_CONFIG";;
  *) printf '%s\n' "$*" "${XAI_API_KEY-unset}" "$GROK_DISABLE_API_KEY_AUTH" > "$AGENT_REPORT";;
esac
"#;
    std::fs::write(&bin, script).expect("write fake Grok executable");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).expect("make fake Grok executable");
    let env = [
        ("NEXUS_GROK_BIN", bin.to_str().expect("UTF-8 test executable path")),
        ("XAI_API_KEY", "test-secret"),
        ("TEST_CONFIG", config.to_str().expect("UTF-8 test configuration path")),
        ("NEXUS_BYO_PROVIDERS", "1"),
    ];
    let id = "7c1e2f4a-93b5-4d06-8a1f-2b3c4d5e6f70";
    for signed_out in [true, false] {
        let setup = Setup {
            signed_out,
            ..Setup::default()
        };
        for contents in [
            "[model.other]\napi_key='test-secret'",
            "[[version_overrides]]\n[version_overrides.model.other]\nenv_key='ANOTHER_KEY'",
            "[model.other]\nbase_url='https://other.invalid'",
            "[auth.oidc]\nissuer='fixture'\nclient_id='test'",
            "[grok_com_config.oauth2]\nissuer='fixture'",
            "[[experiments]]\nid='example'\n[experiments.arms.other.model.foo]\nauth_provider='corp'",
            "invalid toml containing test-secret",
        ] {
            std::fs::write(&config, contents).expect("write test Grok configuration");
            let run = run_in(&home, &["grok", "--subscription"], &env, setup.clone());
            assert_ne!(run.code, 0);
            assert!(run.opened().is_none());
            assert!(!run.err.contains("test-secret"));
        }
        std::fs::write(&config, "[mcp_servers.example.env]\napi_key='fixture'\nmodel='helper'\nbase_url='local'\n[model.grok-build]\ncontext_window=12000").expect("write personal Grok test configuration");
        for name in ["GROK_OIDC_ISSUER", "GROK_OIDC_CLIENT_ID", "GROK_AUTH_PATH"] {
            let mut competing = env.to_vec();
            competing.push((name, "test-secret"));
            let refused = run_in(&home, &["grok", "--subscription"], &competing, setup.clone());
            assert_ne!(refused.code, 0);
            assert!(refused.err.contains(name) && !refused.err.contains("test-secret"));
            assert!(refused.opened().is_none());
        }
        let prompt = run_in(
            &home,
            &[
                "grok",
                "--subscription",
                "--",
                "before",
                "-p=--leader",
                "after",
                "--resume",
            ],
            &env,
            setup.clone(),
        );
        assert_eq!(prompt.code, 0, "{}", prompt.err);
        assert_eq!(
            prompt.agent.as_deref(),
            Some("--no-leader --permission-mode default --single --resume -- before --leader after\nunset\ntrue\n")
        );
        let run = run_in(
            &home,
            &["grok", "--subscription", "--", "--resume", id, "-m=grok-build"],
            &env,
            setup,
        );
        assert_eq!(run.code, 0, "{}", run.err);
        assert_eq!(
            run.agent.as_deref(),
            Some(
                format!("--no-leader --permission-mode default --resume {id} --model grok-build\nunset\ntrue\n")
                    .as_str()
            )
        );
        if signed_out {
            assert!(run.opened().is_none());
        } else {
            let opened = run.opened().expect("Gateway recorded the signed-in session");
            assert_eq!(
                (&opened["agent"], &opened["via"]),
                (&json!("grok"), &json!("subscription"))
            );
            assert_eq!(opened["model"], "grok-build");
            assert_eq!(closed_with(&run)["agentSessionId"], id);
            assert!(
                run.seen
                    .iter()
                    .all(|request| !request.body.to_string().contains("test-secret"))
            );
        }
    }
    let denied = run_in(&home, &["grok", "--api"], &env, Setup::default());
    assert!(denied.code != 0 && denied.opened().is_none());
}
