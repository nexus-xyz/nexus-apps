//! The host's folder, `~/.nexus/host` (or `NEXUS_HOST_DIR`), readable only by its person (0700, files 0600):
//!
//! - `token`: the second lock on the socket, made on first start. The app
//!   reads it over SSH each time it connects.
//! - `credentials`: the host's own Gateway sign-in ([`Credentials`], section 4).
//! - `device-key` and `session-sync-keys.sqlite3`: the server's device key and the content keys it holds, for session
//!   sync ([`crate::session_sync`]).
//! - `workbench.sqlite3`: the sessions and their event logs.
//! - `native-sessions.json`: on a server, the native sessions Gateway started here, by thread ([`crate::session_runner`]).
//! - `run/host.sock`: the WebSocket; `run/host.port` when it listens on a loopback port instead; `run/host.lock`: one
//!   host per person. The desktop app's own background service ([`crate::local`]) puts its socket elsewhere
//!   ([`Paths::socket_at`]).

use std::fs::File;
use std::path::{Path, PathBuf};

use nexus_client::config::{create_private_dir, write_private};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub dir: PathBuf,
    /// The socket, when not `run/host.sock`: the desktop app's service listens in the person's runtime folder.
    pub socket_at: Option<PathBuf>,
}

impl Paths {
    /// Everything in `dir`.
    pub fn at(dir: PathBuf) -> Paths {
        Paths { dir, socket_at: None }
    }

    /// `NEXUS_HOST_DIR`, else `<home>/.nexus/host`.
    pub fn from_env(home: &Path) -> Paths {
        let dir = std::env::var_os("NEXUS_HOST_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".nexus").join("host"));
        Paths::at(dir)
    }

    pub fn token(&self) -> PathBuf {
        self.dir.join("token")
    }

    pub fn credentials(&self) -> PathBuf {
        self.dir.join("credentials")
    }

    /// The server's device key for session sync ([`crate::session_sync`]), beside the token.
    pub fn device_key(&self) -> PathBuf {
        self.dir.join("device-key")
    }

    /// Session sync's key store: the content keys locked to the device key, and the device directory's memory.
    pub fn session_sync_keys(&self) -> PathBuf {
        self.dir.join("session-sync-keys.sqlite3")
    }

    /// On a cloud machine only: its id and Gateway, written by Gateway at create ([`crate::cloud`]).
    pub fn cloud(&self) -> PathBuf {
        self.dir.join("cloud.json")
    }

    /// On a person's own server that `login --pair` paired: `cloud.json`'s shape, read by the line out
    /// only. The heartbeat, the setup and the history upload read `cloud.json` alone, so they never run here.
    pub fn paired(&self) -> PathBuf {
        self.dir.join(PAIRED)
    }

    /// On a cloud machine only: where its own setup at boot stands (nexus-workbench `cloud_setup`).
    pub fn setup(&self) -> PathBuf {
        self.dir.join("setup.json")
    }

    pub fn store(&self) -> PathBuf {
        self.dir.join("workbench.sqlite3")
    }

    /// On a server: the native sessions Gateway started here, by thread ([`crate::session_runner`]).
    pub fn native_sessions(&self) -> PathBuf {
        self.dir.join(crate::session_runner::SAVED)
    }

    /// The socket, the port file and the lock.
    pub fn run(&self) -> PathBuf {
        self.dir.join("run")
    }

    pub fn socket(&self) -> PathBuf {
        self.socket_at.clone().unwrap_or_else(|| self.run().join("host.sock"))
    }

    pub fn port(&self) -> PathBuf {
        self.run().join("host.port")
    }

    pub fn lock(&self) -> PathBuf {
        self.run().join("host.lock")
    }

    pub fn accounts(&self) -> PathBuf {
        self.dir.join("accounts")
    }

    /// Files attached to messages, a folder a session.
    pub fn attachments(&self) -> PathBuf {
        self.dir.join("attachments")
    }

    /// The folder and `run/` made private (also when they were there already, with a wider mode). The socket's folder
    /// too, but only when this call creates it: `--socket /tmp/x.sock` must not change the mode of `/tmp`. A socket
    /// folder that was there and that others can write to gets a warning on stderr, not a refusal: the socket is 0600
    /// once bound and every connection still needs the token, and `/tmp` is a common choice.
    pub fn create(&self) -> Result<()> {
        let socket_dir = self
            .socket_at
            .as_deref()
            .and_then(Path::parent)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let dirs = [(self.dir.clone(), true), (self.run(), true)]
            .into_iter()
            .chain(socket_dir.map(|dir| (dir, false)));
        for (dir, own) in dirs {
            let made = make_dir(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
            if own || made {
                set_mode(&dir, 0o700)?;
            } else if let Some(warning) = open_socket_folder(&dir) {
                eprintln!("nexus-host: {warning}");
            }
        }
        Ok(())
    }
}

/// Makes `dir` (0700, and any missing parent). `true` when this call made `dir` itself, `false` when it was there:
/// the create's own answer, so no check runs before it and nothing can come between the two.
fn make_dir(dir: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::DirBuilderExt as _;
    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        create_private_dir(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(false),
        Err(e) => Err(e),
    }
}

/// The warning for a socket folder that was there and that its group or others can write to. Until the socket is
/// made 0600 after the bind, it has the umask's mode: a private folder closed that window, a shared one does not.
fn open_socket_folder(dir: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(dir).ok()?.permissions().mode();
    (mode & 0o022 != 0).then(|| {
        format!(
            "warning: the socket's folder {} can be written by other users (mode {:o}). The socket is guarded by its \
             token; for a private folder, pass a --socket path in a folder only you can write to.",
            dir.display(),
            mode & 0o7777
        )
    })
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("Could not protect {}: {e}", path.display()))
}

/// Makes a file that others can read or write 0600. Best effort: the file stays as it was on an error.
pub(crate) fn make_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let wide = std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o077 != 0);
    if wide {
        let _ = set_mode(path, 0o600);
    }
}

/// [`Paths::paired`]'s name in the host's folder.
pub const PAIRED: &str = "paired.json";

/// Writes `path` readable by its person only, whatever mode it had.
pub(crate) fn save_private(path: &Path, text: &str) -> Result<()> {
    write_private(path, text.as_bytes()).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    set_mode(path, 0o600)
}

/// The token in `path`, made (random, 0600) when there is none.
pub fn token(path: &Path) -> Result<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let text = text.trim();
        if text.len() >= 32 {
            return Ok(text.to_string());
        }
    }
    let token = new_token();
    save_private(path, &format!("{token}\n"))?;
    Ok(token)
}

/// The socket's token is made and checked as the agents' MCP grants are.
pub use nexus_workbench::orchestra::grant::{new_token, same_token};

/// The host's own Gateway sign-in (`nexus-host login`): its own device token, named after the machine and revocable
/// in Nexus UI. One organization per host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub gateway: String,
    pub token: String,
    /// The organization's slug (or id), for the sessions' scope and `status`.
    pub org: Option<String>,
    /// The label the approval page showed.
    pub label: String,
}

impl Credentials {
    /// The saved sign-in. One saved on a retired Gateway host (`nexus_client::config::moved_gateway`) moves to
    /// production's, and the file is saved with it (best effort). On a cloud machine Gateway writes the file with
    /// the provider's default mode, so a file others can read is made 0600 here (best effort).
    pub fn load(path: &Path) -> Option<Credentials> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut credentials: Credentials = serde_json::from_str(&text).ok()?;
        make_private(path);
        if let Some(moved) = nexus_client::config::moved_gateway(&credentials.gateway) {
            credentials.gateway = moved.to_string();
            let _ = credentials.save(path);
        }
        Some(credentials)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        save_private(path, &format!("{text}\n"))
    }
}

/// How long [`Lock::take`] waits out a probe's shared lock before it says a host already runs.
const TAKE_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
/// How often [`Lock::take`] tries the lock again while it waits.
const TAKE_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

/// Held while a host runs: a second `serve` fails after [`TAKE_WAIT`] instead of fighting over the socket. The system
/// lets go of it when the process ends, however it ends. The file holds the host's process id, for `nexus-host stop`
/// (the app's "Update now").
pub struct Lock {
    _file: Flock<File>,
}

impl Lock {
    /// Blocks the calling thread for up to [`TAKE_WAIT`] while the lock is busy.
    pub fn take(path: &Path) -> Result<Lock> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
        let file = Lock::exclusive(file, path)?;
        {
            use std::io::Write as _;
            let mut held: &File = &file;
            let _ = file.set_len(0);
            let _ = held.write_all(format!("{}\n", std::process::id()).as_bytes());
        }
        Ok(Lock { _file: file })
    }

    /// Retries while the lock is busy: two probers hold a shared lock for an instant, `status` ([`Lock::held`]) and
    /// the desktop app's service (`held()` in nexus-remote's `local.rs`). Both run while a new host starts, so the
    /// app's "Update now" could otherwise refuse it. Any other error fails at once.
    fn exclusive(mut file: File, path: &Path) -> Result<Flock<File>> {
        let started = std::time::Instant::now();
        loop {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(locked) => return Ok(locked),
                Err((_, e)) if e != Errno::EWOULDBLOCK => {
                    return Err(format!("Could not lock {}: {e}", path.display()));
                }
                Err(_) if started.elapsed() >= TAKE_WAIT => {
                    return Err("A Nexus host is already running for this user.".to_string());
                }
                Err((unlocked, _)) => file = unlocked,
            }
            std::thread::sleep(TAKE_RETRY);
        }
    }

    /// The process id of the host that holds the lock now, if one does.
    pub fn holder(path: &Path) -> Option<i32> {
        if !Lock::held(path) {
            return None;
        }
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    /// Whether a host holds the lock now.
    pub fn held(path: &Path) -> bool {
        let Ok(file) = File::open(path) else {
            return false;
        };
        Flock::lock(file, FlockArg::LockSharedNonblock).is_err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("nexus-host-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Paths::at(dir)
    }

    #[test]
    fn makes_a_private_token_once() {
        use std::os::unix::fs::PermissionsExt as _;
        let paths = temp("token");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let first = token(&paths.token()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(first.len(), 64);
        assert_eq!(token(&paths.token()).ok(), Some(first.clone()));
        let mode = |p: &Path| {
            std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0)
        };
        assert_eq!(mode(&paths.token()), 0o600);
        assert_eq!(mode(&paths.dir), 0o700);
        assert_eq!(mode(&paths.run()), 0o700);
        assert!(same_token(&first, &first));
        assert!(!same_token("short", &first));
        assert!(!same_token(&new_token(), &first));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn a_socket_folder_that_was_there_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut paths = temp("socket-dir");
        let shared = std::env::temp_dir().join(format!("nexus-host-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&shared);
        std::fs::create_dir_all(&shared).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
        paths.socket_at = Some(shared.join("x.sock"));
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let mode = |p: &Path| {
            std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0)
        };
        assert_eq!(mode(&shared), 0o755);
        assert_eq!(open_socket_folder(&shared), None);
        // A folder others can write to keeps its mode too, with a warning.
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap_or_else(|e| panic!("{e}"));
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mode(&shared), 0o777);
        assert!(open_socket_folder(&shared).is_some_and(|warning| warning.contains("mode 1777")));
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
        // A folder the host makes for its socket is private.
        let own = shared.join("made-here");
        paths.socket_at = Some(own.join("x.sock"));
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mode(&own), 0o700);
        // A file where the folder should be is an error, not a folder that was there.
        let file = shared.join("a-file");
        std::fs::write(&file, "").unwrap_or_else(|e| panic!("{e}"));
        assert!(make_dir(&file).is_err());
        assert_eq!(make_dir(&own).ok(), Some(false));
        let _ = std::fs::remove_dir_all(&shared);
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn one_host_at_a_time() {
        let paths = temp("lock");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        assert!(!Lock::held(&paths.lock()));
        let lock = Lock::take(&paths.lock()).unwrap_or_else(|e| panic!("{e}"));
        assert!(Lock::held(&paths.lock()));
        assert_eq!(Lock::holder(&paths.lock()), Some(std::process::id() as i32));
        assert!(Lock::take(&paths.lock()).is_err());
        drop(lock);
        assert!(!Lock::held(&paths.lock()));
        assert_eq!(Lock::holder(&paths.lock()), None);
        assert!(Lock::take(&paths.lock()).is_ok());
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    /// "Update now" starts `serve` and runs `status` at once: `status`'s shared lock for an instant does not stop it.
    #[test]
    fn a_status_probe_does_not_stop_a_host_from_starting() {
        let paths = temp("probe");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let lock = paths.lock();
        let (held, probing) = std::sync::mpsc::channel();
        let probe = std::thread::spawn(move || {
            let file = File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock)
                .unwrap_or_else(|e| panic!("{e}"));
            let shared = Flock::lock(file, FlockArg::LockSharedNonblock).unwrap_or_else(|(_, e)| panic!("{e}"));
            let _ = held.send(());
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(shared);
        });
        probing.recv().unwrap_or_else(|e| panic!("{e}"));
        let taken = Lock::take(&paths.lock());
        let _ = probe.join();
        assert!(taken.is_ok(), "{:?}", taken.err());
        drop(taken);
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    /// A running host still stops a second one, after the wait for a probe.
    #[test]
    fn a_running_host_stops_a_second_one_after_the_wait() {
        let paths = temp("running");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(paths.lock())
            .unwrap_or_else(|e| panic!("{e}"));
        let running = Flock::lock(file, FlockArg::LockExclusiveNonblock).unwrap_or_else(|(_, e)| panic!("{e}"));
        let started = std::time::Instant::now();
        let second = Lock::take(&paths.lock());
        let waited = started.elapsed();
        drop(running);
        assert_eq!(
            second.err(),
            Some("A Nexus host is already running for this user.".to_string())
        );
        assert!(
            waited >= TAKE_WAIT - std::time::Duration::from_millis(100),
            "gave up after {waited:?}"
        );
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn a_sign_in_saved_on_a_retired_gateway_host_moves_to_production() {
        let paths = temp("moved");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        for (saved, now) in [
            ("https://api.nexus.xyz", nexus_client::config::GATEWAY_URL),
            ("https://gw.test", "https://gw.test"),
        ] {
            let credentials = Credentials {
                gateway: saved.into(),
                token: "nxc_1".into(),
                org: None,
                label: "host".into(),
            };
            credentials.save(&paths.credentials()).unwrap_or_else(|e| panic!("{e}"));
            let loaded = Credentials::load(&paths.credentials()).map(|c| c.gateway);
            assert_eq!(loaded.as_deref(), Some(now), "{saved}");
            let text = std::fs::read_to_string(paths.credentials()).unwrap_or_default();
            assert!(text.contains(now), "{saved} saved");
        }
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn keeps_credentials_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let paths = temp("credentials");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let saved = Credentials {
            gateway: "https://gw.test".into(),
            token: "nxc_1".into(),
            org: Some("acme".into()),
            label: "Nexus Code host on box".into(),
        };
        saved.save(&paths.credentials()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(Credentials::load(&paths.credentials()), Some(saved));
        let mode = std::fs::metadata(paths.credentials()).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o600));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn a_credentials_file_written_by_others_is_made_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let paths = temp("credentials-wide");
        paths.create().unwrap_or_else(|e| panic!("{e}"));
        let path = paths.credentials();
        let text = r#"{"gateway":"https://gw.test","token":"nxc_1","org":"acme","label":"cloud"}"#;
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap_or_else(|e| panic!("{e}"));
        assert!(Credentials::load(&path).is_some());
        let mode = std::fs::metadata(&path).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o600));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }
}
