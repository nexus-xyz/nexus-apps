//! The pull request watch's view of the sessions on a person's own servers (`pr_watch.rs`): their summaries, the
//! repository each one's pull request is in, and the watch's changes to them. The watch reads a server session's pull
//! request with this computer's `gh` by the repository's name (`nexus_pull_requests::remote`), so the server needs no
//! `gh`. It changes the session through the server's host, with the same calls as the background service's
//! (`nexus_pull_requests::over_service`). A cloud machine's sessions are not here: Gateway relays them.

use nexus_pull_requests::remote::Repository;
use nexus_pull_requests::watch::Changed;
use nexus_workbench::event::{Project, ThreadSummary};
use serde_json::{Value, json};

use super::{Machines, Reach};

/// The repository of `thread` in `project`: the project's `origin` remote (the first it lists), the session's own
/// branch (its worktree's), else the project's.
pub(crate) fn repository_of(thread: &ThreadSummary, project: Option<&Project>) -> Repository {
    Repository {
        remote: project
            .and_then(|project| project.remotes.first())
            .cloned()
            .unwrap_or_default(),
        branch: thread
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.clone())
            .or_else(|| project.and_then(|project| project.branch.clone())),
        host: project.map(|project| project.host).unwrap_or_default(),
    }
}

impl Machines {
    /// Every SSH server's sessions, as last read.
    pub fn ssh_sessions(&self) -> Vec<ThreadSummary> {
        self.state
            .lock()
            .map(|state| {
                state
                    .servers
                    .iter()
                    .filter(|server| matches!(server.reach, Reach::Ssh(_)))
                    .flat_map(|server| server.threads.values().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The SSH server that holds session `thread`, the session, and its repository. None for this computer's sessions
    /// and a cloud machine's.
    pub fn ssh_session(&self, thread: &str) -> Option<(String, ThreadSummary, Repository)> {
        let state = self.state.lock().ok()?;
        state.servers.iter().find_map(|server| {
            if !matches!(server.reach, Reach::Ssh(_)) {
                return None;
            }
            let summary = server.threads.get(thread)?;
            let project = server.projects.iter().find(|project| project.id == summary.project_id);
            Some((
                server.setting.id.clone(),
                summary.clone(),
                repository_of(summary, project),
            ))
        })
    }

    /// The watch's change to session `thread` on server `machine`: one of the calls the background service takes
    /// too (`wb_settle_merged`, `wb_check_failed`, `wb_link_pull_request`, `wb_thread_update`, `wb_send`).
    pub fn pull_change(&self, machine: &str, call: &'static str, args: Value) -> Changed {
        let answer = self.call(machine, call, args);
        Box::pin(async move {
            answer
                .await
                .map_err(|_| "The server's answer was lost.".to_string())?
                .map(drop)
        })
    }

    /// News for a snoozed server session: it wakes (`snoozedUntil: 0`). Any other is left as it is.
    pub fn pull_wake(&self, machine: &str, thread: &ThreadSummary) -> Changed {
        let now = nexus_workbench::now_ms();
        if !thread.aside.snoozed_until.is_some_and(|until| until > now) {
            return Box::pin(async { Ok(()) });
        }
        self.pull_change(
            machine,
            "wb_thread_update",
            json!({ "threadId": thread.id, "snoozedUntil": 0 }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(worktree_branch: Option<&str>) -> ThreadSummary {
        let mut thread: ThreadSummary = serde_json::from_value(json!({
            "id": "t1", "projectId": "p1", "title": "Server session",
            "agent": "claude", "model": null, "permissionMode": "default", "via": "nexus",
            "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false
        }))
        .expect("thread summary wire shape");
        thread.worktree = worktree_branch.map(|branch| {
            serde_json::from_value(json!({ "path": "/srv/wt", "branch": branch, "base": "origin/main" }))
                .expect("worktree wire shape")
        });
        thread
    }

    fn project(remotes: &[&str], branch: Option<&str>) -> Project {
        serde_json::from_value(json!({
            "id": "p1", "name": "app", "path": "/srv/app", "branch": branch,
            "remotes": remotes, "addedAt": 1
        }))
        .expect("project wire shape")
    }

    #[test]
    fn a_server_sessions_repository_is_its_projects_origin_and_its_own_branch() {
        let project = project(
            &["git@github.com:o/r.git", "https://github.com/fork/r.git"],
            Some("main"),
        );
        let worktree = repository_of(&thread(Some("feat")), Some(&project));
        assert_eq!(worktree.remote, "git@github.com:o/r.git");
        assert_eq!(worktree.branch.as_deref(), Some("feat"));
        // A session in the project's own folder is on the project's branch.
        let shared = repository_of(&thread(None), Some(&project));
        assert_eq!(shared.branch.as_deref(), Some("main"));
        // No project read yet: no remote, which the read refuses by name (`remote::NO_REMOTE`).
        let unknown = repository_of(&thread(None), None);
        assert_eq!((unknown.remote.as_str(), unknown.branch), ("", None));
    }
}
