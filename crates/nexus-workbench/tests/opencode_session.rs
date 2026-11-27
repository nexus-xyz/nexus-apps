//! Opens a local OpenCode session with a stand-in for ACP.
#![cfg(feature = "opencode")]

mod support;

use nexus_workbench::NewThread;
use nexus_workbench::event::{Decision, PermissionMode, SessionAgent, Via, WbEvent};
use support::*;

#[tokio::test]
async fn opencode_runs_a_turn_over_acp_and_waits_for_the_persons_approval() {
    let s = setup("opencode-approval", Default::default());
    let session = s
        .workbench
        .create_thread(NewThread {
            agent: SessionAgent::Opencode,
            via: Via::Subscription,
            provider: None,
            permission_mode: PermissionMode::Default,
            orchestration: false,
            ..new_root(&s.project, "run ls")
        })
        .await
        .expect("OpenCode session");
    let ask = approval(&s.workbench, &session.id).await;
    s.workbench
        .approve(&session.id, &ask, Decision::Allow, None, None, None)
        .expect("the person answers OpenCode's ask");
    let events = wait_for(&s.workbench, &session.id, |events| turn_ends(events).len() == 1).await;
    assert!(events.iter().any(|stored| matches!(
        &stored.event, WbEvent::Text { text, .. } if text == "You said: run ls"
    )));
    let args = std::fs::read_to_string(s.host.memory.join("args")).expect("the stand-in reports its arguments");
    assert_eq!(args.trim(), r#"["acp"]"#);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn opencode_refuses_a_mode_it_cannot_enforce() {
    let s = setup("opencode-fixed-permissions", Default::default());
    let error = s
        .workbench
        .create_thread(NewThread {
            agent: SessionAgent::Opencode,
            via: Via::Subscription,
            provider: None,
            permission_mode: PermissionMode::Plan,
            ..new_root(&s.project, "inspect")
        })
        .await
        .expect_err("OpenCode must refuse a Nexus permission preset");
    assert_eq!(error.to_string(), "OpenCode does not offer this permission mode.");
    assert!(!s.host.memory.join("args").exists());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Each start's `OPENCODE_DISABLE_PROJECT_CONFIG`, as the stand-in recorded it (empty: unset).
fn project_config_off(s: &Setup) -> Vec<String> {
    std::fs::read_to_string(s.host.memory.join("project_config_off"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[tokio::test]
async fn opencode_loads_the_projects_own_config_only_after_the_person_trusts_the_project() {
    let s = setup("opencode-trust", Default::default());
    let repo = s.root.join("repo");
    std::fs::write(
        repo.join("opencode.json"),
        r#"{"mcp": {"evil": {"type": "local", "command": ["./evil.sh"]}}}"#,
    )
    .expect("written");
    std::fs::create_dir_all(repo.join(".opencode/plugin")).expect("a folder");
    std::fs::write(
        repo.join(".opencode/plugin/evil.ts"),
        "export const Evil = async () => ({})",
    )
    .expect("written");

    // Never asked: OpenCode's question waits, and the session starts with the project's config off.
    let asked = s.workbench.claude_trust(&s.project).expect("read");
    assert!(asked.asks_for(SessionAgent::Opencode));
    assert_eq!(asked.opencode.summary(), "1 MCP server, 1 plugin or custom tool");
    let session = s
        .workbench
        .create_thread(NewThread {
            agent: SessionAgent::Opencode,
            via: Via::Subscription,
            provider: None,
            permission_mode: PermissionMode::Default,
            orchestration: false,
            ..new_root(&s.project, "hello")
        })
        .await
        .expect("OpenCode session");
    let events = wait_for(&s.workbench, &session.id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(project_config_off(&s), ["1"]);
    assert!(
        events.iter().any(|stored| matches!(
            &stored.event,
            WbEvent::Notice { text } if text.starts_with("This project's own OpenCode config is off")
                && text.contains("1 MCP server, 1 plugin or custom tool")
        )),
        "{events:?}"
    );

    // Trusted: the next start loads the project's config, as before.
    let trusted = s.workbench.set_claude_trust(&s.project, true).expect("saved");
    assert!(!trusted.asks_for(SessionAgent::Opencode));
    s.workbench.stop(&session.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &session.id).await;
    s.workbench
        .send(&session.id, "again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &session.id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(project_config_off(&s), ["1", ""]);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
