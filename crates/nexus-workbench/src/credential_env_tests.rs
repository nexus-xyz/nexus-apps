use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use nexus_client::agents::{LaunchOptions, Route};

use super::stripped;
use crate::event::SessionAgent;
use crate::mcp_servers::{self, McpServer, Values};
use crate::process::Spawn;

/// Every credential in a test launch holds this, so the test finds each variable that carries one by its value.
const SENTINEL: &str = "sentinel-credential";

fn sentinel(of: &str) -> String {
    format!("{SENTINEL}-{of}")
}

fn local(name: &str, secrets: &[&str]) -> McpServer {
    McpServer {
        name: name.into(),
        enabled: true,
        command: Some(format!("{name}-mcp")),
        args: Vec::new(),
        url: None,
        secrets: secrets.iter().map(|secret| secret.to_string()).collect(),
        tool_timeout_sec: None,
    }
}

/// Claude Code's environment as the app builds it, on `route`: the built-in server with its tool credential, a
/// Settings server with a secret, a remote one with a header, the browser server, the orchestration server and the
/// agent's own launch, each credential a sentinel.
async fn claude_environment(route: Route) -> HashMap<String, String> {
    let program = "/opt/Nexus/nexus-desktop";
    let mut remote = local("docs", &["Authorization"]);
    remote.command = None;
    remote.url = Some("https://mcp.example.test".into());
    let mut servers = vec![local("db", &["DB_TOKEN"]), remote];
    let mut values = Values::from([
        (
            "db".into(),
            BTreeMap::from([("DB_TOKEN".into(), "db-own-value".into())]),
        ),
        (
            "docs".into(),
            BTreeMap::from([("Authorization".into(), "docs-own-value".into())]),
        ),
    ]);
    let nexus_token = sentinel("nexus-token");
    let (mut builtin, mut own) = mcp_servers::builtin(&servers, program, "https://gw.example.test", &nexus_token)
        .unwrap_or_else(|| panic!("the built-in server"));
    mcp_servers::with_app_tools(&mut builtin, &mut own, "127.0.0.1:1", &sentinel("app-tools"));
    servers.insert(0, builtin);
    values.extend(own);
    let (browser, own) = mcp_servers::browser(&servers, program, "127.0.0.1:1", &sentinel("browser"))
        .unwrap_or_else(|| panic!("the browser server"));
    servers.push(browser);
    values.extend(own);
    let launched = mcp_servers::through_launcher_or_builtin(SessionAgent::Claude, program, &servers, &values, |_| {
        Some(("127.0.0.1:1".into(), sentinel("launch")))
    })
    .unwrap_or_else(|| panic!("Claude Code starts its servers through the launcher"));
    let mut env: HashMap<String, String> = launched.env.into_iter().collect();
    // The person's own key in their shell: Nexus removes it on both routes.
    env.insert("ANTHROPIC_API_KEY".into(), sentinel("person-key"));
    let mut spawn = Spawn {
        program: PathBuf::from("/nonexistent/claude"),
        args: launched.args,
        env,
        cwd: std::env::temp_dir(),
    };
    let mut grant = crate::orchestra::Grant::new();
    grant.token = sentinel("orchestra");
    crate::orchestra::inject(&mut spawn, SessionAgent::Claude, &grant, 1);
    let (_, env) = crate::agent_launch::launch(
        nexus_agents::Agent::Claude,
        &spawn.program,
        &std::env::temp_dir(),
        true,
        LaunchOptions {
            route,
            core_url: "https://gw.example.test",
            api_key: &nexus_token,
            session_id: "ns_1",
            team: Some("core"),
            model: None,
            model_catalog: Some(Path::new("/nonexistent/catalog.json")),
            web_search: false,
            args: spawn.args,
            env: spawn.env,
        },
    )
    .await;
    env
}

/// The list's guarantee for what Nexus's own code sets: Claude Code's launch on each route (`agent_launch::launch`), the
/// MCP servers' configuration and the orchestration server. A credential any of them sets in Claude Code's environment
/// fails this test until the list names it. Variables a host adds from the person's own files (nexus-host's
/// `agent.env`) are the person's, and are not covered.
#[tokio::test]
async fn no_launch_variable_carries_a_credential_past_the_launcher() {
    for route in [Route::Gateway, Route::Subscription] {
        let env = claude_environment(route).await;
        let carried: Vec<&String> = env
            .iter()
            .filter(|(_, value)| value.contains(SENTINEL))
            .map(|(name, _)| name)
            .collect();
        // Not vacuous: the Nexus token is in the environment on both routes.
        assert!(
            env.values().any(|value| value.contains(&sentinel("nexus-token"))),
            "{route:?}: {carried:?}"
        );
        for name in &carried {
            assert!(
                stripped(name),
                "{route:?}: {name} carries a credential past the launcher"
            );
        }
        // The servers' own values stay in the app; the remote server's header is Claude Code's to send.
        assert!(!env.values().any(|value| value == "db-own-value"), "{route:?}");
        assert!(env.values().any(|value| value == "docs-own-value"), "{route:?}");
    }
}

/// The person's own variables reach their servers: only names Nexus sets or removes for Claude Code are listed.
#[test]
fn the_persons_own_variables_are_not_listed() {
    for name in [
        "PATH",
        "HOME",
        "GITHUB_TOKEN",
        "OPENAI_API_KEY",
        "GOOGLE_CLOUD_PROJECT",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "HTTPS_PROXY",
        "NODE_OPTIONS",
        nexus_client::fence::SESSION_VAR,
        nexus_client::fence::TOKEN_VAR,
    ] {
        assert!(!stripped(name), "{name}");
    }
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "NEXUS_API_KEY",
        "NEXUS_OPENCODE_API_KEY",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "NEXUS_APP_MCP_TOKEN",
        "NEXUS_MCP_TOKEN",
        "NEXUS_MCP_APP_TOKEN",
        "NEXUS_MCP_2_0",
        "NEXUS_BROWSER_TOKEN",
    ] {
        assert!(stripped(name), "{name}");
    }
}
