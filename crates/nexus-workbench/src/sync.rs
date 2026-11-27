//! Keeping a session's branch in step with its remote, from the git panel: a fetch in the background, Pull (a
//! fast-forward, or a rebase when asked), and a new branch from the branch picker. None of it touches a folder while an
//! agent's turn is under way in it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::event::ThreadStatus;
use crate::{Emit, GitState, Result, Workbench, git, overlaps, read_git};

/// `wb_git_pull`'s answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pull {
    pub git: GitState,
    /// The rebase stopped on conflicts in these files and was undone: the branch and its files are as they were.
    /// None: the pull worked.
    pub conflict: Option<Vec<String>>,
}

/// The git panel's Pull row, when the branch's upstream has commits it lacks (the Nexus app's `pullAction` in
/// `git-panel-logic.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOffer {
    pub label: String,
    /// Replay the branch's own commits on top: both sides moved.
    pub rebase: bool,
    /// The row's tooltip.
    pub title: String,
}

/// A plain pull when the branch has no commits of its own, else a rebase of them on top. None: nothing to pull, or no
/// upstream to pull from.
pub fn pull_offer(git: &GitState) -> Option<PullOffer> {
    let upstream = git.upstream.as_ref().filter(|upstream| upstream.behind > 0)?;
    Some(if upstream.ahead == 0 {
        PullOffer {
            label: "Pull".into(),
            rebase: false,
            title: format!("Bring in {} new from {}", upstream.behind, upstream.name),
        }
    } else {
        PullOffer {
            label: format!("Rebase onto {}", upstream.name),
            rebase: true,
            title: format!(
                "Both have new commits: replay this branch's {} on top of {}'s {}. Undone if it conflicts.",
                upstream.ahead, upstream.name, upstream.behind
            ),
        }
    })
}

/// What the panel says when a rebase stopped on conflicts and was undone (`conflictText`).
pub fn conflict_text(files: &[String]) -> String {
    let shown = files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let more = if files.len() > 3 {
        format!(" and {} more", files.len() - 3)
    } else {
        String::new()
    };
    format!(
        "The rebase hit conflicts in {shown}{more}, so it was undone and nothing changed. Ask the agent to rebase and \
         resolve them, or do it in a terminal."
    )
}

const BUSY: &str = "Wait for the agent to finish its turn: it's working in this folder.";

/// The shortest time between two background fetches of one folder: two windows, or a panel opened and closed, don't
/// fetch again at once.
const FETCH_GAP: Duration = Duration::from_secs(60);

/// When each folder was last fetched.
static FETCHED: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);

/// Whether a background fetch of `dir` is due, and if so, marks it fetched now.
fn fetch_due(dir: &Path) -> bool {
    let Ok(mut fetched) = FETCHED.lock() else {
        return true;
    };
    let fetched = fetched.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if fetched.get(dir).is_some_and(|at| now.duration_since(*at) < FETCH_GAP) {
        return false;
    }
    fetched.insert(dir.to_path_buf(), now);
    true
}

impl Workbench {
    /// Whether an agent's turn is under way in `dir`, or in a folder that holds it or sits inside it.
    fn turn_running_in(&self, dir: &Path) -> Result<bool> {
        Ok(self.threads()?.iter().any(|thread| {
            self.is_live(&thread.id)
                && thread.status != ThreadStatus::Idle
                && self
                    .record(&thread.id)
                    .ok()
                    .and_then(|record| self.workdir(&record))
                    .is_some_and(|folder| overlaps(&folder, dir))
        }))
    }

    /// The panel's folder, and whether it is the session's own worktree.
    fn sync_folder(&self, thread_id: &str, repo: Option<&str>) -> Result<(PathBuf, bool)> {
        let record = self.record(thread_id)?;
        Ok((
            self.git_folder(thread_id, repo)?,
            repo.is_none() && record.summary.worktree.is_some(),
        ))
    }

    /// Fetches what the session's branch (or `repo`'s) is compared against, then reads its state. Meant for a timer:
    /// mid-turn, or within a minute of the last fetch, it only reads; a failed fetch (offline) is not an error.
    pub async fn fetch(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState> {
        let (dir, own) = self.sync_folder(thread_id, repo)?;
        let skip = self.turn_running_in(&dir)?;
        tokio::task::spawn_blocking(move || {
            if !skip && git::in_repo(&dir) && fetch_due(&dir) {
                let _ = git::fetch(&dir);
            }
            read_git(&dir, own)
        })
        .await
        .map_err(|error| error.to_string())
    }

    /// Brings the session's branch (or `repo`'s) up to date with its upstream ([`git::pull`]): a fast-forward, or with
    /// `rebase`, the branch's own commits on top of the upstream's. A rebase that conflicts is undone and its files
    /// named. Fails while an agent's turn is under way in the folder.
    pub async fn pull(&self, thread_id: &str, repo: Option<&str>, rebase: bool) -> Result<Pull> {
        let (dir, own) = self.sync_folder(thread_id, repo)?;
        if self.turn_running_in(&dir)? {
            return Err(BUSY.into());
        }
        tokio::task::spawn_blocking(move || {
            let pulled = git::pull(&dir, rebase)?;
            Ok(Pull {
                git: read_git(&dir, own),
                conflict: match pulled {
                    git::Pulled::Done => None,
                    git::Pulled::Conflict(files) => Some(files),
                },
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Makes a new branch at the current commit in the session's project folder (or `repo` inside it) and checks it
    /// out; uncommitted changes come along. A session with its own worktree keeps its branch.
    pub async fn create_branch(&self, thread_id: &str, repo: Option<&str>, name: &str) -> Result<GitState> {
        let (dir, own) = self.sync_folder(thread_id, repo)?;
        if own {
            return Err("A session with its own worktree keeps its branch.".into());
        }
        if self.turn_running_in(&dir)? {
            return Err(BUSY.into());
        }
        let name = name.to_string();
        let state = tokio::task::spawn_blocking(move || {
            git::create_branch(&dir, &name)?;
            Ok::<_, String>(read_git(&dir, false))
        })
        .await
        .map_err(|error| error.to_string())??;
        // The sidebar and the top bar show the project's branch.
        self.inner.host.emit(Emit::Projects(self.projects()?));
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(upstream: Option<(u32, u32)>, base: Option<(u32, u32)>) -> GitState {
        let tracking = |name: &str, (ahead, behind): (u32, u32)| git::Tracking {
            name: name.into(),
            ahead,
            behind,
        };
        GitState {
            path: "/repo".into(),
            branch: Some("feature".into()),
            branches: Vec::new(),
            uncommitted: None,
            repos: None,
            upstream: upstream.map(|counts| tracking("origin/feature", counts)),
            base: base.map(|counts| tracking("origin/main", counts)),
        }
    }

    #[test]
    fn offers_a_pull_when_only_behind_a_rebase_when_both_moved_nothing_otherwise() {
        let pull = pull_offer(&at(Some((0, 2)), None)).expect("pull");
        assert_eq!((pull.label.as_str(), pull.rebase), ("Pull", false));
        assert_eq!(pull.title, "Bring in 2 new from origin/feature");
        let rebase = pull_offer(&at(Some((1, 2)), None)).expect("rebase");
        assert_eq!(
            (rebase.label.as_str(), rebase.rebase),
            ("Rebase onto origin/feature", true)
        );
        assert_eq!(pull_offer(&at(Some((3, 0)), None)), None);
        // No upstream: nothing to pull from, even when behind the branch it was made from.
        assert_eq!(pull_offer(&at(None, Some((0, 4)))), None);
    }

    #[test]
    fn names_a_rebases_conflicted_files_a_few_at_most() {
        assert!(conflict_text(&["a.ts".into()]).contains("conflicts in a.ts, so it was undone"));
        let five: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        assert!(conflict_text(&five).contains("in a, b, c and 2 more,"));
    }

    #[test]
    fn a_folder_is_fetched_at_most_once_a_minute() {
        let dir = std::env::temp_dir().join(format!("nexus-fetch-due-{}", uuid::Uuid::new_v4().simple()));
        assert!(fetch_due(&dir));
        assert!(!fetch_due(&dir));
        assert!(fetch_due(&dir.join("other")));
    }
}
