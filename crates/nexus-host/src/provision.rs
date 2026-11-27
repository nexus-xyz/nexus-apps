//! `host_provision`: a cloud machine's setup, from the desktop app, once
//! per connection through Gateway. The arguments are [`HostProvision`]; nexus-workbench's `provision` says what each
//! one means. This is the SSH start's preparation script in Rust, without what the machine
//! image now does itself (Playwright, linger, the pre-stop hook):
//!
//! - `agent.env` and `mcp.json` in the host's folder, 0600, written whole (a temporary file, then a rename). These are
//!   the files [`crate::agent_files`] reads.
//! - The app's block in each agent's instructions file (nexus-workbench's `cloud_agent`: Claude Code's
//!   `~/.claude/CLAUDE.md`, Codex's `$CODEX_HOME/AGENTS.md` or `~/.codex`), between [`BLOCK_BEGIN`] and
//!   [`BLOCK_END`]. The rest of the file is the person's and stays. A symlinked file is written through to its target,
//!   which keeps its mode. Under a lock on `<folder>/.nexus-md.lock`.
//! - git's global name and email; `gh auth login --with-token` and `gh auth setup-git` when `gh` is there and its
//!   token differs. The token goes on standard input, never the command line.
//! - Each agent's first-run marks (nexus-workbench's `cloud_agent`: Claude Code's `hasCompletedOnboarding` and
//!   `autoUpdates: false` in `~/.claude.json`), so the agent starts without its welcome.
//!
//! Running it twice changes nothing more. It fails only when one of the two files cannot be written: a machine without
//! its Claude token would start sessions that cannot sign in. Every other step is best effort, and the answer says
//! how each went. Nothing is installed.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nexus_workbench::cloud_agent::{FirstRun, InstructionsFile, first_run_files, instructions_files};
use nexus_workbench::provision::{BLOCK_BEGIN, BLOCK_END, HostProvision};
use serde_json::{Map, Value, json};

/// How long one `git` or `gh` command may take.
const COMMAND_LIMIT: Duration = Duration::from_secs(60);

/// Where the setup goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Places {
    /// The host's folder (`~/.nexus/host`): `agent.env` and `mcp.json`.
    pub root: PathBuf,
    pub home: PathBuf,
    /// Each agent's instructions file and the folder it is in, in the order the report lists them.
    pub instructions: Vec<(InstructionsFile, PathBuf)>,
}

impl Places {
    /// The places with this process's environment.
    pub fn new(root: PathBuf, home: PathBuf) -> Places {
        Places::with_vars(root, home, |var| std::env::var_os(var))
    }

    /// The places with `var` as the environment: each instructions file's folder is its variable when that is set and
    /// not empty, else its folder under `home`.
    pub fn with_vars(root: PathBuf, home: PathBuf, var: impl Fn(&str) -> Option<OsString>) -> Places {
        let instructions = instructions_files()
            .into_iter()
            .map(|file| {
                let folder = file
                    .var
                    .and_then(&var)
                    .filter(|dir| !dir.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(file.dir));
                (file, folder)
            })
            .collect();
        Places {
            root,
            home,
            instructions,
        }
    }
}

/// Applies `provision` at `places`: `{ "agentEnv", "mcp", "claudeMd", "codexMd", "gitName", "gitEmail", "github",
/// "claude" }`, each step's outcome as PREPARE reported it (`written`, `kept`, `none`, `removed`, `set`, `already`,
/// `failed`…). Blocks on files and commands: run it off the async threads.
pub fn apply(places: &Places, provision: &HostProvision) -> Result<Value, String> {
    let mut report = Map::new();
    let mut said = |key: &str, outcome: &str| {
        report.insert(key.into(), Value::String(outcome.into()));
    };
    std::fs::create_dir_all(&places.root).map_err(|e| format!("Could not make {}: {e}", places.root.display()))?;
    let _ = std::fs::set_permissions(&places.root, std::fs::Permissions::from_mode(0o700));

    // agent.env: null keeps it, an empty text removes it. mcp.json: null (or empty) removes it.
    let agent_env = match provision.agent_env.as_deref() {
        None => Kept,
        Some("") => Remove,
        Some(text) => Write(text),
    };
    said("agentEnv", file(&places.root.join("agent.env"), agent_env)?);
    let mcp = match provision.mcp.as_deref() {
        None | Some("") => Remove,
        Some(text) => Write(text),
    };
    said("mcp", file(&places.root.join("mcp.json"), mcp)?);

    let block = provision
        .instructions
        .as_deref()
        .map(|text| text.trim_end_matches('\n'))
        .filter(|text| !text.trim().is_empty());
    for (file, folder) in &places.instructions {
        said(file.report, agent_md_in(folder, file.name, block));
    }

    if let Some(github) = &provision.github {
        let line = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty() && !v.chars().any(char::is_control))
                .map(str::to_string)
        };
        if let Some(name) = line(&github.name) {
            said("gitName", git_config(places, "user.name", &name));
        }
        if let Some(email) = line(&github.email) {
            said("gitEmail", git_config(places, "user.email", &email));
        }
        let token = line(&github.token).filter(|token| !token.chars().any(char::is_whitespace));
        if let Some(token) = token {
            let (github, setup_git) = gh_login(places, &token);
            said("github", github);
            if let Some(setup_git) = setup_git {
                said("setupGit", setup_git);
            }
        }
    }
    for first_run in first_run_files() {
        said(first_run.report, first_run_marks(&places.home, &first_run));
    }
    Ok(Value::Object(report))
}

use Change::{Kept, Remove, Write};

/// What becomes of one of the host's files.
enum Change<'a> {
    Kept,
    Remove,
    Write(&'a str),
}

/// One of the host's files, 0600 and written whole. An error only when it cannot be written or removed.
fn file(path: &Path, change: Change) -> Result<&'static str, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("the file");
    match change {
        Kept if path.is_file() => Ok("kept"),
        Kept => Ok("none"),
        Remove => match std::fs::remove_file(path) {
            Ok(()) => Ok("removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("none"),
            Err(e) => Err(format!("Could not remove {name}: {e}")),
        },
        Write(text) => {
            replace(path, text.as_bytes(), 0o600).map_err(|e| format!("Could not write {name}: {e}"))?;
            Ok("written")
        }
    }
}

/// Writes `bytes` to `path` through a temporary file beside it with `mode`, then renames it over.
fn replace(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let part = part_path(path);
    let written = (|| {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&part)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// `<dir>/.<name>.nexus-<pid>-<n>`: a temporary file of this call's own.
fn part_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.nexus-{}-{n}", std::process::id()))
}

/// The app's block in `<folder>/<name>`, under the folder's lock (`.nexus-md.lock`).
fn agent_md_in(folder: &Path, name: &str, block: Option<&str>) -> &'static str {
    if std::fs::create_dir_all(folder).is_err() {
        return "failed";
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(folder.join(".nexus-md.lock"));
    let Ok(lock) = lock else {
        return "failed";
    };
    let Ok(_held) = nix::fcntl::Flock::lock(lock, nix::fcntl::FlockArg::LockExclusive) else {
        return "failed";
    };
    agent_md(&folder.join(name), block)
}

/// The app's block in the instructions file at `link`: the old one goes, then `block` goes last. None only removes.
fn agent_md(link: &Path, block: Option<&str>) -> &'static str {
    let symlink = link.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink());
    let md = if symlink {
        match std::fs::canonicalize(link) {
            Ok(target) => target,
            // A dangling link: its target is made.
            Err(_) => match std::fs::read_link(link) {
                Ok(target) if target.is_absolute() => target,
                Ok(target) => link.parent().unwrap_or(Path::new("/")).join(target),
                Err(_) => return "failed",
            },
        }
    } else {
        link.to_path_buf()
    };
    let text = match std::fs::read(&md) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return "failed",
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return "failed",
    };
    if block.is_none() && !text.as_deref().is_some_and(|text| text.contains(BLOCK_BEGIN)) {
        return "none";
    }
    let mut lines = without_block(text.as_deref().unwrap_or(""), BLOCK_BEGIN, BLOCK_END);
    if let Some(block) = block {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(BLOCK_BEGIN.into());
        lines.extend(block.split('\n').map(String::from));
        lines.push(BLOCK_END.into());
    }
    if lines.is_empty() {
        // Nothing of the person's was left, and no block to add: the file only ever held ours. A symlink's target is
        // emptied, not deleted, so the link stays whole.
        let gone = if symlink {
            File::create(&md).map(|_| ())
        } else {
            std::fs::remove_file(&md)
        };
        return match gone {
            Ok(()) => "removed",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "removed",
            Err(_) => "failed",
        };
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let mode = std::fs::metadata(&md)
        .map(|meta| meta.permissions().mode() & 0o7777)
        .unwrap_or(0o600);
    match replace(&md, out.as_bytes(), mode) {
        Ok(()) => "written",
        Err(_) => "failed",
    }
}

/// `text`'s lines without the app's block, as PREPARE's awk keeps them: a line that starts with `begin` opens a block,
/// one that starts with `end` closes it, and a blank line right after a removed block goes too. A begin line with no
/// end before the next begin or the end of the file is the person's text and stays. Blank lines at the end go.
pub fn without_block(text: &str, begin: &str, end: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut hold: Vec<&str> = Vec::new();
    let (mut inside, mut gone) = (false, false);
    let mut all: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') || text.is_empty() {
        all.pop();
    }
    let blank = |line: &str| line.chars().all(|c| c == ' ' || c == '\t');
    for line in all {
        if line.starts_with(begin) {
            lines.extend(hold.drain(..).map(String::from));
            inside = true;
            hold.push(line);
            continue;
        }
        if inside {
            hold.push(line);
            if line.starts_with(end) {
                inside = false;
                hold.clear();
                gone = true;
            }
            continue;
        }
        if gone && blank(line) {
            gone = false;
            continue;
        }
        gone = false;
        lines.push(line.to_string());
    }
    if inside {
        lines.extend(hold.into_iter().map(String::from));
    }
    while lines.last().is_some_and(|line| blank(line)) {
        lines.pop();
    }
    lines
}

/// Runs `command` with `input` on its standard input, for at most [`COMMAND_LIMIT`]. None when it could not start;
/// else whether it succeeded and its standard output.
fn run(mut command: Command, input: Option<&str>) -> Option<(bool, String)> {
    command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut child = command.spawn().ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(input.as_bytes());
    }
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(out) = stdout.as_mut() {
            use std::io::Read as _;
            let _ = out.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < COMMAND_LIMIT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    Some((status.is_some_and(|s| s.success()), text))
}

fn git_config(places: &Places, key: &str, value: &str) -> &'static str {
    let mut git = nexus_workbench::process::command("git");
    git.args(["config", "--global", key, value]).env("HOME", &places.home);
    match run(git, None) {
        Some((true, _)) => "set",
        _ => "failed",
    }
}

/// `gh` signed in with `token`, and git using it for github.com: `no-gh`, `already`, `set` or `failed`, and
/// `failed` for `gh auth setup-git` when only that failed.
fn gh_login(places: &Places, token: &str) -> (&'static str, Option<&'static str>) {
    let gh = |args: &[&str]| {
        let mut command = nexus_workbench::process::command("gh");
        command.args(args).env("HOME", &places.home);
        command
    };
    let Some((_, current)) = run(gh(&["auth", "token", "--hostname", "github.com"]), None) else {
        return ("no-gh", None);
    };
    if current.trim() == token {
        return ("already", None);
    }
    let login = gh(&[
        "auth",
        "login",
        "--hostname",
        "github.com",
        "--git-protocol",
        "https",
        "--with-token",
    ]);
    if !matches!(run(login, Some(&format!("{token}\n"))), Some((true, _))) {
        return ("failed", None);
    }
    match run(gh(&["auth", "setup-git", "--hostname", "github.com"]), None) {
        Some((true, _)) => ("set", None),
        _ => ("set", Some("failed")),
    }
}

/// An agent's first-run marks ([`FirstRun`], from its `CloudAgent`) in `~/<file>` (through a symlink), the rest kept.
fn first_run_marks(home: &Path, first_run: &FirstRun) -> &'static str {
    let link = home.join(first_run.file);
    let path = std::fs::canonicalize(&link).unwrap_or(link);
    let mut data = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(data)) => data,
            _ => return "unreadable",
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(_) => return "unreadable",
    };
    let marks = first_run.marks;
    if marks
        .iter()
        .all(|(field, value)| data.get(*field) == Some(&json!(value)))
    {
        return "already";
    }
    for (field, value) in marks {
        data.insert((*field).into(), json!(value));
    }
    let Ok(mut text) = serde_json::to_string_pretty(&Value::Object(data)) else {
        return "failed";
    };
    text.push('\n');
    let mode = std::fs::metadata(&path)
        .map(|meta| meta.permissions().mode() & 0o7777)
        .unwrap_or(0o600);
    match replace(&path, text.as_bytes(), mode) {
        Ok(()) => "set",
        Err(_) => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::provision::Github;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-provision-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    fn places(dir: &Path) -> Places {
        Places::with_vars(dir.join(".nexus/host"), dir.to_path_buf(), |_| None)
    }

    /// Each agent's instructions file, from its `CloudAgent`, where the SSH setup writes it.
    #[test]
    fn each_agents_instructions_file() {
        let home = PathBuf::from("/home/ada");
        let folders = |places: Places| -> Vec<(&'static str, PathBuf)> {
            places
                .instructions
                .into_iter()
                .map(|(file, folder)| (file.report, folder.join(file.name)))
                .collect()
        };
        assert_eq!(
            folders(Places::with_vars(home.join(".nexus/host"), home.clone(), |_| None)),
            vec![
                ("claudeMd", home.join(".claude/CLAUDE.md")),
                ("codexMd", home.join(".codex/AGENTS.md")),
            ]
        );
        let moved = |var: &str| Some(OsString::from(format!("/moved/{var}")));
        assert_eq!(
            folders(Places::with_vars(home.join(".nexus/host"), home.clone(), moved)),
            vec![
                ("claudeMd", home.join(".claude/CLAUDE.md")),
                ("codexMd", PathBuf::from("/moved/CODEX_HOME/AGENTS.md")),
            ]
        );
        let empty = |_: &str| Some(OsString::new());
        assert_eq!(
            folders(Places::with_vars(home.join(".nexus/host"), home.clone(), empty))[1].1,
            home.join(".codex/AGENTS.md")
        );
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn writes_the_files_private_and_again_changes_nothing() {
        let dir = temp("files");
        let places = places(&dir);
        let provision = HostProvision {
            github: None,
            agent_env: Some("CLAUDE_CODE_OAUTH_TOKEN=x\n".into()),
            mcp: Some("{\"mcpServers\":{}}".into()),
            instructions: Some("# Nexus\nUse the tools.\n".into()),
        };
        let first = apply(&places, &provision).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(first["agentEnv"], "written");
        assert_eq!(first["mcp"], "written");
        assert_eq!(first["claudeMd"], "written");
        assert_eq!(first["codexMd"], "written");
        assert_eq!(first["claude"], "set");
        let env = places.root.join("agent.env");
        assert_eq!(read(&env), "CLAUDE_CODE_OAUTH_TOKEN=x\n");
        assert_eq!(mode(&env), 0o600);
        assert_eq!(mode(&places.root.join("mcp.json")), 0o600);
        // What the host reads them with takes them.
        let files = crate::agent_files::AgentFiles::new(places.root.clone());
        assert_eq!(files.agent_env(), vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), "x".into())]);
        let claude_md = dir.join(".claude/CLAUDE.md");
        let once = read(&claude_md);
        assert_eq!(once, format!("{BLOCK_BEGIN}\n# Nexus\nUse the tools.\n{BLOCK_END}\n"));
        let claude: Value = serde_json::from_str(&read(&dir.join(".claude.json"))).unwrap_or_default();
        assert_eq!(claude["hasCompletedOnboarding"], true);
        assert_eq!(claude["autoUpdates"], false);
        assert_eq!(mode(&dir.join(".claude.json")), 0o600);

        let second = apply(&places, &provision).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(second["claude"], "already");
        assert_eq!(read(&claude_md), once);
        assert_eq!(read(&dir.join(".codex/AGENTS.md")), once);

        // Null keeps agent.env, an empty text removes it; null mcp and instructions remove theirs.
        let kept = apply(&places, &HostProvision::default()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(kept["agentEnv"], "kept");
        assert_eq!(kept["mcp"], "removed");
        assert_eq!(kept["claudeMd"], "removed");
        assert!(env.is_file() && !claude_md.exists());
        let removed = apply(
            &places,
            &HostProvision {
                agent_env: Some(String::new()),
                ..HostProvision::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(removed["agentEnv"], "removed");
        assert!(!env.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_the_persons_own_instructions_and_settings() {
        let dir = temp("persons");
        let places = places(&dir);
        std::fs::create_dir_all(dir.join(".claude")).unwrap_or_else(|e| panic!("{e}"));
        let md = dir.join(".claude/CLAUDE.md");
        std::fs::write(&md, format!("Mine.\n\n{BLOCK_BEGIN}\nold\n{BLOCK_END}\n\nAlso mine.\n"))
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&md, std::fs::Permissions::from_mode(0o644)).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join(".claude.json"), "{\"theme\":\"dark\"}").unwrap_or_else(|e| panic!("{e}"));
        let provision = HostProvision {
            instructions: Some("new".into()),
            ..HostProvision::default()
        };
        apply(&places, &provision).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read(&md),
            format!("Mine.\n\nAlso mine.\n\n{BLOCK_BEGIN}\nnew\n{BLOCK_END}\n")
        );
        assert_eq!(mode(&md), 0o644);
        let claude: Value = serde_json::from_str(&read(&dir.join(".claude.json"))).unwrap_or_default();
        assert_eq!(claude["theme"], "dark");
        assert_eq!(claude["hasCompletedOnboarding"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unterminated_begin_is_the_persons_text() {
        let text = format!("a\n{BLOCK_BEGIN}\nb\n\n");
        assert_eq!(
            without_block(&text, BLOCK_BEGIN, BLOCK_END),
            vec!["a", BLOCK_BEGIN, "b"]
        );
        let two = format!("{BLOCK_BEGIN}\nx\n{BLOCK_BEGIN}\ny\n{BLOCK_END}\n\nz");
        assert_eq!(without_block(&two, BLOCK_BEGIN, BLOCK_END), vec![BLOCK_BEGIN, "x", "z"]);
        assert!(without_block("", BLOCK_BEGIN, BLOCK_END).is_empty());
    }

    #[test]
    fn a_symlinked_file_is_written_through() {
        let dir = temp("symlink");
        let places = places(&dir);
        let dotfiles = dir.join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(dir.join(".claude")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dotfiles.join("CLAUDE.md"), "Mine.\n").unwrap_or_else(|e| panic!("{e}"));
        let link = dir.join(".claude/CLAUDE.md");
        std::os::unix::fs::symlink(dotfiles.join("CLAUDE.md"), &link).unwrap_or_else(|e| panic!("{e}"));
        let provision = HostProvision {
            instructions: Some("ours".into()),
            ..HostProvision::default()
        };
        apply(&places, &provision).unwrap_or_else(|e| panic!("{e}"));
        assert!(link.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()));
        assert_eq!(
            read(&dotfiles.join("CLAUDE.md")),
            format!("Mine.\n\n{BLOCK_BEGIN}\nours\n{BLOCK_END}\n")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sets_gits_name_and_email() {
        let dir = temp("git");
        let places = places(&dir);
        let provision = HostProvision {
            github: Some(Github {
                token: None,
                name: Some("Ada Lovelace".into()),
                email: Some("ada@example.com".into()),
            }),
            ..HostProvision::default()
        };
        let report = apply(&places, &provision).unwrap_or_else(|e| panic!("{e}"));
        // No token: gh is left as it is.
        assert!(report.get("github").is_none());
        if std::env::var_os("GIT_CONFIG_GLOBAL").is_none() && report["gitName"] == "set" {
            assert!(read(&dir.join(".gitconfig")).contains("Ada Lovelace"));
            assert_eq!(report["gitEmail"], "set");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
