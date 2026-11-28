//! A cross-project subsession starts subsessions of its own: in its own project (an ordinary child), or in a third
//! project, checked again at that hop. Depth counts from the tree's root across projects, and the halt cascade and
//! results reach across every hop.

mod support;

use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
use nexus_workbench::{Halt, Limits, OrchCode, StartChild, Subsessions};
use support::cross::*;
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
}

/// "Enable cross-project subsessions" on or off, the other settings as they are by default.
fn cross_project(s: &Setup, on: bool) {
    s.workbench.set_subsessions(Subsessions {
        cross_project_on: on,
        ..Subsessions::default()
    });
}

/// Both ways of starting in `project`: with no project named (from a parent there), and with it named.
fn in_own(project: &str) -> [StartChild; 2] {
    [
        child(SessionAgent::Claude, "Build here"),
        into(project, "Build here too"),
    ]
}

/// A child in project B, outside the root's project, starting in B passes B's checks again: once B is untrusted,
/// archived or the setting is off, it is refused, with B named or not. The root's own project needs none of them.
#[tokio::test]
async fn a_start_in_the_parents_project_outside_the_roots_is_checked_again() {
    let s = setup("cross-own-checked", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;

    s.workbench
        .set_claude_trust(&b.id, false)
        .unwrap_or_else(|e| panic!("{e}"));
    for start in in_own(&b.id) {
        let (code, message) = refused(&s, &in_b.id, start).await;
        assert_eq!(code, OrchCode::ProjectUntrusted, "{message}");
    }
    s.workbench
        .set_claude_trust(&b.id, true)
        .unwrap_or_else(|e| panic!("{e}"));

    s.workbench
        .update_project(&b.id, None, Some(true))
        .unwrap_or_else(|e| panic!("{e}"));
    for start in in_own(&b.id) {
        let (code, message) = refused(&s, &in_b.id, start).await;
        assert_eq!(code, OrchCode::ProjectNotOpen, "{message}");
    }
    s.workbench
        .update_project(&b.id, None, Some(false))
        .unwrap_or_else(|e| panic!("{e}"));

    cross_project(&s, false);
    for start in in_own(&b.id) {
        let (code, message) = refused(&s, &in_b.id, start).await;
        assert_eq!(code, OrchCode::CrossProjectOff, "{message}");
    }
    // The root's own project is no hop: its children start with the setting off and the project never trusted.
    let home = finished_child(&s, &lead.id, child(SessionAgent::Claude, "Build at home")).await;
    assert_eq!(
        (home.project_id.as_str(), home.cross_project),
        (s.project.as_str(), false)
    );

    cross_project(&s, true);
    let own = finished_child(&s, &in_b.id, child(SessionAgent::Claude, "Build in b")).await;
    assert_eq!((own.project_id.as_str(), own.cross_project), (b.id.as_str(), false));
    done(s).await;
}

/// A child in B that starts in the root's project A is a cross-project start like any other: A must be trusted.
#[tokio::test]
async fn a_start_back_in_the_roots_project_is_trust_checked() {
    let s = setup("cross-back-home", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    let (code, message) = refused(&s, &in_b.id, into(&s.project, "Build at home")).await;
    assert_eq!(code, OrchCode::ProjectUntrusted, "{message}");
    s.workbench
        .set_claude_trust(&s.project, true)
        .unwrap_or_else(|e| panic!("{e}"));
    let back = finished_child(&s, &in_b.id, into(&s.project, "Build at home")).await;
    assert_eq!(
        (back.project_id.as_str(), back.cross_project),
        (s.project.as_str(), true)
    );
    done(s).await;
}

/// The person's Settings reach the engine through `set_subsessions`, as the app hands them on: turned off, a first
/// cross-project start is refused; turned on again, it starts.
#[tokio::test]
async fn set_subsessions_turns_cross_project_starts_off_and_on() {
    let s = setup("cross-set-subsessions", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    cross_project(&s, false);
    let (code, message) = refused(&s, &lead.id, into(&b.id, "Build in b")).await;
    assert_eq!(code, OrchCode::CrossProjectOff, "{message}");
    cross_project(&s, true);
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Build in b")).await;
    assert!(in_b.cross_project);
    done(s).await;
}

/// A second hop is refused when the setting is off, and when the third project is archived.
#[tokio::test]
async fn a_second_hop_is_refused_off_or_archived() {
    let s = setup("cross-second-hop", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    cross_project(&s, false);
    let (code, message) = refused(&s, &in_b.id, into(&c.id, "Build in c")).await;
    assert_eq!(code, OrchCode::CrossProjectOff, "{message}");
    cross_project(&s, true);
    s.workbench
        .update_project(&c.id, None, Some(true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (code, message) = refused(&s, &in_b.id, into(&c.id, "Build in c")).await;
    assert_eq!(code, OrchCode::ProjectNotOpen, "{message}");
    done(s).await;
}

/// A cross-project grandchild counts toward the tree's starts, whichever project it runs in.
#[tokio::test]
async fn a_cross_project_grandchild_counts_toward_the_trees_starts() {
    let s = setup(
        "cross-starts",
        Limits {
            max_starts: Some(2),
            ..Limits::default()
        },
    );
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    finished_child(&s, &in_b.id, into(&c.id, "Build in c")).await;
    for (parent, start) in [
        (&lead.id, child(SessionAgent::Claude, "Third")),
        (&in_b.id, child(SessionAgent::Claude, "Third in b")),
    ] {
        let (code, message) = refused(&s, parent, start).await;
        assert_eq!(code, OrchCode::StartsLimit, "{message}");
    }
    done(s).await;
}

/// A child in project B starts in B (no project, or B named): an ordinary child of its parent. Named project C, it
/// starts there, checked against C; an untrusted project D is refused.
#[tokio::test]
async fn a_cross_project_child_starts_in_its_own_project_or_a_third_one() {
    let s = setup("cross-hops", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let d = repo(&s, "drepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    assert!(in_b.cross_project && !in_b.at_depth_limit);

    for start in [child(SessionAgent::Claude, "Build in b"), into(&b.id, "Build in b too")] {
        let own = finished_child(&s, &in_b.id, start).await;
        assert_eq!((own.project_id.as_str(), own.cross_project), (b.id.as_str(), false));
        assert_eq!(own.parent_thread_id.as_deref(), Some(in_b.id.as_str()));
    }
    let in_c = finished_child(&s, &in_b.id, into(&c.id, "Build in c")).await;
    assert_eq!(
        (in_c.project_id.as_str(), in_c.cross_project, in_c.depth),
        (c.id.as_str(), true, 2)
    );

    let (code, message) = refused(&s, &in_b.id, into(&d.id, "Build in d")).await;
    assert_eq!(code, OrchCode::ProjectUntrusted, "{message}");
    done(s).await;
}

/// Depth counts from the root across projects: at the limit, a cross-project grandchild is refused a child.
#[tokio::test]
async fn depth_counts_across_projects() {
    let s = setup(
        "cross-depth",
        Limits {
            max_depth: Some(2),
            ..Limits::default()
        },
    );
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    let in_c = finished_child(&s, &in_b.id, into(&c.id, "Build in c")).await;
    assert_eq!((in_b.depth, in_c.depth), (1, 2));
    assert!(in_c.at_depth_limit);
    for start in [child(SessionAgent::Claude, "Deeper"), into(&b.id, "Deeper in b")] {
        let (code, message) = refused(&s, &in_c.id, start).await;
        assert_eq!(code, OrchCode::DepthLimit, "{message}");
    }
    done(s).await;
}

/// The person's Stop on the root reaches a grandchild two projects away.
#[tokio::test]
async fn a_stop_on_the_root_reaches_a_grandchild_two_projects_away() {
    let s = setup("cross-halt-hops", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    let in_c = s
        .workbench
        .start_child(&in_b.id, into(&c.id, "slow Build in c"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &in_c.id, started).await;
    s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &in_c.id).await;
    assert_eq!(
        s.workbench.halt(&in_c.id).unwrap_or_else(|e| panic!("{e}")),
        Some(Halt::Parent)
    );
    done(s).await;
}

/// A grandchild's result in a third project reaches its parent in the second.
#[tokio::test]
async fn a_grandchilds_result_reaches_its_parent() {
    let s = setup("cross-result-hops", Limits::default());
    let b = trusted_repo(&s, "brepo");
    let c = trusted_repo(&s, "crepo");
    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    let in_b = finished_child(&s, &lead.id, into(&b.id, "Lead in b")).await;
    let in_c = finished_child(&s, &in_b.id, into(&c.id, "Build in c")).await;
    // The idle parent takes the result in a turn of its own.
    wait_for(&s.workbench, &in_b.id, |e| {
        users(e).iter().any(|(text, _)| text.contains(&in_c.id))
    })
    .await;
    done(s).await;
}
