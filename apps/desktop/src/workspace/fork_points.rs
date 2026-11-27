//! Fork's "Fork from which point?" page, as the Nexus app has it (`workbench/topbar.tsx`): the turns whose files were
//! recorded (`wb_changed_turns`), the latest first, each known by its number and the message that began it. A fork
//! shows its progress and its failure in the window's notice (`notice_host.rs`), not only under the composer.

use crate::workspace::overlay::OverlaySlot;
use std::time::Duration;

use gpui_kit::*;
use nexus_workbench::event::{StoredEvent, WbEvent};

use super::session_menu::MenuPage;
use super::{View, Workspace};

/// What the notice says while a fork is made: a new worktree can take a while.
const FORKING: &str = "Forking…";
/// The "Forking…" notice stays until the fork ends, up to this.
const FORKING_STAY: Duration = Duration::from_secs(600);

/// One turn a fork can start from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ForkPoint {
    /// The event that ended the turn: what the workbench forks from.
    pub turn: i64,
    pub label: String,
    /// The message that began the turn, cut short.
    pub text: String,
}

/// The fork page's points for session `id`: None while they are read.
pub(super) struct ForkPoints {
    pub id: String,
    pub points: Option<Result<Vec<ForkPoint>, String>>,
}

/// The fork in progress: its session and its notice.
pub(super) struct Forking {
    pub id: String,
    notice: u64,
}

/// The stretches of the log that a revert or a rewind undid: events with `after < seq < at`, as the workbench reads them
/// (`reverted_spans`). A fork refuses a turn in one, and its checkpoint stays after the revert.
fn undone(events: &[StoredEvent]) -> Vec<(i64, i64)> {
    events
        .iter()
        .filter_map(|stored| match stored.event {
            WbEvent::Revert { turn, .. } => Some((turn, stored.seq)),
            WbEvent::Rewind { before, .. } => Some((before - 1, stored.seq)),
            _ => None,
        })
        .collect()
}

/// The points to offer, the latest first: the recorded turns after the first (0 is the folder before it) that are still
/// on the session's history, each labelled as the Nexus app labels them (`turnNumber`, `turnText`).
pub(super) fn points(turns: &[i64], events: &[StoredEvent]) -> Vec<ForkPoint> {
    let spans = undone(events);
    let kept = |seq: i64| !spans.iter().any(|&(after, at)| seq > after && seq < at);
    let turns: Vec<i64> = turns.iter().copied().filter(|turn| *turn > 0 && kept(*turn)).collect();
    let count = turns.len();
    turns
        .iter()
        .rev()
        .enumerate()
        .map(|(index, &turn)| {
            let number = events
                .iter()
                .filter(|stored| {
                    stored.seq <= turn && kept(stored.seq) && matches!(stored.event, WbEvent::TurnEnd { .. })
                })
                .count();
            let number = if number > 0 { number } else { count - index };
            ForkPoint {
                turn,
                label: if index == 0 {
                    "The latest turn".into()
                } else {
                    format!("After turn {number}")
                },
                text: turn_text(events, turn),
            }
        })
        .collect()
}

/// The person's message that began the turn ending at `turn`: its first line, at most 48 characters.
fn turn_text(events: &[StoredEvent], turn: i64) -> String {
    let said = events.iter().rev().find_map(|stored| match &stored.event {
        WbEvent::User { text, from: None, .. } if stored.seq < turn => Some(text),
        _ => None,
    });
    let line = said.and_then(|text| text.lines().next()).unwrap_or("").trim();
    if line.chars().count() > 48 {
        format!("{}…", line.chars().take(47).collect::<String>())
    } else {
        line.to_string()
    }
}

impl Workspace {
    /// Fork's page in session `id`'s menu: read its points.
    pub(super) fn open_fork_points(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_menu_page(MenuPage::Fork, cx);
        let Some(runs) = self.runs(&id) else {
            self.fork_points = Some(ForkPoints {
                id,
                points: Some(Err(self.read_only_note().to_string())),
            });
            return;
        };
        self.fork_points = Some(ForkPoints {
            id: id.clone(),
            points: None,
        });
        let turns = runs.call("wb_changed_turns", serde_json::json!({ "threadId": id }));
        let events = runs.session_events(id.clone());
        cx.spawn_in(window, async move |this, cx| {
            let turns = match turns.await {
                Ok(Ok(value)) => serde_json::from_value::<Vec<i64>>(value).map_err(|error| error.to_string()),
                Ok(Err(error)) => Err(error),
                Err(_) => Err("The answer was lost.".into()),
            };
            // A label is a help, not a need: the points stand without the log.
            let events = events.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if let Some(open) = this.fork_points.as_mut().filter(|open| open.id == id) {
                    open.points = Some(turns.map(|turns| points(&turns, &events)));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether session `id` is being forked: its Fork waits, so a second click makes no second fork.
    pub(super) fn fork_in_progress(&self, id: &str) -> bool {
        self.forking.as_ref().is_some_and(|forking| forking.id == id)
    }

    /// Fork the session from turn `turn` (the latest when None) and open the fork. The notice says it is being made,
    /// then why it failed.
    pub(super) fn fork_session(
        &mut self,
        id: String,
        turn: Option<i64>,
        subscription: Option<nexus_workbench::ForkSubscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay.close_session_menu();
        self.fork_points = None;
        if self.forking.is_some() {
            return;
        }
        let Some(runs) = self.runs(&id) else {
            let note = self.read_only_note();
            self.show_notice(
                format!("Couldn't fork it: {note}"),
                None,
                nexus_desktop_core::notice::NOTICE,
                cx,
            );
            return;
        };
        let notice = self.show_notice(FORKING, None, FORKING_STAY, cx);
        self.forking = Some(Forking { id: id.clone(), notice });
        let forked = runs.fork(id, turn, subscription);
        cx.spawn_in(window, async move |this, cx| {
            let answer = forked.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let View::Code(view) = &mut this.view {
                    view.forking = None;
                }
                if let Some(forking) = this.forking.take() {
                    this.notices.dismiss(Some(forking.notice));
                }
                match answer {
                    Ok(fork) => {
                        this.refresh_code(cx);
                        this.open_code(&fork, window, cx);
                    }
                    Err(error) => {
                        this.show_notice(
                            format!("Couldn't fork it: {error}"),
                            None,
                            nexus_desktop_core::notice::NOTICE,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn stored(seq: i64, event: WbEvent) -> StoredEvent {
        StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event,
        }
    }

    fn user(seq: i64, text: &str) -> StoredEvent {
        stored(
            seq,
            WbEvent::User {
                text: text.into(),
                images: Vec::new(),
                from: None,
            },
        )
    }

    fn end(seq: i64) -> StoredEvent {
        serde_json::from_value(serde_json::json!({
            "threadId": "t", "seq": seq, "at": 0,
            "event": { "kind": "turnEnd", "turnId": "x", "status": "completed", "costUsd": null, "durationMs": 1, "error": null }
        }))
        .expect("turn end")
    }

    #[test]
    fn points_are_the_latest_first_with_their_number_and_message() {
        let long = "a".repeat(60);
        let events = vec![user(1, "Fix the login\nmore"), end(4), user(5, &long), end(9)];
        let points = points(&[0, 4, 9], &events);
        assert_eq!(
            points,
            vec![
                ForkPoint {
                    turn: 9,
                    label: "The latest turn".into(),
                    text: format!("{}…", "a".repeat(47)),
                },
                ForkPoint {
                    turn: 4,
                    label: "After turn 1".into(),
                    text: "Fix the login".into(),
                },
            ]
        );
    }

    #[test]
    fn a_revert_leaves_the_turns_it_undid_out() {
        let revert = serde_json::from_value(serde_json::json!({
            "threadId": "t", "seq": 10, "at": 0, "event": { "kind": "revert", "turn": 4, "files": 1 }
        }))
        .expect("revert");
        let events = vec![
            user(1, "Fix the login"),
            end(4),
            user(5, "Try another way"),
            end(9),
            revert,
            user(11, "Add a test"),
            end(14),
        ];
        let points = points(&[0, 4, 9, 14], &events);
        assert_eq!(
            points
                .iter()
                .map(|point| (point.turn, point.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(14, "The latest turn"), (4, "After turn 1")],
            "turn 9 was reverted: the workbench refuses to fork it"
        );
    }

    #[test]
    fn the_folder_before_the_first_turn_is_no_point() {
        assert!(points(&[0], &[]).is_empty());
        assert!(points(&[], &[]).is_empty());
    }
}
