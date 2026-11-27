//! The Grok 1.0.92 ACP handshake, exercised through the same Workbench used by Desktop.
mod support;

use nexus_workbench::NewThread;
use nexus_workbench::event::{Decision, PermissionMode, SessionAgent, TurnStatus, Via, WbEvent};
use support::*;

fn grok(s: &Setup, mode: PermissionMode, text: &str) -> NewThread {
    NewThread {
        agent: SessionAgent::Grok,
        via: Via::Subscription,
        provider: None,
        permission_mode: mode,
        orchestration: false,
        ..new_root(&s.project, text)
    }
}

fn prompts(s: &Setup) -> Vec<String> {
    let log = match std::fs::read_to_string(s.host.memory.join("prompts")) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("{error}"),
    };
    log.lines()
        .map(|line| {
            let blocks: Vec<serde_json::Value> = serde_json::from_str(line).expect("recorded ACP prompt blocks");
            blocks
                .last()
                .and_then(|block| block["text"].as_str())
                .and_then(|text| text.lines().next())
                .expect("the recorded ACP prompt ends with the user message")
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn grok_forwards_approvals_cancels_and_loads_local_history() {
    let s = setup("grok-session", Default::default());
    s.host
        .extra
        .lock()
        .expect("fake process environment")
        .insert("FAKE_GROK_CACHED_LOGIN".into(), "1".into());
    s.host
        .extra
        .lock()
        .expect("fake process environment")
        .insert("XAI_API_KEY".into(), "fake".into());
    std::fs::write(
        s.host.memory.join("config.toml"),
        "[mcp_servers.example.env]\napi_key='fixture'\nmodel='helper'",
    )
    .expect("write the test Grok configuration");
    let thread = s
        .workbench
        .create_thread(grok(&s, PermissionMode::Default, "run ls"))
        .await
        .expect("create the Grok thread");
    let id = &thread.id;
    let ask = approval(&s.workbench, id).await;
    assert_eq!(
        std::fs::read_to_string(s.host.memory.join("modes"))
            .expect("read the fake Grok modes log")
            .trim(),
        "ask"
    );
    s.workbench
        .approve(id, &ask, Decision::Allow, None, None, None)
        .expect("answer the permission request");
    wait_for(&s.workbench, id, |events| turn_ends(events).len() == 1).await;
    assert_eq!(prompts(&s), ["run ls"]);
    assert!(
        std::fs::read_to_string(s.host.memory.join("decisions"))
            .expect("read the fake Grok decisions log")
            .contains("allow-once")
    );
    s.workbench.shutdown().await;
    let reopened = reopen(&s, Default::default());
    reopened.send(id, "run again").await.expect("resume the Grok thread");
    wait_for(&reopened, id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::Approval { .. }))
            .count()
            == 2
    })
    .await;
    assert_eq!(prompts(&s), ["run ls", "run again"]);
    assert_eq!(
        std::fs::read_to_string(s.host.memory.join("loads"))
            .expect("read the fake Grok loads log")
            .trim(),
        "fake-agy-session"
    );
    reopened.interrupt(id).expect("cancel the resumed turn");
    let events = wait_for(&reopened, id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(turn_ends(&events).last(), Some(&TurnStatus::Interrupted));
    assert_eq!(
        std::fs::read_to_string(s.host.memory.join("cancels"))
            .expect("read the fake Grok cancels log")
            .trim(),
        "session/cancel"
    );
    reopened.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn grok_rejects_the_offered_permission_and_interrupts_the_turn() {
    let s = setup("grok-reject", Default::default());
    let thread = s
        .workbench
        .create_thread(grok(&s, PermissionMode::Plan, "run ls"))
        .await
        .expect("create the Grok thread");
    let ask = approval(&s.workbench, &thread.id).await;
    s.workbench
        .approve(&thread.id, &ask, Decision::Deny, None, None, None)
        .expect("answer the permission request");
    let events = wait_for(&s.workbench, &thread.id, |events| !turn_ends(events).is_empty()).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Interrupted]);
    let decision: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(s.host.memory.join("decisions")).expect("read the fake Grok decisions log"),
    )
    .expect("parse the recorded permission response");
    assert_eq!(
        decision,
        serde_json::json!({"jsonrpc":"2.0","id":0,"result":{"outcome":{"outcome":"selected","optionId":"reject-once"}}})
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn grok_refuses_competing_auth_signed_out_and_rejected_plan_before_prompt() {
    for (setting, expected) in [
        ("GROK_OIDC_ISSUER", "personal Grok sign-in"),
        ("GROK_AUTH_PATH", "personal Grok sign-in"),
        ("oidc-config", "credential"),
        ("FAKE_GROK_KEY", "subscription-only"),
        ("FAKE_AGY_SIGNED_OUT", "grok login"),
        ("FAKE_CURSOR_REFUSE_PLAN", "Plan refused"),
    ] {
        let s = setup(setting, Default::default());
        if setting == "oidc-config" {
            std::fs::write(
                s.host.memory.join("config.toml"),
                "[auth.oidc]\nissuer='fixture'\nclient_id='test'",
            )
            .expect("write the test Grok configuration");
        } else {
            s.host
                .extra
                .lock()
                .expect("fake process environment")
                .insert(setting.into(), "1".into());
        }
        let thread = s
            .workbench
            .create_thread(grok(&s, PermissionMode::Plan, "pending"))
            .await
            .expect("create the Grok thread");
        wait_for(&s.workbench, &thread.id, |events| {
            events
                .iter()
                .any(|row| matches!(&row.event, WbEvent::Error { text, .. } if text.contains(expected)))
        })
        .await;
        assert!(prompts(&s).is_empty());
        s.workbench.shutdown().await;
        let _ = std::fs::remove_dir_all(&s.root);
    }
}
