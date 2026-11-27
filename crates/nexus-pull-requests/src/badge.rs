//! A session's pull request in its sidebar row, in one word: where it stands. Nexus GPUI's sidebar draws it.
//! Failed checks and conflicts are not here: they set the watch's "Needs you".

use crate::pull_request::PullRequestStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    Draft,
    Open,
    Approved,
    Changes,
    Merged,
    Closed,
}

impl Badge {
    /// Merged or closed first, then draft, then what its reviews ask (changes before an approval), else open.
    pub fn of(pull: &PullRequestStatus) -> Badge {
        let asked = |state: &str| pull.reviews.iter().any(|review| review.state == state);
        let decision = pull.review_decision.as_deref();
        if pull.state == "MERGED" {
            Badge::Merged
        } else if pull.state != "OPEN" {
            Badge::Closed
        } else if pull.draft {
            Badge::Draft
        } else if decision == Some("CHANGES_REQUESTED") || asked("CHANGES_REQUESTED") {
            Badge::Changes
        } else if decision == Some("APPROVED") || (decision.is_none() && asked("APPROVED")) {
            Badge::Approved
        } else {
            Badge::Open
        }
    }

    /// The row's word.
    pub fn label(self) -> &'static str {
        match self {
            Badge::Draft => "Draft",
            Badge::Open => "Open",
            Badge::Approved => "Approved",
            Badge::Changes => "Changes",
            Badge::Merged => "Merged",
            Badge::Closed => "Closed",
        }
    }

    /// The row's tooltip, such as "Pull request #42: changes requested".
    pub fn text(self, pull: &PullRequestStatus) -> String {
        let said = match self {
            Badge::Changes => "changes requested".to_string(),
            other => other.label().to_lowercase(),
        };
        let host = pull.host;
        format!("{} {}: {said}", host.request_title(), host.reference(pull.number))
    }
}

#[cfg(test)]
mod tests {
    use nexus_workbench::source_host::Host;

    use super::*;
    use crate::pull_request::Review;

    fn pull(change: impl FnOnce(&mut PullRequestStatus)) -> PullRequestStatus {
        let mut pull = PullRequestStatus {
            number: 42,
            title: "Sample".into(),
            url: "https://github.com/example/repo/pull/42".into(),
            draft: false,
            state: "OPEN".into(),
            checks: vec![],
            review_decision: None,
            reviews: vec![],
            mergeable: "MERGEABLE".into(),
            host: Host::GitHub,
            head_branch: String::new(),
            base_branch: String::new(),
            comments: vec![],
        };
        change(&mut pull);
        pull
    }

    fn review(state: &str) -> Review {
        Review {
            id: "1".into(),
            author: "alice".into(),
            state: state.into(),
            body: String::new(),
            submitted_at: None,
        }
    }

    #[test]
    fn says_where_the_pull_request_stands_ended_first_then_draft_then_reviews() {
        let kind = |change: fn(&mut PullRequestStatus)| Badge::of(&pull(change));
        assert_eq!(kind(|_| {}), Badge::Open);
        assert_eq!(
            kind(|p| {
                p.draft = true;
                p.reviews = vec![review("CHANGES_REQUESTED")];
            }),
            Badge::Draft
        );
        assert_eq!(kind(|p| p.review_decision = Some("APPROVED".into())), Badge::Approved);
        assert_eq!(
            kind(|p| {
                p.review_decision = Some("APPROVED".into());
                p.reviews = vec![review("CHANGES_REQUESTED")];
            }),
            Badge::Changes
        );
        assert_eq!(kind(|p| p.reviews = vec![review("APPROVED")]), Badge::Approved);
        assert_eq!(
            kind(|p| {
                p.review_decision = Some("REVIEW_REQUIRED".into());
                p.reviews = vec![review("APPROVED")];
            }),
            Badge::Open
        );
        assert_eq!(kind(|p| p.state = "MERGED".into()), Badge::Merged);
        assert_eq!(kind(|p| p.state = "CLOSED".into()), Badge::Closed);
    }

    #[test]
    fn names_the_request_as_its_host_does() {
        let asked = pull(|p| p.review_decision = Some("CHANGES_REQUESTED".into()));
        assert_eq!(Badge::of(&asked).text(&asked), "Pull request #42: changes requested");
        let gitlab = pull(|p| p.host = Host::GitLab);
        assert_eq!(Badge::of(&gitlab).text(&gitlab), "Merge request !42: open");
    }
}
