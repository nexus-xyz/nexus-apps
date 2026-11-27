//! A Code session's attention mark in the sidebar: a filled dot when a turn ended that the person has not seen ("Finished, unread"), or when the
//! plan's usage limit stopped the turn and nothing runs ("Limited", `CodeSession::limited`).
//!
//! The rule is `nexus_workbench::attention`'s. Looking at a session reads it:
//! the open session is marked seen at each refresh, and its row never shows "Finished, unread". The first read counts
//! every session as seen, so a fresh install does not light up every old session.
//!
//! The Nexus app keeps its "last seen" map in its webview's localStorage, which this build cannot read. So this build
//! keeps its own in `seen.json`, in the same shape, beside its `settings.json`: one map for every window. A write
//! merges the file first, keeping a later mark from another process, and a burst of marks is written once a second.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::*;
use nexus_workbench::attention::{self, Mark, Seen, Throttle, Write};
use nexus_workbench::event::ThreadStatus;

use super::{View, Workspace};
use crate::code::{CodeSession, Status};

/// The marks of every window, and where they are kept.
struct SeenMarks {
    seen: Seen,
    throttle: Throttle,
    file: Option<PathBuf>,
}

impl Global for SeenMarks {}

fn load_from(file: &Path) -> Seen {
    std::fs::read_to_string(file)
        .map(|text| Seen::from_json(&text))
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Reads the kept marks once, for every window. A burst still waiting is written when the app quits.
fn init(cx: &mut App) {
    if cx.has_global::<SeenMarks>() {
        return;
    }
    let file = crate::paths::seen_file();
    let seen = file.as_deref().map(load_from).unwrap_or_default();
    cx.set_global(SeenMarks {
        seen,
        throttle: Throttle::default(),
        file,
    });
    cx.on_app_quit(|cx| {
        if cx.global::<SeenMarks>().throttle.waiting() {
            flush(cx);
        }
        async {}
    })
    .detach();
}

/// Writes the marks now, keeping what another process marked since this one last read.
fn flush(cx: &mut App) {
    cx.update_global::<SeenMarks, _>(|marks, _| {
        marks.throttle.wrote(now_ms());
        let Some(file) = marks.file.clone() else { return };
        marks.seen.merge(&load_from(&file));
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&file, marks.seen.to_json()));
        if let Err(error) = written {
            // Remembered until the app quits.
            eprintln!(
                "nexus-desktop-gpui: read marks not saved to {}: {error}",
                file.display()
            );
        }
    });
}

/// The marks changed: write them now, or once at the end of the burst.
fn changed(cx: &mut App) {
    let write = cx.update_global::<SeenMarks, _>(|marks, _| marks.throttle.changed(now_ms()));
    match write {
        Write::Now => flush(cx),
        Write::After(delay) => cx
            .spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(delay.max(0) as u64))
                    .await;
                cx.update(flush);
            })
            .detach(),
        Write::Waiting => {}
    }
}

/// When the person last saw session `id`; None when never, or before the marks are read.
fn seen_at(id: &str, cx: &App) -> Option<i64> {
    cx.try_global::<SeenMarks>().and_then(|marks| marks.seen.get(id))
}

/// The sidebar's shape for an attention mark.
pub(super) fn status_of(mark: Mark) -> Status {
    match mark {
        Mark::Running => Status::Running,
        Mark::You => Status::NeedsYou,
        Mark::Failed => Status::Failed,
        Mark::Idle => Status::Idle,
        Mark::Background => Status::Background,
        Mark::BackgroundUnread => Status::BackgroundUnread,
    }
}

impl Workspace {
    /// A Code session row's mark: what it needs from the person first, else how it stands. The open session is read as
    /// it happens, so it is never unread. A pull request that needs the person fills the dot of any other session
    /// (`pr_watch::needs_you`), as the Nexus app's sidebar does. Work its agent left running shows over an unread
    /// reply or a failed turn (`attention::with_background`), so a session that still works never looks stopped.
    pub(super) fn session_mark(&self, session: &CodeSession, cx: &App) -> Status {
        let activity = &session.activity;
        let open = matches!(&self.view, View::Code(view) if view.id == session.id);
        let attention = if open && activity.status == ThreadStatus::Idle {
            None
        } else {
            let pull_needs_you = !open && crate::pr_watch::needs_you(&session.id);
            attention::attention_of(activity, session.limited, seen_at(&session.id, cx), pull_needs_you)
        };
        let mark = attention::mark_of(Some(activity.status), attention);
        status_of(attention::with_background(mark, attention, session.background_busy))
    }

    /// The sessions were read again: each one seen for the first time counts as seen, and the open one is seen now.
    pub(super) fn note_seen(&self, cx: &mut App) {
        init(cx);
        let open = match &self.view {
            View::Code(view) if !view.is_draft() => self
                .session_by_id(&view.id)
                .map(|session| (session.id.clone(), session.updated_at)),
            _ => None,
        };
        let sessions = self
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter())
            .chain(self.archived_code.iter())
            .map(|session| (session.id.as_str(), session.updated_at));
        let marked = cx.update_global::<SeenMarks, _>(|marks, _| {
            let first = marks.seen.baseline(sessions);
            let open = open.is_some_and(|(id, at)| marks.seen.mark(&id, at));
            first || open
        });
        if marked {
            changed(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Status, status_of};
    use nexus_workbench::attention::{self, Activity, Mark};
    use nexus_workbench::event::ThreadStatus;

    #[test]
    fn each_mark_takes_the_sidebars_shape() {
        assert_eq!(status_of(Mark::You), Status::NeedsYou);
        assert_eq!(status_of(Mark::Running), Status::Running);
        assert_eq!(status_of(Mark::Failed), Status::Failed);
        assert_eq!(status_of(Mark::Idle), Status::Idle);
        assert_eq!(status_of(Mark::Background), Status::Background);
        assert_eq!(status_of(Mark::BackgroundUnread), Status::BackgroundUnread);
    }

    #[test]
    fn a_turn_finished_since_the_person_looked_fills_the_dot_and_a_stopped_one_does_not() {
        let finished = Activity {
            status: ThreadStatus::Idle,
            updated_at: 20,
            listed_at: Some(20),
        };
        let mark = |activity: &Activity, limited, seen| {
            status_of(attention::mark_of(
                Some(activity.status),
                attention::attention_of(activity, limited, seen, false),
            ))
        };
        assert_eq!(mark(&finished, None, Some(10)), Status::NeedsYou);
        assert_eq!(mark(&finished, None, Some(20)), Status::Idle);
        let stopped = Activity {
            status: ThreadStatus::Interrupted,
            ..finished
        };
        assert_eq!(mark(&stopped, None, Some(10)), Status::Idle);
        let stop = Some(nexus_workbench::event::LimitStop {
            resets_at: None,
            scheduled: false,
        });
        assert_eq!(mark(&finished, stop, Some(20)), Status::NeedsYou);
    }
}
