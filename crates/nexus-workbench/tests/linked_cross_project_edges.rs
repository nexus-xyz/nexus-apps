//! Cross-project subsessions at their edges: the other project's default mode only lowers, trust binds every agent,
//! a removed project stops only its own organization's cross-project children, and a deleted lead frees them.

mod support;

use nexus_workbench::event::{PermissionMode, SessionAgent, StoredEvent, ThreadSummary, WbEvent};
use nexus_workbench::project_defaults::ProjectDefaults;
use nexus_workbench::store::{Store, ThreadRecord};
use nexus_workbench::{Halt, Limits, OrchCode};
use support::cross::*;
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

fn halt(s: &Setup, id: &str) -> Option<Halt> {
    s.workbench.halt(id).unwrap_or_else(|e| panic!("{e}"))
}

/// The other project's default mode is a ceiling, never a floor: Full access there leaves an Auto-accept edits lead's
/// child in Auto-accept edits.
#[tokio::test]
async fn the_other_projects_default_mode_never_raises_the_childs() {
    let s = setup("cross-mode-floor", Limits::default());
    let other = trusted(&s);
    s.workbench
        .set_project_defaults(
            &other.id,
            ProjectDefaults {
                mode: Some(PermissionMode::BypassPermissions),
                ..ProjectDefaults::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    assert_eq!(lead.permission_mode, PermissionMode::AcceptEdits);
    let c = finished_child(&s, &lead.id, into(&other.id, "Build it")).await;
    assert_eq!(c.permission_mode, PermissionMode::AcceptEdits);
    done(s).await;
}

/// Trust binds every agent, not only those whose project settings it limits: a Codex child is refused too.
#[tokio::test]
async fn an_untrusted_project_refuses_a_codex_child_too() {
    let s = setup("cross-codex-trust", Limits::default());
    let other = elsewhere(&s);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let (code, message) = refused(&s, &lead.id, into_as(SessionAgent::Codex, &other.id, "Build it")).await;
    assert_eq!(code, OrchCode::ProjectUntrusted, "{message}");
    done(s).await;
}

/// A row of another organization's: a cross-project child at work in `project_id`, under a lead this workbench does
/// not have.
fn foreign_child(project_id: &str) -> ThreadRecord {
    let summary: ThreadSummary = serde_json::from_value(serde_json::json!({
        "id": "t_foreign", "projectId": project_id, "title": "Foreign", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "running", "costUsd": 0.0, "nexusSessions": [],
        "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
        "parentThreadId": "t_foreign_lead", "depth": 1, "crossProject": true,
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    ThreadRecord {
        summary,
        native_id: "n".into(),
        native_started: true,
        scope: "gw|other".into(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    }
}

/// Removing project X stops this organization's cross-project children in X only: X's own sessions' children, the
/// cross-project children in another project, and another organization's rows are left alone.
#[tokio::test]
async fn removing_a_project_stops_only_its_own_organizations_cross_project_children() {
    let s = setup("cross-remove-scope", Limits::default());
    let x = trusted_repo(&s, "xrepo");
    let y = trusted_repo(&s, "yrepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let lead_x = root(&s, new_root(&x.id, "Plan in x")).await;
    let ordinary = s
        .workbench
        .start_child(&lead_x.id, child(SessionAgent::Claude, "slow Build in x"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let cross_x = s
        .workbench
        .start_child(&lead.id, into(&x.id, "slow Build x"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let cross_y = s
        .workbench
        .start_child(&lead.id, into(&y.id, "slow Build y"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    for id in [&ordinary.id, &cross_x.id, &cross_y.id] {
        wait_for(&s.workbench, id, started).await;
    }
    let store = Store::open(&s.root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    store
        .insert_thread(&foreign_child(&x.id))
        .unwrap_or_else(|e| panic!("{e}"));
    drop(store);

    s.workbench.remove_project(&x.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &cross_x.id).await;
    assert_eq!(halt(&s, &cross_x.id), Some(Halt::Person));
    for (id, what) in [
        (&ordinary.id, "a child of a session in x"),
        (&cross_y.id, "a child in y"),
    ] {
        assert!(s.workbench.is_live(id), "{what} still runs");
        assert_eq!(halt(&s, id), None, "{what}");
    }
    assert_eq!(halt(&s, "t_foreign"), None, "another organization's child");
    done(s).await;
}

/// Deleting the lead frees a cross-project child as a root, and a root is no cross-project subsession: it may start
/// subsessions of its own again.
#[tokio::test]
async fn a_freed_cross_project_child_is_no_longer_a_leaf() {
    let s = setup("cross-freed", Limits::default());
    let other = trusted(&s);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, into(&other.id, "Build it")).await;
    assert!(c.cross_project && c.at_depth_limit);
    s.workbench
        .delete_thread(&lead.id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let freed = thread(&s.workbench, &c.id);
    assert_eq!((freed.parent_thread_id.as_deref(), freed.depth), (None, 0));
    assert!(!freed.cross_project && !freed.at_depth_limit);
    let grandchild = finished_child(&s, &c.id, child(SessionAgent::Claude, "Build more")).await;
    assert_eq!(grandchild.project_id, other.id);
    done(s).await;
}
