//! Gateway's cloud machines API in memory (feature `fake`), for the app's tests: the calls of
//! Gateway's API under `/v1/cloud/machines`, answered by doing what Gateway does,
//! with Nexus's own boat key, against a [`crate::fake::FakeBoat`] (or any boat API).
//!
//! - Every call needs `Authorization: Bearer <token>` with one of [`GatewayConfig::tokens`]: each token is one device
//!   of the same person, so two app instances see the same machine.
//! - Gateway's own ids (`cm_…`), never boat's: the app must not mix them up.
//! - One machine per desktop session: a create names its `sessionId` and `title`, and a create for a
//!   session that has a live machine answers that machine. A create with an `Idempotency-Key` seen before does too.
//! - With [`GatewayConfig::capacity`], a create past it is queued (202, `state: queued`, `queuedUntil`); it is made at
//!   boat once a machine is deleted.
//! - `POST …/{id}/activity` (the host's heartbeat) answers 204.
//! - [`FakeGateway::refuse`] answers every call with a refusal, as Gateway does with no boat key (503
//!   `cloud_unavailable`) or for a role without `cloud.machines.use` (403).
//! - [`FakeGateway::add_stranger`] accepts another person's token: its list is empty, and the person's machines are
//!   404 to it, as Gateway answers for anyone else's machine.
//! - [`FakeGateway::delay`] answers every call late, for the tests of an answer that arrives after a sign-out.
//! - [`FakeGateway::calls`] lists what was called, for the tests' assertions.
//! - Cloud session history: [`FakeGateway::store_history`] stores a thread's events as the machine would
//!   send them, and `GET …/{id}/history/{thread}/events` pages them (`after`, `before`, `limit`) as Gateway does.
//!   [`FakeGateway::set_history_enabled`] is the organization's `cloudHistory` switch.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::boat::Boat;
use crate::{Error, Provider, Size, Spec, State};

/// How the fake Gateway behaves.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// The Nexus tokens it accepts, one per device of the same person.
    pub tokens: Vec<String>,
    /// The boat API it calls (a [`crate::fake::FakeBoat`]'s URL) and Nexus's key there.
    pub boat_url: String,
    pub boat_key: String,
    /// How many machines boat may run at once; more are queued. None: no limit.
    pub capacity: Option<usize>,
}

/// One call the fake answered.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayCall {
    pub method: String,
    /// The path after `/v1/cloud/machines` (`""`, `/cm_1/access`).
    pub path: String,
    /// The query, without its `?` (`before=41&limit=100`); empty when none.
    pub query: String,
    pub body: Value,
    /// The bearer token it came with.
    pub token: Option<String>,
    pub idempotency_key: Option<String>,
}

struct Record {
    /// boat's id; None while queued.
    boat: Option<String>,
    session_id: String,
    title: Option<String>,
    size: String,
    created_at: i64,
    /// Why it stopped last (`idle`, `resize`), as Gateway says it.
    stop_reason: Option<String>,
}

struct Shared {
    config: GatewayConfig,
    boat: Boat,
    machines: Mutex<BTreeMap<String, Record>>,
    idempotency: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<GatewayCall>>,
    refusal: Mutex<Option<(u16, String)>>,
    /// Other people's tokens: accepted, and they see none of the person's machines.
    strangers: Mutex<HashSet<String>>,
    /// How long each answer waits.
    delay: Mutex<Option<Duration>>,
    next: AtomicU64,
    /// Each thread's stored events by machine and thread: `{seq, at, body}` or `{seq, at, body: null, omittedBytes}`.
    history: Mutex<HashMap<(String, String), BTreeMap<i64, Value>>>,
    /// The organization's `cloudHistory`.
    history_off: AtomicBool,
}

/// A running fake Gateway. Dropping it stops the server.
pub struct FakeGateway {
    url: String,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeGateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Gateway's error envelope.
fn refusal(status: u16, code: &str, message: &str) -> (u16, Value) {
    (
        status,
        json!({ "error": { "message": message, "type": "invalid_request_error", "code": code, "param": null } }),
    )
}

/// A boat failure as Gateway passes it on: boat's code kept, so the app knows a machine still starting.
fn from_boat(error: &Error) -> (u16, Value) {
    match error {
        Error::NotFound => refusal(404, "not_found", "No such cloud machine."),
        Error::DataExpired => refusal(409, "snapshot_data_expired", &error.to_string()),
        Error::OverBudget => refusal(402, "budget_exceeded", &error.to_string()),
        Error::RateLimited { .. } => refusal(429, "rate_limited", &error.to_string()),
        Error::Api { status, code, message } => refusal(*status, code, message),
        Error::Unauthorized(_) => refusal(502, "provider_error", "The cloud provider refused Nexus's key."),
        Error::Network(message) | Error::Other(message) => refusal(502, "provider_error", message),
    }
}

impl FakeGateway {
    /// Starts on a free port on 127.0.0.1.
    pub async fn start(config: GatewayConfig) -> std::io::Result<FakeGateway> {
        let boat = Boat::new(Some(&config.boat_url), config.boat_key.clone(), None)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            config,
            boat,
            machines: Mutex::new(BTreeMap::new()),
            idempotency: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            refusal: Mutex::new(None),
            strangers: Mutex::new(HashSet::new()),
            delay: Mutex::new(None),
            next: AtomicU64::new(1),
            history: Mutex::new(HashMap::new()),
            history_off: AtomicBool::new(false),
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
        Ok(FakeGateway {
            url: format!("http://{address}"),
            shared,
            task,
        })
    }

    /// Gateway's base URL, `http://127.0.0.1:<port>` (the paths start with `/v1/`).
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<GatewayCall> {
        self.shared.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// From now on every call is refused with `status` and `code` (503 `cloud_unavailable`, 403 `forbidden`); None
    /// answers again.
    pub fn refuse(&self, refusal: Option<(u16, &str)>) {
        if let Ok(mut slot) = self.shared.refusal.lock() {
            *slot = refusal.map(|(status, code)| (status, code.to_string()));
        }
    }

    /// Accepts `token` as another person's sign-in: its list is empty, and every machine is 404 to it.
    pub fn add_stranger(&self, token: &str) {
        if let Ok(mut strangers) = self.shared.strangers.lock() {
            strangers.insert(token.to_string());
        }
    }

    /// From now on every answer waits `delay` first (the call is recorded at once); None answers at once again.
    pub fn delay(&self, delay: Option<Duration>) {
        if let Ok(mut slot) = self.shared.delay.lock() {
            *slot = delay;
        }
    }

    /// Stores thread `thread`'s events on machine `id` as the machine sends them: `(seq, at, body)`, a body of None
    /// being one over the cap (stored as omitted, 300 000 bytes).
    pub fn store_history(&self, id: &str, thread: &str, events: &[(i64, i64, Option<&str>)]) {
        let Ok(mut history) = self.shared.history.lock() else {
            return;
        };
        let stored = history.entry((id.to_string(), thread.to_string())).or_default();
        for (seq, at, body) in events {
            let event = match body {
                Some(body) => json!({ "seq": seq, "at": at, "body": body }),
                None => json!({ "seq": seq, "at": at, "body": null, "omittedBytes": 300_000 }),
            };
            stored.insert(*seq, event);
        }
    }

    /// The organization's `cloudHistory`: off, reads answer `enabled: false` and nothing.
    pub fn set_history_enabled(&self, enabled: bool) {
        self.shared.history_off.store(!enabled, Ordering::SeqCst);
    }

    /// boat's id for Gateway's machine `id`.
    pub fn boat_id(&self, id: &str) -> Option<String> {
        self.shared.machines.lock().ok()?.get(id).and_then(|r| r.boat.clone())
    }

    /// Says why machine `id` stopped (`idle`, `resize`), as Gateway does after it put it to sleep.
    pub fn set_stop_reason(&self, id: &str, reason: Option<&str>) {
        if let Ok(mut machines) = self.shared.machines.lock()
            && let Some(record) = machines.get_mut(id)
        {
            record.stop_reason = reason.map(String::from);
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
    let query = request.uri().query().unwrap_or_default().to_string();
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let token = header("authorization").and_then(|a| a.strip_prefix("Bearer ").map(str::to_string));
    let idempotency_key = header("idempotency-key");
    let body = match Limited::new(request.into_body(), 1 << 20).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return reply(413, refusal(413, "body_too_large", "Too large.").1),
    };
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let Some(route) = path.strip_prefix("/v1/cloud/machines") else {
        return reply(404, refusal(404, "not_found", "No such route.").1);
    };
    if let Ok(mut calls) = shared.calls.lock() {
        calls.push(GatewayCall {
            method: method.clone(),
            path: route.to_string(),
            query: query.clone(),
            body: body.clone(),
            token: token.clone(),
            idempotency_key: idempotency_key.clone(),
        });
    }
    let delay = shared.delay.lock().ok().and_then(|d| *d);
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    let stranger = token
        .as_ref()
        .is_some_and(|t| shared.strangers.lock().is_ok_and(|s| s.contains(t)));
    if !stranger && !token.as_ref().is_some_and(|t| shared.config.tokens.contains(t)) {
        return reply(401, refusal(401, "unauthorized", "Sign in again.").1);
    }
    if let Some((status, code)) = shared.refusal.lock().ok().and_then(|r| r.clone()) {
        let message = match code.as_str() {
            "cloud_unavailable" => "Cloud machines are not available on this Gateway.",
            _ => "You may not use cloud machines in this organization.",
        };
        return reply(status, refusal(status, &code, message).1);
    }
    if stranger {
        // Another person: none of the machines here are theirs.
        return match (method.as_str(), route.trim_matches('/')) {
            ("GET", "") => reply(200, json!({ "machines": [] })),
            _ => reply(404, refusal(404, "not_found", "No such cloud machine.").1),
        };
    }
    let segments: Vec<&str> = route.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let (status, body) = match (method.as_str(), segments.as_slice()) {
        ("GET", [id, "history", thread, "events"]) => history_page(shared, id, thread, &query),
        _ => route_call(shared, &method, &segments, body, idempotency_key).await,
    };
    if status == 204 {
        let mut response = Response::new(Full::new(Bytes::new()));
        *response.status_mut() = StatusCode::NO_CONTENT;
        return response;
    }
    reply(status, body)
}

/// Gateway's machine: `{id, name, sessionId, title, provider, size, state, error, createdAt, sleepingSince,
/// queuedUntil, stopReason}`, its state read from boat. A queued one is made first if boat has room now.
async fn machine_json(shared: &Shared, id: &str) -> (u16, Value) {
    make_queued(shared).await;
    let Some((boat_id, session_id, title, size, created_at, stop_reason)) = shared.machines.lock().ok().and_then(|m| {
        m.get(id).map(|r| {
            (
                r.boat.clone(),
                r.session_id.clone(),
                r.title.clone(),
                r.size.clone(),
                r.created_at,
                r.stop_reason.clone(),
            )
        })
    }) else {
        return refusal(404, "not_found", "No such cloud machine.");
    };
    let name = title.clone().unwrap_or_else(|| "Cloud".into());
    let Some(boat_id) = boat_id else {
        return (
            200,
            json!({ "id": id, "name": name, "sessionId": session_id, "title": title, "provider": "boat", "size": size,
                "state": "queued", "error": null, "createdAt": created_at, "sleepingSince": null,
                "queuedUntil": now_ms() + 120_000, "stopReason": null }),
        );
    };
    let machine = match shared.boat.get(&boat_id).await {
        Ok(machine) => machine,
        Err(error) => return from_boat(&error),
    };
    let (state, error) = match &machine.state {
        State::Creating => ("creating", None),
        State::Starting => ("starting", None),
        State::Ready => ("ready", None),
        State::Sleeping if machine.data_expired_at.is_some() => ("failed", Some(Error::DataExpired.to_string())),
        State::Sleeping => ("sleeping", None),
        State::Stopping => ("stopping", None),
        State::Failed { message } => ("failed", Some(message.clone())),
        State::Gone => ("gone", None),
    };
    let sleeping_since = match machine.state {
        State::Sleeping => machine.stopped_at.clone(),
        _ => None,
    };
    let stop_reason = stop_reason.filter(|_| matches!(machine.state, State::Sleeping | State::Stopping));
    (
        200,
        json!({ "id": id, "name": name, "sessionId": session_id, "title": title, "provider": "boat", "size": size,
            "state": state, "error": error, "createdAt": created_at, "sleepingSince": sleeping_since,
            "queuedUntil": null, "stopReason": stop_reason }),
    )
}

/// Makes the queued machines boat has room for now, oldest first.
async fn make_queued(shared: &Shared) {
    loop {
        let next = shared.machines.lock().ok().and_then(|machines| {
            let made = machines.values().filter(|r| r.boat.is_some()).count();
            if shared.config.capacity.is_some_and(|capacity| made >= capacity) {
                return None;
            }
            machines
                .iter()
                .filter(|(_, r)| r.boat.is_none())
                .min_by_key(|(id, r)| (r.created_at, id.to_string()))
                .map(|(id, r)| (id.clone(), r.size.clone()))
        });
        let Some((id, size)) = next else { return };
        let spec = Spec {
            name: format!("nexus-{id}"),
            size: Size::parse(&size).unwrap_or_default(),
            labels: [("nexus.machine".to_string(), id.clone())].into_iter().collect(),
        };
        let Ok(boat_id) = shared.boat.create(spec).await else {
            return;
        };
        if let Ok(mut machines) = shared.machines.lock()
            && let Some(record) = machines.get_mut(&id)
        {
            record.boat = Some(boat_id);
        }
    }
}

async fn route_call(
    shared: &Shared,
    method: &str,
    segments: &[&str],
    body: Value,
    idempotency_key: Option<String>,
) -> (u16, Value) {
    match (method, segments) {
        ("GET", []) => {
            let ids: Vec<String> = shared
                .machines
                .lock()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let mut machines = Vec::new();
            for id in ids {
                let (status, machine) = machine_json(shared, &id).await;
                if status == 200 {
                    machines.push(machine);
                }
            }
            (200, json!({ "machines": machines }))
        }
        ("POST", []) => create(shared, &body, idempotency_key).await,
        ("GET", [id]) => machine_json(shared, id).await,
        ("DELETE", [id]) => {
            if !shared.machines.lock().is_ok_and(|m| m.contains_key(*id)) {
                return refusal(404, "not_found", "No such cloud machine.");
            }
            // A queued one has nothing at boat yet.
            if let Some(boat_id) = boat_of(shared, id)
                && let Err(error) = shared.boat.delete(&boat_id).await
            {
                return from_boat(&error);
            }
            if let Ok(mut machines) = shared.machines.lock() {
                machines.remove(*id);
            }
            make_queued(shared).await;
            (200, json!({ "id": id, "deleted": true }))
        }
        ("POST", [id, "activity"]) => {
            if !shared.machines.lock().is_ok_and(|m| m.contains_key(*id)) {
                return refusal(404, "not_found", "No such cloud machine.");
            }
            if body.get("busy").and_then(Value::as_bool).is_none() {
                return refusal(400, "invalid_request", "busy is required.");
            }
            (204, Value::Null)
        }
        ("POST", [id, action]) => {
            let Some(boat_id) = boat_of(shared, id) else {
                return refusal(404, "not_found", "No such cloud machine.");
            };
            let boat = &shared.boat;
            match *action {
                "wake" => match boat.wake(&boat_id, None).await {
                    Ok(()) => machine_json(shared, id).await,
                    Err(error) => from_boat(&error),
                },
                "sleep" => match boat.sleep(&boat_id).await {
                    Ok(()) => machine_json(shared, id).await,
                    Err(error) => from_boat(&error),
                },
                "access" => {
                    let Some(key) = body.get("publicKey").and_then(Value::as_str) else {
                        return refusal(400, "invalid_request", "publicKey is required.");
                    };
                    match boat.authorize_ssh(&boat_id, key).await {
                        Ok(access) => (
                            200,
                            json!({ "host": access.host, "port": access.port, "user": access.user,
                                "hostKey": access.host_key }),
                        ),
                        Err(error) => from_boat(&error),
                    }
                }
                "links" => {
                    let link = match body.get("kind").and_then(Value::as_str) {
                        Some("port") => match body
                            .get("port")
                            .and_then(Value::as_u64)
                            .and_then(|p| u16::try_from(p).ok())
                        {
                            Some(port) => boat.preview(&boat_id, port, false).await,
                            None => return refusal(400, "invalid_request", "port is required."),
                        },
                        Some("desktop") => boat.desktop(&boat_id).await,
                        _ => return refusal(400, "invalid_request", "kind is port or desktop."),
                    };
                    match link {
                        Ok(url) => (200, json!({ "url": url, "expiresAt": null })),
                        Err(error) => from_boat(&error),
                    }
                }
                _ => refusal(404, "not_found", "No such route."),
            }
        }
        _ => refusal(404, "not_found", "No such route."),
    }
}

/// One page of a thread's stored history, as Gateway's `GET …/{id}/history/{thread}/events` answers it: the tail with
/// neither `after` nor `before`, ascending, at most `limit` (100 by default, 500 at most).
fn history_page(shared: &Shared, id: &str, thread: &str, query: &str) -> (u16, Value) {
    if !shared.machines.lock().is_ok_and(|m| m.contains_key(id)) {
        return refusal(404, "not_found", "No such cloud machine.");
    }
    let param = |key: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.parse::<i64>())
    };
    let (after, before, limit) = match (param("after"), param("before"), param("limit")) {
        (Some(Err(_)), ..) | (_, Some(Err(_)), _) | (.., Some(Err(_))) => {
            return refusal(400, "invalid_request", "after, before and limit are numbers.");
        }
        (Some(_), Some(_), _) => return refusal(400, "invalid_request", "after and before together."),
        (after, before, limit) => (
            after.and_then(Result::ok),
            before.and_then(Result::ok),
            limit.and_then(Result::ok).unwrap_or(100),
        ),
    };
    if !(1..=500).contains(&limit) {
        return refusal(400, "invalid_request", "limit is 1 to 500.");
    }
    let limit = limit as usize;
    if shared.history_off.load(Ordering::SeqCst) {
        return (
            200,
            json!({ "machineId": id, "threadId": thread, "enabled": false, "firstSeq": null, "lastSeq": null,
                "events": [] }),
        );
    }
    let stored = shared
        .history
        .lock()
        .ok()
        .and_then(|h| h.get(&(id.to_string(), thread.to_string())).cloned())
        .unwrap_or_default();
    let (first, last) = (stored.keys().next().copied(), stored.keys().next_back().copied());
    let events: Vec<Value> = match (after, before) {
        (Some(after), _) => stored.range(after + 1..).take(limit).map(|(_, e)| e.clone()).collect(),
        (_, before) => {
            let mut page: Vec<Value> = stored
                .range(..before.unwrap_or(i64::MAX))
                .rev()
                .take(limit)
                .map(|(_, e)| e.clone())
                .collect();
            page.reverse();
            page
        }
    };
    (
        200,
        json!({ "machineId": id, "threadId": thread, "enabled": true, "firstSeq": first, "lastSeq": last,
            "events": events }),
    )
}

fn boat_of(shared: &Shared, id: &str) -> Option<String> {
    shared.machines.lock().ok()?.get(id).and_then(|r| r.boat.clone())
}

async fn create(shared: &Shared, body: &Value, idempotency_key: Option<String>) -> (u16, Value) {
    if let Some(key) = &idempotency_key
        && let Some(id) = shared.idempotency.lock().ok().and_then(|k| k.get(key).cloned())
    {
        return machine_json(shared, &id).await;
    }
    let Some(session_id) = body
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
    else {
        return refusal(400, "invalid_request", "sessionId is required.");
    };
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from);
    // The session's live machine is the answer.
    let known = shared.machines.lock().ok().and_then(|machines| {
        machines
            .iter()
            .find(|(_, r)| r.session_id == session_id)
            .map(|(id, _)| id.clone())
    });
    if let Some(id) = known {
        return machine_json(shared, &id).await;
    }
    let n = shared.next.fetch_add(1, Ordering::SeqCst);
    let id = format!("cm_{n}");
    if let Ok(mut machines) = shared.machines.lock() {
        machines.insert(
            id.clone(),
            Record {
                boat: None,
                session_id,
                title,
                size: Size::default().as_str().into(),
                created_at: now_ms(),
                stop_reason: None,
            },
        );
    }
    if let Some(key) = idempotency_key
        && let Ok(mut keys) = shared.idempotency.lock()
    {
        keys.insert(key, id.clone());
    }
    let (status, machine) = machine_json(shared, &id).await;
    if status != 200 {
        return (status, machine);
    }
    let queued = machine.get("state").and_then(Value::as_str) == Some("queued");
    (if queued { 202 } else { 201 }, machine)
}
