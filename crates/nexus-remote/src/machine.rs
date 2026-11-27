//! One remote machine, kept connected while the app runs (a private plan doc, sections 3 and 5).
//!
//! A [`Remote`] opens its [`Link`] (over SSH: install if needed, start if needed, read the token, `nexus-host proxy`),
//! says hello, and relays: [`Remote::call`] sends a `wb_*` call, and every event the host pushes goes to the [`Sink`].
//! When the connection drops (the laptop slept, the network changed, the host restarted) it reconnects, waiting 1, 2,
//! 4 … up to 30 seconds between tries, or at once when asked ([`Remote::reconnect`]).
//! [`Remote::start_manual`] instead waits for an explicit retry after a failure or dropped connection.
//!
//! **Catching up, exactly once.** The relay remembers, per session, the last event number (`seq`) it has delivered or
//! the webview has read (`wb_thread_events`). Back after a drop, it reads each of those sessions' events after that
//! number before it passes on anything pushed live (the host subscribed the connection before its hello, so nothing
//! falls between the two), and it passes on a session's event only past the last number it holds. The webview sees
//! one gapless stream, as if nothing happened (nexus-host's tests/host.rs `Log` is the same rule).
//!
//! **Versions.** The hello carries the host's version and protocol. Another protocol: [`Status::NeedsUpdate`], and
//! nothing else works until [`Remote::update`] ("Update now"). The same protocol but another version ([`replaces`]): an idle host
//! (no session working or waiting) is updated at once, silently; a busy one keeps serving and the status says which
//! version is waiting ([`Status::Connected`]'s `update`). When this app's own version is not published, the host on
//! the machine stands in for it ([`crate::install`]) and is not replaced.
//!
//! **Errors that do not heal.** A try that fails for a reason waiting cannot fix (Gateway refused the sign-in, no host
//! build for this machine) stops the tries: the status is [`Status::Unreachable`] with `retry_in_secs` 0 until
//! [`Remote::reconnect`] or [`Remote::resume`] ("Try again"). Every other failure waits and tries again ([`heals`]).
//!
//! **Through Gateway.** A link may reach the host as a WebSocket already upgraded ([`Link::reach_host`],
//! [`crate::relay::RelayLink`]). The hello is the same; both ends ping, and the setup calls run before the status says
//! connected.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use nexus_workbench::api::HostInfo;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use crate::client::{self, Connection, HelloError, Pushed};
use crate::install::Installer;
use crate::{Duplex, Result};

/// A stream to a running host, and what keeps it open.
pub struct Opened {
    pub stream: Box<dyn Duplex>,
    pub token: String,
    /// The version this app installs there; None when the link installs nothing (a socket in tests), or when another
    /// host stands in for this app's unpublished version ([`crate::install::Ready::exact`]).
    pub version: Option<String>,
    /// The `ssh … proxy` child: the stream ends when it is dropped (killed).
    pub keep: Option<tokio::process::Child>,
    /// The app's Gateway sign-in, said in the hello: only to the app's own service ([`crate::local`]).
    pub sign_in: Option<Value>,
}

/// A host reached through Gateway's relay: the WebSocket is already upgraded ([`crate::relay`]).
pub struct SocketOpened {
    pub socket: crate::relay::Socket,
    /// The hello's token: empty through Gateway, which checked the person already.
    pub token: String,
    /// Calls made after the hello and before [`Status::Connected`] (`host_provision`), in order. One that fails fails
    /// the try.
    pub setup: Vec<(String, Value)>,
}

/// How a link reached its host: a byte stream to upgrade, or a WebSocket.
pub enum Reached {
    Stream(Opened),
    Socket(SocketOpened),
}

/// Why a link could not reach its host, and whether waiting can fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenError {
    pub message: String,
    /// False when only a person can fix it: the tries stop until "Try again".
    pub heals: bool,
}

impl From<String> for OpenError {
    fn from(message: String) -> OpenError {
        OpenError {
            heals: heals(&message),
            message,
        }
    }
}

/// Whether a link's error may go away by itself. No host build for the machine (no manifest entry for its system,
/// a download not found) does not; a lost network, a host that did not start yet and the rest may.
pub fn heals(error: &str) -> bool {
    let lower = error.to_lowercase();
    let unavailable = lower.contains("the nexus host") && lower.contains("is not available");
    let missing = lower.contains("404") || lower.contains("not found") || lower.contains("is not available for");
    !(lower.contains("has no linux") || lower.contains("the nexus host runs on linux") || (unavailable && missing))
}

/// How a [`Remote`] reaches its host.
pub trait Link: Send + Sync + 'static {
    /// Makes sure a host runs there and opens a stream to it.
    fn open(&self) -> BoxFuture<'_, Result<Opened>>;
    /// Replaces the running host with this app's version; its sessions stop as when the app quits.
    fn update(&self) -> BoxFuture<'_, Result<()>>;
    /// What a [`Remote`] calls: [`Link::open`] unless the link reaches its host another way (through Gateway).
    fn reach_host(&self) -> BoxFuture<'_, std::result::Result<Reached, OpenError>> {
        Box::pin(async move { self.open().await.map(Reached::Stream).map_err(OpenError::from) })
    }
}

/// Over the system `ssh`: [`Installer::ensure`], then `nexus-host proxy`.
pub struct SshLink {
    pub installer: Installer,
}

impl Link for SshLink {
    fn open(&self) -> BoxFuture<'_, Result<Opened>> {
        Box::pin(async move {
            let ready = self.installer.ensure().await?;
            let mut child = self.installer.proxy(&ready.dir)?;
            let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
                return Err("Could not talk to ssh.".into());
            };
            Ok(Opened {
                stream: Box::new(tokio::io::join(stdout, stdin)),
                token: ready.token,
                // A host that stands in for this app's unpublished version is not replaced: there is nothing to
                // replace it with, and a silent update would only restart it and connect again.
                version: ready.exact.then_some(ready.version),
                keep: Some(child),
                sign_in: None,
            })
        })
    }

    fn update(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.installer.update().await.map(|_| ()) })
    }
}

/// Straight to a host's socket on this computer, with its token: the tests' link, and a host started by hand.
#[cfg(unix)]
pub struct SocketLink {
    pub socket: std::path::PathBuf,
    pub token: String,
}

#[cfg(unix)]
impl Link for SocketLink {
    fn open(&self) -> BoxFuture<'_, Result<Opened>> {
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .map_err(|e| format!("Could not reach the Nexus host: {e}"))?;
            Ok(Opened {
                stream: Box::new(stream),
                token: self.token.clone(),
                version: None,
                keep: None,
                sign_in: None,
            })
        })
    }

    fn update(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err("This host is not installed by the app.".to_string()) })
    }
}

/// Where the machine's connection is, for the sidebar ("Connecting…", "Connected", "Can't reach").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    #[serde(rename_all = "camelCase")]
    Connecting {
        attempt: u32,
    },
    #[serde(rename_all = "camelCase")]
    Connected {
        host: HostInfo,
        /// The version this app would install, when the host runs another one and is busy (same protocol).
        update: Option<String>,
    },
    /// The host speaks another protocol: nothing works until "Update now".
    #[serde(rename_all = "camelCase")]
    NeedsUpdate {
        host: HostInfo,
        error: String,
    },
    Updating,
    #[serde(rename_all = "camelCase")]
    Unreachable {
        error: String,
        retry_in_secs: u64,
    },
    Closed,
}

/// Where the relay's events go: the app tags them with the machine and emits them to the webview.
pub trait Sink: Send + Sync + 'static {
    fn push(&self, push: &Pushed);
    fn status(&self, status: &Status);
}

/// `wb_*` commands about this computer's own window and settings. A window sends them to this computer even for a
/// remote session; a machine never gets them, as a backstop.
pub const LOCAL_ONLY: [&str; 8] = [
    "wb_viewing",
    "wb_notifications",
    "wb_notifications_set",
    "wb_path_notifications",
    "wb_path_notifications_set",
    "wb_pick_folder",
    // The person's hooks (Settings › Hooks): only this computer's settings.json sets one.
    "wb_hooks",
    "wb_hooks_set",
];

/// Whether `call` may go to a machine: a workbench command that is not about this computer.
pub fn relayable(call: &str) -> Result<()> {
    if !call.starts_with("wb_") {
        return Err(format!("{call} is not a workbench command."));
    }
    if LOCAL_ONLY.contains(&call) {
        return Err(format!("{call} is about this computer, not a machine."));
    }
    Ok(())
}

/// Whether an app that installs host `wanted` replaces a running host of version `running`: when they differ, except
/// that a release build (`0.10.0+abc`) keeps an internal build of its own release line (`0.10.0-internal.42+def`).
/// A release build and an employee's internal build of the same line then share the internal host instead of replacing
/// each other's whenever the server is idle. An internal app still replaces a release host, and builds of one kind still
/// replace each other, as before.
pub fn replaces(wanted: &str, running: &str) -> bool {
    if wanted == running {
        return false;
    }
    let release = |version: &str| version.split('+').next().unwrap_or(version).to_string();
    let (wanted, running) = (release(wanted), release(running));
    let plain = !wanted.contains('-');
    !(plain && running.split_once("-internal.").is_some_and(|(line, _)| line == wanted))
}

/// The waits between tries.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
    /// A connection that lasted this long starts the waits over.
    pub healthy: Duration,
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff {
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
            healthy: Duration::from_secs(60),
        }
    }
}

struct Inner {
    link: Arc<dyn Link>,
    sink: Arc<dyn Sink>,
    backoff: Backoff,
    /// Wait for an explicit reconnect after a failure or dropped connection.
    manual_retry: bool,
    current: Mutex<Option<Arc<Connection>>>,
    status: Mutex<Status>,
    /// The last `seq` delivered or read, per session.
    seqs: Mutex<HashMap<String, i64>>,
    wake: Notify,
    /// Held while connecting and while updating, so the two never overlap.
    busy: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    /// Not connecting until [`Remote::resume`] ([`Remote::hold`]): a cloud machine asleep.
    held: AtomicBool,
    /// The version a silent update was tried for (once: a failed one is not retried on every reconnect).
    silent_tried: Mutex<Option<String>>,
}

/// A remote machine's connection. Dropping it does not stop it: [`Remote::close`] does.
#[derive(Clone)]
pub struct Remote {
    inner: Arc<Inner>,
}

/// Why a try did not end connected.
enum Failure {
    NeedsUpdate {
        host: Box<HostInfo>,
        error: String,
    },
    /// Updated (or tried to): connect again at once.
    Again,
    Other(String),
    /// Waiting will not fix it: no more tries until "Try again".
    Stopped(String),
}

struct Session {
    connection: Arc<Connection>,
    pushed: mpsc::UnboundedReceiver<Pushed>,
    host: HostInfo,
    update: Option<String>,
    _keep: Option<tokio::process::Child>,
}

impl Remote {
    /// Starts connecting, and keeps at it until [`Remote::close`].
    pub fn start(link: Arc<dyn Link>, sink: Arc<dyn Sink>, backoff: Backoff) -> Remote {
        Self::start_with_retry(link, sink, backoff, false)
    }

    /// Connects once, then waits for [`Remote::reconnect`] after any failure or dropped connection.
    pub fn start_manual(link: Arc<dyn Link>, sink: Arc<dyn Sink>, backoff: Backoff) -> Remote {
        Self::start_with_retry(link, sink, backoff, true)
    }

    fn start_with_retry(link: Arc<dyn Link>, sink: Arc<dyn Sink>, backoff: Backoff, manual_retry: bool) -> Remote {
        let inner = Arc::new(Inner {
            link,
            sink,
            backoff,
            manual_retry,
            current: Mutex::new(None),
            status: Mutex::new(Status::Connecting { attempt: 1 }),
            seqs: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            busy: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            held: AtomicBool::new(false),
            silent_tried: Mutex::new(None),
        });
        tokio::spawn(run(inner.clone()));
        Remote { inner }
    }

    pub fn status(&self) -> Status {
        self.inner.status.lock().map(|s| s.clone()).unwrap_or(Status::Closed)
    }

    /// Whether session `thread` is one this relay carries: the webview read its events through it, or they arrived
    /// live. No call to the host: what the relay already knows.
    pub fn holds(&self, thread: &str) -> bool {
        self.inner.seqs.lock().is_ok_and(|seqs| seqs.contains_key(thread))
    }

    fn connection(&self) -> Option<Arc<Connection>> {
        if self.is_held() {
            return None;
        }
        self.inner
            .current
            .lock()
            .ok()?
            .clone()
            .filter(|connection| connection.is_open())
    }

    /// A `wb_*` call (or `host_status`) on the host. Fails at once while not connected.
    pub async fn call(&self, call: &str, args: Value) -> Result<Value> {
        self.call_if_current(call, args, || true).await
    }

    /// Verify the caller's authority after capturing the connection the request would use.
    pub async fn call_if_current(&self, call: &str, args: Value, current: impl FnOnce() -> bool) -> Result<Value> {
        let connection = self.connection().ok_or_else(|| match self.status() {
            Status::NeedsUpdate { .. } => "This machine runs an older Nexus host: update it first.".to_string(),
            Status::Updating => "The Nexus host on this machine is updating.".to_string(),
            Status::Unreachable { error, .. } => format!("Can't reach this machine: {error}"),
            _ => "Not connected to this machine yet.".to_string(),
        })?;
        if !current() {
            return Err("This command's original source authority expired.".into());
        }
        let answer = connection.call(call, args.clone()).await;
        if (call == "wb_thread_events" || call == "wb_thread_page")
            && let (Ok(answer), Some(thread)) = (&answer, args.get("threadId").and_then(Value::as_str))
        {
            // What the webview read counts as held: catch-up starts past it. A page holds its events in `events`.
            let events = answer.get("events").unwrap_or(answer);
            let after = args.get("afterSeq").and_then(Value::as_i64).unwrap_or(0);
            let last = events
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e.get("seq").and_then(Value::as_i64))
                .fold(after, i64::max);
            if let Ok(mut seqs) = self.inner.seqs.lock() {
                let held = seqs.entry(thread.to_string()).or_insert(last);
                *held = (*held).max(last);
            }
        }
        answer
    }

    /// Whether the host runs sessions now, from its `host_status`; None when it did not say (not connected, or no
    /// answer). The app's background service is asked before an update restarts the app, and before it is replaced.
    pub async fn sessions_running(&self) -> Option<bool> {
        self.call("host_status", Value::Null)
            .await
            .ok()
            .and_then(|status| status.get("sessions").and_then(Value::as_array).map(|s| !s.is_empty()))
    }

    /// Connect again now (after the laptop wakes, or the person asks), dropping a connection that may be stale.
    pub fn reconnect(&self) {
        self.drop_connection();
        self.inner.wake.notify_one();
    }

    /// Cuts the connection as a lost network would; the relay reconnects and catches up.
    pub fn drop_connection(&self) {
        let current = self.inner.current.lock().ok().and_then(|mut c| c.take());
        if let Some(connection) = current {
            connection.abort();
        }
    }

    /// "Update now": stops the host's sessions (as quitting the app does), installs and starts this app's version,
    /// and reconnects.
    pub async fn update(&self) -> Result<()> {
        let _busy = self.inner.busy.lock().await;
        if self.is_held() {
            return Err("The connection is paused.".into());
        }
        self.inner.set_status(Status::Updating);
        self.drop_connection();
        let updated = self.inner.link.update().await;
        self.inner.wake.notify_one();
        updated
    }

    /// Stops connecting, without forgetting anything: the connection is dropped, no new one is tried, and the status is
    /// [`Status::Closed`] until [`Remote::resume`]. For a cloud machine put to sleep, whose link would otherwise wake
    /// it again on the next try. What the webview holds of each session is kept, so a resume catches up as after any
    /// drop.
    pub fn hold(&self) {
        self.inner.held.store(true, Ordering::SeqCst);
        self.drop_connection();
        self.inner.set_status(Status::Closed);
        self.inner.wake.notify_one();
    }

    /// After holding the relay, wait for any connection or update already opening the link to finish.
    pub async fn wait_for_link(&self) {
        let _busy = self.inner.busy.lock().await;
    }

    /// Connects again after [`Remote::hold`], at once.
    pub fn resume(&self) {
        if self.is_held() {
            self.inner.set_status(Status::Connecting { attempt: 1 });
        }
        self.inner.held.store(false, Ordering::SeqCst);
        self.inner.wake.notify_one();
    }

    /// Whether it is held ([`Remote::hold`]).
    pub fn is_held(&self) -> bool {
        self.inner.held.load(Ordering::SeqCst)
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Stops for good: the connection is dropped (the host and its sessions keep running).
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.drop_connection();
        self.inner.wake.notify_one();
        self.inner.set_status(Status::Closed);
    }
}

impl Inner {
    fn set_status(&self, status: Status) {
        if self.closed.load(Ordering::SeqCst) && status != Status::Closed {
            return;
        }
        if let Ok(mut current) = self.status.lock() {
            if *current == status {
                return;
            }
            *current = status.clone();
        }
        self.sink.status(&status);
    }

    fn current_connection(&self, connection: &Connection) -> bool {
        !self.held.load(Ordering::SeqCst)
            && self.current.lock().is_ok_and(|current| {
                current
                    .as_ref()
                    .is_some_and(|current| std::ptr::eq(current.as_ref(), connection))
            })
    }

    /// Passes `push` on, unless it is a session's event the webview already has.
    fn deliver(&self, push: Pushed) {
        if push.event == "wb:event" {
            let thread = push.payload.get("threadId").and_then(Value::as_str);
            let seq = push.payload.get("seq").and_then(Value::as_i64);
            if let (Some(thread), Some(seq)) = (thread, seq) {
                let Ok(mut seqs) = self.seqs.lock() else {
                    return;
                };
                let held = seqs.entry(thread.to_string()).or_insert(0);
                if seq <= *held {
                    return;
                }
                *held = seq;
            }
        }
        self.sink.push(&push);
    }

    async fn connect(&self) -> std::result::Result<Session, Failure> {
        let opened = match self.link.reach_host().await {
            Ok(Reached::Stream(opened)) => opened,
            Ok(Reached::Socket(opened)) => return self.connect_socket(opened).await,
            Err(OpenError { message, heals: true }) => return Err(Failure::Other(message)),
            Err(OpenError { message, heals: false }) => return Err(Failure::Stopped(message)),
        };
        let (connection, host, pushed) = match client::open(opened.stream, &opened.token, opened.sign_in.as_ref()).await
        {
            Ok(open) => open,
            Err(HelloError::Protocol { error, host }) => return Err(Failure::NeedsUpdate { host, error }),
            Err(HelloError::Failed(error)) => return Err(Failure::Other(error)),
        };
        let mut update = None;
        if let Some(wanted) = opened.version.filter(|wanted| replaces(wanted, &host.version)) {
            let status = connection.call("host_status", Value::Null).await;
            let idle = status
                .ok()
                .and_then(|s| s.get("sessions").and_then(Value::as_array).map(Vec::is_empty))
                .unwrap_or(false);
            // The latch records an attempt, so only an idle host (one we really try) takes it.
            let tried = !idle
                || self
                    .silent_tried
                    .lock()
                    .map(|mut tried| tried.replace(wanted.clone()) == Some(wanted.clone()))
                    .unwrap_or(true);
            if idle && !tried {
                drop(connection);
                self.set_status(Status::Updating);
                if let Err(error) = self.link.update().await {
                    eprintln!("nexus-remote: silent update to {wanted} failed: {error}");
                }
                return Err(Failure::Again);
            }
            update = Some(wanted);
        }
        Ok(Session {
            connection: Arc::new(connection),
            pushed,
            host,
            update,
            _keep: opened.keep,
        })
    }

    /// A host through Gateway: the hello, then the setup calls. Its version is the machine image's, so no update.
    async fn connect_socket(&self, opened: SocketOpened) -> std::result::Result<Session, Failure> {
        let (connection, host, pushed) = match client::open_socket(opened.socket, &opened.token, None, true).await {
            Ok(open) => open,
            Err(HelloError::Protocol { error, host }) => return Err(Failure::NeedsUpdate { host, error }),
            Err(HelloError::Failed(error)) => return Err(Failure::Other(error)),
        };
        for (call, args) in opened.setup {
            if let Err(error) = connection.call(&call, args).await {
                return Err(Failure::Other(format!("The machine's setup failed: {error}")));
            }
        }
        Ok(Session {
            connection: Arc::new(connection),
            pushed,
            host,
            update: None,
            _keep: None,
        })
    }

    /// Every session the webview holds, read past its last event, before anything live.
    async fn catch_up(&self, connection: &Connection) {
        let held: Vec<(String, i64)> = self
            .seqs
            .lock()
            .map(|seqs| seqs.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        for (thread, after) in held {
            let answer = connection
                .call("wb_thread_events", json!({ "threadId": thread, "afterSeq": after }))
                .await;
            if !self.current_connection(connection) {
                return;
            }
            match answer {
                Ok(Value::Array(events)) => {
                    for event in events {
                        self.deliver(Pushed {
                            event: "wb:event".into(),
                            payload: event,
                        });
                    }
                }
                Ok(_) => {}
                Err(_) if !connection.is_open() => return,
                // Deleted while away, say: nothing to catch up.
                Err(_) => {
                    if let Ok(mut seqs) = self.seqs.lock() {
                        seqs.remove(&thread);
                    }
                }
            }
        }
    }
}

async fn run(inner: Arc<Inner>) {
    let mut delay = inner.backoff.first;
    let mut attempt = 0u32;
    loop {
        if inner.closed.load(Ordering::SeqCst) {
            return;
        }
        if inner.held.load(Ordering::SeqCst) {
            inner.set_status(Status::Closed);
            inner.wake.notified().await;
            delay = inner.backoff.first;
            attempt = 0;
            continue;
        }
        attempt += 1;
        let busy = inner.busy.lock().await;
        if inner.held.load(Ordering::SeqCst) || inner.closed.load(Ordering::SeqCst) {
            continue;
        }
        inner.set_status(Status::Connecting { attempt });
        let (tried, busy) = match inner.connect().await {
            Ok(session) => (Ok(session), Some(busy)),
            Err(error) => {
                drop(busy);
                (Err(error), None)
            }
        };
        match tried {
            Ok(mut session) => {
                let started = Instant::now();
                if let Ok(mut current) = inner.current.lock() {
                    *current = Some(session.connection.clone());
                }
                if inner.closed.load(Ordering::SeqCst) {
                    session.connection.abort();
                    return;
                }
                // Held while the link was opening: the connection is not used.
                if inner.held.load(Ordering::SeqCst) {
                    session.connection.abort();
                    if let Ok(mut current) = inner.current.lock() {
                        *current = None;
                    }
                    continue;
                }
                inner.set_status(Status::Connected {
                    host: session.host.clone(),
                    update: session.update.clone(),
                });
                // A paused link stays busy until the old connection is installed or discarded.
                drop(busy);
                inner.catch_up(&session.connection).await;
                while let Some(push) = session.pushed.recv().await {
                    if !inner.current_connection(&session.connection) {
                        break;
                    }
                    inner.deliver(push);
                }
                if let Ok(mut current) = inner.current.lock()
                    && current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &session.connection))
                {
                    *current = None;
                }
                // A session that ended quickly waits like a failed connect, so a host that drops the stream at
                // once does not cost a full ensure over ssh in a tight loop.
                if inner.manual_retry {
                    inner.set_status(Status::Unreachable {
                        error: "The connection to this computer closed. Click Retry to reconnect.".into(),
                        retry_in_secs: 0,
                    });
                    inner.wake.notified().await;
                    attempt = 0;
                } else if started.elapsed() >= inner.backoff.healthy {
                    delay = inner.backoff.first;
                    attempt = 0;
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = inner.wake.notified() => {}
                    }
                    delay = (delay * 2).min(inner.backoff.max);
                }
            }
            Err(Failure::Again) => {}
            Err(Failure::NeedsUpdate { host, error }) => {
                inner.set_status(Status::NeedsUpdate { host: *host, error });
                inner.wake.notified().await;
            }
            // Held while the link was opening (it may have failed because of it): no error to show.
            Err(Failure::Other(_) | Failure::Stopped(_)) if inner.held.load(Ordering::SeqCst) => {}
            Err(Failure::Stopped(error)) => {
                inner.set_status(Status::Unreachable {
                    error,
                    retry_in_secs: 0,
                });
                inner.wake.notified().await;
                delay = inner.backoff.first;
                attempt = 0;
            }
            Err(Failure::Other(error)) if inner.manual_retry => {
                inner.set_status(Status::Unreachable {
                    error,
                    retry_in_secs: 0,
                });
                inner.wake.notified().await;
                attempt = 0;
            }
            Err(Failure::Other(error)) => {
                inner.set_status(Status::Unreachable {
                    error,
                    retry_in_secs: delay.as_secs().max(1),
                });
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = inner.wake.notified() => {}
                }
                delay = (delay * 2).min(inner.backoff.max);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Collect(Mutex<Vec<Pushed>>);

    impl Sink for Collect {
        fn push(&self, push: &Pushed) {
            if let Ok(mut pushed) = self.0.lock() {
                pushed.push(push.clone());
            }
        }
        fn status(&self, _: &Status) {}
    }

    struct Nowhere;

    impl Link for Nowhere {
        fn open(&self) -> BoxFuture<'_, Result<Opened>> {
            Box::pin(async { Err("no route to host".to_string()) })
        }
        fn update(&self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn event(thread: &str, seq: i64) -> Pushed {
        Pushed {
            event: "wb:event".into(),
            payload: json!({ "threadId": thread, "seq": seq }),
        }
    }

    #[tokio::test]
    async fn passes_each_sessions_event_once_and_in_order() {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let remote = Remote::start(Arc::new(Nowhere), sink.clone(), Backoff::default());
        for push in [
            event("a", 1),
            event("a", 2),
            event("b", 1),
            event("a", 2),
            event("a", 1),
            event("a", 3),
            Pushed {
                event: "wb:projects".into(),
                payload: json!([]),
            },
        ] {
            remote.inner.deliver(push);
        }
        let got: Vec<(String, Value, Value)> = sink
            .0
            .lock()
            .map(|p| {
                p.iter()
                    .map(|p| (p.event.clone(), p.payload["threadId"].clone(), p.payload["seq"].clone()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got,
            vec![
                ("wb:event".into(), json!("a"), json!(1)),
                ("wb:event".into(), json!("a"), json!(2)),
                ("wb:event".into(), json!("b"), json!(1)),
                ("wb:event".into(), json!("a"), json!(3)),
                ("wb:projects".into(), Value::Null, Value::Null),
            ]
        );
        assert!(remote.holds("a") && remote.holds("b"));
        assert!(!remote.holds("c"), "a session it never carried");
        remote.close();
    }

    /// Counts its tries; never opens.
    struct Counting(std::sync::atomic::AtomicU32);

    impl Link for Counting {
        fn open(&self) -> BoxFuture<'_, Result<Opened>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("asleep".to_string()) })
        }
        fn update(&self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn manual_retry_makes_one_attempt_per_click() {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let link = Arc::new(Counting(std::sync::atomic::AtomicU32::new(0)));
        let remote = Remote::start_manual(
            link.clone(),
            sink,
            Backoff {
                first: Duration::from_millis(5),
                max: Duration::from_millis(5),
                healthy: Duration::from_secs(60),
            },
        );
        for expected in 1..=3 {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !matches!(remote.status(), Status::Unreachable { retry_in_secs: 0, .. })
                    || link.0.load(Ordering::SeqCst) != expected
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("one attempt finishes");
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(link.0.load(Ordering::SeqCst), expected, "retried without a click");
            if expected < 3 {
                remote.reconnect();
            }
        }
        remote.close();
    }

    #[tokio::test]
    async fn a_held_remote_stops_trying_until_resumed() {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let link = Arc::new(Counting(std::sync::atomic::AtomicU32::new(0)));
        let backoff = Backoff {
            first: Duration::from_millis(10),
            max: Duration::from_millis(10),
            healthy: Duration::from_secs(60),
        };
        let remote = Remote::start(link.clone(), sink, backoff);
        tokio::time::sleep(Duration::from_millis(60)).await;
        remote.hold();
        assert!(remote.is_held());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(remote.status(), Status::Closed);
        let tries = link.0.load(Ordering::SeqCst);
        assert!(tries >= 1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(link.0.load(Ordering::SeqCst), tries, "a held remote kept trying");
        remote.resume();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            link.0.load(Ordering::SeqCst) > tries,
            "a resumed remote did not try again"
        );
        assert!(matches!(
            remote.status(),
            Status::Unreachable { .. } | Status::Connecting { .. }
        ));
        remote.close();
    }

    /// Fails with an error waiting cannot fix, counting its tries.
    struct NoBuild(std::sync::atomic::AtomicU32);

    impl Link for NoBuild {
        fn open(&self) -> BoxFuture<'_, Result<Opened>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("The Nexus host 0.9.0 has no Linux riscv64 build.".to_string()) })
        }
        fn update(&self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn says_which_errors_heal() {
        assert!(heals("no route to host"));
        assert!(heals("The Nexus host on box did not start: log"));
        assert!(heals(
            "The Nexus host 0.9.0 is not available: Could not reach downloads: timed out"
        ));
        assert!(!heals("The Nexus host 0.9.0 is not available: HTTP 404"));
        assert!(!heals("The Nexus host 0.9.0 is not available: Not Found"));
        assert!(!heals("The Nexus host 0.9.0 is not available for riscv64."));
        assert!(!heals("The Nexus host 0.9.0 has no Linux x86_64 build."));
        assert!(!heals(
            "box runs Darwin on arm64: the Nexus host runs on Linux (x86-64 or arm64) for now."
        ));
    }

    #[tokio::test]
    async fn an_error_that_does_not_heal_stops_the_tries_until_try_again() {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let link = Arc::new(NoBuild(std::sync::atomic::AtomicU32::new(0)));
        let backoff = Backoff {
            first: Duration::from_millis(5),
            max: Duration::from_millis(5),
            healthy: Duration::from_secs(60),
        };
        let remote = Remote::start(link.clone(), sink, backoff);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(link.0.load(Ordering::SeqCst), 1, "it tried again");
        assert!(
            matches!(remote.status(), Status::Unreachable { retry_in_secs: 0, ref error } if error.contains("no Linux")),
            "{:?}",
            remote.status()
        );
        remote.reconnect();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(link.0.load(Ordering::SeqCst), 2, "Try again did not try");
        remote.close();
    }

    #[test]
    fn a_release_app_keeps_an_internal_host_of_its_line() {
        assert!(!replaces("0.10.0+abc", "0.10.0+abc"));
        // A release build meets an employee's internal host of the same line: it keeps it.
        assert!(!replaces("0.10.0+abc", "0.10.0-internal.42+def"));
        // The internal app meets the release host: it installs its own, once, and the release app keeps that.
        assert!(replaces("0.10.0-internal.42+def", "0.10.0+abc"));
        // Builds of one kind, and other lines, still replace each other.
        assert!(replaces("0.10.0-internal.43+aaa", "0.10.0-internal.42+def"));
        assert!(replaces("0.10.0+abc", "0.10.0+fed"));
        assert!(replaces("0.10.0+abc", "0.9.0-internal.7+def"));
        assert!(replaces("0.10.0+abc", "0.10.1-internal.1+def"));
    }

    #[test]
    fn this_computers_own_commands_are_not_relayed() {
        for call in LOCAL_ONLY {
            assert!(call.starts_with("wb_"));
            assert!(relayable(call).is_err());
        }
        assert!(LOCAL_ONLY.contains(&"wb_viewing"));
        assert_eq!(relayable("wb_send"), Ok(()));
        assert_eq!(relayable("wb_thread_stream"), Ok(()));
        assert_eq!(
            relayable("host_status"),
            Err("host_status is not a workbench command.".into())
        );
    }

    #[tokio::test]
    async fn waits_longer_after_each_failure_and_says_why() {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let backoff = Backoff {
            first: Duration::from_millis(20),
            max: Duration::from_millis(40),
            healthy: Duration::from_secs(60),
        };
        let remote = Remote::start(Arc::new(Nowhere), sink, backoff);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            matches!(remote.status(), Status::Unreachable { ref error, .. } if error == "no route to host"),
            "{:?}",
            remote.status()
        );
        let refused = remote.call("wb_overview", json!({})).await;
        assert_eq!(refused, Err("Can't reach this machine: no route to host".into()));
        remote.close();
        assert_eq!(remote.status(), Status::Closed);
        let status = serde_json::to_value(Status::Unreachable {
            error: "x".into(),
            retry_in_secs: 2,
        })
        .unwrap_or_default();
        assert_eq!(
            status,
            json!({ "state": "unreachable", "error": "x", "retryInSecs": 2 })
        );
    }
}
