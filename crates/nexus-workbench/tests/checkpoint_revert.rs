//! Reverts wait for checkpoint refs that Git has not published yet.

#![cfg(unix)]

mod support;

use std::path::{Path, PathBuf};

use nexus_workbench::event::ThreadStatus;
use support::*;

/// Holds Git's ref transaction after the snapshot exists, before its ref does. Drop releases it on assertion failure.
struct CheckpointGate(PathBuf);

impl CheckpointGate {
    fn hold(repo: &Path) -> Self {
        let hook = repo.join(".git/hooks/reference-transaction");
        nexus_workbench::process::write_fake_script(
            &hook,
            r#"#!/bin/sh
[ "$1" = prepared ] || exit 0
while read -r old new ref; do
    case "$ref" in
        refs/nexus/checkpoints/*)
            touch .git/checkpoint-held
            while [ ! -e .git/checkpoint-release ]; do sleep 0.01; done
            ;;
    esac
done
"#,
        );
        Self(repo.join(".git/checkpoint-release"))
    }
}

impl Drop for CheckpointGate {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

async fn delayed_checkpoint(preview: bool) {
    let s = setup(&format!("checkpoint-revert-{preview}"), bounded());
    let repo = s.root.join("repo");
    let lead = root_at_work(&s, new_root(&s.project, "relay Hold the turn")).await;
    let gate = CheckpointGate::hold(&repo);
    end_relay(&s, &lead.id).await;
    let events = s
        .workbench
        .events(&lead.id, 0)
        .unwrap_or_else(|e| panic!("cannot read turn events: {e}"));
    let turn = *turn_end_seqs(&events)
        .first()
        .unwrap_or_else(|| panic!("relay turn never ended"));
    until("Git holds the checkpoint ref", || {
        repo.join(".git/checkpoint-held").exists()
    })
    .await;
    until("the lead is idle", || {
        thread(&s.workbench, &lead.id).status == ThreadStatus::Idle
    })
    .await;
    assert!(s.workbench.changed_turns(&lead.id).await.unwrap_or_default().is_empty());
    // The held snapshot excludes this file. Preview counts it; revert removes it, after the ref is published.
    let later = repo.join("later.txt");
    std::fs::write(&later, "after the snapshot").unwrap_or_else(|e| panic!("{e}"));
    let mut revert = Box::pin(async {
        if preview {
            s.workbench.revert_preview(&lead.id, turn).await
        } else {
            s.workbench.revert_to_turn(&lead.id, turn).await
        }
    });
    assert!(
        futures::poll!(&mut revert).is_pending(),
        "revert must wait before validating checkpoint {turn}"
    );
    assert!(later.exists(), "files stay untouched while the checkpoint is held");
    drop(gate);
    assert_eq!(revert.await, Ok(1));
    assert_eq!(later.exists(), preview);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn preview_waits_for_a_checkpoint_ref_being_written() {
    delayed_checkpoint(true).await;
}

#[tokio::test]
async fn revert_waits_for_a_checkpoint_ref_being_written() {
    delayed_checkpoint(false).await;
}
