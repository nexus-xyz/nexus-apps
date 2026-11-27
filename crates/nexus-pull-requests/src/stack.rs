//! A session's pull requests as stacks. A pull request whose base branch is another's head branch sits on that one,
//! and merges after it. Pull requests with no such link stand alone. The windows draw each stack as one column, its
//! top first, and say what must merge first ([`waits_on`]). Nexus GPUI draws them.

use crate::pull_request::PullRequestStatus;

/// One pull request in a [`Group`], with what the windows need to draw its rail.
#[derive(Debug, Clone, PartialEq)]
pub struct Row<'a> {
    pub pull: &'a PullRequestStatus,
    /// The pull request it sits on; None at the bottom of a stack and for one that stands alone.
    pub on: Option<&'a PullRequestStatus>,
    /// Its place from the bottom, from 1.
    pub level: usize,
}

/// A stack, or a pull request that stands alone (one row).
#[derive(Debug, Clone, PartialEq)]
pub struct Group<'a> {
    /// The branch the bottom pull request merges into, such as `main`; empty when the host did not say.
    pub trunk: &'a str,
    /// Top first: the last to merge first, as `git log` and stack tools list them.
    pub rows: Vec<Row<'a>>,
}

impl Group<'_> {
    pub fn stacked(&self) -> bool {
        self.rows.len() > 1
    }

    /// The highest pull request number in it: the newest group comes first.
    fn newest(&self) -> u64 {
        self.rows.iter().map(|row| row.pull.number).max().unwrap_or_default()
    }
}

/// The pull request `pull` sits on: one of `pulls` whose head branch is its base branch. An open one before a closed
/// one, then the newest, when more than one has that branch.
pub fn parent<'a>(pull: &PullRequestStatus, pulls: &'a [PullRequestStatus]) -> Option<&'a PullRequestStatus> {
    if pull.base_branch.is_empty() {
        return None;
    }
    pulls
        .iter()
        .filter(|other| other.url != pull.url && other.head_branch == pull.base_branch)
        .max_by_key(|other| (other.state == "OPEN", other.number))
}

/// The session's pull requests in groups: each stack top first, the newest group first. A loop of bases (which a host
/// should refuse) breaks at the pull request already placed.
pub fn groups(pulls: &[PullRequestStatus]) -> Vec<Group<'_>> {
    let parents: Vec<Option<usize>> = pulls
        .iter()
        .map(|pull| parent(pull, pulls).and_then(|on| pulls.iter().position(|other| other.url == on.url)))
        .collect();
    let mut placed = vec![false; pulls.len()];
    let mut groups = Vec::new();
    // The bottoms first, then any loop left over, each from its oldest pull request.
    let mut starts: Vec<usize> = (0..pulls.len()).collect();
    starts.sort_by_key(|&ix| (parents[ix].is_some(), pulls[ix].number));
    for start in starts {
        if placed[start] {
            continue;
        }
        // Depth first, oldest child first, so a straight stack reads bottom to top; then turned over.
        let mut order: Vec<(usize, Option<usize>, usize)> = Vec::new();
        let mut next = vec![(start, None, 1)];
        while let Some((ix, on, level)) = next.pop() {
            if placed[ix] {
                continue;
            }
            placed[ix] = true;
            order.push((ix, on, level));
            let mut children: Vec<usize> = (0..pulls.len())
                .filter(|&child| parents[child] == Some(ix) && !placed[child])
                .collect();
            children.sort_by_key(|&child| std::cmp::Reverse(pulls[child].number));
            next.extend(children.into_iter().map(|child| (child, Some(ix), level + 1)));
        }
        order.reverse();
        groups.push(Group {
            trunk: pulls[start].base_branch.as_str(),
            rows: order
                .into_iter()
                .map(|(ix, on, level)| Row {
                    pull: &pulls[ix],
                    on: on.map(|on| &pulls[on]),
                    level,
                })
                .collect(),
        });
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.newest()));
    groups
}

/// The open pull request under `pull` that must merge first; None when it sits on the trunk or on a merged one.
pub fn waits_on<'a>(pull: &PullRequestStatus, pulls: &'a [PullRequestStatus]) -> Option<&'a PullRequestStatus> {
    parent(pull, pulls).filter(|on| on.state == "OPEN" && pull.state == "OPEN")
}

/// Where `pull` merges, for a card: `main`, or `feat/a · #121` when it sits on another. None when the host did not say.
pub fn into_line(pull: &PullRequestStatus, pulls: &[PullRequestStatus]) -> Option<String> {
    if pull.base_branch.is_empty() {
        return None;
    }
    Some(match parent(pull, pulls) {
        Some(on) => format!("{} · {}", pull.base_branch, on.host.reference(on.number)),
        None => pull.base_branch.clone(),
    })
}

/// The branch at the bottom of `pull`'s stack, such as `main`: the base of the lowest pull request under it.
pub fn trunk<'a>(pull: &'a PullRequestStatus, pulls: &'a [PullRequestStatus]) -> &'a str {
    let mut at = pull;
    for _ in 0..pulls.len() {
        match parent(at, pulls) {
            Some(on) => at = on,
            None => break,
        }
    }
    at.base_branch.as_str()
}

/// What a card says about a stacked pull request: what to merge first, or that the one under it ended and its base
/// still names that branch. None when nothing is in the way.
pub fn stack_note(pull: &PullRequestStatus, pulls: &[PullRequestStatus]) -> Option<String> {
    if pull.state != "OPEN" {
        return None;
    }
    let on = parent(pull, pulls)?;
    let reference = on.host.reference(on.number);
    let trunk = trunk(pull, pulls);
    match on.state.as_str() {
        "OPEN" => Some(format!(
            "Merge {reference} first. Merged now, this goes into {}, not {trunk}.",
            pull.base_branch
        )),
        "MERGED" => Some(format!(
            "{reference} merged. Retarget this to {trunk} before it merges."
        )),
        _ => Some(format!(
            "{reference} closed. Retarget this to {trunk} before it merges."
        )),
    }
}

/// A short count for a group of several, such as "Stack of 3".
pub fn group_label(group: &Group) -> String {
    if group.stacked() {
        format!("Stack of {}", group.rows.len())
    } else {
        String::new()
    }
}

/// The session's pull requests in a few words, for a tooltip: "Stack of 3", "3 pull requests", or
/// "3 pull requests · stack of 2". None for one.
pub fn count_line(pulls: &[PullRequestStatus]) -> Option<String> {
    if pulls.len() < 2 {
        return None;
    }
    let word = pulls[0].host.request_title().to_lowercase();
    let groups = groups(pulls);
    let stacks: Vec<usize> = groups
        .iter()
        .filter(|group| group.stacked())
        .map(|group| group.rows.len())
        .collect();
    let all = format!("{} {word}s", pulls.len());
    Some(match stacks.as_slice() {
        [] => all,
        [one] if *one == pulls.len() => format!("Stack of {one}"),
        [one] => format!("{all} · stack of {one}"),
        many => format!("{all} · {} stacks", many.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::source_host::Host;

    fn pull(number: u64, head: &str, base: &str, state: &str) -> PullRequestStatus {
        PullRequestStatus {
            number,
            title: format!("Pull {number}"),
            url: format!("https://github.com/example/repo/pull/{number}"),
            draft: false,
            state: state.into(),
            checks: vec![],
            review_decision: None,
            reviews: vec![],
            mergeable: "MERGEABLE".into(),
            host: Host::GitHub,
            head_branch: head.into(),
            base_branch: base.into(),
            comments: vec![],
        }
    }

    fn numbers(group: &Group) -> Vec<u64> {
        group.rows.iter().map(|row| row.pull.number).collect()
    }

    #[test]
    fn a_straight_stack_reads_top_first_with_its_trunk() {
        // Newest first, as the watch keeps them.
        let pulls = vec![
            pull(12, "c", "b", "OPEN"),
            pull(11, "b", "a", "OPEN"),
            pull(10, "a", "main", "OPEN"),
        ];
        let groups = groups(&pulls);
        assert_eq!(groups.len(), 1);
        assert_eq!(numbers(&groups[0]), vec![12, 11, 10]);
        assert_eq!(groups[0].trunk, "main");
        assert!(groups[0].stacked());
        let levels: Vec<usize> = groups[0].rows.iter().map(|row| row.level).collect();
        assert_eq!(levels, vec![3, 2, 1]);
        assert_eq!(groups[0].rows[0].on.map(|on| on.number), Some(11));
        assert_eq!(groups[0].rows[2].on, None);
        assert_eq!(group_label(&groups[0]), "Stack of 3");
    }

    #[test]
    fn pull_requests_on_other_branches_stand_alone_newest_first() {
        let pulls = vec![
            pull(21, "y", "main", "OPEN"),
            pull(20, "b", "a", "OPEN"),
            pull(19, "x", "main", "MERGED"),
            pull(18, "a", "main", "OPEN"),
        ];
        let groups = groups(&pulls);
        let shape: Vec<Vec<u64>> = groups.iter().map(numbers).collect();
        assert_eq!(shape, vec![vec![21], vec![20, 18], vec![19]]);
        assert!(!groups[0].stacked());
        assert_eq!(group_label(&groups[0]), "");
    }

    #[test]
    fn a_branch_with_two_pull_requests_on_it_keeps_both_in_its_stack() {
        let pulls = vec![
            pull(32, "c", "a", "OPEN"),
            pull(31, "b", "a", "OPEN"),
            pull(30, "a", "main", "OPEN"),
        ];
        let groups = groups(&pulls);
        assert_eq!(groups.len(), 1);
        assert_eq!(numbers(&groups[0]), vec![32, 31, 30]);
        assert!(
            groups[0].rows[..2]
                .iter()
                .all(|row| row.on.map(|on| on.number) == Some(30))
        );
    }

    #[test]
    fn an_older_host_without_branches_lists_each_alone() {
        let pulls = vec![pull(41, "", "", "OPEN"), pull(40, "", "", "OPEN")];
        let groups = groups(&pulls);
        assert_eq!(groups.len(), 2);
        assert_eq!(into_line(&pulls[0], &pulls), None);
    }

    #[test]
    fn a_loop_of_bases_still_lists_each_once() {
        let pulls = vec![pull(51, "b", "a", "OPEN"), pull(50, "a", "b", "OPEN")];
        let groups = groups(&pulls);
        let all: Vec<u64> = groups.iter().flat_map(numbers).collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn says_what_merges_first_and_where_each_goes() {
        let pulls = vec![pull(61, "b", "a", "OPEN"), pull(60, "a", "main", "OPEN")];
        assert_eq!(waits_on(&pulls[0], &pulls).map(|on| on.number), Some(60));
        assert_eq!(waits_on(&pulls[1], &pulls), None);
        assert_eq!(into_line(&pulls[0], &pulls).as_deref(), Some("a · #60"));
        assert_eq!(into_line(&pulls[1], &pulls).as_deref(), Some("main"));
        assert_eq!(
            stack_note(&pulls[0], &pulls).as_deref(),
            Some("Merge #60 first. Merged now, this goes into a, not main.")
        );
        assert_eq!(stack_note(&pulls[1], &pulls), None);
        let merged = vec![pull(61, "b", "a", "OPEN"), pull(60, "a", "main", "MERGED")];
        assert_eq!(waits_on(&merged[0], &merged), None);
        assert_eq!(
            stack_note(&merged[0], &merged).as_deref(),
            Some("#60 merged. Retarget this to main before it merges.")
        );
    }

    #[test]
    fn counts_them_in_a_few_words() {
        let one = vec![pull(1, "a", "main", "OPEN")];
        assert_eq!(count_line(&one), None);
        let stack = vec![pull(3, "b", "a", "OPEN"), pull(2, "a", "main", "OPEN")];
        assert_eq!(count_line(&stack).as_deref(), Some("Stack of 2"));
        let apart = vec![pull(3, "b", "main", "OPEN"), pull(2, "a", "main", "OPEN")];
        assert_eq!(count_line(&apart).as_deref(), Some("2 pull requests"));
        let mixed = vec![
            pull(4, "c", "main", "OPEN"),
            pull(3, "b", "a", "OPEN"),
            pull(2, "a", "main", "OPEN"),
        ];
        assert_eq!(count_line(&mixed).as_deref(), Some("3 pull requests · stack of 2"));
    }

    #[test]
    fn an_open_parent_wins_over_a_closed_one_on_the_same_branch() {
        let pulls = vec![
            pull(72, "b", "a", "OPEN"),
            pull(71, "a", "main", "OPEN"),
            pull(70, "a", "main", "CLOSED"),
        ];
        assert_eq!(parent(&pulls[0], &pulls).map(|on| on.number), Some(71));
    }
}
