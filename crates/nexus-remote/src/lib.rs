//! `nexus-remote`: the desktop app's side of a remote machine. A remote machine runs
//! `nexus-host` (the same workbench as the app, with no window); this crate gets it there and talks to it.
//!
//! - [`ssh`]: the system `ssh` (the person's own config, keys, agent, jump hosts and Tailscale names), never `-A`.
//! - [`install`]: the bootstrap over SSH: find the system, install the app's own version of the host (downloaded on
//!   the server and checked against the channel's manifest, else downloaded here and copied over, or a local build in
//!   development), start it detached, read its token; and "Update now".
//! - [`client`]: one WebSocket to a host: the hello, calls answered by id, pushed events.
//! - [`probe`]: what a server has (the system, the host's Nexus sign-in, Claude Code and Codex and their sign-ins), and
//!   `nexus-host login` there, for the app's server screens.
//! - [`login`]: a server's Nexus sign-in on its way, one per machine, cancelled by a new one or "Cancel".
//! - [`ssh_config`]: the hosts `~/.ssh/config` names, for the "Add a server" list.
//! - [`machine`]: a [`machine::Remote`] keeps one connection up: reconnects with backoff, catches every open session
//!   up from its event log (each event exactly once), checks the host's version and updates an idle older host.
//! - [`relay`]: a cloud machine's host through Gateway: one WebSocket, no SSH.
//! - [`cleanup`]: the worktree sweep on a server, and the report of a sweep everywhere it ran.
//! - [`forward`]: a dev server's port on a machine, forwarded to this computer (`ssh -N -L`), for a cloud machine's
//!   Preview and a server session's Browser.
//! - [`activity`]: which calls wake a sleeping cloud machine, and which count as the person using one.
//! - [`local`]: the same host as the app's own background service on this computer: started detached,
//!   reached over its private socket, replaced on an update, stopped with its agents. [`engine`]: an app's Code
//!   commands, on its own workbench or on that service.

pub mod activity;
pub mod cleanup;
pub mod client;
pub mod engine;
pub mod forward;
pub mod install;
#[cfg(unix)]
pub mod local;
pub mod login;
pub mod machine;
pub mod probe;
pub mod relay;
pub mod ssh;
pub mod ssh_config;

use tokio::io::{AsyncRead, AsyncWrite};

/// A byte stream to a host: the `ssh … proxy` child's standard output and input, or a socket in tests.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Duplex for T {}

pub type Result<T> = std::result::Result<T, String>;

/// A development setting: read in debug builds only, so nothing outside a release build can move which `ssh` runs or
/// where its host comes from.
pub fn development_setting(name: &str) -> Option<std::ffi::OsString> {
    setting_when(cfg!(debug_assertions), name)
}

fn setting_when(enabled: bool, name: &str) -> Option<std::ffi::OsString> {
    if !enabled {
        return None;
    }
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod setting_tests {
    use super::*;

    #[test]
    fn a_development_setting_reads_nothing_in_a_release_build() {
        // PATH is set wherever the tests run, and this test has no reason to change the environment.
        assert!(setting_when(true, "PATH").is_some());
        assert_eq!(setting_when(false, "PATH"), None);
        assert_eq!(setting_when(true, "NEXUS_NOT_A_SETTING"), None);
        // The build this test runs in: `cargo test` and `cargo test --release` give each answer.
        assert_eq!(development_setting("PATH").is_some(), cfg!(debug_assertions));
    }
}
