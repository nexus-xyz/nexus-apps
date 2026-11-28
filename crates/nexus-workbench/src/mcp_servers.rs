//! The MCP servers configured for Code sessions. Settings and nexus-host supply the servers and their values.
//! Claude Code and Codex get only environment variable references in their arguments. The environment holds the
//! values. The built-in server is added through [`builtin`] and [`with_app_tools`].

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::event::SessionAgent;
use crate::harness::{LauncherDelivery, McpDelivery, harness};

/// By server, then by variable or header name.
pub type Values = BTreeMap<String, BTreeMap<String, String>>;

/// What the built-in server's relayed `request_secret` calls say in `tools`, so the socket hands them to the app's card.
pub const SECRET_TOOLS: &str = "secret";

/// How long `request_secret` waits for the person. A card still open then closes, and the call says so.
pub const SECRET_WAIT: Duration = Duration::from_secs(10 * 60);

/// An MCP server configured for Code sessions in Settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    /// Its name in the agents' tool names (`mcp__<name>__<tool>`): letters, digits, `-` and `_`.
    pub name: String,
    #[serde(default = "on")]
    pub enabled: bool,
    /// A local server: the program the agent starts, with `args`. Exactly one of `command` and `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// A remote server, over streamable HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The names of a local server's environment variables, or of a remote server's headers. Their values are in the
    /// keychain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// How long Codex waits for one of a local server's tools, in seconds (its `tool_timeout_sec`, 60 by default).
    /// Not in Settings: only the built-in server sets it, for `request_secret`, which waits for the person.
    #[serde(skip)]
    pub tool_timeout_sec: Option<u64>,
    /// A plugin's folder (plugin API v0, [`crate::plugins`]). A local plugin runs there, and only through the launcher
    /// with no credential variable, under every agent ([`through_launcher_or_builtin`]). Settings' servers have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
}

impl McpServer {
    /// A local plugin: it never starts but through the launcher.
    pub fn launcher_only(&self) -> bool {
        self.plugin.is_some() && self.command.is_some()
    }
}

fn on() -> bool {
    true
}

/// What a session's agent starts with for the enabled servers: arguments to put before its own, and variables to add
/// to its environment (before Nexus's own, which win).
/// An agent that takes its servers in its session ([`session`]) gets none here.
/// A local plugin is never among them: it starts only through the launcher ([`through_launcher_or_builtin`]).
pub fn launch(agent: SessionAgent, servers: &[McpServer], values: &Values) -> (Vec<String>, Vec<(String, String)>) {
    let direct: Vec<McpServer> = servers
        .iter()
        .filter(|server| !server.launcher_only())
        .cloned()
        .collect();
    match harness(agent).mcp_servers(&direct, values) {
        McpDelivery::Args { args, env } => (args, env),
        McpDelivery::Session(_) | McpDelivery::NotOffered => (Vec::new(), Vec::new()),
    }
}

/// Whether the agent takes its MCP servers in its session protocol (Antigravity: ACP's `session/new`).
pub fn in_session(agent: SessionAgent) -> bool {
    matches!(harness(agent).mcp_servers(&[], &Values::new()), McpDelivery::Session(_))
}

/// The enabled servers for an agent that takes them in its session; None for any other agent.
pub fn session(agent: SessionAgent, servers: &[McpServer], values: &Values) -> Option<SessionServers> {
    match harness(agent).mcp_servers(servers, values) {
        McpDelivery::Session(servers) => Some(servers),
        McpDelivery::Args { .. } | McpDelivery::NotOffered => None,
    }
}

/// One local server as the launcher starts it: its command, arguments and whole added environment, values included.
/// Only the app holds it; the agent gets the launcher's arguments ([`session_entries`]).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Launchable {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// What the server keeps of the environment the launcher inherited from the agent. The app socket's answers carry
    /// it as `strip_credentials`; an answer without it keeps today's [`InheritedEnv::AppCredentialOnly`].
    #[serde(default, rename = "strip_credentials")]
    pub inherited: InheritedEnv,
    /// The folder it runs in: a plugin's own. None runs it where the agent runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// What a server the launcher starts keeps of the environment the launcher inherited from the agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InheritedEnv {
    /// All of it but the launcher's own app socket credential: Antigravity's servers, whose agent holds no Nexus
    /// credential in its environment.
    #[default]
    #[serde(rename = "app_credential_only")]
    AppCredentialOnly,
    /// All of it but every credential variable ([`crate::credential_env`]): Claude Code's servers
    /// ([`launcher_config`]), which inherit Claude Code's whole environment, the Nexus token included.
    #[serde(rename = "no_credentials")]
    NoCredentials,
}

/// The servers for an agent that takes them in its session: the local ones the launcher starts, and the remote ones
/// it does not take, by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionServers {
    pub launchable: crate::app_socket::Launchables,
    pub skipped: Vec<String>,
}

impl SessionServers {
    /// Adds `key=value` to the built-in server's environment ([`builtin`]: named nexus, started with [`SERVE_ARG`]
    /// alone), if it is one of them. A Settings server of the same name replaces it, and gets nothing.
    pub(crate) fn builtin_env(&mut self, key: &str, value: &str) {
        if let Some(server) = self
            .launchable
            .get_mut(BUILTIN)
            .filter(|server| server.args == [SERVE_ARG])
        {
            server.env.insert(key.into(), value.into());
        }
    }
}

/// The agent the built-in server serves, when it needs the server to differ from what Claude Code and Codex get
/// ([`ANTIGRAVITY_CLIENT`]).
pub const CLIENT_VAR: &str = "NEXUS_MCP_CLIENT";
/// Antigravity (agy 1.5.15): its Gemini models take one type per tool property (nexus-desktop-mcp's
/// `gemini_schema`), and it forwards no tool's answer to the app over ACP, so a page answer carries the viewer link
/// for the model to print (nexus-desktop-mcp's `with_view_link`).
pub const ANTIGRAVITY_CLIENT: &str = "antigravity";

/// The enabled local servers as the launcher starts them, each with its own secrets' values. Remote servers are
/// listed in `skipped`: an agent that takes only local servers in its session cannot reach them.
pub(crate) fn session_servers(servers: &[McpServer], values: &Values) -> SessionServers {
    let mut session = SessionServers::default();
    for (_, server) in enabled(servers) {
        let Some(command) = &server.command else {
            session.skipped.push(server.name.clone());
            continue;
        };
        let own = values.get(&server.name);
        let env = server
            .secrets
            .iter()
            .filter_map(|secret| Some((secret.clone(), own?.get(secret)?.clone())))
            .collect();
        // A plugin keeps no credential variable under any agent, Antigravity's included.
        let inherited = match server.plugin {
            Some(_) => InheritedEnv::NoCredentials,
            None => InheritedEnv::AppCredentialOnly,
        };
        let launchable = Launchable {
            command: command.clone(),
            args: server.args.clone(),
            env,
            inherited,
            cwd: server.plugin.clone(),
        };
        session.launchable.insert(server.name.clone(), launchable);
    }
    session
}

/// `nexus-desktop --mcp-launch=<name>`: starts one of the run's servers, as the app answers for the launch credential
/// in its environment ([`APP_ADDRESS_VAR`], [`APP_TOKEN_VAR`]); nexus-desktop-mcp's `launch`. The name rides in the
/// same argument: a server named like another mode's flag (`--mcp`) never becomes an argument of its own.
pub const LAUNCH_ARG: &str = "--mcp-launch";

/// The launcher's one argument for the server `name` ([`LAUNCH_ARG`]).
pub fn launch_arg(name: &str) -> String {
    format!("{LAUNCH_ARG}={name}")
}

/// What a session-protocol agent gets for its MCP servers at launch ([`crate::Launch::session_mcp`]).
#[derive(Clone, Default, PartialEq)]
pub struct SessionMcp {
    /// ACP's stdio servers: `{name, command, args, env: [{name, value}]}`.
    pub servers: Vec<serde_json::Value>,
    /// The remote servers it does not take, by name, for the session's notice.
    pub skipped: Vec<String>,
}

impl SessionMcp {
    /// The servers the agent gets, by name.
    pub fn names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter_map(|server| server["name"].as_str().map(String::from))
            .collect()
    }
}

/// The ACP entries for `servers`: each starts `program` as the launcher for its name, with the app's socket and the
/// launch credential as its only environment. The agent may store this environment in plain text (agy writes it to its
/// conversation file); the credential ends with the run, and the servers' own values never leave the app.
pub fn session_entries(program: &str, address: &str, token: &str, servers: &SessionServers) -> SessionMcp {
    let env = serde_json::json!([
        {"name": APP_ADDRESS_VAR, "value": address},
        {"name": APP_TOKEN_VAR, "value": token},
    ]);
    SessionMcp {
        servers: servers
            .launchable
            .keys()
            .map(|name| serde_json::json!({"name": name, "command": program, "args": [launch_arg(name)], "env": env}))
            .collect(),
        skipped: servers.skipped.clone(),
    }
}

/// What an agent that starts its local servers through the launcher gets ([`Harness::mcp_launcher`]): its arguments
/// and variables, with no local server's value in either, and the servers the launcher may start, values included,
/// for the run's launch credential.
///
/// [`Harness::mcp_launcher`]: crate::harness::Harness::mcp_launcher
#[derive(Debug, Clone, PartialEq)]
pub struct LauncherConfig {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub launchable: crate::app_socket::Launchables,
}

/// The launcher's configuration for the enabled `servers`, each local one started as `program --mcp-launch=<name>`;
/// None for an agent that does not start its servers so. Each local server loses the credential variables it would
/// inherit from the agent ([`crate::credential_env`]).
pub(crate) fn launcher_config(
    agent: SessionAgent,
    program: &str,
    servers: &[McpServer],
    values: &Values,
) -> Option<LauncherConfig> {
    let LauncherDelivery { args, env } = harness(agent).mcp_launcher(program, servers, values)?;
    let mut launchable = session_servers(servers, values).launchable;
    for server in launchable.values_mut() {
        server.inherited = InheritedEnv::NoCredentials;
    }
    Some(LauncherConfig { args, env, launchable })
}

/// What a launch through the launcher starts with ([`through_launcher_or_builtin`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LauncherLaunch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The enabled servers that do not start: none when the launcher has its credential.
    pub left_out: Vec<String>,
}

/// The servers of an agent that starts them through the launcher, None for any other agent. `grant` makes the run's
/// launch credential for the servers it is given, and answers the app socket's address and the credential. Without
/// one, there is no launcher, and only the built-in server starts, directly: no other server starts with the agent's
/// environment. An agent that starts its servers itself (Codex) still starts each local plugin through the launcher
/// ([`direct_with_plugins`]); it gets None only when there is no plugin.
pub fn through_launcher_or_builtin(
    agent: SessionAgent,
    program: &str,
    servers: &[McpServer],
    values: &Values,
    grant: impl FnOnce(crate::app_socket::Launchables) -> Option<(String, String)>,
) -> Option<LauncherLaunch> {
    let Some(LauncherConfig {
        args,
        mut env,
        launchable,
    }) = launcher_config(agent, program, servers, values)
    else {
        return direct_with_plugins(agent, program, servers, values, grant);
    };
    if let Some((address, token)) = grant(launchable) {
        env.push((APP_ADDRESS_VAR.into(), address));
        env.push((APP_TOKEN_VAR.into(), token));
        return Some(LauncherLaunch {
            args,
            env,
            left_out: Vec::new(),
        });
    }
    let is_builtin = |server: &McpServer| {
        server.name == BUILTIN && server.args == [SERVE_ARG] && server.command.as_deref() == Some(program)
    };
    let builtin: Vec<McpServer> = servers.iter().filter(|server| is_builtin(server)).cloned().collect();
    let left_out = enabled(servers)
        .filter(|(_, server)| !is_builtin(server))
        .map(|(_, server)| server.name.clone())
        .collect();
    let (args, env) = launch(agent, &builtin, values);
    Some(LauncherLaunch { args, env, left_out })
}

pub(crate) fn check_credential_name(servers: &[McpServer], variable: &str) -> Result<(), String> {
    if let Some((_, server)) = enabled(servers).find(|(_, server)| {
        server.command.is_some()
            && server
                .secrets
                .iter()
                .any(|name| crate::local_provider::same_env_name(name, variable))
    }) {
        return Err(format!(
            "MCP server {} declares {variable}. Rename that secret before starting this session.",
            server.name
        ));
    }
    Ok(())
}

fn codex_launcher(program: &str, servers: &[McpServer], values: &Values) -> LauncherConfig {
    let entries: Vec<_> = servers
        .iter()
        .cloned()
        .map(|mut server| {
            if server.command.is_some() {
                server.command = Some(program.into());
                server.args = vec![launch_arg(&server.name)];
                server.secrets = vec![APP_ADDRESS_VAR.into(), APP_TOKEN_VAR.into()];
            }
            server
        })
        .collect();
    let (args, _) = launch(SessionAgent::Codex, &entries, &Values::new());
    let env = header_env(servers, values);
    let mut launchable = session_servers(servers, values).launchable;
    for server in launchable.values_mut() {
        server.inherited = InheritedEnv::NoCredentials;
    }
    LauncherConfig { args, env, launchable }
}

/// For an agent that starts its servers itself (Codex): its servers, with each local plugin replaced by the
/// launcher's entry, `program --mcp-launch=<id>`. Codex gives a server only a base environment and the variables its
/// `env_vars` names, so the entry names the launch credential's own two ([`LAUNCH_ADDRESS_VAR`],
/// [`LAUNCH_TOKEN_VAR`]). The launcher adds the plugin's values from the app and strips every credential variable;
/// the values never reach Codex's environment. `grant` gets the plugins as the launcher starts them. Without a
/// credential, no plugin starts and the other servers start as before. None when the agent has no local plugin, or
/// does not start its servers so.
fn direct_with_plugins(
    agent: SessionAgent,
    program: &str,
    servers: &[McpServer],
    values: &Values,
    grant: impl FnOnce(crate::app_socket::Launchables) -> Option<(String, String)>,
) -> Option<LauncherLaunch> {
    let plugins: Vec<McpServer> = servers
        .iter()
        .filter(|server| server.enabled && server.launcher_only())
        .cloned()
        .collect();
    let starts_its_own = matches!(
        harness(agent).mcp_servers(&[], &Values::new()),
        McpDelivery::Args { .. }
    );
    if plugins.is_empty() || !starts_its_own {
        return None;
    }
    let launchable = session_servers(&plugins, values).launchable;
    let names: Vec<String> = launchable.keys().cloned().collect();
    let mut direct: Vec<McpServer> = servers
        .iter()
        .filter(|server| !server.launcher_only())
        .cloned()
        .collect();
    let mut values = values.clone();
    let credential = if program.is_empty() { None } else { grant(launchable) };
    let left_out = match credential {
        Some((address, token)) => {
            for name in names {
                let own = BTreeMap::from([
                    (LAUNCH_ADDRESS_VAR.to_string(), address.clone()),
                    (LAUNCH_TOKEN_VAR.to_string(), token.clone()),
                ]);
                values.insert(name.clone(), own);
                direct.push(McpServer {
                    args: vec![launch_arg(&name)],
                    name,
                    enabled: true,
                    command: Some(program.into()),
                    url: None,
                    secrets: vec![LAUNCH_ADDRESS_VAR.into(), LAUNCH_TOKEN_VAR.into()],
                    tool_timeout_sec: None,
                    plugin: None,
                });
            }
            Vec::new()
        }
        None => names,
    };
    let (args, env) = launch(agent, &direct, &values);
    Some(LauncherLaunch { args, env, left_out })
}

/// Configures Settings servers for a signed-out run. The socket only grants server starts.
pub fn configure_local(
    launch: &mut crate::Launch,
    agent: SessionAgent,
    program: Option<&Path>,
    servers: &[McpServer],
    values: &Values,
    grant: impl FnOnce(&str, crate::app_socket::Launchables) -> Option<(String, String)>,
) {
    let Some(run) = launch.local_run.as_deref() else {
        return;
    };
    let program = program.map(Path::to_string_lossy);
    if let Some(session) = session(agent, servers, values) {
        launch.session_mcp = match program.as_deref().and_then(|program| {
            let (address, token) = grant(run, session.launchable.clone())?;
            Some(session_entries(program, &address, &token, &session))
        }) {
            Some(entries) => entries,
            None => SessionMcp {
                servers: Vec::new(),
                skipped: enabled(servers).map(|(_, s)| s.name.clone()).collect(),
            },
        };
        return;
    }

    let exe = program.as_deref().unwrap_or_default();
    let config = if launch.local_provider.is_some() && agent == SessionAgent::Codex {
        Some(codex_launcher(exe, servers, values))
    } else {
        launcher_config(agent, exe, servers, values)
    };
    let Some(config) = config else {
        if launch.local_provider.is_some() {
            launch.session_mcp.skipped = enabled(servers).map(|(_, server)| server.name.clone()).collect();
            return;
        }
        let direct = direct_with_plugins(agent, exe, servers, values, |launchable| grant(run, launchable));
        let (args, env) = match direct {
            Some(launched) => {
                launch.session_mcp.skipped = launched.left_out;
                (launched.args, launched.env)
            }
            None => self::launch(agent, servers, values),
        };
        launch.spawn.args.splice(0..0, args);
        launch.spawn.env.extend(env);
        return;
    };
    let LauncherConfig {
        mut args,
        mut env,
        launchable,
    } = config;
    if !launchable.is_empty() {
        let names = launchable.keys().cloned().collect();
        if let Some((address, token)) = program.and_then(|_| grant(run, launchable)) {
            env.push((APP_ADDRESS_VAR.into(), address));
            env.push((APP_TOKEN_VAR.into(), token));
        } else {
            launch.session_mcp.skipped = names;
            let remote: Vec<_> = servers
                .iter()
                .filter(|server| server.command.is_none())
                .cloned()
                .collect();
            (args, env) = self::launch(agent, &remote, values);
        }
    }
    launch.spawn.args.splice(0..0, args);
    launch.spawn.env.extend(env);
}

/// The session's notice for the servers that did not start ([`LauncherLaunch::left_out`]); None when all did.
pub fn left_out_notice(left_out: &[String]) -> Option<String> {
    (!left_out.is_empty()).then(|| {
        format!(
            "These MCP servers did not start in this session: {}. Start the session again to try them again.",
            left_out.join(", ")
        )
    })
}

pub(crate) fn enabled(servers: &[McpServer]) -> impl Iterator<Item = (usize, &McpServer)> {
    servers.iter().filter(|server| server.enabled).enumerate()
}

pub(crate) fn header_var(server: usize, header: usize) -> String {
    format!("NEXUS_MCP_{server}_{header}")
}

/// Both launch formats refer to the same environment values in their server configs.
pub(crate) fn launch_env(servers: &[McpServer], values: &Values) -> Vec<(String, String)> {
    env_of(servers, values, true)
}

/// Only the remote servers' header values: the launcher hands each local server its own values.
pub(crate) fn header_env(servers: &[McpServer], values: &Values) -> Vec<(String, String)> {
    env_of(servers, values, false)
}

fn env_of(servers: &[McpServer], values: &Values, local: bool) -> Vec<(String, String)> {
    let mut env = Vec::new();
    for (i, server) in enabled(servers) {
        let value = |key: &str| values.get(&server.name).and_then(|values| values.get(key));
        if server.command.is_some() {
            if !local {
                continue;
            }
            // Two servers naming the same variable share the later server's value.
            for secret in &server.secrets {
                if let Some(value) = value(secret) {
                    env.push((secret.clone(), value.clone()));
                }
            }
        } else if server.url.is_some() {
            for (j, header) in server.secrets.iter().enumerate() {
                env.push((header_var(i, j), value(header).cloned().unwrap_or_default()));
            }
        }
    }
    env
}

/// `nexus-desktop --mcp`: the built-in Nexus server (`nexus_client::mcp`).
pub const SERVE_ARG: &str = "--mcp";
pub const URL_VAR: &str = "NEXUS_MCP_URL";
pub const TOKEN_VAR: &str = "NEXUS_MCP_TOKEN";
/// The app's loopback socket and the run's credential for it ([`crate::app_socket`]), for the pull request tools and
/// `request_secret`.
pub const APP_ADDRESS_VAR: &str = "NEXUS_MCP_APP_ADDR";
pub const APP_TOKEN_VAR: &str = "NEXUS_MCP_APP_TOKEN";
/// The app's socket and the run's launch credential, for a plugin's launcher under an agent that starts its servers
/// itself (Codex, [`through_launcher_or_builtin`]). Their own names: the built-in server's [`APP_TOKEN_VAR`] holds
/// another credential in the same environment. The launcher reads these first.
pub const LAUNCH_ADDRESS_VAR: &str = "NEXUS_MCP_LAUNCH_ADDR";
pub const LAUNCH_TOKEN_VAR: &str = "NEXUS_MCP_LAUNCH_TOKEN";
pub(crate) const BUILTIN: &str = "nexus";

/// What the built-in server's relayed calls say in `tools`, so the socket hands them to the pull request watch.
pub const PULL_REQUEST_TOOLS: &str = "pull_request";

/// How long a relayed call waits for the app's reply, unless it says otherwise.
pub const REPLY_WAIT: Duration = Duration::from_secs(15);

/// The Nexus server every session gets: this app's own binary with [`SERVE_ARG`], signed in as the app. The Gateway
/// and the token reach it as its secrets do, through the agent's environment, which already holds the token on both
/// routes. `None` when Settings has a server of the same name: the person's wins.
pub fn builtin(servers: &[McpServer], program: &str, gateway: &str, token: &str) -> Option<(McpServer, Values)> {
    if named(servers, BUILTIN) {
        return None;
    }
    let server = McpServer {
        name: BUILTIN.into(),
        enabled: true,
        command: Some(program.into()),
        args: vec![SERVE_ARG.into()],
        url: None,
        secrets: vec![URL_VAR.into(), TOKEN_VAR.into()],
        tool_timeout_sec: None,
        plugin: None,
    };
    let values = BTreeMap::from([(URL_VAR.into(), gateway.into()), (TOKEN_VAR.into(), token.into())]);
    Some((server, Values::from([(BUILTIN.into(), values)])))
}

/// Gives the built-in server the session's pull request tools and `request_secret`: the app's socket and the run's credential for it, through the agent's environment as its
/// other secrets. Codex waits for a tool for a minute by default; `request_secret` waits for the person longer.
pub fn with_app_tools(server: &mut McpServer, values: &mut Values, address: &str, token: &str) {
    server.secrets.extend([APP_ADDRESS_VAR.into(), APP_TOKEN_VAR.into()]);
    server.tool_timeout_sec = Some(SECRET_WAIT.as_secs() + 60);
    let own = values.entry(server.name.clone()).or_default();
    own.insert(APP_ADDRESS_VAR.into(), address.into());
    own.insert(APP_TOKEN_VAR.into(), token.into());
}

pub fn with_shared_memory(server: &mut McpServer, values: &mut Values, agent: SessionAgent, session: &str) {
    if let Ok(store) = nexus_agent_memory::Store::local() {
        add_memory_env(server, values, agent, session, &store);
    }
}

fn add_memory_env(
    server: &mut McpServer,
    values: &mut Values,
    agent: SessionAgent,
    session: &str,
    store: &nexus_agent_memory::Store,
) {
    use nexus_agent_memory::tools::{AGENT_ENV, ROOT_ENV, SESSION_ENV};
    let Some(root) = store.root().to_str().filter(|_| store.enabled().unwrap_or(false)) else {
        return;
    };
    // Codex forwards only the configured names to stdio servers. Pass the selected store by name too.
    server
        .secrets
        .extend([AGENT_ENV.into(), SESSION_ENV.into(), ROOT_ENV.into()]);
    let own = values.entry(server.name.clone()).or_default();
    own.insert(AGENT_ENV.into(), agent.as_str().into());
    own.insert(SESSION_ENV.into(), session.into());
    own.insert(ROOT_ENV.into(), root.into());
}

/// `nexus-desktop --browser-mcp`: the agent's browser server, `nexus_browser` (nexus-desktop-mcp's `browser`). It
/// relays each call to the app's socket with its run's credential, from its environment only. Its calls carry no
/// `tools`, which sends them to the session's browser.
pub const BROWSER_ARG: &str = "--browser-mcp";
pub const BROWSER_ADDRESS_VAR: &str = "NEXUS_BROWSER_ADDR";
pub const BROWSER_TOKEN_VAR: &str = "NEXUS_BROWSER_TOKEN";
/// The server's name in the agent's config, and in `initialize`.
pub const BROWSER: &str = "nexus_browser";
/// How long a browser call may wait for the person to allow the browser (`nexus_browser::access::WAIT`).
pub const BROWSER_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long the agent waits for one browser tool: the person's answer, and a little more.
pub const BROWSER_TOOL_TIMEOUT_SEC: u64 = 660;

/// Whether Settings has a server named `name`: then the person's wins over the app's own.
pub fn named(servers: &[McpServer], name: &str) -> bool {
    servers.iter().any(|server| server.name.eq_ignore_ascii_case(name))
}

/// The browser server for one run, with the app's socket and the run's credential, unless Settings has a server of
/// the same name.
pub fn browser(servers: &[McpServer], program: &str, address: &str, token: &str) -> Option<(McpServer, Values)> {
    if named(servers, BROWSER) {
        return None;
    }
    let server = McpServer {
        name: BROWSER.into(),
        enabled: true,
        command: Some(program.into()),
        args: vec![BROWSER_ARG.into()],
        url: None,
        secrets: vec![BROWSER_ADDRESS_VAR.into(), BROWSER_TOKEN_VAR.into()],
        tool_timeout_sec: Some(BROWSER_TOOL_TIMEOUT_SEC),
        plugin: None,
    };
    let values = BTreeMap::from([
        (BROWSER_ADDRESS_VAR.into(), address.to_string()),
        (BROWSER_TOKEN_VAR.into(), token.to_string()),
    ]);
    Some((server, Values::from([(BROWSER.into(), values)])))
}

/// Settings' servers as an app hands them to its background service (nexus-host's `local`): the enabled servers, and
/// only the values their secrets name. The service keeps them in memory, never on disk.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Handed {
    #[serde(default)]
    pub servers: Vec<McpServer>,
    #[serde(default)]
    pub values: Values,
}

/// The names of each server's values, never the values: what [`Handed`]'s `{:?}` shows.
fn value_names(values: &Values) -> BTreeMap<&str, Vec<&str>> {
    values
        .iter()
        .map(|(server, values)| (server.as_str(), values.keys().map(String::as_str).collect()))
        .collect()
}

/// The environment shows by variable name only: its values are secrets and launch credentials.
impl std::fmt::Debug for Launchable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launchable")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("inherited", &self.inherited)
            .finish()
    }
}

/// The servers show by name only: their `env` holds secret values.
impl std::fmt::Debug for SessionMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionMcp")
            .field("servers", &self.names())
            .field("skipped", &self.skipped)
            .finish()
    }
}

/// The values show by name only.
impl std::fmt::Debug for Handed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handed")
            .field("servers", &self.servers)
            .field("values", &value_names(&self.values))
            .finish()
    }
}

/// The service's call that takes a [`Handed`] list again, after Settings saved it. The sign-in carries it too.
pub const SERVICE_CALL: &str = "host_mcp_servers";

/// What an app hands its service for Settings' `servers`, given every saved value (`values`, from the keychain).
pub fn handed(servers: &[McpServer], values: &Values) -> Handed {
    let servers: Vec<McpServer> = servers.iter().filter(|server| server.enabled).cloned().collect();
    let values = servers
        .iter()
        .filter_map(|server| {
            let saved = values.get(&server.name)?;
            let own: BTreeMap<String, String> = server
                .secrets
                .iter()
                .filter_map(|secret| Some((secret.clone(), saved.get(secret)?.clone())))
                .collect();
            (!own.is_empty()).then(|| (server.name.clone(), own))
        })
        .collect();
    Handed { servers, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_names_never_secret_values() {
        let launchable = Launchable {
            command: "nexus".into(),
            args: vec!["mcp".into()],
            env: BTreeMap::from([("NEXUS_MCP_TOKEN".into(), "launch_secret".into())]),
            inherited: InheritedEnv::NoCredentials,
            cwd: None,
        };
        let session = SessionMcp {
            servers: vec![serde_json::json!({
                "name": "db",
                "command": "db-mcp",
                "env": [{"name": "DB_KEY", "value": "session_secret"}],
            })],
            skipped: Vec::new(),
        };
        let handed = Handed {
            servers: Vec::new(),
            values: BTreeMap::from([("db".into(), BTreeMap::from([("DB_KEY".into(), "handed_secret".into())]))]),
        };
        let shown = format!("{launchable:?} {session:?} {handed:?}");
        for secret in ["launch_secret", "session_secret", "handed_secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        for name in ["NEXUS_MCP_TOKEN", "db", "DB_KEY"] {
            assert!(shown.contains(name), "{shown}");
        }
    }

    fn server(name: &str, enabled: bool, secrets: &[&str]) -> McpServer {
        McpServer {
            name: name.into(),
            enabled,
            command: Some("srv".into()),
            args: Vec::new(),
            url: None,
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            tool_timeout_sec: None,
            plugin: None,
        }
    }

    #[test]
    fn the_service_gets_the_enabled_servers_and_only_their_named_values() {
        let servers = [server("db", true, &["DB_TOKEN"]), server("off", false, &["OFF_TOKEN"])];
        let values = Values::from([
            (
                "db".into(),
                BTreeMap::from([("DB_TOKEN".into(), "s1".into()), ("STALE".into(), "old".into())]),
            ),
            ("off".into(), BTreeMap::from([("OFF_TOKEN".into(), "s2".into())])),
            ("gone".into(), BTreeMap::from([("X".into(), "s3".into())])),
        ]);
        let handed = handed(&servers, &values);
        assert_eq!(handed.servers, vec![servers[0].clone()]);
        assert_eq!(
            handed.values,
            Values::from([("db".into(), BTreeMap::from([("DB_TOKEN".into(), "s1".into())]))])
        );
        // As the service reads it back from the hello.
        let read: Handed =
            serde_json::from_value(serde_json::to_value(&handed).unwrap_or_default()).unwrap_or_default();
        assert_eq!(read, handed);
    }

    #[test]
    fn codex_forwards_the_shared_memory_store_to_the_builtin_server() {
        use nexus_agent_memory::tools::ROOT_ENV;

        let root = std::env::temp_dir().join(format!("nexus-memory-launch-{}", uuid::Uuid::new_v4()));
        let store = nexus_agent_memory::Store::new(root.clone());
        let (mut server, mut values) =
            builtin(&[], "/opt/nexus", "https://example.test", "fixture").expect("built-in server");
        add_memory_env(&mut server, &mut values, SessionAgent::Codex, "test-session", &store);
        assert!(!server.secrets.iter().any(|name| name == ROOT_ENV));

        store.set_enabled(true).expect("enable shared memory");
        add_memory_env(&mut server, &mut values, SessionAgent::Codex, "test-session", &store);
        let (args, env) = launch(SessionAgent::Codex, &[server], &values);
        assert!(
            args.iter()
                .any(|arg| { arg.contains("env_vars=[") && arg.contains(&format!("{ROOT_ENV:?}")) })
        );
        assert!(env.contains(&(ROOT_ENV.into(), root.display().to_string())));
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn configured_servers_keep_secrets_in_the_environment() {
        let mut http = server("remote", true, &["Authorization", "Missing"]);
        http.command = None;
        http.url = Some("https://mcp.example.test".into());
        let mut local = server("db", true, &["DB_TOKEN"]);
        local.tool_timeout_sec = Some(660);
        let servers = [local, server("off", false, &["OFF_TOKEN"]), http];
        let values = Values::from([
            ("db".into(), BTreeMap::from([("DB_TOKEN".into(), "db-secret".into())])),
            (
                "off".into(),
                BTreeMap::from([("OFF_TOKEN".into(), "disabled-secret".into())]),
            ),
            (
                "remote".into(),
                BTreeMap::from([("Authorization".into(), "http-secret".into())]),
            ),
        ]);
        let expected_env: Vec<(String, String)> = vec![
            ("DB_TOKEN".into(), "db-secret".into()),
            ("NEXUS_MCP_1_0".into(), "http-secret".into()),
            ("NEXUS_MCP_1_1".into(), String::new()),
        ];
        let (claude, env) = launch(SessionAgent::Claude, &servers, &values);
        assert_eq!(env, expected_env);
        let config = claude[0].strip_prefix("--mcp-config=").expect("a Claude config");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(config).expect("MCP configuration"),
            serde_json::json!({"mcpServers": {
                "db": {"type": "stdio", "command": "srv", "args": [], "env": {"DB_TOKEN": "${DB_TOKEN}"}},
                "remote": {"type": "http", "url": "https://mcp.example.test", "headers": {
                    "Authorization": "${NEXUS_MCP_1_0}", "Missing": "${NEXUS_MCP_1_1}"
                }}
            }})
        );
        let (codex, env) = launch(SessionAgent::Codex, &servers, &values);
        assert_eq!(env, expected_env);
        assert_eq!(
            codex,
            [
                "-c",
                r#"mcp_servers.db={command="srv",args=[],env_vars=["DB_TOKEN"],tool_timeout_sec=660}"#,
                "-c",
                r#"mcp_servers.remote={url="https://mcp.example.test",env_http_headers={"Authorization"="NEXUS_MCP_1_0","Missing"="NEXUS_MCP_1_1"}}"#,
            ]
        );
        assert_eq!(launch(SessionAgent::Nexus, &servers, &values), (Vec::new(), Vec::new()));
    }

    /// Claude Code starts every local server through the launcher: its config names only the launcher, its environment
    /// holds no local server's value, and the launcher's servers keep their values and lose the credential variables.
    /// A remote server is as before, its header at the index it has among all the enabled servers.
    #[test]
    fn claude_starts_every_stdio_server_through_the_launcher() {
        let mut remote = server("remote", true, &["Authorization"]);
        remote.command = None;
        remote.url = Some("https://mcp.example.test".into());
        let servers = [server("db", true, &["DB_TOKEN"]), server("off", false, &[]), remote];
        let values = Values::from([
            ("db".into(), BTreeMap::from([("DB_TOKEN".into(), "db-secret".into())])),
            (
                "remote".into(),
                BTreeMap::from([("Authorization".into(), "http-secret".into())]),
            ),
        ]);
        for other in [SessionAgent::Codex, SessionAgent::Antigravity] {
            assert_eq!(launcher_config(other, "/opt/nexus", &servers, &values), None);
        }
        let config = launcher_config(SessionAgent::Claude, "/opt/nexus", &servers, &values).expect("Claude's");
        let mcp = config.args[0].strip_prefix("--mcp-config=").expect("a Claude config");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(mcp).expect("MCP configuration"),
            serde_json::json!({"mcpServers": {
                "db": {"type": "stdio", "command": "/opt/nexus", "args": ["--mcp-launch=db"], "env": {}},
                "remote": {"type": "http", "url": "https://mcp.example.test", "headers": {
                    "Authorization": "${NEXUS_MCP_1_0}"
                }}
            }})
        );
        assert_eq!(config.env, [("NEXUS_MCP_1_0".to_string(), "http-secret".to_string())]);
        assert!(!config.args.concat().contains("db-secret") && !config.args.concat().contains("DB_TOKEN"));
        assert_eq!(config.launchable.keys().collect::<Vec<_>>(), ["db"]);
        assert_eq!(config.launchable["db"].env["DB_TOKEN"], "db-secret");
        assert_eq!(config.launchable["db"].inherited, InheritedEnv::NoCredentials);
        // agy's servers keep the default.
        let agy = session(SessionAgent::Antigravity, &servers, &values).expect("agy's servers");
        assert_eq!(agy.launchable["db"].inherited, InheritedEnv::AppCredentialOnly);

        // With the launcher's credential, the agent gets the socket and the credential, and every server starts.
        let launched = through_launcher_or_builtin(SessionAgent::Claude, "/opt/nexus", &servers, &values, |servers| {
            assert_eq!(servers.keys().collect::<Vec<_>>(), ["db"]);
            Some(("127.0.0.1:4123".into(), "launch_tok".into()))
        })
        .expect("Claude's");
        assert!(launched.left_out.is_empty());
        assert!(
            launched
                .env
                .contains(&(APP_ADDRESS_VAR.into(), "127.0.0.1:4123".into()))
        );
        assert!(launched.env.contains(&(APP_TOKEN_VAR.into(), "launch_tok".into())));
        assert_eq!(
            through_launcher_or_builtin(SessionAgent::Codex, "/opt/nexus", &servers, &values, |_| None),
            None
        );
    }

    /// Without the launcher's credential, only the built-in server starts, directly; no other server starts with
    /// Claude Code's environment, and the session says which did not.
    #[test]
    fn without_the_launcher_only_the_builtin_server_starts() {
        let (nexus, mut values) = builtin(&[], "/opt/nexus", "https://gw", "tok").expect("the built-in");
        let mut remote = server("remote", true, &[]);
        remote.command = None;
        remote.url = Some("https://mcp.example.test".into());
        let servers = [nexus, server("db", true, &["DB_TOKEN"]), remote];
        values.insert("db".into(), BTreeMap::from([("DB_TOKEN".into(), "db-secret".into())]));
        let launched = through_launcher_or_builtin(SessionAgent::Claude, "/opt/nexus", &servers, &values, |_| None)
            .expect("Claude's");
        assert_eq!(launched.left_out, ["db", "remote"]);
        let mcp: serde_json::Value =
            serde_json::from_str(launched.args[0].strip_prefix("--mcp-config=").expect("a config")).expect("JSON");
        assert_eq!(
            mcp["mcpServers"]
                .as_object()
                .map(|servers| servers.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["nexus".to_string()])
        );
        assert_eq!(mcp["mcpServers"]["nexus"]["command"], "/opt/nexus");
        assert!(!launched.env.iter().any(|(_, value)| value == "db-secret"));
        assert_eq!(
            left_out_notice(&launched.left_out).as_deref(),
            Some(
                "These MCP servers did not start in this session: db, remote. Start the session again to try them again."
            )
        );
        assert_eq!(left_out_notice(&[]), None);
    }

    /// agy takes its servers in `session/new`: nothing reaches its arguments or its environment, where its shell would
    /// inherit them. Each local server's values stay with the app, for the launcher; a remote server is skipped.
    #[test]
    fn antigravity_takes_local_servers_in_its_session_and_no_secret_leaves_the_app() {
        let mut remote = server("docs", true, &["Authorization"]);
        remote.command = None;
        remote.url = Some("https://mcp.example.test".into());
        let servers = [server("db", true, &["DB_TOKEN"]), server("off", false, &[]), remote];
        let values = Values::from([("db".into(), BTreeMap::from([("DB_TOKEN".into(), "s1".into())]))]);
        assert_eq!(
            launch(SessionAgent::Antigravity, &servers, &values),
            (Vec::new(), Vec::new())
        );
        assert!(in_session(SessionAgent::Antigravity));
        assert!(!in_session(SessionAgent::Claude) && !in_session(SessionAgent::Codex));
        assert_eq!(session(SessionAgent::Claude, &servers, &values), None);

        let session = session(SessionAgent::Antigravity, &servers, &values).expect("agy's servers");
        assert_eq!(session.skipped, ["docs"]);
        assert_eq!(session.launchable.keys().collect::<Vec<_>>(), ["db"]);
        assert_eq!(session.launchable["db"].env["DB_TOKEN"], "s1");

        let entries = session_entries("/opt/nexus", "127.0.0.1:4123", "launch_tok", &session);
        assert_eq!(entries.skipped, ["docs"]);
        assert_eq!(
            entries.servers,
            [
                serde_json::json!({"name": "db", "command": "/opt/nexus", "args": ["--mcp-launch=db"], "env": [
                    {"name": APP_ADDRESS_VAR, "value": "127.0.0.1:4123"},
                    {"name": APP_TOKEN_VAR, "value": "launch_tok"},
                ]})
            ]
        );
        assert!(
            !serde_json::to_string(&entries.servers)
                .unwrap_or_default()
                .contains("s1")
        );

        // agy's models take one type per property: only the built-in server is told to serve its schemas so.
        let (nexus, mut nexus_values) = builtin(&servers, "/opt/nexus", "https://gw", "tok").expect("the built-in");
        nexus_values.extend(values.clone());
        let with_builtin = [nexus, servers[0].clone()];
        let session = super::session(SessionAgent::Antigravity, &with_builtin, &nexus_values).expect("agy's servers");
        assert_eq!(session.launchable["nexus"].env[CLIENT_VAR], ANTIGRAVITY_CLIENT);
        assert!(!session.launchable["db"].env.contains_key(CLIENT_VAR));
    }
}
