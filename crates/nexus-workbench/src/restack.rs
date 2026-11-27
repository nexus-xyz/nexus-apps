//! Rebases a stack of branches onto its trunk, bottom first, and pushes each one with a lease. A stack of
//! pull requests needs it when its trunk moved, and after its bottom pull request merged by squash: the layers above
//! still carry the merged commits. GitHub has no API for it. It runs in a detached worktree of its own, so the
//! session's folder and the agent's work in it are never touched. The steps are T3 Code's (`githubStackRebase.ts`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::{changing_worktrees, git, git_with};

/// One branch of the stack, at the head the person confirmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer<'a> {
    pub number: u64,
    pub branch: &'a str,
    pub head: &'a str,
}

/// Where a restack stopped. The `done` layers under `number` were rebased and pushed; the others were not touched.
#[derive(Debug, Clone, PartialEq)]
pub struct Stopped {
    pub number: u64,
    pub done: usize,
    /// The rebase itself stopped, on a conflict or something like it; false: a fetch, a checkout or a push failed.
    pub rebase: bool,
    pub message: String,
}

/// Rebases `layers` (bottom first) onto `origin/<trunk>` from `repo` and force-pushes each one that moved, leased on
/// its confirmed head. `merged_below` is the last head of the merged layer under the bottom one, when there is one:
/// a squash left its commits outside the trunk's history, so the bottom layer is cut at it. Returns how many layers
/// were rebased.
pub fn restack(repo: &Path, trunk: &str, layers: &[Layer], merged_below: Option<&str>) -> Result<usize, Stopped> {
    let Some(first) = layers.first() else {
        return Ok(0);
    };
    let stopped = |message: String| Stopped {
        number: first.number,
        done: 0,
        rebase: false,
        message,
    };
    let mut fetch = vec![format!("+refs/heads/{trunk}:refs/remotes/origin/{trunk}")];
    fetch.extend(
        layers
            .iter()
            .map(|layer| format!("+refs/heads/{0}:refs/remotes/origin/{0}", layer.branch)),
    );
    let mut args = vec!["fetch", "--quiet", "--no-tags", "origin"];
    args.extend(fetch.iter().map(String::as_str));
    git(repo, &args).map_err(|error| stopped(format!("Could not fetch the stack: {error}")))?;

    let dir = scratch_dir();
    let path = dir.to_string_lossy().into_owned();
    changing_worktrees(repo, || {
        git(repo, &["worktree", "add", "--quiet", "--detach", &path, first.head])
    })
    .map_err(|error| stopped(format!("Could not prepare the rebase: {error}")))?;
    let result = cascade(&dir, trunk, layers, merged_below);
    // A throwaway detached worktree: nothing in it is lost, and a failed removal leaves only a folder git prunes later.
    let _ = changing_worktrees(repo, || git(repo, &["worktree", "remove", "--force", &path]));
    result
}

fn scratch_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("nexus-restack-{}-{nanos}", std::process::id()))
}

fn cascade(dir: &Path, trunk: &str, layers: &[Layer], merged_below: Option<&str>) -> Result<usize, Stopped> {
    let mut parent_old = format!("origin/{trunk}");
    let mut parent_new = parent_old.clone();
    let merged_parent = merged_below.filter(|merged| {
        layers
            .first()
            .is_some_and(|first| git(dir, &["merge-base", "--is-ancestor", merged, first.head]).is_ok())
    });
    for (done, layer) in layers.iter().enumerate() {
        let stopped = |rebase: bool, message: String| Stopped {
            number: layer.number,
            done,
            rebase,
            message,
        };
        // The commits to move: those above the old parent, the merged layer or the fork point for the bottom one.
        let upstream = match (done, merged_parent) {
            (0, Some(merged)) => merged.to_string(),
            (0, None) => git(dir, &["merge-base", &parent_old, layer.head]).map_err(|error| stopped(false, error))?,
            _ => parent_old.clone(),
        };
        git(dir, &["checkout", "--quiet", "--detach", layer.head]).map_err(|error| stopped(false, error))?;
        // No autostash, no other branches moved along, no editor: the person's config could ask for any of these.
        let rebased = git_with(
            dir,
            &[
                "-c",
                "rebase.autoStash=false",
                "-c",
                "rebase.updateRefs=false",
                "rebase",
                "--quiet",
                "--onto",
                &parent_new,
                &upstream,
            ],
            &[("GIT_EDITOR", "true"), ("GIT_SEQUENCE_EDITOR", "true")],
        );
        if let Err(error) = rebased {
            // The worktree is thrown away next; the abort only keeps git from leaving a rebase state behind.
            let _ = git(dir, &["rebase", "--abort"]);
            return Err(stopped(true, error));
        }
        let head = git(dir, &["rev-parse", "HEAD"]).map_err(|error| stopped(false, error))?;
        if head != layer.head {
            let lease = format!("--force-with-lease=refs/heads/{}:{}", layer.branch, layer.head);
            let refspec = format!("{head}:refs/heads/{}", layer.branch);
            git(dir, &["push", "--quiet", "origin", &lease, &refspec]).map_err(|error| stopped(false, error))?;
        }
        parent_old = layer.head.to_string();
        parent_new = head;
    }
    Ok(layers.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare `origin` with `main` and a two-layer stack (`a` on `main`, `b` on `a`), and a clone of it. Returns the
    /// clone, and the heads of `a` and `b`.
    struct Fixture {
        root: PathBuf,
        clone: PathBuf,
        a: String,
        b: String,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("nexus-restack-{name}-{}", uuid::Uuid::new_v4().simple()));
            let origin = root.join("origin.git");
            let clone = root.join("clone");
            std::fs::create_dir_all(&origin).unwrap_or_else(|e| panic!("{e}"));
            run(&origin, &["init", "--quiet", "--bare", "--initial-branch", "main"]);
            run(&root, &["clone", "--quiet", &origin.to_string_lossy(), "clone"]);
            run(&clone, &["config", "user.name", "t"]);
            run(&clone, &["config", "user.email", "t@t"]);
            run(&clone, &["switch", "--quiet", "--create", "main"]);
            commit(&clone, "base.txt", "base\n");
            run(&clone, &["push", "--quiet", "origin", "main"]);
            run(&clone, &["switch", "--quiet", "--create", "a"]);
            // Two commits: a squash of them matches neither, so git cannot drop them by patch on its own.
            commit(&clone, "a.txt", "draft\n");
            let a = commit(&clone, "a.txt", "a\n");
            run(&clone, &["switch", "--quiet", "--create", "b"]);
            let b = commit(&clone, "b.txt", "b\n");
            run(&clone, &["push", "--quiet", "origin", "a", "b"]);
            run(&clone, &["switch", "--quiet", "main"]);
            Self { root, clone, a, b }
        }

        /// What `origin` holds for `refname`.
        fn origin(&self, refname: &str) -> String {
            run(&self.root.join("origin.git"), &["rev-parse", refname])
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    fn commit(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap_or_else(|e| panic!("{e}"));
        run(dir, &["add", file]);
        run(dir, &["commit", "--quiet", "-m", file]);
        run(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn after_a_squash_merge_the_layer_above_moves_onto_the_trunk_without_the_merged_commits() {
        let fixture = Fixture::new("squash");
        // GitHub squashes `a` into `main`: the same change, in a new commit (its own message, or it would be `a`).
        std::fs::write(fixture.clone.join("a.txt"), "a\n").unwrap_or_else(|e| panic!("{e}"));
        run(&fixture.clone, &["add", "a.txt"]);
        run(&fixture.clone, &["commit", "--quiet", "-m", "a (#1)"]);
        run(&fixture.clone, &["push", "--quiet", "origin", "main"]);
        let main = fixture.origin("main");

        let layers = [Layer {
            number: 2,
            branch: "b",
            head: &fixture.b,
        }];
        assert_eq!(restack(&fixture.clone, "main", &layers, Some(&fixture.a)), Ok(1));

        let b = fixture.origin("b");
        assert_ne!(b, fixture.b, "b was pushed");
        assert_eq!(fixture.origin("b^"), main, "b sits on main, its one commit on top");
        // No scratch worktree is left behind.
        assert_eq!(run(&fixture.clone, &["worktree", "list"]).lines().count(), 1);
    }

    #[test]
    fn a_whole_stack_follows_a_trunk_that_moved_and_each_layer_stays_on_the_one_below() {
        let fixture = Fixture::new("trunk");
        commit(&fixture.clone, "other.txt", "other\n");
        run(&fixture.clone, &["push", "--quiet", "origin", "main"]);
        let main = fixture.origin("main");

        let layers = [
            Layer {
                number: 1,
                branch: "a",
                head: &fixture.a,
            },
            Layer {
                number: 2,
                branch: "b",
                head: &fixture.b,
            },
        ];
        assert_eq!(restack(&fixture.clone, "main", &layers, None), Ok(2));

        assert_eq!(fixture.origin("a~2"), main, "both of a's commits sit on main");
        assert_eq!(fixture.origin("b^"), fixture.origin("a"));
    }

    #[test]
    fn a_conflict_stops_at_that_layer_and_pushes_nothing_from_it() {
        let fixture = Fixture::new("conflict");
        commit(&fixture.clone, "b.txt", "main's own b\n");
        run(&fixture.clone, &["push", "--quiet", "origin", "main"]);

        let layers = [
            Layer {
                number: 1,
                branch: "a",
                head: &fixture.a,
            },
            Layer {
                number: 2,
                branch: "b",
                head: &fixture.b,
            },
        ];
        let stopped = restack(&fixture.clone, "main", &layers, None).expect_err("b conflicts with main");
        assert_eq!((stopped.number, stopped.done, stopped.rebase), (2, 1, true));
        assert_ne!(fixture.origin("a"), fixture.a, "a, below the conflict, was pushed");
        assert_eq!(fixture.origin("b"), fixture.b, "b was not");
        assert_eq!(run(&fixture.clone, &["worktree", "list"]).lines().count(), 1);
    }

    #[test]
    fn a_layer_pushed_since_it_was_confirmed_is_refused_by_the_lease() {
        let fixture = Fixture::new("lease");
        commit(&fixture.clone, "other.txt", "other\n");
        run(&fixture.clone, &["push", "--quiet", "origin", "main"]);
        // Someone pushes to `a` after the person confirmed its old head.
        run(&fixture.clone, &["switch", "--quiet", "a"]);
        let pushed = commit(&fixture.clone, "a2.txt", "a2\n");
        run(&fixture.clone, &["push", "--quiet", "origin", "a"]);
        run(&fixture.clone, &["switch", "--quiet", "main"]);

        let layers = [Layer {
            number: 1,
            branch: "a",
            head: &fixture.a,
        }];
        let stopped = restack(&fixture.clone, "main", &layers, None).expect_err("the lease refuses");
        assert_eq!((stopped.number, stopped.done, stopped.rebase), (1, 0, false));
        assert_eq!(fixture.origin("a"), pushed, "their push stays");
    }
}
