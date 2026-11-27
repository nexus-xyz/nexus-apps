//! The Changes view's other sources (`wb_diff`, `wb_diff_targets`): the branch against the branch it is based on, one
//! commit, and what is not committed yet. The session's own views (all its changes, one turn) read its checkpoints
//! instead ([`crate::Workbench::changes`]). Git runs in the session's folder, so through a Nexus host it runs on the
//! host's machine; an older host lacks [`FEATURE`] and the window does not offer these.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Changes, MAX_DIFF, Result, Workbench, git};

/// What a core that answers `wb_diff` and `wb_diff_targets` says in its features.
pub const FEATURE: &str = "diff-sources";

/// How many of the branch's newest commits the picker lists.
pub const COMMIT_LIMIT: usize = 100;

/// What the Changes view compares, besides the session's checkpoints.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DiffSource {
    /// The folder as it is now (commits and uncommitted files) against where its branch left its base branch.
    Branch,
    /// What one commit changed, against its first parent.
    Commit { id: String },
    /// The folder as it is now (staged, not staged and untracked files) against HEAD.
    Uncommitted,
}

/// One commit in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub id: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// When it was committed, in seconds since 1970.
    pub time: i64,
}

/// What the picker offers: the branch the folder's branch is compared against, and its newest commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffTargets {
    /// `origin/main`; None when there is no branch to compare against (no commits, no remote, no `main`).
    pub base: Option<String>,
    /// Newest first, at most [`COMMIT_LIMIT`].
    pub commits: Vec<Commit>,
}

impl Workbench {
    /// The base branch and the newest commits of the session's folder, for the Changes view's picker.
    pub async fn diff_targets(&self, thread_id: &str) -> Result<DiffTargets> {
        let (dir, recorded) = self.diff_folder(thread_id)?;
        tokio::task::spawn_blocking(move || {
            Ok(DiffTargets {
                base: base_branch(&dir, recorded.as_deref()),
                commits: commits(&dir, COMMIT_LIMIT)?,
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// The session's folder compared as `source` says, as a unified diff.
    pub async fn diff(&self, thread_id: &str, source: DiffSource) -> Result<Changes> {
        let (dir, recorded) = self.diff_folder(thread_id)?;
        tokio::task::spawn_blocking(move || {
            let (diff, truncated) = diff(&dir, &source, recorded.as_deref())?;
            Ok(Changes { diff, truncated })
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Where the session works, and the base its worktree was made from (None: the project's own folder).
    fn diff_folder(&self, thread_id: &str) -> Result<(std::path::PathBuf, Option<String>)> {
        let record = self.record(thread_id)?;
        let dir = self
            .workdir(&record)
            .ok_or_else(|| "This session's folder is gone.".to_string())?;
        Ok((dir, record.summary.worktree.map(|worktree| worktree.base)))
    }
}

/// The branch the checked-out one is compared against: the one its worktree was made from (`gh-merge-base`, else the
/// session's recorded base), else the remote's default branch (`origin/HEAD`), else the first of `origin/main`,
/// `origin/master`, `main` and `master` that exists. Never the checked-out branch itself.
pub fn base_branch(dir: &Path, recorded: Option<&str>) -> Option<String> {
    let exists = |name: &str| {
        git::git(
            dir,
            &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")],
        )
        .is_ok()
    };
    let current = git::checked_out(dir);
    let tracked = current.as_deref().and_then(|branch| git::base_ref(dir, branch));
    let remote_head = git::git(dir, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .filter(|name| !name.is_empty());
    tracked
        .into_iter()
        .chain(recorded.map(String::from))
        .chain(remote_head)
        .chain(["origin/main", "origin/master", "main", "master"].map(String::from))
        .filter(|name| !name.starts_with('-') && Some(name.as_str()) != current.as_deref())
        .find(|name| exists(name))
}

/// The newest `limit` commits reachable from HEAD; none on a branch without commits.
pub fn commits(dir: &Path, limit: usize) -> Result<Vec<Commit>> {
    if head(dir).is_none() {
        return Ok(vec![]);
    }
    let count = format!("--max-count={limit}");
    let out = git::git(
        dir,
        &[
            "log",
            "--no-color",
            &count,
            "--format=%H%x1f%h%x1f%an%x1f%ct%x1f%s",
            "HEAD",
            "--",
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\x1f');
            Some(Commit {
                id: fields.next()?.to_string(),
                short: fields.next()?.to_string(),
                author: fields.next()?.to_string(),
                time: fields.next()?.parse().ok()?,
                subject: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// The diff `source` names in `dir`, cut at [`MAX_DIFF`]. `recorded`: the session's worktree base, for [`base_branch`].
pub fn diff(dir: &Path, source: &DiffSource, recorded: Option<&str>) -> Result<(String, bool)> {
    let (from, to) = match source {
        DiffSource::Uncommitted => (head(dir).map_or_else(|| empty_tree(dir), Ok)?, git::snapshot(dir)?),
        DiffSource::Branch => {
            let head = head(dir).ok_or_else(|| "This branch has no commits yet.".to_string())?;
            let base = base_branch(dir, recorded).ok_or_else(|| {
                "There is no base branch to compare against (no origin/HEAD, main or master).".to_string()
            })?;
            (git::merge_base(dir, &base, &head)?, git::snapshot(dir)?)
        }
        DiffSource::Commit { id } => {
            // An id, never an option or a range: it is passed to git as it is.
            if !(4..=64).contains(&id.len()) || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("Not a commit id: {id}"));
            }
            let commit = git::git(dir, &["rev-parse", "--verify", "--quiet", &format!("{id}^{{commit}}")])
                .map_err(|_| format!("There is no commit {id} here."))?;
            let parent = git::git(dir, &["rev-parse", "--verify", "--quiet", &format!("{commit}^1")])
                .map_or_else(|_| empty_tree(dir), Ok)?;
            (parent, commit)
        }
    };
    git::diff(dir, &from, &to, MAX_DIFF)
}

/// HEAD's commit; None on a branch without commits.
fn head(dir: &Path) -> Option<String> {
    git::git(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .ok()
        .filter(|id| !id.is_empty())
}

/// The empty tree's id in this repository's hash, for a diff from nothing (a first commit, a branch with none).
fn empty_tree(dir: &Path) -> Result<String> {
    // stdin is empty: git's `output()` gives the child none.
    git::git(dir, &["hash-object", "-t", "tree", "--stdin"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch repository with a fixed identity, removed when dropped.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Repo {
            let dir = std::env::temp_dir().join(format!("nexus-diff-source-{name}-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
            let repo = Repo(dir);
            repo.git(&["init", "--quiet", "--initial-branch=main"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "user.email", "test@example.invalid"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            git::git(&self.0, args).unwrap_or_else(|error| panic!("git {args:?}: {error}"))
        }

        fn write(&self, path: &str, text: &str) {
            std::fs::write(self.0.join(path), text).unwrap_or_else(|e| panic!("{e}"));
        }

        fn commit(&self, path: &str, text: &str, message: &str) -> String {
            self.write(path, text);
            self.git(&["add", "--all"]);
            self.git(&["commit", "--quiet", "-m", message]);
            self.git(&["rev-parse", "HEAD"])
        }

        fn diff(&self, source: DiffSource) -> Result<String> {
            diff(&self.0, &source, None).map(|(text, _)| text)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn uncommitted_shows_staged_unstaged_and_untracked_files_against_head() {
        let repo = Repo::new("uncommitted");
        repo.commit("a.txt", "one\n", "first");
        repo.commit("b.txt", "two\n", "second");
        repo.write("a.txt", "one changed\n");
        repo.write("b.txt", "two staged\n");
        repo.git(&["add", "b.txt"]);
        repo.write("c.txt", "new\n");
        let text = repo.diff(DiffSource::Uncommitted).unwrap_or_else(|e| panic!("{e}"));
        assert!(text.contains("+one changed"), "{text}");
        assert!(text.contains("+two staged"), "{text}");
        assert!(text.contains("b/c.txt") && text.contains("+new"), "{text}");
        // The person's index is left as it was: c.txt is still untracked, b.txt still staged.
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "b.txt");
        assert!(repo.git(&["status", "--porcelain"]).contains("?? c.txt"));
    }

    #[test]
    fn uncommitted_on_a_branch_without_commits_is_every_file() {
        let repo = Repo::new("unborn");
        repo.write("a.txt", "one\n");
        let text = repo.diff(DiffSource::Uncommitted).unwrap_or_else(|e| panic!("{e}"));
        assert!(text.contains("new file mode") && text.contains("+one"), "{text}");
        assert!(
            commits(&repo.0, COMMIT_LIMIT)
                .unwrap_or_else(|e| panic!("{e}"))
                .is_empty()
        );
        assert_eq!(
            repo.diff(DiffSource::Branch)
                .err()
                .unwrap_or_else(|| panic!("no error")),
            "This branch has no commits yet."
        );
    }

    #[test]
    fn a_commit_shows_only_its_own_change_and_a_first_commit_shows_its_files() {
        let repo = Repo::new("commit");
        let first = repo.commit("a.txt", "one\n", "first");
        let second = repo.commit("a.txt", "one\ntwo\n", "second");
        repo.write("a.txt", "uncommitted\n");
        let text = repo
            .diff(DiffSource::Commit { id: second.clone() })
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(text.contains("+two") && !text.contains("uncommitted"), "{text}");
        assert!(!text.contains("+one"), "{text}");
        let short = repo
            .diff(DiffSource::Commit {
                id: second[..8].to_string(),
            })
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(short, text);
        let root = repo
            .diff(DiffSource::Commit { id: first })
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(root.contains("new file mode") && root.contains("+one"), "{root}");
    }

    #[test]
    fn a_commit_id_must_be_hex_and_must_exist() {
        let repo = Repo::new("bad-commit");
        repo.commit("a.txt", "one\n", "first");
        for id in ["--output=x", "HEAD", "main..HEAD", "abc"] {
            let error = repo
                .diff(DiffSource::Commit { id: id.into() })
                .err()
                .unwrap_or_else(|| panic!("no error"));
            assert!(error.starts_with("Not a commit id"), "{id}: {error}");
        }
        let error = repo
            .diff(DiffSource::Commit { id: "deadbeef".into() })
            .err()
            .unwrap_or_else(|| panic!("no error"));
        assert_eq!(error, "There is no commit deadbeef here.");
    }

    #[test]
    fn the_branch_compares_against_where_it_left_main_with_uncommitted_files() {
        let repo = Repo::new("branch");
        repo.commit("a.txt", "one\n", "first");
        repo.git(&["switch", "--quiet", "-c", "feature"]);
        repo.commit("b.txt", "feature\n", "on the branch");
        // main moves on after the branch left it: its change is not the branch's.
        repo.git(&["switch", "--quiet", "main"]);
        repo.commit("c.txt", "main only\n", "on main");
        repo.git(&["switch", "--quiet", "feature"]);
        repo.write("d.txt", "not committed\n");
        assert_eq!(base_branch(&repo.0, None).as_deref(), Some("main"));
        let text = repo.diff(DiffSource::Branch).unwrap_or_else(|e| panic!("{e}"));
        assert!(text.contains("+feature"), "{text}");
        assert!(text.contains("+not committed"), "{text}");
        assert!(!text.contains("main only"), "{text}");
    }

    #[test]
    fn the_base_branch_is_the_recorded_one_then_origin_head_then_main_or_master() {
        let repo = Repo::new("base");
        repo.git(&["switch", "--quiet", "-c", "master"]);
        repo.commit("a.txt", "one\n", "first");
        repo.git(&["branch", "release"]);
        repo.git(&["branch", "develop"]);
        repo.git(&["switch", "--quiet", "-c", "feature"]);
        // No main: master.
        assert_eq!(base_branch(&repo.0, None).as_deref(), Some("master"));
        // The remote's default branch, as `git clone` records it.
        repo.git(&["update-ref", "refs/remotes/origin/develop", "HEAD"]);
        repo.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ]);
        assert_eq!(base_branch(&repo.0, None).as_deref(), Some("origin/develop"));
        // The session's recorded base comes before it, and the worktree's `gh-merge-base` before that.
        assert_eq!(base_branch(&repo.0, Some("release")).as_deref(), Some("release"));
        assert_eq!(base_branch(&repo.0, Some("gone")).as_deref(), Some("origin/develop"));
        repo.git(&["config", "branch.feature.gh-merge-base", "develop"]);
        assert_eq!(base_branch(&repo.0, Some("release")).as_deref(), Some("origin/develop"));
        // On the base branch itself, never itself.
        repo.git(&["switch", "--quiet", "master"]);
        repo.git(&["symbolic-ref", "-d", "refs/remotes/origin/HEAD"]);
        repo.git(&["update-ref", "-d", "refs/remotes/origin/develop"]);
        assert_eq!(base_branch(&repo.0, Some("master")), None);
    }

    #[test]
    fn commits_are_the_newest_first_up_to_the_limit() {
        let repo = Repo::new("commits");
        for n in 0..5 {
            repo.commit("a.txt", &format!("{n}\n"), &format!("change {n}: a | b"));
        }
        let listed = commits(&repo.0, 3).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].subject, "change 4: a | b");
        assert_eq!(listed[0].author, "Test");
        assert_eq!(listed[0].id, repo.git(&["rev-parse", "HEAD"]));
        assert!(listed[0].id.starts_with(&listed[0].short));
        assert!(listed[0].time > 0);
    }
}
