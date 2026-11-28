//! A cross-project subsession starts subsessions of its own: in its own project (an ordinary child), or in a third
//! project, checked again at that hop. Depth counts from the tree's root across projects, and the halt cascade and
//! results reach across every hop.

mod support;

use nexus_workbench::event::{SessionAgent, StoredEvent, WbEvent};
use nexus_workbench::{Halt, Limits, OrchCode};
use support::cross::*;
use support::*;

fn started(events: &[StoredEvent]) -> bool {
    events.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
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
