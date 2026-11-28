//! Cross-project subsessions through the public workbench, with the stand-in `claude`: a lead in one project starts a
//! linked subsession in another project open here (`StartChild::project`). The child keeps its parent, so its result,
//! the halt cascade and the person's message to the lead work as for any child. It can't hand off, and the target
//! project must be open, trusted and allowed. tests/linked_cross_project_hops.rs covers its own subsessions.

mod support;

use std::path::Path;
use std::time::Duration;

use nexus_workbench::event::{PermissionMode, SessionAgent, StoredEvent, ThreadSummary, WbEvent};
use nexus_workbench::project_defaults::ProjectDefaults;
use nexus_workbench::{Config, Halt, HandOff, OrchCode, ThreadPatch, Wait, WaitMode};
use support::cross::*;
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

fn halt(s: &Setup, id: &str) -> Option<Halt> {
    s.workbench.halt(id).unwrap_or_else(|e| panic!("{e}"))
}

/// The child runs in the other project: its project id, a worktree of that repository, that project's setup script,
/// the lead's organization, and the lower of the lead's mode and that project's default mode. Its name finds it too.
#[tokio::test]
async fn a_lead_starts_a_subsession_in_another_trusted_project() {
    let s = setup("cross-admitted", bounded());
    let other = trusted(&s);
    s.workbench
        .set_project_setup(&other.id, Some("echo ran > setup-marker"))
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .set_project_defaults(
            &other.id,
            ProjectDefaults {
                mode: Some(PermissionMode::Default),
                ..ProjectDefaults::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    assert_eq!(lead.permission_mode, PermissionMode::AcceptEdits);

    let c = finished_child(&s, &lead.id, into(&other.path, "Build it")).await;
    assert_eq!(c.project_id, other.id);
    assert_eq!(c.parent_thread_id.as_deref(), Some(lead.id.as_str()));
    assert!(c.cross_project);
    assert_eq!(
        c.permission_mode,
        PermissionMode::Default,
        "the other project's default is lower"
    );
    let worktree = c.worktree.clone().unwrap_or_else(|| panic!("a worktree"));
    let common = nexus_workbench::process::command("git")
        .args([
            "-C",
            &worktree.path,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    let common = String::from_utf8_lossy(&common.stdout).trim().to_string();
    assert_eq!(
        Path::new(&common).canonicalize().ok(),
        Path::new(&other.path).join(".git").canonicalize().ok(),
        "a worktree of the other repository"
    );
    assert!(
        Path::new(&worktree.path).join("setup-marker").exists(),
        "the other project's setup script ran"
    );
    // The lead's organization: the child is listed in the workbench's scope with it.
    assert!(s.workbench.threads().unwrap_or_default().iter().any(|t| t.id == c.id));

    // By the project's name, in any case.
    let by_name = finished_child(&s, &lead.id, into("ELSEWHERE", "Build it again")).await;
    assert_eq!((by_name.project_id, by_name.cross_project), (other.id.clone(), true));
    done(s).await;
}

/// The lead's own project, named: an ordinary child.
#[tokio::test]
async fn the_leads_own_project_named_is_an_ordinary_child() {
    let s = setup("cross-own", nexus_workbench::Limits::default());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, into(&s.project, "Build it")).await;
    assert_eq!(c.project_id, s.project);
    assert!(!c.cross_project && !c.at_depth_limit);
    done(s).await;
}

/// An unknown or archived project, one never trusted or not trusted, and the setting off: each refusal says why.
#[tokio::test]
async fn a_project_not_open_trusted_or_allowed_is_refused() {
    let s = setup("cross-refused", bounded());
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let other = elsewhere(&s);

    let (code, message) = refused(&s, &lead.id, into("/nowhere/at/all", "Build it")).await;
    assert_eq!(code, OrchCode::ProjectNotOpen, "{message}");
    let (code, message) = refused(&s, &lead.id, into("mytest", "Build it")).await;
    assert_eq!(
        (code, message.as_str()),
        (
            OrchCode::ProjectNotOpen,
            "No open project is named mytest or is its repository. Give its folder, GitHub owner/repo or id."
        )
    );

    let (code, message) = refused(&s, &lead.id, into(&other.id, "Build it")).await;
    assert_eq!(code, OrchCode::ProjectUntrusted, "never asked: {message}");
    assert_eq!(
        message,
        "The project elsewhere is not trusted in Nexus, so a subsession can't start there. The person can trust it \
         in Settings › Computer › Projects › elsewhere: turn on \"Trust this project's Claude and OpenCode settings\"."
    );
    s.workbench
        .set_claude_trust(&other.id, false)
        .unwrap_or_else(|e| panic!("{e}"));
    let (code, _) = refused(&s, &lead.id, into(&other.id, "Build it")).await;
    assert_eq!(code, OrchCode::ProjectUntrusted, "Don't trust");

    s.workbench
        .set_claude_trust(&other.id, true)
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench
        .update_project(&other.id, None, Some(true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (code, message) = refused(&s, &lead.id, into(&other.id, "Build it")).await;
    assert_eq!(
        (code, message.as_str()),
        (
            OrchCode::ProjectNotOpen,
            "The project elsewhere is archived in Nexus. Unarchive it in the app first."
        )
    );
    assert!(
        s.workbench
            .list_children(&lead.id)
            .unwrap_or_else(|e| panic!("{e}"))
            .sessions
            .is_empty()
    );
    done(s).await;

    let s = setup_with("cross-off", bounded(), true, |config| Config {
        cross_project_on: false,
        ..config
    });
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let other = trusted(&s);
    // Off, no other project is looked at: an unknown or archived one gets the same refusal, and none names a folder.
    let _archived = s
        .workbench
        .update_project(&repo(&s, "shelved").id, None, Some(true))
        .unwrap_or_else(|e| panic!("{e}"));
    for asked in ["/nowhere/at/all", "shelved", "mytest"] {
        let (code, message) = refused(&s, &lead.id, into(asked, "Build it")).await;
        assert_eq!(code, OrchCode::CrossProjectOff, "{asked}: {message}");
        assert!(!message.contains(&s.root.to_string_lossy().to_string()), "{message}");
    }
    let (code, message) = refused(&s, &lead.id, into(&other.id, "Build it")).await;
    assert_eq!(code, OrchCode::CrossProjectOff, "{message}");
    assert_eq!(
        message,
        "The person turned off \"Enable cross-project subsessions\", so this session starts subsessions only in its \
         own project. The person can turn it on in Settings › Code › Subsessions."
    );
    // Its own project still starts.
    let own = finished_child(&s, &lead.id, into(&s.project, "Build it")).await;
    assert!(!own.cross_project);
    // The person's Settings turn it back on.
    s.workbench.set_subsessions(nexus_workbench::Subsessions::default());
    let c = finished_child(&s, &lead.id, into(&other.id, "Build it")).await;
    assert!(c.cross_project);
    done(s).await;
}

/// The child can't hand off yet; its orchestration stays on, and the switch can be turned on as for any child.
#[tokio::test]
async fn a_cross_project_subsession_cannot_hand_off() {
    let s = setup("cross-handoff", nexus_workbench::Limits::default());
    let other = trusted(&s);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c = finished_child(&s, &lead.id, into(&other.id, "Build it")).await;
    assert!(c.orchestration && !c.at_depth_limit);
    let handoff = HandOff {
        agent: SessionAgent::Codex,
        prompt: "Take over".into(),
        title: None,
        model: None,
        effort: None,
    };
    let refusal = s.workbench.hand_off(&c.id, handoff).await.err();
    assert_eq!(refusal.map(|e| e.code), Some(OrchCode::CrossProject));
    assert!(!thread(&s.workbench, &c.id).archived);
    let on = ThreadPatch {
        orchestration: Some(true),
        ..ThreadPatch::default()
    };
    s.workbench.update(&c.id, on).unwrap_or_else(|e| panic!("{e}"));
    done(s).await;
}

/// The child's result reaches the lead across projects, through a wait.
#[tokio::test]
async fn the_result_comes_back_across_projects() {
    let s = setup("cross-result", bounded());
    let other = trusted(&s);
    // A lead in a turn: an idle one would read the result in an automatic turn of its own.
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = finished_child(&s, &lead.id, into(&other.id, "Build it")).await;
    assert!(has_result(&s.workbench, &lead.id, &c.id));
    let waited = s
        .workbench
        .wait_children(
            &lead.id,
            Wait {
                session_ids: Some(vec![c.id.clone()]),
                timeout: Duration::from_secs(20),
                mode: WaitMode::All,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let ids: Vec<&str> = waited.results.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(ids, [c.id.as_str()]);
    end_relay(&s, &lead.id).await;
    done(s).await;
}

/// The person's Stop on the lead halts the child in the other project; their next message to the lead frees it.
#[tokio::test]
async fn the_halt_cascade_reaches_across_projects() {
    let s = setup("cross-halt", bounded());
    let other = trusted(&s);
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let c: ThreadSummary = s
        .workbench
        .start_child(&lead.id, into(&other.id, "slow Build it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, started).await;
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &c.id).await;
    assert_eq!(
        (halt(&s, &lead.id), halt(&s, &c.id)),
        (Some(Halt::Person), Some(Halt::Parent))
    );
    s.workbench
        .send(&lead.id, "Go on")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    until("the lead's message frees the child", || halt(&s, &c.id).is_none()).await;
    done(s).await;
}

/// Removing the other project stops a child at work there first, so its lead gets a stopped result.
#[tokio::test]
async fn removing_the_other_project_stops_its_cross_project_child() {
    let s = setup("cross-remove", bounded());
    let other = trusted(&s);
    let lead = root_at_work(&s, new_root(&s.project, "relay Plan the release")).await;
    let c = s
        .workbench
        .start_child(&lead.id, into(&other.id, "slow Build it"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &c.id, started).await;
    s.workbench.remove_project(&other.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &c.id).await;
    wait_for(&s.workbench, &c.id, |e| !turn_ends(e).is_empty()).await;
    assert!(has_result(&s.workbench, &lead.id, &c.id), "the lead hears it stopped");
    assert!(thread(&s.workbench, &c.id).archived);
    end_relay(&s, &lead.id).await;
    done(s).await;
}
