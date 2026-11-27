//! MCP servers (Settings, MCP servers): one list the app hands to every Claude Code and Codex session when its agent
//! starts, new or resumed and on any account, on top of the servers in the agent's own config. The list is in
//! `settings.json`; the values of its servers' environment variables and headers are in the keychain (one entry,
//! [`VAULT_ACCOUNT`], read only when an enabled server has one) and reach the agent only through its environment,
//! never its arguments or a file: Claude Code expands `${NAME}` in `--mcp-config`, Codex reads `env_vars` and
//! `env_http_headers`. The built-in server's process (`serve`) and the Settings commands are the shell's.

use std::collections::BTreeSet;

use nexus_workbench::orchestra;
use serde::Serialize;
use serde_json::{Value, json};

use nexus_desktop_core::secrets::Vault;

/// The keychain entry holding every server's values: `{"<server>": {"<variable or header>": "<value>"}}`.
pub const VAULT_ACCOUNT: &str = "mcp-servers";

/// A relayed call's answer: `nexus_workbench::app_socket`'s, which the background service uses too.
pub use nexus_workbench::app_socket::relayed;
/// The servers' formatting for the agents and the built-in server: `nexus_workbench::mcp_servers`, which nexus-host
/// uses too. `McpServer` is the server in settings.json.
pub use nexus_workbench::mcp_servers::{
    APP_ADDRESS_VAR, APP_TOKEN_VAR, Handed, McpServer, PULL_REQUEST_TOOLS, REPLY_WAIT, SERVE_ARG, SERVICE_CALL,
    TOKEN_VAR, URL_VAR, Values, builtin, launch, with_app_tools,
};

/// A server as Settings shows it: which of its secrets have a value saved (never the values).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    #[serde(flatten)]
    pub server: McpServer,
    pub saved: Vec<String>,
}

pub fn values(vault: &Vault) -> Values {
    vault
        .get(VAULT_ACCOUNT)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Settings' `servers` as the app hands them to the background service (nexus-host's `local`), in each sign-in and
/// after each save: the enabled ones, with their values from the keychain. The keychain is read only when an enabled
/// server has a secret, as at an agent's start.
pub fn for_service(servers: &[McpServer], vault: &Vault) -> Handed {
    let saved = if servers.iter().any(|s| s.enabled && !s.secrets.is_empty()) {
        values(vault)
    } else {
        Values::new()
    };
    nexus_workbench::mcp_servers::handed(servers, &saved)
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_variable(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A header name is an HTTP token.
fn valid_header(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Why the list cannot be saved, in words for Settings.
pub fn check(servers: &[McpServer]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for server in servers {
        let name = &server.name;
        if !valid_name(name) {
            return Err(format!(
                "“{name}” can't be a server's name: use letters, digits, - and _."
            ));
        }
        if !names.insert(name.to_lowercase()) {
            return Err(format!("There are two servers named {name}."));
        }
        // Nexus's own server for linked sessions, whose tools are pre-allowed by name (nexus_workbench::orchestra).
        if name.eq_ignore_ascii_case(orchestra::SERVER) {
            return Err(format!(
                "{name} is the name of Nexus's own app tools. Choose another name."
            ));
        }
        // Its token reaches the agent in this variable: no other server may name it, or expand it.
        let names_token = |text: &str| text.to_ascii_uppercase().contains(orchestra::TOKEN_VAR);
        if server.secrets.iter().any(|secret| names_token(secret))
            || server.args.iter().any(|arg| names_token(arg))
            || [&server.command, &server.url]
                .into_iter()
                .flatten()
                .any(|text| names_token(text))
        {
            return Err(format!(
                "{name} can't use {}: Nexus keeps it for its own tools.",
                orchestra::TOKEN_VAR
            ));
        }
        let command = server.command.as_deref().is_some_and(|c| !c.trim().is_empty());
        let url = server.url.as_deref().is_some_and(|u| !u.trim().is_empty());
        match (command, url) {
            (true, true) | (false, false) => return Err(format!("{name} needs a command or a URL, not both.")),
            (false, true)
                if !server
                    .url
                    .as_deref()
                    .is_some_and(|u| u.starts_with("https://") || u.starts_with("http://")) =>
            {
                return Err(format!("{name}'s URL must start with https:// or http://."));
            }
            _ => {}
        }
        let mut seen = BTreeSet::new();
        for secret in &server.secrets {
            let fits = if command {
                valid_variable(secret)
            } else {
                valid_header(secret)
            };
            if !fits {
                let what = if command { "an environment variable" } else { "a header" };
                return Err(format!("“{secret}” can't be {what} of {name}."));
            }
            if !seen.insert(secret.to_lowercase()) {
                return Err(format!("{name} names {secret} twice."));
            }
        }
    }
    Ok(())
}

/// The values to keep after a save: what was given, else what was saved, for the secrets the servers still name.
pub fn merge(servers: &[McpServer], saved: &Values, given: &Values) -> Values {
    let mut kept = Values::new();
    for server in servers {
        for secret in &server.secrets {
            let pick = |values: &Values| {
                values
                    .get(&server.name)
                    .and_then(|v| v.get(secret))
                    .filter(|v| !v.is_empty())
                    .cloned()
            };
            if let Some(value) = pick(given).or_else(|| pick(saved)) {
                kept.entry(server.name.clone())
                    .or_default()
                    .insert(secret.clone(), value);
            }
        }
    }
    kept
}

/// The built-in server's routing, its relay to the app's socket and its loop: `nexus_workbench`'s, which `nexus-host`
/// serves on a person's own server too.
pub use nexus_workbench::mcp_bridge::{relay_to_app, route, serve_lines};

/// A tool's input schema in the form Gemini's function calling takes (Antigravity's models): `nexus_workbench`'s,
/// which `nexus-host` uses too.
pub use nexus_workbench::mcp_bridge::gemini_schema;

/// Adds a `View it:` line with the page's viewer link to a page tool's answer, for an agent that forwards no tool answer
/// to the app (Antigravity: agy 1.5.15's ACP `tool_call_update` for an MCP tool has no content). The app finds the
/// page only in the agent's reply then, so the model needs a link to print. `ui`: Nexus UI's base URL. The
/// `/app?next=/pages/<id>` link opens the page through the person's active organization (`page_links::page_link`).
pub fn with_view_link(message: &Value, answer: &mut Value, ui: &str) {
    if message["method"] != "tools/call" {
        return;
    }
    let tool = message["params"]["name"].as_str().unwrap_or_default();
    // An error answer has no `result`, and gets none: JSON-RPC allows one of the two.
    let Some(result) = answer.get_mut("result") else {
        return;
    };
    if result["isError"] == true {
        return;
    }
    let text: String = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect();
    // The page tools as the app names them (`page_links::PAGE_TOOLS`).
    let named = format!("mcp__nexus__{tool}");
    let Some(page) = nexus_desktop_core::page_links::page_in_answer(&named, &text) else {
        return;
    };
    let link = format!("{}/app?next=/pages/{}", ui.trim_end_matches('/'), page.page_id);
    if let Some(content) = result["content"].as_array_mut() {
        content.push(json!({ "type": "text", "text": format!("View it: {link}") }));
    }
}

/// The app's socket and the run's credential for it, from the built-in server's environment ([`with_app_tools`]). A
/// loopback address only. `nexus_workbench`'s, which `nexus-host` uses too.
pub use nexus_workbench::mcp_bridge::app_socket_env as app_socket;

/// The servers as Settings shows them, with which of their secrets have a value saved.
pub fn view(servers: Vec<McpServer>, values: &Values) -> Vec<McpServerView> {
    servers
        .into_iter()
        .map(|server| McpServerView {
            saved: values
                .get(&server.name)
                .map(|v| v.keys().cloned().collect())
                .unwrap_or_default(),
            server,
        })
        .collect()
}

pub fn memory_session(cwd: &std::path::Path) -> Option<nexus_agent_memory::tools::Session> {
    nexus_agent_memory::tools::Session::from_mcp_env(cwd, &|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::event::SessionAgent;
    use std::collections::BTreeMap;

    /// A page answer carries the link the app reads back from the agent's reply; any other answer stays as it is.
    #[test]
    fn a_page_answer_carries_its_viewer_link_the_app_reads() {
        let call = |name: &str| json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name}});
        let page = json!({"page": {"id": "pag_abc123", "title": "C2"}, "version": {"n": 1}}).to_string();
        let answer = |text: &str, error: bool| json!({"jsonrpc": "2.0", "id": 3, "result": {"content": [{"type": "text", "text": text}], "isError": error}});
        let mut created = answer(&page, false);
        with_view_link(&call("createPage"), &mut created, "https://app.nexus.test/");
        let line = created["result"]["content"][1]["text"].as_str().unwrap_or_default();
        assert_eq!(line, "View it: https://app.nexus.test/app?next=/pages/pag_abc123");
        let link = line.trim_start_matches("View it: ");
        assert_eq!(
            nexus_desktop_core::page_links::page_link(link).as_deref(),
            Some("pag_abc123")
        );
        // A JSON-RPC error answer stays one: no `result` beside its `error`.
        let mut refused = json!({"jsonrpc": "2.0", "id": 3, "error": {"code": -32602, "message": "Unknown tool"}});
        let before = refused.clone();
        with_view_link(&call("createPage"), &mut refused, "https://app.nexus.test");
        assert_eq!(refused, before);
        for (name, error) in [("createPage", true), ("listPages", false)] {
            let mut other = answer(&page, error);
            let before = other.clone();
            with_view_link(&call(name), &mut other, "https://app.nexus.test");
            assert_eq!(other, before, "{name}");
        }
    }

    fn local() -> McpServer {
        McpServer {
            name: "linear".into(),
            enabled: true,
            command: Some("npx".into()),
            args: vec!["-y".into(), "linear-mcp".into()],
            url: None,
            secrets: vec!["LINEAR_KEY".into()],
            tool_timeout_sec: None,
        }
    }

    fn remote() -> McpServer {
        McpServer {
            name: "docs".into(),
            enabled: true,
            command: None,
            args: vec![],
            url: Some("https://mcp.example.test/mcp".into()),
            secrets: vec!["Authorization".into()],
            tool_timeout_sec: None,
        }
    }

    fn saved() -> Values {
        Values::from([
            ("linear".into(), BTreeMap::from([("LINEAR_KEY".into(), "lin_1".into())])),
            (
                "docs".into(),
                BTreeMap::from([("Authorization".into(), "Bearer d_1".into())]),
            ),
        ])
    }

    #[test]
    fn claude_gets_one_config_with_no_secret_in_it() {
        let (args, env) = launch(SessionAgent::Claude, &[local(), remote()], &saved());
        assert_eq!(args.len(), 1);
        let config: Value = serde_json::from_str(args[0].strip_prefix("--mcp-config=").expect("flag")).expect("json");
        assert_eq!(
            config,
            json!({"mcpServers": {
                "linear": {"type": "stdio", "command": "npx", "args": ["-y", "linear-mcp"], "env": {"LINEAR_KEY": "${LINEAR_KEY}"}},
                "docs": {"type": "http", "url": "https://mcp.example.test/mcp", "headers": {"Authorization": "${NEXUS_MCP_1_0}"}},
            }})
        );
        assert!(!args[0].contains("lin_1") && !args[0].contains("d_1"));
        assert_eq!(
            env,
            [
                ("LINEAR_KEY".to_string(), "lin_1".to_string()),
                ("NEXUS_MCP_1_0".to_string(), "Bearer d_1".to_string())
            ]
        );
    }

    #[test]
    fn codex_gets_overrides_with_no_secret_in_them() {
        let (args, env) = launch(SessionAgent::Codex, &[local(), remote()], &saved());
        assert_eq!(
            args,
            [
                "-c",
                r#"mcp_servers.linear={command="npx",args=["-y","linear-mcp"],env_vars=["LINEAR_KEY"]}"#,
                "-c",
                r#"mcp_servers.docs={url="https://mcp.example.test/mcp",env_http_headers={"Authorization"="NEXUS_MCP_1_0"}}"#,
            ]
        );
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn every_session_gets_the_nexus_server_with_its_token_only_in_the_environment() {
        let (server, values) = builtin(
            &[local()],
            "/Applications/Nexus.app/nexus-desktop",
            "https://gw.test",
            "nx_tok",
        )
        .expect("the built-in");
        let (args, env) = launch(SessionAgent::Claude, std::slice::from_ref(&server), &values);
        let config: Value = serde_json::from_str(args[0].strip_prefix("--mcp-config=").expect("flag")).expect("json");
        assert_eq!(
            config["mcpServers"]["nexus"],
            json!({"type": "stdio", "command": "/Applications/Nexus.app/nexus-desktop", "args": ["--mcp"],
                   "env": {"NEXUS_MCP_URL": "${NEXUS_MCP_URL}", "NEXUS_MCP_TOKEN": "${NEXUS_MCP_TOKEN}"}})
        );
        assert!(!args[0].contains("nx_tok"));
        assert!(env.contains(&("NEXUS_MCP_TOKEN".to_string(), "nx_tok".to_string())));
        assert!(env.contains(&("NEXUS_MCP_URL".to_string(), "https://gw.test".to_string())));
        let (args, _) = launch(SessionAgent::Codex, &[server], &values);
        assert_eq!(
            args,
            [
                "-c",
                r#"mcp_servers.nexus={command="/Applications/Nexus.app/nexus-desktop",args=["--mcp"],env_vars=["NEXUS_MCP_URL","NEXUS_MCP_TOKEN"]}"#
            ]
        );
    }

    #[test]
    fn the_built_in_gets_the_app_socket_and_its_credential_only_in_the_environment() {
        let (mut server, mut values) = builtin(&[], "/x", "https://gw.test", "nx_tok").expect("the built-in");
        with_app_tools(&mut server, &mut values, "127.0.0.1:4123", "app_tok");
        let (args, env) = launch(SessionAgent::Claude, std::slice::from_ref(&server), &values);
        assert!(!args[0].contains("app_tok") && !args[0].contains("4123"));
        assert!(args[0].contains("${NEXUS_MCP_APP_TOKEN}"));
        assert!(env.contains(&("NEXUS_MCP_APP_ADDR".to_string(), "127.0.0.1:4123".to_string())));
        assert!(env.contains(&("NEXUS_MCP_APP_TOKEN".to_string(), "app_tok".to_string())));
        // Settings refuses neither name: they are not the linked sessions' token.
        assert!(!APP_TOKEN_VAR.contains(orchestra::TOKEN_VAR));
        // Codex waits for `request_secret` longer than the person may take; Claude Code's wait is long already.
        let (args, _) = launch(SessionAgent::Codex, std::slice::from_ref(&server), &values);
        assert!(args[1].ends_with(&format!(
            ",tool_timeout_sec={}}}",
            crate::secret_request::WAIT.as_secs() + 60
        )));
        // Settings never saves it.
        assert!(!serde_json::to_string(&server).expect("json").contains("timeout"));
    }

    fn gateway_tools(message: &Value) -> Option<Value> {
        let id = message.get("id")?.clone();
        match message["method"].as_str() {
            Some("tools/list") => {
                Some(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "listPages"}]}}))
            }
            _ => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"gateway": true}})),
        }
    }

    #[test]
    fn the_pull_request_tools_go_to_the_app_and_everything_else_to_the_gateway() {
        let relay = |message: &Value| json!({"jsonrpc": "2.0", "id": message["id"], "result": {"app": true}});
        let relay: Option<&dyn Fn(&Value) -> Value> = Some(&relay);
        let list = route(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            gateway_tools,
            relay,
        )
        .expect("answer");
        let names: Vec<_> = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            [
                "listPages",
                "watch_pull_request",
                "unwatch_pull_request",
                "request_secret"
            ]
        );
        for tool in nexus_pull_requests::watch::TOOL_NAMES
            .into_iter()
            .chain([crate::secret_request::TOOL_NAME])
        {
            let call =
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": {}}});
            assert_eq!(
                route(&call, gateway_tools, relay).expect("answer")["result"]["app"],
                true,
                "{tool}"
            );
        }
        let other = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "listPages"}});
        assert_eq!(
            route(&other, gateway_tools, relay).expect("answer")["result"]["gateway"],
            true
        );
        // A notification gets no answer.
        let note = json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "watch_pull_request"}});
        assert_eq!(route(&note, gateway_tools, relay), None);
    }

    #[test]
    fn without_the_app_socket_the_server_is_the_gateways_alone() {
        let list = route(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            gateway_tools,
            None,
        )
        .expect("answer");
        assert_eq!(list["result"]["tools"].as_array().expect("tools").len(), 1);
        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "watch_pull_request"}});
        assert_eq!(
            route(&call, gateway_tools, None).expect("answer")["result"]["gateway"],
            true
        );
    }

    #[test]
    fn a_relay_that_fails_is_a_tool_error_with_the_call_id() {
        let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "watch_pull_request"}});
        let failed = relayed(&call, Err("connection refused".into()));
        assert_eq!(failed["id"], 7);
        assert_eq!(failed["result"]["isError"], true);
        assert!(
            failed["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("connection refused")
        );
        let garbled = relayed(&call, Ok("not json\n".into()));
        assert_eq!(garbled["result"]["isError"], true);
        let fine = relayed(&call, Ok("{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n".into()));
        assert_eq!(fine, json!({"jsonrpc": "2.0", "id": 7, "result": {}}));
    }

    #[test]
    fn the_loop_answers_each_line_and_relays_the_app_tools() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"request_secret"}}"#,
            "\n\nnot json\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"listPages"}}"#,
            "\n",
        );
        let relay = |message: &Value| json!({"jsonrpc": "2.0", "id": message["id"], "result": {"app": true}});
        let mut lines = Vec::new();
        serve_lines(
            input.as_bytes(),
            &mut |line| lines.push(serde_json::from_str::<Value>(line).expect("json")),
            &gateway_tools,
            Some(&relay),
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["result"]["app"], true);
        assert_eq!(lines[1]["error"]["code"], -32700);
        assert_eq!(lines[2]["result"]["gateway"], true);
    }

    #[test]
    fn a_relay_to_an_app_that_is_gone_is_a_tool_error() {
        // A port nothing listens on: bound, then let go.
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("address");
        let call = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "watch_pull_request"}});
        let answer = relay_to_app(&address, "t", &call);
        assert_eq!(answer["id"], 4);
        assert_eq!(answer["result"]["isError"], true);
    }

    #[test]
    fn a_settings_server_named_nexus_replaces_the_built_in() {
        let mine = McpServer {
            name: "Nexus".into(),
            ..local()
        };
        assert_eq!(builtin(&[mine], "/x", "https://gw.test", "nx_tok"), None);
    }

    #[test]
    fn a_disabled_server_is_left_out() {
        let off = McpServer {
            enabled: false,
            ..local()
        };
        assert_eq!(launch(SessionAgent::Claude, &[off], &saved()), (vec![], vec![]));
    }

    #[test]
    fn checks_the_list() {
        assert_eq!(check(&[local(), remote()]), Ok(()));
        let bad = |change: fn(&mut McpServer)| {
            let mut server = local();
            change(&mut server);
            check(&[server]).is_err()
        };
        assert!(bad(|s| s.name = "has space".into()));
        assert!(bad(|s| s.url = Some("https://x.test".into())));
        assert!(bad(|s| s.command = None));
        assert!(bad(|s| s.secrets = vec!["1BAD".into()]));
        assert!(bad(|s| s.secrets = vec!["A".into(), "a".into()]));
        assert!(check(&[local(), local()]).is_err());
        assert!(
            check(&[McpServer {
                url: Some("ftp://x".into()),
                ..remote()
            }])
            .is_err()
        );
    }

    #[test]
    fn refuses_the_name_and_the_token_variable_nexus_keeps_for_linked_sessions() {
        for name in ["nexus_app", "Nexus_App", "NEXUS_APP"] {
            let mine = McpServer {
                name: name.into(),
                ..local()
            };
            assert!(check(&[mine]).is_err(), "{name}");
        }
        assert_eq!(
            check(&[McpServer {
                name: "nexus_apps".into(),
                ..local()
            }]),
            Ok(())
        );
        let bad = |change: fn(&mut McpServer)| {
            let mut server = local();
            change(&mut server);
            check(&[server]).is_err()
        };
        assert!(bad(|s| s.secrets = vec!["NEXUS_APP_MCP_TOKEN".into()]));
        assert!(bad(|s| s.secrets = vec!["nexus_app_mcp_token".into()]));
        assert!(bad(|s| s.args = vec!["--token=${NEXUS_APP_MCP_TOKEN}".into()]));
        let mut header = remote();
        header.secrets = vec!["NEXUS_APP_MCP_TOKEN".into()];
        assert!(check(&[header]).is_err());
        let mut url = remote();
        url.url = Some("https://x.test/${NEXUS_APP_MCP_TOKEN}".into());
        assert!(check(&[url]).is_err());
    }

    #[test]
    fn a_save_keeps_what_was_not_given_and_drops_what_is_gone() {
        let given = Values::from([(
            "docs".into(),
            BTreeMap::from([("Authorization".into(), "Bearer d_2".into())]),
        )]);
        let kept = merge(&[local(), remote()], &saved(), &given);
        assert_eq!(kept["linear"]["LINEAR_KEY"], "lin_1");
        assert_eq!(kept["docs"]["Authorization"], "Bearer d_2");
        assert_eq!(merge(&[remote()], &saved(), &Values::new()).len(), 1);
    }
}
