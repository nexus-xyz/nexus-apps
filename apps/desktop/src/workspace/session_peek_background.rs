//! The hover card's "In background" section (`session_peek.rs`): what a session's agent left running past its turn.
//! Sub-agents and commands come from its summary (`ThreadSummary::background`), linked sessions from what its agent
//! waits on. The card is read-only: its foot points to the Agents tab, where Stop lives.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::BackgroundKind;

use super::glyph;
use super::sidebar::hint;
use crate::code::{self, CodeSession, Status};
use crate::palette::Palette;

/// The card lists this many, then "+N more".
pub(super) const SHOWN: usize = 4;

/// The section's text when only an older host's flag says work runs: the design's words.
const EMPTY_LIST: &str = "This server does not list its background work. Update it to see the list.";

/// One line of the section.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Line {
    pub icon: Lucide,
    pub text: String,
    /// What it is and how long it has run: "sub-agent · 4m", "2m", "subsession".
    pub meta: String,
}

/// The section's lines, newest first, at most [`SHOWN`], and how many more there are. `title_of` names a linked
/// session by its id.
pub(super) fn lines(session: &CodeSession, now: i64, title_of: impl Fn(&str) -> Option<String>) -> (Vec<Line>, usize) {
    let ran = |started_at: Option<i64>| {
        started_at.map(|at| match code::age(at, now).as_str() {
            "now" => "<1m".to_string(),
            age => age.to_string(),
        })
    };
    let work = session.background_work.iter().rev().map(|work| match work.kind {
        BackgroundKind::Agent => Line {
            icon: Lucide::Bot,
            text: or(&work.title, "A sub-agent"),
            meta: match ran(work.started_at) {
                Some(ran) => format!("sub-agent · {ran}"),
                None => "sub-agent".into(),
            },
        },
        BackgroundKind::Shell => Line {
            icon: Lucide::SquareTerminal,
            text: or(work.title.lines().next().unwrap_or_default(), "A command"),
            meta: ran(work.started_at).unwrap_or_default(),
        },
    });
    let sessions = session.waits_on.iter().map(|id| Line {
        icon: Lucide::Link,
        text: match title_of(id) {
            Some(title) => format!("Waits on “{title}”"),
            None => "Waits on a subsession".into(),
        },
        meta: "subsession".into(),
    });
    let all: Vec<Line> = sessions.chain(work).collect();
    // The summary lists at most `BACKGROUND_LISTED`; the count says how many there are past it.
    let more = session.background_count().max(all.len()).saturating_sub(SHOWN);
    (all.into_iter().take(SHOWN).collect(), more)
}

fn or(text: &str, fallback: &str) -> String {
    if text.trim().is_empty() {
        fallback.into()
    } else {
        text.trim().to_string()
    }
}

/// The section for `session` under its `mark`; None when nothing runs in the background.
pub(super) fn section(
    session: &CodeSession,
    mark: Status,
    now: i64,
    title_of: impl Fn(&str) -> Option<String>,
    palette: Palette,
) -> Option<Div> {
    if !session.background_busy {
        return None;
    }
    let (lines, more) = lines(session, now, title_of);
    let behind = matches!(mark, Status::Background | Status::BackgroundUnread);
    let note = |text: &'static str| div().text_size(px(11.)).text_color(palette.faint).child(text);
    Some(
        v_flex()
            .gap(px(6.))
            .pt(px(8.))
            .border_t_1()
            .border_color(palette.border)
            // Under a louder state, say these run too.
            .when(!behind, |list| list.child(note("Also in background")))
            .when(lines.is_empty(), |list| list.child(note(EMPTY_LIST)))
            .children(lines.into_iter().map(|line| {
                h_flex()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .child(glyph(line.icon, palette.faint).size(px(14.)))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(line.text))
                    .when(!line.meta.is_empty(), |row| row.child(hint(line.meta, palette)))
            }))
            .when(more > 0, |list| {
                list.child(note_owned(format!("+{more} more"), palette))
            })
            .child(note("The Background tasks tab stops them.")),
    )
}

fn note_owned(text: String, palette: Palette) -> Div {
    div().text_size(px(11.)).text_color(palette.faint).child(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_workbench::event::BackgroundWork;

    fn session(work: Vec<BackgroundWork>, waits_on: Vec<&str>) -> CodeSession {
        let json = r#"{"id":"s","projectId":"p","title":"t","agent":"claude","model":null,"permissionMode":"default",
            "via":"nexus","status":"idle","costUsd":0,"nexusSessions":[],"contextTokens":null,"contextWindow":null,
            "createdAt":1,"updatedAt":1,"archived":false}"#;
        let mut thread: nexus_workbench::event::ThreadSummary = serde_json::from_str(json).expect("a summary");
        thread.background = work;
        thread.waits_on = waits_on.into_iter().map(String::from).collect();
        CodeSession::of(thread)
    }

    fn work(kind: BackgroundKind, title: &str, started_at: Option<i64>) -> BackgroundWork {
        BackgroundWork {
            kind,
            title: title.into(),
            started_at,
        }
    }

    #[test]
    fn names_each_thing_newest_first_with_how_long_it_ran() {
        let now = 10 * 60_000;
        let s = session(
            vec![
                work(BackgroundKind::Agent, "Explore the repo", Some(6 * 60_000)),
                work(BackgroundKind::Shell, "cargo test\n--locked", Some(now - 1000)),
                work(BackgroundKind::Shell, "", None),
            ],
            vec!["child"],
        );
        assert!(s.background_busy);
        assert_eq!(s.background_count(), 4);
        let (lines, more) = lines(&s, now, |id| (id == "child").then(|| "Fix CI".to_string()));
        let shown: Vec<_> = lines.iter().map(|l| (l.text.as_str(), l.meta.as_str())).collect();
        assert_eq!(
            shown,
            vec![
                ("Waits on “Fix CI”", "subsession"),
                ("A command", ""),
                ("cargo test", "<1m"),
                ("Explore the repo", "sub-agent · 4m"),
            ]
        );
        assert_eq!(more, 0);
    }

    #[test]
    fn shows_four_then_a_count() {
        let s = session(
            (0..7)
                .map(|i| work(BackgroundKind::Shell, &format!("job {i}"), Some(0)))
                .collect(),
            vec![],
        );
        let (lines, more) = lines(&s, 0, |_| None);
        assert_eq!((lines.len(), more), (SHOWN, 3));
        assert_eq!(lines[0].text, "job 6");
    }

    #[test]
    fn counts_past_the_summarys_cap() {
        let mut s = session(
            (0..20)
                .map(|i| work(BackgroundKind::Shell, &format!("job {i}"), Some(0)))
                .collect(),
            vec![],
        );
        s.background_total = 31;
        assert_eq!(s.background_count(), 31);
        assert_eq!(lines(&s, 0, |_| None).1, 27);
    }

    #[test]
    fn an_older_hosts_flag_alone_lists_nothing() {
        let mut s = session(vec![], vec![]);
        assert!(!s.background_busy);
        s.background_busy = true;
        assert_eq!(lines(&s, 0, |_| None), (vec![], 0));
    }
}
