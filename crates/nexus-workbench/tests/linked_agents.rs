//! Subsessions of another agent through the public workbench, with the stand-ins: an agent Gateway has no route for
//! runs on the person's own sign-in, a signed-out agent's failure reaches the parent as its result, the person's
//! "Enable cross-agent subsessions" switch, turned off, refuses another agent on both routes, and a mode the agent
//! does not offer is refused rather than run looser. Each test replays one scenario.

mod support;

use std::time::Duration;

use nexus_workbench::event::{SessionAgent, Via};
use nexus_workbench::{Limits, NewThread, OrchCode, ResultStatus, Subsessions, Wait, WaitMode};
use support::*;

/// The parent's wait for `child`, answered once it ended.
async fn result_of(s: &Setup, parent: &str, child: &str) -> nexus_workbench::SessionResult {
    let answer = s
        .workbench
        .wait_children(
            parent,
            Wait {
                session_ids: Some(vec![child.to_string()]),
                timeout: Duration::from_secs(60),
                mode: WaitMode::All,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    answer
        .results
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no result for {child}"))
}

/// A Claude Code lead on Gateway starts an Antigravity subsession: it runs on the person's Google sign-in, as Gateway
/// has no route for Antigravity, and its answer reaches the lead.
#[tokio::test]
async fn an_antigravity_subsession_of_a_gateway_lead_runs_on_the_persons_own_sign_in() {
    let s = setup("agents-antigravity", Limits::default());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    assert_eq!(lead.via, Via::Nexus);
    let done = finished_child(&s, &lead.id, child(SessionAgent::Antigravity, "Review the changelog")).await;
    assert_eq!(
        (done.agent, done.via, done.account),
        (SessionAgent::Antigravity, Via::Subscription, None)
    );
    let launch = launches_of(&s, &done.id).pop().unwrap_or_else(|| panic!("a launch"));
    assert_eq!(
        (launch.agent, launch.via),
        (SessionAgent::Antigravity, Via::Subscription)
    );
    let result = result_of(&s, &lead.id, &done.id).await;
    assert_eq!(result.status, ResultStatus::Completed);
    assert!(
        result.last_message.contains("Review the changelog"),
        "{}",
        result.last_message
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Nexus does not read a harness's sign-in before a subsession starts: the start goes ahead, and a signed-out agent's
/// own failure is the subsession's result, naming the agent and where to sign in.
#[tokio::test]
async fn a_signed_out_agents_failure_reaches_the_parent_as_its_result() {
    let s = setup("agents-signed-out", Limits::default());
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_AGY_SIGNED_OUT".into(), "1".into());
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let done = finished_child(&s, &lead.id, child(SessionAgent::Antigravity, "Review the changelog")).await;
    let result = result_of(&s, &lead.id, &done.id).await;
    assert_eq!(result.status, ResultStatus::Failed);
    assert!(
        result.last_message.contains("Antigravity is not signed in")
            && result
                .last_message
                .contains("Run `agy` in a terminal and sign in with Google"),
        "{}",
        result.last_message
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// With "Enable cross-agent subsessions" off, a lead starts only its own agent's subsessions, on Gateway and on the
/// person's plan; the refusal names the setting. Off again, another agent starts.
#[tokio::test]
async fn disabling_cross_agent_subsessions_refuses_another_agent_on_every_route() {
    let s = setup("agents-cross-off", Limits::default());
    s.workbench.set_subsessions(Subsessions {
        cross_agent_off: true,
        ..Subsessions::default()
    });
    for via in [Via::Nexus, Via::Subscription] {
        let lead = root(
            &s,
            NewThread {
                via,
                ..new_root(&s.project, "Plan the release")
            },
        )
        .await;
        for other in [SessionAgent::Codex, SessionAgent::Antigravity, SessionAgent::Nexus] {
            let refused = s
                .workbench
                .start_child(&lead.id, child(other, "Review it"))
                .await
                .err()
                .unwrap_or_else(|| panic!("{via:?}: started {other:?}"));
            assert_eq!(refused.code, OrchCode::AgentNotAllowed, "{via:?} {other:?}");
            assert!(
                refused
                    .message
                    .contains("turned off \"Enable cross-agent subsessions\"")
                    && refused.message.contains("only Claude Code subsessions")
                    && refused.message.contains("Settings › Code › Subsessions"),
                "{}",
                refused.message
            );
        }
        let own = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
        assert_eq!((own.agent, own.via), (SessionAgent::Claude, via));
    }
    s.workbench.set_subsessions(Subsessions::default());
    let lead = root(&s, new_root(&s.project, "Plan again")).await;
    let codex = finished_child(&s, &lead.id, child(SessionAgent::Codex, "Review it")).await;
    assert_eq!(codex.agent, SessionAgent::Codex);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Cursor offers only Auto-accept edits and Plan only. Under an Ask first lead, Ask first is refused, with the modes
/// Cursor offers there, rather than run in Cursor's own default; Plan only starts.
#[cfg(feature = "cursor")]
#[tokio::test]
async fn a_mode_the_agent_does_not_offer_is_refused_never_run_looser() {
    use nexus_workbench::event::PermissionMode;
    let s = setup("agents-mode", Limits::default());
    let lead = root(
        &s,
        NewThread {
            permission_mode: PermissionMode::Default,
            ..new_root(&s.project, "Plan the release")
        },
    )
    .await;
    let refused = s
        .workbench
        .start_child(&lead.id, child(SessionAgent::Cursor, "Review it"))
        .await
        .err()
        .unwrap_or_else(|| panic!("a Cursor subsession ran in Ask first"));
    assert_eq!(refused.code, OrchCode::BadInput);
    assert_eq!(
        refused.message,
        "Cursor can't run in Ask first. The modes it offers here: permission_mode plan."
    );
    let started = s
        .workbench
        .start_child(
            &lead.id,
            nexus_workbench::StartChild {
                permission_mode: Some(PermissionMode::Plan),
                ..child(SessionAgent::Cursor, "Review it")
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (started.permission_mode, started.via),
        (PermissionMode::Plan, Via::Subscription)
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A lead lowered to Ask first takes its Cursor subsession to Plan only, the highest mode Cursor offers at or below:
/// never Cursor's own default.
#[cfg(feature = "cursor")]
#[tokio::test]
async fn lowering_a_lead_takes_a_subsession_to_the_highest_mode_its_agent_offers_below() {
    use nexus_workbench::event::PermissionMode;
    let s = setup("agents-lower-cursor", Limits::default());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    assert_eq!(lead.permission_mode, PermissionMode::AcceptEdits);
    let done = finished_child(&s, &lead.id, child(SessionAgent::Cursor, "Review it")).await;
    assert_eq!(done.permission_mode, PermissionMode::AcceptEdits);
    s.workbench
        .update(&lead.id, mode(PermissionMode::Default))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the Cursor subsession is in Plan only", || {
        thread(&s.workbench, &done.id).permission_mode == PermissionMode::Plan
    })
    .await;
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// OpenCode offers only Ask first. A lead lowered to Plan only stops its OpenCode subsession, and the subsession's next
/// run is refused with the reason: it never runs above its lead.
#[cfg(feature = "opencode")]
#[tokio::test]
async fn a_subsession_whose_agent_has_no_mode_that_low_stops_and_does_not_run_again() {
    use nexus_workbench::event::{PermissionMode, WbEvent};
    let s = setup("agents-lower-opencode", Limits::default());
    let lead = root(
        &s,
        NewThread {
            permission_mode: PermissionMode::Default,
            ..new_root(&s.project, "Plan the release")
        },
    )
    .await;
    let done = finished_child(&s, &lead.id, child(SessionAgent::Opencode, "Review it")).await;
    assert_eq!(done.permission_mode, PermissionMode::Default);
    s.workbench
        .update(&lead.id, mode(PermissionMode::Plan))
        .unwrap_or_else(|e| panic!("{e}"));
    until("the OpenCode subsession is halted", || {
        thread(&s.workbench, &done.id).halt.is_some()
    })
    .await;
    assert_eq!(thread(&s.workbench, &done.id).permission_mode, PermissionMode::Default);
    let reason = "OpenCode has no permission mode at or below Plan only, so it can't run under this session.";
    let said = |events: &[nexus_workbench::event::StoredEvent]| {
        events.iter().any(|e| match &e.event {
            WbEvent::Error { text, .. } => text.contains(reason),
            WbEvent::TurnEnd { error, .. } => error.as_deref().is_some_and(|error| error.contains(reason)),
            _ => false,
        })
    };
    match s.workbench.send(&done.id, "Go on").await {
        Err(error) => assert!(error.contains(reason), "{error}"),
        Ok(_) => {
            wait_for(&s.workbench, &done.id, said).await;
        }
    }
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
