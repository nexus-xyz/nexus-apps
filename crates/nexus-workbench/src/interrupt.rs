//! What the screen says after an Interrupt: the Continue row over the composer, and the row that names what still runs
//! with one click to stop it all. The Nexus app's `workbench/interrupt.ts` and `composer.tsx`'s `ContinueBar` and
//! `StillRunningBar` hold the same rules; Nexus GPUI reads them here.

use crate::crew_fold::CrewCounts;
use crate::event::{StoredEvent, TurnStatus, WbEvent};

/// What the Continue row says.
pub const CONTINUE_TEXT: &str = "The agent stopped before it finished this turn.";

/// The tooltip on "Stop all".
pub const STOP_ALL_HINT: &str =
    "End the agent and everything it still runs. Your next message resumes the conversation.";

/// A turn ended interrupted since the person's last message. A turn the agent starts by itself, to answer a sub-agent
/// that reported back, is not the person's: an Interrupt before it still counts until they send a message.
pub fn interrupted_since_message(events: &[StoredEvent]) -> bool {
    for stored in events.iter().rev() {
        match &stored.event {
            WbEvent::User { from: None, .. } | WbEvent::AgentSwitched { .. } => return false,
            WbEvent::TurnEnd {
                status: TurnStatus::Interrupted,
                ..
            } => return true,
            _ => {}
        }
    }
    false
}

/// After an Interrupt that left work running, while its agent process lives (`live`: the summary has capabilities, so
/// "Stop all" has a process to end): what still runs, for the row that offers to stop it all. None otherwise.
pub fn still_running(interrupted: bool, live: bool, counts: &CrewCounts) -> Option<String> {
    let (agents, shells) = (counts.agents_running, counts.shells_running);
    if !interrupted || !live || agents + shells == 0 {
        return None;
    }
    let part = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let what = [
        (agents > 0).then(|| part(agents, "sub-agent", "sub-agents")),
        (shells > 0).then(|| part(shells, "background command", "background commands")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" and ");
    let verb = if agents + shells == 1 { "is" } else { "are" };
    Some(format!("{what} {verb} still running."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Sender, TurnStatus};

    fn counts(agents_running: u64, shells_running: u64) -> CrewCounts {
        CrewCounts {
            agents_running,
            shells_running,
            total: agents_running + shells_running,
            ..CrewCounts::default()
        }
    }

    #[test]
    fn says_what_still_runs_after_an_interrupt_and_nothing_otherwise() {
        assert_eq!(
            still_running(true, true, &counts(2, 1)).as_deref(),
            Some("2 sub-agents and 1 background command are still running.")
        );
        assert_eq!(
            still_running(true, true, &counts(1, 0)).as_deref(),
            Some("1 sub-agent is still running.")
        );
        assert_eq!(
            still_running(true, true, &counts(0, 3)).as_deref(),
            Some("3 background commands are still running.")
        );
        assert_eq!(still_running(true, true, &counts(0, 0)), None);
        assert_eq!(still_running(false, true, &counts(2, 0)), None);
        // No agent process (Nexus was killed, so the log still shows them running): "Stop all" would have nothing to end.
        assert_eq!(still_running(true, false, &counts(2, 0)), None);
    }

    #[test]
    fn reads_whether_a_turn_ended_interrupted_since_the_persons_last_message() {
        let log = |events: Vec<WbEvent>| -> Vec<StoredEvent> {
            events
                .into_iter()
                .enumerate()
                .map(|(i, event)| StoredEvent {
                    thread_id: "t".into(),
                    seq: i as i64 + 1,
                    at: i as i64 + 1,
                    event,
                })
                .collect()
        };
        let start = || WbEvent::TurnStart { turn_id: "u".into() };
        let end = |status: TurnStatus| WbEvent::TurnEnd {
            turn_id: "u".into(),
            status,
            cost_usd: None,
            duration_ms: 0,
            error: None,
            cursor: None,
        };
        let text = || WbEvent::Text {
            id: "m".into(),
            text: "later".into(),
            parent: Some("toolu_a".into()),
        };
        let user = || WbEvent::User {
            text: "next".into(),
            images: Vec::new(),
            from: None,
        };
        let interrupted = || end(TurnStatus::Interrupted);
        let completed = || end(TurnStatus::Completed);
        assert!(interrupted_since_message(&log(vec![user(), start(), interrupted()])));
        // A sub-agent's work after the turn does not change how the turn ended.
        assert!(interrupted_since_message(&log(vec![
            user(),
            start(),
            interrupted(),
            text()
        ])));
        // A turn the agent starts by itself (a spared sub-agent reported back) keeps the row: others may still run.
        assert!(interrupted_since_message(&log(vec![
            user(),
            start(),
            interrupted(),
            text(),
            start(),
            completed()
        ])));
        // The person's next message moves on, queued or not.
        assert!(!interrupted_since_message(&log(vec![
            user(),
            start(),
            interrupted(),
            user()
        ])));
        assert!(!interrupted_since_message(&log(vec![
            user(),
            start(),
            interrupted(),
            user(),
            start(),
            completed()
        ])));
        assert!(!interrupted_since_message(&log(vec![user(), start(), completed()])));
        assert!(!interrupted_since_message(&[]));
        // A linked session's message is not the person's: the Interrupt before it still counts.
        let linked = WbEvent::User {
            text: "from a linked session".into(),
            images: Vec::new(),
            from: Some(Sender::Session {
                thread_id: "s".into(),
                title: "S".into(),
            }),
        };
        assert!(interrupted_since_message(&log(vec![
            user(),
            start(),
            interrupted(),
            linked
        ])));
    }
}
