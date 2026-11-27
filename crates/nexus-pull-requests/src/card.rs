//! The words a window shows for a session's pull request watch ([`crate::watch::PullWatch`]): whether something waits
//! to be sent to the agent, the Overview's one-word summary, and the notes beside the switch and the agent's own watch.
//! Nexus GPUI says them from here.

use nexus_workbench::source_host::Host;

use crate::pull_request::PullRequestStatus;
use crate::watch::MAX_WAKES;

/// An open pull request with a failed check, a review that asks for changes, or a conflict: something to send.
pub fn needs_fix(pull: Option<&PullRequestStatus>) -> bool {
    let Some(pull) = pull.filter(|pull| pull.state == "OPEN") else {
        return false;
    };
    pull.checks.iter().any(|check| check.state == "failure")
        || pull.reviews.iter().any(|review| review.state == "CHANGES_REQUESTED")
        || pull.mergeable == "CONFLICTING"
}

fn left(wakes: u32) -> u32 {
    MAX_WAKES.saturating_sub(wakes)
}

/// The switch's note: how many of its wakes are left.
pub fn wakes_left(wakes: u32) -> String {
    match left(wakes) {
        0 => format!("(used all {MAX_WAKES} times)"),
        left => format!("({left} of {MAX_WAKES} left)"),
    }
}

/// The line under the checks while the agent watches its own pull request.
pub fn agent_watch_note(wakes: u32, host: Host) -> String {
    let request = host.request();
    match left(wakes) {
        0 => format!(
            "The agent asked to be told when this {request} changes. It was told {MAX_WAKES} times, the most per session."
        ),
        left => {
            format!("The agent asked to be told when this {request} changes ({left} of {MAX_WAKES} messages left).")
        }
    }
}

/// The Overview section's one-word summary: what the pull request needs, else how its checks stand.
pub fn pull_summary(pull: Option<&PullRequestStatus>) -> Option<String> {
    let pull = pull?;
    if pull.state != "OPEN" {
        return Some(if pull.state == "MERGED" { "Merged" } else { "Closed" }.into());
    }
    let failed = pull.checks.iter().filter(|check| check.state == "failure").count();
    if failed == 1 {
        return Some("1 check failed".into());
    }
    if failed > 1 {
        return Some(format!("{failed} checks failed"));
    }
    if pull.reviews.iter().any(|review| review.state == "CHANGES_REQUESTED") {
        return Some("Changes requested".into());
    }
    if pull.mergeable == "CONFLICTING" {
        return Some("Conflicts".into());
    }
    if pull.checks.iter().any(|check| check.state == "pending") {
        return Some("Checks running".into());
    }
    Some(if pull.draft {
        "Draft".into()
    } else {
        pull.host.reference(pull.number)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{Check, Review};

    fn pull() -> PullRequestStatus {
        PullRequestStatus {
            number: 42,
            title: "Sample".into(),
            url: "https://github.com/example/repo/pull/42".into(),
            draft: false,
            state: "OPEN".into(),
            checks: vec![check("success")],
            review_decision: None,
            reviews: Vec::new(),
            mergeable: "MERGEABLE".into(),
            host: Host::GitHub,
            head_branch: String::new(),
            base_branch: String::new(),
            comments: Vec::new(),
        }
    }

    fn check(state: &str) -> Check {
        Check {
            name: "rust".into(),
            state: state.into(),
            url: None,
        }
    }

    fn failed() -> PullRequestStatus {
        PullRequestStatus {
            checks: vec![Check {
                url: Some("https://github.com/run/1".into()),
                ..check("failure")
            }],
            ..pull()
        }
    }

    fn asked(base: PullRequestStatus) -> PullRequestStatus {
        PullRequestStatus {
            reviews: vec![Review {
                id: "1".into(),
                author: "alice".into(),
                state: "CHANGES_REQUESTED".into(),
                body: "Rename it.".into(),
                submitted_at: None,
            }],
            ..base
        }
    }

    #[test]
    fn needs_a_fix_for_a_failed_check_a_review_asking_for_changes_or_a_conflict_on_an_open_pull_request() {
        assert!(needs_fix(Some(&failed())));
        assert!(needs_fix(Some(&asked(pull()))));
        let conflict = PullRequestStatus {
            mergeable: "CONFLICTING".into(),
            ..pull()
        };
        assert!(needs_fix(Some(&conflict)));
        assert!(!needs_fix(Some(&pull())));
        let merged = PullRequestStatus {
            state: "MERGED".into(),
            ..failed()
        };
        assert!(!needs_fix(Some(&merged)));
        assert!(!needs_fix(None));
    }

    #[test]
    fn the_summary_says_what_needs_fixing_first_then_how_the_checks_stand() {
        let summary = |pull: PullRequestStatus| pull_summary(Some(&pull)).unwrap_or_default();
        assert_eq!(summary(asked(failed())), "1 check failed");
        let two = PullRequestStatus {
            checks: vec![check("failure"), check("failure")],
            ..pull()
        };
        assert_eq!(summary(two), "2 checks failed");
        assert_eq!(summary(asked(pull())), "Changes requested");
        assert_eq!(
            summary(PullRequestStatus {
                mergeable: "CONFLICTING".into(),
                ..pull()
            }),
            "Conflicts"
        );
        assert_eq!(
            summary(PullRequestStatus {
                checks: vec![check("pending")],
                ..pull()
            }),
            "Checks running"
        );
        assert_eq!(summary(pull()), "#42");
        assert_eq!(
            summary(PullRequestStatus {
                host: Host::GitLab,
                ..pull()
            }),
            "!42"
        );
        assert_eq!(summary(PullRequestStatus { draft: true, ..pull() }), "Draft");
        assert_eq!(
            summary(PullRequestStatus {
                state: "MERGED".into(),
                ..pull()
            }),
            "Merged"
        );
        assert_eq!(
            summary(PullRequestStatus {
                state: "CLOSED".into(),
                ..pull()
            }),
            "Closed"
        );
        assert_eq!(pull_summary(None), None);
    }

    #[test]
    fn the_agent_watch_note_counts_the_messages_left_from_the_same_cap_as_the_switch() {
        assert_eq!(
            agent_watch_note(0, Host::GitHub),
            "The agent asked to be told when this pull request changes (3 of 3 messages left)."
        );
        assert!(agent_watch_note(3, Host::GitHub).contains("the most per session"));
    }

    #[test]
    fn the_switch_counts_down_from_three() {
        assert_eq!(wakes_left(0), "(3 of 3 left)");
        assert_eq!(wakes_left(2), "(1 of 3 left)");
        assert_eq!(wakes_left(3), "(used all 3 times)");
        assert_eq!(wakes_left(5), "(used all 3 times)");
    }

    #[test]
    fn the_agent_watch_note_names_a_merge_request_on_gitlab() {
        assert!(agent_watch_note(1, Host::GitLab).contains("this merge request changes"));
        assert!(agent_watch_note(1, Host::GitHub).contains("this pull request changes"));
    }
}
