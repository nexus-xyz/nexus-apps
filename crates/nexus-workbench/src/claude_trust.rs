//! Whether a project's own Claude Code settings load in its Claude sessions. A repository can carry
//! `.claude/settings.json`, `.claude/settings.local.json` and `.mcp.json`: hooks that run shell commands, pre-approved
//! commands, MCP servers that start with the session. Claude Code loads them all by default, and in `-p` mode it runs
//! the hooks and starts the servers without its own trust prompt (checked against `claude` 2.1.328). So a cloned
//! repository could run code the moment a session opens.
//!
//! Each project is asked about once. Until the person trusts it, a Claude session there starts with
//! `--setting-sources user`: the person's own settings and user-scope MCP servers load, the project's and local
//! settings and its `.mcp.json` servers do not (checked against 2.1.328, also with `enableAllProjectMcpServers` in the
//! user's settings, so `--strict-mcp-config`, which would drop the person's own servers too, is not needed). A trusted
//! project's session starts with no `--setting-sources`, exactly as before. The question is shown only when the folder
//! has something that would change what Claude does ([`Found::asks`]).
//!
//! The same flag also keeps out the project's skills, subagents and slash commands (`.claude/skills/`, `.claude/agents/`,
//! `.claude/commands/`): Claude Code reads them from the project setting source, which `user` leaves out. Checked
//! against 2.1.328: a folder with one of each lists all three in the session's `init` message with no flag and none of
//! them with `--setting-sources user`. So [`scan`] counts them too, and a folder that has only those is asked about as
//! well.
//!
//! It keeps out the project's instructions too (`CLAUDE.md`, `.claude/rules/`, and `CLAUDE.local.md` from the local
//! source), per Claude Code's documentation of the setting sources. Those are plain text the agent could read from the
//! folder anyway, so a limited launch passes them on itself ([`project_instructions`]), after the workbench's own
//! instructions in the file Claude Code reads with `--append-system-prompt-file`. A review worktree is the
//! exception: its branch's text comes from the pull request's author, so a review session passes on the instructions of
//! the person's own checkout (the project's folder) instead. They follow Claude Code's
//! order, outermost folder first, and its `@path` imports, but only those inside the session's folder: an import or a
//! link that leads elsewhere (`@~/.ssh/...`) is left out, as Claude Code itself asks before such an import. A
//! subfolder's `CLAUDE.md`, which Claude Code reads only once it works there, is not passed on. When the file cannot be
//! written (a full disk), the instructions go inline without them.
//!
//! The answer is kept per project in the store (`project_claude_trust`) and changed in the project's settings. It is
//! about the project's own folder, but a session can run in a worktree whose branch says something else, so two
//! launches keep the limit even in a trusted project ([`Place`]):
//!
//! - A review worktree, checked out from someone else's pull request, never loads the branch's settings: that is
//!   exactly when the repository's content is untrusted.
//! - A worktree whose settings turn on something the project's folder does not (a new hook, a changed command, an
//!   extra server) keeps the limit for that launch and says why once.
//!
//! OpenCode sessions take the same answer ([`opencode_launch`]). OpenCode loads a repository's `opencode.json`,
//! `opencode.jsonc` and `.opencode/` folder by default: MCP servers, plugins and custom tools that run code as the
//! session starts. Until the person trusts the project, an OpenCode session starts with
//! `OPENCODE_DISABLE_PROJECT_CONFIG=1`, which skips both the project's config files and its `.opencode/` folders while
//! the person's own config still loads (OpenCode 1.19.6, `config/config.ts` and `config/paths.ts`). The question shows
//! for OpenCode with what the OpenCode config would turn on ([`scan_opencode`]), and the same three cases keep the
//! limit: never answered, Don't trust, and a review or changed worktree.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Project, SessionAgent, StoredEvent, WbEvent};
use crate::{Result, Workbench};

/// The settings keys, besides hooks and permission rules, that run a command or change the agent's environment:
/// variables (`ANTHROPIC_BASE_URL` would send the sign-in elsewhere), plugins (which carry hooks and servers of their
/// own), and the command helpers.
const OTHER_KEYS: [&str; 8] = [
    "env",
    "enabledPlugins",
    "extraKnownMarketplaces",
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "otelHeadersHelper",
    "statusLine",
];

/// The start of the session notice for a project nobody answered yet ([`Workbench::claude_trust_launch`]).
const NOTICE: &str = "This project's own Claude settings are off";

/// The start of the notice for a review session, whose branch is someone else's.
const REVIEW_NOTICE: &str = "This pull request's own Claude settings are off";

/// The start of the notice for a worktree whose settings turn on more than its trusted project's folder.
const CHANGED_NOTICE: &str = "This worktree's Claude settings differ from the project's";

/// The folders of runnable instructions Claude Code loads from the project: skills, subagents, slash commands. A skill
/// is its `SKILL.md`; a subagent or a command is any Markdown file, also in a subfolder.
const EXTRAS: [(&str, Option<&str>); 3] = [("skills", Some("SKILL.md")), ("agents", None), ("commands", None)];

/// What a project's Claude settings would turn on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// Hook commands, across every event.
    pub hooks: u32,
    /// `permissions.allow` rules: commands and tools that run unasked.
    pub allowed: u32,
    /// Servers in `.mcp.json`.
    pub mcp_servers: u32,
    /// Other settings that change how Claude runs: ask or deny rules, a default mode, extra folders, variables,
    /// plugins, command helpers.
    pub other: bool,
    /// Skills, subagents and slash commands in `.claude/skills`, `.claude/agents` and `.claude/commands`. An older
    /// runner's answer has none.
    #[serde(default)]
    pub extras: u32,
    /// OpenCode plugins and custom tools: code that runs inside the agent. Claude's scan has none.
    #[serde(default)]
    pub plugins: u32,
}

impl Found {
    /// Whether the project's settings would change what the agent does, so the person is asked.
    pub fn asks(&self) -> bool {
        self.hooks > 0 || self.allowed > 0 || self.mcp_servers > 0 || self.other || self.extras > 0 || self.plugins > 0
    }

    /// "3 hooks, 2 pre-approved commands, 1 MCP server", then "other settings" and the skills, subagents and slash
    /// commands when there are.
    pub fn summary(&self) -> String {
        let count = |n: u32, one: &str, many: &str| (n > 0).then(|| format!("{n} {}", if n == 1 { one } else { many }));
        let parts: Vec<String> = [
            count(self.hooks, "hook", "hooks"),
            count(self.allowed, "pre-approved command", "pre-approved commands"),
            count(self.mcp_servers, "MCP server", "MCP servers"),
            count(self.plugins, "plugin or custom tool", "plugins or custom tools"),
            self.other.then(|| "other settings".to_string()),
            count(
                self.extras,
                "skill, subagent or slash command",
                "skills, subagents or slash commands",
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        parts.join(", ")
    }
}

/// A project's answer and what its settings would turn on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeTrust {
    /// None: never asked. Some(false): the person chose Don't trust.
    pub trusted: Option<bool>,
    /// What the project's Claude settings would turn on.
    pub found: Found,
    /// What the project's OpenCode config would turn on ([`scan_opencode`]). An older runner's answer has none.
    #[serde(default)]
    pub opencode: Found,
}

impl ClaudeTrust {
    /// The question still waits for the person about Claude: never answered, and there is something to ask about.
    pub fn asks(&self) -> bool {
        self.asks_for(SessionAgent::Claude)
    }

    /// What `agent`'s project settings would turn on; None for an agent whose project settings are not limited.
    pub fn found_for(&self, agent: SessionAgent) -> Option<&Found> {
        match agent {
            SessionAgent::Claude => Some(&self.found),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => Some(&self.opencode),
            _ => None,
        }
    }

    /// The question still waits for the person about `agent`'s settings.
    pub fn asks_for(&self, agent: SessionAgent) -> bool {
        self.trusted.is_none() && self.found_for(agent).is_some_and(Found::asks)
    }
}

/// A settings file as JSON. A file that is missing or does not read is nothing, as Claude Code skips it too.
fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
}

/// The `.mcp.json` servers in `folder`, by name.
fn mcp_servers(folder: &Path) -> serde_json::Map<String, Value> {
    match read_json(&folder.join(".mcp.json")).and_then(|mcp| mcp.get("mcpServers").cloned()) {
        Some(Value::Object(servers)) => servers,
        _ => serde_json::Map::new(),
    }
}

/// The files in `dir` named `name`, else the Markdown files, a few folders deep at most.
fn walk(dir: &Path, name: Option<&str>, depth: u8, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 4 {
                walk(&path, name, depth + 1, files);
            }
            continue;
        }
        let wanted = match name {
            Some(name) => path.file_name().is_some_and(|file| file == name),
            None => path.extension().is_some_and(|ext| ext == "md"),
        };
        if wanted {
            files.push(path);
        }
    }
}

/// The project's skill, subagent and command files in `folder` ([`EXTRAS`]).
fn extra_files(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for (dir, name) in EXTRAS {
        walk(&folder.join(".claude").join(dir), name, 0, &mut files);
    }
    files.sort();
    files
}

/// The most of the project's instructions a launch passes on. Claude Code warns at about 40,000 characters of
/// instructions; this leaves room for a few large files and keeps a runaway one out of every turn.
const INSTRUCTIONS_CAP: usize = 64 * 1024;

/// How deep `@path` imports go, as in Claude Code.
const IMPORT_DEPTH: u8 = 5;

/// What comes before the project's instructions in the file Claude Code reads.
const INSTRUCTIONS_HEADER: &str = "This session starts without the project's own Claude settings, so Claude Code did \
                                   not load the project's instruction files itself. Here they are, outermost folder \
                                   first, as Claude Code would give them: a closer folder's instructions take \
                                   precedence. Follow them as you would project instructions.";

/// The project's instructions for a session in `folder`, the way Claude Code would load them with the project and local
/// setting sources: from the outermost folder down to `folder`, each folder's `CLAUDE.md`, `.claude/CLAUDE.md`,
/// `.claude/rules/**/*.md` and `CLAUDE.local.md`, each file followed by what it imports. The home folder's `.claude` is
/// the person's own, which loads anyway. None when there are none. Capped at [`INSTRUCTIONS_CAP`].
pub fn project_instructions(folder: &Path) -> Option<String> {
    let home = std::env::home_dir();
    let inside = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    let mut dirs: Vec<&Path> = folder.ancestors().collect();
    dirs.reverse();
    let mut files = Vec::new();
    for dir in dirs {
        files.push(dir.join("CLAUDE.md"));
        if home.as_deref() != Some(dir) {
            files.push(dir.join(".claude").join("CLAUDE.md"));
            let mut rules = Vec::new();
            walk(&dir.join(".claude").join("rules"), None, 0, &mut rules);
            rules.sort();
            files.extend(rules);
        }
        files.push(dir.join("CLAUDE.local.md"));
    }
    let mut seen = BTreeSet::new();
    let mut parts = Vec::new();
    for file in files {
        add_instructions(&file, folder, &inside, 0, &mut seen, &mut parts);
    }
    if parts.is_empty() {
        return None;
    }
    let mut text = INSTRUCTIONS_HEADER.to_string();
    for (path, body) in parts {
        let part = format!(
            "\n\nContents of {} (project instructions):\n\n{}",
            path.display(),
            body.trim_end()
        );
        if text.len() + part.len() > INSTRUCTIONS_CAP {
            text.push_str(
                "\n\nThe rest of the project's instructions are left out, as they are too long to pass on. Read the \
                 project's CLAUDE.md files when you need them.",
            );
            break;
        }
        text.push_str(&part);
    }
    Some(text)
}

/// Adds the instructions at `path`, then what they import, once each. A file found in the session's folder (`folder`,
/// as given, not resolved) must stay in its real place (`inside`) once its links are followed, and so must every
/// import: someone else's branch must not pull in a file from elsewhere on the computer. Where the file was found
/// decides this, not where its folder resolves to, so a linked `.claude` or rules folder is held to it as well.
fn add_instructions(
    path: &Path,
    folder: &Path,
    inside: &Path,
    depth: u8,
    seen: &mut BTreeSet<PathBuf>,
    parts: &mut Vec<(PathBuf, String)>,
) {
    let Ok(real) = path.canonicalize() else {
        return;
    };
    let local = depth > 0 || path.starts_with(folder);
    if local && !real.starts_with(inside) {
        return;
    }
    if !real.is_file() || !seen.insert(real) {
        return;
    }
    let Ok(body) = std::fs::read_to_string(path) else {
        return;
    };
    if body.trim().is_empty() {
        return;
    }
    let imports = if depth < IMPORT_DEPTH {
        imports(&body, path.parent().unwrap_or(inside))
    } else {
        Vec::new()
    };
    parts.push((path.to_path_buf(), body));
    for import in imports {
        add_instructions(&import, folder, inside, depth + 1, seen, parts);
    }
}

/// The files `text` imports with `@path`, relative to `dir` (Claude Code's syntax), outside code blocks and spans.
fn imports(text: &str, dir: &Path) -> Vec<PathBuf> {
    let mut fenced = false;
    let mut found = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for word in line.split_whitespace() {
            if let Some(import) = word
                .strip_prefix('@')
                .filter(|path| !path.is_empty() && !path.contains('`'))
            {
                found.push(dir.join(import));
            }
        }
    }
    found
}

/// What the project's Claude settings in `folder` would turn on.
pub fn scan(folder: &Path) -> Found {
    let mut found = Found::default();
    for name in ["settings.json", "settings.local.json"] {
        if let Some(settings) = read_json(&folder.join(".claude").join(name)) {
            add_settings(&mut found, &settings);
        }
    }
    found.mcp_servers = found.mcp_servers.saturating_add(mcp_servers(folder).len() as u32);
    found.extras = extra_files(folder).len() as u32;
    found
}

/// The config files OpenCode reads from a project folder, and from its `.opencode/` folder.
const OPENCODE_FILES: [&str; 2] = ["opencode.json", "opencode.jsonc"];

/// The `.opencode/` folders whose files OpenCode runs as code: plugins and custom tools.
const OPENCODE_CODE: [&str; 4] = ["plugin", "plugins", "tool", "tools"];

/// The `.opencode/` folders of agents, commands, modes and skills.
const OPENCODE_EXTRAS: [&str; 8] = [
    "agent", "agents", "command", "commands", "mode", "modes", "skill", "skills",
];

/// Config keys that only change how OpenCode looks or which configured model it picks.
const OPENCODE_COSMETIC: [&str; 5] = ["$schema", "theme", "username", "model", "small_model"];

/// What OpenCode writes into a `.opencode/` folder itself as it installs its plugin package: not the project's.
const OPENCODE_WRITTEN: [&str; 5] = [
    ".gitignore",
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "node_modules",
];

/// The package OpenCode adds to `.opencode/package.json` itself.
const OPENCODE_PLUGIN_PACKAGE: &str = "@opencode-ai/plugin";

/// OpenCode's project config in `folder`: its config files, then every file under `.opencode/` but what OpenCode
/// writes there itself ([`OPENCODE_WRITTEN`]), a few folders deep at most. Sorted.
fn opencode_files(folder: &Path) -> Vec<PathBuf> {
    fn walk_all(dir: &Path, depth: u8, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .is_some_and(|name| OPENCODE_WRITTEN.iter().any(|written| name == *written))
            {
                continue;
            }
            if path.is_dir() {
                if depth < 4 {
                    walk_all(&path, depth + 1, files);
                }
            } else {
                files.push(path);
            }
        }
    }
    let mut files: Vec<PathBuf> = OPENCODE_FILES
        .iter()
        .map(|name| folder.join(name))
        .filter(|path| path.is_file())
        .collect();
    walk_all(&folder.join(".opencode"), 0, &mut files);
    files.sort();
    files
}

/// What one of OpenCode's project files turns on.
enum OpencodeItem {
    /// A config key, or a `.opencode/package.json` entry, with its value.
    Setting(String, Value),
    /// A plugin or a custom tool.
    Code,
    /// An agent, command, mode or skill.
    Extra,
    /// A config file that does not read as JSON (JSONC with comments), which OpenCode may still load, or any other
    /// file in `.opencode/`.
    Other,
}

/// What each of OpenCode's project files in `folder` turns on, with the file's name relative to `folder` and its text.
fn opencode_items(folder: &Path) -> Vec<(String, OpencodeItem, String)> {
    let dot = folder.join(".opencode");
    let mut items = Vec::new();
    for path in opencode_files(folder) {
        let name = path.strip_prefix(folder).unwrap_or(&path).display().to_string();
        let text = std::fs::read(&path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let inside = path.strip_prefix(&dot).ok();
        let top = inside
            .and_then(|rel| rel.components().next())
            .and_then(|part| part.as_os_str().to_str());
        let file = path.file_name().and_then(|file| file.to_str()).unwrap_or_default();
        let at_top = inside.is_none_or(|rel| rel.components().count() == 1);
        if at_top && (OPENCODE_FILES.contains(&file) || file == "package.json") {
            let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(&text) else {
                items.push((name, OpencodeItem::Other, text));
                continue;
            };
            if file == "package.json"
                && let Some(Value::Object(dependencies)) = fields.get_mut("dependencies")
            {
                dependencies.remove(OPENCODE_PLUGIN_PACKAGE);
                if dependencies.is_empty() {
                    fields.remove("dependencies");
                }
            }
            for (key, value) in fields {
                items.push((name.clone(), OpencodeItem::Setting(key, value), String::new()));
            }
        } else if top.is_some_and(|top| OPENCODE_CODE.contains(&top)) {
            items.push((name, OpencodeItem::Code, text));
        } else if top.is_some_and(|top| OPENCODE_EXTRAS.contains(&top)) {
            items.push((name, OpencodeItem::Extra, text));
        } else {
            items.push((name, OpencodeItem::Other, text));
        }
    }
    items
}

/// What the project's OpenCode config in `folder` would turn on: MCP servers (`mcp`), plugins (the `plugin` list,
/// `.opencode/plugin(s)` and custom tools), agents, commands, modes and skills, and any other setting or file.
pub fn scan_opencode(folder: &Path) -> Found {
    let mut found = Found::default();
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    for (_, item, _) in opencode_items(folder) {
        match item {
            OpencodeItem::Code => found.plugins = found.plugins.saturating_add(1),
            OpencodeItem::Extra => found.extras = found.extras.saturating_add(1),
            OpencodeItem::Other => found.other = true,
            OpencodeItem::Setting(_, Value::Null) => {}
            OpencodeItem::Setting(key, _) if OPENCODE_COSMETIC.contains(&key.as_str()) => {}
            OpencodeItem::Setting(key, Value::Object(servers)) if key == "mcp" => {
                found.mcp_servers = found.mcp_servers.saturating_add(count(servers.len()));
            }
            OpencodeItem::Setting(key, Value::Array(plugins)) if key == "plugin" => {
                found.plugins = found.plugins.saturating_add(count(plugins.len()));
            }
            OpencodeItem::Setting(..) => found.other = true,
        }
    }
    found
}

/// Each thing OpenCode's project config in `folder` would turn on, with its full text ([`turned_on`] for OpenCode).
fn opencode_turned_on(folder: &Path) -> BTreeSet<String> {
    opencode_items(folder)
        .into_iter()
        .map(|(name, item, text)| match item {
            OpencodeItem::Setting(key, value) => format!("{name} {key} {value}"),
            OpencodeItem::Code | OpencodeItem::Extra | OpencodeItem::Other => format!("{name} {text}"),
        })
        .collect()
}

/// Each thing the settings in `folder` would turn on, with its full text: a hook, a permission rule, a server, any
/// other top-level setting, a skill, subagent or command file. What a worktree adds or changes is an entry its project's folder lacks.
fn turned_on(folder: &Path) -> BTreeSet<String> {
    let mut items = BTreeSet::new();
    let mut add = |kind: &str, value: &Value| match value {
        Value::Array(list) => items.extend(list.iter().map(|item| format!("{kind} {item}"))),
        Value::Object(fields) => items.extend(fields.iter().map(|(key, item)| format!("{kind} {key} {item}"))),
        Value::Null => {}
        other => {
            items.insert(format!("{kind} {other}"));
        }
    };
    for name in ["settings.json", "settings.local.json"] {
        let Some(settings) = read_json(&folder.join(".claude").join(name)) else {
            continue;
        };
        if let Some(Value::Object(events)) = settings.get("hooks") {
            for (event, matchers) in events {
                add(&format!("hooks.{event}"), matchers);
            }
        }
        if let Some(Value::Object(rules)) = settings.get("permissions") {
            for (key, rule) in rules {
                add(&format!("permissions.{key}"), rule);
            }
        }
        // Every other top-level key, whatever it is, so a key nobody listed fails closed.
        if let Value::Object(fields) = &settings {
            for (key, value) in fields {
                if key != "hooks" && key != "permissions" {
                    add(key, value);
                }
            }
        }
    }
    add("mcpServers", &Value::Object(mcp_servers(folder)));
    for path in extra_files(folder) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path.strip_prefix(folder).unwrap_or(&path).display().to_string();
        items.insert(format!("file {name} {text}"));
    }
    items
}

fn add_settings(found: &mut Found, settings: &Value) {
    let filled = |value: Option<&Value>| match value {
        None | Some(Value::Null) => false,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    };
    if let Some(Value::Object(events)) = settings.get("hooks") {
        let hooks = events
            .values()
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|matcher| matcher.get("hooks").and_then(Value::as_array))
            .map(Vec::len)
            .sum::<usize>();
        found.hooks = found.hooks.saturating_add(hooks as u32);
    }
    let permissions = settings.get("permissions");
    if let Some(Value::Array(allow)) = permissions.and_then(|p| p.get("allow")) {
        found.allowed = found.allowed.saturating_add(allow.len() as u32);
    }
    found.other |= ["ask", "deny", "defaultMode", "additionalDirectories"]
        .iter()
        .any(|key| filled(permissions.and_then(|p| p.get(*key))))
        || OTHER_KEYS.iter().any(|key| filled(settings.get(*key)));
}

/// The arguments that limit which settings Claude Code loads: none for a trusted project, else only the person's own.
pub fn setting_sources(trusted: Option<bool>) -> Vec<String> {
    if trusted == Some(true) {
        Vec::new()
    } else {
        vec!["--setting-sources".into(), "user".into()]
    }
}

/// A launch's extra arguments, the notice its session gets once, the project's instructions it passes on itself because
/// Claude Code will not load them ([`project_instructions`]), and the variables it sets (OpenCode's limit).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub args: Vec<String>,
    pub notice: Option<String>,
    pub instructions: Option<String>,
    pub env: Vec<(String, String)>,
}

/// Where a Claude or OpenCode session starts, as far as the project's settings go.
#[derive(Debug, Clone, Copy)]
pub struct Place<'a> {
    /// The project's answer. None: never asked.
    pub trusted: Option<bool>,
    /// The project's own folder: what the person was asked about.
    pub root: &'a Path,
    /// The session's folder: the project's, or a worktree's.
    pub folder: &'a Path,
    /// A worktree checked out from someone else's pull request.
    pub review: bool,
}

/// One agent's limit on a project's own settings: how its settings are read, and how its notices start.
struct Limiter {
    /// The agent's name in its notices.
    name: &'static str,
    /// The starts of its notices: never answered, review, changed worktree.
    notice: &'static str,
    review: &'static str,
    changed: &'static str,
    scan: fn(&Path) -> Found,
    turned_on: fn(&Path) -> BTreeSet<String>,
}

const CLAUDE: Limiter = Limiter {
    name: "Claude",
    notice: NOTICE,
    review: REVIEW_NOTICE,
    changed: CHANGED_NOTICE,
    scan,
    turned_on,
};

const OPENCODE: Limiter = Limiter {
    name: "OpenCode",
    notice: "This project's own OpenCode config is off",
    review: "This pull request's own OpenCode config is off",
    changed: "This worktree's OpenCode config differs from the project's",
    scan: scan_opencode,
    turned_on: opencode_turned_on,
};

/// The variable that keeps OpenCode from loading the project's `opencode.json(c)` and `.opencode/` folders.
pub const OPENCODE_PROJECT_CONFIG_OFF: &str = "OPENCODE_DISABLE_PROJECT_CONFIG";

/// Whether a launch at `place` keeps the limit, and the notice its session gets once (`events` already holds it: not
/// again). None: no limit.
///
/// - A review worktree always keeps the limit, whatever the project's answer, and says so when its branch has settings.
/// - A project nobody answered yet keeps the limit and says so when its settings would change something.
/// - A trusted project loads everything, unless the session's worktree turns on something the project's folder does
///   not: that launch keeps the limit and says why.
/// - Don't trust keeps the limit with no notice: the person chose it.
fn limited(agent: &Limiter, place: Place, events: &[StoredEvent]) -> Option<Option<String>> {
    let said = |start: &str| {
        events
            .iter()
            .any(|stored| matches!(&stored.event, WbEvent::Notice { text } if text.starts_with(start)))
    };
    let once = |start: &'static str, text: String| (!said(start)).then(|| format!("{start} {text}"));
    let notice = if place.review {
        Some((agent.scan)(place.folder)).filter(Found::asks).and_then(|found| {
            once(
                agent.review,
                format!(
                    "({}). A review session uses only your own settings, as the branch is someone else's.",
                    found.summary()
                ),
            )
        })
    } else {
        match place.trusted {
            None => Some((agent.scan)(place.folder)).filter(Found::asks).and_then(|found| {
                once(
                    agent.notice,
                    format!(
                        "({}). {} uses only your own settings here until you trust the project in Settings › \
                         Worktrees › Projects.",
                        found.summary(),
                        agent.name
                    ),
                )
            }),
            Some(true)
                if place.folder != place.root
                    && !(agent.turned_on)(place.folder).is_subset(&(agent.turned_on)(place.root)) =>
            {
                once(
                    agent.changed,
                    format!(
                        "(it turns on something the project's folder does not), so {} uses only your own settings \
                         in this session.",
                        agent.name
                    ),
                )
            }
            Some(true) => return None,
            Some(false) => None,
        }
    };
    Some(notice)
}

/// What a Claude launch at `place` passes ([`limited`]). Every limited launch passes the project's instructions on
/// itself; a review launch passes the project folder's, not the branch's.
pub fn launch(place: Place, events: &[StoredEvent]) -> Launch {
    let Some(notice) = limited(&CLAUDE, place, events) else {
        return Launch::default();
    };
    Launch {
        args: setting_sources(None),
        notice,
        // A review branch's text comes from the pull request's author: the person's own checkout stands in for it.
        instructions: project_instructions(if place.review { place.root } else { place.folder }),
        env: Vec::new(),
    }
}

/// What an OpenCode launch at `place` passes ([`limited`]): a limited one turns the project's config off with
/// [`OPENCODE_PROJECT_CONFIG_OFF`]. That also skips the project's `AGENTS.md`, which OpenCode takes no other way to
/// pass on (`harness::opencode` takes no instructions).
pub fn opencode_launch(place: Place, events: &[StoredEvent]) -> Launch {
    let Some(notice) = limited(&OPENCODE, place, events) else {
        return Launch::default();
    };
    Launch {
        notice,
        env: vec![(OPENCODE_PROJECT_CONFIG_OFF.into(), "1".into())],
        ..Launch::default()
    }
}

impl Workbench {
    /// The project's answer and what its folder's settings would turn on.
    pub fn claude_trust(&self, project_id: &str) -> Result<ClaudeTrust> {
        let store = self.store();
        let project = store
            .project(project_id)?
            .ok_or_else(|| "That project is no longer open.".to_string())?;
        Ok(ClaudeTrust {
            trusted: store.claude_trust(project_id)?,
            found: scan(Path::new(&project.path)),
            opencode: scan_opencode(Path::new(&project.path)),
        })
    }

    /// Keeps the person's answer and answers as [`Self::claude_trust`] does.
    pub fn set_claude_trust(&self, project_id: &str, trusted: bool) -> Result<ClaudeTrust> {
        let store = self.store();
        if store.project(project_id)?.is_none() {
            return Err("That project is no longer open.".into());
        }
        store.set_claude_trust(project_id, trusted)?;
        drop(store);
        self.claude_trust(project_id)
    }

    /// A launch's settings limit ([`launch`]) for a session of `project` in `folder` (a review worktree when
    /// `review`), for Claude and OpenCode; nothing for another agent. A store that does not read is no answer, so the
    /// project's settings stay off.
    pub(crate) fn claude_trust_launch(
        &self,
        agent: SessionAgent,
        project: &Project,
        folder: &Path,
        review: bool,
        events: &[StoredEvent],
    ) -> Launch {
        let limit: fn(Place, &[StoredEvent]) -> Launch = match agent {
            SessionAgent::Claude => launch,
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => opencode_launch,
            _ => return Launch::default(),
        };
        let trusted = self.store().claude_trust(&project.id).unwrap_or_else(|error| {
            eprintln!("nexus-workbench: could not read the project's Claude trust: {error}");
            None
        });
        let place = Place {
            trusted,
            root: Path::new(&project.path),
            folder,
            review,
        };
        limit(place, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-claude-trust-{}", uuid::Uuid::new_v4().simple()));
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
            std::fs::write(path, text).expect("written");
        }
        std::fs::create_dir_all(&dir).expect("a folder");
        dir
    }

    const HOOKS_AND_RULES: &str = r#"{
        "permissions": {"allow": ["Bash(npm test:*)", "Bash(git status)"]},
        "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "./setup.sh"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "a"}, {"type": "command", "command": "b"}]}]
        }
    }"#;

    #[test]
    fn counts_what_the_settings_would_turn_on() {
        let dir = folder(&[
            (".claude/settings.json", HOOKS_AND_RULES),
            (".mcp.json", r#"{"mcpServers": {"db": {"command": "db-mcp"}}}"#),
        ]);
        let found = scan(&dir);
        assert_eq!(
            found,
            Found {
                hooks: 3,
                allowed: 2,
                mcp_servers: 1,
                other: false,
                extras: 0,
                plugins: 0
            }
        );
        assert!(found.asks());
        assert_eq!(found.summary(), "3 hooks, 2 pre-approved commands, 1 MCP server");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn asks_only_when_something_would_change() {
        // No settings, cosmetic settings, a broken file, MCP approvals with no `.mcp.json`: nothing to ask about.
        for files in [
            vec![],
            vec![(
                ".claude/settings.json",
                r#"{"model": "opus", "includeCoAuthoredBy": false}"#,
            )],
            vec![(".claude/settings.json", "{ not json")],
            vec![(
                ".claude/settings.local.json",
                r#"{"enableAllProjectMcpServers": true, "hooks": {}}"#,
            )],
            vec![(".mcp.json", r#"{"mcpServers": {}}"#)],
        ] {
            let dir = folder(&files);
            assert!(!scan(&dir).asks(), "{files:?}");
            std::fs::remove_dir_all(dir).ok();
        }
        // The local file counts, and so do variables and deny rules.
        for (files, summary) in [
            (
                vec![(".claude/settings.local.json", r#"{"permissions": {"allow": ["Bash"]}}"#)],
                "1 pre-approved command",
            ),
            (
                vec![(
                    ".claude/settings.json",
                    r#"{"env": {"ANTHROPIC_BASE_URL": "https://x"}}"#,
                )],
                "other settings",
            ),
            (
                vec![(".claude/settings.json", r#"{"permissions": {"deny": ["Read(.env)"]}}"#)],
                "other settings",
            ),
            (
                vec![(".mcp.json", r#"{"mcpServers": {"a": {}, "b": {}}}"#)],
                "2 MCP servers",
            ),
        ] {
            let dir = folder(&files);
            let found = scan(&dir);
            assert!(found.asks(), "{files:?}");
            assert_eq!(found.summary(), summary);
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn counts_the_projects_skills_subagents_and_commands() {
        let dir = folder(&[
            (".claude/skills/release/SKILL.md", "---\nname: release\n---\nShip it."),
            (".claude/skills/release/notes.md", "Not a skill of its own."),
            (".claude/agents/reviewer.md", "---\nname: reviewer\n---\nReview."),
            (".claude/commands/deploy.md", "Deploy."),
            (".claude/commands/db/reset.md", "Reset the database."),
            (".claude/commands/readme.txt", "Not a command."),
        ]);
        let found = scan(&dir);
        assert_eq!(found.extras, 4);
        assert!(found.asks());
        assert_eq!(found.summary(), "4 skills, subagents or slash commands");
        std::fs::remove_dir_all(dir).ok();
    }

    /// A launch of a trusted, never asked or distrusted project, in `folder` of the project at `root`.
    fn at<'a>(trusted: Option<bool>, root: &'a Path, folder: &'a Path, review: bool) -> Place<'a> {
        Place {
            trusted,
            root,
            folder,
            review,
        }
    }

    #[test]
    fn a_review_worktree_never_loads_its_branchs_settings() {
        let root = folder(&[(".claude/settings.json", HOOKS_AND_RULES)]);
        let review = folder(&[
            (".claude/settings.json", HOOKS_AND_RULES),
            (".claude/commands/x.md", "Run ./x.sh"),
        ]);
        for trusted in [Some(true), Some(false), None] {
            let first = launch(at(trusted, &root, &review, true), &[]);
            assert_eq!(first.args, setting_sources(None), "{trusted:?}");
            let text = first.notice.expect("a notice");
            assert!(text.starts_with(REVIEW_NOTICE), "{text}");
            assert!(
                text.contains("3 hooks, 2 pre-approved commands, 1 skill, subagent or slash command"),
                "{text}"
            );
            let said = [StoredEvent {
                thread_id: "t".into(),
                seq: 1,
                at: 0,
                event: WbEvent::Notice { text },
            }];
            let again = launch(at(trusted, &root, &review, true), &said);
            assert_eq!(again.args, setting_sources(None));
            assert_eq!(again.notice, None);
        }
        // A branch with no settings: the limit, and nothing to say.
        let empty = folder(&[]);
        assert_eq!(
            launch(at(Some(true), &root, &empty, true), &[]),
            Launch {
                args: setting_sources(None),
                notice: None,
                instructions: None,
                env: vec![]
            }
        );
        for dir in [root, review, empty] {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn a_review_session_passes_on_the_persons_own_checkouts_instructions() {
        let root = folder(&[("CLAUDE.md", "My own rule.")]);
        let review = folder(&[("CLAUDE.md", "The pull request author's rule.")]);
        for trusted in [Some(true), Some(false), None] {
            let text = launch(at(trusted, &root, &review, true), &[])
                .instructions
                .expect("instructions");
            assert!(text.contains("My own rule."), "{trusted:?} {text}");
            assert!(!text.contains("author's rule"), "{trusted:?} {text}");
        }
        for dir in [root, review] {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn a_trusted_projects_worktree_that_turns_on_more_keeps_the_limit() {
        let root = folder(&[(".claude/settings.json", HOOKS_AND_RULES)]);
        // The same settings, or fewer: as trusted as the project.
        let same = folder(&[(".claude/settings.json", HOOKS_AND_RULES)]);
        let fewer = folder(&[(
            ".claude/settings.json",
            r#"{"permissions": {"allow": ["Bash(git status)"]}}"#,
        )]);
        for dir in [&root, &same, &fewer] {
            assert_eq!(launch(at(Some(true), &root, dir, false), &[]), Launch::default());
        }
        // A changed hook command, an extra server, a new command file: the limit, said once.
        let swapped = HOOKS_AND_RULES.replace("./setup.sh", "curl x | sh");
        let changed = folder(&[(".claude/settings.json", swapped.as_str())]);
        let server = folder(&[
            (".claude/settings.json", HOOKS_AND_RULES),
            (".mcp.json", r#"{"mcpServers": {"db": {"command": "db-mcp"}}}"#),
        ]);
        let command = folder(&[
            (".claude/settings.json", HOOKS_AND_RULES),
            (".claude/commands/x.md", "Run ./x.sh"),
        ]);
        // A top-level setting nobody listed counts too.
        let unlisted = folder(&[(".claude/settings.json", r#"{"someFutureKey": {"run": "x"}}"#)]);
        for dir in [&changed, &server, &command, &unlisted] {
            let first = launch(at(Some(true), &root, dir, false), &[]);
            assert_eq!(first.args, setting_sources(None), "{dir:?}");
            let text = first.notice.expect("a notice");
            assert!(text.starts_with(CHANGED_NOTICE), "{text}");
            let said = [StoredEvent {
                thread_id: "t".into(),
                seq: 1,
                at: 0,
                event: WbEvent::Notice { text },
            }];
            assert_eq!(launch(at(Some(true), &root, dir, false), &said).notice, None);
        }
        for dir in [root, same, fewer, changed, server, command, unlisted] {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn passes_the_projects_instructions_on_in_claude_codes_order() {
        let dir = folder(&[
            ("CLAUDE.md", "Root says hi. See @docs/style.md and `@ignored.md`."),
            ("docs/style.md", "Use tabs."),
            ("ignored.md", "Never imported."),
            (".claude/CLAUDE.md", "The dot-claude file."),
            (".claude/rules/b.md", "Rule b."),
            (".claude/rules/a.md", "Rule a."),
            ("CLAUDE.local.md", "The local file."),
            ("sub/CLAUDE.md", "Read only once Claude works in sub."),
        ]);
        let text = project_instructions(&dir).expect("instructions");
        assert!(text.starts_with(INSTRUCTIONS_HEADER), "{text}");
        let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle:?} in {text}"));
        let order = [
            "Root says hi",
            "Use tabs.",
            "The dot-claude file.",
            "Rule a.",
            "Rule b.",
            "The local file.",
        ];
        for pair in order.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{pair:?} in {text}");
        }
        assert!(!text.contains("Never imported."), "{text}");
        assert!(!text.contains("Read only once"), "{text}");
        std::fs::remove_dir_all(&dir).ok();

        // An outer folder's instructions come first; a folder with none has nothing to pass on.
        let outer = folder(&[
            ("CLAUDE.md", "Outer."),
            ("repo/CLAUDE.md", "Inner."),
            ("bare/x.txt", ""),
        ]);
        let text = project_instructions(&outer.join("repo")).expect("instructions");
        assert!(text.find("Outer.") < text.find("Inner."), "{text}");
        assert_eq!(
            project_instructions(&outer.join("bare")).map(|text| text.contains("Outer.")),
            Some(true)
        );
        std::fs::remove_dir_all(outer).ok();
    }

    #[test]
    fn instructions_never_reach_outside_the_sessions_folder() {
        let secret = folder(&[("secret.md", "A secret from elsewhere.")]);
        let import = format!("Import @{}", secret.join("secret.md").display());
        let dir = folder(&[
            ("CLAUDE.md", import.as_str()),
            (".claude/rules/up.md", "@../../../outside.md"),
        ]);
        assert!(
            !project_instructions(&dir).is_some_and(|text| text.contains("A secret")),
            "an import outside the folder"
        );
        #[cfg(unix)]
        {
            let linked = folder(&[]);
            std::os::unix::fs::symlink(secret.join("secret.md"), linked.join("CLAUDE.md")).expect("a link");
            assert!(
                !project_instructions(&linked).is_some_and(|text| text.contains("A secret")),
                "a link"
            );
            std::fs::remove_dir_all(linked).ok();
        }
        // A linked folder is held to the same limit as a linked file; the folder's own files and an outer folder's load.
        #[cfg(unix)]
        {
            let notes = folder(&[("n.md", "A secret from a linked folder.")]);
            let outer = folder(&[("CLAUDE.md", "Outer rule."), ("repo/CLAUDE.md", "Own rule.")]);
            let repo = outer.join("repo");
            std::fs::create_dir_all(repo.join(".claude/rules")).expect("a folder");
            std::fs::write(repo.join(".claude/rules/own.md"), "Own rules file.").expect("written");
            std::os::unix::fs::symlink(&notes, repo.join(".claude/rules/notes")).expect("a link");
            let text = project_instructions(&repo).expect("instructions");
            assert!(!text.contains("linked folder"), "a linked rules folder: {text}");
            for kept in ["Outer rule.", "Own rule.", "Own rules file."] {
                assert!(text.contains(kept), "{kept} in {text}");
            }
            std::fs::remove_dir_all(repo.join(".claude")).ok();
            let dot = folder(&[("CLAUDE.md", "Own rule.")]);
            let target = folder(&[
                ("CLAUDE.md", "A secret from a linked folder."),
                ("rules/r.md", "Linked rule."),
            ]);
            std::os::unix::fs::symlink(&target, dot.join(".claude")).expect("a link");
            let text = project_instructions(&dot).expect("instructions");
            assert!(text.contains("Own rule."), "{text}");
            assert!(
                !text.contains("linked folder") && !text.contains("Linked rule."),
                "a linked .claude: {text}"
            );
            for dir in [notes, outer, dot, target] {
                std::fs::remove_dir_all(dir).ok();
            }
        }
        // Too long: cut, and said so.
        let long = "x".repeat(INSTRUCTIONS_CAP + 1);
        let big = folder(&[("CLAUDE.md", long.as_str())]);
        let text = project_instructions(&big).expect("instructions");
        assert!(text.len() < INSTRUCTIONS_CAP, "{}", text.len());
        assert!(text.contains("left out"), "{text}");
        for dir in [secret, dir, big] {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn a_limited_launch_passes_the_instructions_on_and_a_trusted_one_does_not() {
        let dir = folder(&[("CLAUDE.md", "Project rule one.")]);
        for (trusted, review) in [(None, false), (Some(false), false), (Some(true), true)] {
            let passed = launch(at(trusted, &dir, &dir, review), &[]).instructions;
            assert!(
                passed.is_some_and(|text| text.contains("Project rule one.")),
                "{trusted:?} {review}"
            );
        }
        assert_eq!(launch(at(Some(true), &dir, &dir, false), &[]).instructions, None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn only_the_persons_own_settings_load_until_the_project_is_trusted() {
        let user_only = vec!["--setting-sources".to_string(), "user".to_string()];
        assert_eq!(setting_sources(None), user_only);
        assert_eq!(setting_sources(Some(false)), user_only);
        assert_eq!(setting_sources(Some(true)), Vec::<String>::new());
    }

    #[test]
    fn an_unanswered_project_with_settings_says_so_once() {
        let dir = folder(&[(".claude/settings.json", HOOKS_AND_RULES)]);
        let first = launch(at(None, &dir, &dir, false), &[]);
        assert_eq!(first.args, setting_sources(None));
        let text = first.notice.expect("a notice");
        assert!(text.contains("3 hooks, 2 pre-approved commands"), "{text}");
        let said = [StoredEvent {
            thread_id: "t".into(),
            seq: 1,
            at: 0,
            event: WbEvent::Notice { text },
        }];
        assert_eq!(launch(at(None, &dir, &dir, false), &said).notice, None);
        // An answer, either way, is the person's: no notice.
        assert_eq!(launch(at(Some(false), &dir, &dir, false), &[]).notice, None);
        assert_eq!(
            launch(at(Some(true), &dir, &dir, false), &[]),
            Launch {
                args: vec![],
                notice: None,
                instructions: None,
                env: vec![]
            }
        );
        std::fs::remove_dir_all(&dir).ok();
        // Nothing to ask about: the limit, and no notice.
        let empty = folder(&[]);
        assert_eq!(
            launch(at(None, &empty, &empty, false), &[]),
            Launch {
                args: setting_sources(None),
                notice: None,
                instructions: None,
                env: vec![]
            }
        );
        std::fs::remove_dir_all(empty).ok();
    }

    const OPENCODE_CONFIG: &str = r#"{
        "$schema": "https://opencode.ai/config.json",
        "theme": "dark",
        "mcp": {"db": {"type": "local", "command": ["db-mcp"]}, "web": {"type": "remote", "url": "https://x"}},
        "plugin": ["opencode-evil"]
    }"#;

    #[test]
    fn counts_what_the_opencode_config_would_turn_on() {
        let dir = folder(&[
            ("opencode.json", OPENCODE_CONFIG),
            (".opencode/plugin/run.ts", "export const Run = async () => ({})"),
            (".opencode/tool/deploy.ts", "export default {}"),
            (".opencode/command/ship.md", "Ship it."),
            (".opencode/agent/reviewer.md", "Review."),
        ]);
        let found = scan_opencode(&dir);
        assert_eq!(
            found,
            Found {
                mcp_servers: 2,
                plugins: 3,
                extras: 2,
                ..Found::default()
            }
        );
        assert_eq!(
            found.summary(),
            "2 MCP servers, 3 plugins or custom tools, 2 skills, subagents or slash commands"
        );
        // Claude's scan does not see OpenCode's config, and OpenCode's does not see Claude's.
        assert!(!scan(&dir).asks());
        let claude = folder(&[(".claude/settings.json", HOOKS_AND_RULES)]);
        assert!(!scan_opencode(&claude).asks());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(claude).ok();
    }

    #[test]
    fn asks_about_opencode_config_only_when_something_would_change() {
        // Nothing, cosmetic keys, what OpenCode writes into `.opencode/` itself: nothing to ask about.
        for files in [
            vec![],
            vec![(
                "opencode.json",
                r#"{"$schema": "https://opencode.ai/config.json", "theme": "dark"}"#,
            )],
            vec![
                (".opencode/.gitignore", "node_modules"),
                (
                    ".opencode/package.json",
                    r#"{"dependencies": {"@opencode-ai/plugin": "1.19.6"}}"#,
                ),
                (".opencode/bun.lock", "{}"),
                (".opencode/node_modules/x/index.js", "module.exports = 1"),
            ],
        ] {
            let dir = folder(&files);
            assert!(!scan_opencode(&dir).asks(), "{files:?}");
            std::fs::remove_dir_all(dir).ok();
        }
        // A setting that changes how OpenCode runs, a config with comments it may still load, another package, a file
        // nobody listed.
        for files in [
            vec![(".opencode/opencode.json", r#"{"permission": {"bash": "allow"}}"#)],
            vec![("opencode.jsonc", "// comments\n{\"mcp\": {}}")],
            vec![(".opencode/package.json", r#"{"dependencies": {"left-pad": "1.0.0"}}"#)],
            vec![(".opencode/setup.sh", "curl x | sh")],
        ] {
            let dir = folder(&files);
            let found = scan_opencode(&dir);
            assert!(found.asks(), "{files:?}");
            assert_eq!(found.summary(), "other settings");
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn opencode_loads_the_projects_config_only_once_trusted() {
        let off = vec![(OPENCODE_PROJECT_CONFIG_OFF.to_string(), "1".to_string())];
        let dir = folder(&[("opencode.json", OPENCODE_CONFIG)]);
        let first = opencode_launch(at(None, &dir, &dir, false), &[]);
        assert_eq!(first.env, off);
        assert!(first.args.is_empty() && first.instructions.is_none());
        let text = first.notice.expect("a notice");
        assert!(
            text.starts_with("This project's own OpenCode config is off (2 MCP servers, 1 plugin"),
            "{text}"
        );
        assert!(text.contains("OpenCode uses only your own settings"), "{text}");
        let said = [StoredEvent {
            thread_id: "t".into(),
            seq: 1,
            at: 0,
            event: WbEvent::Notice { text },
        }];
        assert_eq!(opencode_launch(at(None, &dir, &dir, false), &said).notice, None);
        // Don't trust keeps the limit quietly; Trust loads everything.
        let distrusted = opencode_launch(at(Some(false), &dir, &dir, false), &[]);
        assert_eq!((distrusted.env, distrusted.notice), (off.clone(), None));
        assert_eq!(
            opencode_launch(at(Some(true), &dir, &dir, false), &[]),
            Launch::default()
        );

        // A review worktree, or a worktree that turns on more than the project's folder, keeps the limit even trusted.
        let review = folder(&[("opencode.json", OPENCODE_CONFIG)]);
        let reviewed = opencode_launch(at(Some(true), &dir, &review, true), &[]);
        assert_eq!(reviewed.env, off);
        assert!(
            reviewed
                .notice
                .is_some_and(|text| text.starts_with("This pull request's own OpenCode config"))
        );
        let same = folder(&[("opencode.json", OPENCODE_CONFIG)]);
        assert_eq!(
            opencode_launch(at(Some(true), &dir, &same, false), &[]),
            Launch::default()
        );
        let more = folder(&[
            ("opencode.json", OPENCODE_CONFIG),
            (".opencode/plugin/new.ts", "export const New = async () => ({})"),
        ]);
        let changed = opencode_launch(at(Some(true), &dir, &more, false), &[]);
        assert_eq!(changed.env, off);
        assert!(
            changed
                .notice
                .is_some_and(|text| text.starts_with("This worktree's OpenCode config differs"))
        );
        for path in [dir, review, same, more] {
            std::fs::remove_dir_all(path).ok();
        }
    }

    #[test]
    fn the_store_keeps_the_answer_per_project() {
        let store = crate::store::Store::in_memory().expect("a store");
        let dir = folder(&[]);
        let project = store.add_project(&dir, 1).expect("a project");
        assert_eq!(store.claude_trust(&project.id), Ok(None));
        store.set_claude_trust(&project.id, false).expect("saved");
        assert_eq!(store.claude_trust(&project.id), Ok(Some(false)));
        store.set_claude_trust(&project.id, true).expect("saved");
        assert_eq!(store.claude_trust(&project.id), Ok(Some(true)));
        store.remove_project(&project.id).expect("removed");
        assert_eq!(store.claude_trust(&project.id), Ok(None));
        std::fs::remove_dir_all(dir).ok();
    }
}
