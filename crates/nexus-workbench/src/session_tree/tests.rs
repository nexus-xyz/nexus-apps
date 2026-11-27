//! Linked sessions in sidebar order.

use std::collections::{HashMap, HashSet};

use super::{Link, TreeRow, session_root, session_tree};

fn link<'a>(parent: Option<&'a str>, project: &'a str) -> Link<'a> {
    Link {
        parent,
        project,
        archived: false,
    }
}

fn rows<'a>(list: &[(&'a str, usize)]) -> Vec<TreeRow<'a>> {
    list.iter().map(|&(id, depth)| TreeRow { id, depth }).collect()
}

#[test]
fn keeps_parents_before_children_with_each_child_at_its_own_depth() {
    let links = HashMap::from([
        ("root", link(None, "p")),
        ("a", link(Some("root"), "p")),
        ("b", link(Some("root"), "p")),
        ("grandchild", link(Some("a"), "p")),
        ("other", link(None, "p")),
    ]);
    let ids = ["grandchild", "b", "other", "a", "root"];
    assert_eq!(
        session_tree(&ids, &links),
        rows(&[("other", 0), ("root", 0), ("b", 1), ("a", 1), ("grandchild", 2)])
    );
    assert_eq!(session_root("grandchild", &links), "root");
}

#[test]
fn never_drops_an_orphan_a_cross_project_link_a_self_link_or_a_cycle() {
    let links = HashMap::from([
        ("a", link(Some("b"), "p")),
        ("b", link(Some("a"), "p")),
        ("c", link(Some("gone"), "p")),
        ("d", link(Some("d"), "p")),
        ("e", link(Some("c"), "other")),
    ]);
    let ids = ["a", "b", "c", "d", "e"];
    let tree = session_tree(&ids, &links);
    assert_eq!(tree.len(), 5);
    assert_eq!(
        tree.iter().map(|row| row.id).collect::<HashSet<_>>(),
        ids.into_iter().collect::<HashSet<_>>()
    );
    assert_eq!(tree.iter().find(|row| row.id == "e").map(|row| row.depth), Some(0));
    assert_eq!(session_root("a", &links), "a");
}

#[test]
fn a_child_whose_parent_is_not_listed_stays_at_the_top() {
    // A search or the archive filter can leave the parent out: the matching child shows at depth 0.
    let links = HashMap::from([
        ("root", link(None, "p")),
        ("a", link(Some("root"), "p")),
        ("grandchild", link(Some("a"), "p")),
    ]);
    assert_eq!(session_tree(&["grandchild"], &links), rows(&[("grandchild", 0)]));
}

#[test]
fn an_archived_or_other_project_parent_is_not_a_root() {
    let mut links = HashMap::from([("lead", link(None, "p")), ("child", link(Some("lead"), "p"))]);
    assert_eq!(session_root("child", &links), "lead");
    links.insert(
        "lead",
        Link {
            archived: true,
            ..link(None, "p")
        },
    );
    assert_eq!(session_root("child", &links), "child");
    links.insert("lead", link(None, "q"));
    assert_eq!(session_root("child", &links), "child");
}
