//! One running copy of this build per person. A second launch hands its
//! arguments to the running one, which brings its window forward, and exits.
//!
//! A lock file decides who runs (`instance.lock` in this build's own folder): the operating system releases it when the
//! process ends, a crash included, so a stale lock never blocks a start. The process that holds it listens on a local
//! socket (a Unix socket beside the lock, a named pipe on Windows). A launch that finds the lock taken connects to it,
//! sends one line of JSON ([`Handoff`]) and exits.
//!
//! This is this build's own lock only. Who runs the Code sessions is another lock, shared with an older Nexus
//! install (`code_run.rs`).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name};
use serde::{Deserialize, Serialize};

/// What a second launch hands over: its arguments (without the program) and its working folder. A `nexus://` prompt
/// link arrives among `args`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handoff {
    pub args: Vec<String>,
    pub cwd: String,
}

impl Handoff {
    fn of_this_process() -> Handoff {
        Handoff {
            args: std::env::args().skip(1).collect(),
            cwd: std::env::current_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// How a launch went.
pub enum Launch {
    /// This process runs the app. Keep the value alive as long as the app runs: it holds the lock.
    First(Instance),
    /// Another copy runs and has this launch's arguments. Exit.
    HandedOff,
    /// The check itself failed (no folder, no socket). Run anyway, and say why.
    Unchecked(String),
}

/// The running copy's hold: the lock and the listener.
pub struct Instance {
    lock: File,
    listener: LocalSocketListener,
}

/// How long a second launch keeps trying to reach the running copy: the lock is taken a moment before the socket
/// listens, so a launch right after the first may find the lock and no listener yet.
const CONNECT_FOR: Duration = Duration::from_secs(3);

/// Take the lock in `dir`, or hand this process's arguments to the copy that holds it.
pub fn claim(dir: &Path) -> Launch {
    claim_with(dir, &socket_name(dir), &Handoff::of_this_process())
}

/// How long a copy that the running one restarted (`workspace/sign_in.rs`) waits for it to quit and let go.
const RESTART_WAIT: Duration = Duration::from_secs(10);

/// For a copy the running one started in its place: wait for that one to let go of the lock, then hold it. Handing
/// off instead would give the launch to a copy that is quitting.
pub fn claim_after_restart(dir: &Path) -> Launch {
    let socket = socket_name(dir);
    let deadline = Instant::now() + RESTART_WAIT;
    loop {
        match take_lock(dir) {
            Ok(Some(lock)) => return first(lock, &socket),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => return claim(dir),
            Err(error) => return Launch::Unchecked(format!("could not take the instance lock: {error}")),
        }
    }
}

fn first(lock: File, socket: &str) -> Launch {
    match listen(socket) {
        Ok(listener) => Launch::First(Instance { lock, listener }),
        Err(error) => Launch::Unchecked(format!("could not listen for a second launch: {error}")),
    }
}

fn claim_with(dir: &Path, socket: &str, handoff: &Handoff) -> Launch {
    match take_lock(dir) {
        Ok(Some(lock)) => first(lock, socket),
        Ok(None) => match hand_off(socket, handoff) {
            Ok(()) => Launch::HandedOff,
            Err(error) => Launch::Unchecked(format!("another copy runs, but did not answer: {error}")),
        },
        Err(error) => Launch::Unchecked(format!("could not take the instance lock: {error}")),
    }
}

/// The lock, or `None` when another process holds it.
fn take_lock(dir: &Path) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(dir)?;
    // Only this person may reach the socket: it takes arguments, and later prompt links.
    #[cfg(unix)]
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("instance.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// The socket's name. Unix: a file beside the lock, in a folder only this person can open. Windows: a named pipe, which
/// has no folder. It is named for the person, so two people on one computer each get their own copy, and only its
/// creator may write to it (the default pipe security).
fn socket_name(dir: &Path) -> String {
    if cfg!(windows) {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let user: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
        format!(r"\\.\pipe\nexus-desktop-gpui-{user}")
    } else {
        dir.join("instance.sock").to_string_lossy().into_owned()
    }
}

fn name(socket: &str) -> std::io::Result<Name<'_>> {
    socket.to_fs_name::<GenericFilePath>()
}

fn listen(socket: &str) -> std::io::Result<LocalSocketListener> {
    // The lock is ours, so a socket file still there is from a copy that crashed: replace it.
    ListenerOptions::new()
        .name(name(socket)?)
        .try_overwrite(true)
        .create_sync()
}

fn hand_off(socket: &str, handoff: &Handoff) -> std::io::Result<()> {
    let started = Instant::now();
    let mut stream = loop {
        match LocalSocketStream::connect(name(socket)?) {
            Ok(stream) => break stream,
            Err(error) if started.elapsed() >= CONNECT_FOR => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let mut line = serde_json::to_vec(handoff).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()
}

impl Instance {
    /// Read second launches on a thread of their own and pass each to `on_handoff`, until the process ends.
    pub fn serve(self, on_handoff: impl Fn(Handoff) + Send + 'static) {
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                let Instance { lock, listener } = self;
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    // A launch that connects and sends nothing does not hold up the next one.
                    let _ = stream.set_recv_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new(stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    match serde_json::from_str::<Handoff>(&line) {
                        Ok(handoff) => on_handoff(handoff),
                        Err(error) => eprintln!("nexus-desktop-gpui: a second launch sent {error}"),
                    }
                }
                drop(lock);
            })
            .expect("the single-instance thread starts");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn handoff(args: &[&str]) -> Handoff {
        Handoff {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: "/tmp".into(),
        }
    }

    /// The first launch runs; a second hands its arguments over and exits; the first gets them.
    #[test]
    fn a_second_launch_hands_its_arguments_to_the_first() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-instance-{}", std::process::id()));
        let socket = if cfg!(windows) {
            format!(r"\\.\pipe\nexus-desktop-gpui-test-{}", std::process::id())
        } else {
            socket_name(&dir)
        };
        let Launch::First(instance) = claim_with(&dir, &socket, &handoff(&[])) else {
            panic!("the first launch does not run");
        };
        let (sent, received) = mpsc::channel();
        instance.serve(move |handoff| {
            let _ = sent.send(handoff);
        });

        let link = handoff(&["nexus://prompt?text=hi"]);
        assert!(matches!(claim_with(&dir, &socket, &link), Launch::HandedOff));
        assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), link);
        let plain = handoff(&[]);
        assert!(matches!(claim_with(&dir, &socket, &plain), Launch::HandedOff));
        assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), plain);
    }

    /// Two windows on one workspace cannot corrupt its state.
    ///
    /// Two mechanisms hold together, so the test covers both. The lock means one copy writes: a second window never
    /// becomes the writer while the first lives, and starts once it goes. Atomic writes mean that a writer which did
    /// slip past the lock still cannot leave half a file, because it renames a whole one over the target.
    #[cfg(unix)]
    #[test]
    fn two_windows_on_one_workspace_cannot_corrupt_its_state() {
        use nexus_desktop_core::atomic_file::write_atomic;

        let dir = std::env::temp_dir().join(format!("nexus-gpui-two-windows-{}", std::process::id()));
        let socket = socket_name(&dir);
        let Launch::First(first) = claim_with(&dir, &socket, &handoff(&[])) else {
            panic!("the first window does not run");
        };

        // A second window finds the lock held, so it never writes.
        assert!(take_lock(&dir).expect("the lock is readable").is_none());

        // Values of different lengths: a torn or truncated write matches none of them.
        let values: Vec<Vec<u8>> = (0..8u8).map(|n| vec![b'a' + n; 1 + usize::from(n) * 4096]).collect();
        let state = dir.join("state.json");
        write_atomic(&state, &values[0]).expect("the first write");

        // Writers racing on one file each rename a whole file over it, so every read sees one whole value.
        std::thread::scope(|scope| {
            for value in &values {
                scope.spawn(|| write_atomic(&state, value).expect("a concurrent write"));
            }
            scope.spawn(|| {
                for _ in 0..200 {
                    let seen = std::fs::read(&state).expect("the state is always readable");
                    assert!(
                        values.contains(&seen),
                        "a reader saw {} bytes, which is no whole value",
                        seen.len()
                    );
                }
            });
        });
        assert!(values.contains(&std::fs::read(&state).expect("the final state")));

        // The lock frees when the window goes, so the next one starts.
        drop(first);
        assert!(take_lock(&dir).expect("the lock is readable").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A copy that crashed leaves its socket file behind; the next launch takes the lock and replaces it.
    #[cfg(unix)]
    #[test]
    fn a_stale_socket_does_not_block_a_start() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = socket_name(&dir);
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        assert!(Path::new(&socket).exists());
        assert!(matches!(claim_with(&dir, &socket, &handoff(&[])), Launch::First(_)));
    }
}
