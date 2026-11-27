//! boat's API in memory (feature `fake`), for this crate's tests and the `fake-boat` binary: the endpoints the app
//! uses, with boat's JSON shapes (from its OpenAPI) and its state changes, so the client and the link run against
//! something that behaves like boat without an account or a network.
//!
//! - A create starts `provisioning` and turns `ready` after [`Config::ready_after`]; a stop is `archiving`, then
//!   `archived` after [`Config::stop_after`]; a resume goes back through `provisioning` to `ready`.
//! - `/sshkey` appends the key to [`FakeSsh::authorized_keys`] and answers with [`FakeSsh::endpoint`] and
//!   [`FakeSsh::host_key`], so a real `sshd` on this computer (with that file as its `AuthorizedKeysFile`) is the
//!   machine: the whole path, keys and host-key pinning included, runs for real.
//! - `/commands` runs `sh -c` here, in [`Config::exec_cwd`].
//! - `/saved-snapshots` (create, get, list, delete) keeps named templates of a stopped machine, `saving` then `ready`
//!   after [`Config::ready_after`], and a create with `from` names one (an unknown name is boat's 404). `PUT /upload/..`
//!   takes the file that `/files/upload-url` promised, into [`Config::exec_cwd`].
//! - `/api-keys/scoped` mints a key that may only read and stop its one machine, as boat's scoped keys do.
//! - [`FakeBoat::fail_next`] makes the next matching call fail with a given boat error (a start limit, an expired
//!   disk), and [`FakeBoat::calls`] lists what was called, for the tests' assertions.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use nexus_workbench::process::async_command;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// A real SSH server standing in for the machines.
#[derive(Debug, Clone)]
pub struct FakeSsh {
    /// The `AuthorizedKeysFile` of the sshd; registered keys are appended to it.
    pub authorized_keys: PathBuf,
    /// Where the sshd listens, `host:port`: answered as `sshEndpoint`.
    pub endpoint: String,
    /// The sshd's host key, `ssh-ed25519 AAAA…`: answered as `hostKey`.
    pub host_key: String,
    /// The user to log in as (boat's is `user`; a local sshd's is whoever runs it).
    pub user: String,
}

/// How the fake behaves.
#[derive(Debug, Clone)]
pub struct Config {
    /// The API key it accepts (`Authorization: Bearer <key>`).
    pub key: String,
    pub ready_after: Duration,
    pub stop_after: Duration,
    /// A real sshd for `/sshkey`; None answers a made-up address and host key.
    pub ssh: Option<FakeSsh>,
    /// Where `/commands` runs; None: this process's folder.
    pub exec_cwd: Option<PathBuf>,
    /// Whether `/api-keys/scoped` may mint keys (boat refuses a `ci` key with 403).
    pub scoped_keys: bool,
    /// Extra environment for `/commands` (a test's `HOME` and `PATH`).
    pub exec_env: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            key: "boat_fake".into(),
            ready_after: Duration::from_millis(300),
            stop_after: Duration::from_millis(300),
            ssh: None,
            exec_cwd: None,
            scoped_keys: true,
            exec_env: Vec::new(),
        }
    }
}

/// One call the fake answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: String,
    /// The path after `/api/v1`.
    pub path: String,
    pub body: Value,
    pub idempotency_key: Option<String>,
    pub org: Option<String>,
    /// Whether it came with a minted stop key rather than the account key.
    pub stop_key: bool,
}

struct Sandbox {
    id: String,
    name: String,
    state: String,
    labels: Value,
    size: String,
    /// When `provisioning` turns `ready`, and `archiving` turns `archived`.
    due: Option<Instant>,
    stopped: bool,
    data_expired: bool,
    snapshotted: bool,
}

impl Sandbox {
    /// Moves on to the state its timer reached.
    fn advance(&mut self) {
        if let Some(due) = self.due
            && Instant::now() >= due
        {
            self.due = None;
            match self.state.as_str() {
                "provisioning" => {
                    self.state = "ready".into();
                    self.stopped = false;
                }
                "archiving" => {
                    self.state = "archived".into();
                    self.stopped = true;
                    self.snapshotted = true;
                }
                _ => {}
            }
        }
    }

    fn json(&self) -> Value {
        let up = matches!(self.state.as_str(), "ready" | "idle" | "running");
        let (vcpu, memory) = match self.size.as_str() {
            "small" => (2, 4),
            "large" => (8, 16),
            "xlarge" => (16, 32),
            _ => (4, 8),
        };
        json!({
            "id": self.id,
            "name": self.name,
            "state": self.state,
            "error": null,
            "dataExpiredAt": if self.data_expired { json!("2026-11-01T00:00:00Z") } else { Value::Null },
            "stoppedAt": if self.stopped { json!("2026-10-31T00:00:00Z") } else { Value::Null },
            "labels": self.labels,
            "health": "ok",
            "hydrated": true,
            "type": self.size,
            "vcpu": vcpu,
            "memoryGB": memory,
            "billingMultiplier": 1,
            "machineProvider": if up { json!("hetzner") } else { Value::Null },
            "url": null,
            "ip": if up { json!("127.0.0.1") } else { Value::Null },
            "sshEndpoint": null,
            "createdAt": "2026-10-31T00:00:00Z",
            "updatedAt": "2026-10-31T00:00:00Z",
            "archiveAfter": null,
            "ttlSeconds": null,
            "desktopAvailable": up,
            "desktopUrl": null,
            "snapshots": true,
            "monitoring": true,
            "snapshotAvailable": self.snapshotted,
            "snapshotCompletedAt": if self.snapshotted { json!("2026-10-31T00:00:00Z") } else { Value::Null },
            "access": "owner",
            "visibility": null,
            "environment": null,
            "stop": null,
        })
    }
}

/// A saved snapshot (boat's reusable template).
struct Saved {
    name: String,
    source: String,
    size: String,
    /// Order of creation; the timestamps answered are made from it.
    seq: u64,
    ready_at: Instant,
}

impl Saved {
    fn json(&self) -> Value {
        let ready = Instant::now() >= self.ready_at;
        json!({
            "name": self.name,
            "status": if ready { "ready" } else { "saving" },
            "snapshotId": format!("snap_{}", self.seq),
            "sizeBytes": if ready { json!(1024) } else { Value::Null },
            "sourceSandboxId": self.source,
            "type": self.size,
            "createdAt": format!("2026-11-04T00:00:{:02}Z", self.seq % 60),
            "seq": self.seq,
        })
    }
}

/// boat's rule for a saved snapshot's name.
fn valid_saved_name(name: &str) -> bool {
    let reserved = ["latest", "tree", "pull", "rm", "save", "current", "self", "new"];
    let mut chars = name.chars();
    name.len() <= 63
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !reserved.contains(&name)
}

struct Failure {
    method: String,
    path_suffix: String,
    status: u16,
    code: String,
    message: String,
    extra: Value,
}

struct Shared {
    config: Config,
    sandboxes: Mutex<BTreeMap<String, Sandbox>>,
    idempotency: Mutex<HashMap<String, String>>,
    stop_keys: Mutex<HashMap<String, String>>,
    saved: Mutex<BTreeMap<String, Saved>>,
    calls: Mutex<Vec<Call>>,
    failures: Mutex<Vec<Failure>>,
    next: AtomicU64,
}

/// A running fake. Dropping it stops the server.
pub struct FakeBoat {
    url: String,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeBoat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// boat's id alphabet (no 0, 1, i, l, o).
const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";

fn short(mut n: u64) -> String {
    let mut out = String::new();
    for _ in 0..8 {
        out.push(char::from(ALPHABET[(n % ALPHABET.len() as u64) as usize]));
        n /= ALPHABET.len() as u64;
    }
    out
}

/// boat's error envelope; `extra` also goes to the top level (boat puts `nextStartAt` there).
fn failure(status: u16, code: &str, message: &str, extra: &Value) -> (u16, Value) {
    let mut body = json!({
        "ok": false,
        "type": "sandbox.error",
        "status": status,
        "code": code,
        "message": message,
        "error": { "code": code, "message": message, "status": status, "details": extra },
        "requestId": "req_fake",
    });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            body.insert(k.clone(), v.clone());
        }
    }
    (status, body)
}

impl FakeBoat {
    /// Starts on a free port on 127.0.0.1.
    pub async fn start(config: Config) -> std::io::Result<FakeBoat> {
        FakeBoat::start_on(SocketAddr::from(([127, 0, 0, 1], 0)), config).await
    }

    /// Starts on `address`.
    pub async fn start_on(address: SocketAddr, config: Config) -> std::io::Result<FakeBoat> {
        let listener = TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            config,
            sandboxes: Mutex::new(BTreeMap::new()),
            idempotency: Mutex::new(HashMap::new()),
            stop_keys: Mutex::new(HashMap::new()),
            saved: Mutex::new(BTreeMap::new()),
            calls: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
        });
        let serving = shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let shared = serving.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let shared = shared.clone();
                        async move { Ok::<_, Infallible>(handle(&shared, request).await) }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(FakeBoat {
            url: format!("http://{address}/api/v1"),
            shared,
            task,
        })
    }

    /// The API's base URL, `http://127.0.0.1:<port>/api/v1`.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.shared.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// The next `method` call whose path ends with `path_suffix` fails with this boat error.
    pub fn fail_next(&self, method: &str, path_suffix: &str, status: u16, code: &str, message: &str, extra: Value) {
        if let Ok(mut failures) = self.shared.failures.lock() {
            failures.push(Failure {
                method: method.into(),
                path_suffix: path_suffix.into(),
                status,
                code: code.into(),
                message: message.into(),
                extra,
            });
        }
    }

    /// A machine created here directly, in `state` (`ready`, `archived`), for a test's starting point.
    pub fn insert(&self, state: &str) -> String {
        let n = self.shared.next.fetch_add(1, Ordering::SeqCst);
        let id = format!("bx_{}", short(n));
        if let Ok(mut sandboxes) = self.shared.sandboxes.lock() {
            sandboxes.insert(
                id.clone(),
                Sandbox {
                    id: id.clone(),
                    name: format!("test-{n}"),
                    state: state.into(),
                    labels: json!({}),
                    size: "large".into(),
                    due: None,
                    stopped: state == "archived",
                    data_expired: false,
                    snapshotted: state == "archived",
                },
            );
        }
        id
    }

    /// boat's state for `id` now (`ready`, `archived`), None once deleted.
    pub fn state(&self, id: &str) -> Option<String> {
        let mut sandboxes = self.shared.sandboxes.lock().ok()?;
        let sandbox = sandboxes.get_mut(id)?;
        sandbox.advance();
        Some(sandbox.state.clone())
    }

    /// Marks a sleeping machine's disk as expired (14 days asleep): waking it fails.
    pub fn expire(&self, id: &str) {
        if let Ok(mut sandboxes) = self.shared.sandboxes.lock()
            && let Some(sandbox) = sandboxes.get_mut(id)
        {
            sandbox.data_expired = true;
        }
    }
}

fn reply(status: u16, body: Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let Ok(value) = "application/json".parse() {
        response.headers_mut().insert(hyper::header::CONTENT_TYPE, value);
    }
    response
}

async fn handle(shared: &Shared, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let bearer = header("authorization").and_then(|a| a.strip_prefix("Bearer ").map(str::to_string));
    let idempotency_key = header("idempotency-key");
    let org = header("x-boat-org");
    let confirm = header("x-ascii-confirm-delete");
    let host = header("host").unwrap_or_else(|| "127.0.0.1".into());
    let query = request.uri().query().unwrap_or("").to_string();
    let body = match Limited::new(request.into_body(), 1 << 20).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return reply(413, failure(413, "body_too_large", "Too large.", &json!({})).1),
    };
    if method == "PUT" && path.starts_with("/upload/") {
        // The URL from `/files/upload-url` carries its own authority: no bearer key, like boat's.
        return upload(shared, &path, &query, &body).await;
    }
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(body) => body,
            Err(_) => return reply(400, failure(400, "invalid_json", "Not JSON.", &json!({})).1),
        }
    };
    let Some(route) = path.strip_prefix("/api/v1") else {
        return reply(404, failure(404, "not_found", "No such route.", &json!({})).1);
    };
    let route = route.to_string();
    let stop_key_for = bearer
        .as_ref()
        .and_then(|b| shared.stop_keys.lock().ok().and_then(|k| k.get(b).cloned()));
    if let Ok(mut calls) = shared.calls.lock() {
        calls.push(Call {
            method: method.clone(),
            path: route.clone(),
            body: body.clone(),
            idempotency_key: idempotency_key.clone(),
            org,
            stop_key: stop_key_for.is_some(),
        });
    }
    let account = bearer.as_deref() == Some(shared.config.key.as_str());
    if !account && stop_key_for.is_none() {
        return reply(401, failure(401, "unauthorized", "Invalid API key.", &json!({})).1);
    }
    let failed = shared.failures.lock().ok().and_then(|mut failures| {
        let at = failures
            .iter()
            .position(|f| f.method == method && route.ends_with(&f.path_suffix))?;
        Some(failures.remove(at))
    });
    if let Some(f) = failed {
        let (status, body) = failure(f.status, &f.code, &f.message, &f.extra);
        return reply(status, body);
    }
    let segments: Vec<&str> = route.trim_matches('/').split('/').collect();
    if let Some(allowed) = &stop_key_for {
        // A minted stop key reads and stops its own machine, nothing else.
        let ok = match (method.as_str(), segments.as_slice()) {
            ("GET", ["sandboxes", id]) | ("POST", ["sandboxes", id, "stop"]) => id == allowed,
            _ => false,
        };
        if !ok {
            return reply(
                403,
                failure(403, "api_key_action_forbidden", "This key may not do that.", &json!({})).1,
            );
        }
    }
    let (status, body) = route_call(shared, &method, &segments, body, idempotency_key, confirm, &host).await;
    reply(status, body)
}

async fn route_call(
    shared: &Shared,
    method: &str,
    segments: &[&str],
    body: Value,
    idempotency_key: Option<String>,
    confirm: Option<String>,
    host: &str,
) -> (u16, Value) {
    let none = json!({});
    match (method, segments) {
        ("POST", ["saved-snapshots"]) => save_snapshot(shared, &body),
        ("GET", ["saved-snapshots"]) => {
            let mut all: Vec<Value> = shared
                .saved
                .lock()
                .map(|s| s.values().map(Saved::json).collect())
                .unwrap_or_default();
            all.sort_by_key(|v| v["seq"].as_u64().unwrap_or(0));
            (200, json!({ "ok": true, "type": "saved_snapshots", "snapshots": all }))
        }
        ("GET", ["saved-snapshots", name]) => {
            match shared.saved.lock().ok().and_then(|s| s.get(*name).map(Saved::json)) {
                Some(saved) => (
                    200,
                    json!({ "ok": true, "type": "saved_snapshot", "snapshot": saved.clone(), "status": saved["status"],
                "name": saved["name"] }),
                ),
                None => failure(
                    404,
                    "saved_snapshot_not_found",
                    "No saved snapshot has that name.",
                    &none,
                ),
            }
        }
        ("DELETE", ["saved-snapshots", name]) => match shared.saved.lock().ok().and_then(|mut s| s.remove(*name)) {
            Some(_) => (
                200,
                json!({ "ok": true, "type": "saved_snapshot.deleted", "name": name }),
            ),
            None => failure(
                404,
                "saved_snapshot_not_found",
                "No saved snapshot has that name.",
                &none,
            ),
        },
        ("POST", ["sandboxes"]) => create(shared, body, idempotency_key),
        ("POST", ["api-keys", "scoped"]) => mint(shared, &body),
        ("POST", ["sandboxes", id, "commands"]) => {
            match with(shared, id, |s| Ok((200, json!(s.state)))) {
                Ok((_, state)) if state == "ready" => {}
                Ok(_) => return failure(409, "sandbox_not_ready", "The sandbox is not running.", &none),
                Err(error) => return error,
            }
            exec(shared, &body).await
        }
        (_, ["sandboxes", id, rest @ ..]) => {
            let id = id.to_string();
            match (method, rest) {
                ("GET", []) => with(shared, &id, |s| {
                    Ok((200, json!({ "ok": true, "type": "sandbox.info", "sandbox": s.json() })))
                })
                .unwrap_or_else(|e| e),
                ("DELETE", []) => {
                    if confirm.as_deref() != Some(id.as_str()) {
                        return failure(409, "confirm_delete_mismatch", "X-Ascii-Confirm-Delete must equal the id.", &none);
                    }
                    let removed = shared.sandboxes.lock().ok().and_then(|mut s| s.remove(&id));
                    match removed {
                        Some(_) => (
                            202,
                            json!({ "ok": true, "type": "sandbox.deleting", "operation": {
                                "id": "bdop_00000000000000000000000000000001", "kind": "sandbox", "targetId": id,
                                "reason": "explicit", "status": "pending", "attemptCount": 0,
                                "requestedAt": "2026-11-01T00:00:00Z", "completedAt": null } }),
                        ),
                        None => failure(404, "sandbox_not_found", "Sandbox not found.", &none),
                    }
                }
                ("POST", ["stop"]) => with(shared, &id, |s| {
                    if matches!(s.state.as_str(), "archived" | "archiving") {
                        return Err(failure(409, "sandbox_not_running", "The sandbox is already stopped.", &json!({})));
                    }
                    s.state = "archiving".into();
                    s.due = Some(Instant::now() + shared.config.stop_after);
                    Ok((
                        202,
                        json!({ "ok": true, "type": "sandbox.stopping", "id": s.id, "status": "archiving", "sandbox": s.json() }),
                    ))
                })
                .unwrap_or_else(|e| e),
                ("POST", ["resume"]) => with(shared, &id, |s| {
                    if s.data_expired {
                        return Err(failure(
                            409,
                            "snapshot_data_expired",
                            "This sandbox's snapshot data was deleted 14 days after it stopped.",
                            &json!({}),
                        ));
                    }
                    if !matches!(s.state.as_str(), "archived" | "archiving") {
                        return Err(failure(409, "sandbox_running", "The sandbox is already running.", &json!({})));
                    }
                    if let Some(size) = body.get("type").and_then(Value::as_str) {
                        s.size = size.into();
                    }
                    s.state = "provisioning".into();
                    s.due = Some(Instant::now() + shared.config.ready_after);
                    Ok((
                        202,
                        json!({ "ok": true, "type": "sandbox.resuming", "id": s.id, "status": "resuming", "sandbox": s.json() }),
                    ))
                })
                .unwrap_or_else(|e| e),
                ("POST", ["files", "upload-url"]) => with(shared, &id, |_| {
                    let Some(path) = body.get("path").and_then(Value::as_str) else {
                        return Err(failure(400, "invalid_path", "path is required.", &json!({})));
                    };
                    Ok((200, json!({ "ok": true, "url": format!("http://{host}/upload/{id}?path={path}") })))
                })
                .unwrap_or_else(|e| e),
                ("POST", ["sshkey"]) => sshkey(shared, &id, &body).await,
                ("POST", ["host"]) => with(shared, &id, |s| {
                    let port = body.get("port").and_then(Value::as_u64).unwrap_or(0);
                    if port == 0 || port > 65535 {
                        return Err(failure(400, "invalid_port", "Port must be 1-65535.", &json!({})));
                    }
                    let public = body.get("public").and_then(Value::as_bool).unwrap_or(false);
                    let localhost = body.get("localhost").and_then(Value::as_bool).unwrap_or(false);
                    let token = if public { String::new() } else { format!("?_token=tok_{}", s.id) };
                    Ok((
                        200,
                        json!({ "ok": true, "type": "port.hosted", "success": true, "port": port,
                            "url": format!("https://{}-{port}.on.boat.test/{token}", s.id),
                            "isProtected": !public, "access": if public { "public" } else { "private" },
                            "localhost": localhost }),
                    ))
                })
                .unwrap_or_else(|e| e),
                ("POST", ["desktop"]) => with(shared, &id, |s| {
                    if s.state != "ready" {
                        return Err(failure(400, "machine_not_running", "The sandbox is not running.", &json!({})));
                    }
                    Ok((
                        200,
                        json!({ "ok": true, "type": "desktop.url", "success": true,
                            "desktopUrl": format!("https://desktop.boat.test/stream.html?hostId={}#token=tok", s.id),
                            "ip": "127.0.0.1", "mode": "moonlight" }),
                    ))
                })
                .unwrap_or_else(|e| e),
                _ => failure(404, "not_found", "No such route.", &none),
            }
        }
        _ => failure(404, "not_found", "No such route.", &none),
    }
}

/// Runs `f` on machine `id` after moving its state on; a missing one is boat's 404.
fn with(
    shared: &Shared,
    id: &str,
    f: impl FnOnce(&mut Sandbox) -> Result<(u16, Value), (u16, Value)>,
) -> Result<(u16, Value), (u16, Value)> {
    let Ok(mut sandboxes) = shared.sandboxes.lock() else {
        return Err(failure(500, "internal", "Lock poisoned.", &json!({})));
    };
    let Some(sandbox) = sandboxes.get_mut(id) else {
        return Err(failure(404, "sandbox_not_found", "Sandbox not found.", &json!({})));
    };
    sandbox.advance();
    f(sandbox)
}

fn create(shared: &Shared, body: Value, idempotency_key: Option<String>) -> (u16, Value) {
    if let Some(key) = &idempotency_key
        && let Some(id) = shared.idempotency.lock().ok().and_then(|k| k.get(key).cloned())
        && let Ok((_, existing)) = with(shared, &id, |s| Ok((200, s.json())))
    {
        let status = if existing["state"] == "provisioning" {
            "provisioning"
        } else {
            "ready"
        };
        return (
            202,
            json!({ "ok": true, "type": "sandbox.created", "status": status, "ttlSeconds": null, "sandbox": existing }),
        );
    }
    if let Some(from) = body.get("from").and_then(Value::as_str)
        && !shared.saved.lock().is_ok_and(|s| s.contains_key(from))
    {
        return failure(
            404,
            "saved_snapshot_not_found",
            "No saved snapshot has that name.",
            &json!({}),
        );
    }
    let n = shared.next.fetch_add(1, Ordering::SeqCst);
    let id = format!("bx_{}", short(n));
    let sandbox = Sandbox {
        id: id.clone(),
        name: body.get("name").and_then(Value::as_str).unwrap_or("Boat").to_string(),
        state: "provisioning".into(),
        labels: body
            .get("labels")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        size: body
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string(),
        due: Some(Instant::now() + shared.config.ready_after),
        stopped: false,
        data_expired: false,
        snapshotted: false,
    };
    let json = sandbox.json();
    if let Ok(mut sandboxes) = shared.sandboxes.lock() {
        sandboxes.insert(id.clone(), sandbox);
    }
    if let (Some(key), Ok(mut keys)) = (idempotency_key, shared.idempotency.lock()) {
        keys.insert(key, id);
    }
    (
        202,
        json!({ "ok": true, "type": "sandbox.created", "status": "provisioning", "ttlSeconds": null, "sandbox": json }),
    )
}

/// `POST /saved-snapshots {sandboxId, name}`: boat saves a stopped machine's disk; a name taken is replaced.
fn save_snapshot(shared: &Shared, body: &Value) -> (u16, Value) {
    let none = json!({});
    let name = body.get("name").and_then(Value::as_str).unwrap_or("");
    if !valid_saved_name(name) {
        return failure(
            400,
            "invalid_name",
            "The name must match ^[a-z0-9][a-z0-9-]{0,62}$.",
            &none,
        );
    }
    let Some(id) = body.get("sandboxId").and_then(Value::as_str) else {
        return failure(400, "invalid_request", "sandboxId is required.", &none);
    };
    let found = with(shared, id, |s| {
        if !s.snapshotted {
            return Err(failure(
                409,
                "sandbox_not_stopped",
                "Stop the sandbox first.",
                &json!({}),
            ));
        }
        Ok((200, json!(s.size)))
    });
    let size = match found {
        Ok((_, size)) => size.as_str().unwrap_or("default").to_string(),
        Err(error) => return error,
    };
    let seq = shared.next.fetch_add(1, Ordering::SeqCst);
    let saved = Saved {
        name: name.into(),
        source: id.into(),
        size,
        seq,
        ready_at: Instant::now() + shared.config.ready_after,
    };
    if let Ok(mut all) = shared.saved.lock() {
        all.insert(name.into(), saved);
    }
    (
        202,
        json!({ "ok": true, "type": "saved_snapshot.saving", "status": "saving", "name": name }),
    )
}

/// `PUT /upload/<id>?path=<relative path>`: the file's bytes, written under [`Config::exec_cwd`].
async fn upload(shared: &Shared, path: &str, query: &str, bytes: &[u8]) -> Response<Full<Bytes>> {
    let known = path
        .strip_prefix("/upload/")
        .is_some_and(|id| shared.sandboxes.lock().is_ok_and(|s| s.contains_key(id)));
    let target = query.strip_prefix("path=").unwrap_or("");
    if !known || target.is_empty() || target.starts_with('/') || target.split('/').any(|part| part == "..") {
        return reply(400, failure(400, "invalid_path", "No such upload.", &json!({})).1);
    }
    let file = shared.config.exec_cwd.clone().unwrap_or_default().join(target);
    if let Some(dir) = file.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    match tokio::fs::write(&file, bytes).await {
        Ok(()) => reply(200, json!({ "ok": true })),
        Err(_) => reply(500, failure(500, "internal", "Could not write the file.", &json!({})).1),
    }
}

fn mint(shared: &Shared, body: &Value) -> (u16, Value) {
    let none = json!({});
    if !shared.config.scoped_keys {
        return failure(403, "api_key_action_forbidden", "This key may not create keys.", &none);
    }
    let allowed = ["name", "ttl", "preset", "actions", "sandboxIds", "environmentIds"];
    if body
        .as_object()
        .is_none_or(|fields| fields.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return failure(400, "invalid_request", "Unknown field.", &none);
    }
    let Some(id) = body
        .get("sandboxIds")
        .and_then(Value::as_array)
        .and_then(|ids| ids.first())
        .and_then(Value::as_str)
    else {
        return failure(400, "invalid_request", "sandboxIds is required here.", &none);
    };
    let n = shared.next.fetch_add(1, Ordering::SeqCst);
    let secret = format!("boat_stop_{}", short(n));
    if let Ok(mut keys) = shared.stop_keys.lock() {
        keys.insert(secret.clone(), id.to_string());
    }
    let key = json!({ "id": format!("sak_{n}"), "name": body.get("name"), "credentialLane": "scoped-v1",
        "keyPrefix": "boat_stop", "keyLastFour": &secret[secret.len() - 4..], "sandboxId": null,
        "createdAt": "2026-11-01T00:00:00Z", "lastUsedAt": null, "usage": {}, "resources": {},
        "scope": { "actions": body.get("actions"), "sandboxes": [id] } });
    (
        201,
        json!({ "ok": true, "type": "api_key.created", "apiKey": key, "secret": secret, "apiKeys": [key] }),
    )
}

async fn sshkey(shared: &Shared, id: &str, body: &Value) -> (u16, Value) {
    let none = json!({});
    let state = with(shared, id, |s| Ok((200, json!(s.state)))).map(|(_, s)| s);
    match state {
        Ok(state) if state == "ready" => {}
        Ok(_) => return failure(400, "machine_not_running", "The sandbox is not running.", &none),
        Err(error) => return error,
    }
    let key = body.get("key").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if !key.starts_with("ssh-") || key.contains("PRIVATE") || key.contains('\n') {
        return failure(400, "invalid_key", "Send a public SSH key in OpenSSH format.", &none);
    }
    let Some(ssh) = &shared.config.ssh else {
        return (
            200,
            json!({ "ok": true, "type": "ssh_key.configured", "success": true, "machineIp": "203.0.113.10",
                "sshUser": "user", "sshEndpoint": null,
                "hostKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeHostKeyForTheFakeBoatServerOnly0000" }),
        );
    };
    let existing = tokio::fs::read_to_string(&ssh.authorized_keys)
        .await
        .unwrap_or_default();
    if !existing.lines().any(|line| line.trim() == key) {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ssh.authorized_keys)
            .await;
        let Ok(mut file) = file else {
            return failure(500, "internal", "Could not write authorized_keys.", &none);
        };
        let _ = file.write_all(format!("{key}\n").as_bytes()).await;
    }
    (
        200,
        json!({ "ok": true, "type": "ssh_key.configured", "success": true, "machineIp": null,
            "sshUser": ssh.user, "sshEndpoint": ssh.endpoint, "hostKey": ssh.host_key }),
    )
}

async fn exec(shared: &Shared, body: &Value) -> (u16, Value) {
    let Some(command) = body.get("command").and_then(Value::as_str) else {
        return failure(400, "invalid_command", "command is required.", &json!({}));
    };
    let seconds = body.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(30);
    if !(1..=600).contains(&seconds) {
        return failure(400, "invalid_timeout", "timeoutSeconds must be 1-600.", &json!({}));
    }
    let mut child = async_command("sh");
    child.arg("-c").arg(command).kill_on_drop(true);
    if let Some(cwd) = &shared.config.exec_cwd {
        child.current_dir(cwd);
    }
    child.envs(shared.config.exec_env.iter().map(|(k, v)| (k, v)));
    let n = shared.next.fetch_add(1, Ordering::SeqCst);
    let ran = tokio::time::timeout(Duration::from_secs(seconds), child.output()).await;
    let (exit, stdout, stderr, timed_out) = match ran {
        Ok(Ok(output)) => (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            false,
        ),
        Ok(Err(e)) => (Some(127), String::new(), e.to_string(), false),
        Err(_) => (None, String::new(), String::new(), true),
    };
    (
        200,
        json!({ "ok": true, "type": "command.finished", "success": exit == Some(0), "commandId": format!("cmd_{n}"),
            "exitCode": exit, "signal": null, "oomKilled": false, "stdout": stdout, "stderr": stderr,
            "stdoutTruncated": false, "stderrTruncated": false, "timedOut": timed_out, "cwd": "/home/user",
            "startedAt": "2026-11-01T00:00:00Z", "finishedAt": "2026-11-01T00:00:00Z" }),
    )
}
