//! A shell on a server, for a server session's Terminal tab: the same screen as a shell here, but its pseudo-terminal
//! is the host's (`wb_shell_*`, `nexus_workbench::shell_pty`), as VS Code's and T3 Code's terminals run where the
//! workspace lives. Two threads keep the UI thread free. One opens the shell and reads its output by cursor, so a
//! dropped connection reads on from where it was, without loss. The other sends typing and resizes one call at a time,
//! so they reach the shell in order (the host answers each call on its own).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use nexus_workbench::shell_pty::{self, ReadOut};
use serde_json::{Value, json};

/// How long a read waits on the host for output: well under a call's own limits.
const WAIT_MS: u64 = 20_000;
/// The first and the longest pause before a failed read is tried again.
const RETRY: (Duration, Duration) = (Duration::from_millis(500), Duration::from_secs(5));
/// The most typing one call carries; more goes in the next.
const MAX_SEND: usize = 32 * 1024;

/// Where a server's calls go: the app's servers (`machines.rs`), or a test's.
pub trait ShellHost: Send + Sync + 'static {
    fn call(&self, call: &'static str, args: Value) -> BoxFuture<'static, Result<Value, String>>;
}

/// A server's session or project, where its shells open.
#[derive(Clone)]
pub struct ServerTarget {
    pub host: Arc<dyn ShellHost>,
    pub thread: Option<String>,
    pub project: Option<String>,
    /// Run this instead of an interactive shell, once (a project command). A shell started again is interactive.
    pub command: Option<String>,
}

/// What the shell's reader tells the screen.
pub(super) enum Event {
    Output {
        bytes: Vec<u8>,
        truncated: bool,
    },
    /// It ended, with this exit code if the host knows it.
    Exited(Option<u32>),
    /// The server can't be reached right now (Some), or can again (None).
    Problem(Option<String>),
}

enum Op {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

/// A running shell on a server. Dropping it closes the shell there.
pub(super) struct ServerShell {
    ops: Sender<Op>,
    stopped: Arc<AtomicBool>,
}

impl Drop for ServerShell {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = self.ops.send(Op::Close);
    }
}

fn call(host: &Arc<dyn ShellHost>, name: &'static str, args: Value) -> Result<Value, String> {
    futures::executor::block_on(host.call(name, args))
}

impl ServerShell {
    /// Opens a `cols` by `rows` shell at `target` and reads it; `on` hears its output in order.
    pub(super) fn start(target: &ServerTarget, cols: u16, rows: u16, on: impl FnMut(Event) + Send + 'static) -> Self {
        let (ops, queue) = channel();
        let (opened, id) = channel::<String>();
        let stopped = Arc::new(AtomicBool::new(false));
        let args = json!({
            "threadId": target.thread,
            "projectId": target.project,
            "cols": cols,
            "rows": rows,
            "command": target.command,
        });
        let (host, stop) = (target.host.clone(), stopped.clone());
        std::thread::spawn(move || read(host, args, opened, stop, on));
        let host = target.host.clone();
        std::thread::spawn(move || write(host, id, queue));
        ServerShell { ops, stopped }
    }

    #[cfg(test)]
    pub(super) fn input(&self, bytes: Vec<u8>) {
        let _ = self.ops.send(Op::Input(bytes));
    }

    /// Types into the shell from another thread: false once the shell is gone.
    pub(super) fn typing(&self) -> impl Fn(Vec<u8>) -> bool + Send + 'static {
        let ops = self.ops.clone();
        move |bytes| ops.send(Op::Input(bytes)).is_ok()
    }

    pub(super) fn resize(&self, cols: u16, rows: u16) {
        let _ = self.ops.send(Op::Resize(cols, rows));
    }
}

/// Opens the shell, then reads it until it ends or its owner goes.
fn read(
    host: Arc<dyn ShellHost>,
    args: Value,
    opened: Sender<String>,
    stopped: Arc<AtomicBool>,
    mut on: impl FnMut(Event),
) {
    let id = match call(&host, shell_pty::OPEN, args) {
        Ok(answer) => answer["id"].as_str().unwrap_or_default().to_string(),
        Err(error) => {
            on(Event::Problem(Some(format!(
                "The shell didn't start on the server: {error}"
            ))));
            on(Event::Exited(None));
            return;
        }
    };
    // The writer closes it, even when the owner went while it opened.
    let _ = opened.send(id.clone());
    let (mut after, mut retry, mut failing) = (0u64, RETRY.0, false);
    while !stopped.load(Ordering::Relaxed) {
        let read = call(
            &host,
            shell_pty::READ,
            json!({ "id": id, "after": after, "waitMs": WAIT_MS }),
        )
        .and_then(|answer| serde_json::from_value::<ReadOut>(answer).map_err(|e| e.to_string()));
        // A read that comes back after the owner went is not shown: a new shell may own the screen now.
        if stopped.load(Ordering::Relaxed) {
            return;
        }
        match read {
            Ok(read) => {
                if std::mem::take(&mut failing) {
                    on(Event::Problem(None));
                }
                retry = RETRY.0;
                let bytes = read.bytes();
                if !bytes.is_empty() || read.truncated {
                    on(Event::Output {
                        bytes,
                        truncated: read.truncated,
                    });
                }
                after = read.next;
                if read.exited {
                    on(Event::Exited(read.exit_code));
                    return;
                }
            }
            Err(error) if error.starts_with(shell_pty::CLOSED) => {
                on(Event::Exited(None));
                return;
            }
            Err(error) => {
                if !std::mem::replace(&mut failing, true) {
                    on(Event::Problem(Some(format!("Reconnecting to the server… ({error})"))));
                }
                std::thread::sleep(retry);
                retry = (retry * 2).min(RETRY.1);
            }
        }
    }
}

/// Sends the typing and the resizes in order, typing that waits together in one call, then closes the shell.
fn write(host: Arc<dyn ShellHost>, id: Receiver<String>, queue: Receiver<Op>) {
    let Ok(id) = id.recv() else {
        return;
    };
    let mut waiting: VecDeque<Op> = VecDeque::new();
    loop {
        let op = match waiting.pop_front() {
            Some(op) => op,
            None => queue.recv().unwrap_or(Op::Close),
        };
        let result = match op {
            Op::Input(mut bytes) => {
                while waiting.is_empty() && bytes.len() < MAX_SEND {
                    match queue.try_recv() {
                        Ok(Op::Input(more)) => bytes.extend(more),
                        Ok(other) => waiting.push_back(other),
                        Err(_) => break,
                    }
                }
                call(
                    &host,
                    shell_pty::INPUT,
                    json!({ "id": id, "data": STANDARD.encode(&bytes) }),
                )
            }
            Op::Resize(cols, rows) => call(
                &host,
                shell_pty::RESIZE,
                json!({ "id": id, "cols": cols, "rows": rows }),
            ),
            Op::Close => {
                if let Err(error) = call(&host, shell_pty::CLOSE, json!({ "id": id }))
                    && !error.starts_with(shell_pty::CLOSED)
                {
                    eprintln!("nexus-desktop-gpui: closing a server shell: {error}");
                }
                return;
            }
        };
        if let Err(error) = result {
            eprintln!("nexus-desktop-gpui: server shell: {error}");
        }
    }
}

/// The app's servers as a [`ShellHost`]: server `machine`'s host, through its relay.
pub struct MachineShells {
    pub machines: Arc<crate::machines::Machines>,
    pub machine: String,
}

impl ShellHost for MachineShells {
    fn call(&self, call: &'static str, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let answered = self.machines.call(&self.machine, call, args);
        Box::pin(async move {
            answered
                .await
                .map_err(|_| "The server's answer was lost.".to_string())?
        })
    }
}

#[cfg(test)]
#[path = "server_shell_tests.rs"]
mod tests;
