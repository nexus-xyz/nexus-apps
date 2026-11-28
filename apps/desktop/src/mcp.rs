//! MCP servers for Code sessions: the list in the Nexus
//! app's `settings.json` (`mcpServers`, edited in Settings, MCP servers), plus the built-in Nexus server, on top of the
//! servers in the agent's own config. Their secrets' values are in the Nexus app's keychain entry (`mcp-servers`) and
//! reach the agent only through its environment: Claude Code expands `${NAME}` in `--mcp-config`, Codex reads
//! `env_vars` and `env_http_headers`. The list's shape, `launch` and `builtin` are `nexus-desktop-mcp`'s, which the
//! Nexus app uses too. While the background service runs the sessions, this build hands it the list and its values
//! ([`for_service`]), and the service's agents start with them.
//!
//! The built-in server is this binary started with [`SERVE_ARG`], signed in as the app. Gateway's tools go to
//! Gateway. The pull request tools and `request_secret` go to this app's socket (`app_socket.rs`), when the run that
//! started it got one: only while this build owns the Code sessions store.

pub use nexus_desktop_core::settings::McpServer;
/// The list's server, its secrets' keychain entry and their shape, the launch arguments and the built-in server, as
/// the Nexus app has them (`nexus-desktop-mcp`).
pub use nexus_desktop_mcp::mcp::{SERVE_ARG, VAULT_ACCOUNT, Values, builtin, launch, with_app_tools};
use nexus_desktop_mcp::mcp::{TOKEN_VAR, URL_VAR, app_socket, relay_to_app, serve_lines};
use nexus_workbench::mcp_servers::{ANTIGRAVITY_CLIENT, CLIENT_VAR};
use serde_json::Value;

/// Every server's saved values, from the Nexus app's keychain entry.
pub fn saved_values() -> Values {
    crate::keychain::vault()
        .get(VAULT_ACCOUNT)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The servers' secrets: Settings' from the Nexus app's keychain entry, a plugin's from the plugins' own. Each entry is
/// read only when an enabled server that keeps its values there has a secret.
pub fn values(servers: &[McpServer]) -> Values {
    nexus_desktop_mcp::mcp::saved_for(servers, &crate::keychain::vault())
}

/// What Code sessions start with: Settings' servers, then the approved, enabled plugins (plugin API v0, only with
/// `NEXUS_DESKTOP_PLUGINS=1`).
pub fn session_servers(settings: &nexus_desktop_core::settings::Settings) -> Vec<McpServer> {
    let mut servers = settings.mcp_servers.clone();
    if let Some(config) = nexus_desktop_core::settings::config_dir() {
        let plugins = nexus_desktop_core::plugins::dir(&config);
        servers.extend(nexus_desktop_core::plugins::session::servers(
            &plugins,
            &settings.plugins,
            &settings.mcp_servers,
        ));
    }
    servers
}

/// Settings' `servers` as the background service takes them, in the sign-in and after each save: the enabled ones, with
/// their values from the Nexus app's keychain entry (`nexus_desktop_mcp::mcp::for_service`, shared with the Nexus app).
pub fn for_service(servers: &[McpServer]) -> Value {
    let handed = nexus_desktop_mcp::mcp::for_service(servers, &crate::keychain::vault());
    serde_json::to_value(handed).unwrap_or_default()
}

/// Serves the built-in Nexus server on stdin and stdout when started with [`SERVE_ARG`]; `None` otherwise.
pub fn serve() -> Option<i32> {
    if !std::env::args().any(|arg| arg == SERVE_ARG) {
        return None;
    }
    let (Ok(url), Ok(token)) = (std::env::var(URL_VAR), std::env::var(TOKEN_VAR)) else {
        eprintln!("nexus-desktop-gpui {SERVE_ARG}: start it from a Code session ({URL_VAR} and {TOKEN_VAR}).");
        return Some(1);
    };
    nexus_client::install_tls();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return Some(1);
    };
    let version = crate::channel::VERSION;
    let gateway = |api_key: Option<String>, headers: Vec<(String, String)>| {
        nexus_client::Client::new(nexus_client::ClientOptions {
            api_key,
            headers,
            client: Some(format!("desktop-mcp/{version}")),
            user_agent: Some(crate::gateway::user_agent()),
            ..nexus_client::ClientOptions::new(&url)
        })
    };
    let plain = |api_key: Option<String>, method: nexus_client::Method, path: &str, body: Option<Value>| {
        runtime.block_on(gateway(api_key, Vec::new()).call(method, path, body))
    };
    // Connector changes go as this session's own agent link, made on the first one with the person's sign-in.
    // Gateway refuses a person's own key any connector change, so without the link the organization's agent writes and
    // the person's own setting never apply. Everything else goes as the person. The link is deleted when the agent
    // closes stdin. The workbench usually ends the agent's process group first (`end_leftovers`), and then the link ends
    // on its own after 15 to 30 minutes without use.
    let label = match std::env::current_dir()
        .ok()
        .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().to_string()))
    {
        Some(folder) => format!("Nexus for desktop session in {folder}"),
        None => "Nexus for desktop session".to_string(),
    };
    let session = nexus_client::mcp::SessionLink::new(Some(token.clone()), label);
    let unfenced = |method: nexus_client::Method, path: &str, headers: Vec<(String, String)>, body: Option<Value>| {
        // The tool's own headers ride on the call, not on the link's own request.
        let with_headers = |api_key: Option<String>, method: nexus_client::Method, path: &str, body: Option<Value>| {
            runtime.block_on(gateway(api_key, headers.clone()).call(method, path, body))
        };
        session.call(&plain, &with_headers, method, path, body)
    };
    // Inside a native session, the runner that started the agent handed its fence in the environment: each
    // connector change carries it, and none goes once Gateway said the runner lost the session.
    let fence = nexus_client::fence::Fence::from_env(|name| std::env::var(name).ok());
    let send = nexus_client::mcp::fenced(fence.as_ref(), &unfenced);
    // Set when this app gave the run its socket. A loopback address only.
    let relay = app_socket().map(|(address, token)| move |message: &Value| relay_to_app(&address, &token, message));
    let relay = relay.as_ref().map(|relay| relay as &dyn Fn(&Value) -> Value);
    let mut tools = nexus_client::mcp::tools();
    // Started for Antigravity: its Gemini models take one type per property, or refuse the whole request; and it
    // forwards no tool answer to the app, so a page answer carries the viewer link for the model to print.
    let antigravity = std::env::var(CLIENT_VAR).is_ok_and(|client| client == ANTIGRAVITY_CLIENT);
    if antigravity {
        for tool in &mut tools {
            nexus_desktop_mcp::mcp::gemini_schema(&mut tool.input_schema);
        }
    }
    let memory = std::env::current_dir()
        .ok()
        .and_then(|cwd| nexus_desktop_mcp::mcp::memory_session(&cwd));
    let local = memory
        .as_ref()
        .map(nexus_workbench::memory::local_tools)
        .unwrap_or_default();
    let ui = nexus_client::config::ui_url(&url, &|name| std::env::var(name).ok());
    serve_lines(
        std::io::stdin().lock(),
        &mut |line| println!("{line}"),
        &|message| {
            let mut answer = nexus_client::mcp::handle_with(&tools, &local, version, message, &send)?;
            if antigravity {
                nexus_desktop_mcp::mcp::with_view_link(message, &mut answer, &ui);
            }
            Some(answer)
        },
        relay,
    );
    session.end(&plain);
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::event::SessionAgent;
    use std::collections::BTreeMap;

    fn local(name: &str, secrets: &[&str]) -> McpServer {
        McpServer {
            name: name.into(),
            enabled: true,
            command: Some("srv".into()),
            args: vec!["--x".into()],
            url: None,
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            tool_timeout_sec: None,
            plugin: None,
        }
    }

    #[test]
    fn passes_enabled_servers_with_secrets_through_the_environment() {
        let mut off = local("off", &[]);
        off.enabled = false;
        let servers = [local("db", &["DB_KEY"]), off];
        let values = Values::from([("db".into(), BTreeMap::from([("DB_KEY".into(), "s3cret".into())]))]);
        let (args, env) = launch(SessionAgent::Claude, &servers, &values);
        assert_eq!(args.len(), 1);
        assert!(args[0].starts_with("--mcp-config="));
        assert!(args[0].contains("${DB_KEY}") && !args[0].contains("s3cret") && !args[0].contains("\"off\""));
        assert_eq!(env, vec![("DB_KEY".to_string(), "s3cret".to_string())]);
        let (args, _) = launch(SessionAgent::Codex, &servers, &values);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("mcp_servers.db={command=\"srv\""));
    }

    /// Every tool the built-in server lists, in Gemini's form, names one type per property: the contract's type lists
    /// (`["string", "null"]`) made agy's every turn fail (Vertex: "didn't specify the schema type field").
    #[test]
    fn every_builtin_tool_has_one_type_per_property_for_gemini() {
        let tools = nexus_client::mcp::tools();
        assert!(
            tools
                .iter()
                .any(|tool| tool.input_schema.to_string().contains("\"type\":["))
        );
        for tool in tools {
            let mut input = tool.input_schema.clone();
            nexus_desktop_mcp::mcp::gemini_schema(&mut input);
            assert!(!input.to_string().contains("\"type\":["), "{}", tool.name);
        }
    }

    #[test]
    fn the_persons_nexus_server_wins_over_the_builtin() {
        assert!(builtin(&[], "/app", "https://g", "t").is_some());
        assert!(builtin(&[local("Nexus", &[])], "/app", "https://g", "t").is_none());
    }

    #[test]
    fn the_builtin_gets_the_app_socket_only_through_the_environment() {
        let (mut server, mut values) = builtin(&[], "/app", "https://g", "t").expect("the built-in");
        with_app_tools(&mut server, &mut values, "127.0.0.1:4123", "app_tok");
        let (args, env) = launch(SessionAgent::Codex, std::slice::from_ref(&server), &values);
        assert!(!args.concat().contains("app_tok"));
        // Codex waits for `request_secret` longer than its own minute.
        assert!(args[1].contains("tool_timeout_sec="));
        assert!(env.contains(&("NEXUS_MCP_APP_ADDR".to_string(), "127.0.0.1:4123".to_string())));
    }

    #[test]
    fn reads_the_settings_shape() {
        let servers: Vec<McpServer> =
            serde_json::from_str(r#"[{"name":"web","url":"https://m","secrets":["Authorization"]}]"#).unwrap();
        assert!(servers[0].enabled);
        assert_eq!(servers[0].url.as_deref(), Some("https://m"));
    }
}
