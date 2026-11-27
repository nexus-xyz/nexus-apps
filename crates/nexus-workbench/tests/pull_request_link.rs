//! The pull requests the agent opened are saved on its session: the watch found the branch's pull
//! request, and the session keeps its link as if the app's form had opened it. One session may open several, from more
//! than one branch: each is saved once, the latest last. The watch's guess from the branch saves none for a session in
//! its project's folder; the one its agent's `gh pr create` printed is saved there too.

#![cfg(unix)]

mod support;

use nexus_workbench::NewWorktree;
use support::*;

const FIRST: &str = "https://github.com/example/repo/pull/42";
const SECOND: &str = "https://github.com/example/repo/pull/43";

fn pulls(summary: &nexus_workbench::event::ThreadSummary) -> (Vec<String>, Option<String>) {
    let worktree = summary.worktree.as_ref();
    (
        worktree
            .map(|w| w.pulls().into_iter().map(String::from).collect())
            .unwrap_or_default(),
        worktree.and_then(|w| w.pull_request.clone()),
    )
}

#[tokio::test]
async fn a_session_saves_each_pull_request_it_opened_once() {
    let s = setup("pull-request-link", bounded());
    let own = root(
        &s,
        nexus_workbench::NewThread {
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "In a worktree")
        },
    )
    .await;
    let shared = root(&s, new_root(&s.project, "In the folder")).await;

    let linked = s
        .workbench
        .link_pull_request(&own.id, FIRST)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a worktree saves its first pull request"));
    assert_eq!(pulls(&linked), (vec![FIRST.to_string()], Some(FIRST.to_string())));
    assert_eq!(linked.updated_at, own.updated_at, "saving a link is not activity");

    let again = s
        .workbench
        .link_pull_request(&own.id, FIRST)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(again.is_none(), "a saved pull request is saved once");

    let second = s
        .workbench
        .link_pull_request(&own.id, SECOND)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a second pull request, from another branch, is saved too"));
    assert_eq!(
        pulls(&second),
        (vec![FIRST.to_string(), SECOND.to_string()], Some(SECOND.to_string())),
        "both kept, oldest first; the latest is the session's pull request"
    );
    assert_eq!(
        pulls(&thread(&s.workbench, &own.id)),
        pulls(&second),
        "saved in the store"
    );

    let folder = s
        .workbench
        .link_pull_request(&shared.id, FIRST)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        folder.is_none(),
        "a session in its project's folder has no branch of its own"
    );
    assert!(thread(&s.workbench, &shared.id).worktree.is_none());

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The pull request a session's agent opened from its shell is saved wherever the session works: its `gh pr create`
/// printed it, so no branch is guessed. A session in its project's folder keeps it on the session.
#[tokio::test]
async fn a_pull_request_the_agent_opened_is_saved_in_a_worktree_and_in_the_folder() {
    let s = setup("pull-request-created", bounded());
    let own = root(
        &s,
        nexus_workbench::NewThread {
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "In a worktree")
        },
    )
    .await;
    let shared = root(&s, new_root(&s.project, "In the folder")).await;

    let linked = s
        .workbench
        .link_created_pull_request(&shared.id, FIRST)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a session in its project's folder saves the one its agent opened"));
    assert_eq!(linked.pulls(), vec![FIRST]);
    assert_eq!(linked.latest_pull(), Some(FIRST));
    assert_eq!(linked.updated_at, shared.updated_at, "saving a link is not activity");
    let stored = thread(&s.workbench, &shared.id);
    assert!(stored.worktree.is_none());
    assert_eq!(stored.pulls(), vec![FIRST], "saved in the store");
    assert!(
        s.workbench
            .link_created_pull_request(&shared.id, FIRST)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_none(),
        "saved once"
    );
    s.workbench
        .link_created_pull_request(&shared.id, SECOND)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        thread(&s.workbench, &shared.id).latest_pull(),
        Some(SECOND),
        "the latest last"
    );

    s.workbench
        .link_created_pull_request(&own.id, FIRST)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("a worktree saves it too"));
    assert_eq!(
        pulls(&thread(&s.workbench, &own.id)),
        (vec![FIRST.to_string()], Some(FIRST.to_string()))
    );

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// End to end: the agent's `gh pr create` runs in a session in its project's folder, and the session saves the URL the
/// command printed, during its turn, and tells the windows (`wb:thread`).
#[tokio::test]
async fn a_session_saves_the_pull_request_its_agents_shell_opened() {
    let s = setup("pull-request-shell", bounded());
    let session = root(&s, new_root(&s.project, "openpr")).await;
    let url = "https://github.com/example/repo/pull/77";
    until("the session saves the pull request", || {
        thread(&s.workbench, &session.id).pulls() == vec![url]
    })
    .await;
    assert!(thread(&s.workbench, &session.id).worktree.is_none());
    let told = s
        .host
        .summaries
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|summary| summary.id == session.id && summary.latest_pull() == Some(url));
    assert!(told, "the windows hear of it");

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[test]
fn an_older_sessions_one_link_joins_the_list_first() {
    let mut worktree = nexus_workbench::event::Worktree {
        path: "/w".into(),
        branch: "b".into(),
        base: "origin/main".into(),
        removed: false,
        pull_request: Some(FIRST.into()),
        pull_requests: Vec::new(),
        review: None,
    };
    assert_eq!(worktree.pulls(), vec![FIRST]);
    worktree.add_pull(SECOND);
    assert_eq!(worktree.pulls(), vec![FIRST, SECOND]);
    assert_eq!(worktree.pull_request.as_deref(), Some(SECOND));
    worktree.add_pull(FIRST);
    assert_eq!(worktree.pulls(), vec![FIRST, SECOND], "the list keeps its order");
    assert_eq!(
        worktree.pull_request.as_deref(),
        Some(FIRST),
        "the latest opened again is the session's"
    );
}
