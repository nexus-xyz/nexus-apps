//! The agent's tools: what the model may ask for, and how each runs on this machine. Reads stay inside the workspace
//! (the folder `nexus` started in). An edit or a command runs only once the person approves it (`mod.rs` asks).
//!
//! - `read_file`, `list_dir`, `search`: read, without asking.
//! - `edit_file`, `write_file`: change a file; the person sees the diff first.
//! - `run_command`: run a shell command in the workspace; the person sees it first.
//! - `update_plan`: the agent's step list for a longer task, shown to the person as it changes.
//!
//! The agent has two more tools, which run elsewhere: `nexus_docs` reads its knowledge base about Nexus
//! (`knowledge.rs`), and `agent` starts a subagent (`mod.rs`).

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::text_diff::unified_diff;

/// The most text a tool sends back to the model.
const MAX_RESULT_CHARS: usize = 20_000;
/// The most lines `read_file` returns at once.
const MAX_LINES: usize = 2_000;
/// How long a command may run, unless the model asks for less.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// The tools, as `/v1/chat/completions` takes them.
pub fn specs() -> Value {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": { "type": "object", "properties": properties, "required": required },
            }
        })
    };
    let string = |description: &str| json!({ "type": "string", "description": description });
    let integer = |description: &str| json!({ "type": "integer", "description": description });
    json!([
        tool(
            "read_file",
            "Read a text file in the workspace, with line numbers.",
            json!({
                "path": string("Path, relative to the workspace."),
                "offset": integer("First line to read, from 1."),
                "limit": integer("How many lines to read (at most 2000)."),
            }),
            &["path"],
        ),
        tool(
            "list_dir",
            "List a folder in the workspace: its files, and its folders ending in /.",
            json!({ "path": string("Folder, relative to the workspace; the workspace itself when empty.") }),
            &[],
        ),
        tool(
            "search",
            "Search file contents in the workspace with a regular expression (ripgrep syntax).",
            json!({
                "pattern": string("Regular expression."),
                "path": string("File or folder to search, relative to the workspace."),
            }),
            &["pattern"],
        ),
        tool(
            "edit_file",
            "Replace text in a file. old_string must occur exactly once, unless replace_all is true.",
            json!({
                "path": string("Path, relative to the workspace."),
                "old_string": string("The exact text to replace."),
                "new_string": string("The text to put in its place."),
                "replace_all": { "type": "boolean", "description": "Replace every occurrence." },
            }),
            &["path", "old_string", "new_string"],
        ),
        tool(
            "write_file",
            "Create a file, or replace a whole file.",
            json!({
                "path": string("Path, relative to the workspace."),
                "content": string("The whole new content."),
            }),
            &["path", "content"],
        ),
        tool(
            "run_command",
            "Run a shell command in the workspace and read its output and exit code.",
            json!({
                "command": string("The command, as a shell runs it."),
                "timeout_seconds": integer("Stop it after this many seconds (at most 600)."),
            }),
            &["command"],
        ),
        tool(
            "update_plan",
            "Keep a short plan for a task of several steps: send the whole list each time a step starts or ends.",
            json!({
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": string("What the step does, in a few words."),
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                        },
                        "required": ["step", "status"],
                    },
                },
            }),
            &["steps"],
        ),
    ])
}

/// What a tool did, for the person to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Read {
        path: String,
        lines: usize,
    },
    Listed {
        path: String,
        entries: usize,
    },
    Searched {
        pattern: String,
        matches: usize,
    },
    Edited {
        path: String,
        added: usize,
        removed: usize,
        diff: Vec<String>,
        created: bool,
    },
    Ran {
        command: String,
        exit: Option<i32>,
        output: String,
    },
    /// The agent's plan: each step and whether it is `pending`, `in_progress` or `completed`.
    Planned {
        steps: Vec<(String, String)>,
    },
    /// The agent read a topic of its knowledge base (`knowledge.rs`): which one, and what it said.
    Docs {
        topic: String,
        text: String,
    },
    /// A tool of an MCP server (`mcp.rs`) answered: `input` as JSON, `error` when it said it failed.
    Called {
        name: String,
        input: String,
        output: String,
        error: bool,
    },
    /// The tool could not run, or the person declined it.
    Failed {
        what: String,
        error: String,
    },
}

/// What needs the person's yes before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Edit {
        path: String,
        diff: Vec<String>,
    },
    Run {
        command: String,
    },
    /// A tool of an MCP server, with its input as JSON.
    Tool {
        name: String,
        input: String,
    },
}

/// The person's answer to an `Ask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Yes, and do not ask again for this kind (edits, or commands) in this session.
    Always,
    Deny,
}

/// Run one tool call. `ask` gets the person's decision when the call changes something. Answers what to tell the
/// model, and what to show the person.
pub fn run(name: &str, arguments: &str, workspace: &Path, ask: &mut dyn FnMut(Ask) -> Decision) -> (String, Action) {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let number = |key: &str| args.get(key).and_then(Value::as_u64);
    let failed = |what: &str, error: String| {
        (
            format!("Error: {error}"),
            Action::Failed {
                what: what.into(),
                error,
            },
        )
    };
    match name {
        "read_file" => match read_file(workspace, &text("path"), number("offset"), number("limit")) {
            Ok((content, lines)) => (
                content,
                Action::Read {
                    path: text("path"),
                    lines,
                },
            ),
            Err(error) => failed(&format!("read {}", text("path")), error),
        },
        "list_dir" => match list_dir(workspace, &text("path")) {
            Ok((content, entries)) => {
                let path = Some(text("path"))
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| ".".into());
                (content, Action::Listed { path, entries })
            }
            Err(error) => failed(&format!("list {}", text("path")), error),
        },
        "search" => match search(workspace, &text("pattern"), &text("path")) {
            Ok((content, matches)) => (
                content,
                Action::Searched {
                    pattern: text("pattern"),
                    matches,
                },
            ),
            Err(error) => failed(&format!("search {}", text("pattern")), error),
        },
        "edit_file" | "write_file" => {
            let path = text("path");
            let planned = if name == "edit_file" {
                plan_edit(
                    workspace,
                    &path,
                    &text("old_string"),
                    &text("new_string"),
                    args.get("replace_all") == Some(&Value::Bool(true)),
                )
            } else {
                plan_write(workspace, &path, &text("content"))
            };
            let (target, old, new) = match planned {
                Ok(planned) => planned,
                Err(error) => return failed(&format!("edit {path}"), error),
            };
            let changes = unified_diff(old.as_deref().unwrap_or(""), &new, &path, &path);
            let (added, removed) = (changes.added, changes.removed);
            let diff: Vec<String> = changes
                .lines
                .into_iter()
                .filter(|line| !line.starts_with("---") && !line.starts_with("+++"))
                .collect();
            if ask(Ask::Edit {
                path: path.clone(),
                diff: diff.clone(),
            }) == Decision::Deny
            {
                return declined(&format!("edit {path}"));
            }
            if let Some(parent) = target.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return failed(&format!("edit {path}"), error.to_string());
            }
            match std::fs::write(&target, &new) {
                Ok(()) => {
                    let created = old.is_none();
                    let verb = if created { "Created" } else { "Edited" };
                    (
                        format!("{verb} {path}: +{added} -{removed} lines."),
                        Action::Edited {
                            path,
                            added,
                            removed,
                            diff,
                            created,
                        },
                    )
                }
                Err(error) => failed(&format!("edit {path}"), error.to_string()),
            }
        }
        "run_command" => {
            let command = text("command");
            if command.trim().is_empty() {
                return failed("run", "no command".into());
            }
            if ask(Ask::Run {
                command: command.clone(),
            }) == Decision::Deny
            {
                return declined(&format!("run {command}"));
            }
            let timeout =
                number("timeout_seconds").map_or(COMMAND_TIMEOUT, |seconds| Duration::from_secs(seconds.clamp(1, 600)));
            match run_command(workspace, &command, timeout) {
                Ok((exit, output)) => {
                    let status = match exit {
                        Some(code) => format!("Exit code {code}."),
                        None => format!("Stopped after {} seconds.", timeout.as_secs()),
                    };
                    (
                        format!("{status}\n{}", clip(&output)),
                        Action::Ran { command, exit, output },
                    )
                }
                Err(error) => failed(&format!("run {command}"), error),
            }
        }
        "update_plan" => {
            let steps: Vec<(String, String)> = args
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|step| {
                    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    (text("step"), text("status"))
                })
                .filter(|(step, _)| !step.is_empty())
                .collect();
            if steps.is_empty() {
                return failed("update the plan", "no steps".into());
            }
            ("Plan updated.".into(), Action::Planned { steps })
        }
        other => failed(other, format!("there is no tool named {other}")),
    }
}

/// A tool call of a saved conversation as it showed when it ran, from its name, its arguments and what it told the
/// model, so a resumed conversation shows its history whole. An MCP tool (`mcp__…`) is a call; the `agent` tool is not a
/// tool here (the session shows it as a subagent).
pub fn replay(name: &str, arguments: &str, result: &str) -> Action {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let target = match name {
        "read_file" => format!("read {}", text("path")),
        "list_dir" => format!("list {}", text("path")),
        "search" => format!("search {}", text("pattern")),
        "edit_file" | "write_file" => format!("edit {}", text("path")),
        "run_command" => format!("run {}", text("command")),
        "update_plan" => "update the plan".into(),
        other => format!("call {other}"),
    };
    if name == super::knowledge::TOOL {
        // The topic as it showed: its own name, whatever word asked for it. Nothing is read from Gateway again.
        return Action::Docs {
            topic: super::knowledge::read(&text("topic"), &String::new).0,
            text: result.to_string(),
        };
    }
    if name.starts_with("mcp__") {
        let error = result.strip_prefix("Error: ");
        return Action::Called {
            name: name.to_string(),
            input: args.to_string(),
            output: error.unwrap_or(result).to_string(),
            error: error.is_some(),
        };
    }
    for (prefix, why) in [
        ("Error: ", None),
        ("Declined: ", Some("declined")),
        ("Not run: ", Some("not run")),
    ] {
        if let Some(rest) = result.strip_prefix(prefix) {
            return Action::Failed {
                what: target,
                error: why.map_or_else(|| rest.to_string(), String::from),
            };
        }
    }
    let lines = result.lines().count();
    match name {
        "read_file" => Action::Read {
            path: text("path"),
            lines,
        },
        "list_dir" => Action::Listed {
            path: Some(text("path"))
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| ".".into()),
            entries: lines,
        },
        "search" => Action::Searched {
            pattern: text("pattern"),
            matches: lines,
        },
        "edit_file" | "write_file" => {
            let path = text("path");
            let created = result.starts_with("Created");
            // An edit's diff from its own strings; a new file's, its content. A file written over shows its counts only.
            let (old, new) = match name {
                "edit_file" => (text("old_string"), text("new_string")),
                _ if created => (String::new(), text("content")),
                _ => (String::new(), String::new()),
            };
            let changes = unified_diff(&old, &new, &path, &path);
            let diff: Vec<String> = changes
                .lines
                .into_iter()
                .filter(|line| !line.starts_with("---") && !line.starts_with("+++"))
                .collect();
            // The counts the result says: `Edited path: +2 -1 lines.`
            let count = |sign: char| {
                result.split_whitespace().find_map(|word| {
                    word.strip_prefix(sign)?
                        .trim_end_matches(['.', ','])
                        .parse::<usize>()
                        .ok()
                })
            };
            Action::Edited {
                path,
                added: count('+').unwrap_or(changes.added),
                removed: count('-').unwrap_or(changes.removed),
                diff,
                created,
            }
        }
        "run_command" => {
            let (status, output) = result.split_once('\n').unwrap_or((result, ""));
            let exit = status
                .strip_prefix("Exit code ")
                .and_then(|code| code.trim_end_matches('.').parse::<i32>().ok());
            Action::Ran {
                command: text("command"),
                exit,
                output: output.to_string(),
            }
        }
        "update_plan" => Action::Planned {
            steps: args
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|step| {
                    let text = |key: &str| step.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    (text("step"), text("status"))
                })
                .collect(),
        },
        _ => Action::Failed {
            what: target,
            error: "a tool this version does not know".into(),
        },
    }
}

pub fn declined(what: &str) -> (String, Action) {
    let error = "the person declined it. Ask them what to do instead, or try another way.".to_string();
    (
        format!("Declined: {error}"),
        Action::Failed {
            what: what.into(),
            error: "declined".into(),
        },
    )
}

/// `relative` inside the workspace, or why not. Only the path's own words count (`..` that climbs out is refused),
/// so a file that does not exist yet can be a target.
pub fn inside(workspace: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim();
    let path = Path::new(relative);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    let outside = || Err(format!("{relative} is outside the workspace ({})", workspace.display()));
    if !normal.starts_with(workspace) {
        return outside();
    }
    // A symlink inside the folder can point out of it: where the deepest part that exists really is must be inside
    // too. (A file that does not exist yet is judged by its folder.)
    let mut existing = normal.clone();
    while !existing.exists() && existing.pop() {}
    if existing.starts_with(workspace)
        && let (Ok(real), Ok(root)) = (existing.canonicalize(), workspace.canonicalize())
        && !real.starts_with(&root)
    {
        return outside();
    }
    Ok(normal)
}

fn read_file(workspace: &Path, path: &str, offset: Option<u64>, limit: Option<u64>) -> Result<(String, usize), String> {
    let target = inside(workspace, path)?;
    let bytes = std::fs::read(&target).map_err(|error| format!("{path}: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not a text file"))?;
    let first = offset.unwrap_or(1).max(1) as usize;
    let count = limit.map_or(MAX_LINES, |limit| (limit as usize).clamp(1, MAX_LINES));
    let lines: Vec<String> = text
        .lines()
        .enumerate()
        .skip(first - 1)
        .take(count)
        .map(|(index, line)| format!("{:>5}  {line}", index + 1))
        .collect();
    let total = text.lines().count();
    let mut out = clip(&lines.join("\n"));
    if first - 1 + lines.len() < total {
        out.push_str(&format!(
            "\n({} of {total} lines shown; read on with offset.)",
            lines.len()
        ));
    }
    Ok((out, lines.len()))
}

fn list_dir(workspace: &Path, path: &str) -> Result<(String, usize), String> {
    let target = inside(workspace, if path.is_empty() { "." } else { path })?;
    let mut entries: Vec<String> = std::fs::read_dir(&target)
        .map_err(|error| format!("{path}: {error}"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name() != ".git")
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    entries.sort();
    let count = entries.len();
    Ok((clip(&entries.join("\n")), count))
}

fn search(workspace: &Path, pattern: &str, path: &str) -> Result<(String, usize), String> {
    if pattern.is_empty() {
        return Err("no pattern".into());
    }
    let target = inside(workspace, if path.is_empty() { "." } else { path })?;
    let ripgrep = Command::new("rg")
        .args([
            "--line-number",
            "--no-heading",
            "--color",
            "never",
            "--max-count",
            "50",
            "--",
            pattern,
        ])
        .arg(&target)
        .current_dir(workspace)
        .output();
    let output = match ripgrep {
        Ok(output) => output,
        Err(_) => Command::new("grep")
            .args(["-rnE", "--exclude-dir=.git", "--", pattern])
            .arg(&target)
            .current_dir(workspace)
            .output()
            .map_err(|error| format!("neither rg nor grep could run: {error}"))?,
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let prefix = format!("{}/", workspace.display());
    let lines: Vec<String> = text
        .lines()
        .take(300)
        .map(|line| line.replacen(&prefix, "", 1))
        .collect();
    let count = text.lines().count();
    if count == 0 {
        return Ok(("No matches.".into(), 0));
    }
    Ok((clip(&lines.join("\n")), count))
}

/// The file an edit would write: where, what it holds now, and what it would hold.
type Plan = (PathBuf, Option<String>, String);

fn plan_edit(workspace: &Path, path: &str, old: &str, new: &str, all: bool) -> Result<Plan, String> {
    let target = inside(workspace, path)?;
    let current = std::fs::read_to_string(&target).map_err(|error| format!("{path}: {error}"))?;
    if old.is_empty() {
        return Err("old_string is empty; use write_file to create a file".into());
    }
    let found = current.matches(old).count();
    let replaced = match (found, all) {
        (0, _) => return Err(format!("old_string does not occur in {path}")),
        (1, _) | (_, true) => current.replace(old, new),
        (many, false) => {
            return Err(format!(
                "old_string occurs {many} times in {path}; add context or set replace_all"
            ));
        }
    };
    Ok((target, Some(current), replaced))
}

fn plan_write(workspace: &Path, path: &str, content: &str) -> Result<Plan, String> {
    let target = inside(workspace, path)?;
    let current = std::fs::read_to_string(&target).ok();
    Ok((target, current, content.to_string()))
}

/// Run `command` in a shell in the workspace: its exit code (`None` when it ran out of time and was stopped) and its
/// output, standard error after standard output.
pub fn run_command(workspace: &Path, command: &str, timeout: Duration) -> Result<(Option<i32>, String), String> {
    let mut shell = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.args(["/C", command]);
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.args(["-c", command]);
        shell
    };
    shell
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = CLI_FOLDER.get().and_then(|folder| path_with(folder)) {
        shell.env("PATH", path);
    }
    // Its own process group, so a timeout stops what the shell started too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut shell, 0);
    let mut child = shell.spawn().map_err(|error| error.to_string())?;
    // Read both pipes while it runs: a command that fills a pipe would otherwise wait for us forever.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || read_all(stdout.as_mut()));
    let err = std::thread::spawn(move || read_all(stderr.as_mut()));
    let deadline = Instant::now() + timeout;
    let exit = loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => break status.code().or(Some(-1)),
            None if Instant::now() >= deadline => {
                #[cfg(unix)]
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(child.id() as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut output = out.join().unwrap_or_default();
    let errors = err.join().unwrap_or_default();
    if !errors.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&errors);
    }
    Ok((exit, output))
}

/// The folder that is first on a command's `PATH` ([`link_this_cli`]). It holds one thing: `nexus`, a link to this
/// program.
static CLI_FOLDER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Makes `nexus` in a command this CLI, for the agent's commands and the person's `!` commands. The agent's knowledge
/// base tells it to run `nexus` commands (`knowledge.rs`), and the person's `PATH` may hold another program of that
/// name before this one (the JavaScript CLI, the Exchange's), or none (an app started from the dock has a short
/// `PATH`). `folder` is the CLI's own, beside its config. The link is alone in a folder there, so nothing else on the
/// `PATH` changes its place. Where the link cannot be made, the `PATH` stays as it is.
pub fn link_this_cli(folder: &Path) {
    if let Some(linked) = std::env::current_exe()
        .ok()
        .and_then(|program| cli_link(folder, &program))
    {
        let _ = CLI_FOLDER.set(linked);
    }
}

/// A folder under `folder` that holds a link named `nexus` to `program`. Each program has a folder of its own, by its
/// path, so two builds of the CLI that share a config never repoint each other's link.
#[cfg(unix)]
fn cli_link(folder: &Path, program: &Path) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let id = hex::encode(&Sha256::digest(program.to_string_lossy().as_bytes())[..6]);
    let own = folder.join("agent-bin").join(id);
    let link = own.join("nexus");
    if std::fs::read_link(&link).is_ok_and(|target| target == program) {
        return Some(own);
    }
    std::fs::create_dir_all(&own).ok()?;
    // Made under another name and renamed into place, so a command never finds the folder without its link.
    let staged = own.join(format!(".nexus-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(program, &staged).ok()?;
    if std::fs::rename(&staged, &link).is_err() {
        let _ = std::fs::remove_file(&staged);
        return None;
    }
    Some(own)
}

/// Windows needs a privilege to make a link, so there the `PATH` stays as it is.
#[cfg(not(unix))]
fn cli_link(_: &Path, _: &Path) -> Option<PathBuf> {
    None
}

/// The `PATH` with `folder` first.
fn path_with(folder: &Path) -> Option<std::ffi::OsString> {
    let rest = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(folder.to_path_buf()).chain(std::env::split_paths(&rest))).ok()
}

fn read_all(pipe: Option<&mut impl std::io::Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// At most `MAX_RESULT_CHARS`: the start and the end, with what was left out said in between.
fn clip(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_RESULT_CHARS {
        return text.to_string();
    }
    let half = MAX_RESULT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
    format!("{head}\n… {} characters left out …\n{tail}", count - MAX_RESULT_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).expect("workspace");
        std::fs::write(dir.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").expect("file");
        dir
    }

    fn allow(_: Ask) -> Decision {
        Decision::Allow
    }

    #[test]
    fn paths_stay_inside_the_workspace() {
        let root = Path::new("/work/repo");
        assert_eq!(
            inside(root, "src/../Cargo.toml"),
            Ok(PathBuf::from("/work/repo/Cargo.toml"))
        );
        assert!(inside(root, "../other/secret").is_err());
        assert!(inside(root, "/etc/passwd").is_err());
        assert_eq!(inside(root, "/work/repo/a.rs"), Ok(PathBuf::from("/work/repo/a.rs")));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_workspace_is_outside() {
        let dir = workspace();
        let elsewhere = std::env::temp_dir().join(format!("nexus-elsewhere-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&elsewhere).expect("folder");
        std::fs::write(elsewhere.join("secret.txt"), "x").expect("file");
        std::os::unix::fs::symlink(&elsewhere, dir.join("link")).expect("symlink");
        assert!(inside(&dir, "link/secret.txt").is_err());
        assert!(inside(&dir, "link/new.txt").is_err(), "a new file behind the link too");
        assert!(inside(&dir, "src/new.rs").is_ok());
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(elsewhere);
    }

    #[test]
    fn reads_lists_and_edits() {
        let dir = workspace();
        let (text, action) = run(
            "read_file",
            r#"{"path":"src/main.rs","offset":2,"limit":1}"#,
            &dir,
            &mut allow,
        );
        assert!(text.starts_with("    2      println!"), "{text}");
        assert!(text.ends_with("(1 of 3 lines shown; read on with offset.)"));
        assert_eq!(
            action,
            Action::Read {
                path: "src/main.rs".into(),
                lines: 1
            }
        );
        let (text, _) = run("list_dir", "{}", &dir, &mut allow);
        assert_eq!(text, "src/");

        let args = r#"{"path":"src/main.rs","old_string":"\"hi\"","new_string":"\"hello\""}"#;
        let mut asked = Vec::new();
        let (text, action) = run("edit_file", args, &dir, &mut |ask| {
            asked.push(ask);
            Decision::Allow
        });
        assert_eq!(text, "Edited src/main.rs: +1 -1 lines.");
        assert!(matches!(
            action,
            Action::Edited {
                added: 1,
                removed: 1,
                created: false,
                ..
            }
        ));
        assert!(
            matches!(&asked[0], Ask::Edit { diff, .. } if diff.iter().any(|line| line.contains("+    println!(\"hello\");")))
        );
        assert!(
            std::fs::read_to_string(dir.join("src/main.rs"))
                .unwrap_or_default()
                .contains("hello")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_declined_change_is_not_made() {
        let dir = workspace();
        let (text, action) = run("write_file", r#"{"path":"notes.md","content":"x"}"#, &dir, &mut |_| {
            Decision::Deny
        });
        assert!(text.starts_with("Declined:"));
        assert!(matches!(action, Action::Failed { .. }));
        assert!(!dir.join("notes.md").exists());
        let (text, _) = run(
            "edit_file",
            r#"{"path":"src/main.rs","old_string":"nope","new_string":"x"}"#,
            &dir,
            &mut allow,
        );
        assert_eq!(text, "Error: old_string does not occur in src/main.rs");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_command_and_stops_a_slow_one() {
        let dir = workspace();
        let (text, action) = run(
            "run_command",
            r#"{"command":"echo out; echo err >&2; exit 3"}"#,
            &dir,
            &mut allow,
        );
        assert_eq!(text, "Exit code 3.\nout\nerr\n");
        assert!(matches!(action, Action::Ran { exit: Some(3), .. }));
        let (text, _) = run(
            "run_command",
            r#"{"command":"sleep 5","timeout_seconds":1}"#,
            &dir,
            &mut allow,
        );
        assert!(text.starts_with("Stopped after 1 seconds."), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `nexus` in a command is the CLI that runs the agent: a link to it, alone in a folder that is first on the `PATH`.
    #[cfg(unix)]
    #[test]
    fn a_folder_with_only_a_link_to_this_cli_goes_first_on_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = workspace();
        // A stand-in for the CLI: a program that says it ran.
        let program = dir.join("the-cli");
        std::fs::write(&program, "#!/bin/sh\necho linked \"$@\"\n").expect("the program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("its mode");
        let program = program.as_path();
        let linked = cli_link(&dir, program).expect("the link");
        assert!(linked.starts_with(dir.join("agent-bin")));
        let entries: Vec<_> = std::fs::read_dir(&linked)
            .expect("the folder")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(
            entries,
            ["nexus"],
            "nothing else: no other program changes its place on the PATH"
        );
        assert_eq!(std::fs::read_link(linked.join("nexus")).ok().as_deref(), Some(program));
        assert_eq!(cli_link(&dir, program).as_ref(), Some(&linked), "made once");
        assert_ne!(
            cli_link(&dir, Path::new("/bin/ls")).as_ref(),
            Some(&linked),
            "another build has a folder of its own"
        );
        let path = path_with(&linked).expect("a PATH");
        assert_eq!(std::env::split_paths(&path).next().as_ref(), Some(&linked));
        // With the folder first on its `PATH`, a command's `nexus` is that program.
        let command = format!("export PATH=\"{}:$PATH\"; nexus pages list", linked.display());
        let (exit, output) = run_command(&dir, &command, COMMAND_TIMEOUT).expect("the command ran");
        assert_eq!((exit, output.trim()), (Some(0), "linked pages list"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_saved_conversations_tools_show_as_they_ran() {
        let edit = replay(
            "edit_file",
            r#"{"path":"a.py","old_string":"hi","new_string":"hello"}"#,
            "Edited a.py: +1 -1 lines.",
        );
        assert!(
            matches!(&edit, Action::Edited { path, added: 1, removed: 1, created: false, diff } if path == "a.py" && diff.iter().any(|line| line == "+hello")),
            "{edit:?}"
        );
        assert_eq!(
            replay("run_command", r#"{"command":"ls"}"#, "Exit code 0.\na.py\nb.py"),
            Action::Ran {
                command: "ls".into(),
                exit: Some(0),
                output: "a.py\nb.py".into()
            }
        );
        assert_eq!(
            replay("read_file", r#"{"path":"a.py"}"#, "1  hi\n2  there"),
            Action::Read {
                path: "a.py".into(),
                lines: 2
            }
        );
        assert!(matches!(
            replay("run_command", r#"{"command":"rm -rf x"}"#, "Declined: the person declined it."),
            Action::Failed { error, .. } if error == "declined"
        ));
        assert!(matches!(
            replay("mcp__nexus_app__list_sessions", "{}", "Error: off"),
            Action::Called { error: true, output, .. } if output == "off"
        ));
        assert_eq!(
            replay("nexus_docs", r#"{"topic":"billing"}"#, "# Budgets"),
            Action::Docs {
                topic: "spend".into(),
                text: "# Budgets".into()
            }
        );
    }
}
