//! Which commands and agents get `terminal_run`'s `watch` option.
#![cfg(unix)]

mod support;

use nexus_workbench::event::{PermissionMode, SessionAgent};
use serde_json::json;
use support::watch::*;
use support::*;

/// Claude Code sees `watch` on `terminal_run` and never on `terminal_open`.
#[tokio::test]
async fn tools_list_offers_watch_on_terminal_run_only() {
    let s = setup_terminal("watch-tools-list");
    let _thread = idle_session(&s).await;
    let at = first_endpoint(&s).await;

    let list = post_rpc(&at, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let tools = list["result"]["tools"].as_array().expect("tools list");
    let schema = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["inputSchema"]["properties"]
            .clone()
    };
    assert!(schema("terminal_run").get("watch").is_some());
    assert!(schema("terminal_open").get("watch").is_none());

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Unwatched commands and `terminal_open` shells never send notices; `terminal_open` refuses `watch: true`.
#[tokio::test]
async fn unwatched_exit_and_terminal_open_send_no_notice() {
    let s = setup_terminal("watch-unwatched");
    let thread = idle_session(&s).await;
    let at = first_endpoint(&s).await;

    for (req_id, args) in [
        ("u1", json!({"command": "printf 'unwatched-1'", "request_id": "u1"})),
        (
            "u2",
            json!({"command": "printf 'unwatched-2'", "request_id": "u2", "watch": false}),
        ),
    ] {
        let (started, failed) = mcp_call(&at, 10, "terminal_run", args).await;
        assert!(!failed, "{req_id}: {started}");
        wait_command_ended(&s.workbench, &thread.id, started["id"].as_str().expect("id")).await;
    }

    let (err, failed) = mcp_call(&at, 11, "terminal_open", json!({"request_id": "sh-bad", "watch": true})).await;
    assert!(failed, "terminal_open must refuse watch: {err}");

    let (shell, failed) = mcp_call(&at, 12, "terminal_open", json!({"request_id": "sh-1"})).await;
    assert!(!failed, "{shell}");
    let shell_id = shell["id"].as_str().expect("shell id").to_string();
    let lease = shell["lease"].as_u64().expect("lease");
    let (_, failed) = mcp_call(
        &at,
        13,
        "terminal_input",
        json!({"id": shell_id, "lease": lease, "data": "exit\n"}),
    )
    .await;
    assert!(!failed);
    wait_command_ended(&s.workbench, &thread.id, &shell_id).await;

    // The marker's notice comes alone: none of the commands above sent one.
    let notices = marker(&s.workbench, &thread.id, "unwatched-marker").await;
    assert_eq!(notices.len(), 1, "{notices:?}");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Agents without `steers_mid_turn` (Nexus and ACP agents) do not see `watch` in `tools/list` and are refused when
/// passing `watch: true`, with a message naming the supported agents.
#[tokio::test]
async fn unsupported_agents_omit_watch_and_refuse_watch_calls() {
    let s = setup_terminal("watch-unsupported");
    let _thread = session_of(&s, SessionAgent::Nexus).await;
    let at = first_endpoint(&s).await;

    let list = post_rpc(&at, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
    let tools = list["result"]["tools"].as_array().expect("tools list");
    let run_tool = tools
        .iter()
        .find(|t| t["name"] == "terminal_run")
        .expect("Nexus has terminal_run");
    assert!(
        run_tool["inputSchema"]["properties"].get("watch").is_none(),
        "Nexus must not get watch: {run_tool}"
    );

    let (refused, failed) = mcp_call(
        &at,
        2,
        "terminal_run",
        json!({"command": "printf 'nope'", "request_id": "nx-1", "watch": true}),
    )
    .await;
    assert!(failed, "watch must be refused for Nexus: {refused}");
    assert_eq!(refused["code"], "terminal_watch_unsupported");
    let message = refused["message"].as_str().expect("error message");
    assert!(message.contains("Claude Code and Codex"), "{message}");
    assert!(message.contains("terminal_read"), "{message}");

    // Unwatched `terminal_run` still works for Nexus.
    let (ok, failed) = mcp_call(
        &at,
        3,
        "terminal_run",
        json!({"command": "printf 'ok'", "request_id": "nx-2"}),
    )
    .await;
    assert!(!failed, "{ok}");

    // An ACP agent thread also refuses `watch: true`.
    let mut agy = new_root(&s.project, "hello");
    agy.agent = SessionAgent::Antigravity;
    agy.orchestration = false;
    agy.permission_mode = PermissionMode::BypassPermissions;
    let agy_thread = s.workbench.create_thread(agy).await.expect("create agy");
    let err = s
        .workbench
        .terminal(
            &agy_thread.id,
            "run",
            json!({"command": "printf 'nope'", "request_id": "agy-1", "watch": true}),
            false,
        )
        .await
        .expect_err("Antigravity must refuse watch");
    assert!(err.starts_with("terminal_watch_unsupported:"), "{err}");
    assert!(err.contains("Claude Code and Codex"), "{err}");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
