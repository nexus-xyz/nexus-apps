//! `locate` across checkouts: a file the agent read in another git worktree, named by its name alone.

use std::path::{Path, PathBuf};

use super::locate;

/// Two worktrees of one repository, `a` (the session's) and `b`, removed when dropped.
struct Worktrees(PathBuf);

impl Drop for Worktrees {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Worktrees {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("nexus-worktrees-test-{}", uuid::Uuid::new_v4()));
        for tree in ["a", "b"] {
            std::fs::create_dir_all(dir.join(tree)).expect("make the worktree");
            // A worktree's `.git` is a file that points at the repository.
            std::fs::write(dir.join(tree).join(".git"), "gitdir: /r/.git/worktrees/x").expect("mark it");
        }
        Worktrees(dir)
    }

    fn root(&self, tree: &str) -> PathBuf {
        self.0.join(tree).canonicalize().expect("the worktree")
    }

    /// Write `path` in `tree`; answers its absolute path.
    fn at(&self, tree: &str, path: &str) -> String {
        let file = self.root(tree).join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("make its folder");
        std::fs::write(&file, tree).expect("write the file");
        file.to_string_lossy().into_owned()
    }

    /// The files under `folder`, as paths from it.
    fn list(folder: &Path) -> Vec<String> {
        fn walk(base: &Path, folder: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else if path.file_name().is_some_and(|name| name != ".git") {
                    out.push(
                        path.strip_prefix(base)
                            .expect("under base")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        let mut out = Vec::new();
        walk(folder, folder, &mut out);
        out
    }
}

#[test]
fn a_name_the_agent_read_in_another_worktree_opens_that_file() {
    let trees = Worktrees::new();
    let read = trees.at("b", "crates/x/src/manual_claude_launcher.rs");
    trees.at("a", "src/main.rs");
    let found = locate(
        &trees.root("a"),
        "manual_claude_launcher.rs",
        std::slice::from_ref(&read),
        Worktrees::list,
    );
    assert_eq!(found, Some(read));
}

#[test]
fn the_copy_the_agent_read_wins_over_the_same_path_in_the_session_folder() {
    let trees = Worktrees::new();
    trees.at("a", "src/same.rs");
    let read = trees.at("b", "src/same.rs");
    let root = trees.root("a");
    for path in ["same.rs", "src/same.rs", "./src/same.rs"] {
        assert_eq!(
            locate(&root, path, std::slice::from_ref(&read), Worktrees::list),
            Some(read.clone()),
            "{path}"
        );
    }
    // Without that evidence, the session's own copy opens, as before.
    assert_eq!(
        locate(&root, "src/same.rs", &[], Worktrees::list),
        Some("src/same.rs".into())
    );
    // The agent read the session's copy last: it opens.
    let own = trees.root("a").join("src/same.rs").to_string_lossy().into_owned();
    assert_eq!(
        locate(&root, "same.rs", &[own, read], Worktrees::list),
        Some("src/same.rs".into())
    );
}

#[test]
fn a_touched_file_in_the_session_folder_never_hides_the_path_as_written() {
    let trees = Worktrees::new();
    trees.at("a", "README.md");
    let nested = trees.at("a", "crates/x/README.md");
    assert_eq!(
        locate(&trees.root("a"), "README.md", &[nested], Worktrees::list),
        Some("README.md".into())
    );
}

#[test]
fn a_name_found_once_in_a_worktree_the_agent_worked_in_opens_there() {
    let trees = Worktrees::new();
    trees.at("a", "src/main.rs");
    let other = trees.at("b", "docs/notes.md");
    // The agent only ran a command in `b`; it never read the file.
    let worked = trees.root("b").to_string_lossy().into_owned();
    let root = trees.root("a");
    assert_eq!(
        locate(&root, "notes.md", std::slice::from_ref(&worked), Worktrees::list),
        Some(other)
    );
    // Two files with the name there: left as written.
    trees.at("b", "more/notes.md");
    assert_eq!(locate(&root, "notes.md", &[worked], Worktrees::list), None);
    // No evidence of `b`: never searched.
    assert_eq!(locate(&root, "notes.md", &[], Worktrees::list), None);
}
