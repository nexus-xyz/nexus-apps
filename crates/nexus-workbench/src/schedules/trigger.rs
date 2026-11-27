//! A schedule's webhook: `POST http://127.0.0.1:<port>/v1/schedules/<id>/run` with `Authorization: Bearer <token>`
//! starts a run, with the body (at most [`MAX_PAYLOAD`] of UTF-8) added to the prompt as data. It listens on the
//! loopback only, so a script, a git hook or a tunnel the person runs on this computer can call it, and nothing on
//! the network can. Each schedule has its own token, compared in constant time. The same checks as the agents' MCP
//! server ([`crate::orchestra::http`]) keep a web page out: the exact `Host`, so DNS rebinding fails, and no `Origin`,
//! which every browser sends with a POST. One answer for an unknown schedule and a wrong token.
//!
//! The app binds it on a fixed port ([`crate::Config::trigger_port`]), so an address stays the same across launches,
//! and on a free port when another program holds that one. A host with no port configured has no webhooks.

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};

use super::{RunStatus, Trigger};
use crate::event::ThreadSummary;
use crate::orchestra::grant::same_token;
use crate::{Inner, Result, Workbench};

/// The port the desktop app asks for first.
pub const DEFAULT_PORT: u16 = 47_617;
/// The largest body: what a webhook adds to the prompt.
pub const MAX_PAYLOAD: usize = 16 * 1024;
/// Connections at once.
const MAX_CONNECTIONS: usize = 16;
/// A request's headers, and then its body, must arrive within this.
const DEADLINE: Duration = Duration::from_secs(10);

/// A schedule's webhook address on `port`.
pub fn url(port: u16, schedule_id: &str) -> String {
    format!("http://127.0.0.1:{port}/v1/schedules/{schedule_id}/run")
}

/// Why a webhook call starts no run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No such schedule here, its webhook is off, or the token is wrong: one answer for all three.
    Unauthorized,
    /// The schedule is off.
    Off,
    /// It reached its daily cap.
    DailyLimit(String),
    /// The session could not start.
    Failed(String),
}

impl Refusal {
    /// What a person reads.
    pub fn message(self) -> String {
        match self {
            Refusal::Unauthorized => "That schedule no longer exists.".into(),
            Refusal::Off => "That schedule is off.".into(),
            Refusal::DailyLimit(message) | Refusal::Failed(message) => message,
        }
    }
}

/// The bound listener: its port, and its accept loop, which ends with it.
pub(crate) struct Server {
    pub(crate) port: u16,
    accept: tokio::task::AbortHandle,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

struct Shared {
    inner: Weak<Inner>,
    port: u16,
    connections: AtomicUsize,
}

/// Binds on the loopback at `port`, or at a free port when that one is taken (or 0).
pub(crate) async fn bind(inner: Weak<Inner>, port: u16) -> std::io::Result<Server> {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(listener) => listener,
        Err(_) if port != 0 => TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?,
        Err(error) => return Err(error),
    };
    let port = listener.local_addr()?.port();
    let shared = Arc::new(Shared {
        inner,
        port,
        connections: AtomicUsize::new(0),
    });
    let accept = tokio::spawn(accept(listener, shared)).abort_handle();
    Ok(Server { port, accept })
}

/// Boxed: a run starts a session, whose future would otherwise contain this one's type.
fn accept(listener: TcpListener, shared: Arc<Shared>) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.connections.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            tokio::spawn(serve(stream, shared.clone()));
        }
    })
}

async fn serve(stream: TcpStream, shared: Arc<Shared>) {
    let service = {
        let shared = shared.clone();
        service_fn(move |request| {
            let shared = shared.clone();
            async move { Ok::<_, Infallible>(handle(&shared, request).await) }
        })
    };
    let _ = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(DEADLINE)
        .keep_alive(false)
        .serve_connection(TokioIo::new(stream), service)
        .await;
    shared.connections.fetch_sub(1, Ordering::SeqCst);
}

fn answer(status: StatusCode, body: Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    if status == StatusCode::METHOD_NOT_ALLOWED {
        headers.insert(header::ALLOW, HeaderValue::from_static("POST"));
    }
    response
}

fn refuse(status: StatusCode, code: &str, message: &str) -> Response<Full<Bytes>> {
    answer(status, json!({"error": code, "message": message}))
}

async fn handle(shared: &Shared, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let headers = request.headers();
    // The exact Host: a page that reached this port through another name (DNS rebinding) is refused.
    let hosts: Vec<&HeaderValue> = headers.get_all(header::HOST).iter().collect();
    let allowed = [
        format!("127.0.0.1:{}", shared.port),
        format!("localhost:{}", shared.port),
    ];
    let host_ok = hosts.len() == 1 && allowed.iter().any(|host| hosts[0].as_bytes() == host.as_bytes());
    let authority_ok = request
        .uri()
        .authority()
        .is_none_or(|authority| allowed.iter().any(|host| authority.as_str() == host));
    // A browser always says where a page came from; a script never does.
    if !host_ok || !authority_ok || headers.contains_key(header::ORIGIN) {
        return refuse(StatusCode::FORBIDDEN, "forbidden", "Forbidden.");
    }
    if request.method() != Method::POST {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "method", "Only POST.");
    }
    let Some(id) = request
        .uri()
        .path()
        .strip_prefix("/v1/schedules/")
        .and_then(|rest| rest.strip_suffix("/run"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .map(str::to_string)
    else {
        return refuse(StatusCode::NOT_FOUND, "not_found", "Not found.");
    };
    let mut authorizations = headers.get_all(header::AUTHORIZATION).iter();
    let token = match (authorizations.next(), authorizations.next()) {
        (Some(value), None) => value
            .to_str()
            .ok()
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    let Some(inner) = shared.inner.upgrade() else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "closing", "Nexus is closing.");
    };
    let workbench = Workbench { inner };
    // Checked before any body byte is read.
    if token.is_empty() || !workbench.webhook_token_matches(&id, &token) {
        return refuse(StatusCode::UNAUTHORIZED, "unauthorized", "Unauthorized.");
    }
    let too_long = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_PAYLOAD as u64);
    if too_long {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "The body is at most 16 KiB.",
        );
    }
    let body = Limited::new(request.into_body(), MAX_PAYLOAD).collect();
    let body = match tokio::time::timeout(DEADLINE, body).await {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(_)) => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "The body is at most 16 KiB.",
            );
        }
        Err(_) => return refuse(StatusCode::REQUEST_TIMEOUT, "timeout", "The body came too slowly."),
    };
    let Ok(payload) = String::from_utf8(body.to_vec()) else {
        return refuse(StatusCode::BAD_REQUEST, "bad_body", "The body is UTF-8 text.");
    };
    match workbench.trigger_schedule(&id, &token, Some(payload)).await {
        Ok(thread) => answer(StatusCode::ACCEPTED, json!({"sessionId": thread.id})),
        Err(Refusal::Unauthorized) => refuse(StatusCode::UNAUTHORIZED, "unauthorized", "Unauthorized."),
        Err(Refusal::Off) => refuse(StatusCode::CONFLICT, "off", "That schedule is off."),
        Err(Refusal::DailyLimit(message)) => refuse(StatusCode::TOO_MANY_REQUESTS, "daily_limit", &message),
        Err(Refusal::Failed(message)) => refuse(StatusCode::BAD_GATEWAY, "failed", &message),
    }
}

impl Workbench {
    /// The trigger's port once it listens.
    pub(crate) fn trigger_port(&self) -> Option<u16> {
        self.inner.triggers.get().map(|server| server.port)
    }

    /// Binds the trigger now (when the host has a port for it), the first time: what turning a webhook on needs.
    pub(crate) async fn serve_triggers_now(&self) -> Result<u16> {
        let Some(port) = self.config().trigger_port else {
            return Err("Webhooks are not available here.".into());
        };
        let weak = Arc::downgrade(&self.inner);
        self.inner
            .triggers
            .get_or_try_init(|| bind(weak, port))
            .await
            .map(|server| server.port)
            .map_err(|error| format!("The webhook listener could not start: {error}"))
    }

    /// Binds the trigger when a schedule here has its webhook on: at launch, and at each tick until it listens.
    pub async fn serve_triggers(&self) {
        if self.trigger_port().is_some() || self.config().trigger_port.is_none() {
            return;
        }
        let scope = self.config().scope;
        let wanted = self
            .store()
            .schedules(&scope)
            .is_ok_and(|schedules| schedules.iter().any(|schedule| schedule.webhook));
        if wanted && let Err(error) = self.serve_triggers_now().await {
            eprintln!("nexus-workbench: {error}");
        }
    }

    /// Whether `token` is the webhook token of schedule `id`, in the signed-in organization, with its webhook on.
    pub(crate) fn webhook_token_matches(&self, id: &str, token: &str) -> bool {
        let store = self.store();
        let on = store
            .schedule(id)
            .ok()
            .flatten()
            .is_some_and(|(scope, schedule)| self.in_scope(&scope) && schedule.webhook);
        let saved = store.schedule_token_hash(id).ok().flatten();
        on && saved.is_some_and(|saved| same_token(&crate::orchestra::grant::hash_token(token), &saved))
    }

    /// A webhook call: checks the token again under the store's lock, takes a run under the day's cap, and starts it
    /// with `payload` framed as data in its first message.
    pub async fn trigger_schedule(
        &self,
        id: &str,
        token: &str,
        payload: Option<String>,
    ) -> std::result::Result<ThreadSummary, Refusal> {
        if !self.webhook_token_matches(id, token) {
            return Err(Refusal::Unauthorized);
        }
        let schedule = match self.claim_run(id, true) {
            Ok(schedule) => schedule,
            Err(Refusal::DailyLimit(message)) => {
                self.remember_refusal(id, &message);
                return Err(Refusal::DailyLimit(message));
            }
            Err(refusal) => return Err(refusal),
        };
        let request = uuid::Uuid::new_v4().to_string();
        self.start_run(schedule, Trigger::Webhook, request, payload)
            .await
            .map_err(Refusal::Failed)
    }

    /// A webhook call past the daily cap, in the schedule's runs: the deliveries log the person reads.
    fn remember_refusal(&self, id: &str, note: &str) {
        let store = self.store();
        if let Ok(Some((scope, mut schedule))) = store.schedule(id) {
            schedule.record(super::Run {
                at: self.config().schedule_clock.now(),
                trigger: Trigger::Webhook,
                status: RunStatus::Skipped,
                thread_id: None,
                note: Some(note.to_string()),
            });
            let _ = store.save_schedule(&scope, &schedule);
        }
    }
}
