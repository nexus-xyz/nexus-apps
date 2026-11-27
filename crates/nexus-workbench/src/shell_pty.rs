//! The person's own shells on a host: the pseudo-terminals the desktop app opens on a server or a cloud machine for a
//! session's Terminal tab and its project commands, as VS Code's and T3 Code's terminals run where the workspace lives.
//! The host answers the `wb_shell_*` calls ([`CALLS`]) for the person's own app only (nexus-host's `server.rs`), and
//! names [`FEATURE`] in its hello.
//!
//! This is not the shared terminal ([`crate::terminal`]), which journals the agents' commands.
//!
//! - `wb_shell_open { threadId | projectId, cols, rows, command? }` → `{ id }`: the login shell, or `command` in it,
//!   in the session's folder (its worktree) or the project's.
//! - `wb_shell_input { id, data }`: `data` is base64, typed as it is.
//! - `wb_shell_resize { id, cols, rows }`.
//! - `wb_shell_read { id, after, waitMs }` → [`ReadOut`]: the output after cursor `after`, waiting up to `waitMs` (at
//!   most [`MAX_WAIT`]) for some. A client that reconnects reads on from its cursor, so nothing is lost or repeated
//!   while the bytes are kept ([`Limits::buffer`]); `truncated` says when older ones were dropped.
//! - `wb_shell_close { id }`: hangs the shell up and ends its process group.
//!
//! A shell nobody read, typed into or resized for [`Limits::orphan`] is closed ([`Shells::reap`]), and a host keeps at
//! most [`Limits::shells`].

mod output;
mod pty;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Result;
use crate::process::Shell;
use pty::{Pty, Start};

/// What a host that opens the person's shells says in its hello's features.
pub const FEATURE: &str = "server-shells";

pub const OPEN: &str = "wb_shell_open";
pub const INPUT: &str = "wb_shell_input";
pub const RESIZE: &str = "wb_shell_resize";
pub const READ: &str = "wb_shell_read";
pub const CLOSE: &str = "wb_shell_close";
/// Every call of a shell.
pub const CALLS: [&str; 5] = [OPEN, INPUT, RESIZE, READ, CLOSE];
/// The calls the app makes by itself while a shell is on screen: they are not the person using the machine, and they
/// do not wake a sleeping cloud machine (`nexus_remote::activity`).
pub const QUIET: [&str; 3] = [READ, RESIZE, CLOSE];

/// The start of the error for a shell that is not open (closed, ended and reaped, or from before the host restarted).
pub const CLOSED: &str = "shell_closed";

/// The longest a read waits for output.
pub const MAX_WAIT: Duration = Duration::from_secs(25);
/// The most a single input carries.
const MAX_INPUT: usize = 64 * 1024;

/// How much a host keeps.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Shells open at once.
    pub shells: usize,
    /// Output bytes kept per shell.
    pub buffer: usize,
    /// How long a shell may go unused before it is closed.
    pub orphan: Duration,
    /// The most bytes one read returns.
    pub read_max: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            shells: 16,
            buffer: 1024 * 1024,
            orphan: Duration::from_secs(10 * 60),
            read_max: 256 * 1024,
        }
    }
}

/// `wb_shell_open`'s arguments.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenArgs {
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Run this in the login shell (`$SHELL -lc`) instead of an interactive shell.
    #[serde(default)]
    pub command: Option<String>,
}

/// `wb_shell_read`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOut {
    /// The output, base64.
    pub data: String,
    /// The cursor to read after next.
    pub next: u64,
    /// The shell ended and `data` holds the last of its output.
    pub exited: bool,
    pub exit_code: Option<u32>,
    /// Bytes before `data` were dropped.
    pub truncated: bool,
}

impl ReadOut {
    /// The output's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        STANDARD.decode(&self.data).unwrap_or_default()
    }
}

/// The host's shells, by id.
#[derive(Clone)]
pub struct Shells {
    open: Arc<Mutex<HashMap<String, Arc<Pty>>>>,
    shell: Shell,
    /// The agents' `PATH`, asked at each open.
    path: Arc<dyn Fn() -> String + Send + Sync>,
    limits: Limits,
}

fn closed() -> String {
    format!("{CLOSED}: That shell is no longer open.")
}

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a Value> {
    args.get(name).ok_or_else(|| format!("Say {name}."))
}

fn id_of(args: &Value) -> Result<String> {
    arg(args, "id")?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Say which shell.".into())
}

fn size_of(args: &Value) -> Result<(u16, u16)> {
    let number = |name| {
        arg(args, name)?
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| format!("{name} is not a size."))
    };
    Ok((number("cols")?, number("rows")?))
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("The shell's work stopped: {e}"))?
}

impl Shells {
    /// Shells started with `shell` ([`Shell::Login`] on a host), with the `PATH` that `path` says.
    pub fn new(shell: Shell, path: Arc<dyn Fn() -> String + Send + Sync>, limits: Limits) -> Shells {
        Shells {
            open: Arc::default(),
            shell,
            path,
            limits,
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Pty>> {
        self.open
            .lock()
            .map_err(|_| closed())?
            .get(id)
            .cloned()
            .ok_or_else(closed)
    }

    fn take(&self, id: &str) -> Option<Arc<Pty>> {
        self.open.lock().ok()?.remove(id)
    }

    /// How many are open.
    pub fn len(&self) -> usize {
        self.open.lock().map(|open| open.len()).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Opens a shell in `folder`; its id.
    pub fn open(&self, folder: PathBuf, args: &OpenArgs) -> Result<String> {
        self.reap();
        if self.len() >= self.limits.shells {
            return Err(format!(
                "This machine has {} shells open. Close one first.",
                self.limits.shells
            ));
        }
        let pty = Pty::start(Start {
            folder: &folder,
            cols: args.cols,
            rows: args.rows,
            command: args.command.as_deref().filter(|command| !command.trim().is_empty()),
            shell: &self.shell,
            path: (self.path)(),
            buffer: self.limits.buffer,
        })?;
        let id = format!("sh_{}", uuid::Uuid::new_v4().simple());
        self.open.lock().map_err(|_| closed())?.insert(id.clone(), pty);
        Ok(id)
    }

    /// Types `bytes` into shell `id`. It may wait while the shell reads nothing: call it off the async threads.
    pub fn input(&self, id: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_INPUT {
            return Err("That input is too long: send it in pieces.".into());
        }
        self.get(id)?.write(bytes)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        self.get(id)?.resize(cols, rows)
    }

    /// The output of shell `id` after cursor `after`, waiting up to `wait` for some.
    pub async fn read(&self, id: &str, after: u64, wait: Duration) -> Result<ReadOut> {
        let pty = self.get(id)?;
        let mut changed = pty.changed.subscribe();
        let deadline = tokio::time::Instant::now() + wait.min(MAX_WAIT);
        loop {
            changed.borrow_and_update();
            {
                let mut state = pty.state();
                state.used = Instant::now();
                let slice = state.output.read(after, self.limits.read_max);
                let exited = state.exited && slice.next == state.output.end();
                if !slice.bytes.is_empty() || slice.truncated || exited || tokio::time::Instant::now() >= deadline {
                    return Ok(ReadOut {
                        data: STANDARD.encode(&slice.bytes),
                        next: slice.next,
                        exited,
                        exit_code: if exited { state.exit_code } else { None },
                        truncated: slice.truncated,
                    });
                }
            }
            // A timeout reads once more, and answers with what there is.
            let _ = tokio::time::timeout_at(deadline, changed.changed()).await;
        }
    }

    /// Closes shell `id`: hangs it up and ends its process group. Waits a moment: call it off the async threads.
    pub fn close(&self, id: &str) -> Result<()> {
        let pty = self.take(id).ok_or_else(closed)?;
        pty.stop();
        Ok(())
    }

    /// Closes the shells nobody used for [`Limits::orphan`]: an app that went away without closing them.
    pub fn reap(&self) {
        let orphans: Vec<Arc<Pty>> = match self.open.lock() {
            Ok(mut open) => {
                let ids: Vec<String> = open
                    .iter()
                    .filter(|(_, pty)| pty.state().used.elapsed() >= self.limits.orphan)
                    .map(|(id, _)| id.clone())
                    .collect();
                ids.iter().filter_map(|id| open.remove(id)).collect()
            }
            Err(_) => Vec::new(),
        };
        for pty in orphans {
            pty.stop();
        }
    }

    /// Closes every shell: the host is stopping.
    pub fn close_all(&self) {
        let all: Vec<Arc<Pty>> = match self.open.lock() {
            Ok(mut open) => open.drain().map(|(_, pty)| pty).collect(),
            Err(_) => Vec::new(),
        };
        for pty in all {
            pty.stop();
        }
    }

    /// Answers a `wb_shell_*` call. `folder` finds a session's folder (`threadId`) or a project's (`projectId`).
    pub async fn answer(
        &self,
        call: &str,
        args: Value,
        folder: impl FnOnce(Option<&str>, Option<&str>) -> Result<PathBuf>,
    ) -> Result<Value> {
        match call {
            OPEN => {
                let open: OpenArgs = serde_json::from_value(args).map_err(|e| format!("Not a shell to open: {e}."))?;
                let dir = folder(open.thread_id.as_deref(), open.project_id.as_deref())?;
                let shells = self.clone();
                let id = blocking(move || shells.open(dir, &open)).await?;
                Ok(json!({ "id": id }))
            }
            INPUT => {
                let id = id_of(&args)?;
                let data = arg(&args, "data")?.as_str().ok_or("data is not base64.")?;
                let bytes = STANDARD.decode(data).map_err(|_| "data is not base64.".to_string())?;
                let shells = self.clone();
                blocking(move || shells.input(&id, &bytes)).await?;
                Ok(Value::Null)
            }
            RESIZE => {
                let (cols, rows) = size_of(&args)?;
                self.resize(&id_of(&args)?, cols, rows)?;
                Ok(Value::Null)
            }
            READ => {
                let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
                let wait = args.get("waitMs").and_then(Value::as_u64).unwrap_or(0);
                let read = self.read(&id_of(&args)?, after, Duration::from_millis(wait)).await?;
                serde_json::to_value(read).map_err(|e| e.to_string())
            }
            CLOSE => {
                let id = id_of(&args)?;
                let shells = self.clone();
                blocking(move || shells.close(&id)).await?;
                Ok(Value::Null)
            }
            _ => Err(format!("{call} is not a shell call.")),
        }
    }
}

#[cfg(test)]
#[path = "shell_pty_tests.rs"]
mod tests;
