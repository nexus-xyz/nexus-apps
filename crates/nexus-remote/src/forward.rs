//! A dev server's port on a remote machine, reached from this computer: an
//! `ssh -N -L` child ([`Target::forward`]) from a free port on this computer's loopback to the port on the machine.
//! A cloud machine's Preview (nexus-cloud) and a server session's Browser (Nexus GPUI) both keep theirs here.
//!
//! [`Forwards`] holds one machine's forwards by remote port. It reuses a live one, starts a new one when its child
//! died, and closes them all when the machine goes away (dropped or [`Forwards::close`]): each child is killed when
//! dropped.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::Result;
use crate::ssh::{Target, ssh_error};

/// How long a new forward has to accept a connection on this computer.
pub const START_WITHIN: Duration = Duration::from_secs(20);

/// One forward: its port on this computer and the `ssh -N -L` child, killed when dropped.
pub struct Forward {
    local: u16,
    child: tokio::process::Child,
}

impl Forward {
    /// Its port on this computer: `http://127.0.0.1:<local>` reaches the machine's port.
    pub fn local(&self) -> u16 {
        self.local
    }

    /// Whether its child still runs.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Forwards `remote_port` on `target` to a free port here and waits until it accepts. `still` is asked while it
/// waits: an error there (the sign-in or the machine changed) closes the forward at once.
pub async fn open(target: &Target, remote_port: u16, still: impl Fn() -> Result<()>) -> Result<Forward> {
    if remote_port == 0 {
        return Err("Port 0 cannot be previewed.".into());
    }
    start(|local| target.forward(local, remote_port), START_WITHIN, still).await
}

/// A free port on this computer's loopback.
fn free_port() -> Result<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|e| format!("No free port on this computer: {e}"))
}

/// Runs the command `make` builds for a free local port, and waits up to `within` until that port accepts. The
/// command's last error line is the error when it exits first (ssh's "Address already in use", "Connection refused").
async fn start(
    make: impl FnOnce(u16) -> Result<tokio::process::Command>,
    within: Duration,
    still: impl Fn() -> Result<()>,
) -> Result<Forward> {
    let local = free_port()?;
    let mut command = make(local)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))?;
    let started = Instant::now();
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", local)).await.is_ok() {
            return Ok(Forward { local, child });
        }
        if let Ok(Some(_)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = tokio::io::AsyncReadExt::read_to_string(&mut err, &mut stderr).await;
            }
            return Err(ssh_error(&stderr).unwrap_or_else(|| "The port forward did not start.".into()));
        }
        if started.elapsed() > within {
            return Err(format!(
                "The port forward did not start within {} seconds.",
                within.as_secs()
            ));
        }
        still()?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// One machine's forwards, by the port on the machine.
#[derive(Default)]
pub struct Forwards {
    open: Mutex<HashMap<u16, Forward>>,
    /// Held while one starts, so two asks for the same port share one child.
    starting: tokio::sync::Mutex<()>,
}

impl Forwards {
    /// The local port of `remote`'s forward while its child runs. A dead one is dropped.
    pub fn live(&self, remote: u16) -> Option<u16> {
        let mut open = self.open.lock().ok()?;
        let forward = open.get_mut(&remote)?;
        if forward.alive() {
            return Some(forward.local);
        }
        open.remove(&remote);
        None
    }

    /// Keeps `forward` for `remote`, in place of an older one (closed), and returns its local port.
    pub fn keep(&self, remote: u16, forward: Forward) -> u16 {
        let local = forward.local;
        if let Ok(mut open) = self.open.lock() {
            open.insert(remote, forward);
        }
        local
    }

    /// Closes every forward: the machine went away, or the sign-in changed.
    pub fn close(&self) {
        if let Ok(mut open) = self.open.lock() {
            open.clear();
        }
    }

    /// The forwards that run: (port on the machine, port here).
    pub fn ports(&self) -> Vec<(u16, u16)> {
        let Ok(mut open) = self.open.lock() else {
            return Vec::new();
        };
        open.retain(|_, forward| forward.alive());
        let mut ports: Vec<(u16, u16)> = open.iter().map(|(remote, forward)| (*remote, forward.local)).collect();
        ports.sort_unstable();
        ports
    }

    /// `remote`'s local port: the live forward's, else a new one on `target`.
    pub async fn get(&self, target: &Target, remote: u16) -> Result<u16> {
        self.get_with(remote, |still| open(target, remote, still)).await
    }

    async fn get_with<F, Fut>(&self, remote: u16, start: F) -> Result<u16>
    where
        F: FnOnce(fn() -> Result<()>) -> Fut,
        Fut: Future<Output = Result<Forward>>,
    {
        let _one = self.starting.lock().await;
        if let Some(local) = self.live(remote) {
            return Ok(local);
        }
        let forward = start(|| Ok(())).await?;
        Ok(self.keep(remote, forward))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nexus_workbench::process::async_command;

    /// A stand-in for `ssh -N -L`: the test listens on the port itself, and `sleep` is the child.
    fn fake(listeners: &Mutex<Vec<std::net::TcpListener>>) -> impl FnOnce(u16) -> Result<tokio::process::Command> {
        move |local| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", local)).map_err(|e| e.to_string())?;
            listeners.lock().map_err(|e| e.to_string())?.push(listener);
            let mut command = async_command("sleep");
            command.arg("30");
            Ok(command)
        }
    }

    fn failing(line: &'static str) -> impl FnOnce(u16) -> Result<tokio::process::Command> {
        move |_| {
            let mut command = async_command("sh");
            command.args(["-c", &format!("echo 'debug1: noise' >&2; echo '{line}' >&2; exit 255")]);
            Ok(command)
        }
    }

    fn never(_: u16) -> Result<tokio::process::Command> {
        let mut command = async_command("sleep");
        command.arg("30");
        Ok(command)
    }

    #[tokio::test]
    async fn a_live_forward_is_reused_and_a_dead_one_starts_again() {
        let listeners = Mutex::new(Vec::new());
        let forwards = Forwards::default();
        let first = forwards
            .get_with(3000, |still| start(fake(&listeners), START_WITHIN, still))
            .await
            .expect("started");
        let again = forwards
            .get_with(3000, |_| async { Err::<Forward, _>("not started again".to_string()) })
            .await;
        assert_eq!(again, Ok(first), "the live forward is reused");
        assert_eq!(forwards.ports(), [(3000, first)]);

        // Its child dies (ssh lost the network): the next ask starts a new one.
        if let Ok(mut open) = forwards.open.lock()
            && let Some(forward) = open.get_mut(&3000)
        {
            forward.child.start_kill().expect("killed");
        }
        for _ in 0..100 {
            if forwards.live(3000).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(forwards.live(3000), None);
        let second = forwards
            .get_with(3000, |still| start(fake(&listeners), START_WITHIN, still))
            .await
            .expect("started again");
        assert_ne!(second, first);
        assert_eq!(forwards.live(3000), Some(second));

        forwards.close();
        assert_eq!(forwards.live(3000), None);
        assert!(forwards.ports().is_empty());
    }

    #[tokio::test]
    async fn a_forward_that_exits_says_ssh_s_last_line() {
        let error = start(
            failing("bind [127.0.0.1]:3000: Address already in use"),
            START_WITHIN,
            || Ok(()),
        )
        .await
        .err();
        assert_eq!(error.as_deref(), Some("bind [127.0.0.1]:3000: Address already in use"));
        let forwards = Forwards::default();
        let error = forwards
            .get_with(3000, |still| {
                start(
                    failing("channel_setup_fwd_listener_tcpip: cannot listen"),
                    START_WITHIN,
                    still,
                )
            })
            .await;
        assert_eq!(error, Err("channel_setup_fwd_listener_tcpip: cannot listen".into()));
        assert!(forwards.ports().is_empty(), "nothing kept");
    }

    #[tokio::test]
    async fn a_forward_that_never_accepts_times_out_or_stops_when_asked() {
        let error = start(never, Duration::from_millis(300), || Ok(())).await.err();
        assert_eq!(
            error.as_deref(),
            Some("The port forward did not start within 0 seconds.")
        );
        let error = start(never, START_WITHIN, || Err("The sign-in changed.".into()))
            .await
            .err();
        assert_eq!(error.as_deref(), Some("The sign-in changed."));
    }

    #[tokio::test]
    async fn port_zero_and_this_computer_are_refused() {
        let local = Target::Local { env: Vec::new() };
        assert_eq!(
            open(&local, 0, || Ok(())).await.err().as_deref(),
            Some("Port 0 cannot be previewed.")
        );
        assert_eq!(
            open(&local, 3000, || Ok(())).await.err().as_deref(),
            Some("This computer is not reached over ssh.")
        );
    }
}
