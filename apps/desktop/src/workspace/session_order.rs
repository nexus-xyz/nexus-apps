//! The order of a project's sessions in the sidebar once the person dragged them. The store lists them
//! pinned first, then by their latest activity. A drag keeps the project's whole order (`project_groups.rs`). A session
//! the order does not name yet (a new one, or one back from Settled) shows on top, as it would with no order. Pinned
//! sessions stay above the others: a drag moves a session only among the ones pinned as it is.

use crate::code::CodeSession;

/// `sessions`, in the store's order, arranged by `order`: pinned first; in each part, the ones `order` does not name
/// first, as they came, then the named ones in `order`'s order.
pub(super) fn arrange<'a>(mut sessions: Vec<&'a CodeSession>, order: &[String]) -> Vec<&'a CodeSession> {
    if order.is_empty() {
        return sessions;
    }
    let rank = |session: &CodeSession| order.iter().position(|id| *id == session.id).map_or(0, |at| at + 1);
    // Stable: what `order` does not name keeps the store's order.
    sessions.sort_by_key(|session| (!session.pinned, rank(session)));
    sessions
}

/// The project's order after the `dragged` sessions were dropped on `target`, before it or `after` it. Several land
/// together at the spot, in `dragged`'s order. `listed` is the order on screen; a session not in it (one
/// back from a group put aside, or from the archive) joins it there. None when nothing moves: a target not listed, a
/// drop on one of them, or one where they already are.
pub(super) fn moved_all(listed: &[&str], dragged: &[&str], target: &str, after: bool) -> Option<Vec<String>> {
    if dragged.is_empty() || dragged.contains(&target) || !listed.contains(&target) {
        return None;
    }
    let mut order: Vec<&str> = listed.to_vec();
    order.retain(|id| !dragged.contains(id));
    let to = order.iter().position(|id| *id == target)? + usize::from(after);
    order.splice(to..to, dragged.iter().copied());
    (order != listed).then(|| order.into_iter().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One session dropped on `target`.
    fn moved(listed: &[&str], dragged: &str, target: &str, after: bool) -> Option<Vec<String>> {
        moved_all(listed, &[dragged], target, after)
    }

    fn session(id: &str, pinned: bool) -> CodeSession {
        let mut session = crate::workspace::session_keys::tests::session(id);
        session.pinned = pinned;
        session
    }

    fn ids(sessions: &[&CodeSession]) -> Vec<String> {
        sessions.iter().map(|session| session.id.clone()).collect()
    }

    #[test]
    fn with_no_order_the_store_order_stands() {
        let (a, b) = (session("a", false), session("b", false));
        assert_eq!(ids(&arrange(vec![&a, &b], &[])), ["a", "b"]);
    }

    #[test]
    fn a_dragged_order_holds_and_a_new_session_shows_on_top() {
        let (new, a, b, c) = (
            session("new", false),
            session("a", false),
            session("b", false),
            session("c", false),
        );
        let order = ["c".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(ids(&arrange(vec![&new, &a, &b, &c], &order)), ["new", "c", "a", "b"]);
    }

    #[test]
    fn pinned_sessions_stay_above_the_others() {
        let (pinned, a, b) = (session("p", true), session("a", false), session("b", false));
        let order = ["b".to_string(), "a".to_string(), "p".to_string()];
        assert_eq!(ids(&arrange(vec![&pinned, &a, &b], &order)), ["p", "b", "a"]);
    }

    #[test]
    fn a_session_dropped_on_another_lands_before_or_after_it() {
        let listed = ["a", "b", "c", "d"];
        assert_eq!(moved(&listed, "a", "c", true).unwrap(), ["b", "c", "a", "d"]);
        assert_eq!(moved(&listed, "a", "c", false).unwrap(), ["b", "a", "c", "d"]);
        assert_eq!(moved(&listed, "d", "a", false).unwrap(), ["d", "a", "b", "c"]);
        assert_eq!(moved(&listed, "d", "b", true).unwrap(), ["a", "b", "d", "c"]);
    }

    #[test]
    fn a_drop_that_moves_nothing_keeps_no_order() {
        let listed = ["a", "b", "c"];
        assert_eq!(moved(&listed, "b", "b", true), None, "on itself");
        assert_eq!(moved(&listed, "a", "b", false), None, "where it already is");
        assert_eq!(moved(&listed, "b", "a", true), None, "where it already is");
        assert_eq!(moved(&listed, "a", "x", true), None, "a target not in the list");
    }

    #[test]
    fn sessions_dragged_together_land_together_in_their_order() {
        let listed = ["a", "b", "c", "d"];
        assert_eq!(
            moved_all(&listed, &["a", "c"], "d", true).unwrap(),
            ["b", "d", "a", "c"]
        );
        assert_eq!(
            moved_all(&listed, &["d", "x"], "a", false).unwrap(),
            ["d", "x", "a", "b", "c"]
        );
        assert_eq!(
            moved_all(&listed, &["a", "b"], "c", false),
            None,
            "where they are already"
        );
        assert_eq!(moved_all(&listed, &["a", "c"], "c", true), None, "on one of them");
    }

    #[test]
    fn a_session_from_outside_the_list_joins_it_where_it_lands() {
        let listed = ["a", "b", "c"];
        assert_eq!(moved(&listed, "x", "b", false).unwrap(), ["a", "x", "b", "c"]);
        assert_eq!(moved(&listed, "x", "c", true).unwrap(), ["a", "b", "c", "x"]);
    }
}
