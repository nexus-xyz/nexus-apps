//! The app's tools on a person's own server: the binary serves them, and a client Gateway relays may not
//! hand the host MCP servers or answer its agents' tool calls.

mod common;

use std::process::Stdio;
use std::sync::Arc;

use common::start;
use futures::{SinkExt, StreamExt};
use nexus_host::server::{PROTOCOL, Peer, RELAYED_APP_REPLY, RELAYED_MCP_SERVERS, relayed_refusal, serve_socket};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

/// `nexus-host <args>` with none of a run's variables: its exit code and what it said.
fn run(args: &[&str]) -> (Option<i32>, String) {
    let output = nexus_workbench::process::command(env!("CARGO_BIN_EXE_nexus-host"))
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_binary_serves_the_apps_tools_and_needs_a_runs_variables() {
    let (code, said) = run(&["--mcp"]);
    assert_eq!(code, Some(1));
    assert!(said.contains("NEXUS_MCP_URL"), "{said}");
    // The browser's server and the launcher refuse to start without their credential too, instead of the usage.
    let (code, said) = run(&["--browser-mcp"]);
    assert_eq!(code, Some(1));
    assert!(!said.contains("Usage"), "{said}");
    let (code, said) = run(&["--mcp-launch=db"]);
    assert_eq!(code, Some(1));
    assert!(said.contains("--mcp-launch db"), "{said}");
    // Anything else is still the host's own command line.
    let (code, said) = run(&["--nonsense"]);
    assert_eq!(code, Some(1));
    assert!(said.contains("Usage"), "{said}");
}

/// Claude Code starts the server through the launcher with its whole environment. The server gets its own values and
/// the person's variables, and none of the credentials Claude Code holds.
#[cfg(unix)]
#[test]
fn the_launcher_starts_claudes_server_without_credentials() {
    use nexus_host::app_relay::{AppRelay, AppTools, Current};
    use nexus_host::server::Hub;
    use nexus_workbench::event::SessionAgent;
    use nexus_workbench::mcp_servers::{APP_ADDRESS_VAR, APP_TOKEN_VAR, Handed};

    let root = common::root("launcher");
    let script = root.join("db-mcp");
    nexus_workbench::process::write_fake_script(&script, "#!/bin/sh\nenv\n");

    let sign_in = nexus_workbench::session_choice::sign_in_identity("nx_tok");
    let current: Current = Arc::new(move || ("gw|acme".to_string(), sign_in.clone()));
    let relay = AppRelay::bind(Hub::new(), current).expect("bind");
    let tools = AppTools::new(env!("CARGO_BIN_EXE_nexus-host").into(), relay.clone());
    let handed: Handed = serde_json::from_value(json!({
        "servers": [{"name": "db", "command": script.to_string_lossy(), "secrets": ["DB_TOKEN"]}],
        "values": {"db": {"DB_TOKEN": "db-own"}},
    }))
    .expect("handed");
    tools.set_servers(handed);
    let launched = tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_1", "https://gw", "nx_tok");
    let value = |name: &str| {
        launched
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("no {name} in {:?}", launched.env))
    };

    let output = nexus_workbench::process::command(env!("CARGO_BIN_EXE_nexus-host"))
        .arg("--mcp-launch=db")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env(APP_ADDRESS_VAR, value(APP_ADDRESS_VAR))
        .env(APP_TOKEN_VAR, value(APP_TOKEN_VAR))
        .env("ANTHROPIC_AUTH_TOKEN", "dummy")
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("NEXUS_MCP_TOKEN", "dummy")
        .env("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "dummy")
        .env("CLAUDE_CODE_MESSAGING_TOKEN", "dummy")
        .env("PERSON_OWN", "kept")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{:?}: {stderr}", output.status);
    let lines: Vec<&str> = said.lines().collect();
    assert!(lines.contains(&"DB_TOKEN=db-own"), "{said}\n{stderr}");
    assert!(lines.contains(&"PERSON_OWN=kept"), "{said}\n{stderr}");
    let names: Vec<&str> = lines.iter().filter_map(|line| line.split('=').next()).collect();
    for credential in [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "NEXUS_MCP_TOKEN",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        APP_TOKEN_VAR,
    ] {
        assert!(!names.contains(&credential), "{credential} reached the server: {said}");
    }
}

#[test]
fn only_a_client_gateway_relays_is_refused_the_apps_hand_offs() {
    for (call, refusal) in [
        (nexus_workbench::mcp_servers::SERVICE_CALL, RELAYED_MCP_SERVERS),
        (nexus_workbench::app_socket::REPLY_CALL, RELAYED_APP_REPLY),
    ] {
        assert_eq!(relayed_refusal(true, call), Some(refusal));
        assert_eq!(relayed_refusal(false, call), None);
    }
    assert_eq!(relayed_refusal(true, "wb_overview"), None);
}

#[tokio::test]
async fn a_relayed_client_cannot_hand_mcp_servers_or_answer_tool_calls() {
    let running = start("apptools-relay");
    let (app, host) = tokio::io::duplex(64 * 1024);
    let (_stop, stopped) = watch::channel(false);
    let server = Arc::clone(&running.server);
    tokio::spawn(async move {
        let ws = tokio_tungstenite::accept_async(host)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        serve_socket(ws, server, stopped, Peer::Relayed).await;
    });
    let (mut ws, _) = tokio_tungstenite::client_async("ws://relayed/", app)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let hello = json!({ "hello": { "token": "", "protocol": PROTOCOL } });
    ws.send(Message::Text(hello.to_string().into()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let calls = [
        (1, nexus_workbench::mcp_servers::SERVICE_CALL, RELAYED_MCP_SERVERS),
        (2, nexus_workbench::app_socket::REPLY_CALL, RELAYED_APP_REPLY),
    ];
    let mut answered = 0;
    while let Some(Ok(message)) = ws.next().await {
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(text.as_str()).unwrap_or_else(|e| panic!("{e}"));
        if value.get("hello").is_some() {
            for (id, call, _) in calls {
                let request = json!({ "id": id, "call": call, "args": {"id": 1, "reply": {}} });
                ws.send(Message::Text(request.to_string().into()))
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
            }
            continue;
        }
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let (_, _, refusal) = calls.iter().find(|(at, _, _)| *at == id).expect("one of ours");
        assert_eq!(value.get("error").and_then(Value::as_str), Some(*refusal), "{value}");
        answered += 1;
        if answered == calls.len() {
            return;
        }
    }
    panic!("not every hand-off was answered");
}
