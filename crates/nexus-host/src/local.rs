//! The desktop app's own background service: the same host as on a server, run on this computer so Code
//! sessions keep running when the app quits or crashes. The app starts it detached (`nexus-desktop service …`, or
//! `nexus-host local …` in the tests) and reaches it as it reaches a server's host, through nexus-remote's `local`
//! module, over a Unix socket only this person can open, with the token in the service's folder.
//!
//! What differs from a server:
//!
//! - Its folder is the app's data folder, so it opens the app's own event log (`workbench.sqlite3`), plan accounts
//!   and attachments. The app opens none of them while the service runs its sessions.
//! - It has no sign-in of its own. The app hands it the app's Gateway sign-in in each hello, and again when the
//!   person signs in to another organization (`host_sign_in`); the token stays in memory.
//! - It stops by itself once no app has been connected and no session has been busy for [`Options::idle_exit`], so a
//!   service whose app is gone (uninstalled, or the switch turned off while it was away) does not stay forever.
//! - Its agents get the built-in Nexus server, as the app's own do: the app's binary ([`Options::app_program`]) and
//!   the app's socket, which the service serves and passes on to the app ([`crate::app_relay`]).
//!
//! On stop (SIGTERM from the app, or the idle exit) it stops every agent as quitting the app does: their whole process
//! groups, so their MCP servers and shells end with them. A hangup is ignored: the service has no terminal.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexus_workbench::event::ThreadStatus;
use tokio::sync::watch;

use crate::files::Paths;
use crate::server::Server;
use crate::{Listen, Mode, Shell};

/// Where the service's hooks come from: the app's binary sets it before [`main`] (its settings.json and policy). A server's host sets none, so its sessions run no hook.
static HOOKS: std::sync::OnceLock<Arc<dyn nexus_workbench::hooks::Source>> = std::sync::OnceLock::new();

/// Sets the service's hooks' source, once, before [`main`].
pub fn set_hooks(source: Arc<dyn nexus_workbench::hooks::Source>) {
    let _ = HOOKS.set(source);
}

pub(crate) fn hooks() -> Option<Arc<dyn nexus_workbench::hooks::Source>> {
    HOOKS.get().cloned()
}

/// The probe of the plans' logins and limits that the workbench's rotation reads ([`nexus_workbench::Host::plans`]).
pub type PlanProbe = Arc<dyn Fn() -> Vec<nexus_workbench::plans::PlanUsage> + Send + Sync>;

/// Where the service reads its plans: the app's binary sets it before [`main`], with the probe its Settings page runs.
/// Without it, the service's rotation has no readings: it cannot skip a signed-out account or move near a limit. A
/// server's host sets none.
static PLANS: std::sync::OnceLock<PlanProbe> = std::sync::OnceLock::new();

/// Sets the service's plan probe, once, before [`main`].
pub fn set_plans(probe: PlanProbe) {
    let _ = PLANS.set(probe);
}

pub(crate) fn plans() -> Option<PlanProbe> {
    // Called as OnceLock::get: the desktop API inventory reads a method named get as a Gateway call.
    std::sync::OnceLock::get(&PLANS).cloned()
}

static SUBSESSIONS: std::sync::OnceLock<fn() -> nexus_workbench::Subsessions> = std::sync::OnceLock::new();

/// Supplies the local service with the desktop's saved subsession switch and limits, including when it starts before
/// a window opens.
pub fn set_subsessions_source(source: fn() -> nexus_workbench::Subsessions) {
    let _ = SUBSESSIONS.set(source);
}

pub(crate) fn subsessions() -> Option<nexus_workbench::Subsessions> {
    std::sync::OnceLock::get(&SUBSESSIONS).map(|read| read())
}

/// How long the service waits with no app and nothing busy before it stops, unless the app says otherwise.
pub const IDLE_EXIT: Duration = Duration::from_secs(15 * 60);

/// What the app tells its service on the command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The app's data folder: the token, the lock, the event log.
    pub data: PathBuf,
    /// Where it listens.
    pub socket: PathBuf,
    /// The app's worktree and review folders (Settings, Worktrees); None: `~/nexus-worktrees` and `~/nexus-reviews`.
    pub worktrees: Option<PathBuf>,
    pub reviews: Option<PathBuf>,
    pub idle_exit: Duration,
    /// The app's own binary, which serves the built-in Nexus server with `--mcp` ([`crate::app_relay`]). None: the
    /// agents get no built-in server.
    pub app_program: Option<PathBuf>,
}

impl Options {
    /// The flags, as [`Options::from_args`] reads them.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "--data".to_string(),
            self.data.to_string_lossy().into_owned(),
            "--socket".to_string(),
            self.socket.to_string_lossy().into_owned(),
            "--idle-exit".to_string(),
            self.idle_exit.as_secs().to_string(),
        ];
        for (flag, dir) in [
            ("--worktrees", &self.worktrees),
            ("--reviews", &self.reviews),
            ("--app-program", &self.app_program),
        ] {
            if let Some(dir) = dir {
                args.push(flag.to_string());
                args.push(dir.to_string_lossy().into_owned());
            }
        }
        args
    }

    pub fn from_args(args: &[String]) -> Result<Options, String> {
        let flag = |name: &str| {
            args.iter()
                .position(|arg| arg == name)
                .and_then(|at| args.get(at + 1))
                .filter(|value| !value.is_empty())
                .cloned()
        };
        let data = flag("--data").ok_or("Say where the app's data is: --data <folder>.")?;
        let socket = flag("--socket").ok_or("Say where to listen: --socket <path>.")?;
        let idle_exit = match flag("--idle-exit") {
            Some(secs) => Duration::from_secs(secs.parse().map_err(|_| format!("Not a number of seconds: {secs}"))?),
            None => IDLE_EXIT,
        };
        Ok(Options {
            data: PathBuf::from(data),
            socket: PathBuf::from(socket),
            worktrees: flag("--worktrees").map(PathBuf::from),
            reviews: flag("--reviews").map(PathBuf::from),
            idle_exit,
            app_program: flag("--app-program").map(PathBuf::from),
        })
    }

    /// The service's files: everything in the app's data folder but the socket.
    pub fn paths(&self) -> Paths {
        Paths {
            dir: self.data.clone(),
            socket_at: Some(self.socket.clone()),
        }
    }
}

/// Whether a session's agent is working, or waiting for the person.
fn busy(server: &Server) -> bool {
    server.api.workbench().threads().is_ok_and(|threads| {
        threads.iter().any(|t| {
            matches!(
                t.status,
                ThreadStatus::Starting | ThreadStatus::Running | ThreadStatus::Approval
            )
        })
    })
}

/// Sends `quit` once no client has been connected and no session busy for `limit`.
pub async fn exit_when_idle(server: Arc<Server>, limit: Duration, quit: watch::Sender<bool>) {
    let tick = (limit / 4).clamp(Duration::from_millis(100), Duration::from_secs(15));
    let mut idle_since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(tick).await;
        if server.hub.clients() > 0 || busy(&server) {
            idle_since = tokio::time::Instant::now();
        } else if idle_since.elapsed() >= limit {
            eprintln!(
                "nexus-host: no app and nothing running for {}s; stopping.",
                limit.as_secs()
            );
            let _ = quit.send(true);
            return;
        }
    }
}

/// The service, until SIGTERM or SIGINT (or its idle exit): `nexus-desktop service <flags>`, `nexus-host local
/// <flags>`. Returns the process's exit code.
pub fn main(args: &[String]) -> i32 {
    let options = match Options::from_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let served = runtime.block_on(async move {
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
                signal(SignalKind::hangup()),
            ) else {
                return;
            };
            loop {
                tokio::select! {
                    _ = term.recv() => break,
                    _ = int.recv() => break,
                    // No terminal to hang up: the app that started it closing is not a reason to stop.
                    _ = hup.recv() => continue,
                }
            }
            let _ = stop.send(true);
        });
        let home = nexus_client::config::home_dir(&|name| std::env::var(name).ok());
        let paths = options.paths();
        crate::serve_as(
            Mode::Local(options),
            paths,
            home,
            Listen::Socket,
            stopped,
            Shell::Login,
            |listening| {
                if let crate::Listening::Socket(path) = listening {
                    eprintln!("nexus-host: the app's service listens on {}", path.display());
                }
            },
        )
        .await
    });
    match served {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("nexus-host: {error}");
            1
        }
    }
}

/// Settings › Agents › Antigravity's Sandbox switch, as the app's binary reads it before [`main`] (its settings.json).
/// The service asks each time agy starts, as the app does. A server's host sets no reader.
static ANTIGRAVITY_SANDBOX: std::sync::OnceLock<Arc<dyn Fn() -> bool + Send + Sync>> = std::sync::OnceLock::new();

/// Sets how the service reads the Antigravity Sandbox switch, once, before [`main`].
pub fn set_antigravity_sandbox(read: Arc<dyn Fn() -> bool + Send + Sync>) {
    let _ = ANTIGRAVITY_SANDBOX.set(read);
}

/// The Antigravity Sandbox switch now; off when the app's binary set no reader.
pub(crate) fn antigravity_sandbox() -> bool {
    // Called as OnceLock::get: the desktop API inventory reads a method named get as a Gateway call.
    std::sync::OnceLock::get(&ANTIGRAVITY_SANDBOX).is_some_and(|read| read())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_flags_go_and_come_back() {
        let options = Options {
            data: "/d/x y".into(),
            socket: "/run/user/1/nexus/s.sock".into(),
            worktrees: Some("/w".into()),
            reviews: None,
            idle_exit: Duration::from_secs(90),
            app_program: Some("/Applications/Nexus.app/Contents/MacOS/nexus-desktop".into()),
        };
        assert_eq!(Options::from_args(&options.to_args()), Ok(options.clone()));
        let paths = options.paths();
        assert_eq!(paths.store(), PathBuf::from("/d/x y/workbench.sqlite3"));
        assert_eq!(paths.socket(), PathBuf::from("/run/user/1/nexus/s.sock"));
        assert_eq!(paths.lock(), PathBuf::from("/d/x y/run/host.lock"));
        let bare = Options::from_args(&["--data".into(), "/d".into(), "--socket".into(), "/s".into()]);
        assert_eq!(bare.map(|o| o.idle_exit), Ok(IDLE_EXIT));
        assert!(Options::from_args(&["--data".into(), "/d".into()]).is_err());
    }
}
