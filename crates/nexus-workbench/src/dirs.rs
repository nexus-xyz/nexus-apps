//! The folders inside one folder of the machine (`wb_dirs`), for a project's folder field in Add a project: the window
//! lists them under the field as the person types a path, with each Git repository's branch and each folder that is
//! already a project. The same rule answers for this computer (the app calls [`list`]) and through a Nexus host.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::Project;

/// What a core that lists folders (`wb_dirs`) says in its features. An older host lacks it, and the window keeps the
/// plain path field.
pub const FEATURE: &str = "project-dirs";

/// The most folders one answer lists (`truncated` past it): a home folder with thousands of entries stays one small
/// answer, and the window narrows the list as the person types.
pub const LIMIT: usize = 200;

/// The folders in [`Dirs::path`], visible ones first, each in name order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dirs {
    /// The folder listed, absolute, with `~` resolved to the machine's home folder.
    pub path: String,
    /// The listed folder's own Git branch, when it is a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The project the listed folder already is, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub dirs: Vec<Dir>,
    /// More than [`LIMIT`] folders: the rest are not listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// One folder of a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dir {
    pub name: String,
    /// Its Git branch when it is a repository: `HEAD`'s branch, or the short commit when detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The project it already is, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// `path` on a machine whose home folder is `home`: `~` and `~/…` start there, and an empty path is the home folder.
/// A relative path is refused, as the clone's destination is (`clone::destination`).
pub fn resolve(path: &str, home: &Path) -> Result<PathBuf, String> {
    let path = path.trim();
    let resolved = match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with(['/', '\\']) => home.join(&rest[1..]),
        _ if path.is_empty() => home.to_path_buf(),
        _ => PathBuf::from(path),
    };
    if resolved.is_absolute() {
        Ok(resolved)
    } else {
        Err("Type the folder's full path, or start it with ~/.".into())
    }
}

/// The folders in `path` (resolved by [`resolve`]), marked with their branch and with the `projects` they already are.
pub fn list(path: &str, home: &Path, projects: &[Project]) -> Result<Dirs, String> {
    let folder = resolve(path, home)?;
    let entries = std::fs::read_dir(&folder).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("No folder at {}.", folder.display()),
        std::io::ErrorKind::PermissionDenied => format!("Nexus cannot read {}.", folder.display()),
        _ if folder.is_file() => format!("{} is a file, not a folder.", folder.display()),
        _ => format!("{}: {error}", folder.display()),
    })?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        // A link to a folder counts: `metadata` follows it.
        .filter(|entry| std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort_by_key(|name| order(name));
    let truncated = names.len() > LIMIT;
    names.truncate(LIMIT);
    let project_of = |dir: &Path| {
        projects
            .iter()
            .find(|project| same_path(Path::new(&project.path), dir))
            .map(|project| project.id.clone())
    };
    let dirs = names
        .into_iter()
        .map(|name| {
            let full = folder.join(&name);
            Dir {
                branch: branch(&full),
                project_id: project_of(&full),
                name,
            }
        })
        .collect();
    Ok(Dirs {
        path: folder.to_string_lossy().into_owned(),
        branch: branch(&folder),
        project_id: project_of(&folder),
        dirs,
        truncated,
    })
}

/// Visible folders before hidden ones, each in name order regardless of case.
fn order(name: &str) -> (bool, String, String) {
    (name.starts_with('.'), name.to_lowercase(), name.to_string())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let trim = |path: &Path| path.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    trim(a) == trim(b)
}

/// The branch `HEAD` names in the repository at `dir`, read from its files (no `git` process, so a listing of 200
/// folders stays quick). A worktree's `.git` file points at its own `HEAD`. Detached: the first 7 characters of its
/// commit. None outside a repository.
pub fn branch(dir: &Path) -> Option<String> {
    let dot_git = dir.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = PathBuf::from(target);
        if target.is_absolute() { target } else { dir.join(target) }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(reference) => Some(reference.trim().trim_start_matches("refs/heads/").to_string()),
        None => Some(head.chars().take(7).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nexus-dirs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn project(id: &str, path: &Path) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": path.to_string_lossy(), "branch": null, "addedAt": 0
        }))
        .expect("a project")
    }

    #[test]
    fn a_home_relative_path_resolves_and_a_relative_one_is_refused() {
        let home = Path::new("/home/ada");
        assert_eq!(resolve("~", home).expect("~"), PathBuf::from("/home/ada"));
        assert_eq!(resolve("", home).expect("empty"), PathBuf::from("/home/ada"));
        assert_eq!(
            resolve("~/code/", home).expect("~/code/"),
            PathBuf::from("/home/ada/code/")
        );
        assert_eq!(resolve("/srv", home).expect("/srv"), PathBuf::from("/srv"));
        assert!(resolve("code", home).is_err());
    }

    #[test]
    fn folders_list_visible_first_with_their_branch_and_project() {
        let root = temp("list");
        for dir in ["beta", "Alpha", ".hidden", "repo/.git"] {
            std::fs::create_dir_all(root.join(dir)).expect("folder");
        }
        std::fs::write(root.join("repo/.git/HEAD"), "ref: refs/heads/feat/cart\n").expect("HEAD");
        std::fs::write(root.join("file.txt"), "not a folder").expect("file");
        let projects = [project("p1", &root.join("beta"))];
        let dirs = list(&root.to_string_lossy(), Path::new("/"), &projects).expect("listing");
        let names: Vec<_> = dirs.dirs.iter().map(|dir| dir.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "repo", ".hidden"]);
        assert_eq!(dirs.dirs[1].project_id.as_deref(), Some("p1"));
        assert_eq!(dirs.dirs[2].branch.as_deref(), Some("feat/cart"));
        assert!(dirs.branch.is_none() && dirs.project_id.is_none() && !dirs.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_and_a_detached_head_name_their_branch() {
        let root = temp("worktree");
        std::fs::create_dir_all(root.join("main/.git/worktrees/wt")).expect("git dir");
        std::fs::create_dir_all(root.join("wt")).expect("worktree");
        std::fs::write(root.join("main/.git/HEAD"), "0123456789abcdef\n").expect("HEAD");
        std::fs::write(root.join("main/.git/worktrees/wt/HEAD"), "ref: refs/heads/side\n").expect("worktree HEAD");
        std::fs::write(root.join("wt/.git"), "gitdir: ../main/.git/worktrees/wt\n").expect(".git");
        assert_eq!(branch(&root.join("main")).as_deref(), Some("0123456"));
        assert_eq!(branch(&root.join("wt")).as_deref(), Some("side"));
        assert_eq!(branch(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_folder_says_so_and_a_long_one_is_cut() {
        let root = temp("limit");
        let missing = list(&root.join("nope").to_string_lossy(), Path::new("/"), &[]).expect_err("missing");
        assert!(missing.starts_with("No folder at "), "{missing}");
        for index in 0..=LIMIT {
            std::fs::create_dir(root.join(format!("d{index:03}"))).expect("folder");
        }
        let dirs = list(&root.to_string_lossy(), Path::new("/"), &[]).expect("listing");
        assert_eq!(dirs.dirs.len(), LIMIT);
        assert!(dirs.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }
}
