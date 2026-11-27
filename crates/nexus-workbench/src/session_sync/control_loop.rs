//! The run-once loop for phone commands. The desktop's runner and each `nexus claude` or `nexus codex` run
//! share it. Each verified command is claimed, reserved before its side effect, performed or rejected, then finished.
//! What "performed" means is the runner's own: a [`Performer`].
//!
//! The desktop takes the commands of every synced thread on this computer (`control_sessions_once`). A CLI run takes
//! only its own thread's commands ([`ThreadControl`]): two runs share one device inbox, and neither claims the other's.

use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use super::control::{Action, Command};
use super::{Backend, ContentPolicy, Target, remote};
use crate::{Result, Workbench};

/// How often a thread's lease is published again. Gateway keeps one for 45 seconds.
const RENEW: Duration = Duration::from_secs(15);

/// What a runner does with a verified, reserved command.
pub trait Performer: Send + Sync {
    /// Whether this runner can apply commands now. While it cannot, its lease is disabled and no command is read.
    fn ready(&self) -> bool;
    /// Runs `action` on `local`, the workbench thread. An error rejects the command. Its words stay out of the log.
    fn perform<'a>(&'a self, local: &'a str, waited: Option<i64>, action: Action) -> BoxFuture<'a, Result<()>>;
}

/// The CLI's Phone control switch for `identity` (`nexus devices control`), in the CLI's own store, as the desktop
/// keeps its own. Off until the person turns it on. A CLI run reads it at each pass.
pub fn cli_control(store: &crate::store::Store, identity: &super::Identity) -> Result<bool> {
    store.control_enabled(identity)
}

pub fn set_cli_control(store: &crate::store::Store, identity: &super::Identity, enabled: bool) -> Result<()> {
    store.set_control_enabled(identity, enabled)
}

/// One CLI run's thread, and the lease this process last published for it.
pub struct ThreadControl {
    local: String,
    leased: Option<(Instant, bool)>,
}

impl ThreadControl {
    /// `local` is the run's workbench thread (`cli_thread`).
    pub fn new(local: impl Into<String>) -> Self {
        ThreadControl {
            local: local.into(),
            leased: None,
        }
    }

    /// Whether the last lease this process published was enabled.
    pub fn enabled(&self) -> bool {
        self.leased.is_some_and(|(_, enabled)| enabled)
    }
}

impl Workbench {
    /// One pass for one CLI run: publish its thread's lease, deliver its results, then run its own commands. The lease
    /// is enabled only while Phone control is on here, the content policy allows it and `performer` is ready.
    pub async fn control_thread_once(&self, control: &mut ThreadControl, performer: &dyn Performer) {
        let Some((backend, target)) = self.control_thread_target(control) else {
            return;
        };
        let backend = backend.as_ref();
        let enabled = performer.ready()
            && self
                .store()
                .control_enabled_in(backend.identity(), backend.device_trust())
                .unwrap_or(false)
            && matches!(
                self.sync_policy(backend).await,
                Ok(ContentPolicy::UserOnly | ContentPolicy::UserAndOrgAdmins)
            );
        let due = control
            .leased
            .is_none_or(|(at, was)| was != enabled || at.elapsed() >= RENEW);
        if due && self.sync_current(backend) {
            if remote(backend.control_lease(&target.sync_id, enabled)).await.is_err() {
                return;
            }
            control.leased = Some((Instant::now(), enabled));
        }
        self.control_commands_once(backend, enabled, Some(&target.sync_id), performer)
            .await;
    }

    /// Ends the run's lease, so its closed terminal reads offline at once. Without this call the lease lapses in 45
    /// seconds.
    pub async fn control_thread_end(&self, control: &mut ThreadControl) {
        if control.leased.is_none() {
            return;
        }
        let Some((backend, target)) = self.control_thread_target(control) else {
            return;
        };
        if self.sync_current(backend.as_ref()) && remote(backend.control_lease(&target.sync_id, false)).await.is_ok() {
            control.leased = Some((Instant::now(), false));
        }
    }

    /// The run's thread while it is bound to the person signed in now and Gateway has it.
    fn control_thread_target(&self, control: &ThreadControl) -> Option<(std::sync::Arc<dyn Backend>, Target)> {
        let backend = self.inner.host.session_sync()?;
        if !self.persistent() || !backend.supports_control() || !self.sync_current(backend.as_ref()) {
            return None;
        }
        let target = self.store().sync_target(&control.local).ok().flatten()?;
        (target.identity == *backend.identity() && target.confirmed > 0 && !target.deleted).then_some((backend, target))
    }

    /// Delivers waiting results, then, when `enabled`, reads the inbox and runs each command once. With `only`, takes
    /// only that synced thread's commands and results: another run's command is never claimed.
    pub(super) async fn control_commands_once(
        &self,
        backend: &dyn Backend,
        enabled: bool,
        only: Option<&str>,
        performer: &dyn Performer,
    ) {
        self.control_results(backend, only).await;
        if !enabled || !self.sync_current(backend) {
            return;
        }
        // A failed read is retried on the next pass, as a lease is.
        let Ok(commands) = remote(backend.control_commands()).await else {
            return;
        };
        for command in commands {
            if only.is_some_and(|thread| thread != command.thread_id) {
                continue;
            }
            // Gateway expires it, and refuses its claim.
            if command.expires_at <= crate::now_ms() {
                continue;
            }
            if !self.sync_current(backend) {
                return;
            }
            self.control_one(backend, command, performer).await;
            self.control_results(backend, only).await;
        }
    }

    /// Runs one verified command, or rejects it, and records which. Every command this computer claims gets a result:
    /// rejected when its thread is gone here, Phone control was turned off, its action is invalid, or it failed.
    async fn control_one(&self, backend: &dyn Backend, command: Command, performer: &dyn Performer) {
        let identity = backend.identity();
        let target = match self.store().control_by_sync(identity, &command.thread_id) {
            Ok(target) => target,
            Err(error) => return log(format_args!("the command's session could not be read: {error}")),
        };
        let lock = target.as_ref().map(|target| self.session_sync_lock(&target.local));
        let _guard = match &lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        match remote(backend.control_claim(&command.thread_id, &command.id)).await {
            Ok(true) => {}
            // Claimed before: by this computer, whose receipt reports it, or by another runner.
            Ok(false) => return,
            Err(kind) => return log(format_args!("the command could not be claimed: {kind:?}")),
        }
        // Reserve before the side effect. Even a hostile Gateway replay, or a restart between execution and
        // acknowledgement, cannot run it twice. An interrupted reservation stays uncertain: it is never reported.
        // Bound first: the store's guard is not held across the rejection's request.
        let reserved = self.store().control_reserve(identity, &command.thread_id, &command.id);
        match reserved {
            Ok(true) => {}
            // Reserved before: its own receipt reports it.
            Ok(false) => return,
            Err(error) => {
                // Nothing ran, and with no receipt nothing can: say so now. Gateway delivers a claimed command once.
                log(format_args!(
                    "the command could not be reserved, so it is rejected: {error}"
                ));
                if let Err(kind) = remote(backend.control_finish(&command.thread_id, &command.id, false)).await {
                    log(format_args!("the rejection was not delivered: {kind:?}"));
                }
                return;
            }
        }
        let now = crate::now_ms();
        let completed = match target {
            Some(target)
                if command.action.valid()
                    && command.fresh(now)
                    && performer.ready()
                    && self.control_current(backend, &target, &command.sender_device_id) =>
            {
                // The error's words can come from the agent: they stay out of the log.
                let done = performer
                    .perform(&target.local, command.waited(now), command.action)
                    .await
                    .is_ok();
                if !done {
                    log("the command could not run, so it is rejected");
                }
                done
            }
            _ => false,
        };
        if let Err(error) = self
            .store()
            .control_complete(identity, &command.thread_id, &command.id, completed)
        {
            log(format_args!("the command's result was not recorded: {error}"));
        }
    }

    fn control_current(&self, backend: &dyn Backend, target: &Target, sender: &str) -> bool {
        self.sync_current(backend)
            && self
                .store()
                .control_allows_in(backend.identity(), backend.device_trust(), sender)
                .unwrap_or(false)
            && self
                .store()
                .sync_target(&target.local)
                .ok()
                .flatten()
                .is_some_and(|current| current.identity == *backend.identity() && current.sync_id == target.sync_id)
    }

    async fn control_results(&self, backend: &dyn Backend, only: Option<&str>) {
        let results = match self.store().control_results(backend.identity(), only) {
            Ok(results) => results,
            Err(error) => return log(format_args!("the results could not be read: {error}")),
        };
        for (sync_id, id, completed) in results {
            if !self.sync_current(backend) {
                return;
            }
            // A failed send stays pending and is sent again on the next pass.
            if remote(backend.control_finish(&sync_id, &id, completed)).await.is_ok()
                && let Err(error) = self.store().control_acknowledge(backend.identity(), &sync_id, &id)
            {
                log(format_args!("a delivered result was not recorded: {error}"));
            }
        }
    }
}

/// Says why a phone command was not run or not recorded: in fixed words, with a store's error or a request's
/// [`super::FailureKind`]. Never the command's content.
fn log(what: impl std::fmt::Display) {
    eprintln!("nexus-workbench: phone control: {what}");
}
