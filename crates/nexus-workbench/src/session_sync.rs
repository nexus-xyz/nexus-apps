//! Durable runner-side session sync. SQLite remains authoritative. A host supplies the enrolled
//! account's transport and trusted crypto; this module defines no speculative Gateway routes or replacement crypto.
//! The default Host supplies neither, so shipping this engine does not enable content uploads.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use base64::Engine;
use futures::future::BoxFuture;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::{Result, Workbench};

pub const INTERVAL: Duration = Duration::from_millis(250);
/// The phone control loop's poll of this computer's inbox; a live command wakes it sooner.
const CONTROL_INTERVAL: Duration = Duration::from_secs(2);
/// A live command stream open this long was healthy when it ended: it is reopened without a growing wait.
const WATCH_HEALTHY: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A thread that keeps failing waits at most this long between attempts. 32 s let ten stuck threads poll the Gateway
/// every few seconds for days.
const MAX_RETRY: Duration = Duration::from_secs(300);
/// While this device waits for its person to confirm its approver, the whole pass waits this long between tries:
/// every thread fails the same way, so one directory read stands for all of them.
const APPROVER_WAIT: Duration = Duration::from_secs(60);
pub(crate) const MAX_EVENTS: usize = 32;
pub(crate) const MAX_PLAINTEXT: usize = 256 * 1024;
pub(crate) const MAX_BATCH_BYTES: usize = 512 * 1024;
const MAX_THREADS: usize = 16;
const KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(15);
pub(crate) const MAX_IDLE_THREADS: usize = 2;

pub mod approver;
mod contract;
pub use contract::*;
pub mod control;
pub mod control_loop;
mod gateway;
mod keystore;
pub mod org_key;
pub mod slot;
mod trust;
pub use gateway::GatewayBackend;
pub use keystore::KeyStore;

/// No free-form errors or content enter reports, logs, or plaintext metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Unavailable,
    LocalStore,
    InvalidAcknowledgement,
    IdentityChanged,
    PolicyChanged,
    InvalidEpoch,
    TooLarge,
    InvalidMetadata,
    /// Gateway refused events sealed with an older key ([`KEY_ROTATED`]); the next pass seals them again.
    KeyRotated,
    /// The person's synced threads are full ([`STORE_FULL`]).
    StoreFull,
    /// This device has an approver the person has not confirmed on this computer. Nothing syncs until they confirm it
    /// (Settings › Devices, or `nexus devices pair`).
    ApproverUnconfirmed,
}

#[derive(Debug, Default)]
pub struct Report {
    pub confirmed_events: usize,
    pub deleted_threads: usize,
    pub failures: Vec<(String, FailureKind)>,
}

#[derive(Default)]
pub(crate) struct Coordinator {
    retry: HashMap<String, (u32, Instant)>,
    next_keys: Option<Instant>,
    /// Set when a pass met [`FailureKind::ApproverUnconfirmed`]: no pass runs before it, and any success clears it.
    approver_wait: Option<Instant>,
}

#[derive(Clone)]
pub(crate) struct Target {
    pub local: String,
    pub identity: Identity,
    pub sync_id: String,
    pub confirmed: i64,
    pub epoch: Option<u64>,
    pub deleted: bool,
}

pub(crate) struct RawEvent {
    pub seq: i64,
    pub at: i64,
    pub body: String,
}

/// Kept inside each sealed event so reading a page from the tail also recovers the thread's private metadata.
/// An idle rename appears with the next stored event; metadata-only updates require the final viewer contract.
#[derive(Serialize, Deserialize)]
pub(crate) struct PrivateThread {
    pub local_id: String,
    pub title: String,
    pub project_name: String,
    pub project_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::computer::Origin>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Plaintext {
    pub version: u8,
    pub at: i64,
    pub thread: PrivateThread,
    /// Exact SQLite JSON, including fields this runner does not understand. Deltas never enter SQLite.
    pub event_json: String,
}

/// Whether Gateway's session store takes this agent's sessions: the agents its `SyncedAgent` names, as each agent's
/// harness says ([`crate::harness::Harness::syncs`]).
pub(crate) fn syncs(agent: crate::event::SessionAgent) -> bool {
    crate::harness::harness(agent).syncs()
}

type SyncResult<T> = std::result::Result<T, FailureKind>;

async fn remote<T>(call: BoxFuture<'_, Result<T>>) -> SyncResult<T> {
    tokio::time::timeout(REQUEST_TIMEOUT, call)
        .await
        .map_err(|_| FailureKind::Unavailable)?
        .map_err(|error| match error.as_str() {
            KEY_ROTATED => FailureKind::KeyRotated,
            STORE_FULL => FailureKind::StoreFull,
            trust::APPROVER_UNCONFIRMED => FailureKind::ApproverUnconfirmed,
            _ => FailureKind::Unavailable,
        })
}

fn local<T>(value: Result<T>) -> SyncResult<T> {
    value.map_err(|_| FailureKind::LocalStore)
}

impl Workbench {
    /// Desktop and headless hosts run this once after opening their workbench. Shutdown cancels an in-flight pass;
    /// an uncertain upload remains staged and resumes on the next launch.
    pub async fn run_session_sync_until_shutdown(&self) {
        self.run_session_sync(self.inner.session_sync_stop.subscribe()).await;
    }
    /// Bind only a newly inserted thread, before it has events. Legacy logs have no verified owner and stay local.
    pub(crate) fn session_sync_identity(&self, scope: &str) -> Option<Identity> {
        let backend = self.inner.host.session_sync()?;
        (self.persistent() && backend.identity().scope == scope && self.sync_current(backend.as_ref()))
            .then(|| backend.identity().clone())
    }

    pub(crate) fn inherited_sync_identity(&self, source: &crate::store::ThreadRecord) -> Option<Identity> {
        let target = {
            let store = self.store();
            let current = store.thread(&source.summary.id).ok()??;
            if current.native_id != source.native_id {
                return None;
            }
            store.sync_target(&source.summary.id).ok()??
        };
        (self.session_sync_identity(&target.identity.scope).as_ref() == Some(&target.identity))
            .then_some(target.identity)
    }

    pub(crate) fn session_sync_lock(&self, thread: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .session_sync_threads
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(thread).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(thread.to_string(), std::sync::Arc::downgrade(&lock));
        lock
    }

    pub(crate) fn register_session_sync(&self, thread: &str, identity: Option<&Identity>) {
        let Some(identity) = identity else {
            return;
        };
        let Some(backend) = self.inner.host.session_sync() else {
            return;
        };
        if self.persistent() && backend.identity() == identity && self.sync_current(backend.as_ref()) {
            // Failure leaves the local session usable and unowned; no later pass adopts it implicitly.
            let _ = self.store().sync_register(thread, backend.identity());
        }
    }

    pub(crate) fn sync_current(&self, backend: &dyn Backend) -> bool {
        backend.allow_local_cache()
            && self.config().scope == backend.identity().scope
            && self
                .inner
                .host
                .session_sync()
                .is_some_and(|current| current.allow_local_cache() && current.identity() == backend.identity())
    }

    pub(crate) async fn sync_policy(&self, backend: &dyn Backend) -> SyncResult<ContentPolicy> {
        if !self.sync_current(backend) {
            return Err(FailureKind::IdentityChanged);
        }
        let policy = remote(backend.policy()).await?.content;
        if !self.sync_current(backend) {
            return Err(FailureKind::IdentityChanged);
        }
        Ok(policy)
    }

    /// Whether session sync waits for the person to confirm this device's approver, so the host can say
    /// why nothing syncs. It stays true until a thread uploads again.
    pub async fn session_sync_waits_for_approver(&self) -> bool {
        self.inner.session_sync.lock().await.approver_wait.is_some()
    }

    /// One pass with bounded batches. Per-thread locks order deletion; independent threads progress concurrently.
    /// Hosts may drive this explicitly or call `run_session_sync`. No network runs for an in-memory store.
    pub async fn sync_sessions_once(&self) -> Report {
        let mut report = Report::default();
        if !self.persistent() {
            return report;
        }
        let Some(backend) = self.inner.host.session_sync() else {
            return report;
        };
        if !self.sync_current(backend.as_ref()) {
            return report;
        }
        let mut coordinator = self.inner.session_sync.lock().await;
        let mut targets = match self.store().sync_targets(backend.identity()) {
            Ok(targets) => targets,
            Err(_) => {
                report.failures.push((String::new(), FailureKind::LocalStore));
                return report;
            }
        };
        let now = Instant::now();
        if coordinator.approver_wait.is_some_and(|due| due > now) {
            report.failures.push((String::new(), FailureKind::ApproverUnconfirmed));
            return report;
        }
        if targets.len() < MAX_THREADS && coordinator.next_keys.is_none_or(|due| due <= now) {
            // Uploads and deletions keep priority. Idle history advances in a bounded, oldest-attempt-first wave.
            match self.store().sync_idle_targets(backend.identity()) {
                Ok(idle) => targets.extend(
                    idle.into_iter()
                        .take((MAX_THREADS - targets.len()).min(MAX_IDLE_THREADS)),
                ),
                Err(_) => report.failures.push((String::new(), FailureKind::LocalStore)),
            }
            coordinator.next_keys = Some(now + KEY_REFRESH_INTERVAL);
        }
        {
            let active: HashSet<_> = targets.iter().map(|target| target.sync_id.as_str()).collect();
            coordinator.retry.retain(|sync_id, _| active.contains(sync_id.as_str()));
        }
        let targets = targets
            .into_iter()
            .filter(|target| {
                coordinator
                    .retry
                    .get(&target.sync_id)
                    .is_none_or(|(_, due)| *due <= now)
            })
            .take(MAX_THREADS)
            .collect::<Vec<_>>();
        let mut completed = stream::iter(targets.into_iter().map(|target| {
            let backend = backend.clone();
            async move {
                let outcome = self.sync_thread(backend.as_ref(), &target).await;
                (target, outcome)
            }
        }))
        .buffer_unordered(4);
        while let Some((target, outcome)) = completed.next().await {
            match outcome {
                Ok(confirmed) => {
                    coordinator.retry.remove(&target.sync_id);
                    // Only an upload proves the key step works again; a deletion or an empty pass needs no key.
                    if confirmed > 0 {
                        coordinator.approver_wait = None;
                    }
                    report.confirmed_events += confirmed;
                    report.deleted_threads += usize::from(target.deleted);
                }
                Err(kind) => {
                    if kind == FailureKind::ApproverUnconfirmed {
                        // The pass-wide wait is the only throttle: no per-thread backoff builds up behind it, so the
                        // first pass after the person confirms syncs every thread.
                        coordinator.approver_wait = Some(Instant::now() + APPROVER_WAIT);
                        coordinator.retry.remove(&target.sync_id);
                        report.failures.push((target.local, kind));
                        continue;
                    }
                    let failures = coordinator.retry.get(&target.sync_id).map_or(0, |(count, _)| *count);
                    let delay = INTERVAL.saturating_mul(1 << failures.min(20)).min(MAX_RETRY);
                    coordinator.retry.insert(
                        target.sync_id.clone(),
                        (failures.saturating_add(1), Instant::now() + delay),
                    );
                    report.failures.push((target.local, kind));
                }
            }
        }
        report
    }

    /// Run until the host asks it to stop. Final events remain in SQLite if quit happens while offline.
    /// The host owns this task; `stop` must be sent before its runtime shuts down.
    pub async fn run_session_sync(&self, stop: tokio::sync::watch::Receiver<bool>) {
        self.run_sync(stop, true).await;
    }

    /// Uploads only. A host that answers phone commands for its own thread only, as a CLI run does, runs
    /// `control_thread_once` itself: this computer's other threads belong to other runs.
    pub async fn run_session_uploads(&self, stop: tokio::sync::watch::Receiver<bool>) {
        self.run_sync(stop, false).await;
    }

    async fn run_sync(&self, mut stop: tokio::sync::watch::Receiver<bool>, control: bool) {
        if *stop.borrow() {
            return;
        }
        let uploads = async {
            let mut ticks = tokio::time::interval(INTERVAL);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticks.tick().await;
                self.sync_sessions_once().await;
            }
        };
        let wake = std::sync::Arc::new(tokio::sync::Notify::new());
        let controls = async {
            if !control {
                return std::future::pending::<()>().await;
            }
            let mut ticks = tokio::time::interval(CONTROL_INTERVAL);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                // A command queued for this computer wakes the loop at once. The tick stays the backstop.
                tokio::select! {
                    _ = ticks.tick() => {},
                    () = wake.notified() => {},
                }
                self.control_sessions_once().await;
            }
        };
        // Each worker keeps progressing when the other's Gateway request is slow. Dropping both futures on
        // shutdown cancels in-flight requests; durable outboxes and command receipts survive the next launch.
        tokio::select! {
            biased;
            _ = stop.changed() => {},
            _ = uploads => {},
            _ = controls => {},
            () = self.watch_control(wake.clone()) => {},
        }
    }

    /// Phone control's wake-up: while this computer takes phone commands, follow the person's live command
    /// states and wake the control loop when one is queued here. The stream is reopened after it ends, with a growing
    /// wait while it fails; a Gateway without it leaves the loop on its poll. The account or the local opt-in changing
    /// closes it within [`CONTROL_INTERVAL`].
    async fn watch_control(&self, wake: std::sync::Arc<tokio::sync::Notify>) {
        let mut failures: u32 = 0;
        loop {
            let Some(backend) = self
                .inner
                .host
                .session_sync()
                .filter(|backend| self.control_watched(backend.as_ref()))
            else {
                failures = 0;
                tokio::time::sleep(CONTROL_INTERVAL).await;
                continue;
            };
            let opened = Instant::now();
            let changed = async {
                loop {
                    tokio::time::sleep(CONTROL_INTERVAL).await;
                    if !self.control_watched(backend.as_ref()) {
                        return;
                    }
                }
            };
            // A stream that ran a while was healthy, even when it ended on the client's own time limit.
            let healthy = tokio::select! {
                _ = backend.control_watch(wake.clone()) => opened.elapsed() >= WATCH_HEALTHY,
                () = changed => true,
            };
            failures = if healthy { 0 } else { failures.saturating_add(1) };
            tokio::time::sleep(CONTROL_INTERVAL * (1 << failures.min(5))).await;
        }
    }

    /// Whether `backend` is the current account's and this computer takes phone commands, as the control loop checks.
    fn control_watched(&self, backend: &dyn Backend) -> bool {
        self.persistent()
            && backend.supports_control()
            && self.sync_current(backend)
            && self
                .store()
                .control_enabled_in(backend.identity(), backend.device_trust())
                .unwrap_or(false)
    }

    async fn sync_thread(&self, backend: &dyn Backend, target: &Target) -> SyncResult<usize> {
        let lock = self.session_sync_lock(&target.local);
        let _running = lock.lock().await;
        if !target.deleted {
            let current = local(self.store().sync_target(&target.local))?;
            // A delete or keyed recreation while queued cannot upload this stale incarnation.
            if current.as_ref().is_none_or(|current| current.sync_id != target.sync_id) {
                return Ok(0);
            }
        }
        local(self.store().sync_touch(target))?;
        if target.deleted {
            if !self.sync_current(backend) {
                return Err(FailureKind::IdentityChanged);
            }
            remote(backend.delete(&target.sync_id)).await?;
            local(self.store().sync_forget(target))?;
            return Ok(0);
        }
        let mut policy = self.sync_policy(backend).await?;
        // Resolve an uncertain commit before using a new key. A lost response must not change a retried payload.
        let head = remote(backend.head(&target.sync_id)).await?;
        if !self.sync_current(backend) {
            return Err(FailureKind::IdentityChanged);
        }
        let confirmed = self.store().sync_reconcile(target, head)?;
        let target = local(self.store().sync_target(&target.local))?.ok_or(FailureKind::LocalStore)?;
        let pending = local(self.store().sync_pending(&target.local))?;
        let batch = match pending {
            Some(batch) => {
                if batch.policy != policy {
                    // Do not replace uncertain ciphertext with usage-only rows, or silently wrap it for new readers.
                    return Err(FailureKind::PolicyChanged);
                }
                batch
            }
            None => {
                let (header, agent, rows) = self.store().sync_raw(&target)?;
                if rows.is_empty() {
                    if target.epoch.is_some() && policy != ContentPolicy::NeverLeaves {
                        // Pairing can happen after the final event. Reuse the verified key path without an append.
                        let key = remote(backend.key(&target.sync_id, target.epoch, policy)).await?;
                        validate_epoch(&target, key.epoch())?;
                        if self.sync_policy(backend).await? != policy {
                            return Err(FailureKind::PolicyChanged);
                        }
                        self.store().sync_epoch(&target, key.epoch())?;
                    }
                    return Ok(confirmed);
                }
                // Refuse invalid observations and an oversized first payload before publishing locked keys.
                for row in &rows {
                    metadata(&row.body)?;
                }
                if policy != ContentPolicy::NeverLeaves {
                    plaintext(&header, &rows[0])?;
                }
                let key = if policy == ContentPolicy::NeverLeaves {
                    None
                } else {
                    let key = remote(backend.key(&target.sync_id, target.epoch, policy)).await?;
                    validate_epoch(&target, key.epoch())?;
                    Some(key)
                };
                let batch = seal_batch(&target, header, agent, rows, policy, key.as_deref())?;
                local(self.store().sync_stage(&target, &batch))?;
                batch
            }
        };
        // Policy and identity may change during key preparation or a long offline recovery.
        policy = self.sync_policy(backend).await?;
        if policy != batch.policy {
            return Err(FailureKind::PolicyChanged);
        }
        let through = match remote(backend.append(batch.clone())).await {
            Err(FailureKind::KeyRotated) => {
                // Gateway refuses a whole upload and stores none of it, so its new events are certainly not stored.
                // Only this definite refusal replaces staged ciphertext; an uncertain failure still retries it unchanged.
                let head = remote(backend.head(&target.sync_id)).await?;
                if !self.sync_current(backend) {
                    return Err(FailureKind::IdentityChanged);
                }
                self.store().sync_reconcile(&target, head)?;
                let target = local(self.store().sync_target(&target.local))?.ok_or(FailureKind::LocalStore)?;
                local(self.store().sync_unstage(&target))?;
                return Err(FailureKind::KeyRotated);
            }
            other => other?,
        };
        if !self.sync_current(backend) {
            return Err(FailureKind::IdentityChanged);
        }
        let advanced = self.store().sync_ack(&target, through)?;
        Ok(confirmed + advanced)
    }
}

fn validate_epoch(target: &Target, epoch: u64) -> SyncResult<()> {
    if epoch > i64::MAX as u64
        || target.epoch.is_none() && epoch != 0
        || target.epoch.is_some_and(|previous| epoch < previous)
    {
        return Err(FailureKind::InvalidEpoch);
    }
    Ok(())
}

fn seal_batch(
    target: &Target,
    header: PrivateThread,
    agent: crate::event::SessionAgent,
    rows: Vec<RawEvent>,
    policy: ContentPolicy,
    key: Option<&dyn ContentKey>,
) -> SyncResult<Batch> {
    let mut batch = Batch {
        thread: target.sync_id.clone(),
        policy,
        events: Vec::new(),
    };
    let mut size = 256;
    for row in rows {
        let seq = u64::try_from(row.seq - 1).map_err(|_| FailureKind::LocalStore)?;
        let (usage, status) = metadata(&row.body)?;
        let sealed = match key {
            Some(key) => {
                let bytes = match plaintext(&header, &row) {
                    Ok(bytes) => bytes,
                    Err(FailureKind::TooLarge) if !batch.events.is_empty() => break,
                    Err(error) => return Err(error),
                };
                let sealed = key
                    .seal(&target.sync_id, seq, &bytes)
                    .map_err(|_| FailureKind::Unavailable)?;
                if sealed.len() > MAX_PLAINTEXT + 1024 {
                    return Err(FailureKind::TooLarge);
                }
                Some(base64::engine::general_purpose::STANDARD.encode(sealed))
            }
            None => None,
        };
        let event = Event {
            seq,
            at: row.at,
            agent,
            usage,
            status,
            epoch: key.map(ContentKey::epoch),
            sealed,
        };
        let bytes = serde_json::to_vec(&event).map_err(|_| FailureKind::LocalStore)?.len();
        if size + bytes > MAX_BATCH_BYTES {
            if batch.events.is_empty() {
                return Err(FailureKind::TooLarge);
            }
            break;
        }
        size += bytes;
        batch.events.push(event);
    }
    Ok(batch)
}

fn plaintext(header: &PrivateThread, row: &RawEvent) -> SyncResult<Vec<u8>> {
    let plaintext = Plaintext {
        version: 1,
        at: row.at,
        thread: PrivateThread {
            origin: header.origin.clone(),
            local_id: header.local_id.clone(),
            title: header.title.clone(),
            project_name: header.project_name.clone(),
            project_path: header.project_path.clone(),
        },
        event_json: row.body.clone(),
    };
    let bytes = serde_json::to_vec(&plaintext).map_err(|_| FailureKind::LocalStore)?;
    if bytes.len() > MAX_PLAINTEXT {
        return Err(FailureKind::TooLarge);
    }
    Ok(bytes)
}

fn metadata(body: &str) -> SyncResult<(Option<Usage>, Option<Status>)> {
    let event: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::InvalidMetadata)?;
    let usage = if event["kind"] == "usage" {
        let count = |name: &str| event[name].as_u64().ok_or(FailureKind::InvalidMetadata);
        let model = event["model"]
            .as_str()
            .filter(|model| !model.is_empty() && model.len() <= 256)
            .ok_or(FailureKind::InvalidMetadata)?
            .to_string();
        let cost = event["costUsd"]
            .as_number()
            .filter(|number| number.as_f64().is_some_and(|n| n >= 0.0))
            .ok_or(FailureKind::InvalidMetadata)?;
        Some(Usage {
            model,
            input_tokens: count("inputTokens")?,
            output_tokens: count("outputTokens")?,
            cache_read_tokens: count("cacheReadTokens")?,
            cache_write_tokens: count("cacheWriteTokens")?,
            cost_usd: decimal(&cost.to_string())?,
        })
    } else {
        None
    };
    let status = match event["kind"].as_str() {
        Some("turnStart") => Some(Status::Running),
        Some("turnEnd") => match event["status"].as_str() {
            Some("completed") => Some(Status::Completed),
            Some("interrupted") => Some(Status::Interrupted),
            Some("error") => Some(Status::Error),
            _ => None,
        },
        _ => None,
    };
    Ok((usage, status))
}

/// Preserve JSON's decimal value without introducing exponent notation or an additional floating-point rounding.
fn decimal(number: &str) -> SyncResult<String> {
    let Some((mantissa, exponent)) = number.split_once(['e', 'E']) else {
        return Ok(number.to_string());
    };
    let exponent: i32 = exponent.parse().map_err(|_| FailureKind::InvalidMetadata)?;
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let digits = mantissa.replace('.', "");
    if point <= 0 {
        Ok(format!("0.{}{}", "0".repeat((-point) as usize), digits))
    } else if point as usize >= digits.len() {
        Ok(format!("{}{}", digits, "0".repeat(point as usize - digits.len())))
    } else {
        let (left, right) = digits.split_at(point as usize);
        Ok(format!("{left}.{right}"))
    }
}

#[cfg(test)]
#[path = "session_sync/tests.rs"]
mod tests;

#[cfg(test)]
mod gateway_e2e;
