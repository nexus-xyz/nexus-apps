//! The sidebar's one list of projects. The pinned Code projects come first. Then every project, Code or
//! plain, by when it was added, the newest first. What agents do never moves a project. Plain projects have no pin.
//!
//! Once the person drags a project, the list keeps their order (`project_groups.rs`), by each project's [`Entry::key`].
//! Pinned projects stay above the others, and a project the order does not name yet shows on top of its part.

use super::repo_groups::RepoGroup;
use crate::project_groups::PlainProject;

/// One row of the list: a repository's Code projects, or a plain project.
pub(super) enum Entry<'a, T> {
    Code(RepoGroup<'a, T>),
    Plain(&'a PlainProject),
}

impl<T> Entry<'_, T> {
    /// The name the saved order keeps the project by. A repository keeps its key wherever its copies run, so it
    /// keeps its place when its lead changes; a folder with no remote keeps its Code project's id.
    pub fn key(&self) -> String {
        match self {
            Entry::Code(repo) => match super::repo_groups::repo_key(&repo.lead.remotes) {
                Some(key) => format!("repo:{key}"),
                None => format!("code:{}", repo.lead.id),
            },
            Entry::Plain(project) => format!("plain:{}", project.id),
        }
    }

    /// Whether the project is pinned: a repository is when one of its copies is. A plain project has no pin.
    pub fn pinned(&self) -> bool {
        match self {
            Entry::Code(repo) => repo.members.iter().any(|project| project.pinned),
            Entry::Plain(_) => false,
        }
    }
}

/// `entries`, in `merge`'s order, arranged by `order`: pinned first; in each part, the ones `order` does not name
/// first, as they came, then the named ones in `order`'s order.
pub(super) fn arrange<'a, T>(entries: Vec<Entry<'a, T>>, order: &[String]) -> Vec<Entry<'a, T>> {
    if order.is_empty() {
        return entries;
    }
    let mut keyed: Vec<((bool, usize), Entry<'a, T>)> = entries
        .into_iter()
        .map(|entry| {
            let key = entry.key();
            let rank = order.iter().position(|one| *one == key).map_or(0, |at| at + 1);
            ((!entry.pinned(), rank), entry)
        })
        .collect();
    // Stable: what `order` does not name keeps `merge`'s order.
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

/// Where a repository sorts: pinned when one of its copies is, and when its newest copy of that kind was added.
fn code_key<T>(repo: &RepoGroup<'_, T>) -> (bool, i64) {
    let pinned = repo.members.iter().any(|project| project.pinned);
    let added = repo
        .members
        .iter()
        .filter(|project| project.pinned == pinned)
        .map(|project| project.added_at)
        .max()
        .unwrap_or(0);
    (pinned, added)
}

/// `repos` (in `code::project_order`, as `repo_groups::group` keeps it) and `plain` in one list. On a tie, a Code
/// project comes first, and two plain projects keep their ids' order.
pub(super) fn merge<'a, T>(repos: Vec<RepoGroup<'a, T>>, plain: &'a [PlainProject]) -> Vec<Entry<'a, T>> {
    let mut plain: Vec<&PlainProject> = plain.iter().collect();
    plain.sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then_with(|| a.id.cmp(&b.id)));
    let mut keyed: Vec<((bool, i64), Entry<'a, T>)> = repos
        .into_iter()
        .map(|repo| (code_key(&repo), Entry::Code(repo)))
        .chain(
            plain
                .into_iter()
                .map(|project| ((false, project.created_ms), Entry::Plain(project))),
        )
        .collect();
    // A stable sort: the Code projects keep their order among themselves, and come before plain ones on a tie.
    keyed.sort_by(|(a, _), (b, _)| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::CodeProject;

    fn code(id: &str, added_at: i64, pinned: bool) -> CodeProject {
        CodeProject {
            id: id.into(),
            name: id.into(),
            branch: None,
            pinned,
            remotes: Vec::new(),
            sessions: Vec::new(),
            machine: None,
            added_at,
        }
    }

    fn plain(id: &str, created_ms: i64) -> PlainProject {
        PlainProject {
            id: id.into(),
            name: id.into(),
            created_ms,
        }
    }

    fn ids(entries: &[Entry<'_, ()>]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Code(repo) => repo.lead.id.clone(),
                Entry::Plain(project) => project.id.clone(),
            })
            .collect()
    }

    #[test]
    fn pinned_code_projects_first_then_every_project_newest_added_first() {
        let mut projects = vec![
            code("old", 10, false),
            code("pinned-old", 1, true),
            code("new", 40, false),
            code("pinned-new", 5, true),
        ];
        projects.sort_by(crate::code::project_order);
        let plains = [
            plain("plain-mid", 30),
            plain("plain-oldest", 2),
            plain("plain-newest", 50),
        ];
        let merged = merge(super::super::repo_groups::group_projects(&projects), &plains);
        assert_eq!(
            ids(&merged),
            [
                "pinned-new",
                "pinned-old",
                "plain-newest",
                "new",
                "plain-mid",
                "old",
                "plain-oldest"
            ]
        );
    }

    #[test]
    fn a_code_project_comes_before_a_plain_one_added_at_the_same_time() {
        let projects = [code("code", 7, false)];
        let plains = [plain("b", 7), plain("a", 7)];
        let merged = merge(super::super::repo_groups::group_projects(&projects), &plains);
        assert_eq!(ids(&merged), ["code", "a", "b"]);
    }

    #[test]
    fn with_no_plain_projects_the_code_order_stays() {
        let mut projects = vec![code("a", 1, false), code("b", 2, false), code("c", 3, true)];
        projects.sort_by(crate::code::project_order);
        let merged = merge(super::super::repo_groups::group_projects(&projects), &[]);
        assert_eq!(ids(&merged), ["c", "b", "a"]);
    }

    fn arranged(projects: &[CodeProject], plains: &[PlainProject], order: &[&str]) -> Vec<String> {
        let mut projects = projects.to_vec();
        projects.sort_by(crate::code::project_order);
        let order: Vec<String> = order.iter().map(|key| key.to_string()).collect();
        let merged = merge(super::super::repo_groups::group_projects(&projects), plains);
        ids(&arrange(merged, &order))
    }

    #[test]
    fn a_dragged_order_holds_and_a_new_project_shows_on_top() {
        let projects = [code("a", 1, false), code("b", 2, false)];
        let plains = [plain("p", 3), plain("new", 9)];
        assert_eq!(
            arranged(&projects, &plains, &["code:a", "plain:p", "code:b"]),
            ["new", "a", "p", "b"]
        );
    }

    #[test]
    fn pinned_projects_stay_above_a_dragged_order() {
        let projects = [code("pinned", 1, true), code("a", 2, false)];
        let plains = [plain("p", 3)];
        assert_eq!(
            arranged(&projects, &plains, &["plain:p", "code:a", "code:pinned"]),
            ["pinned", "p", "a"]
        );
    }

    #[test]
    fn a_repository_keeps_its_place_by_its_remote_whatever_its_lead() {
        let mut here = code("here", 1, false);
        here.remotes = vec!["git@github.com:nexus-xyz/nexus.git".into()];
        let projects = [here, code("other", 2, false)];
        let merged = merge(super::super::repo_groups::group_projects(&projects), &[]);
        let keys: Vec<String> = merged.iter().map(Entry::key).collect();
        assert_eq!(keys, ["code:other", "repo:github.com/nexus-xyz/nexus"]);
        assert_eq!(
            arranged(&projects, &[], &["repo:github.com/nexus-xyz/nexus", "code:other"]),
            ["here", "other"]
        );
    }

    #[test]
    fn with_no_order_the_merged_order_stands() {
        let projects = [code("a", 1, false), code("b", 2, false)];
        assert_eq!(arranged(&projects, &[], &[]), ["b", "a"]);
    }
}
