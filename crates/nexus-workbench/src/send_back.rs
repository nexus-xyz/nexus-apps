//! A fork's "Send back to source" (`Workbench::send_back`, src/workbench/send-back.ts): the message queued in the
//! session it was forked from. It holds the fork's latest answer, where its files are and how to see its changes,
//! read from the fork's log with no model call. T3 Code's "Merge back to source conversation" does the same. A side
//! chat's "Bring back" (side_chat.rs) queues its answer the same way.

use crate::event::{StoredEvent, WbEvent, Worktree};
use crate::mentions::cut;

/// The fork's answer is cut to this many characters: enough for a result, not a whole log.
pub const ANSWER_CHARS: usize = 8000;

/// The fork's own latest answer: its agent's last top-level text logged since the fork was made (`since`, its
/// `created_at`). The conversation copied from its source is logged at its first times, so it never counts.
pub fn latest_answer(events: &[StoredEvent], since: i64) -> Option<&str> {
    events
        .iter()
        .rev()
        .filter(|stored| stored.at >= since)
        .find_map(|stored| match &stored.event {
            WbEvent::Text { text, parent: None, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
}

/// A session's title as a link's text, as the composer writes it (mentions.ts `linkTitle`): no brackets, parentheses,
/// backslashes or line breaks, at most 60 characters.
fn link_title(title: &str) -> String {
    let spaced: String = title
        .chars()
        .map(|c| if "[]()\\".contains(c) { ' ' } else { c })
        .collect();
    let spaced = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = cut(&spaced, 60);
    if clean.is_empty() { "session".into() } else { clean }
}

/// The message for the source: a link to the fork (shown as a mention of it), its answer, and a pointer to its
/// changes in its worktree, or on its branch once the worktree is gone.
pub fn message(fork_id: &str, fork_title: &str, worktree: Option<&Worktree>, answer: &str) -> String {
    let mut out = format!(
        "Result from the fork [{}](session:{fork_id}):\n\n{}",
        link_title(fork_title),
        cut(answer, ANSWER_CHARS)
    );
    match worktree {
        Some(worktree) if !worktree.removed => out.push_str(&format!(
            "\n\nIts changes are in the worktree `{path}`, on branch `{branch}` from `{base}`. See them with \
             `git -C {path} diff {base}` and `git -C {path} status`.",
            path = worktree.path,
            branch = worktree.branch,
            base = worktree.base,
        )),
        Some(worktree) => out.push_str(&format!(
            "\n\nIts worktree was removed. Its branch `{branch}` keeps what it committed: `git diff {base}...{branch}`.",
            branch = worktree.branch,
            base = worktree.base,
        )),
        None => {}
    }
    out
}

/// What a side chat's "Bring back" queues in its session (side_chat.rs): a link to the side chat and its answer. It
/// works in the session's folder, so whatever it changed is there already.
pub fn side_chat_message(side_id: &str, side_title: &str, answer: &str) -> String {
    format!(
        "Result from the side chat [{}](session:{side_id}):\n\n{}\n\nIt works in this session's folder, so any files \
         it changed are already changed here.",
        link_title(side_title),
        cut(answer, ANSWER_CHARS)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(seq: i64, at: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "t_fork".into(),
            seq,
            at,
            event,
        }
    }

    fn said(seq: i64, at: i64, text: &str, parent: Option<&str>) -> StoredEvent {
        stored(
            seq,
            at,
            WbEvent::Text {
                id: format!("m{seq}"),
                text: text.into(),
                parent: parent.map(String::from),
            },
        )
    }

    fn worktree(removed: bool) -> Worktree {
        Worktree {
            path: "/wt/repo/nexus-fork".into(),
            branch: "nexus/fork-of-fix".into(),
            base: "origin/main".into(),
            removed,
            pull_request: None,
            pull_requests: Vec::new(),
            review: None,
        }
    }

    #[test]
    fn the_latest_answer_is_the_forks_own_last_top_level_text() {
        let events = vec![
            // Copied from the source, at its own times.
            said(1, 10, "the source's answer", None),
            stored(
                2,
                100,
                WbEvent::Notice {
                    text: "Forked from".into(),
                },
            ),
            said(3, 110, "first try", None),
            said(4, 120, "a sub-agent's words", Some("tool_1")),
            said(5, 130, "  ", None),
        ];
        assert_eq!(latest_answer(&events, 100), Some("first try"));
        // Nothing said in the fork yet: the source's answer is not the fork's.
        assert_eq!(latest_answer(&events[..2], 100), None);
    }

    #[test]
    fn the_message_links_the_fork_and_points_at_its_changes() {
        let text = message(
            "t_fork",
            "Fork of [Fix] (the) login",
            Some(&worktree(false)),
            "  Done: it was the cache.\n",
        );
        assert_eq!(
            text,
            "Result from the fork [Fork of Fix the login](session:t_fork):\n\nDone: it was the cache.\n\nIts changes \
             are in the worktree `/wt/repo/nexus-fork`, on branch `nexus/fork-of-fix` from `origin/main`. See them \
             with `git -C /wt/repo/nexus-fork diff origin/main` and `git -C /wt/repo/nexus-fork status`."
        );
        let gone = message("t_fork", "Fork", Some(&worktree(true)), "Done.");
        assert!(gone.ends_with("Its worktree was removed. Its branch `nexus/fork-of-fix` keeps what it committed: `git diff origin/main...nexus/fork-of-fix`."));
        assert_eq!(
            message("t_fork", "()", None, "Done."),
            "Result from the fork [session](session:t_fork):\n\nDone."
        );
    }

    #[test]
    fn a_side_chats_message_links_it_and_says_it_shares_the_folder() {
        assert_eq!(
            side_chat_message("t_side", "Side chat", " It is the cache.\n"),
            "Result from the side chat [Side chat](session:t_side):\n\nIt is the cache.\n\nIt works in this \
             session's folder, so any files it changed are already changed here."
        );
    }

    #[test]
    fn a_long_answer_is_cut() {
        let text = message("t_fork", "Fork", None, &"x".repeat(ANSWER_CHARS + 50));
        assert!(text.ends_with(&format!("{}…", "x".repeat(ANSWER_CHARS))));
    }
}

/// The menu offers return only while the fork's source still exists.
pub fn source_id<'a>(
    fork: &str,
    source: Option<&'a str>,
    available: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let source = source.filter(|source| *source != fork)?;
    available.into_iter().any(|id| id == source).then_some(source)
}

pub fn hint(title: &str) -> String {
    format!("Queue this fork's latest answer, its branch and where its changes are in “{title}”")
}

#[cfg(test)]
mod menu_tests {
    #[test]
    fn a_fork_returns_only_to_an_existing_other_session() {
        assert_eq!(
            super::source_id("fork", Some("source"), ["fork", "source"]),
            Some("source")
        );
        assert_eq!(super::source_id("source", None, ["fork", "source"]), None);
        assert_eq!(super::source_id("fork", Some("gone"), ["fork"]), None);
        assert_eq!(super::source_id("fork", Some("fork"), ["fork"]), None);
        assert_eq!(
            super::hint("Fix the login"),
            "Queue this fork's latest answer, its branch and where its changes are in “Fix the login”"
        );
    }
}
