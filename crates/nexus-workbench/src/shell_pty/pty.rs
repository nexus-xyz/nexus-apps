//! One shell in a pseudo-terminal: its process, a thread that keeps its output, and a thread that reaps it.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::watch;

use super::output::Output;
use crate::process::Shell;

/// How long a reaped shell's last bytes may take to reach its output.
const LAST_BYTES: Duration = Duration::from_millis(300);

pub(super) struct State {
    pub output: Output,
    pub exited: bool,
    pub exit_code: Option<u32>,
    /// The last read, write or resize: a shell nobody used for long is an orphan ([`super::Shells::reap`]).
    pub used: Instant,
}

pub(super) struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn std::io::Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    state: Mutex<State>,
    /// Moves on with each output and at the exit, for a read that waits.
    pub changed: watch::Sender<u64>,
}

/// What to start: the person's login shell, or `command` in it.
pub(super) struct Start<'a> {
    pub folder: &'a Path,
    pub cols: u16,
    pub rows: u16,
    pub command: Option<&'a str>,
    pub shell: &'a Shell,
    /// The agents' `PATH` on this machine; empty: the host's own.
    pub path: String,
    pub buffer: usize,
}

pub(super) fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.clamp(2, 1000),
        rows: rows.clamp(1, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The processes in session `leader` (the shell's), but the shell itself: Linux's `/proc`. None elsewhere, where the
/// hangup and the process group are what ends a shell's jobs.
fn session_members(leader: u32) -> Vec<i32> {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| u32::try_from(*pid).ok() != Some(leader))
            .filter(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .and_then(|stat| session_of(&stat))
                    .is_some_and(|session| u32::try_from(session).ok() == Some(leader))
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = leader;
        Vec::new()
    }
}

/// The session id in a `/proc/<pid>/stat` line: the fourth field after the command's closing parenthesis.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn session_of(stat: &str) -> Option<i32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(3)?.parse().ok()
}

fn kill_all(pids: &[i32]) {
    #[cfg(unix)]
    for pid in pids {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), nix::sys::signal::Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pids;
}

#[cfg(unix)]
fn command(shell: &Shell, command: Option<&str>) -> CommandBuilder {
    match (shell, command) {
        (Shell::Login, None) => CommandBuilder::new_default_prog(),
        (Shell::Login, Some(command)) => {
            let mut builder = CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
            builder.args(["-lc", command]);
            builder
        }
        (Shell::Plain(program), None) => CommandBuilder::new(program),
        (Shell::Plain(program), Some(command)) => {
            let mut builder = CommandBuilder::new(program);
            builder.args(["-c", command]);
            builder
        }
    }
}

#[cfg(not(unix))]
fn command(_shell: &Shell, command: Option<&str>) -> CommandBuilder {
    match command {
        None => CommandBuilder::new_default_prog(),
        Some(command) => {
            let mut builder = CommandBuilder::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
            builder.args(["/D", "/S", "/C", command]);
            builder
        }
    }
}

impl Pty {
    /// Starts the shell in `start.folder`, with the agents' environment, `TERM=xterm-256color` and true color.
    pub(super) fn start(start: Start<'_>) -> Result<Arc<Pty>, String> {
        if !start.folder.is_dir() {
            return Err(format!("{} is not a folder on this machine.", start.folder.display()));
        }
        let pair = native_pty_system()
            .openpty(size(start.cols, start.rows))
            .map_err(|e| e.to_string())?;
        if let Err(error) = crate::pty::utf8_line_editing(&*pair.master) {
            eprintln!("nexus-workbench: shell: UTF-8 line editing: {error}");
        }
        let mut builder = command(start.shell, start.command);
        builder.env_clear();
        for (key, value) in crate::terminal::process::environment(std::env::vars(), false, false) {
            builder.env(key, value);
        }
        if !start.path.is_empty() {
            builder.env("PATH", &start.path);
        }
        builder.cwd(start.folder);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let mut child = pair.slave.spawn_command(builder).map_err(|e| e.to_string())?;
        // The shell holds its own end now; ours would keep the output open after it ends.
        drop(pair.slave);
        let pid = child.process_id();
        let pty = Arc::new(Pty {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(child.clone_killer()),
            pid,
            state: Mutex::new(State {
                output: Output::new(start.buffer),
                exited: false,
                exit_code: None,
                used: Instant::now(),
            }),
            changed: watch::Sender::new(0),
        });
        let (drained, output_done) = mpsc::channel::<()>();
        let output = pty.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        output.state().output.push(&buffer[..read]);
                        output.changed.send_modify(|n| *n += 1);
                    }
                }
            }
            let _ = drained.send(());
        });
        let reaped = pty.clone();
        std::thread::spawn(move || {
            let code = child.wait().ok().map(|status| status.exit_code());
            // What it left running in its process group goes too.
            crate::process::end_leftovers(pid);
            let _ = output_done.recv_timeout(LAST_BYTES);
            {
                let mut state = reaped.state();
                state.exited = true;
                state.exit_code = code;
            }
            reaped.changed.send_modify(|n| *n += 1);
        });
        Ok(pty)
    }

    pub(super) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Types `bytes`. It may wait while the shell reads nothing: call it off the async threads.
    pub(super) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        self.state().used = Instant::now();
        let mut writer = self.writer.lock().map_err(|_| "The shell is gone.".to_string())?;
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|e| format!("The shell took no input: {e}"))
    }

    pub(super) fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.state().used = Instant::now();
        self.master
            .lock()
            .map_err(|_| "The shell is gone.".to_string())?
            .resize(size(cols, rows))
            .map_err(|e| e.to_string())
    }

    /// Hangs the shell up and ends its process group, then everything else in its session: an interactive shell puts
    /// each job in a group of its own, which the hangup does not always reach. A process that left the session (a
    /// daemon, `tmux`) stays. Waits a moment: call it off the async threads.
    pub(super) fn stop(&self) {
        // Once reaped, its id may name another process's session: what an ended shell left is not ours to end.
        if self.state().exited {
            return;
        }
        // Read before the hangup: the session's members are found by the shell's id, which leads it.
        let members = self.pid.map(session_members).unwrap_or_default();
        crate::terminal::process::stop(self.pid);
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
        kill_all(&members);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_session_from_a_stat_line_whatever_the_command_is_called() {
        let stat = "4242 (my (odd) cmd) S 1 4242 4200 34816 4242 4194560 0 0";
        assert_eq!(super::session_of(stat), Some(4200));
        assert_eq!(super::session_of("garbage"), None);
    }
}
