//! The commit trailer hook against the real git on this machine. On git 2.54 and later it marks only new
//! commits in nexus-xyz repositories; on older git, which has no hooks in config, every commit is exactly as without
//! it. Either way, a commit never fails because of it. Without git the tests do nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use nexus_client::commit_trailer::{NAME, Surface, VARIABLE, parameters};

const NEXUS: &str = "git@github.com:nexus-xyz/nexus.git";
const TRAILER: &str = "Co-authored-by: Nexus CLI <cli@nexus.inc>";

struct Repo {
    dir: PathBuf,
    global: PathBuf,
}

impl Repo {
    /// A new repository with one commit, and `remote` as origin.
    fn new(remote: Option<&str>) -> Option<Repo> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos();
        let root = std::env::temp_dir().join(format!("nexus-commit-trailer-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).ok()?;
        let global = root.join("gitconfig");
        std::fs::write(&global, "").ok()?;
        let repo = Repo {
            dir: root.join("repo"),
            global,
        };
        let init = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&repo.dir)
            .output()
            .ok()?;
        if !init.status.success() {
            return None;
        }
        if let Some(remote) = remote {
            repo.ok(&["remote", "add", "origin", remote]);
        }
        repo.ok(&["commit", "-q", "--allow-empty", "-m", "base"]);
        Some(repo)
    }

    /// git in the repository, with the hook set, as a Nexus session runs it. Nothing of the machine's own git config
    /// or of an outer git process reaches it.
    fn git(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new("git");
        command
            .current_dir(&self.dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_REFLOG_ACTION")
            .env("GIT_CONFIG_GLOBAL", &self.global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@example.com")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.com")
            .env("GIT_EDITOR", "true")
            .env(VARIABLE, parameters(None, Surface::Cli));
        command.envs(env.iter().copied());
        command.output().unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.git(args, &[]);
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn message(&self, rev: &str) -> String {
        self.ok(&["log", "-1", "--format=%B", rev])
    }

    fn trailers(&self, rev: &str) -> usize {
        self.message(rev).lines().filter(|line| *line == TRAILER).count()
    }

    fn hook(&self, name: &str, body: &str) {
        let path = self.dir.join(".git/hooks").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap_or_else(|e| panic!("{e}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.dir.join(file), text).unwrap_or_else(|e| panic!("{e}"));
    }

    /// Whether this git runs hooks from config (2.54 and later).
    fn runs_config_hooks(&self) -> bool {
        self.git(&["hook", "list", "commit-msg"], &[]).status.success()
            && self
                .ok(&["hook", "list", "commit-msg"])
                .lines()
                .any(|line| line.trim() == NAME)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        if let Some(root) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[test]
fn a_new_commit_in_a_nexus_repository_gets_one_trailer() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    let marks = usize::from(repo.runs_config_hooks());
    repo.ok(&["commit", "-q", "--allow-empty", "-m", "feat: one"]);
    assert_eq!(repo.trailers("HEAD"), marks);
    repo.ok(&["commit", "-q", "--amend", "--allow-empty", "--no-edit"]);
    assert_eq!(repo.trailers("HEAD"), marks, "an amend adds no second trailer");
    repo.ok(&[
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "feat: two",
        "-m",
        "Co-authored-by: Jane Doe <jane@example.com>\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
    ]);
    let named = "feat: two\n\nCo-authored-by: Jane Doe <jane@example.com>\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>";
    let expected = if marks == 1 {
        format!("{named}\n{TRAILER}")
    } else {
        named.to_string()
    };
    assert_eq!(
        repo.message("HEAD"),
        expected,
        "Nexus joins the co-authors: Claude Code's trailer and the person stay"
    );
    repo.ok(&["commit", "-q", "--allow-empty", "-m", "feat: three\n\nbody\n---\nmore"]);
    assert!(
        repo.message("HEAD")
            .ends_with(if marks == 1 { TRAILER } else { "more" }),
        "a --- line in the body is not the end of the message"
    );
}

#[test]
fn desktop_attribution_survives_a_nested_cli_without_duplicate_trailers() {
    use nexus_client::commit_trailer::mark;
    use std::collections::HashMap;

    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    let mut env = HashMap::new();
    mark(&mut env, Surface::Desktop);
    mark(&mut env, Surface::Cli);
    let out = repo.git(
        &["commit", "-q", "--allow-empty", "-m", "feat: from desktop"],
        &[(VARIABLE, &env[VARIABLE])],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let message = repo.message("HEAD");
    assert_eq!(
        message
            .lines()
            .filter(|line| *line == Surface::Desktop.trailer())
            .count(),
        usize::from(repo.runs_config_hooks()),
    );
    assert_eq!(repo.trailers("HEAD"), 0, "the nested CLI keeps the Desktop surface");
}

#[test]
fn only_nexus_xyz_repositories_are_marked() {
    for remote in [
        "https://github.com/nexus-xyz/nexus.git",
        "ssh://git@github.com/nexus-xyz/nexus",
        "ssh://git@github.com:22/nexus-xyz/nexus",
        "https://github.com/Nexus-XYZ/nexus",
        // An SSH alias of github.com, as someone with two GitHub accounts names it.
        "git@github-work:nexus-xyz/nexus.git",
    ] {
        let Some(repo) = Repo::new(Some(remote)) else { return };
        let marks = usize::from(repo.runs_config_hooks());
        repo.ok(&["commit", "-q", "--allow-empty", "-m", "x"]);
        assert_eq!(repo.trailers("HEAD"), marks, "{remote}");
    }
    for remote in [
        None,
        Some("git@github.com:someone/nexus.git"),
        Some("git@github.com:someone/nexus-xyz.git"),
        Some("https://github.com/nexus-xyzz/x.git"),
    ] {
        let Some(repo) = Repo::new(remote) else { return };
        let claude = "x\n\nCo-Authored-By: Claude <noreply@anthropic.com>";
        repo.ok(&["commit", "-q", "--allow-empty", "-m", claude]);
        assert_eq!(repo.message("HEAD"), claude, "{remote:?}: untouched");
    }
}

#[cfg(unix)]
#[test]
fn the_hook_preserves_existing_files_and_symlinks_beside_the_message() {
    use std::os::unix::fs::symlink;

    for linked in [false, true] {
        let Some(repo) = Repo::new(Some(NEXUS)) else { return };
        let marks = usize::from(repo.runs_config_hooks());
        let neighbor = repo.dir.join(".git/COMMIT_EDITMSG.nexus");
        let target = if linked {
            repo.dir.join(".git/trailer-sentinel")
        } else {
            neighbor.clone()
        };
        let sentinel = "Keep this existing file unchanged.\n";
        std::fs::write(&target, sentinel).unwrap_or_else(|e| panic!("{e}"));
        if linked {
            symlink(&target, &neighbor).unwrap_or_else(|e| panic!("{e}"));
        }

        repo.ok(&["commit", "-q", "--allow-empty", "-m", "feat: preserve nearby files"]);

        assert_eq!(
            std::fs::read_to_string(&target).unwrap_or_else(|e| panic!("{e}")),
            sentinel,
            "the existing file stays unchanged (symlink: {linked})"
        );
        assert_eq!(
            std::fs::symlink_metadata(&neighbor)
                .unwrap_or_else(|e| panic!("{e}"))
                .file_type()
                .is_symlink(),
            linked,
            "the existing path stays in place"
        );
        assert_eq!(
            repo.trailers("HEAD"),
            marks,
            "the commit still receives one trailer when hooks run"
        );
    }
}

#[test]
fn the_remote_a_push_goes_to_decides() {
    let claude = "x\n\nCo-Authored-By: Claude <noreply@anthropic.com>";
    // A fork with nexus-xyz as its upstream is someone else's work: untouched.
    let Some(fork) = Repo::new(Some("git@github.com:someone/fork.git")) else {
        return;
    };
    fork.ok(&["remote", "add", "upstream", "https://github.com/nexus-xyz/nexus.git"]);
    fork.ok(&["commit", "-q", "--allow-empty", "-m", claude]);
    assert_eq!(fork.message("HEAD"), claude);
    // A push that goes to nexus-xyz counts, whatever the remote is called.
    fork.ok(&["config", "remote.pushDefault", "upstream"]);
    let marks = usize::from(fork.runs_config_hooks());
    fork.ok(&["commit", "-q", "--allow-empty", "-m", "y"]);
    assert_eq!(fork.trailers("HEAD"), marks);
    // The URL as git expands it: an insteadOf alias counts.
    let Some(alias) = Repo::new(Some("nx:nexus.git")) else {
        return;
    };
    alias.ok(&["config", "url.git@github.com:nexus-xyz/.insteadOf", "nx:"]);
    alias.ok(&["commit", "-q", "--allow-empty", "-m", "z"]);
    assert_eq!(alias.trailers("HEAD"), marks);
}

#[test]
fn merges_and_replayed_commits_are_left_alone() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    // Commits made outside a Nexus session: the hook is off for them.
    let outside = format!("hook.{NAME}.enabled=false");
    let commit_outside = |file: &str, message: &str| {
        repo.write(file, &format!("{message}\n"));
        repo.ok(&["add", file]);
        repo.ok(&["-c", &outside, "commit", "-q", "-m", message]);
        repo.message("HEAD")
    };
    repo.ok(&["checkout", "-q", "-b", "picked"]);
    let picked = commit_outside("picked", "picked work");
    repo.ok(&["checkout", "-q", "-b", "rebased", "main"]);
    let rebased = commit_outside("rebased", "rebased work");
    repo.ok(&["checkout", "-q", "-b", "merged", "main"]);
    commit_outside("merged", "merged work");
    repo.ok(&["checkout", "-q", "main"]);
    commit_outside("main", "main work");

    repo.ok(&["cherry-pick", "picked"]);
    assert_eq!(repo.message("HEAD"), picked, "a cherry-pick keeps the message");
    repo.ok(&["revert", "--no-edit", "HEAD"]);
    assert_eq!(repo.trailers("HEAD"), 0, "a revert is not new work");
    repo.ok(&["merge", "-q", "--no-ff", "merged", "-m", "merge merged"]);
    assert_eq!(repo.trailers("HEAD"), 0, "a merge is not new work");
    repo.ok(&["checkout", "-q", "rebased"]);
    repo.ok(&["rebase", "-q", "main"]);
    assert_eq!(repo.message("HEAD"), rebased, "a rebase keeps the message");

    // A conflicted merge, concluded with `git commit`.
    repo.ok(&["checkout", "-q", "-b", "left", "main"]);
    commit_outside("same", "left");
    repo.ok(&["checkout", "-q", "main"]);
    commit_outside("same", "right");
    assert!(!repo.git(&["merge", "-q", "left"], &[]).status.success());
    assert!(exists(&repo.dir.join(".git/MERGE_HEAD")));
    repo.write("same", "both\n");
    repo.ok(&["add", "same"]);
    repo.ok(&["commit", "-q", "--no-edit"]);
    assert_eq!(repo.trailers("HEAD"), 0, "concluding a merge is not new work");
}

#[test]
fn it_never_turns_an_aborted_commit_into_one() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    let head = repo.ok(&["rev-parse", "HEAD"]);
    for editor in [
        "sh -c ': > \"$1\"' editor",
        "sh -c 'printf \"# only a comment\\n\" > \"$1\"' editor",
    ] {
        let out = repo.git(&["commit", "-q", "--allow-empty"], &[("GIT_EDITOR", editor)]);
        assert!(!out.status.success(), "an empty message stops the commit ({editor})");
    }
    // `-v`: the diff below the scissors line is not a message.
    repo.write("f", "x\n");
    repo.ok(&["add", "f"]);
    let keep_only_the_diff = "sh -c 'sed -i.bak \"/^#/,\\$!d\" \"$1\"' editor";
    let out = repo.git(&["commit", "-q", "-v"], &[("GIT_EDITOR", keep_only_the_diff)]);
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(repo.ok(&["rev-parse", "HEAD"]), head);
}

#[test]
fn the_repository_s_own_hooks_still_run_and_still_decide() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    let marks = usize::from(repo.runs_config_hooks());
    repo.hook("commit-msg", "echo ran >> .git/commit-msg-ran");
    repo.hook("post-commit", "echo ran >> .git/post-commit-ran");
    repo.ok(&["commit", "-q", "--allow-empty", "-m", "x"]);
    assert!(exists(&repo.dir.join(".git/commit-msg-ran")));
    assert!(exists(&repo.dir.join(".git/post-commit-ran")));
    assert_eq!(repo.trailers("HEAD"), marks);
    repo.hook("commit-msg", "exit 1");
    assert!(
        !repo
            .git(&["commit", "-q", "--allow-empty", "-m", "refused"], &[])
            .status
            .success(),
        "a failing hook of the repository still refuses the commit"
    );
}

#[test]
fn a_person_turns_it_off_and_no_verify_skips_it() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    std::fs::write(&repo.global, format!("[hook \"{NAME}\"]\n\tenabled = false\n")).unwrap_or_else(|e| panic!("{e}"));
    repo.ok(&["commit", "-q", "--allow-empty", "-m", "x"]);
    assert_eq!(
        repo.trailers("HEAD"),
        0,
        "hook.nexus-commit-trailer.enabled=false in the global config"
    );
    std::fs::write(&repo.global, "").unwrap_or_else(|e| panic!("{e}"));
    repo.ok(&["commit", "-q", "--allow-empty", "--no-verify", "-m", "x"]);
    assert_eq!(repo.trailers("HEAD"), 0, "--no-verify skips commit-msg hooks");
}

#[test]
fn a_person_s_own_git_settings_still_apply() {
    let Some(repo) = Repo::new(Some(NEXUS)) else { return };
    let marks = usize::from(repo.runs_config_hooks());
    // `git -c` adds to the variable that carries the hook, and both apply.
    repo.ok(&["-c", "core.commentChar=;", "commit", "-q", "--allow-empty", "-m", "x"]);
    assert_eq!(repo.trailers("HEAD"), marks);
    assert_eq!(repo.ok(&["-c", "core.abbrev=12", "log", "-1", "--format=%h"]).len(), 12);
    // A setting the run already had stays, before the hook.
    let before = parameters(Some("'core.abbrev=12'"), Surface::Cli);
    let out = repo.git(&["log", "-1", "--format=%h"], &[(VARIABLE, &before)]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim().len(), 12);
    let out = repo.git(&["commit", "-q", "--allow-empty", "-m", "y"], &[(VARIABLE, &before)]);
    assert!(out.status.success());
    assert_eq!(repo.trailers("HEAD"), marks);
}
