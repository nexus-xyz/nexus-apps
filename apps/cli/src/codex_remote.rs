//! `nexus codex` with phone control. With Phone control on (`nexus devices control on`) and session sync
//! on, the CLI starts Codex's own `codex app-server` on a private Unix socket and runs Codex's TUI as its client
//! (`codex --remote unix://PATH`). The person uses the TUI as before. The CLI's own client (`client.rs`) attaches to
//! the same thread, so a phone can send a message, stop the turn and answer an approval while the TUI runs.
//!
//! Both are experimental in Codex 0.169. When the installed Codex has no `--remote`, or its app-server does not start,
//! Codex runs as before: the phone reads the session only, and one line says why.

pub mod client;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long the app-server may take to listen.
const LISTEN: Duration = Duration::from_secs(10);

/// Codex's subcommands that do not start the TUI. `resume` and `fork` start it.
const NOT_TUI: &[&str] = &[
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "mcp-server",
    "plugin",
    "app-server",
    "remote-control",
    "app",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "migrate-nfs-db",
    "unarchive",
    "cloud",
    "exec-server",
    "features",
    "help",
    "agents",
];

/// Codex's options that take a value, so their value is not read as a subcommand.
const WITH_VALUE: &[&str] = &[
    "-c",
    "--config",
    "--enable",
    "--disable",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-C",
    "--cd",
    "-i",
    "--image",
    "--add-dir",
    "--local-provider",
    "--remote-auth-token-env",
];

/// The options the app-server takes too: configuration, so the model calls it makes go where the TUI's would.
const FOR_SERVER: &[&str] = &["-c", "--config", "--enable", "--disable"];

/// Whether `args` start Codex's TUI, which phone control needs. A run that names its own `--remote` keeps it.
pub fn starts_tui(args: &[String]) -> bool {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--remote" || arg.starts_with("--remote=") {
            return false;
        }
        if arg == "--" {
            return true;
        }
        if WITH_VALUE.contains(&arg.as_str()) {
            args.next();
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        return !NOT_TUI.contains(&arg.as_str());
    }
    true
}

/// The app-server's configuration options, from the run's arguments.
pub fn server_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "-p" || arg == "--profile" || arg.starts_with("--profile=") || arg.starts_with("-p") && arg.len() > 2
        {
            return Err("this Codex app-server cannot use named profiles; this run keeps your profile".into());
        }
        if FOR_SERVER.contains(&arg.as_str()) {
            if let Some(value) = args.next() {
                out.extend([arg.clone(), value.clone()]);
            }
        } else if FOR_SERVER
            .iter()
            .any(|flag| flag.starts_with("--") && arg.starts_with(&format!("{flag}=")))
        {
            out.push(arg.clone());
        } else if WITH_VALUE.contains(&arg.as_str()) {
            args.next();
        }
    }
    Ok(out)
}

/// The TUI's arguments: the run's, as a client of the app-server at `socket`.
pub fn tui_args(socket: &Path, args: &[String]) -> Vec<String> {
    let mut out = vec!["--remote".to_string(), format!("unix://{}", socket.display())];
    out.extend(args.iter().cloned());
    out
}

/// Whether this Codex's TUI connects to an app-server on a Unix socket: `codex --help` names `--remote` and `unix://`.
pub fn supported(bin: &Path, env: &HashMap<String, String>) -> bool {
    Command::new(bin)
        .arg("--help")
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            let help = String::from_utf8_lossy(&output.stdout);
            help.contains("--remote <") && help.contains("unix://")
        })
}

/// The run's app-server, when Phone control is on and this run can use it: a TUI run with session sync on. When it is
/// on and the server cannot run, `say` gets one line, and Codex runs as before.
#[allow(clippy::too_many_arguments)]
pub fn for_run(
    ctx: &crate::Context,
    core: &crate::client::Client,
    gateway: &str,
    bin: &Path,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &Path,
    say: impl Fn(&str),
) -> Option<Server> {
    if !ctx.interactive
        || !crate::session_log::enabled(ctx)
        || !starts_tui(args)
        || !crate::phone_control::on(core, ctx, gateway)
    {
        return None;
    }
    let started = if supported(bin, env) {
        Server::start(bin, args, env, cwd, &crate::session_log::dir(ctx).join("run"))
    } else {
        Err("this Codex cannot run as an app-server client; Codex 0.169 or later can".into())
    };
    match started {
        Ok(server) => Some(server),
        Err(why) => {
            say(&format!(
                "Phone control: off for this run ({why}). Your phone only reads it."
            ));
            None
        }
    }
}

/// The run's app-server. Dropping it stops the server and removes its socket.
pub struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    /// Starts `codex app-server` on a new socket in `dir`, the CLI's runtime folder (made private to the person), and
    /// waits until it listens. Codex keeps the socket itself in a folder only the person can read, with mode 0600, and
    /// links it at the path.
    pub fn start(
        bin: &Path,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        dir: &Path,
    ) -> Result<Server, String> {
        let args = server_args(args)?;
        private_dir(dir).map_err(|_| "the CLI's runtime folder could not be made".to_string())?;
        let socket = dir.join(format!("codex-{}.sock", uuid::Uuid::new_v4().simple()));
        let child = Command::new(bin)
            .arg("app-server")
            .arg("--listen")
            .arg(format!("unix://{}", socket.display()))
            .args(args)
            .env_clear()
            .envs(env)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Codex's app-server did not start".to_string())?;
        let mut server = Server { child, socket };
        let started = Instant::now();
        loop {
            if std::os::unix::net::UnixStream::connect(&server.socket).is_ok() {
                return Ok(server);
            }
            if !matches!(server.child.try_wait(), Ok(None)) {
                return Err("Codex's app-server stopped as it started".into());
            }
            if started.elapsed() >= LISTEN {
                return Err("Codex's app-server did not listen in time".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Server {
    /// Asks the server to stop, so it writes the rest of the conversation, and kills it after two seconds.
    fn drop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(self.child.id() as i32);
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        let asked = Instant::now();
        while matches!(self.child.try_wait(), Ok(None)) && asked.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn only_a_tui_run_gets_phone_control() {
        assert!(starts_tui(&args(&[])));
        assert!(starts_tui(&args(&["-c", "model=\"x\"", "fix the tests"])));
        assert!(starts_tui(&args(&["--model", "exec", "resume", "--last"])));
        assert!(starts_tui(&args(&["resume", "019a"])));
        assert!(!starts_tui(&args(&["-c", "a=1", "exec", "hello"])));
        assert!(!starts_tui(&args(&["login"])));
        assert!(!starts_tui(&args(&["--remote", "ws://host:1"])));
    }

    #[test]
    fn named_profiles_are_rejected_without_changing_the_run_configuration() {
        for run in [
            args(&["-p", "work"]),
            args(&["--profile", "work"]),
            args(&["--profile=work"]),
            args(&["-pwork"]),
        ] {
            assert_eq!(
                server_args(&run),
                Err("this Codex app-server cannot use named profiles; this run keeps your profile".into())
            );
        }
        assert_eq!(
            server_args(&args(&["-c", "--profile=work"])),
            Ok(args(&["-c", "--profile=work"]))
        );
        assert_eq!(server_args(&args(&["--", "--profile=work"])), Ok(vec![]));
    }

    #[test]
    fn the_server_gets_the_run_s_configuration_only() {
        let run = args(&[
            "-c",
            "model_provider=\"nexus\"",
            "--enable",
            "goals",
            "--config=web_search=\"disabled\"",
            "-m",
            "gpt-5",
            "resume",
            "019a",
        ]);
        assert_eq!(
            server_args(&run).expect("supported configuration"),
            args(&[
                "-c",
                "model_provider=\"nexus\"",
                "--enable",
                "goals",
                "--config=web_search=\"disabled\""
            ])
        );
        let socket = Path::new("/run/codex.sock");
        assert_eq!(
            tui_args(socket, &run)[..2],
            args(&["--remote", "unix:///run/codex.sock"])
        );
        assert_eq!(tui_args(socket, &run)[2..], run[..]);
    }
}
