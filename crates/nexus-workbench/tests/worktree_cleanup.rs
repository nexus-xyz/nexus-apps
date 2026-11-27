//! Cleaning up sessions' worktrees by rule, on a scratch repository with a remote: each rule removes what it
//! should, a worktree with uncommitted changes or unpushed commits stays and says why, a running agent's stays, and a
//! session in its project's own folder is never touched. GitHub is a fake: the merged rule asks a closure.

#![cfg(unix)]

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nexus_workbench::cleanup::{MergedHead, Report, Rule, Rules};
use nexus_workbench::event::ThreadSummary;
use nexus_workbench::{NewWorktree, ThreadPatch};
use support::*;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A session with its own worktree, its first turn over and its agent stopped.
async fn session(s: &Setup, text: &str, worktree: bool) -> ThreadSummary {
    let new = nexus_workbench::NewThread {
        name: None,
        worktree: worktree.then(NewWorktree::default),
        ..new_root(&s.project, text)
    };
    let created = root(s, new).await;
    s.workbench.stop(&created.id).unwrap_or_else(|e| panic!("{e}"));
    until_stopped(&s.workbench, &created.id).await;
    created
}

fn dir(thread: &ThreadSummary) -> PathBuf {
    PathBuf::from(&thread.worktree.as_ref().map(|w| w.path.clone()).unwrap_or_default())
}

fn archive(s: &Setup, thread: &ThreadSummary) {
    let patch = ThreadPatch {
        archived: Some(true),
        ..ThreadPatch::default()
    };
    s.workbench.update(&thread.id, patch).unwrap_or_else(|e| panic!("{e}"));
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), "x\n").unwrap_or_else(|e| panic!("{e}"));
    git(dir, &["add", file]);
    git(dir, &["commit", "--quiet", "-m", file]);
}

fn head(dir: &Path) -> String {
    let out = nexus_workbench::process::command("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// GitHub as the test says it is: these sessions' pull requests merged, at these heads.
fn merged(heads: HashMap<String, String>) -> MergedHead {
    Arc::new(move |thread: &ThreadSummary, _: &Path| heads.get(&thread.id).cloned())
}

fn kept<'a>(report: &'a Report, thread: &ThreadSummary) -> Option<&'a str> {
    report
        .kept
        .iter()
        .find(|entry| entry.thread_id == thread.id)
        .and_then(|entry| entry.kept.as_deref())
}

fn removed(report: &Report, thread: &ThreadSummary) -> Option<Rule> {
    report
        .removed
        .iter()
        .find(|entry| entry.thread_id == thread.id)
        .map(|entry| entry.rule)
}

fn is_removed(s: &Setup, thread: &ThreadSummary) -> bool {
    support::thread(&s.workbench, &thread.id)
        .worktree
        .is_some_and(|worktree| worktree.removed)
}

#[tokio::test]
async fn each_rule_removes_only_worktrees_that_lose_nothing() {
    let s = setup("worktree-cleanup", bounded());
    let repo = s.root.join("repo");
    // A remote holding main: a new worktree's commit is pushed work.
    let remote = s.root.join("remote.git");
    git(&s.root, &["init", "--quiet", "--bare", &remote.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(&repo, &["push", "--quiet", "origin", "main"]);

    let clean = session(&s, "Archived and clean", true).await;
    let dirty = session(&s, "Archived with changes", true).await;
    let unpushed = session(&s, "Archived with commits", true).await;
    let folder = session(&s, "Archived in the folder", false).await;
    let running = root(
        &s,
        nexus_workbench::NewThread {
            name: None,
            worktree: Some(NewWorktree::default()),
            ..new_root(&s.project, "Archived and running")
        },
    )
    .await;
    let recent = session(&s, "Still in use", true).await;
    let squashed = session(&s, "Merged by squash", true).await;
    let moved_on = session(&s, "Merged then more", true).await;

    std::fs::write(dir(&dirty).join("wip.txt"), "wip\n").unwrap_or_else(|e| panic!("{e}"));
    commit(&dir(&unpushed), "local.txt");
    for thread in [&clean, &dirty, &unpushed, &folder, &running] {
        archive(&s, thread);
    }
    // The squashed branch's commit is on no remote: only its merged head counts it as pushed.
    commit(&dir(&squashed), "feature.txt");
    let squashed_head = head(&dir(&squashed));
    commit(&dir(&moved_on), "feature.txt");
    let moved_on_head = head(&dir(&moved_on));
    commit(&dir(&moved_on), "after.txt");

    let archived_at = support::thread(&s.workbench, &clean.id).updated_at;

    // Archived only.
    let rules = Rules {
        archived: true,
        ..Rules::default()
    };
    let report = s
        .workbench
        .clean_worktrees(&rules, merged(HashMap::new()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(removed(&report, &clean), Some(Rule::Archived));
    assert!(!dir(&clean).exists());
    assert!(is_removed(&s, &clean));
    let why = kept(&report, &dirty).unwrap_or_default();
    assert!(why.contains("uncommitted"), "{why}");
    assert!(dir(&dirty).join("wip.txt").exists());
    let why = kept(&report, &unpushed).unwrap_or_default();
    assert!(why.contains("aren't pushed"), "{why}");
    assert!(dir(&unpushed).exists());
    assert_eq!(kept(&report, &running), Some("Its agent is running."));
    assert!(dir(&running).exists());
    // The session in the project's own folder is not a worktree to clean: never listed, never touched.
    assert!(
        report
            .removed
            .iter()
            .chain(&report.kept)
            .all(|e| e.thread_id != folder.id)
    );
    assert!(repo.join(".git").is_dir());
    // Rules that did not match leave the rest alone.
    for thread in [&recent, &squashed, &moved_on] {
        assert!(dir(thread).exists());
        assert!(removed(&report, thread).is_none() && kept(&report, thread).is_none());
    }
    // Removing is not activity: the session keeps its place.
    assert_eq!(support::thread(&s.workbench, &clean.id).updated_at, archived_at);

    // Merged: the squash merge's head goes; a branch that moved on after its merge stays.
    let rules = Rules {
        merged: true,
        ..Rules::default()
    };
    let heads = HashMap::from([
        (squashed.id.clone(), squashed_head),
        (moved_on.id.clone(), moved_on_head),
    ]);
    let report = s
        .workbench
        .clean_worktrees(&rules, merged(heads))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(removed(&report, &squashed), Some(Rule::Merged));
    assert!(!dir(&squashed).exists());
    let why = kept(&report, &moved_on).unwrap_or_default();
    assert!(why.contains("aren't in its merged pull request"), "{why}");
    assert!(dir(&moved_on).exists());
    assert!(dir(&recent).exists());

    // Idle for 7 days: only what loses nothing, eight days on.
    let rules = Rules {
        idle_days: Some(7),
        ..Rules::default()
    };
    let later = nexus_workbench::now_ms() + 8 * DAY_MS;
    let report = s
        .workbench
        .clean_worktrees_as_of(&rules, merged(HashMap::new()), later)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(removed(&report, &recent), Some(Rule::Idle));
    assert!(!dir(&recent).exists());
    assert!(kept(&report, &moved_on).unwrap_or_default().contains("aren't pushed"));
    assert!(kept(&report, &dirty).unwrap_or_default().contains("uncommitted"));
    // Already removed: not listed again.
    assert!(removed(&report, &clean).is_none() && kept(&report, &clean).is_none());
    assert!(repo.join(".git").is_dir());

    // No rule on: nothing happens.
    let report = s
        .workbench
        .clean_worktrees_as_of(&Rules::default(), merged(HashMap::new()), later)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(report.removed.is_empty() && report.kept.is_empty());
    assert!(dir(&dirty).exists());

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A new worktree's branch takes the prefix set, and an illegal one is refused before anything is made.
#[tokio::test]
async fn new_worktree_branches_take_the_prefix() {
    let s = setup("branch-prefix", bounded());
    assert!(s.workbench.set_branch_prefix("bad prefix/").is_err());
    s.workbench.set_branch_prefix("jd/").unwrap_or_else(|e| panic!("{e}"));
    let made = session(&s, "Fix the login", true).await;
    let branch = made.worktree.as_ref().map(|w| w.branch.clone()).unwrap_or_default();
    assert!(branch.starts_with("jd/fix-the-login-"), "{branch}");
    // A host is told the app's prefix with each new session, which wins over its own.
    let new = nexus_workbench::NewThread {
        name: None,
        worktree: Some(NewWorktree {
            branch_prefix: Some("team/".into()),
            ..NewWorktree::default()
        }),
        ..new_root(&s.project, "Add search")
    };
    let made = root(&s, new).await;
    let branch = made.worktree.as_ref().map(|w| w.branch.clone()).unwrap_or_default();
    assert!(branch.starts_with("team/add-search-"), "{branch}");
    let bad = nexus_workbench::NewThread {
        name: None,
        worktree: Some(NewWorktree {
            branch_prefix: Some("a..b/".into()),
            ..NewWorktree::default()
        }),
        ..new_root(&s.project, "Nope")
    };
    let error = s.workbench.create_thread(bad).await.err().unwrap_or_default();
    assert!(error.contains("can't start a branch name"), "{error}");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
