//! Hooks: a shell command the person runs on a Code session event. A turn ended, the agent needs them,
//! the pull request merged and the session settled, the session was snoozed or archived, or a pull request check
//! failed. The person sets them in their own `settings.json` (`hooks`). The app that
//! owns the store hands the workbench a [`Source`] that reads them ([`Workbench::set_hooks`]). A workbench with no
//! source runs none: a remote machine's host, session sync and Gateway never set one.
//!
//! Each hook runs once per event. Before it runs, it claims a row in the store (the hook, the session, the event's key).
//! A second claim of the same row fails, so a second owner or a repeated call runs nothing. It runs off the caller's
//! thread, in `/bin/sh -c` (Windows: `cmd /C`) with the agents' `PATH`, in the session's worktree (else its project's
//! folder), with the event in `NEXUS_*` variables ([`env`]). It is stopped with its process group after its timeout.
//! Each run is kept (its exit, the last lines of its output), the last [`KEEP_RUNS`] per hook, for Settings › Hooks.
//! A failed run also goes to [`Source::failed`], which shows a notification.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{Project, SetupStatus, StoredEvent, ThreadSummary, TurnStatus, WbEvent};
use crate::process::Shell;
use crate::{Workbench, now_ms, setup};

/// A hook's timeout when it sets none.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// The longest timeout a hook may set.
pub const MAX_TIMEOUT_SECS: u64 = 600;
/// The runs kept per hook.
pub const KEEP_RUNS: usize = 20;
/// The lines of output kept per run.
pub const KEEP_LINES: usize = 20;
/// The output held while a hook runs: its last bytes, enough for [`KEEP_LINES`] lines.
const HOLD: usize = 16 * 1024;

/// When a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookEvent {
    /// A turn ended, done or with an error (not one the person stopped).
    TurnFinished,
    /// The agent asks for an approval or has a question.
    NeedsYou,
    /// The session's pull request merged, and the session settled.
    Settled,
    Snoozed,
    Archived,
    /// A check on the session's pull request failed.
    CheckFailed,
}

impl HookEvent {
    pub const ALL: [HookEvent; 6] = [
        HookEvent::TurnFinished,
        HookEvent::NeedsYou,
        HookEvent::Settled,
        HookEvent::Snoozed,
        HookEvent::Archived,
        HookEvent::CheckFailed,
    ];

    /// Its name in settings.json and in `NEXUS_HOOK_EVENT`.
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::TurnFinished => "turnFinished",
            HookEvent::NeedsYou => "needsYou",
            HookEvent::Settled => "settled",
            HookEvent::Snoozed => "snoozed",
            HookEvent::Archived => "archived",
            HookEvent::CheckFailed => "checkFailed",
        }
    }

    /// Its name in Settings.
    pub fn label(self) -> &'static str {
        match self {
            HookEvent::TurnFinished => "Turn finished",
            HookEvent::NeedsYou => "Needs you",
            HookEvent::Settled => "Pull request merged",
            HookEvent::Snoozed => "Snoozed",
            HookEvent::Archived => "Archived",
            HookEvent::CheckFailed => "Check failed",
        }
    }

    pub fn parse(name: &str) -> Option<HookEvent> {
        HookEvent::ALL.into_iter().find(|event| event.as_str() == name)
    }
}

/// One hook, as settings.json keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    pub event: HookEvent,
    pub command: String,
    /// Seconds; None: [`DEFAULT_TIMEOUT_SECS`]. At most [`MAX_TIMEOUT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl Hook {
    /// Its id in the store: a hash of its event and command, so a reorder keeps its runs and claims.
    pub fn id(&self) -> String {
        let digest = Sha256::digest(format!("{}\n{}", self.event.as_str(), self.command).as_bytes());
        digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(
            self.timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        )
    }
}

/// The hooks in effect: the person's, and whether the managed policy lets them run (`HooksEnabled`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hooks {
    pub enabled: bool,
    pub list: Vec<Hook>,
}

impl Hooks {
    /// The hooks that run on `event`: none when the policy turned them off.
    pub fn matching(&self, event: HookEvent) -> Vec<&Hook> {
        if !self.enabled {
            return Vec::new();
        }
        self.list
            .iter()
            .filter(|hook| hook.event == event && !hook.command.trim().is_empty())
            .collect()
    }
}

/// What the app that owns the store supplies: the hooks, read at each event, and where a failed run is said.
pub trait Source: Send + Sync + 'static {
    fn hooks(&self) -> Hooks;
    /// A run failed (`body` says how), for the session `thread_id` titled `title`. The default says nothing.
    fn failed(&self, _thread_id: &str, _title: &str, _body: &str) {}
}

/// One event a hook may run on.
#[derive(Debug, Clone, PartialEq)]
pub struct Fire {
    pub event: HookEvent,
    pub thread_id: String,
    /// What makes it one event: a log event's number, a check's key, when it settled. Claimed once per hook.
    pub key: String,
    /// turnFinished: `completed` or `error`.
    pub turn_result: Option<&'static str>,
    /// checkFailed: the check's name and link.
    pub check: Option<(String, Option<String>)>,
}

impl Fire {
    /// An event with nothing but its key.
    pub fn plain(event: HookEvent, thread_id: &str, key: String) -> Fire {
        Fire {
            event,
            thread_id: thread_id.to_string(),
            key,
            turn_result: None,
            check: None,
        }
    }
}

/// The hook event a log event is, if any: a turn that ended done or with an error, or an approval or a question.
/// Keyed by the event's number in the session's log and its time.
pub fn fire_for(thread_id: &str, stored: &StoredEvent) -> Option<Fire> {
    let (event, turn_result) = match &stored.event {
        WbEvent::TurnEnd { status, .. } => match status {
            TurnStatus::Completed => (HookEvent::TurnFinished, Some("completed")),
            TurnStatus::Error => (HookEvent::TurnFinished, Some("error")),
            TurnStatus::Interrupted => return None,
        },
        WbEvent::Approval { .. } => (HookEvent::NeedsYou, None),
        _ => return None,
    };
    Some(Fire {
        turn_result,
        // With its time: a rewind can number a later event the same.
        ..Fire::plain(event, thread_id, format!("{}@{}", stored.seq, stored.at))
    })
}

/// One run of a hook, as Settings › Hooks shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookRun {
    /// [`Hook::id`].
    pub hook: String,
    pub event: String,
    pub thread_id: String,
    pub started_at: i64,
    /// None when it timed out or did not start.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last [`KEEP_LINES`] lines of its output, both streams.
    pub output: String,
}

impl HookRun {
    pub fn failed(&self) -> bool {
        self.timed_out || self.exit_code != Some(0)
    }

    /// How it ended, in a few words.
    pub fn outcome(&self) -> String {
        match (self.timed_out, self.exit_code) {
            (true, _) => "timed out".into(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "did not start".into(),
        }
    }
}

/// The `NEXUS_*` variables for `fire` on `session` in `project`. A value not known is empty.
pub fn env(fire: &Fire, session: &ThreadSummary, project: Option<&Project>) -> Vec<(String, String)> {
    let worktree = session.worktree.as_ref().filter(|worktree| !worktree.removed);
    let pull_request = session.worktree.as_ref().and_then(|worktree| {
        worktree
            .pull_request
            .clone()
            .or_else(|| worktree.review.as_ref().map(|review| review.url.clone()))
    });
    let branch = worktree
        .map(|worktree| worktree.branch.clone())
        .or_else(|| project.and_then(|project| project.branch.clone()));
    let (check_name, check_url) = fire.check.clone().unwrap_or_default();
    [
        ("NEXUS_HOOK_EVENT", fire.event.as_str().to_string()),
        ("NEXUS_SESSION_ID", session.id.clone()),
        ("NEXUS_SESSION_TITLE", session.title.clone()),
        ("NEXUS_PROJECT", project.map(|p| p.name.clone()).unwrap_or_default()),
        (
            "NEXUS_PROJECT_PATH",
            project.map(|p| p.path.clone()).unwrap_or_default(),
        ),
        ("NEXUS_WORKTREE", worktree.map(|w| w.path.clone()).unwrap_or_default()),
        ("NEXUS_BRANCH", branch.unwrap_or_default()),
        ("NEXUS_PULL_REQUEST", pull_request.unwrap_or_default()),
        ("NEXUS_TURN_RESULT", fire.turn_result.unwrap_or_default().to_string()),
        ("NEXUS_CHECK_NAME", check_name),
        ("NEXUS_CHECK_URL", check_url.unwrap_or_default()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// Where a hook runs: the session's worktree while it is on disk, else its project's folder.
pub fn folder(session: &ThreadSummary, project: Option<&Project>) -> PathBuf {
    session
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.removed && Path::new(&worktree.path).is_dir())
        .map(|worktree| PathBuf::from(&worktree.path))
        .or_else(|| project.map(|project| PathBuf::from(&project.path)))
        .unwrap_or_else(std::env::temp_dir)
}

/// The last `max` lines of `text`.
pub fn last_lines(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(max)..].join("\n")
}

/// Runs `command` in `cwd` with `env` for at most `timeout`; its process group is stopped then. Answers the exit code
/// (None: it timed out or did not start), whether it timed out, and the last lines of its output.
pub async fn run_command(
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
    timeout: Duration,
    login: &Shell,
) -> (Option<i32>, bool, String) {
    let (command, cwd, env, login) = (command.to_string(), cwd.to_path_buf(), env.to_vec(), login.clone());
    // Asking the login shell for its `PATH` blocks for a moment the first time.
    let spawn = match tokio::task::spawn_blocking(move || setup::shell_in(&command, &cwd, &env, &login)).await {
        Ok(spawn) => spawn,
        Err(error) => return (None, false, format!("Could not prepare the hook: {error}")),
    };
    let mut held = String::new();
    let mut hold = |chunk: String| {
        held.push_str(&chunk);
        if held.len() > HOLD {
            held = crate::tail(&held, HOLD).to_string();
        }
    };
    let outcome = setup::run(&spawn, timeout, std::future::pending(), &mut hold).await;
    hold(outcome.rest);
    let timed_out = outcome.status == SetupStatus::TimedOut;
    (outcome.exit_code, timed_out, last_lines(&held, KEEP_LINES))
}

impl Workbench {
    /// Where hooks come from. Only the app that owns the store calls this; without it, no hook runs.
    pub fn set_hooks(&self, source: Arc<dyn Source>) {
        *self.inner.hooks.lock().unwrap_or_else(|p| p.into_inner()) = Some(source);
    }

    fn hook_source(&self) -> Option<Arc<dyn Source>> {
        self.inner.hooks.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Runs the hooks that match `fire`, off this thread. Cheap when there is no source.
    pub fn fire_hooks(&self, fire: Fire) {
        let Some(source) = self.hook_source() else {
            return;
        };
        let workbench = self.clone();
        let task = async move { workbench.run_hooks(source, fire).await };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(task);
            }
            // A sync command on the main thread (an archive) has no runtime: a thread of its own.
            Err(_) => {
                std::thread::spawn(
                    move || match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                        Ok(runtime) => runtime.block_on(task),
                        Err(error) => eprintln!("nexus-workbench: hooks: {error}"),
                    },
                );
            }
        }
    }

    async fn run_hooks(&self, source: Arc<dyn Source>, fire: Fire) {
        let hooks = source.hooks();
        let matching: Vec<Hook> = hooks.matching(fire.event).into_iter().cloned().collect();
        if matching.is_empty() {
            return;
        }
        let Ok(record) = self.record(&fire.thread_id) else {
            return;
        };
        let session = record.summary;
        let project = self.store().project(&session.project_id).ok().flatten();
        let env = env(&fire, &session, project.as_ref());
        let cwd = folder(&session, project.as_ref());
        let login = self.config().shell;
        let runs = matching.into_iter().filter_map(|hook| {
            let id = hook.id();
            match self
                .store()
                .claim_hook(&id, &fire.thread_id, fire.event.as_str(), &fire.key)
            {
                Ok(true) => Some((hook, id)),
                Ok(false) => None,
                Err(error) => {
                    eprintln!("nexus-workbench: hooks: {error}");
                    None
                }
            }
        });
        let runs: Vec<(Hook, String)> = runs.collect();
        let all = runs.into_iter().map(|(hook, id)| {
            let (source, env, cwd, session, login) =
                (source.clone(), env.clone(), cwd.clone(), session.clone(), login.clone());
            let event = fire.event;
            async move {
                let started_at = now_ms();
                let (exit_code, timed_out, output) =
                    run_command(&hook.command, &cwd, &env, hook.timeout(), &login).await;
                let run = HookRun {
                    hook: id,
                    event: event.as_str().into(),
                    thread_id: session.id.clone(),
                    started_at,
                    exit_code,
                    timed_out,
                    output,
                };
                if let Err(error) = self.store().add_hook_run(&run) {
                    eprintln!("nexus-workbench: hooks: {error}");
                }
                if run.failed() {
                    let body = format!(
                        "The {} hook \u{201c}{}\u{201d} failed: {}.",
                        event.label(),
                        hook.command.trim(),
                        run.outcome()
                    );
                    source.failed(&session.id, &session.title, &body);
                }
            }
        });
        futures::future::join_all(all).await;
    }

    /// A check on the session's pull request failed (the pull request watch): keyed by the check's run.
    pub fn check_failed(&self, thread_id: &str, name: &str, url: Option<&str>) {
        self.fire_hooks(Fire {
            check: Some((name.to_string(), url.map(String::from))),
            ..Fire::plain(
                HookEvent::CheckFailed,
                thread_id,
                format!("{name}\n{}", url.unwrap_or_default()),
            )
        });
    }

    /// Every hook's last runs, newest first.
    pub fn hook_runs(&self) -> crate::Result<Vec<HookRun>> {
        self.store().hook_runs()
    }
}

#[cfg(test)]
#[path = "hooks/tests.rs"]
mod tests;
