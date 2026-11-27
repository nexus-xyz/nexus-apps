//! What the session screen says about a session's crew of linked sessions (desktop-linked-sessions.md, "The crew in the
//! app"): the chips under the composer, and how many sessions below it still work (for its Stop). Read from the
//! summaries' live fields, never from the log.
//!
//! The tests are in `crew_chips/tests.rs`. No I/O and no screen code here.

use std::collections::{HashMap, HashSet};

use crate::event::{ThreadStatus, ThreadSummary};

#[cfg(test)]
mod tests;

/// What the line reads from one session's summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub status: Option<ThreadStatus>,
    pub crew_paused: bool,
    pub crew_tripped: bool,
    pub crew_spend_trip: Option<crate::event::SpendTrip>,
    /// `person`, `parent` or `fault`; None: no halt.
    pub halt: Option<String>,
    pub pending_results: u32,
    pub in_background: bool,
}

impl Facts {
    pub fn of(thread: &ThreadSummary) -> Facts {
        Facts {
            status: Some(thread.status),
            crew_paused: thread.crew_paused,
            crew_tripped: thread.crew_tripped,
            crew_spend_trip: thread.crew_spend_trip,
            halt: thread.halt.clone(),
            pending_results: thread.pending_results,
            in_background: thread.in_background,
        }
    }

    /// Running, starting or waiting on an approval.
    fn running(&self) -> bool {
        matches!(
            self.status,
            Some(ThreadStatus::Running | ThreadStatus::Starting | ThreadStatus::Approval)
        )
    }

    /// It runs nothing itself (idle, stopped or failed): Stop on it is the only way to reach the sessions below it
    /// (`leadIdle` in crew-controls.tsx).
    pub fn lead_idle(&self) -> bool {
        matches!(
            self.status,
            Some(ThreadStatus::Idle | ThreadStatus::Interrupted | ThreadStatus::Error)
        )
    }
}

/// One session as the tree reads it.
#[derive(Debug, Clone, Copy)]
pub struct Session<'a> {
    pub id: &'a str,
    /// The session that started it (`ThreadSummary::parent_thread_id`).
    pub parent: Option<&'a str>,
    pub title: &'a str,
    pub facts: &'a Facts,
}

/// The sessions, by id, with each one's children.
pub struct Crews<'a> {
    by_id: HashMap<&'a str, Session<'a>>,
    children: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> Crews<'a> {
    pub fn new(sessions: impl IntoIterator<Item = Session<'a>>) -> Crews<'a> {
        let mut by_id = HashMap::new();
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for session in sessions {
            if let Some(parent) = session.parent {
                children.entry(parent).or_default().push(session.id);
            }
            by_id.insert(session.id, session);
        }
        Crews { by_id, children }
    }

    /// The sessions below `id`, each once (a damaged store may loop).
    fn descendants(&self, id: &'a str) -> Vec<Session<'a>> {
        let mut seen = HashSet::from([id]);
        let mut found = Vec::new();
        let mut stack: Vec<&str> = self.children.get(id).cloned().unwrap_or_default();
        while let Some(next) = stack.pop() {
            let Some(session) = self.by_id.get(next) else {
                continue;
            };
            if !seen.insert(next) {
                continue;
            }
            found.push(*session);
            stack.extend(self.children.get(next).into_iter().flatten());
        }
        found
    }

    /// The top of `id`'s chain of parents, as the core finds its tree's root: itself for a session the person started.
    pub fn root(&self, id: &'a str) -> &'a str {
        let mut at = id;
        let mut seen = HashSet::from([at]);
        while let Some(parent) = self.by_id.get(at).and_then(|session| session.parent) {
            if !self.by_id.contains_key(parent) || !seen.insert(parent) {
                break;
            }
            at = parent;
        }
        at
    }

    /// How many sessions below `id` work: running, starting, waiting on an approval, or holding a background
    /// sub-agent. Stop on it reaches them.
    pub fn running(&self, id: &'a str) -> usize {
        self.descendants(id)
            .iter()
            .filter(|session| session.facts.running() || session.facts.in_background)
            .count()
    }

    /// Where `id` sits in its tree.
    pub fn place(&self, id: &'a str) -> Place {
        let parent = self.by_id.get(id).and_then(|session| session.parent);
        let root = self.root(id);
        Place {
            parent_title: parent
                .and_then(|parent| self.by_id.get(parent))
                .map(|session| session.title.to_string()),
            is_root: root == id,
            root_title: self.by_id.get(root).map(|session| session.title.to_string()),
            linked: parent.is_some() || self.children.get(id).is_some_and(|list| !list.is_empty()),
        }
    }
}

/// Where a session sits in its tree, as its chips say it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    /// The title of the session that started it; None for one the person started, or whose parent is gone.
    pub parent_title: Option<String>,
    /// It is its tree's root: only a person message to the root ends the tree's pause.
    pub is_root: bool,
    /// The root's title, for a session below it.
    pub root_title: Option<String>,
    /// It is in a crew: it has a parent, or sessions it started.
    pub linked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    Paused,
    Halted,
    Results,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub kind: ChipKind,
    pub text: String,
}

/// The chips under the composer, most pressing first. The tree is paused (the spend cap or another limit): below the
/// root it names the root, since only a message there ends the pause. It was stopped, by the person or with a session
/// above it: only in a crew, since Continue already says it of a session on its own. Results wait for the person's
/// next message: only while it runs nothing, since a running session reads them itself.
pub fn crew_chips(facts: &Facts, place: &Place) -> Vec<Chip> {
    let mut chips = Vec::new();
    if facts.crew_paused {
        let at = if place.is_root {
            "send a message to continue".to_string()
        } else {
            match place.root_title.as_deref().filter(|title| !title.trim().is_empty()) {
                Some(title) => format!("write to the top session «{title}» to continue"),
                None => "write to the top session to continue".to_owned(),
            }
        };
        let why = if let Some(trip) = facts.crew_spend_trip {
            format!("Spend cap of ${} reached (${:.2} spent)", trip.cap_usd, trip.spent_usd)
        } else if facts.crew_tripped {
            "Spend cap reached".into()
        } else {
            "Paused".into()
        };
        chips.push(Chip {
            kind: ChipKind::Paused,
            text: format!("{why} · {at}"),
        });
    }
    let halt = if place.linked { facts.halt.as_deref() } else { None };
    match halt {
        Some("person" | "fault") => chips.push(Chip {
            kind: ChipKind::Halted,
            text: "Stopped by you".into(),
        }),
        Some("parent") => chips.push(Chip {
            kind: ChipKind::Halted,
            text: format!(
                "Stopped with {}",
                place.parent_title.as_deref().unwrap_or("the parent session")
            ),
        }),
        _ => {}
    }
    let pending = facts.pending_results;
    if pending > 0 && !facts.running() {
        let noun = if pending == 1 { "result goes" } else { "results go" };
        chips.push(Chip {
            kind: ChipKind::Results,
            text: format!("{pending} {noun} with your next message"),
        });
    }
    chips
}

/// "1 subsession runs", "3 subsessions run".
pub fn running_text(running: usize) -> String {
    let verb = if running == 1 {
        "subsession runs"
    } else {
        "subsessions run"
    };
    format!("{running} {verb}")
}
