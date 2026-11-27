//! What this machine has of each coding agent `nexus` launches (Claude Code, Codex, Antigravity): whether it is
//! installed, its version, and how it is signed in, from the agent's own status command (`claude auth status --json`,
//! `codex login status`, `agy -p "/usage" --output-format json`). The same checks as the TypeScript CLI's `access.ts`
//! for Claude Code and Codex; the answer has the shape of `@nexus/kit`'s `LocalAgentStatus`, so Nexus for desktop hands
//! it to Nexus UI's pages as it is.
//!
//! Only the sign-in method and the plan's name go into a [`LocalAgent`]; the account's organization is never read, and
//! its email only by [`login_with`], for the line `nexus claude` prints before it starts. Each command runs
//! with a deadline, so a hung agent never holds up a caller for long.
//!
//! A desktop app started from the Finder, the Dock or at login gets a minimal `PATH` (`/usr/bin:/bin:…`), so agents
//! installed in `~/.local/bin`, Homebrew or npm's global folder would look missing: [`search_path`] adds those
//! folders, and the login shell's own `PATH` when the agent is not found otherwise.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use nexus_agents::{Agent, AgentEntry};

pub mod agents;
pub mod antigravity_models;
pub mod claude_models;
pub mod codex_models;
mod codex_rollouts;
pub mod launch;
pub mod models;
pub mod native_memory;
mod output;
pub use output::stdout_within;
mod shell;
pub use agents::{
    CLAUDE_NO_USAGE_ENV, CODEX_KEY_ENV, LocalAgentT, antigravity_login, antigravity_settings, claude_plan,
    read_claude_account, read_claude_status, read_codex_status,
};
pub use shell::{MissingBinary, bin_variable, find_agent, profile_dir, profile_under, route_variable};

use agents::local;

/// How long one status command may take.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// How the agent is signed in on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Login {
    /// To the person's plan (Claude, ChatGPT): what a subscription run needs.
    Plan,
    /// With an API key or another billing (Console, a cloud): a subscription run would not bill the plan.
    ApiKey,
    SignedOut,
    /// The agent did not say (an older version, a timeout, output this module does not know).
    Unknown,
}

/// An agent on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgent {
    pub agent: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// The plan's name as the agent gives it (`max`, `team`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// Where agents are commonly installed, beyond `PATH`: each agent's own installer (Claude Code's), Homebrew, npm's and
/// Volta's global folders.
fn common_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".local/bin")];
    dirs.extend(
        Agent::ALL
            .iter()
            .flat_map(|agent| agent.entry().install.dirs)
            .map(|dir| home.join(dir)),
    );
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".npm-global/bin"),
        home.join(".volta/bin"),
        home.join(".bun/bin"),
    ]);
    if cfg!(windows) {
        dirs.clear();
    }
    dirs
}

/// The folders to look for agents in: `PATH`, then the common install folders.
pub fn search_path(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env("PATH")
        .or_else(|| env("Path"))
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = env("HOME").filter(|home| !home.is_empty()) {
        for dir in common_dirs(Path::new(&home)) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// The readiness check's lookup in `dirs`, the same as the launch's ([`find_agent`]). A relative `NEXUS_<AGENT>_BIN`
/// resolves under the current folder. `None` when the agent is not found, or when `NEXUS_<AGENT>_BIN` names no file.
pub fn find_in(agent: Agent, dirs: &[PathBuf], env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    find_entry_in(agent.entry(), dirs, env)
}

/// [`find_in`] for `entry`.
fn find_entry_in(entry: &AgentEntry, dirs: &[PathBuf], env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    shell::find_entry(entry, dirs, env, cfg!(windows), &cwd).ok().flatten()
}

/// Marks the `PATH` in the login shell's output, which a profile may print around.
const PATH_MARK: &str = "__NEXUS_PATH__";

/// The login shell's `PATH`, for an app that did not inherit one; `None` when it cannot tell in time. Interactive
/// (`-ilc`), so `.zshrc` and `.bashrc` run too: nvm and the like add to `PATH` there.
pub fn login_shell_path(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = env("SHELL").filter(|shell| !shell.is_empty())?;
    let mut command = Command::new(shell);
    command.args(["-ilc", &format!("printf '{PATH_MARK}%s{PATH_MARK}' \"$PATH\"")]);
    let (_, stdout) = stdout_within(command, Duration::from_secs(3))?;
    read_marked_path(&stdout)
}

/// The `PATH` between the marks.
fn read_marked_path(output: &str) -> Option<String> {
    let (_, rest) = output.split_once(PATH_MARK)?;
    let (path, _) = rest.split_once(PATH_MARK)?;
    (!path.trim().is_empty()).then(|| path.trim().to_string())
}

/// Run a command with nothing on stdin and its output collected, all within the deadline (`None`: it could not start,
/// did not finish, or something it started kept its output open). At the deadline the command is no longer waited for,
/// and it is killed [`RETIRE_GRACE`] later if it has not ended ([`retire`]); a reader still waiting on a pipe a
/// grandchild holds is left to end on its own, never waited for.
fn run_with_deadline(command: Command, deadline: Duration) -> Option<(Option<i32>, String, String)> {
    run_until(command, deadline, None)
}

/// [`run_with_deadline`], and with `stop`: once stdout or stderr holds it, the command is killed at once and what it
/// printed so far comes back, with no exit code. For a check that, signed out, waits for a sign-in instead of ending.
fn run_until(mut command: Command, deadline: Duration, stop: Option<&str>) -> Option<(Option<i32>, String, String)> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for each check: Nexus for desktop has no console, so Windows would open one.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let started = Instant::now();
    let mut child = command.spawn().ok()?;
    // Each pipe's reader sends what it reads as it comes, then `None` at its end.
    let (sender, receiver) = mpsc::channel::<(bool, Option<Vec<u8>>)>();
    let pipes: [(bool, Box<dyn Read + Send>); 2] = [
        (true, Box::new(child.stdout.take()?)),
        (false, Box::new(child.stderr.take()?)),
    ];
    for (is_stdout, mut pipe) in pipes {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        let _ = sender.send((is_stdout, None));
                        break;
                    }
                    Ok(read) => {
                        if sender.send((is_stdout, Some(buffer[..read].to_vec()))).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    drop(sender);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let mut open = 2;
    let mut status = None;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let holds = |bytes: &[u8], needle: &str| bytes.windows(needle.len().max(1)).any(|w| w == needle.as_bytes());
    loop {
        match receiver.recv_timeout(Duration::from_millis(15)) {
            Ok((true, Some(chunk))) => stdout.extend(chunk),
            Ok((false, Some(chunk))) => stderr.extend(chunk),
            Ok((_, None)) => open -= 1,
            // Both pipes closed and the command still runs: wait without spinning.
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(15)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(stop) = stop.filter(|stop| !stop.is_empty())
            && (holds(&stdout, stop) || holds(&stderr, stop))
        {
            if status.is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Some((None, text(&stdout), text(&stderr)));
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(ended)) => status = Some(ended),
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        if status.is_some() && open == 0 {
            break;
        }
        if started.elapsed() >= deadline {
            if status.is_none() {
                retire(child);
            }
            return None;
        }
    }
    Some((status.and_then(|status| status.code()), text(&stdout), text(&stderr)))
}

/// How long a check that missed its deadline may take to end by itself before it is killed. `claude auth status`
/// renews an expired sign-in and saves the new token only after the vendor answers: a kill between the two leaves a
/// token the vendor already replaced, and the next renewal blanks the login.
const RETIRE_GRACE: Duration = Duration::from_secs(30);

/// Stops waiting for `child` but lets it end by itself, on a thread of its own; it is killed after [`RETIRE_GRACE`].
fn retire(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let started = Instant::now();
        while started.elapsed() < RETIRE_GRACE {
            match child.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                _ => return,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

/// The agent as a command: a script (the tests' stand-in agents) runs with `node`.
pub fn command(bin: &Path) -> Command {
    let script = bin
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "mjs" | "cjs" | "js"));
    if script {
        let mut command = Command::new("node");
        command.arg(bin);
        command
    } else {
        Command::new(bin)
    }
}

/// The agent with `args` for a status check: `path` as `PATH`, the caller's `HOME`, nothing that bills elsewhere.
fn agent_command(bin: &Path, args: &[&str], path: &str, env: &dyn Fn(&str) -> Option<String>, agent: Agent) -> Command {
    let mut command = crate::command(bin);
    command.args(args).env("PATH", path);
    for name in agent.entry().billing.plan_overrides {
        command.env_remove(name);
    }
    // HOME decides where the agent keeps its login; pass the caller's.
    if let Some(home) = env("HOME") {
        command.env("HOME", home);
    }
    command
}

/// The version in `--version`'s output (`2.1.303 (Claude Code)`, `codex-cli 0.163.0`, `v1.2.3`).
pub fn read_version(output: &str) -> Option<String> {
    // The TypeScript CLI's `/\d+\.\d+(?:\.\d+)?(?:[-+][\w.]+)?/`: its first match.
    let bytes = output.as_bytes();
    let digits = |from: usize| from + bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.';
    for start in 0..bytes.len() {
        let major = digits(start);
        if major == start || bytes.get(major) != Some(&b'.') || !bytes.get(major + 1).is_some_and(u8::is_ascii_digit) {
            continue;
        }
        let mut end = digits(major + 1);
        if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
            end = digits(end + 1);
        }
        if matches!(bytes.get(end), Some(b'-' | b'+')) && bytes.get(end + 1).is_some_and(word) {
            end += 1 + bytes[end + 1..].iter().take_while(|b| word(b)).count();
        }
        return Some(output[start..end].to_string());
    }
    None
}

/// The Google Cloud project of agy's last business sign-in ("Use Business Account", a Gemini Enterprise license), from
/// its settings file under `home`, read without running `agy`. agy keeps it after a personal sign-in, so it names the
/// project and never tells which sign-in is current: `/usage` does, answering placeholders for a Google Cloud sign-in
/// (`nexus-client/tests/fixtures/agy-1.5.11-usage-google-cloud.json`). The file is agy's own state, not an interface.
pub fn antigravity_cloud_project(home: Option<&str>) -> Option<String> {
    let home = home.filter(|home| !home.is_empty())?;
    let text = std::fs::read_to_string(antigravity_settings(Path::new(home))).ok()?;
    let settings: Value = serde_json::from_str(&text).ok()?;
    settings
        .pointer("/gcp/project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|project| !project.is_empty())
        .map(String::from)
}

/// The sign-in known without running the agent's check, which then is not run ([`LocalAgentT::settled_login`]).
fn settled_login(agent: Agent, home: Option<&str>) -> Option<Login> {
    local(agent).settled_login(home)
}

/// Models that share a limit on the person's Google AI plan, as `agy`'s `/usage` lists them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaGroup {
    /// "Gemini Models".
    pub name: String,
    /// Its models as `/usage` names them ("Gemini 3.8 Flash"), from the group's description.
    pub models: Vec<String>,
    pub buckets: Vec<QuotaBucket>,
}

/// One limit of a [`QuotaGroup`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    /// "Weekly Limit Remaining".
    pub name: String,
    /// The limit's window as `agy` names it (`weekly`); empty when it gave none.
    pub window: String,
    /// What is left, 0 to 1.
    pub remaining: f64,
    /// When it refreshes (RFC 3339, UTC), when `agy` says.
    pub resets_at: Option<String>,
}

/// The quota groups in `agy -p "/usage" --output-format json`'s answer: only the `/usage` command's own answer, with
/// no turn taken, counts. `None` for anything else: an error, a sign-in that did not finish, or a model's turn.
pub fn read_antigravity_usage(stdout: &str) -> Option<Vec<QuotaGroup>> {
    let answer: Value = stdout.lines().rev().find_map(|line| {
        serde_json::from_str::<Value>(line.trim())
            .ok()
            .filter(|answer| answer.get("status").is_some())
    })?;
    let answered = answer.get("status").and_then(Value::as_str) == Some("SUCCESS")
        && answer.pointer("/command/name").and_then(Value::as_str) == Some("usage")
        && answer.get("num_turns").and_then(Value::as_u64) == Some(0);
    if !answered {
        return None;
    }
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let groups = answer.pointer("/command/data/groups")?.as_array()?;
    Some(
        groups
            .iter()
            .filter_map(|group| {
                let buckets: Vec<QuotaBucket> = group
                    .get("buckets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|bucket| {
                        let remaining = bucket.get("remaining_fraction")?.as_f64()?.clamp(0.0, 1.0);
                        Some(QuotaBucket {
                            name: text(bucket, "name"),
                            window: text(bucket, "window"),
                            remaining,
                            resets_at: bucket
                                .get("reset_time")
                                .and_then(Value::as_str)
                                .filter(|at| !at.is_empty())
                                .map(String::from),
                        })
                    })
                    .collect();
                let description = text(group, "description");
                let models = description
                    .split_once(": ")
                    .map(|(_, list)| list.split(", ").map(|model| model.trim().to_string()).collect())
                    .unwrap_or_default();
                (!buckets.is_empty()).then(|| QuotaGroup {
                    name: text(group, "name"),
                    models,
                    buckets,
                })
            })
            .collect(),
    )
}

/// Antigravity's check (`/usage`): an answer with quota is a plan sign-in; anything else is not known.
pub fn read_antigravity_status(stdout: &str) -> Login {
    match read_antigravity_usage(stdout) {
        Some(groups) if !groups.is_empty() => Login::Plan,
        _ => Login::Unknown,
    }
}

/// The group a session's model belongs to ([`names_model`]).
pub fn quota_group_of<'a>(groups: &'a [QuotaGroup], model: &str) -> Option<&'a QuotaGroup> {
    groups.iter().find(|group| names_model(&group.models, model))
}

/// Whether `names` ("Gemini 3.8 Flash", "Claude Opus 4.6") include the model whose `agy` id is `model`
/// (`gemini-3.8-flash`, `claude-opus-4-6-thinking`): the id starts with a name once both are reduced to letters and
/// digits.
pub fn names_model(names: &[String], model: &str) -> bool {
    let reduce = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let model = reduce(model);
    !model.is_empty()
        && names.iter().any(|name| {
            let name = reduce(name);
            !name.is_empty() && model.starts_with(&name)
        })
}

/// The sign-in check's own browser launchers: no-op `xdg-open` and `open`, first on its `PATH`. Signed out, `agy`'s
/// check starts a sign-in and runs `xdg-open` (found on `PATH`) with Google's sign-in page, display or not; a check that
/// runs whenever a page opens must never do that. In the person's cache folder, not the temporary folder, which a
/// sandboxed agent may not see. `None` where they can't be put in place: on Windows, or with no home folder.
fn no_browser_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let cache = env("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".cache"))
        })?;
    let dir = cache.join("nexus").join("no-browser");
    std::fs::create_dir_all(&dir).ok()?;
    const SCRIPT: &str =
        "#!/bin/sh\n# Nexus's sign-in checks run with this first on PATH, so a check never opens a browser.\nexit 0\n";
    let in_place =
        |launcher: &Path| std::fs::read_to_string(launcher).is_ok_and(|text| text == SCRIPT) && is_executable(launcher);
    for name in ["xdg-open", "open"] {
        let launcher = dir.join(name);
        if in_place(&launcher) {
            continue;
        }
        // Written beside it and moved into place, so a check running meanwhile never starts half a file. Each writer's
        // own name: two checks of one process (the Agents page reads sign-ins and plans together) never share it.
        let staged = dir.join(format!(".{name}.{}", uuid::Uuid::new_v4().simple()));
        #[cfg(unix)]
        let executable = |path: &Path| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).is_ok()
        };
        #[cfg(not(unix))]
        let executable = |_: &Path| true;
        let placed = std::fs::write(&staged, SCRIPT).is_ok()
            && executable(&staged)
            && std::fs::rename(&staged, &launcher).is_ok();
        let _ = std::fs::remove_file(&staged);
        // Another check may have placed it meanwhile.
        if !(placed || in_place(&launcher)) {
            return None;
        }
    }
    Some(dir)
}

/// "1.5.8" as numbers; `None` when it is not one.
pub fn version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split(['-', '+']).next()?.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    Some((next()?, next()?, next().unwrap_or(0)))
}

/// Whether `version` (`1.5.8`, `1.1.11-beta`) is older than `oldest`, or not a version. A pre-release of `oldest` is
/// older than it.
pub fn older_than(version: &str, oldest: (u64, u64, u64)) -> bool {
    let pre_release = version.trim().split('+').next().is_some_and(|core| core.contains('-'));
    version_triple(version).is_none_or(|triple| triple < oldest || (triple == oldest && pre_release))
}

/// The command for `entry`'s sign-in check, made by `build` from its arguments; `None` when it is not run: the entry
/// has no check, the agent is older than the check's version (or its version unknown), or the check would open a
/// browser where its launchers can't be stubbed. `path` is the `PATH` the command gets.
fn status_command(
    entry: &AgentEntry,
    version: Option<&str>,
    path: &str,
    env: &dyn Fn(&str) -> Option<String>,
    build: impl FnOnce(&[&str]) -> Command,
) -> Option<Command> {
    let args = entry.sign_in.status_args?;
    if let Some(oldest) = entry.sign_in.status_min_version
        && version.is_none_or(|version| older_than(version, oldest))
    {
        return None;
    }
    let mut command = build(args);
    if entry.sign_in.status_sign_in_prompt.is_some() {
        without_browser(&mut command, path, env)?;
    }
    Some(command)
}

/// `command` with [`no_browser_dir`]'s launchers first on its `PATH` (then `path`'s folders, empty ones dropped: an
/// empty entry is the current folder) and no display or desktop session, so a sign-in it starts opens nothing. The
/// D-Bus session stays: agy reads its sign-in from the system keyring over it. `None` where the launchers can't be
/// put in place.
fn without_browser(command: &mut Command, path: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<()> {
    let dir = no_browser_dir(env)?;
    let folders = std::env::split_paths(path).filter(|folder| !folder.as_os_str().is_empty());
    let joined = std::env::join_paths(std::iter::once(dir.clone()).chain(folders))
        .map(|joined| joined.to_string_lossy().into_owned())
        .ok()?;
    command
        .env("PATH", joined)
        .env("BROWSER", dir.join("xdg-open"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_CURRENT_DESKTOP")
        .env_remove("DESKTOP_SESSION");
    Some(())
}

/// What `entry`'s check said, run by [`status_command`]'s command: its sign-in prompt is signed out; a check that did
/// not finish in time, or output this module does not know, is unknown.
fn read_check(agent: Agent, output: Option<&(Option<i32>, String, String)>) -> (Login, Option<String>) {
    let Some((_, stdout, stderr)) = output else {
        return (Login::Unknown, None);
    };
    if let Some(prompt) = agent.entry().sign_in.status_sign_in_prompt
        && (stdout.contains(prompt) || stderr.contains(prompt))
    {
        return (Login::SignedOut, None);
    }
    local(agent).read_status(stdout, stderr)
}

/// What this machine has of `agent`, with the folders to look in (see [`search_path`]).
pub fn detect_in(agent: Agent, dirs: &[PathBuf], env: &dyn Fn(&str) -> Option<String>) -> LocalAgent {
    let Some(bin) = find_in(agent, dirs, env) else {
        return LocalAgent {
            agent: agent.as_str().into(),
            installed: false,
            path: None,
            version: None,
            login: None,
            plan: None,
        };
    };
    // The agent's own folder first: an npm-installed agent is a Node script next to its `node`.
    let mut path_dirs: Vec<PathBuf> = bin.parent().map(Path::to_path_buf).into_iter().collect();
    path_dirs.extend(dirs.iter().cloned());
    let path = std::env::join_paths(path_dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_default();
    let version = run_with_deadline(agent_command(&bin, &["--version"], &path, env, agent), STATUS_TIMEOUT)
        .filter(|(status, _, _)| *status == Some(0))
        .and_then(|(_, stdout, _)| read_version(&stdout));
    // A check that is not run leaves `login` unset, which `LocalAgentStatus` reads as not checked.
    let entry = agent.entry();
    let home = env("HOME").or_else(|| env("USERPROFILE"));
    let (login, plan) = match settled_login(agent, home.as_deref()) {
        Some(login) => (Some(login), None),
        None => match status_command(entry, version.as_deref(), &path, env, |args| {
            agent_command(&bin, args, &path, env, agent)
        }) {
            None => (None, None),
            Some(command) => {
                let output = run_until(
                    command,
                    entry.sign_in.status_timeout,
                    entry.sign_in.status_sign_in_prompt,
                );
                let (login, plan) = read_check(agent, output.as_ref());
                (Some(login), plan)
            }
        },
    };
    LocalAgent {
        agent: agent.as_str().into(),
        installed: true,
        path: Some(bin.to_string_lossy().into_owned()),
        version,
        login,
        plan,
    }
}

/// `bin` run in `cwd` with exactly `env` (the CLI's own environment, as its run of the agent would have it).
fn command_with(bin: &Path, args: &[&str], env: &HashMap<String, String>, cwd: &Path) -> Command {
    let mut command = crate::command(bin);
    command.args(args).env_clear().envs(env).current_dir(cwd);
    command
}

/// How `bin` is signed in, checked with the environment a subscription run would give it (`checkLogin` in access.ts):
/// the sign-in, the plan, and the account (Claude Code only; Codex's status does not name one). `None` when the check
/// is not run ([`status_command`]): the caller says the sign-in is not checked. Antigravity set to bill an API key is
/// known from its settings file, before any check.
pub fn login_with(
    agent: Agent,
    bin: &Path,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> Option<(Login, Option<String>, Option<String>)> {
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE"));
    if let Some(login) = settled_login(agent, home.map(String::as_str)) {
        return Some((login, None, None));
    }
    let output = status_output(agent.entry(), bin, env, cwd)?;
    let (login, plan) = read_check(agent, output.as_ref());
    let account = output
        .as_ref()
        .filter(|_| login == Login::Plan)
        .and_then(|(_, stdout, _)| local(agent).read_account(stdout));
    Some((login, plan, account))
}

/// What `entry`'s sign-in check at `bin` printed, run without what would bill elsewhere; the inner `None` when it did
/// not finish in time. `None` when the check is not run ([`status_command`]).
fn status_output(
    entry: &AgentEntry,
    bin: &Path,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> Option<Option<(Option<i32>, String, String)>> {
    entry.sign_in.status_args?;
    let mut clean = env.clone();
    for name in entry.billing.plan_overrides {
        clean.remove(*name);
    }
    // Asked only when the check needs a version.
    let version = entry
        .sign_in
        .status_min_version
        .and_then(|_| run_with_deadline(command_with(bin, &["--version"], &clean, cwd), STATUS_TIMEOUT))
        .filter(|(status, _, _)| *status == Some(0))
        .and_then(|(_, stdout, _)| read_version(&stdout));
    let path = clean
        .get("PATH")
        .or_else(|| clean.get("Path"))
        .cloned()
        .unwrap_or_default();
    let lookup = |name: &str| clean.get(name).cloned();
    let command = status_command(entry, version.as_deref(), &path, &lookup, |args| {
        command_with(bin, args, &clean, cwd)
    })?;
    Some(run_until(
        command,
        entry.sign_in.status_timeout,
        entry.sign_in.status_sign_in_prompt,
    ))
}

/// Antigravity's sign-in and, on the plan, what is left of it, from its model-free `/usage` at `bin`, run as its
/// sign-in check is (no browser, nothing that bills elsewhere): one run for both. An API-key setting answers without a
/// run. `None` when the check is not run (an agy older than 1.1.11, Windows).
pub fn antigravity_usage(bin: &Path, env: &HashMap<String, String>, cwd: &Path) -> Option<(Login, Vec<QuotaGroup>)> {
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE"));
    if let Some(login) = settled_login(Agent::Antigravity, home.map(String::as_str)) {
        return Some((login, Vec::new()));
    }
    let output = status_output(Agent::Antigravity.entry(), bin, env, cwd)?;
    let (login, _) = read_check(Agent::Antigravity, output.as_ref());
    let groups = output
        .as_ref()
        .filter(|_| login == Login::Plan)
        .and_then(|(_, stdout, _)| read_antigravity_usage(stdout))
        .unwrap_or_default();
    Some((login, groups))
}

/// How long `agy models` may take. It asks Google: about 2.5 s on 2026-11-16.
const ANTIGRAVITY_MODELS_TIMEOUT: Duration = Duration::from_secs(10);

/// The first agy whose `models` prints only the list on stdout (its changelog, 1.1.12). An older one prints its
/// progress there too, and one older than 1.0.5 has no `models` command, so it would take the word as a prompt.
const ANTIGRAVITY_MODELS_SINCE: (u64, u64, u64) = (1, 1, 12);

/// agy's models as `(id, label)`, from `agy models` at `bin`, run as its sign-in check is: no browser, nothing that
/// bills elsewhere, and stopped at a sign-in prompt. The command sends no prompt and runs no turn. `version` is
/// `--version`'s output. `None` when the models are not listed: an agy older than 1.1.12 (or of unknown version),
/// Windows, signed out, or no answer in time.
pub fn antigravity_models(
    bin: &Path,
    version: Option<&str>,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> Option<Vec<(String, String)>> {
    if version
        .and_then(read_version)
        .is_none_or(|version| older_than(&version, ANTIGRAVITY_MODELS_SINCE))
    {
        return None;
    }
    // Set to bill an API key: no session starts, so no models are listed.
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE"));
    if settled_login(Agent::Antigravity, home.map(String::as_str)).is_some() {
        return None;
    }
    let entry = Agent::Antigravity.entry();
    let mut clean = env.clone();
    for name in entry.billing.plan_overrides {
        clean.remove(*name);
    }
    let path = clean
        .get("PATH")
        .or_else(|| clean.get("Path"))
        .cloned()
        .unwrap_or_default();
    let lookup = |name: &str| clean.get(name).cloned();
    let mut command = command_with(bin, &["models"], &clean, cwd);
    without_browser(&mut command, &path, &lookup)?;
    let (status, stdout, _) = run_until(command, ANTIGRAVITY_MODELS_TIMEOUT, entry.sign_in.status_sign_in_prompt)?;
    if status != Some(0) {
        return None;
    }
    read_antigravity_models(&stdout)
}

/// `agy models`' lines, `id<TAB>label`; `None` when it lists none.
pub fn read_antigravity_models(stdout: &str) -> Option<Vec<(String, String)>> {
    let models: Vec<(String, String)> = stdout
        .lines()
        .filter_map(|line| {
            let (id, label) = line.split_once('\t')?;
            let id = id.trim();
            let valid = !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
            let label = label.trim();
            valid.then(|| (id.to_string(), if label.is_empty() { id } else { label }.to_string()))
        })
        .collect();
    (!models.is_empty()).then_some(models)
}

/// What this machine has of `agent`, found at `bin` by the caller (`localAgent` in access.ts).
pub fn local_with(agent: Agent, bin: Option<&Path>, env: &HashMap<String, String>, cwd: &Path) -> LocalAgent {
    let Some(bin) = bin else {
        return LocalAgent {
            agent: agent.as_str().into(),
            installed: false,
            path: None,
            version: None,
            login: None,
            plan: None,
        };
    };
    let version = run_with_deadline(command_with(bin, &["--version"], env, cwd), STATUS_TIMEOUT)
        .filter(|(status, _, _)| *status == Some(0))
        .and_then(|(_, stdout, _)| read_version(&stdout));
    let (login, plan) = match login_with(agent, bin, env, cwd) {
        None => (None, None),
        Some((login, plan, _)) => (Some(login), plan),
    };
    LocalAgent {
        agent: agent.as_str().into(),
        installed: true,
        path: Some(bin.to_string_lossy().into_owned()),
        version,
        login,
        plan,
    }
}

/// What this machine has of each agent Nexus can launch. An agent missing from `PATH` and the common folders is looked for once more
/// on the login shell's `PATH`, for an app that started without one.
pub fn detect_all(env: &dyn Fn(&str) -> Option<String>) -> Vec<LocalAgent> {
    let mut dirs = search_path(env);
    let launchers: Vec<Agent> = Agent::ALL
        .into_iter()
        .filter(|agent| agent.entry().session.cli_launcher)
        .collect();
    let mut agents: Vec<LocalAgent> = launchers.iter().map(|&agent| detect_in(agent, &dirs, env)).collect();
    if agents.iter().any(|agent| !agent.installed)
        && let Some(shell_path) = login_shell_path(env)
    {
        let before = dirs.len();
        for dir in std::env::split_paths(&shell_path) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        if dirs.len() > before {
            for (local, agent) in agents.iter_mut().zip(launchers).filter(|(local, _)| !local.installed) {
                *local = detect_in(agent, &dirs, env);
            }
        }
    }
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn login_path_survives_profile_noise_and_inherited_stdout() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir)?;
        let shell = dir.join("shell");
        crate::output::write_fake_script(
            &shell,
            "#!/bin/sh\nprintf 'welcome\\n__NEXUS_PATH__/custom/bin__NEXUS_PATH__\\n'\nsleep 4 &\n",
        );
        assert_eq!(
            login_shell_path(&|_| Some(shell.display().to_string())).as_deref(),
            Some("/custom/bin")
        );
        std::fs::remove_dir_all(dir)
    }
    #[test]
    fn reads_claude_codes_sign_in_and_never_the_account() {
        let plan = r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"team","email":"ada@example.com","orgId":"o"}"#;
        assert_eq!(read_claude_status(plan), (Login::Plan, Some("team".into())));
        assert_eq!(
            read_claude_status(r#"{"loggedIn":true,"authMethod":"console"}"#),
            (Login::ApiKey, None)
        );
        assert_eq!(
            read_claude_status(r#"{"loggedIn":false,"authMethod":"none"}"#),
            (Login::SignedOut, None)
        );
        assert_eq!(
            read_claude_status(r#"{"loggedIn":true,"authMethod":"oauth_token"}"#),
            (Login::Plan, None)
        );
        assert_eq!(
            read_claude_status("error: unknown command 'auth'"),
            (Login::Unknown, None)
        );
        assert_eq!(read_claude_account(plan).as_deref(), Some("ada@example.com"));
        assert_eq!(read_claude_account(r#"{"loggedIn":false,"authMethod":"none"}"#), None);
        assert_eq!(read_claude_account("not json"), None);
    }

    #[test]
    fn reads_claude_plan_with_or_without_the_sign_in_method() {
        use serde_json::json;

        for (account, expected) in [
            (
                json!({"authMethod": "claude.ai", "subscriptionType": "team"}),
                Some("team"),
            ),
            (
                json!({"authMethod": "oauth_token", "subscriptionType": "max"}),
                Some("max"),
            ),
            (json!({"authMethod": "console", "subscriptionType": "team"}), None),
            (json!({"authMethod": null, "subscriptionType": "team"}), None),
            (json!({"subscriptionType": "Claude Max"}), Some("Claude Max")),
            (json!({"subscriptionType": ""}), None),
        ] {
            assert_eq!(claude_plan(&account).as_deref(), expected, "{account}");
        }
    }

    #[test]
    fn reads_codexs_sign_in() {
        assert_eq!(read_codex_status("Logged in using ChatGPT"), Login::Plan);
        assert_eq!(
            read_codex_status("Logged in using an API key - sk-proj-***abcd"),
            Login::ApiKey
        );
        assert_eq!(read_codex_status("Not logged in"), Login::SignedOut);
        assert_eq!(
            read_codex_status("error: unrecognized subcommand 'status'"),
            Login::Unknown
        );
    }

    #[test]
    fn reads_versions() {
        assert_eq!(read_version("2.1.303 (Claude Code)\n").as_deref(), Some("2.1.303"));
        assert_eq!(read_version("codex-cli 0.163.0").as_deref(), Some("0.163.0"));
        assert_eq!(read_version("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(read_version("no version here"), None);
        // As the TypeScript regex reads them.
        assert_eq!(read_version("Claude Code (build 2.1.303)").as_deref(), Some("2.1.303"));
        assert_eq!(read_version("claude/2.1.303 linux-x64").as_deref(), Some("2.1.303"));
        assert_eq!(read_version("1.2.3-beta+build").as_deref(), Some("1.2.3-beta"));
        assert_eq!(read_version("12 apples, 3. 4.5").as_deref(), Some("4.5"));
        assert_eq!(
            read_marked_path("Welcome!\n__NEXUS_PATH__/a/bin:/usr/bin__NEXUS_PATH__\nprofile finished").as_deref(),
            Some("/a/bin:/usr/bin")
        );
        assert_eq!(read_marked_path("no marks"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_grandchild_that_keeps_the_output_open_never_holds_up_the_caller() {
        let mut command = Command::new("/bin/sh");
        // The shell exits at once; the `sleep` it leaves behind holds stdout far longer than the deadline.
        command.args(["-c", "sleep 30 & echo started"]);
        let started = Instant::now();
        assert_eq!(run_with_deadline(command, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn looks_in_the_common_folders_too_and_finds_an_executable_agent() {
        let dir = std::env::temp_dir().join(format!("nexus-agents-{}", uuid::Uuid::new_v4()));
        let bin = dir.join(".local/bin");
        std::fs::create_dir_all(&bin).expect("dir");
        // An empty folder as PATH, not /usr/bin: a machine with Claude Code installed system-wide has /usr/bin/claude.
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).expect("dir");
        let path = empty.to_string_lossy().into_owned();
        let home = dir.to_string_lossy().into_owned();
        let env = |name: &str| match name {
            "PATH" => Some(path.clone()),
            "HOME" => Some(home.clone()),
            _ => None,
        };
        let dirs = search_path(&env);
        assert_eq!(dirs[0], empty);
        assert!(dirs.contains(&bin), "{dirs:?}");
        // Only this test's folders: the common ones also include /usr/local/bin, which may hold a real agent.
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| d.starts_with(&dir)).collect();
        assert!(!detect_in(Agent::Claude, &dirs, &env).installed);
        #[cfg(unix)]
        {
            let claude = bin.join("claude");
            write_script(
                &claude,
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '9.9.9 (Claude Code)'; else echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\",\"email\":\"ada@example.com\"}'; fi\n",
            );
            let found = detect_in(Agent::Claude, &dirs, &env);
            assert_eq!(
                (
                    found.installed,
                    found.version.as_deref(),
                    found.login,
                    found.plan.as_deref()
                ),
                (true, Some("9.9.9"), Some(Login::Plan), Some("max"))
            );
            let json = serde_json::to_string(&found).expect("json");
            assert!(!json.contains("ada@example.com"), "{json}");
            assert!(json.contains("\"login\":\"plan\""), "{json}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn entry_without_a_sign_in_check_runs_nothing() {
        let dir = std::env::temp_dir().join(format!("nexus-unchecked-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let ran = dir.join("ran");
        let bin = dir.join("agy");
        crate::output::write_fake_script(&bin, &format!("#!/bin/sh\ntouch '{}'\n", ran.display()));
        let claude = Agent::Claude.entry();
        let unchecked = AgentEntry {
            sign_in: nexus_agents::SignIn {
                status_args: None,
                ..claude.sign_in
            },
            ..*claude
        };
        assert_eq!(status_output(&unchecked, &bin, &HashMap::new(), &dir), None);
        assert!(!ran.exists(), "the agent ran for a sign-in check");
        // The same program behind an entry with a check does run.
        assert!(status_output(Agent::Claude.entry(), &bin, &HashMap::new(), &dir).is_some());
        assert!(ran.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const USAGE: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.8-usage.json");
    /// `/usage` on agy 1.5.11 signed in through Google Cloud (a Gemini Enterprise license), as recorded on 2026-11-18.
    const CLOUD_USAGE: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.11-usage-google-cloud.json");

    #[test]
    fn google_cloud_sign_in_answers_placeholder_quota() {
        let groups = read_antigravity_usage(CLOUD_USAGE).expect("an answer");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "All Models");
        assert_eq!(groups[0].buckets.len(), 28);
        assert!(groups[0].buckets.iter().all(|bucket| bucket.remaining == 1.0
            && bucket.window.is_empty()
            && bucket.resets_at.as_deref() == Some("1970-01-01T00:00:00Z")));
    }

    #[test]
    fn reads_the_project_of_agys_last_google_cloud_sign_in() {
        let home = std::env::temp_dir().join(format!("nexus-agy-cloud-{}", uuid::Uuid::new_v4()));
        let settings = antigravity_settings(&home);
        std::fs::create_dir_all(settings.parent().expect("dir")).expect("dir");
        let at = home.to_string_lossy().into_owned();
        // A personal sign-in: no project.
        std::fs::write(&settings, r#"{"trustedWorkspaces": []}"#).expect("write");
        assert_eq!(antigravity_cloud_project(Some(&at)), None);
        std::fs::write(
            &settings,
            r#"{"gcp": {"project": "ai-tools-example", "location": "global"}}"#,
        )
        .expect("write");
        assert_eq!(
            antigravity_cloud_project(Some(&at)).as_deref(),
            Some("ai-tools-example")
        );
        std::fs::write(&settings, r#"{"gcp": {"project": " "}}"#).expect("write");
        assert_eq!(antigravity_cloud_project(Some(&at)), None);
        assert_eq!(antigravity_cloud_project(None), None);
        let _ = std::fs::remove_dir_all(&home);
    }
    const SIGNED_OUT_STDOUT: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.8-usage-signed-out.stdout");
    const SIGNED_OUT_STDERR: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.8-usage-signed-out.stderr");
    /// `agy models` on agy 1.5.8, as recorded on 2026-11-16.
    const MODELS: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.8-models.stdout");

    #[test]
    fn reads_agys_model_list() {
        let models = read_antigravity_models(MODELS).expect("models");
        assert_eq!(models.len(), 6);
        assert_eq!(
            models[0],
            (
                "gemini-3.8-flash-high".to_string(),
                "Gemini 3.8 Flash (High)".to_string()
            )
        );
        assert_eq!(models[5].0, "gpt-oss-120b-medium");
        // An older agy's progress on stdout, a blank line, an id with a space: not models.
        assert_eq!(
            read_antigravity_models("Fetching available models...\n\nbad id\tx\nm-1\t\n"),
            Some(vec![("m-1".to_string(), "m-1".to_string())])
        );
        assert_eq!(read_antigravity_models("Fetching available models...\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn agy_models_run_without_a_browser_and_only_on_agy_1_1_12() {
        let home = std::env::temp_dir().join(format!("nexus-agy-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("dir");
        let (seen, signed_out, models) = (home.join("seen"), home.join("signed-out"), home.join("models.txt"));
        let stderr = home.join("prompt.txt");
        std::fs::write(&models, MODELS).expect("write");
        std::fs::write(&stderr, SIGNED_OUT_STDERR).expect("write");
        let agy = home.join("agy");
        write_script(
            &agy,
            &format!(
                "#!/bin/sh\necho \"$(command -v xdg-open) ${{DISPLAY:-none}} ${{GEMINI_API_KEY:-none}} $*\" > '{seen}'\n\
                 if [ -f '{signed_out}' ]; then /bin/cat '{stderr}' >&2; /bin/sleep 30; fi\n/bin/cat '{models}'\n",
                seen = seen.display(),
                signed_out = signed_out.display(),
                stderr = stderr.display(),
                models = models.display(),
            ),
        );
        let env = HashMap::from([
            ("HOME".to_string(), home.to_string_lossy().into_owned()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("DISPLAY".to_string(), ":9".to_string()),
            ("GEMINI_API_KEY".to_string(), "key".to_string()),
        ]);
        let shim = home.join(".cache/nexus/no-browser/xdg-open");

        let listed = antigravity_models(&agy, Some("1.5.8"), &env, &home).expect("listed");
        assert_eq!(listed.len(), 6);
        let ran = std::fs::read_to_string(&seen).unwrap_or_default();
        assert_eq!(ran.trim(), format!("{} none none models", shim.display()));

        // Signed out: stopped at the prompt, nothing listed.
        std::fs::write(&signed_out, "").expect("write");
        let started = Instant::now();
        assert_eq!(antigravity_models(&agy, Some("1.5.8"), &env, &home), None);
        assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
        std::fs::remove_file(&signed_out).expect("remove");

        // Set to an API key: not run.
        let settings = antigravity_settings(&home);
        std::fs::create_dir_all(settings.parent().unwrap_or(&home)).expect("dir");
        std::fs::write(&settings, r#"{"modelProvider":"gemini"}"#).expect("write");
        std::fs::remove_file(&seen).expect("remove");
        assert_eq!(antigravity_models(&agy, Some("1.5.8"), &env, &home), None);
        assert!(!seen.exists(), "agy models ran despite an API key");
        std::fs::remove_file(&settings).expect("remove");
        std::fs::write(&seen, "").expect("write");

        // Older than 1.1.12, a pre-release of it, or of unknown version: never run.
        std::fs::remove_file(&seen).expect("remove");
        assert_eq!(antigravity_models(&agy, Some("1.1.11"), &env, &home), None);
        assert_eq!(antigravity_models(&agy, Some("1.1.12-rc.1"), &env, &home), None);
        assert_eq!(antigravity_models(&agy, None, &env, &home), None);
        assert!(!seen.exists(), "agy models ran on an agy older than 1.1.12");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A stand-in agent at `path`, mode 755, written by a shell rather than by this process. Tests run in parallel: a
    /// test that spawns between this process's write and its close hands the open file to its child until the child
    /// execs, and running the script meanwhile fails with "Text file busy".
    #[cfg(unix)]
    fn write_script(path: &Path, body: &str) {
        let written = Command::new("/bin/sh")
            .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
            .arg(path)
            .arg(body)
            .status()
            .map(|status| status.success());
        assert!(matches!(written, Ok(true)), "could not write {}", path.display());
    }

    /// agy 1.5.8's `/usage`, as recorded on 2026-11-16: two groups, each with one weekly limit. Only the command's own
    /// answer counts; a model's turn, an error or a sign-in that did not finish is no sign-in.
    #[test]
    fn reads_antigravitys_quota_and_sign_in() {
        let groups = read_antigravity_usage(USAGE).unwrap_or_default();
        let named: Vec<(&str, &[String])> = groups
            .iter()
            .map(|group| (group.name.as_str(), &group.models[..]))
            .collect();
        assert_eq!(named.len(), 2, "{groups:?}");
        assert_eq!(named[0].0, "Gemini Models");
        assert_eq!(named[0].1, ["Gemini 3.8 Flash"]);
        assert_eq!(named[1].1, ["Claude Sonnet 4.6", "Claude Opus 4.6", "GPT-OSS 120B"]);
        let gemini = &groups[0].buckets[0];
        assert_eq!(
            (gemini.name.as_str(), gemini.window.as_str()),
            ("Weekly Limit Remaining", "weekly")
        );
        assert!((gemini.remaining - 0.960_465_610_027_313_2).abs() < 1e-9);
        assert_eq!(gemini.resets_at.as_deref(), Some("2026-11-23T16:28:00Z"));
        assert_eq!(read_antigravity_status(USAGE), Login::Plan);

        assert_eq!(read_antigravity_usage(SIGNED_OUT_STDOUT), None);
        assert_eq!(read_antigravity_status(SIGNED_OUT_STDOUT), Login::Unknown);
        let turn = r#"{"conversation_id":"c1","status":"SUCCESS","response":"Your quota is fine.","num_turns":1}"#;
        assert_eq!(read_antigravity_usage(turn), None);
        assert_eq!(read_antigravity_status("not json"), Login::Unknown);
        // The prompt on stderr is a sign-out, whatever stdout says.
        let prompted = (None, SIGNED_OUT_STDOUT.to_string(), SIGNED_OUT_STDERR.to_string());
        assert_eq!(
            read_check(Agent::Antigravity, Some(&prompted)),
            (Login::SignedOut, None)
        );

        assert_eq!(
            quota_group_of(&groups, "gemini-3.8-flash").map(|group| group.name.as_str()),
            Some("Gemini Models")
        );
        for model in ["claude-opus-4-6-thinking", "claude-sonnet-4-6", "gpt-oss-120b-medium"] {
            assert_eq!(
                quota_group_of(&groups, model).map(|group| group.name.as_str()),
                Some("Claude and GPT models"),
                "{model}"
            );
        }
        assert_eq!(quota_group_of(&groups, "gemini-4-hydrogen"), None);
        assert_eq!(quota_group_of(&groups, ""), None);
        assert_eq!(version_triple("1.5.8"), Some((1, 5, 8)));
        assert_eq!(version_triple("1.1.11-beta"), Some((1, 1, 11)));
        assert_eq!(version_triple("latest"), None);
        // A pre-release of the first version with `/usage` may predate it.
        assert!(older_than("1.1.11-beta", (1, 1, 11)));
        assert!(!older_than("1.1.11", (1, 1, 11)));
        assert!(!older_than("1.1.12-beta", (1, 1, 11)));
        assert!(!older_than("1.1.11+build", (1, 1, 11)));
        assert!(older_than("latest", (1, 1, 11)));
        // No `PATH`: the launchers' folder only, never the current folder (an empty entry).
        let home = std::env::temp_dir().join(format!("nexus-agy-path-{}", uuid::Uuid::new_v4()));
        let home_text = home.to_string_lossy().into_owned();
        let mut command = Command::new("agy");
        assert_eq!(
            without_browser(&mut command, "", &|name: &str| (name == "HOME")
                .then(|| home_text.clone())),
            Some(())
        );
        let path = command
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(
            path,
            Some(home.join(".cache/nexus/no-browser").to_string_lossy().into_owned())
        );
        let _ = std::fs::remove_dir_all(&home);
        // A later line that is not the answer does not hide it.
        assert_eq!(
            read_antigravity_usage(&format!("{USAGE}\n0\n")).map(|groups| groups.len()),
            Some(2)
        );
    }

    /// Antigravity's sign-in, through both finders, against a stand-in `agy` that answers as agy 1.5.8 did and records
    /// how its check ran. An API-key setting decides without a check. A plan answers `/usage` with quota. Signed out,
    /// the check stops at agy's sign-in prompt instead of waiting for the sign-in, and its `xdg-open` is the no-op
    /// launcher, with no display. An agy older than 1.1.11, which would send `/usage` to the model, is never checked.
    #[cfg(unix)]
    #[test]
    fn antigravity_check_reads_usage_and_never_opens_a_browser() {
        let home = std::env::temp_dir().join(format!("nexus-agy-{}", uuid::Uuid::new_v4()));
        let settings = antigravity_settings(&home);
        assert_eq!(settings, home.join(".gemini/antigravity-cli/settings.json"));
        let home_text = home.to_string_lossy().into_owned();
        assert_eq!(antigravity_login(Some(&home_text)), None);
        assert_eq!(antigravity_login(None), None);
        std::fs::create_dir_all(settings.parent().unwrap_or(&home)).expect("dir");
        std::fs::write(&settings, "not json").expect("write");
        assert_eq!(antigravity_login(Some(&home_text)), None);

        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).expect("dir");
        let (seen, signed_out, version) = (home.join("seen"), home.join("signed-out"), home.join("version"));
        let (usage, stderr) = (home.join("usage.json"), home.join("prompt.txt"));
        std::fs::write(&usage, USAGE).expect("write");
        std::fs::write(&stderr, SIGNED_OUT_STDERR).expect("write");
        std::fs::write(&version, "1.5.8").expect("write");
        let agy = bin.join("agy");
        write_script(
            &agy,
            &format!(
                // Absolute paths: a finder's `PATH` may hold only this folder.
                "#!/bin/sh\nif [ \"$1\" = --version ]; then /bin/cat '{version}'; exit 0; fi\n\
                 echo \"$(command -v xdg-open) ${{DISPLAY:-none}} $*\" > '{seen}'\n\
                 if [ -f '{signed_out}' ]; then /bin/cat '{stderr}' >&2; /bin/sleep 30; fi\n/bin/cat '{usage}'\n",
                version = version.display(),
                seen = seen.display(),
                signed_out = signed_out.display(),
                stderr = stderr.display(),
                usage = usage.display(),
            ),
        );
        let env = HashMap::from([
            ("HOME".to_string(), home_text.clone()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("DISPLAY".to_string(), ":9".to_string()),
        ]);
        let lookup = |name: &str| env.get(name).cloned();
        let shim = home.join(".cache/nexus/no-browser/xdg-open");

        // An API-key setting decides, and agy's check does not run.
        std::fs::write(&settings, r#"{"modelProvider":"gemini"}"#).expect("write");
        assert_eq!(
            login_with(Agent::Antigravity, &agy, &env, &home),
            Some((Login::ApiKey, None, None))
        );
        assert_eq!(
            detect_in(Agent::Antigravity, std::slice::from_ref(&bin), &lookup).login,
            Some(Login::ApiKey)
        );
        assert!(!seen.exists(), "agy's check ran despite an API key");
        std::fs::remove_file(&settings).expect("remove");

        // Signed in to the plan.
        assert_eq!(
            login_with(Agent::Antigravity, &agy, &env, &home),
            Some((Login::Plan, None, None))
        );
        let ran = std::fs::read_to_string(&seen).unwrap_or_default();
        assert_eq!(
            ran.trim(),
            format!("{} none -p /usage --output-format json", shim.display())
        );
        assert_eq!(
            detect_in(Agent::Antigravity, std::slice::from_ref(&bin), &lookup).login,
            Some(Login::Plan)
        );
        assert_eq!(
            antigravity_usage(&agy, &env, &home).map(|(login, groups)| (login, groups.len())),
            Some((Login::Plan, 2))
        );

        // Signed out: stopped at the prompt, not after agy's wait.
        std::fs::write(&signed_out, "").expect("write");
        let started = Instant::now();
        assert_eq!(
            login_with(Agent::Antigravity, &agy, &env, &home),
            Some((Login::SignedOut, None, None))
        );
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(
            antigravity_usage(&agy, &env, &home),
            Some((Login::SignedOut, Vec::new()))
        );
        std::fs::remove_file(&signed_out).expect("remove");

        // Older than 1.1.11: never run with `-p`.
        std::fs::remove_file(&seen).expect("remove");
        std::fs::write(&version, "1.1.10").expect("write");
        assert_eq!(login_with(Agent::Antigravity, &agy, &env, &home), None);
        assert_eq!(
            detect_in(Agent::Antigravity, std::slice::from_ref(&bin), &lookup).login,
            None
        );
        assert!(!seen.exists(), "an agy older than 1.1.11 ran /usage");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn finds_an_entry_by_its_executable_not_its_id() {
        let dir = std::env::temp_dir().join(format!("nexus-executable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        for name in ["claude", "agy"] {
            std::fs::write(dir.join(name), "#!/bin/sh\n").expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755)).expect("chmod");
            }
        }
        let claude = Agent::Claude.entry();
        let entry = AgentEntry {
            identity: nexus_agents::Identity {
                executable: "agy",
                ..claude.identity
            },
            ..*claude
        };
        let dirs = [dir.clone()];
        // Windows looks for `agy.EXE` and the like.
        #[cfg(unix)]
        assert_eq!(find_entry_in(&entry, &dirs, &|_| None), Some(dir.join("agy")));
        // `NEXUS_<AGENT>_BIN` is keyed by the id and still wins.
        let claude = dir.join("claude").to_string_lossy().into_owned();
        let named = |name: &str| (name == "NEXUS_CLAUDE_BIN").then(|| claude.clone());
        assert_eq!(find_entry_in(&entry, &dirs, &named), Some(dir.join("claude")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
