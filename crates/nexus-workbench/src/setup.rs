//! Project setup scripts. A session's own worktree starts as a bare checkout: no `node_modules`, no `.env` files, no
//! builds. A project can name one shell command (`npm ci`, `cp ../app/.env . && make deps`) that runs in each new
//! worktree before the agent's first turn, like T3 Code's "setup" project script.
//!
//! The script's output streams into the session's log as [`WbEvent::Setup`] events (one when it starts, more as it
//! prints, one when it ends). The session's messages wait for it, in the order they were sent, and the agent starts
//! when it ends, however it ended: a failure or a timeout is shown in the log, never a reason to refuse the session.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{Notify, mpsc};

use crate::event::{Image, SetupStatus, ThreadSummary, WbEvent};
use crate::process::{self, Shell, Spawn};
use crate::{AdmittedExecution, Result, Workbench};

/// A setup that runs longer than this is stopped (a first `npm ci` in a big repository takes minutes, not ten).
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How often what the script printed is written to the log while it runs.
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// The output kept in the log: its first `KEEP` bytes, then only its last `TAIL` (where a failure says why).
const KEEP: usize = 64 * 1024;
const TAIL: usize = 8 * 1024;
/// After the script exits, how long what it printed last may take to arrive, all told: a process it left in the
/// background (`npm run watch &`) may hold its output open, and keep printing, for good.
const DRAIN: Duration = Duration::from_millis(500);
/// The most the drain takes in all, however often a process left behind keeps printing.
const DRAIN_MAX: Duration = Duration::from_secs(2);

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: SetupStatus,
    pub exit_code: Option<i32>,
    pub duration: Duration,
    /// The output not handed to `flush` yet.
    pub rest: String,
}

/// Runs `spawn` with nothing on its input, gives what it prints (both streams, a line at a time, or in pieces if one runs long) to `flush` about once a
/// second, and stops it with its whole process group after `timeout`, or as soon as `stop` resolves.
pub async fn run(
    spawn: &Spawn,
    timeout: Duration,
    stop: impl Future<Output = ()>,
    mut flush: impl FnMut(String),
) -> Outcome {
    let started = Instant::now();
    let mut child = match process::spawn(spawn) {
        Ok(child) => child,
        Err(error) => {
            return Outcome {
                status: SetupStatus::Failed,
                exit_code: None,
                duration: started.elapsed(),
                rest: format!("Could not start {}: {error}\n", spawn.program.display()),
            };
        }
    };
    drop(child.stdin.take());
    let (tx, mut rx) = mpsc::unbounded_channel();
    if let Some(out) = child.stdout.take() {
        pump(out, tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(err, tx);
    }

    let mut output = Output::default();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    tokio::pin!(stop);
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + FLUSH_EVERY, FLUSH_EVERY);
    let mut cut: Option<SetupStatus> = None;
    let exit = loop {
        tokio::select! {
            Some(chunk) = rx.recv() => output.push(&chunk),
            _ = tick.tick() => {
                let ready = output.take();
                if !ready.is_empty() {
                    flush(ready);
                }
            }
            exit = child.wait() => break exit.ok(),
            _ = &mut deadline => cut = Some(SetupStatus::TimedOut),
            _ = &mut stop => cut = Some(SetupStatus::Stopped),
        }
        if cut.is_some() {
            process::terminate(child.id());
            if tokio::time::timeout(process::TERM_GRACE, child.wait()).await.is_err() {
                process::kill(child.id());
                let _ = child.wait().await;
            }
            break None;
        }
    };
    let drain_end = tokio::time::Instant::now() + DRAIN_MAX;
    while let Ok(Some(chunk)) = tokio::time::timeout(DRAIN, tokio::time::timeout_at(drain_end, rx.recv()))
        .await
        .and_then(|r| r)
    {
        output.push(&chunk);
    }
    let exit_code = exit.and_then(|status| status.code());
    let status = if let Some(cut) = cut {
        cut
    } else if exit.is_some_and(|status| status.success()) {
        SetupStatus::Done
    } else {
        SetupStatus::Failed
    };
    Outcome {
        status,
        exit_code,
        duration: started.elapsed(),
        rest: output.finish(),
    }
}

/// The most of one line held before it is handed on: output with no newline for a long time (a progress bar redrawn
/// with `\r`, a minified dump) goes on in pieces of this size instead of piling up in memory whole.
const PIECE: usize = 8 * 1024;

/// Sends what `pipe` prints, line by line (a line longer than [`PIECE`] in pieces), until it closes. Both streams
/// share one channel, so a line over [`PIECE`] on one can have the other's output between its pieces in the log.
fn pump(pipe: impl AsyncRead + Unpin + Send + 'static, tx: mpsc::UnboundedSender<String>) {
    tokio::spawn(pieces(pipe, PIECE, move |piece| tx.send(piece).is_ok()));
}

/// Reads `pipe` to its end, giving `send` each line, or each `max` bytes of a longer one (cut between characters).
/// Stops early when `send` says no one listens any more.
async fn pieces(pipe: impl AsyncRead + Unpin, max: usize, mut send: impl FnMut(String) -> bool) {
    let mut reader = BufReader::new(pipe);
    let mut line: Vec<u8> = Vec::new();
    loop {
        let available = match reader.fill_buf().await {
            Ok(available) if !available.is_empty() => available,
            _ => break,
        };
        let room = max.saturating_sub(line.len()).max(1);
        let (used, ends) = match available.iter().take(room).position(|&byte| byte == b'\n') {
            Some(newline) => (newline + 1, true),
            None => (available.len().min(room), false),
        };
        line.extend_from_slice(&available[..used]);
        reader.consume(used);
        if ends || line.len() >= max {
            // A character cut in two waits for its other half.
            let cut = match std::str::from_utf8(&line) {
                Err(error) if !ends && error.error_len().is_none() && error.valid_up_to() > 0 => error.valid_up_to(),
                _ => line.len(),
            };
            if !send(String::from_utf8_lossy(&line[..cut]).into_owned()) {
                return;
            }
            line.drain(..cut);
        }
    }
    if !line.is_empty() {
        send(String::from_utf8_lossy(&line).into_owned());
    }
}

/// The output kept for the log ([`KEEP`], [`TAIL`]).
#[derive(Default)]
struct Output {
    /// Kept and not handed out yet.
    pending: String,
    kept: usize,
    /// Past `KEEP`: the latest output, and how much before it was dropped.
    tail: String,
    dropped: usize,
}

impl Output {
    fn push(&mut self, mut text: &str) {
        if self.kept < KEEP {
            let cut = boundary(text, KEEP - self.kept);
            self.pending.push_str(&text[..cut]);
            // Full once something didn't fit, even a character cut in two.
            self.kept = if cut < text.len() { KEEP } else { self.kept + cut };
            text = &text[cut..];
        }
        self.tail.push_str(text);
        if self.tail.len() > TAIL {
            let mut cut = self.tail.len() - TAIL;
            while !self.tail.is_char_boundary(cut) {
                cut += 1;
            }
            self.dropped += cut;
            self.tail.drain(..cut);
        }
    }

    fn take(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }

    /// What is left to write once the script ended.
    fn finish(mut self) -> String {
        let mut rest = self.take();
        if self.dropped > 0 {
            rest.push_str(&format!("\n[{} KiB of output left out]\n", self.dropped.div_ceil(1024)));
        }
        rest + &self.tail
    }
}

/// The largest char boundary in `text` at or below `at`.
fn boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The setup script as a process: the platform's shell in the worktree, with the agents' `PATH` (an app started from
/// the dock has a bare one) and where the project and the worktree are, as T3 Code passes them. `login` says which shell
/// that `PATH` is asked of ([`process::agent_path`]); the script itself always runs in `/bin/sh` (`cmd` on Windows).
fn shell(script: &str, worktree: &Path, project: &Path, login: &Shell) -> Spawn {
    let place = [
        ("NEXUS_PROJECT_ROOT".to_string(), project.to_string_lossy().into_owned()),
        (
            "NEXUS_WORKTREE_PATH".to_string(),
            worktree.to_string_lossy().into_owned(),
        ),
    ];
    shell_in(script, worktree, &place, login)
}

/// `script` in the platform's shell in `cwd`, with the agents' `PATH` and `extra` over this process's environment.
/// Hooks run this way too (`crate::hooks`).
pub(crate) fn shell_in(script: &str, cwd: &Path, extra: &[(String, String)], login: &Shell) -> Spawn {
    static PATH: OnceLock<String> = OnceLock::new();
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(name, _)| !name.starts_with("TAURI_") && !name.starts_with("WEBKIT_"))
        .collect();
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE")).map(PathBuf::from);
    let current = std::env::var("PATH").ok();
    // Asked once of the login shell. A plain shell asks no shell, so its `PATH` is quick to build each time.
    let path = match login {
        Shell::Login => PATH
            .get_or_init(|| process::agent_path(current.as_deref(), home.as_deref(), login))
            .clone(),
        Shell::Plain(_) => process::agent_path(current.as_deref(), home.as_deref(), login),
    };
    env.insert("PATH".into(), path);
    env.extend(extra.iter().cloned());
    // Nobody can answer a prompt or read colour codes here.
    env.insert("NO_COLOR".into(), "1".into());
    env.insert("FORCE_COLOR".into(), "0".into());
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("/bin/sh", "-c")
    };
    Spawn {
        program: PathBuf::from(program),
        args: vec![flag.into(), script.into()],
        env,
        cwd: cwd.to_path_buf(),
    }
}

/// A running setup: its messages wait for `gate`, which the setup holds until it ends. `stop` ends it early.
#[derive(Clone)]
struct Running {
    gate: Arc<tokio::sync::Mutex<()>>,
    stop: Arc<Notify>,
}

/// Sessions whose setup is running. Tokio's lock is first come, first served, so the messages waiting on a gate still
/// reach the agent in the order they were sent.
static RUNNING: LazyLock<Mutex<HashMap<String, Running>>> = LazyLock::new(Default::default);

fn running() -> std::sync::MutexGuard<'static, HashMap<String, Running>> {
    RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Workbench {
    /// The project's setup script, if it has one.
    pub fn project_setup(&self, project_id: &str) -> Result<Option<String>> {
        self.store().project_setup(project_id)
    }

    /// Sets the project's setup script (trimmed; blank takes it away), for worktrees made from now on.
    pub fn set_project_setup(&self, project_id: &str, script: Option<&str>) -> Result<Option<String>> {
        let script = script.map(str::trim).filter(|script| !script.is_empty());
        let store = self.store();
        if store.project(project_id)?.is_none() {
            return Err("That project is no longer open.".into());
        }
        store.set_project_setup(project_id, script)?;
        Ok(script.map(String::from))
    }

    /// Starts the project's setup script in the session's new worktree, when it has both (`project`: the project's
    /// folder). True when it started: the
    /// session's messages then wait for it ([`Workbench::wait_for_setup`]).
    pub(crate) fn begin_setup(&self, summary: &ThreadSummary, project: &str) -> Result<bool> {
        let Some(worktree) = summary.worktree.as_ref() else {
            return Ok(false);
        };
        let Some(command) = self.store().project_setup(&summary.project_id)? else {
            return Ok(false);
        };
        let update_activity = crate::UpdateActivity::begin(self)?;
        let thread_id = summary.id.clone();
        let id = format!("setup_{thread_id}");
        let script = command.clone();
        let event = move |status, output, exit_code, duration_ms| WbEvent::Setup {
            id: id.clone(),
            command: command.clone(),
            status,
            output,
            exit_code,
            duration_ms,
        };
        self.append(&thread_id, event(SetupStatus::Running, String::new(), None, None))?;
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let held = gate
            .clone()
            .try_lock_owned()
            .map_err(|_| "The setup's lock was taken.".to_string())?;
        let stop = Arc::new(Notify::new());
        running().insert(
            thread_id.clone(),
            Running {
                gate,
                stop: stop.clone(),
            },
        );

        let wb = self.clone();
        let login = self.config().shell;
        let (worktree, project) = (PathBuf::from(&worktree.path), PathBuf::from(project));
        tokio::spawn(async move {
            let _update_activity = update_activity;
            let spawn = tokio::task::spawn_blocking(move || shell(&script, &worktree, &project, &login)).await;
            let outcome = match spawn {
                Ok(spawn) => {
                    run(&spawn, TIMEOUT, stop.notified(), |output| {
                        let _ = wb.append(&thread_id, event(SetupStatus::Running, output, None, None));
                    })
                    .await
                }
                Err(error) => Outcome {
                    status: SetupStatus::Failed,
                    exit_code: None,
                    duration: Duration::ZERO,
                    rest: format!("Could not start the setup: {error}\n"),
                },
            };
            let duration_ms = u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX);
            let _ = wb.append(
                &thread_id,
                event(outcome.status, outcome.rest, outcome.exit_code, Some(duration_ms)),
            );
            running().remove(&thread_id);
            drop(held);
        });
        Ok(true)
    }

    /// Waits for the session's setup script, if one is running, and gives the place it held in line. The waiting
    /// messages take it one at a time in the order they came: the caller keeps the guard until its message reached
    /// the agent (`start` registered the run or queued the turn), so the next one cannot overtake it.
    pub(crate) async fn wait_for_setup(&self, thread_id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let gate = running().get(thread_id).map(|running| running.gate.clone())?;
        Some(gate.lock_owned().await)
    }

    /// Stops the session's running setup script with its whole process group. It ends as
    /// [`SetupStatus::Stopped`] and the waiting messages go on to the agent, as after a failure. False when no setup
    /// was running (it had just ended).
    pub fn stop_setup(&self, thread_id: &str) -> bool {
        let Some(running) = running().get(thread_id).cloned() else {
            return false;
        };
        // A stored permit: a stop sent before the run is waiting still counts.
        running.stop.notify_one();
        true
    }

    /// A new session's first message while its setup runs: sent from here, so making the session doesn't wait for the
    /// setup. Sending fails into the session's log, where the person is by then.
    pub(crate) fn send_after_setup(
        &self,
        thread_id: String,
        text: String,
        images: Vec<Image>,
        context: String,
        admitted: Option<AdmittedExecution>,
    ) {
        let wb = self.clone();
        tokio::spawn(async move {
            if let Err(text) = wb
                .send_creation_message(&thread_id, &text, images, &context, admitted.as_ref())
                .await
            {
                let _ = wb.append(&thread_id, WbEvent::Error { text, detail: None });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Spawn {
        shell(script, &std::env::temp_dir(), &std::env::temp_dir(), &Shell::sh())
    }

    #[tokio::test]
    async fn streams_both_outputs_and_reports_the_exit() {
        let mut flushed = Vec::new();
        let outcome = run(
            &sh("echo one; echo two >&2; exit 4"),
            TIMEOUT,
            std::future::pending(),
            |out| flushed.push(out),
        )
        .await;
        let all = flushed.concat() + &outcome.rest;
        assert!(all.contains("one\n") && all.contains("two\n"), "{all:?}");
        assert_eq!((outcome.status, outcome.exit_code), (SetupStatus::Failed, Some(4)));

        let outcome = run(&sh("true"), TIMEOUT, std::future::pending(), |_| {}).await;
        assert_eq!((outcome.status, outcome.exit_code), (SetupStatus::Done, Some(0)));
    }

    #[tokio::test]
    async fn a_process_left_printing_does_not_hold_the_end_back() {
        // The shell exits at once; its background child prints every 50 ms for 6 s.
        let script = sh("(for i in $(seq 1 120); do echo tick; sleep 0.05; done) & exit 0");
        // The login-shell lookup in `sh` stays outside the timer.
        let started = Instant::now();
        let outcome = run(&script, TIMEOUT, std::future::pending(), |_| {}).await;
        assert_eq!(outcome.status, SetupStatus::Done);
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    }

    /// How long a script that waits on [`Gate`] may take before the test fails: far past one flush tick even on a
    /// loaded host, and a timeout is a failure, never a pass.
    const GATE_DEADLINE: Duration = Duration::from_secs(30);

    /// A file the test creates once `flush` has handed it what it waits for. The script polls for it (`wait`) before
    /// it goes on, so the order is set by the hand-over itself, not by how long the script sleeps. If the hand-over
    /// never comes, the script runs into [`GATE_DEADLINE`] and the test fails.
    struct Gate(PathBuf);

    impl Gate {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("nexus-setup-gate-{name}-{}", std::process::id()));
            let _ = std::fs::remove_file(&path);
            Self(path)
        }

        /// Shell that blocks until the gate is open.
        fn wait(&self) -> String {
            format!("while [ ! -e '{}' ]; do sleep 0.02; done", self.0.display())
        }

        fn open(&self) {
            std::fs::write(&self.0, b"").expect("open the gate");
        }
    }

    impl Drop for Gate {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn hands_output_over_while_it_runs() {
        let gate = Gate::new("hands-over");
        let mut flushed = Vec::new();
        // 'late' is only printed once 'early' has reached flush, so it must have been handed over while running.
        let outcome = run(
            &sh(&format!("echo early; {}; echo late", gate.wait())),
            GATE_DEADLINE,
            std::future::pending(),
            |out| {
                if out.contains("early") {
                    gate.open();
                }
                flushed.push(out);
            },
        )
        .await;
        assert_eq!(
            outcome.status,
            SetupStatus::Done,
            "'early' was not handed over while the script ran"
        );
        assert_eq!(flushed.first().map(String::as_str), Some("early\n"));
        assert_eq!(flushed.concat() + &outcome.rest, "early\nlate\n");
    }

    #[tokio::test]
    async fn keeps_handing_output_over_after_the_first_tick() {
        // 'late' prints after the first tick and well before the exit, so a later tick hands it over, not the exit.
        let mut flushed = Vec::new();
        let outcome = run(
            &sh("echo early; sleep 1.5; echo late; sleep 2.5"),
            TIMEOUT,
            std::future::pending(),
            |out| flushed.push(out),
        )
        .await;
        assert_eq!(flushed.concat(), "early\nlate\n");
        assert_eq!(outcome.rest, "");
    }

    #[tokio::test]
    async fn stops_a_script_that_runs_too_long() {
        let script = sh("echo begun; sleep 30");
        // The login-shell lookup in `sh` stays outside the timer.
        let started = Instant::now();
        let mut flushed = Vec::new();
        let outcome = run(&script, Duration::from_millis(300), std::future::pending(), |out| {
            flushed.push(out)
        })
        .await;
        assert_eq!(outcome.status, SetupStatus::TimedOut);
        assert!((flushed.concat() + &outcome.rest).contains("begun"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn stop_ends_the_script_and_what_it_started() {
        let marker = std::env::temp_dir().join(format!("nexus-setup-stop-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("echo begun; (sleep 1; touch '{}') & sleep 30", marker.display());
        let script = sh(&script);
        // The login-shell lookup in `sh` stays outside the timer.
        let started = Instant::now();
        let mut flushed = Vec::new();
        let outcome = run(
            &script,
            TIMEOUT,
            tokio::time::sleep(Duration::from_millis(300)),
            |out| flushed.push(out),
        )
        .await;
        assert_eq!((outcome.status, outcome.exit_code), (SetupStatus::Stopped, None));
        assert!((flushed.concat() + &outcome.rest).contains("begun"));
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        // The background child went with the group: it never gets to write its file.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!marker.exists(), "the script's background child outlived the stop");
    }

    #[tokio::test]
    async fn a_missing_shell_fails_without_running() {
        let mut spawn = sh("true");
        spawn.program = PathBuf::from("/nonexistent/shell");
        let outcome = run(&spawn, TIMEOUT, std::future::pending(), |_| {}).await;
        assert_eq!((outcome.status, outcome.exit_code), (SetupStatus::Failed, None));
        assert!(outcome.rest.contains("Could not start"));
    }

    /// Output with no newline is handed on in pieces no longer than the cap, not held whole; nothing is lost, and a
    /// character on a cut stays whole.
    #[tokio::test]
    async fn a_line_that_never_ends_goes_on_in_pieces() {
        let text = format!("{}é{}\nshort\nlast", "x".repeat(9), "y".repeat(30));
        let mut got = Vec::new();
        pieces(text.as_bytes(), 10, |piece| {
            got.push(piece);
            true
        })
        .await;
        assert!(got.iter().all(|piece| piece.len() <= 10), "{got:?}");
        assert_eq!(got.concat(), text);
        assert!(
            got.contains(&"short\n".to_string()) && got.last().map(String::as_str) == Some("last"),
            "{got:?}"
        );
        assert_eq!(got[0], "x".repeat(9), "the é is not cut in two");
    }

    #[tokio::test]
    async fn a_long_progress_bar_neither_hangs_nor_fills_the_log() {
        let outcome = run(
            &sh("i=0; while [ $i -lt 20000 ]; do printf '\\r[%05d]' $i; i=$((i+1)); done; echo; echo done"),
            TIMEOUT,
            std::future::pending(),
            |_| {},
        )
        .await;
        assert_eq!(outcome.status, SetupStatus::Done);
        assert!(outcome.rest.contains("KiB of output left out"), "{}", outcome.rest);
        assert!(outcome.rest.ends_with("done\n"), "{}", outcome.rest);
    }

    /// The bar shows while the script still runs: a piece is flushed before the script ends, not only at its newline.
    #[tokio::test]
    async fn a_bar_with_no_newline_shows_before_the_script_ends() {
        let gate = Gate::new("bar");
        // The script ends only once a piece of the bar has reached flush.
        let script = sh(&format!(
            "i=0; while [ $i -lt 2000 ]; do printf '\\r[%05d]' $i; i=$((i+1)); done; {}; echo done",
            gate.wait()
        ));
        let mut flushed = Vec::new();
        let outcome = run(&script, GATE_DEADLINE, std::future::pending(), |out| {
            if out.contains('[') {
                gate.open();
            }
            flushed.push(out);
        })
        .await;
        assert_eq!(
            outcome.status,
            SetupStatus::Done,
            "no piece of the bar was handed over while the script ran"
        );
        let first = flushed.first();
        assert!(
            first.is_some_and(|out| out.contains('[') && !out.contains("done")),
            "{flushed:?}"
        );
        assert!(
            (flushed.concat() + &outcome.rest).ends_with("done\n"),
            "{flushed:?} {:?}",
            outcome.rest
        );
    }

    #[test]
    fn keeps_the_start_and_the_end_of_long_output() {
        let mut output = Output::default();
        let line = "é".repeat(500) + "\n";
        for _ in 0..200 {
            output.push(&line);
        }
        output.push("the error\n");
        let start = output.take();
        assert!(start.len() <= KEEP && start.len() > KEEP - line.len());
        let rest = output.finish();
        assert!(rest.contains("KiB of output left out"), "{rest}");
        assert!(rest.ends_with("the error\n"));
        assert!(rest.len() < TAIL + 100);
    }
}
