//! One OS inhibitor for the app, independent of windows, focus and the selected organization.
//! Background tasks and owed parent turns count as work. An unknown service state never declares work finished.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use nexus_remote::engine::Engine;
use nexus_workbench::event::{ThreadStatus, ThreadSummary};

const POLL: Duration = Duration::from_secs(2);
const GRACE: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(30);

static WORKER: OnceLock<mpsc::Sender<Command>> = OnceLock::new();
static ERROR: Mutex<Option<String>> = Mutex::new(None);
static REQUESTS: AtomicUsize = AtomicUsize::new(0);

enum Command {
    Check,
    Configure(bool),
    Runner(Engine),
    Quit,
}

/// A chat or agent API request, including queued startup. Cancellation drops it with its future.
pub struct Request;

impl Request {
    pub fn start() -> Self {
        REQUESTS.fetch_add(1, Ordering::SeqCst);
        wake();
        Self
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        REQUESTS.fetch_sub(1, Ordering::SeqCst);
        wake();
    }
}

fn wake() {
    if let Some(worker) = WORKER.get() {
        let _ = worker.send(Command::Check);
    }
}

pub fn configure(enabled: bool) {
    if let Some(worker) = WORKER.get() {
        let _ = worker.send(Command::Configure(enabled));
    }
}

pub fn runner(engine: Engine) {
    if let Some(worker) = WORKER.get() {
        let _ = worker.send(Command::Runner(engine));
    }
}

pub fn note() -> String {
    let error = ERROR.lock().unwrap_or_else(|p| p.into_inner());
    match error.as_deref() {
        Some(error) => format!("Could not keep this computer awake: {error}. Retrying automatically."),
        None => "Prevents idle sleep and keeps the display on while Nexus is open, including sub-agents and commands left running in the background. Manual locks and system policies still apply.".into(),
    }
}

pub fn start(cx: &mut crate::gpui::App) {
    let (tx, rx) = mpsc::channel();
    if WORKER.set(tx).is_err() {
        return;
    }
    let enabled = crate::desktop_settings::load().keep_awake.unwrap_or(true);
    // Keep creation and destruction on this thread: Windows execution state belongs to its calling thread.
    std::thread::Builder::new()
        .name("nexus-keep-awake".into())
        .spawn(move || run(rx, enabled))
        .expect("the keep-awake worker starts");
    cx.on_app_quit(|_| {
        if let Some(worker) = WORKER.get() {
            let _ = worker.send(Command::Quit);
        }
        async {}
    })
    .detach();
}

/// A server session's work: a turn, an approval, or anything its agent left running (background sub-agents and
/// commands, an owed turn, linked sessions it waits on), as the sidebar's mark reads it.
pub(crate) fn summary_active(summary: &ThreadSummary) -> bool {
    matches!(
        summary.status,
        ThreadStatus::Starting | ThreadStatus::Running | ThreadStatus::Approval
    ) || nexus_workbench::attention::busy_in_background(summary)
}

/// Older services omit activeWork. Their sessions still give a conservative answer.
fn service_active(value: &serde_json::Value) -> Option<bool> {
    value
        .get("activeWork")
        .and_then(serde_json::Value::as_bool)
        .or_else(|| {
            value
                .get("sessions")
                .and_then(serde_json::Value::as_array)
                .map(|sessions| !sessions.is_empty())
        })
}

fn active(engine: Option<&Engine>) -> Option<bool> {
    if REQUESTS.load(Ordering::SeqCst) > 0 || crate::machines::get().has_active_work() {
        return Some(true);
    }
    let Some(engine) = engine else { return Some(false) };
    let Some(remote) = engine.remote() else {
        return Some(engine.workbench().has_active_work());
    };
    crate::code_run::runtime().block_on(async {
        tokio::time::timeout(
            Duration::from_secs(3),
            remote.call("host_status", serde_json::Value::Null),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .as_ref()
        .and_then(service_active)
    })
}

fn run(rx: mpsc::Receiver<Command>, mut enabled: bool) {
    let mut engine = None;
    let mut controller = Controller::default();
    loop {
        let command = rx.recv_timeout(POLL);
        match command {
            Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Check) => {}
            Ok(Command::Configure(on)) => enabled = on,
            Ok(Command::Runner(runner)) => engine = Some(runner),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let busy = if enabled { active(engine.as_ref()) } else { Some(false) };
        let result = controller.update(Instant::now(), enabled, busy, NativeGuard::acquire);
        if let Err(error) = result {
            eprintln!("keep awake: {error}");
            *ERROR.lock().unwrap_or_else(|p| p.into_inner()) = Some(error);
        } else if controller.guard.is_some() || !enabled || busy == Some(false) {
            *ERROR.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }
    // Dropping the controller releases the OS inhibitor on the thread that acquired it.
}

/// keepawake's Linux destructor unwraps the D-Bus release. A desktop bus restart must not kill our worker.
struct NativeGuard(Option<keepawake::KeepAwake>);

impl NativeGuard {
    fn acquire() -> Result<Self, String> {
        std::panic::catch_unwind(|| {
            keepawake::Builder::default()
                .display(true)
                .idle(true)
                .app_name("Nexus")
                .app_reverse_domain("xyz.nexus.desktop")
                .reason("Agents are working in Nexus")
                .create()
                .map(|guard| Self(Some(guard)))
                .map_err(|error| error.to_string())
        })
        .unwrap_or_else(|_| Err("The operating system's inhibition service failed".into()))
    }
}

impl Drop for NativeGuard {
    fn drop(&mut self) {
        if let Some(guard) = self.0.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard)));
        }
    }
}

struct Controller<G> {
    guard: Option<G>,
    idle_since: Option<Instant>,
    retry_at: Option<Instant>,
}

impl<G> Default for Controller<G> {
    fn default() -> Self {
        Self {
            guard: None,
            idle_since: None,
            retry_at: None,
        }
    }
}

impl<G> Controller<G> {
    fn update(
        &mut self,
        now: Instant,
        enabled: bool,
        busy: Option<bool>,
        acquire: impl FnOnce() -> Result<G, String>,
    ) -> Result<(), String> {
        if !enabled {
            self.guard = None;
            self.idle_since = None;
            self.retry_at = None;
            return Ok(());
        }
        match busy {
            None => {
                // No confirmed idle period spans a disconnect.
                self.idle_since = None;
                return Ok(());
            }
            Some(false) => {
                if now.duration_since(*self.idle_since.get_or_insert(now)) >= GRACE {
                    self.guard = None;
                    self.retry_at = None;
                }
                return Ok(());
            }
            Some(true) => self.idle_since = None,
        }
        if self.guard.is_none() && self.retry_at.is_none_or(|retry| now >= retry) {
            match acquire() {
                Ok(guard) => {
                    self.guard = Some(guard);
                    self.retry_at = None;
                }
                Err(error) => {
                    self.retry_at = Some(now + RETRY);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn overlapping_work_disconnect_and_parent_handoff_keep_one_guard() {
        let count = Arc::new(AtomicUsize::new(0));
        let acquire = || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Guard(count.clone()))
        };
        let mut controller = Controller::default();
        let now = Instant::now();
        for (seconds, busy) in [
            (0, Some(true)),
            (1, Some(true)),
            (2, Some(false)),
            (4, Some(true)),
            (5, None),
        ] {
            controller
                .update(now + Duration::from_secs(seconds), true, busy, acquire)
                .unwrap();
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
        controller
            .update(now + Duration::from_secs(30), true, Some(false), acquire)
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        controller
            .update(now + Duration::from_secs(35), true, Some(false), acquire)
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        controller
            .update(now + Duration::from_secs(36), true, Some(true), acquire)
            .unwrap();
        controller
            .update(now + Duration::from_secs(36), false, None, acquire)
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_acquisition_retries_and_quit_releases() {
        let mut controller = Controller::<()>::default();
        let now = Instant::now();
        assert!(
            controller
                .update(now, true, Some(true), || Err("unavailable".into()))
                .is_err()
        );
        controller
            .update(now + POLL, true, Some(true), || panic!("must back off"))
            .unwrap();
        controller.update(now + RETRY, true, Some(true), || Ok(())).unwrap();
        assert!(controller.guard.is_some());
        let count = Arc::new(AtomicUsize::new(1));
        let controller = Controller {
            guard: Some(Guard(count.clone())),
            idle_since: None,
            retry_at: None,
        };
        drop(controller);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn idle_parent_background_tasks_and_hidden_children_count_as_work() {
        use serde_json::json;
        let mut summary: ThreadSummary = serde_json::from_value(json!({
            "id": "child", "projectId": "project", "title": "child", "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
            "nexusSessions": [], "contextTokens": null, "contextWindow": null,
            "createdAt": 0, "updatedAt": 0, "archived": false
        }))
        .unwrap();
        assert!(!summary_active(&summary));
        summary.in_background = true;
        assert!(summary_active(&summary));
        summary.in_background = false;
        summary.waits_on.push("hidden-child".into());
        assert!(summary_active(&summary));
        summary.waits_on.clear();
        // A server session whose only work is a background command keeps the computer awake.
        summary.background = vec![nexus_workbench::event::BackgroundWork {
            kind: nexus_workbench::event::BackgroundKind::Shell,
            title: "cargo test".into(),
            started_at: None,
        }];
        assert!(summary_active(&summary));
        summary.background.clear();
        summary.parent_thread_id = Some("idle-parent".into());
        summary.archived = true;
        summary.status = ThreadStatus::Running;
        assert!(summary_active(&summary));
        summary.status = ThreadStatus::Idle;
        assert!(!summary_active(&summary));
    }

    #[test]
    fn service_reports_all_scope_work_and_supports_older_hosts() {
        use serde_json::json;
        assert_eq!(service_active(&json!({"activeWork": true, "sessions": []})), Some(true));
        assert_eq!(
            service_active(&json!({"activeWork": false, "sessions": [{"id": "idle"}]})),
            Some(false)
        );
        assert_eq!(service_active(&json!({"sessions": [{"id": "working"}]})), Some(true));
        assert_eq!(service_active(&json!({"sessions": []})), Some(false));
        assert_eq!(service_active(&json!({})), None);
    }
}
