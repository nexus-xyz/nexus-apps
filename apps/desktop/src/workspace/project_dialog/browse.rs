//! What Add a project's folder field reads from a typed path: the folder to list, the name being typed, the folders
//! that match it, and what to say about the path. No UI here (`browser.rs` draws it).

use nexus_workbench::dirs::{Dir, Dirs};

/// `typed` as the folder to list and the name being typed in it: `~/code/ne` is `~/code/` and `ne`. A path with no
/// `/` yet is a name in the home folder; `~` alone is the home folder.
pub(super) fn split(typed: &str) -> (String, String) {
    let typed = typed.trim_start();
    if typed == "~" {
        return ("~/".into(), String::new());
    }
    match typed.rfind('/') {
        Some(at) => (typed[..=at].to_string(), typed[at + 1..].to_string()),
        None => ("~/".into(), typed.to_string()),
    }
}

/// The listing's folders that `leaf` starts, in any case: an exact name first, then the listing's order. Hidden
/// folders only once the name being typed starts with a dot.
pub(super) fn matches(listing: &Dirs, leaf: &str) -> Vec<usize> {
    let lower = leaf.to_lowercase();
    let mut found: Vec<usize> = (0..listing.dirs.len())
        .filter(|&index| {
            let name = &listing.dirs[index].name;
            (leaf.starts_with('.') || !name.starts_with('.')) && name.to_lowercase().starts_with(&lower)
        })
        .collect();
    if let Some(exact) = found.iter().position(|&index| listing.dirs[index].name == leaf) {
        let exact = found.remove(exact);
        found.insert(0, exact);
    }
    found
}

/// What the field says under it about the path typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Found {
    /// Nothing to say yet: no answer, or a name still being typed.
    Quiet,
    /// The folder exists. `named`: the person typed its name (a plain folder is worth saying only then).
    Folder {
        branch: Option<String>,
        project: Option<String>,
        named: bool,
    },
    /// The machine refused the listing (no such folder, no access), in its words.
    Refused(String),
    /// No folder in the listing starts with the name typed.
    NoMatch,
}

/// What `listing` (the answer for the folder part of the path) says about `leaf`.
pub(super) fn found(listing: &Result<Dirs, String>, leaf: &str) -> Found {
    let listing = match listing {
        Ok(listing) => listing,
        Err(error) => return Found::Refused(error.clone()),
    };
    if leaf.is_empty() {
        return Found::Folder {
            branch: listing.branch.clone(),
            project: listing.project_id.clone(),
            named: false,
        };
    }
    if let Some(dir) = listing.dirs.iter().find(|dir| dir.name == leaf) {
        return Found::Folder {
            branch: dir.branch.clone(),
            project: dir.project_id.clone(),
            named: true,
        };
    }
    if listing.truncated || !matches(listing, leaf).is_empty() {
        Found::Quiet
    } else {
        Found::NoMatch
    }
}

/// The folder `leaf` names in the listed folder `parent` (absolute), as the host adds it.
pub(super) fn join(parent: &str, leaf: &str) -> String {
    let parent = parent.trim_end_matches('/');
    match leaf.trim_end_matches('/') {
        "" if parent.is_empty() => "/".into(),
        "" => parent.into(),
        leaf => format!("{parent}/{leaf}"),
    }
}

/// The typed path after Tab took `dir`: its folder part, the folder's name, and a `/` so its folders list next.
pub(super) fn complete(typed_dir: &str, dir: &Dir) -> String {
    format!("{typed_dir}{}/", dir.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn listing(names: &[&str]) -> Dirs {
        Dirs {
            path: "/home/ada/code".into(),
            branch: None,
            project_id: None,
            dirs: names
                .iter()
                .map(|name| Dir {
                    name: (*name).into(),
                    branch: (*name == "nexus").then(|| "main".into()),
                    project_id: (*name == "api").then(|| "p-api".into()),
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn a_typed_path_splits_into_its_folder_and_the_name_being_typed() {
        assert_eq!(split("~/code/ne"), ("~/code/".into(), "ne".into()));
        assert_eq!(split("~/code/"), ("~/code/".into(), String::new()));
        assert_eq!(split("/srv"), ("/".into(), "srv".into()));
        assert_eq!(split("~"), ("~/".into(), String::new()));
        assert_eq!(split("code"), ("~/".into(), "code".into()));
    }

    #[test]
    fn names_match_in_any_case_exact_first_and_hidden_only_on_a_dot() {
        let dirs = listing(&["Nexus-old", "nexus", "notes", ".nexus", "api"]);
        let names = |leaf| {
            matches(&dirs, leaf)
                .into_iter()
                .map(|index| dirs.dirs[index].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("nexus"), ["nexus", "Nexus-old"]);
        assert_eq!(names("n"), ["Nexus-old", "nexus", "notes"]);
        assert_eq!(names(".n"), [".nexus"]);
        assert_eq!(names(""), ["Nexus-old", "nexus", "notes", "api"]);
    }

    #[test]
    fn the_path_says_its_branch_its_project_or_that_nothing_matches() {
        let dirs = Ok(listing(&["nexus", "api", "scratch"]));
        assert_eq!(
            found(&dirs, "nexus"),
            Found::Folder {
                branch: Some("main".into()),
                project: None,
                named: true
            }
        );
        assert_eq!(
            found(&dirs, "api"),
            Found::Folder {
                branch: None,
                project: Some("p-api".into()),
                named: true
            }
        );
        assert_eq!(found(&dirs, "ne"), Found::Quiet);
        assert_eq!(found(&dirs, "nexsu"), Found::NoMatch);
        assert_eq!(
            found(&Err("No folder at /x.".into()), "a"),
            Found::Refused("No folder at /x.".into())
        );
        let mut long = listing(&["a"]);
        long.truncated = true;
        assert_eq!(
            found(&Ok(long), "zzz"),
            Found::Quiet,
            "a cut listing cannot say a name is missing"
        );
    }

    #[test]
    fn the_folder_added_is_the_listed_folder_and_the_name() {
        assert_eq!(join("/home/ada/code", "nexus"), "/home/ada/code/nexus");
        assert_eq!(join("/home/ada/code/", ""), "/home/ada/code");
        assert_eq!(join("/", "srv/"), "/srv");
        assert_eq!(join("/", ""), "/");
        let dir = Dir {
            name: "nexus".into(),
            branch: None,
            project_id: None,
        };
        assert_eq!(complete("~/code/", &dir), "~/code/nexus/");
    }
}
