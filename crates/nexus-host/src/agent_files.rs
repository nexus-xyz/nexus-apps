//! What a person puts in the host's folder for the agents it starts: `agent.env` (credentials) and `mcp.json` (tools).
//!
//! A cloud machine is a VM running this host. The desktop app writes the person's
//! Claude token and GitHub token into `<root>/agent.env` each time it connects, and a browser tool into
//! `<root>/mcp.json`. They are files, and not host settings, for three reasons: the host is a long-lived process, and a
//! token replaced or removed in the app has to reach the very next session without restarting it; a token passed as an
//! argument would show in the process list; and a person who set the machine up by hand can use the same files.
//!
//! Both are read fresh at every agent start ([`AgentFiles::apply`]), never cached.
//!
//! # `agent.env`
//!
//! `KEY=VALUE` lines. Blank lines and lines starting with `#` are skipped, `export ` before a key is allowed, and a
//! value may be wrapped in one pair of single or double quotes. Nothing else is interpreted (no `$VAR`, no escapes).
//! Only these keys are ever read; any other line is ignored, and neither its key nor its value is logged:
//!
//! - `CLAUDE_CODE_OAUTH_TOKEN`: the long-lived sign-in `claude setup-token` prints. Passed to Claude Code only for a
//!   session on the person's Claude plan ([`Via::Subscription`]) and only on the agent's own login: not on the
//!   Gateway route (where Claude Code signs in with `ANTHROPIC_AUTH_TOKEN`, and a second credential would compete
//!   with it and bill the wrong place), not to Codex, and not for an extra Claude account (its own `CLAUDE_CONFIG_DIR`
//!   login would lose to the token, and the session would run on the wrong account).
//! - `GH_TOKEN`, `GITHUB_TOKEN`: what `gh` and many tools read to talk to GitHub. Passed on any route to any agent.
//!
//! Each agent's plan sign-in is its `CloudAgent::token_var` ([`plan_credential`]), from the agent's `nexus-agents`
//! entry (`SignIn.token_var`). Codex has none: OpenAI permits a ChatGPT plan on a hosted machine only through its Sign
//! in with ChatGPT partner program, which Nexus is not part of yet. So on a cloud machine (one with Gateway's
//! `cloud.json`) Codex runs through Nexus only, and a start on a ChatGPT plan is refused
//! ([`AgentFiles::launch_refusal`]). When Nexus joins that program, Codex's entry gets its token's variable, and the
//! rest follows.
//!
//! The file must be readable by its owner only. A file anyone else can read (any group or other permission bit) is
//! refused whole: a warning is printed once, and nothing in it is used, because a secret that was already exposed is
//! better replaced than quietly honoured. A missing file means nothing to pass. Values are never logged.
//!
//! # `mcp.json`
//!
//! Claude Code's own `--mcp-config` format, `{ "mcpServers": { "<name>": { ... } } }`: the servers are added to the
//! ones the agent already has (the person's own, from `~/.claude.json`, included). It is under the same private-file
//! rule, because a server's `env` or `headers` often hold keys. A file that is not that shape is refused with one
//! warning. Claude Code gets them as `--mcp-config`. Codex gets each server with a command or a URL as a `-c
//! mcp_servers.<name>=…` override before `app-server`; a server with its own `env` or `headers` is left out for Codex,
//! because Codex's overrides would put those values on the command line.
//!
//! # A native session's fence
//!
//! Inside a native session the agent starts with the session and its fencing token in its environment
//! ([`nexus_client::fence::SESSION_VAR`], [`nexus_client::fence::TOKEN_VAR`]), and the Nexus MCP server (`nexus mcp`)
//! sends both on each connector change. Each agent must pass them on to its MCP servers:
//!
//! - Claude Code and Nexus's own agent start a stdio server with their whole environment, so it gets the fence.
//! - Codex starts a stdio server with a few variables only (`HOME`, `PATH`, …) and the ones its `env_vars` names. So
//!   each stdio server of `mcp.json` names the two variables in its `env_vars`. Each stdio server of the person's own
//!   `config.toml` (`$CODEX_HOME`, else `~/.codex`) gets a `-c mcp_servers.<name>.env_vars=[…]` override with its own
//!   names first and the two after: the file is never written, and the person's other settings stay as they are. A
//!   server with a URL gets nothing (its `env_vars` is an error in Codex), and a server in a project's own
//!   `.codex/config.toml` is not read.
//! - Antigravity runs on the person's Google plan only, which a cloud machine refuses ([`AgentFiles::launch_refusal`]),
//!   so it never runs a native session.
//! - OpenCode gets no MCP server from this host: [`AgentFiles::apply`] passes it nothing of `mcp.json`, its harness
//!   takes no session or app servers, and the config its Gateway launch injects (`OPENCODE_CONFIG_CONTENT`) names none.
//!   It still starts with the fence in its environment. A test pins this, so whoever gives OpenCode a server decides
//!   then how the fence reaches it.

use std::collections::HashSet;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Mutex;

use nexus_workbench::cloud_agent::{cloud_agent, token_vars};
use nexus_workbench::event::{SessionAgent, Via};
use serde_json::{Map, Value, json};

/// GitHub's tokens in `agent.env`, for any agent on any route.
pub const GITHUB_KEYS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

/// Whether a session may be handed `key` from `agent.env`: an agent's plan sign-in ([`plan_credentials`]) or one of
/// [`GITHUB_KEYS`]. No other key is ever read.
pub fn is_agent_env_key(key: &str) -> bool {
    GITHUB_KEYS.contains(&key) || plan_credentials().contains(&key)
}

/// Every agent's plan sign-in variable in `agent.env`: its `CloudAgent::token_var`, from its `nexus-agents` entry.
/// Codex has none yet (see the module's notes on Sign in with ChatGPT).
pub fn plan_credentials() -> Vec<&'static str> {
    token_vars()
}

/// The variable that holds `agent`'s plan sign-in in `agent.env`, if it has one.
pub fn plan_credential(agent: SessionAgent) -> Option<&'static str> {
    cloud_agent(agent).token_var()
}

/// Gateway's file on a cloud machine (crate::cloud): with it, this folder is a cloud machine's.
const CLOUD_FILE: &str = "cloud.json";

/// More than any real file; a bigger one is something else, and is not read.
const MAX_BYTES: u64 = 256 * 1024;

/// The host's folder, and which of its files were last found unusable (so each is warned about once, and again if it
/// is fixed and then breaks again).
pub struct AgentFiles {
    root: PathBuf,
    warned: Mutex<HashSet<&'static str>>,
}

impl AgentFiles {
    pub fn new(root: PathBuf) -> AgentFiles {
        AgentFiles {
            root,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// Adds what the files give an agent about to start: the allowed variables into `env`, and for Claude the tools of
    /// `mcp.json` before `args`. Called on every launch, after the host built its own environment and before
    /// nexus-client's `agent_launch` (which strips what must not reach the route the session runs on).
    ///
    /// `own_login` is false for a session on an extra account: it signs in with that account's folder, so the Claude
    /// token stays out.
    pub fn apply(
        &self,
        agent: SessionAgent,
        via: Via,
        own_login: bool,
        args: &mut Vec<String>,
        env: &mut std::collections::HashMap<String, String>,
    ) {
        let plan = (via == Via::Subscription && own_login)
            .then(|| plan_credential(agent))
            .flatten();
        let credentials = plan_credentials();
        for (key, value) in self.agent_env() {
            let a_plan = credentials.contains(&key.as_str());
            if a_plan && plan != Some(key.as_str()) {
                continue;
            }
            env.insert(key, value);
        }
        let servers = self.mcp_servers();
        match agent.coding() {
            Some(nexus_client::agent_entries::Agent::Claude) => {
                if let Some(servers) = servers {
                    args.insert(0, format!("--mcp-config={}", json!({ "mcpServers": servers })));
                }
            }
            Some(nexus_client::agent_entries::Agent::Codex) => {
                let servers = servers.unwrap_or_default();
                let mut overrides = codex_overrides(&servers);
                // Inside a native session, the person's own servers get the fence too (the module's notes).
                if FENCE_VARS.iter().all(|name| env.contains_key(*name)) {
                    let own = codex_config(env)
                        .map(|text| codex_fence_overrides(&text))
                        .unwrap_or_default();
                    // A server of `mcp.json` replaces the person's of the same name, and already has the fence.
                    overrides.extend(
                        own.into_iter()
                            .filter(|(name, _)| !servers.contains_key(name))
                            .map(|(_, o)| o),
                    );
                }
                // Before `app-server`, where Codex reads its options.
                let at = args.iter().position(|arg| arg == "app-server").unwrap_or(0);
                let overrides = overrides.into_iter().flat_map(|pair| ["-c".to_string(), pair]);
                args.splice(at..at, overrides);
            }
            // OpenCode takes no MCP server from this host; adding one needs the fence too (the module's notes).
            Some(
                nexus_client::agent_entries::Agent::Antigravity
                | nexus_client::agent_entries::Agent::Cursor
                | nexus_client::agent_entries::Agent::Opencode,
            )
            | None => {}
        }
    }

    /// The local desktop service takes only the user's MCP file. Its agent.env has a separate payer.
    pub(crate) fn apply_local_mcp(
        &self,
        agent: SessionAgent,
        args: &mut Vec<String>,
        native: &mut nexus_workbench::local_provider::NativeMcp,
    ) -> Result<(), String> {
        let variable = match agent {
            SessionAgent::Claude => "ANTHROPIC_API_KEY",
            SessionAgent::Codex => nexus_client::agents::CODEX_KEY_ENV,
            _ => return Ok(()),
        };
        let Some(servers) = self.read_mcp_servers() else {
            if !matches!(self.root.join("mcp.json").try_exists(), Ok(false)) {
                native.unresolved();
            }
            return Ok(());
        };
        if servers.is_empty() {
            return Ok(());
        }
        for (name, entry) in &servers {
            if entry.get("command").is_none() || entry.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            native.include(name);
            let env = entry
                .get("env")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|values| values.keys().map(String::as_str));
            let forwarded = entry
                .get("env_vars")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            if env.chain(forwarded).any(|name| {
                if cfg!(windows) {
                    name.eq_ignore_ascii_case(variable)
                } else {
                    name == variable
                }
            }) {
                return Err(format!(
                    "A host MCP server declares {variable}. Rename that secret before starting this session."
                ));
            }
        }
        if agent == SessionAgent::Claude {
            args.insert(0, format!("--mcp-config={}", json!({ "mcpServers": servers })));
        } else {
            let at = args.iter().position(|arg| arg == "app-server").unwrap_or(0);
            args.splice(
                at..at,
                codex_overrides(&servers)
                    .into_iter()
                    .flat_map(|entry| ["-c".into(), entry]),
            );
        }
        Ok(())
    }

    /// Self-hosted sessions use the host's platform.
    /// Cloud machines require a configured sign-in for personal plans, so Codex uses Team API there.
    pub fn launch_refusal(&self, agent: SessionAgent, via: Via) -> Option<String> {
        if crate::cloud::CloudFile::load(&self.root.join(CLOUD_FILE)).is_none() {
            return agent.local_launch_refusal();
        }
        if via != Via::Subscription || agent == SessionAgent::Nexus || plan_credential(agent).is_some() {
            return None;
        }
        Some(format!(
            "{} can't use a personal plan on a cloud machine. Start the session on Team API: it runs through Nexus.",
            nexus_workbench::driver::label(agent)
        ))
    }

    /// The names of the servers `mcp.json` adds, for the session's record.
    pub fn mcp_names(&self) -> Vec<String> {
        self.mcp_servers()
            .map(|servers| servers.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The allowed pairs of `agent.env`, empty when the file is missing, unreadable or not private.
    pub fn agent_env(&self) -> Vec<(String, String)> {
        let Some(text) = self.read("agent.env") else {
            return Vec::new();
        };
        parse_env(&text)
    }

    /// The `mcpServers` of `mcp.json`, None when there are none or the file is unusable.
    pub fn mcp_servers(&self) -> Option<Map<String, Value>> {
        self.read_mcp_servers().filter(|servers| !servers.is_empty())
    }

    fn read_mcp_servers(&self) -> Option<Map<String, Value>> {
        let text = self.read("mcp.json")?;
        match parse_mcp(&text) {
            Ok(servers) => Some(servers),
            Err(why) => {
                self.warn("mcp.json", &why);
                None
            }
        }
    }

    /// The file's text, when it is there, small, and private; a warning (once) when it is there and not usable.
    fn read(&self, name: &'static str) -> Option<String> {
        let path = self.root.join(name);
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.clear(name);
                return None;
            }
            Err(error) => {
                self.warn(name, &format!("it cannot be opened ({})", error.kind()));
                return None;
            }
        };
        // The opened file's own mode, so it cannot change between the check and the read.
        let Ok(meta) = file.metadata() else {
            self.warn(name, "it cannot be inspected");
            return None;
        };
        if !meta.is_file() {
            self.warn(name, "it is not a regular file");
            return None;
        }
        if meta.permissions().mode() & 0o077 != 0 {
            self.warn(
                name,
                &format!(
                    "other users can read it (mode {:o}); run `chmod 600 {}`",
                    meta.permissions().mode() & 0o777,
                    path.display()
                ),
            );
            return None;
        }
        if meta.len() > MAX_BYTES {
            self.warn(name, "it is too big");
            return None;
        }
        let mut text = String::new();
        if file.by_ref().take(MAX_BYTES).read_to_string(&mut text).is_err() {
            self.warn(name, "it is not text");
            return None;
        }
        self.clear(name);
        Some(text)
    }

    fn warn(&self, name: &'static str, why: &str) {
        let first = self.warned.lock().map(|mut w| w.insert(name)).unwrap_or(false);
        if first {
            eprintln!(
                "nexus-host: ignoring {} (nothing in it is used): {why}.",
                self.root.join(name).display()
            );
        }
    }

    fn clear(&self, name: &'static str) {
        if let Ok(mut warned) = self.warned.lock() {
            warned.remove(name);
        }
    }
}

/// The allowed `KEY=VALUE` lines of an `agent.env`; the last of a repeated key wins, as in a shell.
fn parse_env(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !is_agent_env_key(key) {
            continue;
        }
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        found.retain(|(k, _)| k != key);
        found.push((key.to_string(), value.to_string()));
    }
    found
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

/// `mcp.json`'s servers as Codex's `-c` overrides: `mcp_servers.<name>={command=…,args=[…]}` or `{url=…}`. JSON's
/// strings and arrays of strings are TOML's too. A server Codex could not take whole is left out: a name that is not a
/// TOML bare key, neither a command nor a URL, or an `env` or `headers` of its own (their values would be on the
/// command line, and Codex's way to read them from the environment needs names the file does not give).
fn codex_overrides(servers: &Map<String, Value>) -> Vec<String> {
    let mut overrides = Vec::new();
    for (name, server) in servers {
        let bare = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let has = |field: &str| {
            server
                .get(field)
                .is_some_and(|v| v.as_object().is_none_or(|o| !o.is_empty()))
        };
        if !bare || has("env") || has("headers") {
            continue;
        }
        if let Some(command) = server.get("command").and_then(Value::as_str) {
            let listed = server.get("args").and_then(Value::as_array);
            let args: Vec<&str> = listed.into_iter().flatten().filter_map(Value::as_str).collect();
            if listed.is_some_and(|listed| listed.len() != args.len()) {
                continue;
            }
            // The fence's variables, when the agent has them (a name Codex does not find is passed as nothing).
            overrides.push(format!(
                "mcp_servers.{name}={{command={},args={},env_vars={}}}",
                json!(command),
                json!(args),
                json!(FENCE_VARS)
            ));
        } else if let Some(url) = server.get("url").and_then(Value::as_str) {
            overrides.push(format!("mcp_servers.{name}={{url={}}}", json!(url)));
        }
    }
    overrides
}

/// A native session's fence, as the agent gets it in its environment.
const FENCE_VARS: [&str; 2] = [nexus_client::fence::SESSION_VAR, nexus_client::fence::TOKEN_VAR];

/// The text of the person's Codex `config.toml`, from the agent's `CODEX_HOME` (an extra account's folder shares the
/// file), else `~/.codex`. None when there is none; it is the person's, so no private-file rule applies.
fn codex_config(env: &std::collections::HashMap<String, String>) -> Option<String> {
    let home = match env.get("CODEX_HOME").filter(|home| !home.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(env.get("HOME")?).join(".codex"),
    };
    let file = std::fs::File::open(home.join("config.toml")).ok()?;
    let mut text = String::new();
    file.take(MAX_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// For each stdio server of the person's Codex `config.toml` that does not name the fence's variables yet, its name and
/// a `mcp_servers.<name>.env_vars=[…]` override: the names it already has, then the fence's. Codex merges the override
/// into the person's table, so the rest of it stays. A file that is not TOML gives nothing (Codex refuses it too); so
/// does a server with a URL, a name that is not a TOML bare key, or an `env_vars` this cannot copy whole.
fn codex_fence_overrides(text: &str) -> Vec<(String, String)> {
    let Ok(config) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut overrides = Vec::new();
    for (name, server) in servers {
        let bare = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let Some(server) = server.as_table() else {
            continue;
        };
        if !bare || !server.contains_key("command") || server.contains_key("url") {
            continue;
        }
        let listed = match server.get("env_vars") {
            None => Some(Vec::new()),
            Some(value) => value
                .as_array()
                .and_then(|entries| entries.iter().map(env_var_entry).collect::<Option<Vec<_>>>()),
        };
        let Some(mut entries) = listed else {
            continue;
        };
        let missing: Vec<&str> = FENCE_VARS
            .into_iter()
            .filter(|var| !entries.iter().any(|(listed, _)| listed == var))
            .collect();
        if missing.is_empty() {
            continue;
        }
        entries.extend(missing.into_iter().map(|var| (var.to_string(), json!(var).to_string())));
        let list: Vec<String> = entries.into_iter().map(|(_, toml)| toml).collect();
        overrides.push((
            name.clone(),
            format!("mcp_servers.{name}.env_vars=[{}]", list.join(",")),
        ));
    }
    overrides
}

/// One entry of a server's `env_vars`, as its variable's name and its TOML again: a name (`"KEY"`) or a table
/// (`{name = "KEY", source = "local"}`). None for anything else.
fn env_var_entry(entry: &toml::Value) -> Option<(String, String)> {
    if let Some(name) = entry.as_str() {
        return Some((name.to_string(), json!(name).to_string()));
    }
    let table = entry.as_table()?;
    let mut fields = Vec::new();
    for (key, value) in table {
        if !["name", "source"].contains(&key.as_str()) {
            return None;
        }
        fields.push(format!("{key}={}", json!(value.as_str()?)));
    }
    let name = table.get("name")?.as_str()?.to_string();
    Some((name, format!("{{{}}}", fields.join(","))))
}

/// The `mcpServers` of an `mcp.json`, or why it is not one.
fn parse_mcp(text: &str) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "it is not JSON".to_string())?;
    let servers = value
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or_else(|| "it has no \"mcpServers\" object".to_string())?;
    if let Some((name, _)) = servers.iter().find(|(_, server)| !server.is_object()) {
        return Err(format!("server \"{name}\" is not an object"));
    }
    Ok(servers.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;

    /// Claude's plan sign-in, as its `nexus-agents` entry names it.
    const CLAUDE_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

    /// The keys `agent.env` gives, as they were when they were a list here.
    #[test]
    fn the_allowed_keys_are_the_plan_sign_ins_and_githubs() {
        assert_eq!(plan_credentials(), vec![CLAUDE_TOKEN]);
        for key in [CLAUDE_TOKEN, "GH_TOKEN", "GITHUB_TOKEN"] {
            assert!(is_agent_env_key(key), "{key}");
        }
        for key in ["OPENAI_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CODEX_HOME", ""] {
            assert!(!is_agent_env_key(key), "{key}");
        }
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nh-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn write(dir: &Path, name: &str, text: &str, mode: u32) {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("{e}"));
    }

    fn started(
        files: &AgentFiles,
        agent: SessionAgent,
        via: Via,
        own_login: bool,
    ) -> (Vec<String>, HashMap<String, String>) {
        let (mut args, mut env) = (vec!["-p".to_string()], HashMap::new());
        files.apply(agent, via, own_login, &mut args, &mut env);
        (args, env)
    }

    #[test]
    fn keyed_launch_checks_empty_missing_and_invalid_host_mcp_config() {
        let dir = folder("keyed-mcp-config");
        let files = AgentFiles::new(dir.clone());
        let mut args = Vec::new();
        let mut native = nexus_workbench::local_provider::NativeMcp::default();
        let clear = native.clone();
        files
            .apply_local_mcp(SessionAgent::Claude, &mut args, &mut native)
            .expect("missing");
        assert_eq!(native, clear);

        write(&dir, "mcp.json", r#"{"mcpServers":{}}"#, 0o600);
        files
            .apply_local_mcp(SessionAgent::Claude, &mut args, &mut native)
            .expect("empty");
        assert_eq!(native, clear);
        assert!(args.is_empty());

        write(&dir, "mcp.json", "invalid", 0o600);
        files
            .apply_local_mcp(SessionAgent::Claude, &mut args, &mut native)
            .expect("invalid");
        let mut unresolved = clear;
        unresolved.unresolved();
        assert_eq!(native, unresolved);

        write(&dir, "mcp.json", r#"{"mcpServers":{}}"#, 0o644);
        let mut native = nexus_workbench::local_provider::NativeMcp::default();
        files
            .apply_local_mcp(SessionAgent::Claude, &mut args, &mut native)
            .expect("not private");
        assert_eq!(native, unresolved);
        assert!(args.is_empty());
        std::fs::remove_dir_all(dir).expect("clean fixture");
    }

    #[test]
    fn keyed_launch_refuses_a_native_stdio_server_that_declares_the_key() {
        let dir = folder("keyed-mcp-collision");
        let files = AgentFiles::new(dir.clone());
        let mut args = Vec::new();
        let mut native = nexus_workbench::local_provider::NativeMcp::default();
        for (agent, variable, declaration) in [
            (
                SessionAgent::Claude,
                "ANTHROPIC_API_KEY",
                json!({"env": {"ANTHROPIC_API_KEY": ""}}),
            ),
            (
                SessionAgent::Codex,
                nexus_client::agents::CODEX_KEY_ENV,
                json!({"env_vars": [nexus_client::agents::CODEX_KEY_ENV]}),
            ),
        ] {
            let mut entry = declaration;
            entry["command"] = json!("native-db");
            let text = json!({"mcpServers": {"native-db": entry}}).to_string();
            write(&dir, "mcp.json", &text, 0o600);
            let error = files
                .apply_local_mcp(agent, &mut args, &mut native)
                .expect_err("key collision");
            assert!(error.contains(variable));
            assert!(args.is_empty());
        }
        std::fs::remove_dir_all(dir).expect("clean fixture");
    }

    #[test]
    fn reads_only_the_allowed_keys() {
        let text = "# from the app\n\nCLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-a\nexport GH_TOKEN=\"ghp_b\"\n\
                    AWS_SECRET_ACCESS_KEY=nope\nGITHUB_TOKEN='ghp_c'\nno equals sign\nGH_TOKEN=ghp_d\nEMPTY=\n";
        let mut found = parse_env(text);
        found.sort();
        assert_eq!(
            found,
            vec![
                ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-ant-oat01-a".to_string()),
                ("GH_TOKEN".to_string(), "ghp_d".to_string()),
                ("GITHUB_TOKEN".to_string(), "ghp_c".to_string()),
            ]
        );
        assert!(parse_env("GH_TOKEN=\n").is_empty());
    }

    /// The Claude token goes only to Claude, on the plan, on its own login; GitHub's goes everywhere.
    #[test]
    fn the_claude_token_goes_only_where_the_plan_runs() {
        let dir = folder("routes");
        write(
            &dir,
            "agent.env",
            "CLAUDE_CODE_OAUTH_TOKEN=tok\nGH_TOKEN=gh\nOTHER=x\n",
            0o600,
        );
        let files = AgentFiles::new(dir.clone());
        let has = |env: &HashMap<String, String>, key: &str| env.contains_key(key);

        let (_, env) = started(&files, SessionAgent::Claude, Via::Subscription, true);
        assert!(has(&env, CLAUDE_TOKEN) && has(&env, "GH_TOKEN") && !has(&env, "OTHER"));

        let (_, env) = started(&files, SessionAgent::Claude, Via::Nexus, true);
        assert!(!has(&env, CLAUDE_TOKEN) && has(&env, "GH_TOKEN"));

        let (_, env) = started(&files, SessionAgent::Claude, Via::Subscription, false);
        assert!(!has(&env, CLAUDE_TOKEN) && has(&env, "GH_TOKEN"));

        let (_, env) = started(&files, SessionAgent::Codex, Via::Subscription, true);
        assert!(!has(&env, CLAUDE_TOKEN) && has(&env, "GH_TOKEN"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_others_can_read_is_refused_whole_and_a_fix_applies_at_once() {
        let dir = folder("mode");
        write(&dir, "agent.env", "GH_TOKEN=gh\n", 0o640);
        let files = AgentFiles::new(dir.clone());
        assert!(
            started(&files, SessionAgent::Claude, Via::Subscription, true)
                .1
                .is_empty()
        );
        write(&dir, "agent.env", "GH_TOKEN=gh\n", 0o644);
        assert!(files.agent_env().is_empty());
        // Fixed: read fresh, with no restart.
        write(&dir, "agent.env", "GH_TOKEN=new\n", 0o600);
        assert_eq!(files.agent_env(), vec![("GH_TOKEN".to_string(), "new".to_string())]);
        // Removed: nothing.
        let _ = std::fs::remove_file(dir.join("agent.env"));
        assert!(files.agent_env().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_gives_nothing() {
        let dir = folder("missing");
        let files = AgentFiles::new(dir.clone());
        let (args, env) = started(&files, SessionAgent::Claude, Via::Subscription, true);
        assert_eq!((args, env.len()), (vec!["-p".to_string()], 0));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What Codex can take whole goes; anything that would put a secret on the command line, or is not TOML, stays out.
    #[test]
    fn codex_takes_the_servers_it_can_read_without_secrets() {
        let servers = parse_mcp(
            r#"{"mcpServers":{
                "browser":{"command":"npx","args":["-y","@playwright/mcp@0.0.84","--output-dir",".nexus/artifacts"]},
                "docs":{"type":"http","url":"https://docs.example/mcp"},
                "quoted":{"command":"say \"hi\""},
                "keyed":{"command":"tool","env":{"KEY":"secret"}},
                "headed":{"url":"https://x.example","headers":{"Authorization":"Bearer secret"}},
                "empty-env":{"command":"ok","env":{}},
                "bad name":{"command":"tool"},
                "odd-args":{"command":"tool","args":["a",1]},
                "nothing":{}
            }}"#,
        )
        .unwrap_or_default();
        let mut found = codex_overrides(&servers);
        found.sort();
        assert_eq!(
            found,
            vec![
                r#"mcp_servers.browser={command="npx",args=["-y","@playwright/mcp@0.0.84","--output-dir",".nexus/artifacts"],env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]}"#,
                r#"mcp_servers.docs={url="https://docs.example/mcp"}"#,
                r#"mcp_servers.empty-env={command="ok",args=[],env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]}"#,
                r#"mcp_servers.quoted={command="say \"hi\"",args=[],env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]}"#,
            ]
        );
        assert!(found.iter().all(|line| !line.contains("secret")));
    }

    /// On a cloud machine Codex never starts on a ChatGPT plan; on a person's own server it may, and Claude may anywhere.
    #[test]
    fn a_cloud_machine_refuses_a_plan_it_has_no_sign_in_for() {
        let dir = folder("plan");
        let files = AgentFiles::new(dir.clone());
        assert_eq!(
            files.launch_refusal(SessionAgent::Codex, Via::Subscription),
            None,
            "a server is the person's"
        );
        write(&dir, "cloud.json", r#"{"machineId":"bx_1"}"#, 0o600);
        let refused = files
            .launch_refusal(SessionAgent::Codex, Via::Subscription)
            .unwrap_or_default();
        assert!(refused.contains("Codex") && refused.contains("Team API"), "{refused}");
        assert_eq!(files.launch_refusal(SessionAgent::Codex, Via::Nexus), None);
        assert_eq!(files.launch_refusal(SessionAgent::Claude, Via::Subscription), None);
        assert_eq!(files.launch_refusal(SessionAgent::Nexus, Via::Subscription), None);
        assert_eq!(plan_credential(SessionAgent::Claude), Some(CLAUDE_TOKEN));
        assert_eq!(plan_credential(SessionAgent::Codex), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mcp_json_is_added_for_claude_and_codex() {
        let dir = folder("mcp");
        let servers =
            r#"{"mcpServers":{"playwright":{"type":"stdio","command":"npx","args":["@playwright/mcp@latest"]}}}"#;
        write(&dir, "mcp.json", servers, 0o600);
        let files = AgentFiles::new(dir.clone());
        let (args, _) = started(&files, SessionAgent::Claude, Via::Nexus, true);
        assert_eq!(args.len(), 2);
        let config = args[0].strip_prefix("--mcp-config=").unwrap_or_default();
        let config: Value = serde_json::from_str(config).unwrap_or_default();
        assert_eq!(config["mcpServers"]["playwright"]["command"], "npx");
        assert_eq!(args[1], "-p");
        assert_eq!(files.mcp_names(), vec!["playwright".to_string()]);
        // Codex gets the same server as an override before `app-server`.
        let (mut args, mut env) = (
            vec!["-c".to_string(), "x=1".to_string(), "app-server".to_string()],
            HashMap::new(),
        );
        files.apply(SessionAgent::Codex, Via::Nexus, true, &mut args, &mut env);
        assert_eq!(
            args,
            vec![
                "-c",
                "x=1",
                "-c",
                r#"mcp_servers.playwright={command="npx",args=["@playwright/mcp@latest"],env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]}"#,
                "app-server"
            ]
        );

        for bad in ["not json", "{}", r#"{"mcpServers":{"a":1}}"#, r#"{"mcpServers":{}}"#] {
            write(&dir, "mcp.json", bad, 0o600);
            assert_eq!(
                started(&files, SessionAgent::Claude, Via::Nexus, true).0,
                vec!["-p".to_string()]
            );
        }
        write(&dir, "mcp.json", servers, 0o644);
        assert_eq!(
            started(&files, SessionAgent::Claude, Via::Nexus, true).0,
            vec!["-p".to_string()]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The person's own servers: each stdio one gets the fence after its own names; nothing else changes.
    #[test]
    fn codex_hands_the_fence_to_the_persons_own_stdio_servers() {
        let found = codex_fence_overrides(
            r#"
            model = "gpt-5"
            [mcp_servers.nexus]
            command = "nexus"
            args = ["mcp"]
            env = { NEXUS_TEAM = "core" }
            [mcp_servers.keyed]
            command = "tool"
            env_vars = ["KEY", { name = "OTHER", source = "local" }]
            [mcp_servers.done]
            command = "tool"
            env_vars = ["NEXUS_FENCING_TOKEN", "NEXUS_CLOUD_SESSION"]
            [mcp_servers.half]
            command = "tool"
            env_vars = ["NEXUS_CLOUD_SESSION"]
            [mcp_servers.docs]
            url = "https://docs.example/mcp"
            [mcp_servers."dotted.name"]
            command = "tool"
            [mcp_servers.odd]
            command = "tool"
            env_vars = [1]
            "#,
        );
        assert_eq!(
            found,
            vec![
                (
                    "half".to_string(),
                    r#"mcp_servers.half.env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]"#.to_string()
                ),
                (
                    "keyed".to_string(),
                    r#"mcp_servers.keyed.env_vars=["KEY",{name="OTHER",source="local"},"NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]"#
                        .to_string()
                ),
                (
                    "nexus".to_string(),
                    r#"mcp_servers.nexus.env_vars=["NEXUS_CLOUD_SESSION","NEXUS_FENCING_TOKEN"]"#.to_string()
                ),
            ]
        );
        for nothing in ["not toml [", "model = \"x\"", "mcp_servers = 1"] {
            assert!(codex_fence_overrides(nothing).is_empty(), "{nothing}");
        }
    }

    /// Outside a native session the person's Codex servers are left alone: no fence, no override.
    #[test]
    fn codex_overrides_the_persons_servers_only_inside_a_native_session() {
        let dir = folder("codex-plain");
        let codex = dir.join("codex");
        let _ = std::fs::create_dir_all(&codex);
        write(
            &codex,
            "config.toml",
            "[mcp_servers.nexus]\ncommand = \"nexus\"\n",
            0o644,
        );
        let files = AgentFiles::new(dir.clone());
        let (mut args, mut env) = (
            vec!["app-server".to_string()],
            HashMap::from([("CODEX_HOME".to_string(), codex.display().to_string())]),
        );
        files.apply(SessionAgent::Codex, Via::Nexus, true, &mut args, &mut env);
        assert_eq!(args, vec!["app-server".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The environment Codex 0.166 (the cloud image's) starts a stdio MCP server with: its few defaults and the
    /// variables the server's `env_vars` names, from Codex's own environment, then the server's `env`
    /// (codex-rs/rmcp-client/src/utils.rs, `create_env_for_mcp_server`). The server's table is the person's
    /// `config.toml` with each `-c key=value` written at its dotted path on top (codex-rs/config/src/overrides.rs).
    fn codex_server_env(
        config: &str,
        args: &[String],
        agent_env: &HashMap<String, String>,
        server: &str,
    ) -> Option<HashMap<String, String>> {
        const DEFAULTS: [&str; 11] = [
            "HOME",
            "LOGNAME",
            "PATH",
            "SHELL",
            "USER",
            "__CF_USER_TEXT_ENCODING",
            "LANG",
            "LC_ALL",
            "TERM",
            "TMPDIR",
            "TZ",
        ];
        let mut root = toml::Value::Table(config.parse::<toml::Table>().ok()?);
        for pair in args.windows(2).filter(|w| w[0] == "-c").map(|w| &w[1]) {
            let (path, value) = pair.split_once('=')?;
            let value = format!("v={value}").parse::<toml::Table>().ok()?.remove("v")?;
            let mut at = &mut root;
            let parts: Vec<&str> = path.split('.').collect();
            for part in &parts[..parts.len() - 1] {
                at = at
                    .as_table_mut()?
                    .entry(part.to_string())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            }
            at.as_table_mut()?.insert(parts[parts.len() - 1].to_string(), value);
        }
        let table = root.get("mcp_servers")?.get(server)?.as_table()?;
        table.get("command")?;
        let listed = table
            .get("env_vars")
            .and_then(toml::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names = listed
            .iter()
            .filter_map(|entry| entry.as_str().or_else(|| entry.get("name")?.as_str()))
            .map(String::from);
        let mut env: HashMap<String, String> = DEFAULTS
            .iter()
            .map(|name| name.to_string())
            .chain(names)
            .filter_map(|name| agent_env.get(&name).map(|value| (name, value.clone())))
            .collect();
        for (key, value) in table.get("env").and_then(toml::Value::as_table).into_iter().flatten() {
            env.insert(key.clone(), value.as_str()?.to_string());
        }
        Some(env)
    }

    /// Inside a native session, `nexus mcp` gets the fence under every agent this host starts on a cloud
    /// machine, from `mcp.json` and from the agent's own configuration alike.
    #[test]
    fn the_fence_reaches_nexus_mcp_under_each_agent() {
        let dir = folder("fence");
        let codex = dir.join("codex");
        let _ = std::fs::create_dir_all(&codex);
        // `nexus mcp` as `mcp.json` names it, and as the person added it to Codex (`codex mcp add nexus -- nexus mcp`).
        write(
            &dir,
            "mcp.json",
            r#"{"mcpServers":{"nexus_tools":{"command":"nexus","args":["mcp"]}}}"#,
            0o600,
        );
        let config = "[mcp_servers.nexus]\ncommand = \"nexus\"\nargs = [\"mcp\"]\nenv_vars = [\"NEXUS_TEAM\"]\n";
        write(&codex, "config.toml", config, 0o644);
        let files = AgentFiles::new(dir.clone());
        // The agent's environment, as the host builds it for a native session (src/gateway.rs).
        let agent_env = || {
            HashMap::from([
                ("HOME".to_string(), "/home/me".to_string()),
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("CODEX_HOME".to_string(), codex.display().to_string()),
                ("NEXUS_TEAM".to_string(), "core".to_string()),
                ("UNRELATED".to_string(), "x".to_string()),
                (nexus_client::fence::SESSION_VAR.to_string(), "cs_abc".to_string()),
                (nexus_client::fence::TOKEN_VAR.to_string(), "7".to_string()),
            ])
        };
        let fenced = |env: &HashMap<String, String>| {
            env.get(nexus_client::fence::SESSION_VAR).map(String::as_str) == Some("cs_abc")
                && env.get(nexus_client::fence::TOKEN_VAR).map(String::as_str) == Some("7")
        };

        // Codex: a filtered environment, so the fence is named in each server's `env_vars`.
        let (mut args, mut env) = (vec!["app-server".to_string()], agent_env());
        files.apply(SessionAgent::Codex, Via::Nexus, true, &mut args, &mut env);
        for server in ["nexus_tools", "nexus"] {
            let got = codex_server_env(config, &args, &env, server).unwrap_or_default();
            assert!(fenced(&got), "Codex's {server}: {got:?} from {args:?}");
            assert!(!got.contains_key("UNRELATED"), "Codex still filters: {got:?}");
        }
        // The person's own names stay.
        let own = codex_server_env(config, &args, &env, "nexus").unwrap_or_default();
        assert_eq!(own.get("NEXUS_TEAM").map(String::as_str), Some("core"));
        // Without the overrides, Codex's `nexus mcp` would get no fence: the gap this closes.
        let bare = codex_server_env(config, &[], &env, "nexus").unwrap_or_default();
        assert!(!fenced(&bare), "{bare:?}");

        // Claude Code: `mcp.json` comes as `--mcp-config`, and a stdio server starts with Claude Code's whole
        // environment, as does a server of the person's own Claude configuration.
        let (args, env) = started(&files, SessionAgent::Claude, Via::Nexus, true);
        let mut env = env;
        env.extend(agent_env());
        let config = args[0].strip_prefix("--mcp-config=").unwrap_or_default();
        let config: Value = serde_json::from_str(config).unwrap_or_default();
        assert_eq!(config["mcpServers"]["nexus_tools"]["args"], json!(["mcp"]));
        assert!(config["mcpServers"]["nexus_tools"].get("env").is_none());
        assert!(fenced(&env));

        // Nexus's own agent gets no files: it starts with the host's environment and the fence (src/gateway.rs),
        // and starts its stdio servers with all of it.
        assert!(fenced(&agent_env()));
        // OpenCode gets no MCP server from the host, so nothing to fence: not `mcp.json`, not the session's or the
        // app's servers. A server added here must take the fence as Codex's do.
        let (mut args, mut env) = (vec!["acp".to_string()], agent_env());
        files.apply(SessionAgent::Opencode, Via::Nexus, true, &mut args, &mut env);
        assert_eq!(args, vec!["acp".to_string()]);
        assert_eq!(env, agent_env());
        let opencode = nexus_workbench::harness::harness(SessionAgent::Opencode);
        assert!(matches!(
            opencode.mcp_servers(&[], &Default::default()),
            nexus_workbench::harness::McpDelivery::NotOffered
        ));
        assert!(!opencode.app_tools().offered());
        assert!(!nexus_workbench::mcp_servers::in_session(SessionAgent::Opencode));
        // Antigravity never runs a native session: a cloud machine refuses its plan.
        write(&dir, "cloud.json", r#"{"machineId":"bx_1"}"#, 0o600);
        assert!(
            files
                .launch_refusal(SessionAgent::Antigravity, Via::Subscription)
                .is_some()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
