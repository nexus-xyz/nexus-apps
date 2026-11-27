//! Takes sessions' worktrees off the disk by the person's rules (Settings › Worktrees): when the session's
//! pull request merged, when the session is archived, or after some days without activity. A worktree goes only when
//! nothing would be lost ([`git::would_lose_work`]: uncommitted changes, unpushed commits) and its agent is not running,
//! and never when it is its project's own folder. Its branch stays, as with the session's own "Remove worktree".
//! A sweep says what it removed, and what a rule matched but it kept, and why.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::event::ThreadSummary;
use crate::{Result, Workbench, checkpoint_prefix, git, now_ms};

/// Which rules are on. All off: nothing is cleaned up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    /// The session's pull request merged.
    #[serde(default)]
    pub merged: bool,
    /// The session is archived.
    #[serde(default)]
    pub archived: bool,
    /// The session had no activity for this many days; None (or 0): off.
    #[serde(default)]
    pub idle_days: Option<u32>,
}

impl Rules {
    pub fn any(&self) -> bool {
        self.merged || self.archived || self.idle_days.is_some_and(|days| days > 0)
    }
}

/// The rule that matched a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Rule {
    Merged,
    Archived,
    Idle,
}

/// A session a rule matched: its worktree went, or stayed for the reason in `kept`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub thread_id: String,
    pub title: String,
    pub path: String,
    pub branch: String,
    pub rule: Rule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// When it ran (ms since the epoch).
    pub at: i64,
    pub removed: Vec<Entry>,
    pub kept: Vec<Entry>,
}

/// The head commit of the session's pull request when it merged, else None. It may ask GitHub, so it runs off the
/// async threads; the path is the session's worktree.
pub type MergedHead = Arc<dyn Fn(&ThreadSummary, &Path) -> Option<String> + Send + Sync>;

/// [`MergedHead`] through the GitHub CLI, which `path` finds: the branch's pull request, or a review's own.
pub fn merged_by_gh(path: String) -> MergedHead {
    Arc::new(move |thread: &ThreadSummary, dir: &Path| {
        let review = thread
            .worktree
            .as_ref()
            .and_then(|w| w.review.as_ref())
            .map(|r| r.number);
        git::merged_head(dir, &path, review)
    })
}

/// The apps' sweep schedule: the first sweep waits this long after launch, past the workbench's own start and the
/// first pull request reads.
pub const FIRST_SWEEP_AFTER: Duration = Duration::from_secs(60);
/// Then a sweep runs this often.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// At most this many pull requests are looked up in one sweep: each is a `gh` call.
pub const MAX_LOOKUPS: usize = 20;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The rule that matches without asking GitHub (archived, then idle), else None.
fn local_rule(rules: &Rules, thread: &ThreadSummary, now: i64) -> Option<Rule> {
    if rules.archived && thread.archived {
        return Some(Rule::Archived);
    }
    let days = rules.idle_days.filter(|days| *days > 0)?;
    (now - thread.updated_at >= i64::from(days) * DAY_MS).then_some(Rule::Idle)
}

/// Whether removing `dir` would take the project's own folder: it is that folder, or holds it.
fn holds_project(project: &Path, dir: &Path) -> bool {
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let (project, dir) = (real(project), real(dir));
    project.starts_with(&dir)
}

/// What a sweep decided for one session's worktree before removing it.
enum Decision {
    /// No rule matches.
    Skip,
    /// It stays, for this reason.
    Keep(Rule, String),
    /// It goes, with the commits this reaches counted as pushed (a merged pull request's head).
    Remove(Rule, Option<String>),
}

/// Asks GitHub when only the merged rule can match. The session's own branch must still be the head that merged: new
/// commits since are work the pull request does not hold. A review's checkout is someone else's work, kept on GitHub.
fn decide(local: Option<Rule>, merged: &MergedHead, thread: &ThreadSummary, dir: &Path) -> Decision {
    if let Some(rule) = local {
        return Decision::Remove(rule, None);
    }
    let Some(head) = merged(thread, dir) else {
        return Decision::Skip;
    };
    let review = thread.worktree.as_ref().is_some_and(|w| w.review.is_some());
    if review || !dir.exists() {
        return Decision::Remove(Rule::Merged, None);
    }
    if git::head_commit(dir).as_deref() != Some(head.as_str()) {
        return Decision::Keep(
            Rule::Merged,
            "It has commits that aren't in its merged pull request.".into(),
        );
    }
    Decision::Remove(Rule::Merged, Some(head))
}

impl Workbench {
    /// One sweep over the current scope's sessions with a worktree on the disk ([`Rules`]).
    pub async fn clean_worktrees(&self, rules: &Rules, merged: MergedHead) -> Result<Report> {
        self.clean_worktrees_as_of(rules, merged, now_ms()).await
    }

    /// [`Workbench::clean_worktrees`] as of `now` (ms since the epoch), which tests move forward.
    pub async fn clean_worktrees_as_of(&self, rules: &Rules, merged: MergedHead, now: i64) -> Result<Report> {
        let mut report = Report {
            at: now,
            ..Report::default()
        };
        if !rules.any() {
            return Ok(report);
        }
        let mut lookups = 0;
        for thread in self.threads()? {
            let Some(worktree) = thread.worktree.clone().filter(|worktree| !worktree.removed) else {
                continue;
            };
            let local = local_rule(rules, &thread, now);
            if local.is_none() && !(rules.merged && lookups < MAX_LOOKUPS) {
                continue;
            }
            let entry = |rule, kept: Option<String>| Entry {
                thread_id: thread.id.clone(),
                title: thread.title.clone(),
                path: worktree.path.clone(),
                branch: worktree.branch.clone(),
                rule,
                kept,
            };
            // A running agent is never asked about, nor one of its side chats' in its worktree: a merged rule waits for
            // the next sweep, unlisted.
            if self.is_live(&thread.id) || self.side_chat_live(&thread.id) {
                if let Some(rule) = local {
                    report.kept.push(entry(rule, Some("Its agent is running.".into())));
                }
                continue;
            }
            let Some(project) = self.store().project(&thread.project_id)? else {
                if let Some(rule) = local {
                    report
                        .kept
                        .push(entry(rule, Some("Its project is no longer open.".into())));
                }
                continue;
            };
            let (repo, dir) = (PathBuf::from(&project.path), PathBuf::from(&worktree.path));
            if holds_project(&repo, &dir) {
                if let Some(rule) = local {
                    report
                        .kept
                        .push(entry(rule, Some("It is its project's own folder.".into())));
                }
                continue;
            }
            if local.is_none() {
                lookups += 1;
            }
            let decision = {
                let (merged, thread, dir) = (merged.clone(), thread.clone(), dir.clone());
                tokio::task::spawn_blocking(move || decide(local, &merged, &thread, &dir))
                    .await
                    .map_err(|error| error.to_string())?
            };
            let (rule, pushed) = match decision {
                Decision::Skip => continue,
                Decision::Keep(rule, why) => {
                    report.kept.push(entry(rule, Some(why)));
                    continue;
                }
                Decision::Remove(rule, pushed) => (rule, pushed),
            };
            // The person may have started it, or a side chat, while GitHub answered.
            if self.is_live(&thread.id) || self.side_chat_live(&thread.id) {
                report.kept.push(entry(rule, Some("Its agent is running.".into())));
                continue;
            }
            let prefix = checkpoint_prefix(&thread.id);
            let removed = tokio::task::spawn_blocking(move || {
                git::remove_worktree_beyond(&repo, &dir, pushed.as_deref())?;
                // The worktree's checkpoints went with it.
                git::drop_refs(&repo, &prefix)
            })
            .await
            .map_err(|error| error.to_string())?;
            match removed {
                Ok(()) => {
                    // Not activity: the session keeps its place in the list.
                    self.change_at(&thread.id, false, |record| {
                        if let Some(worktree) = record.summary.worktree.as_mut() {
                            worktree.removed = true;
                        }
                    })?;
                    report.removed.push(entry(rule, None));
                }
                Err(why) => report.kept.push(entry(rule, Some(why))),
            }
        }
        Ok(report)
    }
}

/// A worktree as Settings › Worktrees lists it: a session's, or a folder in the worktrees or reviews folder that no
/// session in any scope names (left by a session deleted while the folder was held open, a crash, or another app).
/// T3 Code only reports such folders; here the person can remove them, as safely as a session's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub path: String,
    /// Its checked-out branch; None on a detached HEAD.
    pub branch: Option<String>,
    /// Its session; None: no session's (an orphan).
    pub thread_id: Option<String>,
    pub title: Option<String>,
    pub archived: bool,
    /// The session's last activity, else the folder's last change (ms since the epoch).
    pub updated_at: Option<i64>,
    /// Its agent, or a side chat's, is running: it can't be removed now.
    pub live: bool,
    /// The folder is gone already: removing it only tells git (`git worktree prune`).
    pub missing: bool,
}

/// The folders two levels into `root` (`<root>/<repository>/<name>`) that are a worktree: a `.git` file.
fn worktree_folders(root: &Path) -> Vec<PathBuf> {
    let folders = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    };
    folders(root)
        .iter()
        .flat_map(|repo| folders(repo))
        .filter(|dir| dir.join(".git").is_file())
        .collect()
}

/// A path as compared: real when it exists.
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The repository whose worktree `dir` is: the folder holding its common git folder (`<repo>/.git`).
fn repository_of(dir: &Path) -> Result<PathBuf> {
    let common = git::git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = PathBuf::from(common);
    match common.file_name() {
        Some(name) if name == ".git" => common.parent().map(Path::to_path_buf),
        // A bare repository: git takes its worktrees from the repository folder itself.
        _ => Some(common),
    }
    .ok_or_else(|| format!("{} is not a worktree of a repository.", dir.display()))
}

impl Workbench {
    /// This scope's sessions' worktrees, then the folders in the worktrees and reviews folders no session names.
    pub fn worktrees(&self) -> Result<Vec<Listed>> {
        let mut listed: Vec<Listed> = Vec::new();
        for thread in self.threads()? {
            let Some(worktree) = thread.worktree.clone().filter(|worktree| !worktree.removed) else {
                continue;
            };
            let dir = Path::new(&worktree.path);
            listed.push(Listed {
                branch: git::branch(dir).or(Some(worktree.branch.clone())),
                path: worktree.path.clone(),
                thread_id: Some(thread.id.clone()),
                title: Some(thread.title.clone()),
                archived: thread.archived,
                updated_at: Some(thread.updated_at),
                live: self.is_live(&thread.id) || self.side_chat_live(&thread.id),
                missing: !dir.exists(),
            });
        }
        let claimed: Vec<PathBuf> = self
            .store()
            .worktree_paths()?
            .iter()
            .map(|path| real(Path::new(path)))
            .collect();
        let config = self.config();
        for root in [config.worktrees, config.reviews].into_iter().flatten() {
            for dir in worktree_folders(&root) {
                if claimed.contains(&real(&dir)) {
                    continue;
                }
                let changed = std::fs::metadata(&dir)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| since.as_millis() as i64);
                listed.push(Listed {
                    branch: git::branch(&dir),
                    path: dir.to_string_lossy().into_owned(),
                    thread_id: None,
                    title: None,
                    archived: false,
                    updated_at: changed,
                    live: false,
                    missing: false,
                });
            }
        }
        Ok(listed)
    }

    /// Removes a worktree no session names, as [`Workbench::worktrees`] lists it, unless that would lose work
    /// (uncommitted changes, unpushed commits: [`git::remove_worktree`]). Its branch stays.
    pub async fn remove_orphan_worktree(&self, path: &str) -> Result<()> {
        let orphan = self
            .worktrees()?
            .into_iter()
            .any(|listed| listed.thread_id.is_none() && listed.path == path);
        if !orphan {
            return Err("That folder is not a worktree without a session. Remove a session's worktree from it.".into());
        }
        let dir = PathBuf::from(path);
        if self
            .store()
            .projects()?
            .iter()
            .any(|project| holds_project(Path::new(&project.path), &dir))
        {
            return Err("That folder holds a project: remove the project first.".into());
        }
        tokio::task::spawn_blocking(move || {
            let repo = repository_of(&dir)?;
            git::remove_worktree(&repo, &dir)
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_worktrees_in_a_worktrees_folder_and_their_repository() {
        let root = std::env::temp_dir().join(format!("nexus-cleanup-orphans-{}", uuid::Uuid::new_v4().simple()));
        let repo = root.join("repo");
        let worktrees = root.join("worktrees");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let run = |dir: &Path, args: &[&str]| git::git(dir, args).unwrap_or_else(|e| panic!("{e}"));
        run(&repo, &["init", "--quiet", "--initial-branch", "main"]);
        run(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let dir = worktrees.join("repo").join("nexus-fix");
        run(
            &repo,
            &["worktree", "add", "--quiet", "-b", "nexus/fix", &dir.to_string_lossy()],
        );
        // Not worktrees: a plain folder, and a whole repository.
        std::fs::create_dir_all(worktrees.join("repo").join("notes")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(worktrees.join("other").join("clone/.git")).unwrap_or_else(|e| panic!("{e}"));
        let found: Vec<PathBuf> = worktree_folders(&worktrees).iter().map(|dir| real(dir)).collect();
        assert_eq!(found, [real(&dir)]);
        assert_eq!(
            real(&repository_of(&dir).unwrap_or_else(|e| panic!("{e}"))),
            real(&repo)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn session(archived: bool, updated_at: i64) -> ThreadSummary {
        serde_json::from_value(serde_json::json!({
            "id": "t_1", "projectId": "p", "title": "Fix it", "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": updated_at,
            "archived": archived,
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn archived_then_idle_match_without_asking_github() {
        let now = 30 * DAY_MS;
        let all = Rules {
            merged: true,
            archived: true,
            idle_days: Some(7),
        };
        assert_eq!(local_rule(&all, &session(true, now), now), Some(Rule::Archived));
        assert_eq!(
            local_rule(&all, &session(false, now - 7 * DAY_MS), now),
            Some(Rule::Idle)
        );
        assert_eq!(local_rule(&all, &session(false, now - 6 * DAY_MS), now), None);
        let idle_only = Rules {
            idle_days: Some(7),
            ..Rules::default()
        };
        assert_eq!(local_rule(&idle_only, &session(true, now), now), None);
        // 0 days is off, not "at once".
        let zero = Rules {
            idle_days: Some(0),
            ..Rules::default()
        };
        assert!(!zero.any());
        assert_eq!(local_rule(&zero, &session(false, 0), now), None);
    }

    #[test]
    fn the_projects_own_folder_or_one_holding_it_is_never_removed() {
        let root = std::env::temp_dir().join(format!("nexus-cleanup-holds-{}", std::process::id()));
        let project = root.join("repo");
        let _ = std::fs::create_dir_all(&project);
        assert!(holds_project(&project, &project));
        assert!(holds_project(&project, &root));
        assert!(holds_project(&project, &root.join("repo/.").join("../repo")));
        assert!(!holds_project(&project, &root.join("worktrees/repo/nexus-fix")));
        assert!(!holds_project(&project, &project.join("inner")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
