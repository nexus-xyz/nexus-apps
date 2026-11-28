//! The app's loopback MCP server, `nexus_app` (desktop-linked-sessions.md, "The server", "Injection into each agent"
//! and "Tools"), PR 4.3: the tools a Claude Code or Codex session calls to start, message, wait for, list and stop
//! linked sessions, over the core in [`crate::linked`]. One server per [`Workbench`], bound on 127.0.0.1 the first time
//! a session with orchestration on starts ([`http`]); one grant per run of an agent ([`grant`]); the tools themselves
//! ([`tools`]). [`Workbench::start`] adds the server to the agent's arguments and environment ([`inject`]) between
//! the host's launch and the spawn, so no host changes for it. The side pane's browser tools become a
//! second toolset here later.

pub mod grant;
pub mod http;
pub mod tools;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::event::{SessionAgent, WbEvent};
use crate::harness::{AppTools, harness};
use crate::process::Spawn;
use crate::store::ThreadRecord;
use crate::{Live, Workbench};
pub use grant::{Grant, Toolsets};

/// The server's name in the agents' configs, and in their tool names (`mcp__nexus_app__start_session`). Settings
/// refuses a server of its own under it.
pub const SERVER: &str = "nexus_app";
/// The variable that carries the run's token to the agent. Never in its arguments.
pub const TOKEN_VAR: &str = "NEXUS_APP_MCP_TOKEN";
/// What a host that serves these tools says in its hello (`HostInfo::features`).
pub const FEATURE: &str = "orchestration";
/// What a core that sends the crew's controls says beside [`FEATURE`]: each session's halt, its crew's pause, its
/// background work and its depth limit, and Stop on a lead that runs nothing. An app hides the switch and those
/// controls from a host without it.
pub const CONTROLS_FEATURE: &str = "orchestration-controls";

/// A local desktop service supports reloading the saved bounds from its own settings.
pub const LIMITS_FEATURE: &str = "linked-session-limits";
pub const RELOAD_LIMITS_CALL: &str = "host_reload_linked_limits";
/// How long an agent lets one call run: past the longest wait (600 s).
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(660);
/// Claude Code's limit on one MCP call, in milliseconds, for every server of the run.
const TIMEOUT_VAR: &str = "MCP_TOOL_TIMEOUT";
/// The largest `.mcp.json` read: a larger one counts as naming the server.
const MAX_MCP_JSON: u64 = 1024 * 1024;

/// The server's deadlines and its keep-alive. The tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// A request's headers must arrive within this, and an idle connection closes after it.
    pub header: Duration,
    /// A request's body must arrive within this.
    pub body: Duration,
    /// How often an open wait sends something, so neither agent's idle timeout fires.
    pub keep_alive: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            header: Duration::from_secs(10),
            body: Duration::from_secs(10),
            keep_alive: Duration::from_secs(20),
        }
    }
}

/// The linked-session tool names as the agent knows them, for Claude Code's `--allowedTools`.
pub fn allowed_tools() -> Vec<String> {
    tools::NAMES
        .iter()
        .map(|name| format!("mcp__{SERVER}__{name}"))
        .collect()
}

/// Adds the server to an agent's spawn: its arguments name the server and its URL, its environment holds the token
/// (never an argument), and `127.0.0.1` joins `NO_PROXY` and `no_proxy`, so no proxy setting ever sees the token.
/// Claude Code gets a second `--mcp-config` and the tools pre-allowed (never the server's prefix); Codex gets a
/// `-c` override before `app-server`, where it reads its options, with only this server's tools approved.
pub fn inject(spawn: &mut Spawn, agent: SessionAgent, grant: &Grant, port: u16) {
    inject_tools(spawn, harness(agent).app_tools(), grant, port);
}

fn inject_tools(spawn: &mut Spawn, tools: AppTools, grant: &Grant, port: u16) {
    let url = format!("http://127.0.0.1:{port}/mcp/{}", grant.agent_id);
    let timeout = TOOL_TIMEOUT.as_secs();
    match tools {
        AppTools::ClaudeOptions {
            disallow_native_shell, ..
        } => {
            let config = json!({"mcpServers": {SERVER: {
                "type": "http",
                "url": url,
                "headers": {"Authorization": format!("Bearer ${{{TOKEN_VAR}}}")},
            }}});
            // With `=`, neither variadic option takes the next argument too.
            spawn.args.push(format!("--mcp-config={config}"));
            let mut allowed = if grant.toolsets.crew {
                allowed_tools()
            } else {
                Vec::new()
            };
            if grant.toolsets.terminal {
                allowed.extend(
                    crate::terminal::NAMES
                        .iter()
                        .map(|name| format!("mcp__{SERVER}__{name}")),
                );
            }
            spawn.args.push(format!("--allowedTools={}", allowed.join(",")));
            if grant.toolsets.terminal && disallow_native_shell {
                // Exclusion, unlike pre-approval, removes the provider's native execution path.
                spawn.args.push("--disallowedTools=Bash,KillShell".into());
            }
            // The person's own limit when it is longer: it applies to their servers too.
            let inherited = spawn
                .env
                .get(TIMEOUT_VAR)
                .cloned()
                .or_else(|| std::env::var(TIMEOUT_VAR).ok());
            spawn
                .env
                .insert(TIMEOUT_VAR.into(), tool_timeout_ms(inherited.as_deref()).to_string());
        }
        AppTools::CodexOverride => {
            let server = format!(
                "mcp_servers.{SERVER}={{url={},bearer_token_env_var={},tool_timeout_sec={timeout},\
                 startup_timeout_sec=10,default_tools_approval_mode=\"approve\"}}",
                Value::from(url),
                Value::from(TOKEN_VAR),
            );
            let at = spawn
                .args
                .iter()
                .position(|arg| arg == "app-server")
                .unwrap_or(spawn.args.len());
            let mut overrides = vec!["-c".to_string(), server];
            if grant.toolsets.terminal {
                // Stable Codex control: disabling unified_exec alone still leaves one-shot commands available.
                overrides.extend(["-c".to_string(), "features.shell_tool=false".to_string()]);
            }
            spawn.args.splice(at..at, overrides);
        }
        AppTools::NotOffered => return,
    }
    spawn.env.insert(TOKEN_VAR.into(), grant.token.clone());
    // Each keeps its own value, or the other's when it has none, with the loopback added.
    let (upper, lower) = (spawn.env.get("NO_PROXY").cloned(), spawn.env.get("no_proxy").cloned());
    for (name, value) in [
        ("NO_PROXY", upper.clone().or(lower.clone())),
        ("no_proxy", lower.or(upper)),
    ] {
        spawn.env.insert(name.into(), with_loopback(value.as_deref()));
    }
}

/// `MCP_TOOL_TIMEOUT` for a Claude run: the inherited value when it is longer than [`TOOL_TIMEOUT`], else that.
fn tool_timeout_ms(inherited: Option<&str>) -> u128 {
    let least = TOOL_TIMEOUT.as_millis();
    inherited
        .and_then(|value| value.trim().parse::<u128>().ok())
        .map_or(least, |value| value.max(least))
}

fn with_loopback(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => "127.0.0.1".into(),
        Some(value) if value.split(',').any(|entry| entry.trim() == "127.0.0.1") => value.into(),
        Some(value) => format!("{value},127.0.0.1"),
    }
}

/// Whether a `.mcp.json` in `dir` or a folder above it names `nexus_app` (as a server, or in `NEXUS_APP_MCP_TOKEN`):
/// Claude Code would read it, and the server's pre-allowed tool names would then match a repository's own server.
/// It blocks: the caller runs it off the async workers.
pub fn folder_names_server(dir: &Path) -> bool {
    dir.ancestors().any(|dir| mcp_json_names_server(&dir.join(".mcp.json")))
}

/// Whether the `.mcp.json` at `path` names the server. It fails closed: a file that is not a regular one (a device, a
/// pipe), is over [`MAX_MCP_JSON`] or is not JSON counts as naming it. A missing one, or one this user can't open,
/// does not: Claude Code can't read it either.
fn mcp_json_names_server(path: &Path) -> bool {
    use std::io::Read;
    // Before opening it: opening a pipe would block, and reading a device may never end.
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return true;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(MAX_MCP_JSON + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_MCP_JSON {
        return true;
    }
    let Ok(config) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    // The token's name as written, and as Claude Code decodes it (a JSON escape such as `\u005f`).
    let token = TOKEN_VAR.to_ascii_lowercase();
    if String::from_utf8_lossy(&bytes).to_ascii_lowercase().contains(&token)
        || config.to_string().to_ascii_lowercase().contains(&token)
    {
        return true;
    }
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|servers| {
            servers
                .keys()
                .any(|name| tool_prefix(name).eq_ignore_ascii_case(SERVER))
        })
}

/// A server's name as Claude Code puts it in its tools' names: every character outside `[A-Za-z0-9_-]` becomes `_`,
/// one per UTF-16 unit, as its JavaScript replace does. So `nexus.app` and `nexus app` are `nexus_app` too.
fn tool_prefix(name: &str) -> String {
    name.chars()
        .flat_map(|c| {
            let kept = c.is_ascii_alphanumeric() || c == '_' || c == '-';
            let (c, n) = if kept { (c, 1) } else { ('_', c.len_utf16()) };
            std::iter::repeat_n(c, n)
        })
        .collect()
}

/// Why a session starts without the tools, in its log. The server also carries the Terminal tools, and `arm` runs for
/// those with subsessions off, so neither text names one toolset.
const UNAVAILABLE: &str = "Nexus's app tools are unavailable";
const FOLDER_NAMES_SERVER: &str = "This session starts without Nexus's app tools: a .mcp.json in its folder names \
                                   nexus_app, the server Nexus keeps for them. Rename that server to use them.";

impl Workbench {
    /// The server's port, bound the first time (a failure is tried again at the next start).
    pub(crate) async fn orchestra(&self) -> Result<u16, String> {
        let timing = self.config().orchestra_timing;
        let weak = Arc::downgrade(&self.inner);
        self.inner
            .orchestra
            .get_or_try_init(|| http::bind(weak, timing))
            .await
            .map(|server| server.port)
            .map_err(|error| error.to_string())
    }

    /// What this workbench serves its agents, for a host's hello (`HostInfo::features`).
    pub fn features(&self) -> Vec<String> {
        let mut features = vec![
            FEATURE.to_string(),
            crate::commands::FEATURE.to_string(),
            crate::connection::FEATURE.to_string(),
            CONTROLS_FEATURE.to_string(),
            crate::session_choice::FEATURE.to_string(),
            crate::clone::FEATURE.to_string(),
            crate::dirs::FEATURE.to_string(),
            crate::diff_source::FEATURE.to_string(),
            crate::project_files::FEATURE.to_string(),
            crate::output_files::FEATURE.to_string(),
            crate::output_files::PIECES_FEATURE.to_string(),
            crate::output_media::FEATURE.to_string(),
            crate::PROJECT_PIN_ARCHIVE.to_string(),
            crate::side_chat::FEATURE.to_string(),
            crate::moving::FEATURE.to_string(),
        ];
        if self.config().terminal {
            features.push(crate::terminal::FEATURE.into());
        }
        features
    }

    /// Gives the run its tools, between the host's launch and the spawn: when the session's orchestration is on, it is
    /// above the deepest level, and the server is bound. A Claude session whose folder names `nexus_app` starts
    /// without them, and so does any session when the server can't be bound; its log says why.
    pub(crate) async fn arm(&self, record: &ThreadRecord, live: &Live, spawn: &mut Spawn) {
        let summary = &record.summary;
        let tools = harness(summary.agent).app_tools();
        if !tools.offered() {
            return;
        }
        let crew = summary.orchestration && summary.depth < self.config().orchestration.depth();
        let terminal = self.config().terminal;
        if !crew && !terminal {
            return;
        }
        let notice = |text: String| {
            if let Err(error) = self.append(&summary.id, WbEvent::Notice { text }) {
                eprintln!("nexus-workbench: {}: {error}", summary.id);
            }
        };
        if matches!(
            tools,
            AppTools::ClaudeOptions {
                reads_project_mcp: true,
                ..
            }
        ) {
            // The walk up the folders reads files, which may be slow (a network mount): off the async workers.
            let cwd = spawn.cwd.clone();
            let named = tokio::task::spawn_blocking(move || folder_names_server(&cwd))
                .await
                .unwrap_or(true);
            if named {
                notice(FOLDER_NAMES_SERVER.into());
                return;
            }
        }
        let port = match self.orchestra().await {
            Ok(port) => port,
            Err(error) => {
                notice(format!("{UNAVAILABLE}: {error}"));
                return;
            }
        };
        let grant = {
            let mut state = live.state();
            let Some(grant) = state.grant.as_mut() else {
                // Stopped meanwhile: the run ends before its agent reads anything.
                return;
            };
            grant.toolsets.crew = crew;
            grant.toolsets.terminal = terminal;
            grant.choice_identity = self.inner.host.session_choice_identity();
            grant.clone()
        };
        inject_tools(spawn, tools, &grant, port);
    }

    /// Whether the session's current run still has the grant `agent_id`: not stopped, exited or restarted since.
    pub(crate) fn holds_grant(&self, thread_id: &str, agent_id: &str) -> bool {
        self.live(thread_id).is_some_and(|live| {
            live.state()
                .grant
                .as_ref()
                .is_some_and(|grant| grant.agent_id == agent_id)
        })
    }

    /// The running session whose grant has `agent_id` and `token`, and that grant. Every run is looked at, each
    /// under its own lock, so a stopped or ended run (its grant cleared) is never found.
    pub(crate) fn grant_for(&self, agent_id: &str, token: &str) -> Option<(String, Grant)> {
        if agent_id.is_empty() || token.is_empty() {
            return None;
        }
        self.all_live().into_iter().find_map(|(thread_id, live)| {
            let state = live.state();
            state
                .grant
                .as_ref()
                .filter(|grant| grant.matches(agent_id, token))
                .map(|grant| (thread_id, grant.clone()))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;

    fn spawn(args: &[&str], env: &[(&str, &str)]) -> Spawn {
        Spawn {
            program: PathBuf::from("agent"),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
            cwd: PathBuf::from("/tmp"),
        }
    }

    fn grant() -> Grant {
        let mut grant = Grant::new();
        grant.toolsets.crew = true;
        grant.agent_id = "a".repeat(32);
        grant
    }

    #[test]
    fn claude_gets_a_second_config_the_tools_and_the_token_only_in_its_environment() {
        let grant = grant();
        let mut claude = spawn(
            &["--mcp-config={\"mcpServers\":{}}", "-p"],
            &[("NO_PROXY", "corp.test")],
        );
        inject(&mut claude, SessionAgent::Claude, &grant, 4123);
        let id = "a".repeat(32);
        assert_eq!(
            claude.args,
            [
                "--mcp-config={\"mcpServers\":{}}".to_string(),
                "-p".to_string(),
                format!(
                    "--mcp-config={{\"mcpServers\":{{\"nexus_app\":{{\"type\":\"http\",\"url\":\"http://127.0.0.1:4123/mcp/{id}\",\
                     \"headers\":{{\"Authorization\":\"Bearer ${{NEXUS_APP_MCP_TOKEN}}\"}}}}}}}}"
                ),
                "--allowedTools=mcp__nexus_app__start_session,mcp__nexus_app__message_session,\
                 mcp__nexus_app__wait_sessions,mcp__nexus_app__list_sessions,mcp__nexus_app__stop_session,\
                 mcp__nexus_app__handoff_session,mcp__nexus_app__archive_session,mcp__nexus_app__subscribe_session,mcp__nexus_app__schedule_session,mcp__nexus_app__list_schedules,mcp__nexus_app__cancel_schedule"
                    .to_string(),
            ]
        );
        assert!(claude.args.iter().all(|arg| !arg.contains(&grant.token)));
        assert_eq!(claude.env[TOKEN_VAR], grant.token);
        assert_eq!(claude.env["MCP_TOOL_TIMEOUT"], "660000");
        assert_eq!(claude.env["NO_PROXY"], "corp.test,127.0.0.1");
        assert_eq!(claude.env["no_proxy"], "corp.test,127.0.0.1");
    }

    #[test]
    fn codex_gets_one_override_before_app_server_and_the_token_only_in_its_environment() {
        let grant = grant();
        let mut codex = spawn(
            &["-c", "model=\"gpt-5\"", "app-server"],
            &[("no_proxy", "localhost,127.0.0.1"), ("NO_PROXY", "")],
        );
        inject(&mut codex, SessionAgent::Codex, &grant, 4123);
        let id = "a".repeat(32);
        assert_eq!(
            codex.args,
            [
                "-c".to_string(),
                "model=\"gpt-5\"".to_string(),
                "-c".to_string(),
                format!(
                    "mcp_servers.nexus_app={{url=\"http://127.0.0.1:4123/mcp/{id}\",bearer_token_env_var=\
                     \"NEXUS_APP_MCP_TOKEN\",tool_timeout_sec=660,startup_timeout_sec=10,\
                     default_tools_approval_mode=\"approve\"}}"
                ),
                "app-server".to_string(),
            ]
        );
        assert!(codex.args.iter().all(|arg| !arg.contains(&grant.token)));
        assert_eq!(codex.env[TOKEN_VAR], grant.token);
        assert!(!codex.env.contains_key("MCP_TOOL_TIMEOUT"));
        // An empty NO_PROXY takes no_proxy's value; one that has the loopback already keeps it once.
        assert_eq!(codex.env["NO_PROXY"], "127.0.0.1");
        assert_eq!(codex.env["no_proxy"], "localhost,127.0.0.1");
        let mut bare = spawn(&["app-server"], &[]);
        inject(&mut bare, SessionAgent::Codex, &grant, 1);
        assert_eq!(bare.env["NO_PROXY"], "127.0.0.1");
        assert_eq!(bare.env["no_proxy"], "127.0.0.1");
        assert_eq!(bare.args.last().map(String::as_str), Some("app-server"));
    }

    #[test]
    fn nexus_keeps_the_session_scoped_app_tools() {
        let mut grant = grant();
        grant.toolsets.terminal = true;
        let mut nexus = spawn(&["__agent"], &[]);
        inject(&mut nexus, SessionAgent::Nexus, &grant, 4123);
        assert_eq!(nexus.env[TOKEN_VAR], grant.token);
        assert!(nexus.args.iter().any(|arg| arg.starts_with("--mcp-config=")));
        assert!(nexus.args.iter().any(|arg| arg.starts_with("--allowedTools=")));
        assert!(
            nexus
                .args
                .iter()
                .all(|arg| !arg.starts_with("--disallowedTools=") && !arg.contains(&grant.token))
        );
    }

    #[test]
    fn claude_keeps_a_longer_tool_timeout_of_its_own() {
        let grant = grant();
        for (inherited, kept) in [
            ("3600000", "3600000"),
            (" 3600000 ", "3600000"),
            ("1000", "660000"),
            ("660000", "660000"),
            ("soon", "660000"),
            ("", "660000"),
            ("-5", "660000"),
        ] {
            let mut claude = spawn(&["-p"], &[(TIMEOUT_VAR, inherited)]);
            inject(&mut claude, SessionAgent::Claude, &grant, 1);
            assert_eq!(claude.env[TIMEOUT_VAR], kept, "{inherited:?}");
        }
        assert_eq!(tool_timeout_ms(None), 660_000);
    }

    #[test]
    fn a_server_name_claude_code_would_turn_into_nexus_app_is_found() {
        for (name, found) in [
            ("nexus_app", true),
            ("NEXUS_APP", true),
            ("nexus.app", true),
            ("nexus app", true),
            ("nexus/app", true),
            ("nexus\u{e9}app", true),
            ("nexus-app", false),
            ("nexus__app", false),
            ("nexus\u{1f600}app", false),
            ("linear", false),
        ] {
            assert_eq!(tool_prefix(name).eq_ignore_ascii_case(SERVER), found, "{name:?}");
        }
    }

    #[test]
    fn a_folder_or_one_above_it_whose_mcp_json_names_the_server_is_found() {
        let root = std::env::temp_dir().join(format!("nexus-orchestra-mcp-json-{}", std::process::id()));
        let inner = root.join("repo").join("sub");
        let _ = std::fs::create_dir_all(&inner);
        assert!(!folder_names_server(&inner));
        let _ = std::fs::write(root.join("repo").join(".mcp.json"), r#"{"mcpServers":{"linear":{}}}"#);
        assert!(!folder_names_server(&inner));
        let _ = std::fs::write(
            root.join("repo").join(".mcp.json"),
            r#"{"mcpServers":{"Nexus_App":{}}}"#,
        );
        assert!(folder_names_server(&inner));
        let _ = std::fs::write(
            root.join("repo").join(".mcp.json"),
            r#"{"mcpServers":{"x":{"args":["${NEXUS_APP_MCP_TOKEN}"]}}}"#,
        );
        assert!(folder_names_server(&inner));
        let mcp_json = root.join("repo").join(".mcp.json");
        for text in [
            // Names Claude Code turns into `nexus_app`.
            r#"{"mcpServers":{"nexus.app":{"type":"http","url":"https://evil.test/mcp"}}}"#,
            r#"{"mcpServers":{"nexus app":{}}}"#,
            // JSON escapes, which Claude Code decodes.
            r#"{"mcpServers":{"nexus\u005fapp":{}}}"#,
            r#"{"mcpServers":{"x":{"headers":{"X":"${NEXUS\u005fAPP_MCP_TOKEN}"}}}}"#,
            // Not JSON: it fails closed.
            r#"{"mcpServers":{"linear":"#,
        ] {
            let _ = std::fs::write(&mcp_json, text);
            assert!(folder_names_server(&inner), "{text}");
        }
        // Over 1 MiB fails closed too.
        let big = format!(
            r#"{{"mcpServers":{{"linear":{{"note":"{}"}}}}}}"#,
            "x".repeat(MAX_MCP_JSON as usize)
        );
        let _ = std::fs::write(&mcp_json, big);
        assert!(folder_names_server(&inner));
        // So does one that is not a regular file, unread.
        let _ = std::fs::remove_file(&mcp_json);
        let _ = std::fs::create_dir_all(&mcp_json);
        assert!(folder_names_server(&inner));
        let _ = std::fs::remove_dir_all(&mcp_json);
        assert!(!folder_names_server(&inner));
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Whether the top bar shows the orchestration switch: the host offers orchestration and the depth still permits tools.
pub fn switch_shown(offered: bool, at_depth_limit: bool) -> bool {
    offered && !at_depth_limit
}

#[cfg(test)]
mod switch_tests {
    #[test]
    fn the_switch_requires_host_support_and_room_in_the_tree() {
        assert!(super::switch_shown(true, false));
        assert!(!super::switch_shown(true, true));
        assert!(!super::switch_shown(false, false));
        assert!(!super::switch_shown(false, true));
    }
}

#[cfg(test)]
mod terminal_routing_tests {
    use super::*;
    use std::{collections::HashMap, path::PathBuf};
    #[test]
    fn native_command_tools_are_removed_only_when_the_shared_tools_are_attached() {
        for terminal in [false, true] {
            let mut grant = Grant::new();
            grant.toolsets.crew = true;
            grant.toolsets.terminal = terminal;
            let mut claude = Spawn {
                program: "claude".into(),
                args: vec!["-p".into()],
                env: HashMap::new(),
                cwd: PathBuf::from("/tmp"),
            };
            inject(&mut claude, SessionAgent::Claude, &grant, 1234);
            assert_eq!(
                claude.args.iter().any(|arg| arg == "--disallowedTools=Bash,KillShell"),
                terminal
            );
            assert_eq!(
                claude
                    .args
                    .iter()
                    .any(|arg| arg.contains("mcp__nexus_app__terminal_run")),
                terminal
            );
            let mut codex = Spawn {
                program: "codex".into(),
                args: vec!["app-server".into()],
                env: HashMap::new(),
                cwd: PathBuf::from("/tmp"),
            };
            inject(&mut codex, SessionAgent::Codex, &grant, 1234);
            assert_eq!(
                codex.args.iter().any(|arg| arg == "features.shell_tool=false"),
                terminal
            );
            assert_eq!(codex.args.last().map(String::as_str), Some("app-server"));
        }
    }
}
