//! Helpers for the cross-project subsession tests: another git repository open in the workbench, and a child that
//! names it.

use nexus_workbench::event::{Project, SessionAgent};
use nexus_workbench::{OrchCode, StartChild};

use super::{Setup, child, git};

/// A git repository `name` with one commit, open in the workbench, not yet trusted.
pub fn repo(s: &Setup, name: &str) -> Project {
    let repo = s.root.join(name);
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "one"]);
    s.workbench.add_project(&repo).unwrap_or_else(|e| panic!("{e}"))
}

/// The repository `elsewhere`, not yet trusted.
pub fn elsewhere(s: &Setup) -> Project {
    repo(s, "elsewhere")
}

/// [`repo`] `name`, trusted.
pub fn trusted_repo(s: &Setup, name: &str) -> Project {
    let project = repo(s, name);
    s.workbench
        .set_claude_trust(&project.id, true)
        .unwrap_or_else(|e| panic!("{e}"));
    project
}

/// [`elsewhere`], trusted.
pub fn trusted(s: &Setup) -> Project {
    trusted_repo(s, "elsewhere")
}

/// A child of `agent` that runs `prompt` in `project`.
pub fn into_as(agent: SessionAgent, project: &str, prompt: &str) -> StartChild {
    StartChild {
        project: Some(project.into()),
        ..child(agent, prompt)
    }
}

/// A Claude child that runs `prompt` in `project`.
pub fn into(project: &str, prompt: &str) -> StartChild {
    into_as(SessionAgent::Claude, project, prompt)
}

/// The refusal of a start of `start` under `parent`: its code and message.
pub async fn refused(s: &Setup, parent: &str, start: StartChild) -> (OrchCode, String) {
    match s.workbench.start_child(parent, start).await {
        Ok(summary) => panic!("started {}", summary.id),
        Err(error) => (error.code, error.message),
    }
}

/// Shuts the workbench down and removes its folder.
pub async fn done(s: Setup) {
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
