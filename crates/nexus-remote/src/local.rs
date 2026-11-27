//! The app's own background service on this computer: `nexus-host` run from the app's own binary, so Code
//! sessions keep running when the app quits or crashes (nexus-host's src/local.rs is the other side). A
//! [`LocalService`] is a [`Link`]: a [`crate::machine::Remote`] over it reconnects, catches up from the event log and
//! replaces a service of another version once it is idle, exactly as for a server.
//!
//! - **Started detached.** Its own process group, standard input and output closed, its errors in `service.log` in the
//!   app's data folder. It is not the app's child for long: when the app quits or crashes it keeps running.
//! - **Reached only by this person.** A Unix socket (0600, in a 0700 folder), never a TCP port, and the token in the
//!   service's folder (0600) in the hello.
//! - **Signed in by the app.** Each hello carries the app's Gateway sign-in; with none (signed out) no service starts.
//! - **Stopped with its agents.** SIGTERM: the service stops every agent's whole process group, as quitting the app
//!   does, then lets go of its lock. One that has not after a minute is killed.
//!
//! The files are nexus-host's layout (its src/files.rs): `token`, `run/host.lock` (held while it runs, with its
//! process id) and `workbench.sqlite3`, all in the app's data folder; the socket where the app says.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::Result;
use crate::machine::{Link, Opened};

/// How long a new service has to start listening.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a stopping service has to stop its agents before it is killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);

/// The log is started over once it is this big.
const LOG_LIMIT: u64 = 1 << 20;

/// The app's Gateway sign-in now, as the service takes it (`{ gateway, token, scope }`); None when signed out.
pub type SignIn = Arc<dyn Fn() -> Option<Value> + Send + Sync>;

/// The app's service: how to start it and where it is.
#[derive(Clone)]
pub struct LocalService {
    /// The binary, and the words before the flags: `nexus-desktop service`, or `nexus-host local` in the tests.
    pub program: PathBuf,
    pub command: Vec<String>,
    /// The app's data folder: the service's token, lock, log and event log.
    pub data: PathBuf,
    /// Where it listens.
    pub socket: PathBuf,
    /// The app's worktree and review folders.
    pub worktrees: Option<PathBuf>,
    pub reviews: Option<PathBuf>,
    /// How long it stays with no app and nothing busy.
    pub idle_exit: Duration,
    /// The app's own binary, for the built-in Nexus server its agents start (`--mcp`). None: they get none.
    pub app_program: Option<PathBuf>,
    /// More environment for the service and so its agents (the tests' stand-in agents).
    pub env: Vec<(String, String)>,
    /// The app's version: a service of another version is replaced once it is idle. None: never.
    pub version: Option<String>,
    pub sign_in: SignIn,
}

impl LocalService {
    fn lock(&self) -> PathBuf {
        self.data.join("run").join("host.lock")
    }

    fn token_path(&self) -> PathBuf {
        self.data.join("token")
    }

    pub fn log(&self) -> PathBuf {
        self.data.join("service.log")
    }

    /// The flags nexus-host's `local::Options` reads.
    fn flags(&self) -> Vec<String> {
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

    /// The running service's process id; None when none runs.
    pub fn running(&self) -> Option<i32> {
        held(&self.lock()).then(|| {
            std::fs::read_to_string(self.lock())
                .ok()
                .and_then(|pid| pid.trim().parse().ok())
        })?
    }

    fn token(&self) -> Result<String> {
        let token = std::fs::read_to_string(self.token_path())
            .map_err(|e| format!("Could not read the background service's token: {e}"))?;
        let token = token.trim();
        if token.len() < 32 {
            return Err("The background service's token is not there yet.".into());
        }
        Ok(token.to_string())
    }

    fn ready(&self) -> bool {
        self.running().is_some() && self.socket.exists() && self.token().is_ok()
    }

    /// Starts the service unless one runs, and waits until it listens.
    pub async fn start(&self) -> Result<()> {
        if self.ready() {
            return Ok(());
        }
        if self.running().is_none() {
            self.spawn()?;
        }
        let started = Instant::now();
        while started.elapsed() < START_TIMEOUT {
            if self.ready() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(format!(
            "The background service did not start{}",
            last_line(&self.log())
                .map(|line| format!(": {line}"))
                .unwrap_or_else(|| ".".into())
        ))
    }

    fn spawn(&self) -> Result<()> {
        use std::os::unix::process::CommandExt as _;
        create_data(&self.data)?;
        let log = open_log(&self.log())?;
        let mut command = nexus_workbench::process::command(&self.program);
        command
            .args(&self.command)
            .args(self.flags())
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log)
            // Its own group: a Ctrl-C or a hangup meant for the app's terminal does not reach it.
            .process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start the background service: {e}"))?;
        // Reaped when it ends while the app still runs; after the app is gone, the system reaps it.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// Stops the running service and waits for it: its agents, their MCP servers and shells end first. True when one
    /// ran.
    pub async fn stop(&self) -> Result<bool> {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let Some(pid) = self.running() else {
            return Ok(false);
        };
        kill(Pid::from_raw(pid), Signal::SIGTERM)
            .map_err(|e| format!("Could not stop the background service ({pid}): {e}"))?;
        let started = Instant::now();
        while started.elapsed() < STOP_TIMEOUT {
            if !held(&self.lock()) {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        tokio::time::sleep(Duration::from_millis(500)).await;
        Err(format!(
            "The background service ({pid}) did not stop within a minute and was killed."
        ))
    }

    /// End a sign-in's service and wait for the store lock. Leave a slow service running for Retry.
    pub async fn stop_for_sign_out(&self) -> Result<()> {
        self.wait_for_sign_out(STOP_TIMEOUT).await
    }

    async fn wait_for_sign_out(&self, timeout: Duration) -> Result<()> {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let lock = self.lock();
        if !held(&lock) {
            return Ok(());
        }
        let pid = self.running().filter(|pid| *pid > 0);
        if let Some(pid) = pid
            && let Err(error) = kill(Pid::from_raw(pid), Signal::SIGTERM)
            && held(&lock)
        {
            return Err(format!("Could not stop the background service: {error}"));
        }
        let started = Instant::now();
        while held(&lock) {
            if started.elapsed() >= timeout {
                return Err("The background service still holds the sessions store. Try again.".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    async fn connect(&self) -> Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(&self.socket)
            .await
            .map_err(|e| format!("Could not reach the background service: {e}"))
    }
}

impl Link for LocalService {
    fn open(&self) -> BoxFuture<'_, Result<Opened>> {
        Box::pin(async move {
            let sign_in = (self.sign_in)().ok_or("Sign in to Nexus to run Code sessions.")?;
            self.start().await?;
            let stream = match self.connect().await {
                Ok(stream) => stream,
                // Running but not reachable (its socket was removed with the runtime folder): start it over.
                Err(_) => {
                    self.stop().await?;
                    self.start().await?;
                    self.connect().await?
                }
            };
            Ok(Opened {
                stream: Box::new(stream),
                token: self.token()?,
                version: self.version.clone(),
                keep: None,
                sign_in: Some(sign_in),
            })
        })
    }

    /// Replaces the service with this app's version. The Remote asks only when no session is busy.
    fn update(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.stop().await?;
            self.start().await
        })
    }
}

/// A service of another version that was busy when the app connected is replaced once it is idle: checked every
/// minute until the relay closes.
pub async fn update_when_idle(remote: crate::machine::Remote) {
    use crate::machine::Status;
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if !matches!(remote.status(), Status::Connected { update: Some(_), .. }) {
            if remote.is_closed() {
                return;
            }
            continue;
        }
        let idle = remote.sessions_running().await == Some(false);
        if idle && let Err(error) = remote.update().await {
            eprintln!("nexus-desktop: background service: update: {error}");
        }
    }
}

/// Whether a process holds `lock` (nexus-host's `Lock`, an exclusive `flock`).
fn held(lock: &Path) -> bool {
    let Ok(file) = std::fs::File::open(lock) else {
        return false;
    };
    matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// The data folder, made if missing (the service makes it private).
fn create_data(data: &Path) -> Result<()> {
    std::fs::create_dir_all(data).map_err(|e| format!("Could not create {}: {e}", data.display()))
}

/// `service.log`, readable by this person only, started over once it is large.
fn open_log(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let large = std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT);
    std::fs::OpenOptions::new()
        .create(true)
        .append(!large)
        .write(true)
        .truncate(large)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
}

fn last_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(data: PathBuf) -> LocalService {
        LocalService {
            program: PathBuf::from("/nonexistent/nexus-desktop"),
            command: vec!["service".into()],
            data,
            socket: PathBuf::from("/run/user/1/nexus-desktop/service.sock"),
            worktrees: Some(PathBuf::from("/w")),
            reviews: None,
            idle_exit: Duration::from_secs(60),
            app_program: Some(PathBuf::from("/nonexistent/nexus-desktop")),
            env: vec![],
            version: None,
            sign_in: Arc::new(|| None),
        }
    }

    #[test]
    fn says_where_its_files_are() {
        let local = service(PathBuf::from("/d"));
        assert_eq!(
            local.flags(),
            [
                "--data",
                "/d",
                "--socket",
                "/run/user/1/nexus-desktop/service.sock",
                "--idle-exit",
                "60",
                "--worktrees",
                "/w",
                "--app-program",
                "/nonexistent/nexus-desktop"
            ]
        );
        assert_eq!(local.running(), None);
    }

    #[tokio::test]
    async fn sign_out_retry_waits_for_a_service_with_a_held_lock() {
        let data = std::env::temp_dir().join(format!("nexus-remote-sign-out-{}", std::process::id()));
        let local = service(data);
        std::fs::create_dir_all(local.data.join("run")).expect("service run directory");
        let lock = std::fs::File::create(local.lock()).expect("service lock file");
        lock.lock().expect("hold service lock");
        assert!(local.wait_for_sign_out(Duration::ZERO).await.is_err());
        assert!(held(&local.lock()));
        drop(lock);
        assert!(local.stop_for_sign_out().await.is_ok());
    }

    #[tokio::test]
    async fn signed_out_it_starts_nothing() {
        let data = std::env::temp_dir().join(format!("nexus-remote-local-{}", std::process::id()));
        let local = service(data.clone());
        let opened = local.open().await;
        assert_eq!(opened.err(), Some("Sign in to Nexus to run Code sessions.".to_string()));
        assert!(!data.exists());
    }
}
