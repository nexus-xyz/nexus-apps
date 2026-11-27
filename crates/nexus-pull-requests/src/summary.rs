//! What the card beside a hovered session row says about its pull request: where it stands, who reviewed it and
//! what each asked, how its checks stand, and what else blocks it. Nexus GPUI caches it when the watch changes.

mod excerpt;

use excerpt::Excerpt;
pub use excerpt::{Mark, Style};

use crate::badge::Badge;
use crate::pull_request::PullRequestStatus;

/// How a review reads in the card: its dot's hue and its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    /// Changes first: they are what the person acts on.
    Changes,
    Approved,
    Commented,
    Pending,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewLine {
    pub author: String,
    pub tone: Tone,
    /// "requested changes", "approved", "commented".
    pub said: String,
    /// The review's Markdown as a bounded preview, with byte ranges for inline formatting.
    pub excerpt: String,
    pub marks: Vec<Mark>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Checks {
    /// "1 failed · 4 passed · 2 running": only the counts that are not zero.
    pub line: String,
    /// The failed checks' names, at most [`FAILED_SHOWN`], then "+N more".
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub badge: Badge,
    /// "#42", or "!42" on GitLab.
    pub reference: String,
    pub title: String,
    /// "Changes requested", "Approved", "Open", "Draft", "Merged", "Closed".
    pub headline: &'static str,
    pub reviews: Vec<ReviewLine>,
    /// None when it is no longer open, or has no checks.
    pub checks: Option<Checks>,
    /// What else blocks an open one: a conflict, a review it still waits for.
    pub notes: Vec<&'static str>,
}

pub const FAILED_SHOWN: usize = 3;

impl Summary {
    pub fn of(pull: &PullRequestStatus) -> Summary {
        let badge = Badge::of(pull);
        let mut reviews: Vec<ReviewLine> = pull
            .reviews
            .iter()
            .map(|review| {
                let (tone, said) = tone_of(&review.state);
                let preview = Excerpt::of(&review.body);
                ReviewLine {
                    author: review.author.clone(),
                    tone,
                    said,
                    excerpt: preview.text,
                    marks: preview.marks,
                }
            })
            .collect();
        reviews.sort_by_key(|review| review.tone);
        Summary {
            badge,
            reference: pull.host.reference(pull.number),
            title: pull.title.clone(),
            headline: headline(badge),
            checks: checks(pull),
            notes: notes(pull, &reviews),
            reviews,
        }
    }
}

fn headline(badge: Badge) -> &'static str {
    match badge {
        Badge::Changes => "Changes requested",
        other => other.label(),
    }
}

fn tone_of(state: &str) -> (Tone, String) {
    match state {
        "CHANGES_REQUESTED" => (Tone::Changes, "requested changes".into()),
        "APPROVED" => (Tone::Approved, "approved".into()),
        "COMMENTED" => (Tone::Commented, "commented".into()),
        "DISMISSED" => (Tone::Dismissed, "review dismissed".into()),
        "PENDING" => (Tone::Pending, "review pending".into()),
        other => (Tone::Commented, other.to_lowercase().replace('_', " ")),
    }
}

/// A review's readable Markdown text, limited to 280 characters.
pub fn excerpt(body: &str) -> String {
    Excerpt::of(body).text
}

fn checks(pull: &PullRequestStatus) -> Option<Checks> {
    if pull.state != "OPEN" || pull.checks.is_empty() {
        return None;
    }
    let count = |state: &str| pull.checks.iter().filter(|check| check.state == state).count();
    let line = [
        (count("failure"), "failed"),
        (count("success"), "passed"),
        (count("pending"), "running"),
        (count("skipped"), "skipped"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, word)| format!("{n} {word}"))
    .collect::<Vec<_>>()
    .join(" · ");
    let names: Vec<String> = pull
        .checks
        .iter()
        .filter(|check| check.state == "failure")
        .map(|check| check.name.clone())
        .collect();
    let mut failed: Vec<String> = names.iter().take(FAILED_SHOWN).cloned().collect();
    if names.len() > FAILED_SHOWN {
        failed.push(format!("+{} more", names.len() - FAILED_SHOWN));
    }
    Some(Checks { line, failed })
}

fn notes(pull: &PullRequestStatus, reviews: &[ReviewLine]) -> Vec<&'static str> {
    if pull.state != "OPEN" {
        return vec![];
    }
    let mut notes = vec![];
    if pull.mergeable == "CONFLICTING" {
        notes.push("Conflicts with its base branch");
    }
    let decided = reviews
        .iter()
        .any(|review| matches!(review.tone, Tone::Approved | Tone::Changes));
    if !pull.draft && pull.review_decision.as_deref() == Some("REVIEW_REQUIRED") && !decided {
        notes.push("Waiting for a review");
    }
    notes
}

#[cfg(test)]
mod tests {
    use nexus_workbench::source_host::Host;

    use super::*;
    use crate::pull_request::{Check, Review};

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

    fn review(author: &str, state: &str, body: &str) -> Review {
        Review {
            id: author.into(),
            author: author.into(),
            state: state.into(),
            body: body.into(),
            submitted_at: None,
        }
    }

    fn check(name: &str, state: &str) -> Check {
        Check {
            name: name.into(),
            state: state.into(),
            url: None,
        }
    }

    #[test]
    fn says_who_asked_for_changes_first_with_the_text_of_what_they_asked() {
        let summary = Summary::of(&pull(|p| {
            p.reviews = vec![
                review("bob", "APPROVED", ""),
                review("carol", "COMMENTED", "Nice."),
                review("alice", "CHANGES_REQUESTED", "> quoted\nRename it."),
            ];
        }));
        assert_eq!(summary.headline, "Changes requested");
        let said: Vec<String> = summary
            .reviews
            .iter()
            .map(|r| format!("{} {}", r.author, r.said))
            .collect();
        assert_eq!(said, ["alice requested changes", "bob approved", "carol commented"]);
        assert_eq!(summary.reviews[0].excerpt, "quoted Rename it.");
    }

    #[test]
    fn counts_the_checks_that_ran_and_names_the_failed_ones() {
        let summary = Summary::of(&pull(|p| {
            p.checks = ["a", "b", "c", "d"].iter().map(|name| check(name, "failure")).collect();
            p.checks.push(check("ok", "success"));
            p.checks.push(check("slow", "pending"));
        }));
        let failed = ["a", "b", "c", "+1 more"].map(String::from).to_vec();
        assert_eq!(
            summary.checks,
            Some(Checks {
                line: "4 failed · 1 passed · 1 running".into(),
                failed
            })
        );
    }

    #[test]
    fn notes_a_conflict_and_a_review_it_still_waits_for_only_while_open() {
        let blocked = |p: &mut PullRequestStatus| {
            p.mergeable = "CONFLICTING".into();
            p.review_decision = Some("REVIEW_REQUIRED".into());
        };
        assert_eq!(
            Summary::of(&pull(blocked)).notes,
            ["Conflicts with its base branch", "Waiting for a review"]
        );
        let approved = Summary::of(&pull(|p| {
            blocked(p);
            p.reviews = vec![review("bob", "APPROVED", "")];
        }));
        assert_eq!(approved.notes, ["Conflicts with its base branch"]);
        let draft = Summary::of(&pull(|p| {
            blocked(p);
            p.draft = true;
        }));
        assert_eq!(draft.notes, ["Conflicts with its base branch"]);
        let merged = Summary::of(&pull(|p| {
            blocked(p);
            p.state = "MERGED".into();
            p.checks = vec![check("rust", "failure")];
        }));
        assert_eq!(
            (merged.headline, merged.notes.len(), merged.checks),
            ("Merged", 0, None)
        );
    }

    #[test]
    fn names_the_request_as_its_host_does() {
        assert_eq!(Summary::of(&pull(|p| p.host = Host::GitLab)).reference, "!42");
    }

    #[test]
    fn review_previews_read_markdown_as_text_instead_of_showing_its_syntax() {
        let summary = Summary::of(&pull(|p| {
            p.reviews = vec![review(
                "reviewer",
                "APPROVED",
                "## Summary\n\n**Verdict: APPROVE.** This adds `GET /v1/invites` and [audited routes](https://example.com/routes).\n\nThe checks pass.",
            )];
        }));
        assert_eq!(
            summary.reviews[0].excerpt,
            "Verdict: APPROVE. This adds GET /v1/invites and audited routes. The checks pass."
        );
    }

    #[test]
    fn nested_formatting_and_unicode_survive_truncation_without_invalid_ranges() {
        let summary = Summary::of(&pull(|p| {
            p.reviews = vec![review(
                "reviewer",
                "COMMENTED",
                &format!(
                    "***Révision***: `échec` and [the fix](https://example.com). ~~Old~~ {}",
                    "é".repeat(400)
                ),
            )];
        }));
        let review = &summary.reviews[0];
        assert!(review.excerpt.starts_with("Révision: échec and the fix. Old "));
        assert_eq!(review.excerpt.chars().count(), excerpt::MAX);
        assert!(review.excerpt.ends_with('…'));
        let marks: Vec<_> = review
            .marks
            .iter()
            .map(|mark| (&review.excerpt[mark.range.clone()], mark.style))
            .collect();
        assert!(marks.contains(&(
            "Révision",
            Style {
                strong: true,
                emphasis: true,
                ..Style::default()
            }
        )));
        assert!(marks.contains(&(
            "échec",
            Style {
                code: true,
                ..Style::default()
            }
        )));
        assert!(marks.contains(&(
            "the fix",
            Style {
                link: true,
                ..Style::default()
            }
        )));
        assert!(marks.contains(&(
            "Old",
            Style {
                deleted: true,
                ..Style::default()
            }
        )));
        assert!(
            review
                .marks
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start)
        );
    }

    #[test]
    fn previews_lists_code_and_escaped_markdown_without_rendering_html() {
        for (body, expected) in [
            (
                "<!-- hidden -->\n\n- **Fix** the route\n- Check `rust`",
                "Fix the route Check rust",
            ),
            ("```rust\nlet route = 1;\n```", "let route = 1;"),
            ("Use \\*literal\\* and &amp;", "Use *literal* and &"),
            (
                "<details><summary>Detail</summary>\n\nThe actual **review**.\n\n</details>",
                "The actual review.",
            ),
        ] {
            assert_eq!(excerpt(body), expected, "{body}");
        }
    }

    #[test]
    fn bounds_the_preview_and_skips_boilerplate_headings() {
        assert_eq!(excerpt("\n\n## Summary\nmore"), "more");
        assert_eq!(excerpt(&"x".repeat(400)).chars().count(), excerpt::MAX);
        assert_eq!(excerpt(""), "");
    }
}
