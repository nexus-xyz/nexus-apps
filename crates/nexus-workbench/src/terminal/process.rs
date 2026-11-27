//! Owns each command process and reaps it. Reader and input threads never block the app or the agent runtime.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use nexus_client::commit_trailer;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::Execution;
use crate::process::Shell;

pub(super) enum Control {
    Stop,
    Resize(u16, u16),
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Counts the cursor position queries (DSR, `ESC[6n`) in `bytes`, with a query split across reads. `tail` keeps the
/// previous read's last bytes.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn cursor_queries(tail: &mut Vec<u8>, bytes: &[u8]) -> usize {
    const QUERY: &[u8] = b"\x1b[6n";
    tail.extend_from_slice(bytes);
    let count = tail.windows(QUERY.len()).filter(|window| *window == QUERY).count();
    tail.drain(..tail.len().saturating_sub(QUERY.len() - 1));
    count
}

/// Hangs up a shell, then ends its process group. Also the person's shells on a host (`crate::shell_pty`).
pub(crate) fn stop(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|id| i32::try_from(id).ok()) {
        // Interactive shells put jobs in separate process groups. Hang up the shell first so it ends those jobs.
        // The worker has not reaped it, so this PID stays reserved during the grace period.
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGHUP);
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::process::kill(pid);
}

/// The pagers an agent's command never opens: each would wait at its prompt for a person. `GIT_PAGER` and
/// `GH_PAGER` win over the person's git and gh config. A command that sets its own value still overrides these.
pub(crate) const NO_PAGER: [(&str, &str); 3] = [("PAGER", "cat"), ("GIT_PAGER", "cat"), ("GH_PAGER", "cat")];

/// The app's variables a terminal command gets: what a shell and the usual tools need to find programs, the home
/// folder, the locale, temporary files, the SSH agent, the display, the proxy and certificate settings, and the desktop session's bus and signing. Everything else (the app's tokens, keys and
/// any secret it was started with) stays out.
const PASSED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    // The session's network setup, which no shell profile restores: proxies (both cases, Unix tools read lowercase)
    // and custom certificate authorities.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    // The desktop session's: the X11 cookie, the D-Bus address (credential helpers, notifications) and signing.
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "GPG_TTY",
    "GNUPGHOME",
    // The person's pager: a command they wrote keeps it; an agent's gets [`NO_PAGER`] instead.
    "PAGER",
    "GIT_PAGER",
    "GH_PAGER",
    // git's own `-c` settings: a desktop started from a Nexus session keeps its commit trailer hook.
    commit_trailer::VARIABLE,
    // What Windows programs need to start.
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "TEMP",
    "TMP",
];

/// Variables passed by their prefix: the locale (`LC_*`) and the XDG folders (`XDG_*`).
const PASSED_PREFIXES: &[&str] = &["LC_", "XDG_"];

/// Whether a terminal command gets the app's variable `key`. Windows reads names in any case.
fn passed(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    let key = if cfg!(windows) { upper.as_str() } else { key };
    PASSED.contains(&key) || PASSED_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

/// A command's environment, for a terminal: only the app's variables in [`PASSED`], so none of its secrets. `marked`:
/// the commit trailer hook, as an agent session's own environment has it, since agents run `git commit`
/// here and not in their process. `agent_written`: no pager ([`NO_PAGER`]); a command the person wrote keeps theirs.
pub(crate) fn environment(
    app: impl IntoIterator<Item = (String, String)>,
    marked: bool,
    agent_written: bool,
) -> HashMap<String, String> {
    let mut env: HashMap<_, _> = app.into_iter().filter(|(key, _)| passed(key)).collect();
    if marked {
        commit_trailer::mark(&mut env, commit_trailer::Surface::Desktop);
    }
    if agent_written {
        for (key, value) in NO_PAGER {
            env.insert(key.into(), value.into());
        }
    }
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env
}

/// Starts the execution's process in `shell` ([`Shell::Login`] in the apps).
pub(super) fn start(
    entry: &Arc<Execution>,
    human: bool,
    shell: &Shell,
    expected: Option<&str>,
    marked: bool,
) -> Result<(), String> {
    let mut state = entry.state();
    // A retry attaches to the existing process, including after it finished.
    if state.info.status != "awaitingUser" {
        return Ok(());
    }
    if expected.is_some_and(|expected| expected != state.info.command) {
        return Err("terminal_command_changed: Review the command again.".into());
    }
    if state.info.restored {
        return Err("terminal_execution_expired: Recovered commands cannot be replayed; submit a new request.".into());
    }
    state.info.status = "starting".into();
    if let Err(error) = entry.persist(&state, "starting") {
        state.info.status = "unknown".into();
        return Err(error);
    }
    let launched = (|| -> Result<_, String> {
        let pair = native_pty_system().openpty(size(100, 30)).map_err(|e| e.to_string())?;
        if let Err(error) = crate::pty::utf8_line_editing(&*pair.master) {
            eprintln!(
                "nexus-workbench: terminal {}: UTF-8 line editing: {error}",
                state.info.id
            );
        }
        // A command an agent wrote runs below the app, as the agent does (`crate::process::AGENT_NICENESS`): a build
        // it starts here yields the CPU to the window. A command the person wrote or edited keeps their priority.
        #[cfg(unix)]
        let shell_command = |program: &std::path::Path, flag: &str| {
            let (program, before) = if state.info.agent_written {
                crate::process::below_the_app(program)
            } else {
                (program.to_path_buf(), Vec::new())
            };
            let mut command = CommandBuilder::new(program);
            command.args(before);
            command.args([flag, state.info.command.as_str()]);
            command
        };
        #[cfg(unix)]
        let mut command = match (shell, state.info.interactive) {
            (Shell::Login, true) => CommandBuilder::new_default_prog(),
            (Shell::Login, false) => shell_command(
                std::path::Path::new(&std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())),
                "-lc",
            ),
            (Shell::Plain(program), true) => CommandBuilder::new(program),
            (Shell::Plain(program), false) => shell_command(std::path::Path::new(program), "-c"),
        };
        #[cfg(windows)]
        let _ = shell;
        #[cfg(windows)]
        let mut command = {
            if state.info.interactive {
                CommandBuilder::new_default_prog()
            } else {
                let mut command = CommandBuilder::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
                command.args(["/D", "/S", "/C", state.info.command.as_str()]);
                command
            }
        };
        command.env_clear();
        for (key, value) in environment(std::env::vars(), marked, state.info.agent_written) {
            command.env(key, value);
        }
        command.cwd(&state.info.cwd);
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
        drop(pair.slave);
        Ok((pair.master, reader, writer, child))
    })();
    let (master, mut reader, mut writer, mut child) = match launched {
        Ok(launched) => launched,
        Err(error) => {
            state.info.status = "failed".into();
            entry.persist_result(&state, "failedToStart");
            return Err(format!("terminal_spawn_failed: {error}"));
        }
    };
    let (input, typed) = mpsc::sync_channel::<Vec<u8>>(32);
    #[cfg(windows)]
    let replies = input.clone();
    let (control, commands) = mpsc::channel();
    state.input = Some(input);
    state.control = Some(control);
    state.info.status = "running".into();
    state.info.started_at = Some(crate::now_ms());
    state.last_output = Some(std::time::Instant::now());
    state.info.owner = if human { "user" } else { "agent" }.into();
    state.info.lease += 1;
    entry.persist_result(&state, "running");
    drop(state);
    entry.changed();
    std::thread::spawn(move || {
        for data in typed {
            if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });
    let output = entry.clone();
    let (drained, output_done) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = [0; 16 * 1024];
        #[cfg(windows)]
        let mut tail = Vec::new();
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut state = output.state();
                    state.output.push(&bytes[..n]);
                    state.last_output = Some(std::time::Instant::now());
                    // ConPTY holds a command's output until its cursor query is answered. A screen answers only for
                    // a person's command (GPUI's `accepts_input`), so answer the agent's. Two answers re-run a
                    // command: conhost reads the second one as F3.
                    #[cfg(windows)]
                    for _ in 0..cursor_queries(&mut tail, &bytes[..n]) {
                        if state.info.owner == "agent" {
                            let _ = replies.try_send(b"\x1b[1;1R".to_vec());
                        }
                    }
                }
            }
        }
        let _ = drained.send(());
    });
    let runtime = tokio::runtime::Handle::try_current().ok();
    let entry = entry.clone();
    std::thread::spawn(move || {
        let pid = child.process_id();
        let mut stopped = false;
        loop {
            while let Ok(command) = commands.try_recv() {
                match command {
                    Control::Resize(cols, rows) => {
                        let _ = master.resize(size(cols, rows));
                    }
                    Control::Stop => {
                        // The child is not reaped yet, so its PID cannot have been reused.
                        stop(pid);
                        #[cfg(windows)]
                        let _ = child.kill();
                        stopped = true;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(exit)) => {
                    crate::process::end_leftovers(pid);
                    // Give final bytes a bounded chance to arrive before reporting completion.
                    let _ = output_done.recv_timeout(Duration::from_millis(300));
                    let mut state = entry.state();
                    state.info.exit_code = Some(exit.exit_code());
                    if state.info.status != "interrupted" {
                        state.info.status = if stopped {
                            "interrupted"
                        } else if exit.success() {
                            "completed"
                        } else {
                            "failed"
                        }
                        .into();
                    }
                    state.input = None;
                    state.control = None;
                    state.info.ended_at = Some(crate::now_ms());
                    entry.persist_result(&state, "ended");
                    if state.watch.exit() {
                        entry.notify_exit(runtime.as_ref());
                    }
                    drop(state);
                    entry.changed();
                    break;
                }
                Err(error) => {
                    stop(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    let mut state = entry.state();
                    state.output.push(error.to_string().as_bytes());
                    state.info.status = "failed".into();
                    state.input = None;
                    state.control = None;
                    state.info.ended_at = Some(crate::now_ms());
                    entry.persist_result(&state, "ended");
                    if state.watch.exit() {
                        entry.notify_exit(runtime.as_ref());
                    }
                    drop(state);
                    entry.changed();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            }
        }
    });
    Ok(())
}
