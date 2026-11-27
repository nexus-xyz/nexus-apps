//! Starting an agent as a child process and stopping it, with everything it started. On Unix the agent leads its own
//! process group, so a stop reaches its MCP servers and shells too (they otherwise outlive it).
//!
//! Every process the app starts is built by [`command`] or [`async_command`], which open no console window on
//! Windows. clippy.toml refuses `Command::new` elsewhere; a process that should have a console allows it and says why.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Child;

/// A command for `program` that opens no console window on Windows. The app has no console of its own, so Windows
/// gives each console program it starts (git, gh, an agent, taskkill) a new window, which flashes up on the screen.
pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(clippy::disallowed_methods)]
    let mut command = std::process::Command::new(program);
    hide_console(&mut command);
    command
}

/// Test helper. Writes an executable script (mode 755) from a child shell, so this process never holds the file open
/// for writing. Written here, a test thread that forks (any spawn) between the write and the close hands the open file
/// to its child until that child execs, and running the script then fails with "Text file busy" (ETXTBSY).
#[cfg(unix)]
#[doc(hidden)]
pub fn write_fake_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let written = command("/bin/sh")
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .arg(body)
        .status()
        .unwrap_or_else(|e| panic!("could not run sh to write {}: {e}", path.display()));
    assert!(written.success(), "could not write {}", path.display());
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("could not read {} back: {e}", path.display()))
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "{} is not mode 755", path.display());
}

/// [`command`] for tokio.
pub fn async_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(command(program))
}

#[cfg(windows)]
fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// A child gets no window of its own outside Windows.
#[cfg(not(windows))]
fn hide_console(_: &mut std::process::Command) {}

/// What to run: the program and its arguments, its whole environment and its folder.
#[derive(Clone, PartialEq)]
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: PathBuf,
}

/// The environment carries the Nexus token: debug output shows its names, never its values.
impl std::fmt::Debug for Spawn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.env.keys().collect();
        names.sort();
        f.debug_struct("Spawn")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &names)
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Why Windows can't start `spec`, when its program is a batch file (`claude.cmd`, as npm installs Claude Code) and an
/// argument has a line break: cmd.exe would end the command there, so Rust refuses it (CVE-2024-24576) with only
/// "batch file arguments are invalid". This names the argument instead. None for any other program.
pub fn batch_line_break(spec: &Spawn) -> Option<String> {
    let batch = spec
        .program
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat"));
    if !batch {
        return None;
    }
    let at = spec.args.iter().position(|arg| arg.contains(['\r', '\n']))?;
    // The option it belongs to, when it is a value: the value itself may hold a secret or a page of text.
    let arg = &spec.args[at];
    let name = if arg.starts_with('-') {
        arg.split('=').next().unwrap_or_default()
    } else {
        at.checked_sub(1)
            .and_then(|before| spec.args.get(before))
            .filter(|before| before.starts_with('-'))
            .map_or("an argument", String::as_str)
    };
    Some(format!(
        "{} is a batch file, and Windows can't pass it an argument with a line break ({name}). Install the agent's \
         native build, or update Nexus.",
        spec.program.display()
    ))
}

/// The niceness agents run at, and everything they start inherits: their builds, tests and MCP servers. The app's
/// window stays at the default, 0, so the scheduler gives it the CPU first. Under a 32-process load in one CPU group,
/// a 60 fps thread missed 40% of its frames against load at 0, and 4% against load at 10.
pub const AGENT_NICENESS: &str = "10";

/// The program and the arguments before the command's own that start `program` at [`AGENT_NICENESS`]:
/// `nice -n 10 program`. `program` alone on Windows, where there is no `nice`, and where `program` is not an
/// executable file at an absolute path: started without `nice`, it fails as before (a missing agent is still
/// `NotFound`), where `nice` would turn that into its own exit code.
pub fn below_the_app(program: &Path) -> (PathBuf, Vec<OsString>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = |path: &Path| {
            path.metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        };
        let nice = ["/usr/bin/nice", "/bin/nice"]
            .into_iter()
            .map(Path::new)
            .find(|nice| executable(nice));
        if let Some(nice) = nice
            && program.is_absolute()
            && executable(program)
        {
            return (
                nice.to_path_buf(),
                vec!["-n".into(), AGENT_NICENESS.into(), program.into()],
            );
        }
    }
    (program.to_path_buf(), Vec::new())
}

pub fn spawn(spec: &Spawn) -> std::io::Result<Child> {
    if cfg!(windows)
        && let Some(why) = batch_line_break(spec)
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, why));
    }
    let (program, before) = below_the_app(&spec.program);
    let mut command = async_command(&program);
    command
        .args(&before)
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command.spawn()
}

/// Asks the agent's process group to end (SIGTERM).
pub fn terminate(pid: Option<u32>) {
    signal(pid, false);
}

/// Ends the agent's process group at once (SIGKILL).
pub fn kill(pid: Option<u32>) {
    signal(pid, true);
}

/// How long a short-lived agent run (a plan probe) may take to end by itself once its stdin closed, before it is
/// signalled. Claude Code renews an expired sign-in as it starts, and saves the new token only after the vendor
/// answers. A run killed between the two leaves its login with a token the vendor already replaced, and the next
/// renewal finds it refused and blanks the login. A renewal takes seconds; this leaves it room.
pub const RETIRE_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Lets `child`, its stdin closed, end by itself: after [`RETIRE_GRACE`] its group gets SIGTERM, then SIGKILL. It
/// runs on a thread of its own that holds the child, so an app that quits meanwhile leaves the run to finish instead
/// of killing it through `kill_on_drop`.
pub fn retire(child: tokio::process::Child) {
    retire_after(child, RETIRE_GRACE, std::time::Duration::from_secs(5));
}

/// [`retire`] with its two waits: `grace` before SIGTERM, `term` before SIGKILL.
pub fn retire_after(mut child: tokio::process::Child, grace: std::time::Duration, term: std::time::Duration) {
    let pid = child.id();
    std::thread::spawn(move || {
        let ended = |child: &mut tokio::process::Child, within: std::time::Duration| {
            let deadline = std::time::Instant::now() + within;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => return true,
                    Ok(None) if std::time::Instant::now() >= deadline => return false,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
        };
        if !ended(&mut child, grace) {
            terminate(pid);
            if !ended(&mut child, term) {
                kill(pid);
                ended(&mut child, term);
            }
        }
        end_leftovers(pid);
    });
}

/// The agent exited and was just reaped: ends what it left running in its process group (MCP servers, shells), which
/// would otherwise live on as orphans and could keep its output open. Call it right after the reap: a group with a
/// process left in it keeps its id from being reused, and an empty one leaves only an instant for a new process to
/// take the id and lead a group under it. Not on Windows, where a dead process's id is soon reused, and `taskkill /T`
/// could reach someone else's processes.
pub fn end_leftovers(pid: Option<u32>) {
    #[cfg(unix)]
    signal(pid, true);
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(unix)]
fn signal(pid: Option<u32>, force: bool) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    let _ = killpg(
        Pid::from_raw(pid),
        if force { Signal::SIGKILL } else { Signal::SIGTERM },
    );
}

#[cfg(not(unix))]
fn signal(pid: Option<u32>, _force: bool) {
    // Windows (after v1): `taskkill /T /F` ends the process tree.
    if let Some(pid) = pid {
        let _ = command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// How long a stop waits after closing stdin, then after SIGTERM, before the next step.
pub const GRACE: Duration = Duration::from_secs(5);
pub const TERM_GRACE: Duration = Duration::from_secs(2);
/// After the agent exited, how long its output is still read: what it wrote last, unless a process it left behind
/// (out of reach of [`end_leftovers`]) holds the pipe open.
pub const DRAIN: Duration = Duration::from_secs(2);

/// Directories where `claude` and `codex` are usually installed, for an app started from the dock or the launcher,
/// which does not get the login shell's `PATH`.
pub fn usual_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        for dir in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            "bin",
        ] {
            dirs.push(home.join(dir));
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(dir));
    }
    dirs
}

/// The shell that runs terminal commands and gives the agents their `PATH`. The apps use [`Shell::Login`]; tests pass
/// [`Shell::sh`], so they never read `$SHELL` or start the person's login shell, which reads their startup files (a
/// zsh profile can start tmux).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Shell {
    /// The person's login shell: `$SHELL`, else `/bin/sh`.
    #[default]
    Login,
    /// This program, never the login shell. A command runs with `-c` (no login, so no startup files), an interactive
    /// terminal starts it with no arguments, and the agents' `PATH` is not asked of any shell. On Windows, terminal
    /// commands keep `%COMSPEC%`.
    Plain(PathBuf),
}

impl Shell {
    /// `/bin/sh` as [`Shell::Plain`]: what tests pass.
    pub fn sh() -> Shell {
        Shell::Plain(PathBuf::from("/bin/sh"))
    }
}

/// `PATH` for the agents: the login shell's (asked only for [`Shell::Login`]; it can take a moment), then the usual
/// install folders.
pub fn agent_path(current: Option<&str>, home: Option<&Path>, shell: &Shell) -> String {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let mut parts: Vec<String> = Vec::new();
    let mut add = |dir: String| {
        if !dir.is_empty() && !parts.contains(&dir) {
            parts.push(dir);
        }
    };
    if *shell == Shell::Login
        && let Some(shell) = nexus_local_agents::login_shell_path(&|name| std::env::var(name).ok())
    {
        shell.split(separator).for_each(|d| add(d.to_string()));
    }
    if let Some(current) = current {
        current.split(separator).for_each(|d| add(d.to_string()));
    }
    for dir in usual_dirs(home) {
        add(dir.to_string_lossy().to_string());
    }
    parts.join(separator)
}

pub use nexus_local_agents::stdout_within;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A retired run that never ends by itself is still stopped once its grace is over.
    #[cfg(unix)]
    #[test]
    fn a_retired_run_that_outlives_its_grace_is_stopped() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let spawn = Spawn {
            program: PathBuf::from("/bin/sleep"),
            args: vec!["30".into()],
            env: std::env::vars().collect(),
            cwd: std::env::temp_dir(),
        };
        let child = runtime.block_on(async { super::spawn(&spawn) }).expect("sleep");
        let pid = child.id().and_then(|pid| i32::try_from(pid).ok()).expect("pid");
        retire_after(
            child,
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(200),
        );
        let alive = || nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
        assert!(alive(), "not stopped before its grace");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!alive(), "still running after its grace");
    }

    #[test]
    fn a_line_break_for_a_batch_file_names_its_option_never_its_value() {
        let spawn = |program: &str, args: &[&str]| Spawn {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: HashMap::new(),
            cwd: PathBuf::from("."),
        };
        let why = batch_line_break(&spawn(
            r"C:\Users\me\AppData\Roaming\npm\claude.CMD",
            &["-p", "--append-system-prompt", "one\n\ntwo"],
        ));
        assert!(
            why.as_deref()
                .is_some_and(|why| why.contains("(--append-system-prompt)") && !why.contains("one"))
        );
        let why = batch_line_break(&spawn("codex.bat", &["--settings={\"a\":\"x\r\ny\"}"]));
        assert!(why.as_deref().is_some_and(|why| why.contains("(--settings)")));
        assert!(batch_line_break(&spawn("codex.cmd", &["-p", "one\ntwo"])).is_some_and(|why| why.contains("(-p)")));
        // A native build takes any argument, and a batch file any single-line one.
        assert_eq!(
            batch_line_break(&spawn("claude.exe", &["--append-system-prompt", "one\ntwo"])),
            None
        );
        assert_eq!(
            batch_line_break(&spawn("claude", &["--append-system-prompt", "one\ntwo"])),
            None
        );
        assert_eq!(
            batch_line_break(&spawn("claude.cmd", &["--append-system-prompt-file", "x.md"])),
            None
        );
    }

    /// The niceness `nice` prints in this test's process, plus `more`, capped at 19 as the kernel caps it.
    #[cfg(unix)]
    pub(crate) fn niceness_plus(more: i32) -> String {
        let out = command("nice").output().expect("run nice");
        let own: i32 = String::from_utf8_lossy(&out.stdout).trim().parse().expect("a niceness");
        (own + more).min(19).to_string()
    }

    /// an agent runs below the app, and so does what it starts. A missing agent still fails to start as
    /// `NotFound`, not as `nice`'s exit code.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_agent_runs_below_the_app_and_a_missing_one_still_is_not_found() {
        let spawn = |program: &str| Spawn {
            program: PathBuf::from(program),
            args: vec!["-c".into(), "nice; sh -c nice".into()],
            env: HashMap::new(),
            cwd: PathBuf::from("/"),
        };
        let child = super::spawn(&spawn("/bin/sh")).expect("start the agent");
        let out = child.wait_with_output().await.expect("the agent's output");
        let lowered = niceness_plus(AGENT_NICENESS.parse().expect("a number"));
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{lowered}\n{lowered}\n"));

        let missing = super::spawn(&spawn("/no/such/agent")).expect_err("no agent there");
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        // A bare name is found on PATH by the OS, as before.
        assert_eq!(below_the_app(Path::new("sh")), (PathBuf::from("sh"), Vec::new()));
    }

    /// A process left running with the output open (as a shell profile's daemon) does not hold the reader: what was
    /// printed before the exit comes back soon after it.
    #[cfg(unix)]
    #[test]
    fn output_held_open_by_a_process_left_behind_does_not_hold_the_caller() {
        let mut shell = command("/bin/sh");
        shell.args(["-c", "sleep 30 & printf 'first\\n__NEXUS_PATH__/x\\n'"]);
        let started = std::time::Instant::now();
        let (status, stdout) = stdout_within(shell, Duration::from_secs(3)).unwrap_or_else(|| panic!("ran over"));
        assert!(status.success());
        assert_eq!(stdout, "first\n__NEXUS_PATH__/x\n");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    /// More than a pipe holds is read as it comes, so the command finishes; one that runs over is killed.
    #[cfg(unix)]
    #[test]
    fn reads_long_output_and_stops_a_command_that_runs_over() {
        let mut long = command("/bin/sh");
        long.args(["-c", "head -c 300000 /dev/zero | tr '\\0' x"]);
        let (_, stdout) = stdout_within(long, Duration::from_secs(10)).unwrap_or_else(|| panic!("ran over"));
        assert_eq!(stdout.len(), 300_000);
        let mut slow = command("/bin/sh");
        slow.args(["-c", "exec sleep 30"]);
        let started = std::time::Instant::now();
        assert!(stdout_within(slow, Duration::from_millis(200)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }
}
