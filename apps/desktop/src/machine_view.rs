//! How a server's connection reads in the sidebar and in Settings › Servers: the Nexus app's words, from its
//! `workbench/machine-state.ts` (`statusView`, `plainError`, `suggestedName`, `serverCommands`).

use nexus_remote::machine::Status;

/// For the dot: connected, working, needs you, or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Warn,
    Off,
}

/// The button a status offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Update,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusView {
    /// "Connected", "Connecting…", "Reconnecting…", "Needs update", "Updating…", "Offline".
    pub label: &'static str,
    pub tone: Tone,
    /// A sentence under it, when there is more to say.
    pub detail: Option<String>,
    pub action: Option<Action>,
}

pub fn status_view(status: &Status) -> StatusView {
    let view = |label, tone, detail: Option<String>, action| StatusView {
        label,
        tone,
        detail,
        action,
    };
    match status {
        Status::Connecting { attempt } if *attempt > 1 => view("Reconnecting…", Tone::Busy, None, None),
        Status::Connecting { .. } => view(
            "Connecting…",
            Tone::Busy,
            Some("Setting up the Nexus host on it the first time takes a minute.".into()),
            None,
        ),
        Status::Connected {
            update: Some(version), ..
        } => view(
            "Connected",
            Tone::Ok,
            Some(format!(
                "Nexus {version} is ready for it. Update when its sessions are done; running ones stop, as when you quit."
            )),
            Some(Action::Update),
        ),
        Status::Connected { .. } => view("Connected", Tone::Ok, None, None),
        Status::NeedsUpdate { .. } => view(
            "Needs update",
            Tone::Warn,
            Some(
                "It runs an older Nexus host that this app can't talk to. Update it to use it; sessions running there \
                 stop, as when you quit."
                    .into(),
            ),
            Some(Action::Update),
        ),
        Status::Updating => view("Updating…", Tone::Busy, None, None),
        Status::Unreachable { error, .. } => view("Offline", Tone::Off, Some(plain_error(error)), Some(Action::Retry)),
        Status::Closed => view("Offline", Tone::Off, None, Some(Action::Retry)),
    }
}

/// ssh's and the installer's reasons in plain words, where we know what they mean; as they are otherwise.
pub fn plain_error(error: &str) -> String {
    const REASONS: [(&[&str], &str); 7] = [
        (
            &["permission denied"],
            "The server turned down this computer's SSH key. Check that “ssh <server>” works in a terminal without a \
             password.",
        ),
        (
            &["could not resolve hostname"],
            "No server by that name. Check the name, or the Host entry in ~/.ssh/config.",
        ),
        (
            &["connection refused"],
            "The server is up but not taking SSH connections (is sshd running, on that port?).",
        ),
        (
            &["timed out", "did not answer within"],
            "The server didn't answer. It may be off, asleep, or behind a firewall.",
        ),
        (
            &["host key verification failed"],
            "SSH doesn't know this server's key yet. Connect once with “ssh <server>” in a terminal to accept it, then \
             try again.",
        ),
        (
            &["no route to host", "network is unreachable"],
            "This computer can't reach the server's network right now.",
        ),
        (
            &["is openssh installed"],
            "This computer has no ssh command. Install OpenSSH, then try again.",
        ),
    ];
    let lower = error.to_lowercase();
    let Some((_, reason)) = REASONS
        .iter()
        .find(|(patterns, _)| patterns.iter().any(|pattern| lower.contains(pattern)))
    else {
        return error.to_string();
    };
    // `Could not connect to <name>: ` adds nothing to the reason.
    let raw = match error.strip_prefix("Could not connect to") {
        Some(rest) => rest.split_once(": ").map_or(error, |(_, reason)| reason),
        None => error,
    };
    format!("{reason} ({raw})")
}

/// A server's name from what was typed: the host part of `user@host`, without a port or domain.
pub fn suggested_name(ssh: &str) -> String {
    let host = ssh.trim().rsplit('@').next().unwrap_or_default();
    let ipv4 = host.split('.').count() == 4
        && host
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if ipv4 {
        return host.to_string();
    }
    match host.split('.').next() {
        Some(first) if !first.is_empty() => first.to_string(),
        _ => host.to_string(),
    }
}

/// Whether a session failed because its server's host is not signed in to Nexus (`nexus-host`'s `gateway.rs`): the
/// chat then offers to sign that server in from here.
pub fn needs_nexus_sign_in(error: &str) -> bool {
    error.contains("This machine is not signed in to Nexus")
}

/// The commands a person runs on their server themselves: Nexus never sees these sign-ins.
pub struct ServerCommands {
    pub claude_install: String,
    /// Then `/login` inside it; on a server with no browser Claude Code shows a link and asks for the code back.
    pub claude_login: String,
    pub codex_install: String,
    pub codex_login: String,
}

pub fn server_commands(ssh: &str) -> ServerCommands {
    ServerCommands {
        claude_install: format!("ssh -t {ssh} 'curl -fsSL https://claude.ai/install.sh | bash'"),
        claude_login: format!("ssh -t {ssh} claude"),
        codex_install: format!("ssh -t {ssh} 'npm install -g @openai/codex'"),
        codex_login: format!("ssh -t {ssh} codex login --device-auth"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::api::HostInfo;

    fn host() -> HostInfo {
        HostInfo {
            version: "0.10.0".into(),
            protocol: 1,
            machine: "box".into(),
            org: None,
            features: vec![],
        }
    }

    #[test]
    fn reads_each_status_as_the_nexus_app_does() {
        let first = status_view(&Status::Connecting { attempt: 1 });
        assert_eq!((first.label, first.action), ("Connecting…", None));
        let again = status_view(&Status::Connecting { attempt: 3 });
        assert_eq!((again.label, again.action), ("Reconnecting…", None));
        let busy = status_view(&Status::Connected {
            host: host(),
            update: Some("0.11.0".into()),
        });
        assert_eq!((busy.tone, busy.action), (Tone::Ok, Some(Action::Update)));
        assert!(
            busy.detail
                .is_some_and(|detail| detail.starts_with("Nexus 0.11.0 is ready"))
        );
        let old = status_view(&Status::NeedsUpdate {
            host: host(),
            error: "protocol".into(),
        });
        assert_eq!((old.label, old.tone), ("Needs update", Tone::Warn));
        let off = status_view(&Status::Unreachable {
            error: "Could not connect to box: Permission denied (publickey).".into(),
            retry_in_secs: 4,
        });
        assert_eq!(off.label, "Offline");
        assert_eq!(
            off.detail.as_deref(),
            Some(
                "The server turned down this computer's SSH key. Check that “ssh <server>” works in a terminal \
                 without a password. (Permission denied (publickey).)"
            )
        );
        assert_eq!(plain_error("something else"), "something else");
    }

    #[test]
    fn knows_a_host_that_is_not_signed_in() {
        assert!(needs_nexus_sign_in(
            "This machine is not signed in to Nexus: run `nexus-host login` on it to start Claude Code."
        ));
        assert!(needs_nexus_sign_in(
            "This machine is not signed in to Nexus, so Codex can't start here. Sign it in from Remote servers in \
             the Nexus app's settings, or run `~/.nexus/host/0.10.0/nexus-host login` on it."
        ));
        assert!(!needs_nexus_sign_in("Claude Code is not installed on this machine."));
        // The CLI's own sign-in, which the person fixes with `nexus auth login`, not the server's.
        assert!(!needs_nexus_sign_in("You are not signed in to Nexus."));
    }

    #[test]
    fn names_a_server_from_its_destination() {
        assert_eq!(suggested_name("me@build.example.com"), "build");
        assert_eq!(suggested_name("10.0.0.7"), "10.0.0.7");
        assert_eq!(suggested_name("vps"), "vps");
        assert_eq!(
            server_commands("vps").codex_login,
            "ssh -t vps codex login --device-auth"
        );
    }
}
