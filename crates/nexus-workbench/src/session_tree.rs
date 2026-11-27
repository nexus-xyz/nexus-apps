//! Linked sessions in sidebar order: a session a lead started (its `parent_thread_id`) sits under that lead, one level
//! deeper per link.
//!
//! The tests are in `session_tree/tests.rs`. A missing parent, an archived one, one in another
//! project or an old host's session with no link leaves the session at the top level. A malformed cycle never hides a
//! session.
//!
//! No I/O and no screen code here: the app passes the ids it lists and draws each row at its depth.

use std::collections::{HashMap, HashSet};

use crate::event::ThreadSummary;

#[cfg(test)]
mod tests;

/// What the rule reads from one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link<'a> {
    /// The session that started it (`ThreadSummary::parent_thread_id`).
    pub parent: Option<&'a str>,
    pub project: &'a str,
    pub archived: bool,
}

impl<'a> Link<'a> {
    pub fn of(thread: &'a ThreadSummary) -> Link<'a> {
        Link {
            parent: thread.parent_thread_id.as_deref(),
            project: &thread.project_id,
            archived: thread.archived,
        }
    }
}

/// One row of the tree: a session id and how deep it sits (0 at the top level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRow<'a> {
    pub id: &'a str,
    pub depth: usize,
}

/// The highest parent present in the same project and not archived (`sessionRoot`). A cycle returns `id` itself.
pub fn session_root<'a>(id: &'a str, links: &HashMap<&'a str, Link<'a>>) -> &'a str {
    let project = links.get(id).map(|link| link.project);
    let mut seen = HashSet::new();
    let mut root = id;
    while seen.insert(root) {
        let Some(parent) = links.get(root).and_then(|link| link.parent) else {
            return root;
        };
        match links.get(parent) {
            Some(link) if !link.archived && Some(link.project) == project => root = parent,
            _ => return root,
        }
    }
    id
}

/// `ids` as a tree (`sessionTree`): parents precede their children; roots and siblings keep the input order. A session
/// whose parent is not in `ids`, is itself or is in another project stays at the top level. Each id appears once, even
/// in a cycle.
pub fn session_tree<'a>(ids: &[&'a str], links: &HashMap<&'a str, Link<'a>>) -> Vec<TreeRow<'a>> {
    let present: HashSet<&str> = ids.iter().copied().collect();
    let mut children: HashMap<&str, Vec<&'a str>> = HashMap::new();
    let mut roots = Vec::new();
    for &id in ids {
        let link = links.get(id);
        let parent = link.and_then(|link| link.parent).filter(|parent| {
            *parent != id
                && present.contains(parent)
                && links.get(parent).map(|parent| parent.project) == link.map(|link| link.project)
        });
        match parent {
            Some(parent) => children.entry(parent).or_default().push(id),
            None => roots.push(id),
        }
    }
    let mut rows = Vec::with_capacity(ids.len());
    let mut visited = HashSet::new();
    let mut visit = |first: &'a str| {
        let mut stack = vec![TreeRow { id: first, depth: 0 }];
        while let Some(row) = stack.pop() {
            if !visited.insert(row.id) {
                continue;
            }
            rows.push(row);
            for &child in children.get(row.id).into_iter().flatten().rev() {
                stack.push(TreeRow {
                    id: child,
                    depth: row.depth + 1,
                });
            }
        }
    };
    roots.iter().for_each(|&id| visit(id));
    // An old or damaged database can hold a cycle with no root.
    ids.iter().for_each(|&id| visit(id));
    rows
}
