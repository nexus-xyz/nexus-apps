//! Running things on a remote machine with the system `ssh`, so a person's
//! `~/.ssh/config`, keys, ssh-agent, 1Password, ProxyJump hosts and Tailscale names all work as in their terminal (as
//! VS Code and Zed do it). Every run is non-interactive (`BatchMode`: a key or an agent, never a password prompt the
//! app could not show), forwards nothing (no agent, no X11, none of the config's port forwards) and notices a dead
//! network within about 45 seconds (`ServerAliveInterval`).
//!
//! What runs there is always `sh -c '<script>' nexus <args…>`, quoted here, whatever the person's login shell is.
//!
//! A machine the app made itself (a cloud machine) is the exception to "the person's own config": it is
//! [`Target::Direct`], an explicit address, user and key, and the host key the provider's API handed over, with the
//! person's `~/.ssh/config` and known hosts left out entirely. Such a machine gets a new address and a new host key
//! every time it wakes, so the host key is pinned under a stable name ([`Target::Direct`]'s `alias`, ssh's
//! `HostKeyAlias`) and its line in the app's own known_hosts file is rewritten before every connect
//! ([`write_known_hosts`]): never trusted on first use, never a prompt.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use nexus_workbench::process::async_command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::Result;

/// Where scripts run.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// `ssh <destination>`: a host from `~/.ssh/config`, `user@server`, or a Tailscale name (a port goes in the
    /// config's `Port`).
    Ssh { destination: String },
    /// An explicit address the app manages itself (a cloud machine): `ssh -F none -p <port> -i <identity> -l <user>
    /// <host>`, the host key checked against `known_hosts` under `alias`, nothing from the person's ssh config.
    Direct {
        host: String,
        port: u16,
        user: String,
        /// The private key the app generated for this machine.
        identity: PathBuf,
        /// The app's own known_hosts file for this machine ([`write_known_hosts`]).
        known_hosts: PathBuf,
        /// The stable name the host key is pinned under (`nexus-cloud-bx_23456789`): the address changes on every
        /// wake, the name does not.
        alias: String,
    },
    /// This computer, with these variables set (`HOME` for a throwaway host folder): the tests' stand-in for a server,
    /// running the same scripts through the same code.
    Local { env: Vec<(String, String)> },
}

/// What a script printed and how it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// `value` in single quotes, for `sh` and for the login shells ssh hands the command to (sh, bash, zsh, fish).
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Whether `destination` can be handed to `ssh` as a destination: not empty, not an option, no spaces or controls.
pub fn valid_destination(destination: &str) -> bool {
    !destination.is_empty()
        && !destination.starts_with('-')
        && !destination
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\'' || c == '"')
}

/// Whether `value` can go in an option without changing its meaning: a user name or a host-key alias. Letters, digits,
/// `.`, `_` and `-`, and not starting with `-`.
pub fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A path as the value of an `-o` option or `-i`: in double quotes (the macOS data folder is "Application Support"),
/// backslashes doubled (Windows paths; ssh reads `\\` as one). Refused: double quotes and controls, which would end
/// or break the value, and `%`, which ssh expands as a token (`%d`, `%u`) in these options.
fn quoted_path(path: &Path) -> Result<String> {
    let text = path.to_string_lossy();
    if text.is_empty() || text.chars().any(|c| c == '"' || c == '%' || c.is_control()) {
        return Err(format!("{} cannot be handed to ssh as a file.", path.display()));
    }
    Ok(format!("\"{}\"", text.replace('\\', "\\\\")))
}

/// The options every run gets.
pub const SSH_OPTIONS: &[&str] = &[
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=15",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ForwardX11=no",
    "-o",
    "ClearAllForwardings=yes",
];

impl Target {
    /// What the person calls it.
    pub fn name(&self) -> &str {
        match self {
            Target::Ssh { destination } => destination,
            Target::Direct { alias, .. } => alias,
            Target::Local { .. } => "this computer",
        }
    }

    /// `ssh` with this target's options, and the destination that goes after `--`. `forwarding`: for
    /// [`Target::forward`], which needs the forwards that [`SSH_OPTIONS`] otherwise clears.
    fn ssh(&self, forwarding: bool) -> Result<(tokio::process::Command, String)> {
        let mut command = async_command(ssh_program());
        let destination = match self {
            Target::Ssh { destination } => {
                if !valid_destination(destination) {
                    return Err(format!("{destination:?} is not an SSH host."));
                }
                destination.clone()
            }
            Target::Direct {
                host,
                port,
                user,
                identity,
                known_hosts,
                alias,
            } => {
                // The user goes in `-l`, so a host with an `@` would be a second, conflicting user.
                if !valid_destination(host) || host.contains('@') {
                    return Err(format!("{host:?} is not an SSH host."));
                }
                if *port == 0 {
                    return Err("Port 0 is not an SSH port.".into());
                }
                if !valid_name(user) {
                    return Err(format!("{user:?} is not a user name."));
                }
                if !valid_name(alias) {
                    return Err(format!("{alias:?} is not a host-key name."));
                }
                // `-F none`: nothing from the person's ~/.ssh/config (a `Host *` with another key, a ProxyCommand,
                // a ControlMaster) applies to a machine the app manages. The host key is checked only against the
                // app's own file, under the alias, and never learned or updated from the connection.
                command
                    .args(["-F", "none", "-p", &port.to_string(), "-l", user, "-o"])
                    .arg(format!("IdentityFile={}", quoted_path(identity)?))
                    .args(["-o", "IdentitiesOnly=yes", "-o"])
                    .arg(format!("UserKnownHostsFile={}", quoted_path(known_hosts)?))
                    .args([
                        "-o",
                        "GlobalKnownHostsFile=/dev/null",
                        "-o",
                        "StrictHostKeyChecking=yes",
                        "-o",
                        "UpdateHostKeys=no",
                        "-o",
                    ])
                    .arg(format!("HostKeyAlias={alias}"));
                host.clone()
            }
            Target::Local { .. } => return Err("This computer is not reached over ssh.".into()),
        };
        if forwarding {
            let mut options: Vec<&str> = SSH_OPTIONS.to_vec();
            if let Some(at) = options.iter().position(|o| *o == "ClearAllForwardings=yes") {
                options.drain(at.saturating_sub(1)..=at);
            }
            command.args(options);
        } else {
            command.args(SSH_OPTIONS);
        }
        Ok((command, destination))
    }

    /// The command that runs `script` with `args` there.
    pub fn command(&self, script: &str, args: &[&str]) -> Result<tokio::process::Command> {
        let mut command = match self {
            Target::Ssh { .. } | Target::Direct { .. } => {
                let (mut command, destination) = self.ssh(false)?;
                let mut remote = format!("sh -c {} nexus", quote(script));
                for arg in args {
                    remote.push(' ');
                    remote.push_str(&quote(arg));
                }
                command.arg("--").arg(destination).arg(remote);
                command
            }
            Target::Local { env } => {
                let mut command = async_command("sh");
                command.arg("-c").arg(script).arg("nexus").args(args);
                command.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                command
            }
        };
        command.kill_on_drop(true);
        Ok(command)
    }

    /// `ssh -N -L 127.0.0.1:<local_port>:127.0.0.1:<remote_port>`: a port on this computer that reaches a port on the
    /// machine (a dev server's preview), for as long as the child runs. Only this computer's loopback listens. It
    /// exits at once when the forward cannot be set up (`ExitOnForwardFailure`), say the local port is taken. On a
    /// [`Target::Ssh`] the person's config's own forwards come along too (only [`Target::Direct`] ignores the config).
    pub fn forward(&self, local_port: u16, remote_port: u16) -> Result<tokio::process::Command> {
        if local_port == 0 || remote_port == 0 {
            return Err("Port 0 cannot be forwarded.".into());
        }
        let (mut command, destination) = self.ssh(true)?;
        command
            .args(["-N", "-o", "ExitOnForwardFailure=yes", "-L"])
            .arg(format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"))
            .arg("--")
            .arg(destination);
        command.kill_on_drop(true);
        Ok(command)
    }

    /// Runs `script` there, `input` on its standard input, for at most `limit`.
    pub async fn run(&self, script: &str, args: &[&str], input: Option<Vec<u8>>, limit: Duration) -> Result<Output> {
        let mut command = self.command(script, args)?;
        command
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let work = async move {
            let feed = async {
                if let (Some(stdin), Some(input)) = (stdin.as_mut(), input) {
                    let _ = stdin.write_all(&input).await;
                    let _ = stdin.shutdown().await;
                }
                drop(stdin);
            };
            let read_out = async {
                let mut text = Vec::new();
                if let Some(out) = stdout.as_mut() {
                    let _ = out.read_to_end(&mut text).await;
                }
                String::from_utf8_lossy(&text).into_owned()
            };
            let read_err = async {
                let mut text = Vec::new();
                if let Some(err) = stderr.as_mut() {
                    let _ = err.read_to_end(&mut text).await;
                }
                String::from_utf8_lossy(&text).into_owned()
            };
            let ((), stdout, stderr) = tokio::join!(feed, read_out, read_err);
            let status = child.wait().await.map_err(|e| e.to_string())?;
            Ok(Output {
                ok: status.success(),
                stdout,
                stderr,
            })
        };
        tokio::time::timeout(limit, work)
            .await
            .map_err(|_| format!("{} did not answer within {} seconds.", self.name(), limit.as_secs()))?
    }
}

/// The known_hosts line that pins `host_key` (`ssh-ed25519 AAAA… comment`, as a provider's API or `ssh-keyscan` gives
/// it) for `name` at `port`: `name keytype base64`, or `[name]:port keytype base64` when the port is not 22, which is
/// how ssh looks a host up. With a [`Target::Direct`] the name is its alias and the port 22: ssh looks an alias up
/// without a port.
pub fn known_hosts_line(name: &str, port: u16, host_key: &str) -> Result<String> {
    let pattern_free = !name
        .chars()
        .any(|c| matches!(c, ',' | '*' | '?' | '!' | '[' | ']' | '|' | '#'));
    if !valid_destination(name) || !pattern_free {
        return Err(format!("{name:?} cannot be a known host."));
    }
    let mut parts = host_key.split_whitespace();
    let (Some(kind), Some(key)) = (parts.next(), parts.next()) else {
        return Err("The machine's host key is missing or malformed.".into());
    };
    let kind_ok = kind
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '@'));
    let key_ok = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    if !kind_ok || !key_ok {
        return Err("The machine's host key is malformed.".into());
    }
    let host = if port == 22 {
        name.to_string()
    } else {
        format!("[{name}]:{port}")
    };
    Ok(format!("{host} {kind} {key}"))
}

/// Replaces `path` with the one line that pins `host_key` for `name` ([`known_hosts_line`]): written beside it and
/// renamed over it, readable only by this user on Unix. The file holds this machine alone, so a stale key from its
/// previous wake is gone, not merely shadowed.
pub fn write_known_hosts(path: &Path, name: &str, port: u16, host_key: &str) -> Result<()> {
    let line = known_hosts_line(name, port, host_key)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let part = path.with_extension("part");
    std::fs::write(&part, format!("{line}\n")).map_err(|e| format!("Could not write {}: {e}", part.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Could not protect {}: {e}", part.display()))?;
    }
    std::fs::rename(&part, path).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// `ssh`, or `NEXUS_SSH` (another OpenSSH, or a wrapper; debug builds only).
fn ssh_program() -> PathBuf {
    crate::development_setting("NEXUS_SSH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ssh"))
}

/// The person-facing reason in ssh's own words: its last non-empty line ("Permission denied (publickey).").
pub fn ssh_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_any_shell() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME; rm"), "'$HOME; rm'");
    }

    #[test]
    fn a_destination_is_not_an_option() {
        assert!(valid_destination("box"));
        assert!(valid_destination("me@vps.example.com"));
        assert!(valid_destination("tail-box"));
        assert!(!valid_destination(""));
        assert!(!valid_destination("-oProxyCommand=evil"));
        assert!(!valid_destination("box rm"));
        assert!(!valid_destination("box\n"));
    }

    /// `NEXUS_SSH=/x cargo test --release` shows the release side; a test here must not change the environment.
    #[test]
    fn only_a_debug_build_follows_nexus_ssh() {
        let told = std::env::var_os("NEXUS_SSH").filter(|program| !program.is_empty());
        let expected = match told {
            Some(program) if cfg!(debug_assertions) => PathBuf::from(program),
            _ => PathBuf::from("ssh"),
        };
        assert_eq!(ssh_program(), expected);
    }

    #[test]
    fn ssh_runs_sh_with_quoted_arguments_and_forwards_nothing() {
        let target = Target::Ssh {
            destination: "me@box".into(),
        };
        let command = target
            .command("echo \"$1\"", &["it's"])
            .unwrap_or_else(|e| panic!("{e}"));
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ForwardAgent=no".to_string()));
        assert!(!args.iter().any(|a| a == "-A"));
        let at = args.iter().position(|a| a == "--").unwrap_or_default();
        assert_eq!(args[at + 1], "me@box");
        assert_eq!(args[at + 2], r#"sh -c 'echo "$1"' nexus 'it'\''s'"#);
        assert!(
            Target::Ssh {
                destination: "-oProxyCommand=x".into()
            }
            .command("true", &[])
            .is_err()
        );
    }

    fn direct(alias: &str) -> Target {
        Target::Direct {
            host: "203.0.113.10".into(),
            port: 22001,
            user: "user".into(),
            identity: PathBuf::from("/data/Application Support/cloud/bx_1"),
            known_hosts: PathBuf::from("/data/Application Support/cloud/bx_1.known_hosts"),
            alias: alias.into(),
        }
    }

    fn args(command: &tokio::process::Command) -> Vec<String> {
        command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn direct_ignores_the_persons_config_and_pins_the_host_key_under_its_alias() {
        let command = direct("nexus-cloud-bx_1")
            .command("echo hi", &[])
            .unwrap_or_else(|e| panic!("{e}"));
        let args = args(&command);
        let pair = |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(pair("-F", "none"));
        assert!(pair("-p", "22001"));
        assert!(pair("-l", "user"));
        assert!(pair("-o", r#"IdentityFile="/data/Application Support/cloud/bx_1""#));
        assert!(pair("-o", "IdentitiesOnly=yes"));
        assert!(pair(
            "-o",
            r#"UserKnownHostsFile="/data/Application Support/cloud/bx_1.known_hosts""#
        ));
        assert!(pair("-o", "GlobalKnownHostsFile=/dev/null"));
        assert!(pair("-o", "StrictHostKeyChecking=yes"));
        assert!(pair("-o", "HostKeyAlias=nexus-cloud-bx_1"));
        assert!(pair("-o", "ClearAllForwardings=yes"));
        assert!(pair("-o", "BatchMode=yes"));
        let at = args.iter().position(|a| a == "--").unwrap_or_default();
        assert_eq!(args[at + 1], "203.0.113.10");
        assert_eq!(args[at + 2], "sh -c 'echo hi' nexus");
        assert_eq!(direct("nexus-cloud-bx_1").name(), "nexus-cloud-bx_1");
    }

    #[test]
    fn direct_refuses_what_would_change_the_command() {
        assert!(direct("-oProxyCommand=x").command("true", &[]).is_err());
        assert!(direct("a b").command("true", &[]).is_err());
        let mut target = direct("ok");
        if let Target::Direct { host, .. } = &mut target {
            *host = "me@203.0.113.10".into();
        }
        assert!(target.command("true", &[]).is_err());
        let mut target = direct("ok");
        if let Target::Direct { identity, .. } = &mut target {
            *identity = PathBuf::from("/tmp/%d/key");
        }
        assert!(target.command("true", &[]).is_err());
        let mut target = direct("ok");
        if let Target::Direct { user, .. } = &mut target {
            *user = "-oops".into();
        }
        assert!(target.command("true", &[]).is_err());
        let mut target = direct("ok");
        if let Target::Direct { port, .. } = &mut target {
            *port = 0;
        }
        assert!(target.command("true", &[]).is_err());
        assert_eq!(
            quoted_path(Path::new(r"C:\Users\me\kh")).unwrap_or_default(),
            r#""C:\\Users\\me\\kh""#
        );
    }

    #[test]
    fn a_forward_keeps_forwards_and_listens_on_loopback_only() {
        let command = direct("nexus-cloud-bx_1")
            .forward(53000, 3000)
            .unwrap_or_else(|e| panic!("{e}"));
        let args = args(&command);
        assert!(!args.iter().any(|a| a == "ClearAllForwardings=yes"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "ExitOnForwardFailure=yes")
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-L" && w[1] == "127.0.0.1:53000:127.0.0.1:3000")
        );
        assert!(args.iter().any(|a| a == "-N"));
        assert_eq!(args.last().map(String::as_str), Some("203.0.113.10"));
        assert!(direct("x").forward(0, 3000).is_err());
        assert!(Target::Local { env: vec![] }.forward(1, 2).is_err());
        let ssh = Target::Ssh {
            destination: "me@box".into(),
        };
        assert!(ssh.forward(53000, 3000).is_ok());
    }

    #[test]
    fn known_hosts_lines() {
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFake+/= user@host";
        assert_eq!(
            known_hosts_line("nexus-cloud-bx_1", 22, key).unwrap_or_default(),
            "nexus-cloud-bx_1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFake+/="
        );
        assert_eq!(
            known_hosts_line("203.0.113.10", 22001, key).unwrap_or_default(),
            "[203.0.113.10]:22001 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFake+/="
        );
        assert!(known_hosts_line("*", 22, key).is_err());
        assert!(known_hosts_line("a,b", 22, key).is_err());
        assert!(known_hosts_line("box", 22, "ssh-ed25519").is_err());
        // Only the type and the key are taken: a second line cannot ride along.
        assert_eq!(
            known_hosts_line("box", 22, "ssh-ed25519 AAAA\nevil ssh-rsa BBBB").unwrap_or_default(),
            "box ssh-ed25519 AAAA"
        );
        assert!(known_hosts_line("box", 22, "ssh-ed25519 AA;rm").is_err());
        assert!(known_hosts_line("box", 22, "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writes_one_private_line() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexus-remote-kh-{}", std::process::id()));
        let path = dir.join("bx_1.known_hosts");
        write_known_hosts(&path, "a", 22, "ssh-ed25519 AAAA").unwrap_or_else(|e| panic!("{e}"));
        write_known_hosts(&path, "a", 22, "ssh-ed25519 BBBB").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "a ssh-ed25519 BBBB\n"
        );
        let mode = std::fs::metadata(&path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_script_with_its_input() {
        let target = Target::Local {
            env: vec![("GREETING".into(), "hi".into())],
        };
        let out = target
            .run(
                "read line; echo \"$GREETING $1 $line\"",
                &["there"],
                Some(b"you\n".to_vec()),
                Duration::from_secs(5),
            )
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(out.ok);
        assert_eq!(out.stdout, "hi there you\n");
        let slow = target.run("sleep 5", &[], None, Duration::from_millis(100)).await;
        assert!(slow.is_err());
    }
}
