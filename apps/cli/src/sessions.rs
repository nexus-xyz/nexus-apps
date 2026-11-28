//! `nexus sessions` (`sessions.ts`): Code sessions in Nexus for desktop, from a script or another agent, through the
//! app's outside access (`desktop/README.md`, "Outside access"). Nothing goes to Gateway: the CLI talks to the app over
//! its Unix socket, which only this person can open. The first command asks the app for access, and the person
//! approves it there; the token is kept beside the CLI's config (`desktop-access.json`, 0600).
//!
//! `list`, `show`, `start`, `send`, `stop` and `name` call the app's MCP tools; `mcp` relays an outside agent's MCP
//! messages.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::Context;
use crate::args::Args;
use crate::error::{CliError, Error, Result, usage};
use crate::json::pretty;
use crate::sessions_wait;
use crate::style::{Style, pad_end};

#[path = "sessions_desktop.rs"]
mod desktop;
pub use desktop::{Desktop, exchange};

pub const SESSIONS_COMMANDS: &[(&str, &str)] = &[
    (
        "list [--project P] [--archived] [--limit N] [--json]",
        "Code sessions in Nexus for desktop, newest first: id, status, name, title and project",
    ),
    (
        "show <id|name|title> [--turns N] [--json]",
        "A session's status and its latest turns: each prompt, the agent's last answer and how the turn ended. Name a session by its id (a word starting with t_), or by its name or exact title among the newest 200; a name past those goes to the app's own lookup",
    ),
    (
        sessions_wait::USAGE,
        "Wait for a session to finish, stop, fail or need approval. The default timeout is 1800 seconds. It can run up to 3 seconds past --timeout for one last check.",
    ),
    (
        "start \"<prompt>\" [--project P] [--agent A] [--model M] [--mode M] [--worktree] [--folder] [--name N] [--json]",
        "Start a session with its project's defaults (the current folder's project when --project is left out; - reads the prompt from stdin)",
    ),
    (
        "send <id|name|title> \"<message>\"",
        "Message a session; a busy one gets it after its current turn (- reads stdin). Name a session by its id (a word starting with t_), or by its name or exact title among the newest 200; a name past those goes to the app's own lookup",
    ),
    (
        "stop <id|name|title>",
        "Stop a session's current turn. Name a session by its id (a word starting with t_), or by its name or exact title among the newest 200; a name past those goes to the app's own lookup",
    ),
    (
        "name <id|name|title> [<new-name>] [--clear] [--json]",
        "Give a session a unique name, which Claude Code's ListAgents and SendMessage reach it by; --clear removes it. A name is 1 to 40 letters, digits, dots, dashes or underscores, and never starts with t_",
    ),
    (
        "connect [--read-only]",
        "Ask Nexus for desktop for access now (the other commands ask by themselves); you approve it in the app",
    ),
    (
        "disconnect",
        "Forget this CLI's access on this computer (revoke it in the app: Settings, MCP servers, Outside access)",
    ),
    (
        "mcp [--name N]",
        "The same tools as an MCP server on stdin and stdout, for another agent: claude mcp add nexus-sessions -- nexus sessions mcp",
    ),
];

pub(crate) const CLIENT: &str = "nexus CLI";
const NOT_RUNNING: &str = "Nexus for desktop is not running, or its outside access is off.";
const NOT_RUNNING_HINT: &str = "Open Nexus and turn on Settings › MCP servers › Outside access.";
const REVOKE_HINT: &str = "Revoke it in Nexus too: Settings › MCP servers › Outside access.";
/// What the app answers a request with no access, or with revoked access.
const NO_ACCESS: i64 = -32001;
const DESKTOP_UNAVAILABLE: &str = "desktop_unavailable";
const ACCESS_REFUSED: &str = "desktop_access_refused";
const SESSION_NOT_FOUND: &str = "session_not_found";
const SESSION_TITLE_AMBIGUOUS: &str = "session_title_ambiguous";
const NO_ANSWER: &str = "Nexus for desktop did not answer in time.";

pub(crate) fn ends_wait(error: &CliError) -> bool {
    matches!(
        error.code,
        Some(DESKTOP_UNAVAILABLE | ACCESS_REFUSED | SESSION_NOT_FOUND | SESSION_TITLE_AMBIGUOUS)
    )
}

/// The app is not running or its outside access is off. After a read succeeded, the app may be restarting.
pub(crate) fn app_unavailable(error: &CliError) -> bool {
    error.code == Some(DESKTOP_UNAVAILABLE)
}

/// How long a request for access waits: the app's prompt waits two minutes for the person.
const CONNECT_WAIT: Duration = Duration::from_secs(130);
/// How long a call waits: a start waits up to 90 seconds for the app's window.
const CALL_WAIT: Duration = Duration::from_secs(120);
/// The most sessions `list_sessions` gives: a title names one of the newest 200, archived ones included.
const TITLE_LIMIT: u64 = 200;
/// How every session id starts, side chats' too (the workbench's `t_…` ids).
const ID_PREFIX: &str = "t_";

/// The app's socket: `NEXUS_DESKTOP_SOCKET`, else where Nexus for desktop listens (src-tauri/src/outside/socket.rs):
/// `$XDG_RUNTIME_DIR/xyz.nexus.desktop/sessions.sock`, else the app's data folder's `run/sessions.sock`. None on
/// Windows.
pub fn socket(ctx: &Context) -> Option<PathBuf> {
    if let Some(path) = ctx.var("NEXUS_DESKTOP_SOCKET").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    if cfg!(windows) {
        return None;
    }
    let runtime = ctx
        .var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && dir.is_dir());
    if let Some(dir) = runtime {
        return Some(dir.join("xyz.nexus.desktop").join("sessions.sock"));
    }
    let home = PathBuf::from(ctx.var("HOME")?);
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        ctx.var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
    };
    Some(data.join("xyz.nexus.desktop").join("run").join("sessions.sock"))
}

/// Where the tokens the app granted are kept: beside the CLI's config.
fn tokens_path(ctx: &Context) -> PathBuf {
    ctx.config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("desktop-access.json")
}

fn saved_token(ctx: &Context, client: &str) -> Option<String> {
    let text = std::fs::read_to_string(tokens_path(ctx)).ok()?;
    let saved: Value = serde_json::from_str(&text).ok()?;
    saved["tokens"][client].as_str().map(String::from)
}

fn save_token(ctx: &Context, client: &str, token: Option<&str>) -> Result<()> {
    let path = tokens_path(ctx);
    let mut saved: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if !saved["tokens"].is_object() {
        saved["tokens"] = json!({});
    }
    match token {
        Some(token) => saved["tokens"][client] = json!(token),
        None => {
            if let Some(tokens) = saved["tokens"].as_object_mut() {
                tokens.remove(client);
            }
        }
    }
    write_private(&path, &pretty(&saved))
        .map_err(|e| CliError::new(format!("Could not save {}: {e}", path.display())).into())
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")
}

/// The text of argument `word`, or stdin when it is `-`.
fn text_of(ctx: &Context, word: Option<&String>, what: &str, usage_line: &str) -> Result<String> {
    let text = match word.map(String::as_str) {
        Some("-") => ctx.io.read_stdin(),
        Some(text) => text.to_string(),
        None => String::new(),
    };
    if text.trim().is_empty() {
        return usage(format!("Give the {what}."), Some(usage_line));
    }
    Ok(text)
}

fn session_not_found(message: &str, word: &str) -> bool {
    message == format!("No session {word}.")
}

/// The session `word` names: its id, else its name in any case, else its exact title. A word that starts with `t_` is
/// an id and goes to the app as it is, with no list: so a title that starts with `t_` is read as an id (a name never
/// starts with `t_`). Any other word is first the name of one of the newest 200 sessions. A name is unique ignoring
/// case (the app's `lower(name)` index), so at most one matches. When none does but a title matches (exactly, not
/// trimmed), the app's own lookup (id, then name, among all its sessions) runs first, because a name wins over a
/// title: its session when it finds one, the title's only on its "No session" answer. When nothing matches, `word`
/// goes to the app as it is. Several sessions with that title are refused with their ids.
pub(crate) fn session_of(desktop: &Desktop, word: &str) -> Result<String> {
    if word.starts_with(ID_PREFIX) {
        return Ok(word.to_string());
    }
    let listed = desktop
        .call("list_sessions", json!({"archived": true, "limit": TITLE_LIMIT}))
        .map_err(|error| {
            // Keep the cause's hint: "not running" still says how to turn outside access on.
            if let Error::Cli(cause) = &error {
                if cause.code == Some(ACCESS_REFUSED) {
                    return error;
                }
                let message = title_lookup_error(word, &cause.message);
                return CliError {
                    message,
                    ..cause.clone()
                }
                .into();
            }
            CliError::new(title_lookup_error(word, &message_of(&error))).into()
        })?;
    let sessions = listed["sessions"].as_array().cloned().unwrap_or_default();
    if let Some(id) = sessions
        .iter()
        .find(|session| {
            session["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(word))
        })
        .and_then(|session| session["id"].as_str())
    {
        return Ok(id.to_string());
    }
    let ids: Vec<&str> = sessions
        .iter()
        .filter(|session| session["title"] == word)
        .filter_map(|session| session["id"].as_str())
        .collect();
    if ids.is_empty() {
        return Ok(word.to_string());
    }
    // A session past the newest 200 may hold `word` as its name. The app's "No session" answer (outside/tools.rs
    // `find`) says none does; any other failure stops here rather than act on the title's session.
    match desktop.call("show_session", json!({"session_id": word, "turns": 1})) {
        Ok(shown) => {
            if let Some(id) = shown["id"].as_str() {
                return Ok(id.to_string());
            }
        }
        Err(error) if session_not_found(&message_of(&error), word) => {}
        Err(error) => return Err(error),
    }
    match ids.as_slice() {
        [id] => Ok((*id).to_string()),
        _ => Err(desktop
            .cli_error(
                format!(
                    "{} sessions are titled “{word}”: {}. Name one by its id.",
                    ids.len(),
                    ids.join(", ")
                ),
                SESSION_TITLE_AMBIGUOUS,
            )
            .into()),
    }
}

fn title_lookup_error(word: &str, cause: &str) -> String {
    format!(
        "Could not look up the title “{word}”: {}. Name the session by its id.",
        cause.trim_end_matches('.')
    )
}

fn number(args: &Args, name: &str) -> Result<Option<u64>> {
    match args.flag_set(name) {
        None => Ok(None),
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| CliError::usage(format!("--{name} takes a number."), None).into()),
    }
}

pub fn sessions_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  nexus sessions [list|show|wait|start|send|stop|name|connect|disconnect|mcp]",
            style.bold("Usage")
        ),
        String::new(),
    ];
    let width = SESSIONS_COMMANDS
        .iter()
        .map(|(usage, _)| usage.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let width = width.min(40);
    for (usage, text) in SESSIONS_COMMANDS {
        if usage.chars().count() + 2 <= width {
            lines.push(format!("  {}{text}", style.cyan(&pad_end(usage, width))));
        } else {
            lines.push(format!("  {}", style.cyan(usage)));
            lines.push(format!("      {text}"));
        }
    }
    lines.push(String::new());
    lines.push(style.dim("Turn on Outside access in Nexus for desktop first: Settings, MCP servers."));
    lines
}

/// One session's line in `list`. `names` is the name column's width: 0 when no listed session has a name, and then
/// the line has no such column.
fn line_of(style: &Style, session: &Value, names: usize) -> String {
    let status = session["status"].as_str().unwrap_or_default();
    let project = session["project"]["name"].as_str().unwrap_or_default();
    let name = if names == 0 {
        String::new()
    } else {
        pad_end(session["name"].as_str().unwrap_or_default(), names + 2)
    };
    format!(
        "{}  {}{name}{}{}",
        style.cyan(session["id"].as_str().unwrap_or_default()),
        pad_end(status, 12),
        session["title"].as_str().unwrap_or_default(),
        if project.is_empty() {
            String::new()
        } else {
            style.dim(&format!("  {project}"))
        }
    )
}

pub fn sessions_command(rest: &[String], ctx: &Context, args: &Args, json: bool, style: &Style) -> Result<i32> {
    let out = |line: &str| ctx.io.out(line);
    let print = |value: &Value| {
        ctx.io.out(&pretty(value));
        Ok(0)
    };
    let action = rest.first().map(String::as_str).unwrap_or("list");
    let word_of = |usage_line: &str| -> Result<String> {
        match rest.get(1) {
            Some(id) if !id.is_empty() => Ok(id.clone()),
            _ => usage(
                "Name the session: its id, name or title, from nexus sessions list.",
                Some(usage_line),
            ),
        }
    };
    match action {
        "help" => {
            if json {
                return print(&json!({
                    "usage": "nexus sessions [list|show|wait|start|send|stop|name|connect|disconnect|mcp]",
                    "commands": ["list", "show", "wait", "start", "send", "stop", "name", "connect", "disconnect", "mcp"],
                }));
            }
            for line in sessions_help(style) {
                out(&line);
            }
            Ok(0)
        }
        "list" => {
            let mut arguments = Map::new();
            if let Some(project) = args.flag_set("project") {
                arguments.insert("project".into(), json!(project_of(ctx, project)));
            }
            if args.has("archived") {
                arguments.insert("archived".into(), json!(true));
            }
            if let Some(limit) = number(args, "limit")? {
                arguments.insert("limit".into(), json!(limit));
            }
            let listed = Desktop::new(ctx, CLIENT)?.call("list_sessions", Value::Object(arguments))?;
            if json {
                return print(&listed);
            }
            let sessions = listed["sessions"].as_array().cloned().unwrap_or_default();
            if sessions.is_empty() {
                out("No Code sessions in Nexus for desktop.");
                return Ok(0);
            }
            let names = sessions
                .iter()
                .filter_map(|session| session["name"].as_str())
                .map(|name| name.chars().count())
                .max()
                .unwrap_or(0);
            for session in &sessions {
                out(&line_of(style, session, names));
            }
            Ok(0)
        }
        "show" => {
            let word = word_of("nexus sessions show <id|name|title>")?;
            let turns = number(args, "turns")?;
            let desktop = Desktop::new(ctx, CLIENT)?;
            let mut arguments = json!({"session_id": session_of(&desktop, &word)?});
            if let Some(turns) = turns {
                arguments["turns"] = json!(turns);
            }
            let shown = desktop.call("show_session", arguments)?;
            if json {
                return print(&shown);
            }
            out(&style.bold(shown["title"].as_str().unwrap_or_default()));
            let facts = [
                shown["status"].as_str(),
                shown["agent"].as_str(),
                shown["model"].as_str(),
                shown["permissionMode"].as_str(),
                shown["name"].as_str(),
                shown["id"].as_str(),
            ];
            out(&style.dim(&facts.into_iter().flatten().collect::<Vec<_>>().join(" · ")));
            if let Some(path) = shown["worktree"]["path"].as_str().or(shown["project"]["path"].as_str()) {
                out(&style.dim(path));
            }
            for turn in shown["turns"].as_array().into_iter().flatten() {
                out("");
                out(&format!(
                    "{} {}",
                    style.cyan("›"),
                    turn["prompt"].as_str().unwrap_or_default()
                ));
                if let Some(answer) = turn["answer"].as_str().filter(|answer| !answer.is_empty()) {
                    out(answer);
                }
                let status = turn["status"].as_str().unwrap_or_default();
                match turn["error"].as_str() {
                    Some(error) => out(&style.dim(&format!("{status}: {error}"))),
                    None => out(&style.dim(status)),
                }
            }
            Ok(0)
        }
        "wait" => {
            let word = word_of(sessions_wait::COMMAND)?;
            sessions_wait::command(ctx, style, word, args)
        }
        "start" => {
            const USAGE: &str =
                "nexus sessions start \"<prompt>\" [--project P] [--agent A] [--model M] [--mode M] [--name N]";
            let prompt = text_of(ctx, rest.get(1), "first message", USAGE)?;
            if args.has("worktree") && args.has("folder") {
                return usage("Pick --worktree or --folder, not both.", Some(USAGE));
            }
            let project = project_of(ctx, args.flag_set("project").unwrap_or("."));
            let mut arguments = json!({"project": project, "prompt": prompt});
            for (flag, key) in [
                ("agent", "agent"),
                ("model", "model"),
                ("mode", "permission_mode"),
                ("name", "name"),
            ] {
                if let Some(value) = args.flag_set(flag) {
                    arguments[key] = json!(value);
                }
            }
            if args.has("worktree") || args.has("folder") {
                arguments["worktree"] = json!(args.has("worktree"));
            }
            let started = Desktop::new(ctx, CLIENT)?.call("start_session", arguments)?;
            if json {
                return print(&started);
            }
            let id = started["id"].as_str().unwrap_or_default();
            out(&format!(
                "Started {id}: {}",
                started["title"].as_str().unwrap_or_default()
            ));
            out(&style.dim(&format!("nexus sessions show {id}")));
            Ok(0)
        }
        "send" => {
            const USAGE: &str = "nexus sessions send <id|name|title> \"<message>\"";
            let word = word_of(USAGE)?;
            let text = text_of(ctx, rest.get(2), "message", USAGE)?;
            let desktop = Desktop::new(ctx, CLIENT)?;
            let id = session_of(&desktop, &word)?;
            let sent = desktop.call("send_message", json!({"session_id": id, "text": text}))?;
            if json {
                return print(&sent);
            }
            out(&format!("Sent to {id}."));
            Ok(0)
        }
        "stop" => {
            let word = word_of("nexus sessions stop <id|name|title>")?;
            let desktop = Desktop::new(ctx, CLIENT)?;
            let id = session_of(&desktop, &word)?;
            let stopped = desktop.call("stop_session", json!({"session_id": id}))?;
            if json {
                return print(&stopped);
            }
            out(&format!("Stopped {id}."));
            Ok(0)
        }
        "name" => {
            const USAGE: &str = "nexus sessions name <id|name|title> (<new-name> | --clear)";
            let word = word_of(USAGE)?;
            let name = match (rest.get(2), args.has("clear")) {
                (Some(name), false) if !name.is_empty() => name.clone(),
                (None, true) => String::new(),
                (Some(_), true) => return usage("Give a new name or --clear, not both.", Some(USAGE)),
                _ => return usage("Give the new name, or --clear to remove it.", Some(USAGE)),
            };
            let desktop = Desktop::new(ctx, CLIENT)?;
            let id = session_of(&desktop, &word)?;
            let named = desktop.call("name_session", json!({"session_id": id, "name": name}))?;
            if json {
                return print(&named);
            }
            if name.is_empty() {
                out(&format!("{id} has no name now."));
            } else {
                out(&format!("{id} is named {name}."));
            }
            Ok(0)
        }
        "connect" => {
            let scope = if args.has("read-only") { "read" } else { "act" };
            let (_, granted) = Desktop::new(ctx, CLIENT)?.connect(scope)?;
            if json {
                return print(&json!({"client": CLIENT, "scope": granted}));
            }
            let what = if granted == "act" { "read and act" } else { "read only" };
            out(&format!("Nexus for desktop granted {CLIENT} {what} access."));
            Ok(0)
        }
        "disconnect" => {
            Desktop::new(ctx, CLIENT)?.forget()?;
            if json {
                return print(&json!({"client": CLIENT, "forgotten": true}));
            }
            out(&format!("Forgot {CLIENT}'s access on this computer."));
            out(&style.dim(REVOKE_HINT));
            Ok(0)
        }
        "mcp" => {
            let client = args.flag_set("name").unwrap_or(CLIENT).to_string();
            let desktop = Desktop::new(ctx, &client)?;
            let stdin = std::io::stdin();
            bridge(&desktop, stdin.lock(), |line| println!("{line}"));
            Ok(0)
        }
        other => usage(
            format!("Unknown sessions command: {other}."),
            Some("nexus sessions [list|show|wait|start|send|stop|name|connect|disconnect|mcp]"),
        ),
    }
}

/// A project as the app reads it: `.` or a relative folder becomes an absolute path from the current folder; an
/// absolute path, an id or GitHub `owner/repo` stay as they are.
fn project_of(ctx: &Context, project: &str) -> String {
    let looks_like_path = project == "." || project.starts_with("./") || project.starts_with("../");
    if looks_like_path {
        let joined = ctx.cwd.join(project);
        return joined.canonicalize().unwrap_or(joined).display().to_string();
    }
    project.to_string()
}

/// Relays MCP messages, one per line, from `input` to the app and its answers to `output`. `initialize` is answered
/// here, so an agent starts before the person approves; the first tool list or call asks the app for access.
pub fn bridge(desktop: &Desktop, input: impl std::io::BufRead, mut output: impl FnMut(String)) {
    for line in input.lines() {
        let Ok(line) = line else { return };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            // A notification: nothing to answer.
            continue;
        };
        let reply = if message["method"] == "initialize" {
            json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "nexus_sessions", "version": crate::VERSION},
            }})
        } else {
            match desktop.send(&message) {
                Ok(reply) => reply,
                Err(error) => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message_of(&error)}})
                }
            }
        };
        output(reply.to_string());
    }
}

/// An error's text, for the MCP error an agent reads.
fn message_of(error: &Error) -> String {
    match error {
        Error::Cli(error) => error.message.clone(),
        Error::Core(error) => error.message.clone(),
        Error::Transport(error) => format!("{error:?}"),
    }
}

#[cfg(all(test, unix))]
#[path = "sessions_desktop_tests.rs"]
mod sessions_wait_desktop_tests;
