//! What the top bar's pull request chip and its card say: the chip's word after the number, the checks in one line,
//! the review and merge lines, and which of a session's pull requests the form pushes to. Nexus GPUI says them
//! from here.

use crate::pull_request::PullRequestStatus;
use crate::watch::PullWatch;

/// How a chip's word reads: `Failed` (a failed check or a conflict) and `Changes` (a review asked for changes) ask the
/// person to act, each in its own hue; `Done` is merged or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Failed,
    Changes,
    Quiet,
    Done,
}

/// The checks by state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub passed: usize,
    pub failed: usize,
    pub running: usize,
    pub skipped: usize,
}

pub fn tally(pull: &PullRequestStatus) -> Tally {
    let mut tally = Tally::default();
    for check in &pull.checks {
        match check.state.as_str() {
            "success" => tally.passed += 1,
            "failure" => tally.failed += 1,
            "pending" => tally.running += 1,
            _ => tally.skipped += 1,
        }
    }
    tally
}

/// The checks in one line: `1 failed · 2 running · 3 passed`; `No checks` when none reported.
pub fn checks_line(tally: Tally) -> String {
    let parts: Vec<String> = [
        (tally.failed, "failed"),
        (tally.running, "running"),
        (tally.passed, "passed"),
        (tally.skipped, "skipped"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, word)| format!("{count} {word}"))
    .collect();
    if parts.is_empty() {
        "No checks".into()
    } else {
        parts.join(" · ")
    }
}

fn changes_asked(pull: &PullRequestStatus) -> bool {
    pull.review_decision.as_deref() == Some("CHANGES_REQUESTED")
        || pull.reviews.iter().any(|review| review.state == "CHANGES_REQUESTED")
}

/// The chip's word after the number: what most needs the person first (a failed check, asked changes, a conflict),
/// then what is still moving (checks running), then where it stands (draft, approved, open).
pub fn word(pull: &PullRequestStatus) -> (String, Tone) {
    if pull.state == "MERGED" {
        return ("Merged".into(), Tone::Done);
    }
    if pull.state != "OPEN" {
        return ("Closed".into(), Tone::Done);
    }
    let tally = tally(pull);
    if tally.failed == 1 {
        return ("1 check failed".into(), Tone::Failed);
    }
    if tally.failed > 1 {
        return (format!("{} checks failed", tally.failed), Tone::Failed);
    }
    if changes_asked(pull) {
        return ("Changes requested".into(), Tone::Changes);
    }
    if pull.mergeable == "CONFLICTING" {
        return ("Conflicts".into(), Tone::Failed);
    }
    if tally.running > 0 {
        return ("Checks running".into(), Tone::Quiet);
    }
    if pull.draft {
        return ("Draft".into(), Tone::Quiet);
    }
    if pull.review_decision.as_deref() == Some("APPROVED") {
        return ("Approved".into(), Tone::Quiet);
    }
    ("Open".into(), Tone::Quiet)
}

/// The card's review line: who asked for changes, an approval, or what the repository still waits for.
pub fn review_line(pull: &PullRequestStatus) -> String {
    let by = |state: &str| -> Vec<&str> {
        pull.reviews
            .iter()
            .filter(|review| review.state == state)
            .map(|review| review.author.as_str())
            .collect()
    };
    let asked = by("CHANGES_REQUESTED");
    if !asked.is_empty() {
        return format!("Changes requested by {}", asked.join(", "));
    }
    if pull.review_decision.as_deref() == Some("CHANGES_REQUESTED") {
        return "Changes requested".into();
    }
    let approved = by("APPROVED");
    if !approved.is_empty() {
        return format!("Approved by {}", approved.join(", "));
    }
    match pull.review_decision.as_deref() {
        Some("APPROVED") => "Approved".into(),
        Some("REVIEW_REQUIRED") => "Waiting for review".into(),
        _ => "No review required".into(),
    }
}

/// The card's merge line, for an open pull request; None once it is merged or closed.
pub fn merge_line(pull: &PullRequestStatus) -> Option<&'static str> {
    if pull.state != "OPEN" {
        return None;
    }
    Some(match pull.mergeable.as_str() {
        "CONFLICTING" => "Has conflicts with its base",
        "MERGEABLE" => "No conflicts",
        _ => "Checking for conflicts…",
    })
}

/// The session's pull requests, newest first: one session may open several. Its shown one alone from an older core.
pub fn session_pulls(watch: &PullWatch) -> Vec<PullRequestStatus> {
    if !watch.pulls.is_empty() {
        return watch.pulls.clone();
    }
    watch.status.clone().into_iter().collect()
}

/// Whether the branch has an open pull request, so the form pushes to it rather than opening another: the session's
/// latest (the branch's, as the core saves it), while open. Unread yet: open. None saved: the watch's, while open.
/// `latest` is the worktree's `pull_request`.
pub fn branch_pull_open(latest: Option<&str>, watch: Option<&PullWatch>) -> bool {
    if let Some(latest) = latest {
        let pulls = watch.map(session_pulls).unwrap_or_default();
        return pulls
            .iter()
            .find(|pull| pull.url == latest)
            .is_none_or(|pull| pull.state == "OPEN");
    }
    watch
        .and_then(|watch| watch.status.as_ref())
        .is_some_and(|status| status.state == "OPEN")
}

/// The number at the end of a pull request's or a merge request's link; None when it has none.
pub fn number_of_url(url: &str) -> Option<u64> {
    let path = url.trim().split(['?', '#']).next()?.trim_end_matches('/');
    let (head, number) = path.rsplit_once('/')?;
    (head.ends_with("/pull") || head.ends_with("/-/merge_requests"))
        .then(|| number.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{Check, Review};

    fn pull(change: impl FnOnce(&mut PullRequestStatus)) -> PullRequestStatus {
        let mut pull = PullRequestStatus {
            number: 42,
            title: "Sample".into(),
            url: "https://github.com/example/repo/pull/42".into(),
            draft: false,
            state: "OPEN".into(),
            checks: vec![check("success")],
            review_decision: None,
            reviews: vec![],
            mergeable: "MERGEABLE".into(),
            host: Default::default(),
            head_branch: String::new(),
            base_branch: String::new(),
            comments: vec![],
        };
        change(&mut pull);
        pull
    }

    fn check(state: &str) -> Check {
        Check {
            name: state.into(),
            state: state.into(),
            url: None,
        }
    }

    fn review(author: &str, state: &str) -> Review {
        Review {
            id: author.into(),
            author: author.into(),
            state: state.into(),
            body: String::new(),
            submitted_at: None,
        }
    }

    #[test]
    fn the_word_puts_what_needs_the_person_first() {
        let word = |change: fn(&mut PullRequestStatus)| word(&pull(change));
        assert_eq!(
            word(|p| {
                p.checks = vec![check("failure"), check("pending")];
                p.reviews = vec![review("alice", "CHANGES_REQUESTED")];
            }),
            ("1 check failed".into(), Tone::Failed)
        );
        assert_eq!(
            word(|p| p.checks = vec![check("failure"), check("failure")]).0,
            "2 checks failed"
        );
        assert_eq!(
            word(|p| {
                p.checks = vec![check("pending")];
                p.reviews = vec![review("alice", "CHANGES_REQUESTED")];
            }),
            ("Changes requested".into(), Tone::Changes)
        );
        assert_eq!(
            word(|p| {
                p.checks = vec![check("pending")];
                p.mergeable = "CONFLICTING".into();
            }),
            ("Conflicts".into(), Tone::Failed)
        );
    }

    #[test]
    fn then_what_is_moving_then_where_it_stands() {
        let word = |change: fn(&mut PullRequestStatus)| word(&pull(change)).0;
        assert_eq!(
            word(|p| {
                p.checks = vec![check("pending")];
                p.draft = true;
            }),
            "Checks running"
        );
        assert_eq!(word(|p| p.draft = true), "Draft");
        assert_eq!(word(|p| p.review_decision = Some("APPROVED".into())), "Approved");
        assert_eq!(word(|_| {}), "Open");
        assert_eq!(
            super::word(&pull(|p| p.state = "MERGED".into())),
            ("Merged".into(), Tone::Done)
        );
        assert_eq!(super::word(&pull(|p| p.state = "CLOSED".into())).0, "Closed");
    }

    #[test]
    fn the_checks_line_says_the_failures_first() {
        let all = pull(|p| {
            p.checks = vec![
                check("success"),
                check("success"),
                check("failure"),
                check("pending"),
                check("skipped"),
            ]
        });
        assert_eq!(checks_line(tally(&all)), "1 failed · 1 running · 2 passed · 1 skipped");
        assert_eq!(checks_line(tally(&pull(|p| p.checks.clear()))), "No checks");
    }

    #[test]
    fn the_review_and_merge_lines() {
        assert_eq!(
            review_line(&pull(|p| p.reviews = vec![review("alice", "CHANGES_REQUESTED")])),
            "Changes requested by alice"
        );
        assert_eq!(
            review_line(&pull(|p| p.reviews = vec![review("bob", "APPROVED")])),
            "Approved by bob"
        );
        assert_eq!(
            review_line(&pull(|p| p.review_decision = Some("REVIEW_REQUIRED".into()))),
            "Waiting for review"
        );
        assert_eq!(review_line(&pull(|_| {})), "No review required");
        assert_eq!(
            merge_line(&pull(|p| p.mergeable = "CONFLICTING".into())),
            Some("Has conflicts with its base")
        );
        assert_eq!(
            merge_line(&pull(|p| p.mergeable = "UNKNOWN".into())),
            Some("Checking for conflicts…")
        );
        assert_eq!(merge_line(&pull(|p| p.state = "MERGED".into())), None);
    }

    #[test]
    fn a_session_with_several_pull_requests() {
        let first = pull(|p| {
            p.number = 41;
            p.url = "https://github.com/example/repo/pull/41".into();
            p.state = "MERGED".into();
        });
        let second = pull(|_| {});
        let both = PullWatch {
            status: Some(second.clone()),
            pulls: vec![second.clone(), first.clone()],
            ..PullWatch::default()
        };
        assert_eq!(session_pulls(&both).len(), 2);
        let older = PullWatch {
            status: Some(second.clone()),
            ..PullWatch::default()
        };
        assert_eq!(
            session_pulls(&older),
            vec![second.clone()],
            "an older core: the one it shows"
        );
        assert!(branch_pull_open(Some(&second.url), Some(&both)));
        assert!(
            !branch_pull_open(Some(&first.url), Some(&both)),
            "merged: the form opens another"
        );
        assert!(
            branch_pull_open(Some("https://github.com/example/repo/pull/50"), Some(&both)),
            "not read yet: open"
        );
        assert!(
            branch_pull_open(None, Some(&both)),
            "none saved: the watch's, while open"
        );
        assert!(!branch_pull_open(None, None));
    }

    #[test]
    fn the_number_of_a_link() {
        assert_eq!(number_of_url("https://github.com/acme/web/pull/42"), Some(42));
        assert_eq!(
            number_of_url("https://gitlab.com/group/sub/app/-/merge_requests/7"),
            Some(7)
        );
        assert_eq!(number_of_url("https://github.com/acme/web"), None);
    }
}
