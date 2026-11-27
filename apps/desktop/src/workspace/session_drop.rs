//! What dropping a dragged session does, with no UI: the drag's state machine
//! (`session_drag_machine.rs`) asks [`plan`] whether a target takes the session, and the sidebar applies the [`Plan`].
//!
//! A session lands on a row of its own project, before or after it, wherever it comes from: from its project it moves,
//! from a group put aside or the archive it comes back to that spot. Landing among the pinned sessions pins it, and
//! landing among the others unpins it. Dropped on a group, it goes in that group; dropped on its own project's head or
//! foot, it comes back on top. A drag of a picked row carries the other picked sessions: each goes where
//! it goes, by [`plan_one`].

use std::rc::Rc;

use nexus_workbench::api::UpdateArgs;

use crate::code::Aside;

/// A list a session row is in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Place {
    /// A Code project, by its lead's id (`repo_groups.rs`).
    Project(String),
    Aside(Aside),
    Archived,
}

/// What a drop needs to know of the dragged session.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Facts {
    pub id: String,
    /// Its own Code project (one copy of a repository).
    pub project: String,
    pub from: Place,
    pub pinned: bool,
    /// A linked session, drawn under the one that started it: it moves with it, never on its own.
    pub linked: bool,
    /// The other picked sessions it carries, in the sidebar's order: each goes where it goes.
    pub with: Vec<Facts>,
}

impl Facts {
    /// The dragged session and the ones it carries, in the sidebar's order, the dragged one first.
    pub fn all(&self) -> impl Iterator<Item = &Facts> {
        std::iter::once(self).chain(&self.with)
    }
}

/// A spot in a project's list: before or after one of its sessions.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Spot {
    /// The project, by its lead's id: its order is kept under it (`project_groups.rs`).
    pub project: String,
    /// The project's Code projects: one per copy of the repository.
    pub members: Rc<[String]>,
    /// The project's sessions in their order on screen.
    pub listed: Rc<[String]>,
    /// The session it lands next to: for a linked one's row, the top of its family.
    pub id: String,
    pub after: bool,
    /// Whether the sessions around the spot are pinned.
    pub pinned: bool,
}

/// What the pointer is over during a drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    /// A row of a project: the session lands at this spot.
    Row(Spot),
    /// A project's head or foot: a session put aside comes back to it.
    Project { lead: String, members: Rc<[String]> },
    /// "Settled", "Snoozed" or "Archived".
    Group(Place),
    /// One of the snooze presets shown under "Snoozed" during a drag: snoozed until this time (ms).
    SnoozeUntil(i64),
}

impl Target {
    /// The group or project this target lights up, if it lights one.
    pub fn place(&self) -> Option<Place> {
        match self {
            Target::Row(_) | Target::SnoozeUntil(_) => None,
            Target::Project { lead, .. } => Some(Place::Project(lead.clone())),
            Target::Group(place) => Some(place.clone()),
        }
    }
}

/// Where a dropped session goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Put {
    /// It stays in its project.
    Stay,
    Settle,
    /// Snoozed until this time (ms), or, with none, the drop's default ("Until tomorrow morning").
    Snooze(Option<i64>),
    Archive,
    /// Back in its project from the group it was put aside in, or from the archive.
    Return,
}

/// What a drop does: where the session goes, whether it is pinned or unpinned, and the spot it lands at.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Plan {
    pub put: Put,
    pub pin: Option<bool>,
    pub spot: Option<Spot>,
}

/// What dropping `drag` on `target` does to the dragged session itself; None when that target does not take it. A
/// drag that carries several sessions (`Facts::with`) lands on a row only if each of them may land there, and in a
/// group if any of them may: the others are there already.
pub(super) fn plan(drag: &Facts, target: &Target) -> Option<Plan> {
    let mut each = drag.all().map(|one| plan_one(one, target));
    match target {
        Target::Row(_) => {
            let plans: Option<Vec<Plan>> = each.collect();
            plans.and_then(|plans| plans.into_iter().next())
        }
        _ => each.find_map(|plan| plan),
    }
}

/// What dropping one session, `drag`, on `target` does to it.
pub(super) fn plan_one(drag: &Facts, target: &Target) -> Option<Plan> {
    let from = &drag.from;
    let put = |put| Plan {
        put,
        pin: None,
        spot: None,
    };
    match target {
        Target::Row(spot) => {
            if drag.linked || drag.all().any(|one| one.id == spot.id) || !spot.members.contains(&drag.project) {
                return None;
            }
            Some(Plan {
                put: if matches!(from, Place::Project(_)) {
                    Put::Stay
                } else {
                    Put::Return
                },
                pin: (spot.pinned != drag.pinned).then_some(spot.pinned),
                spot: Some(spot.clone()),
            })
        }
        Target::Group(Place::Aside(Aside::Settled)) => {
            (*from != Place::Aside(Aside::Settled)).then(|| put(Put::Settle))
        }
        Target::Group(Place::Aside(Aside::Snoozed)) => {
            (*from != Place::Aside(Aside::Snoozed)).then(|| put(Put::Snooze(None)))
        }
        Target::SnoozeUntil(at) => (*from != Place::Aside(Aside::Snoozed)).then(|| put(Put::Snooze(Some(*at)))),
        Target::Group(Place::Archived) => (*from != Place::Archived).then(|| put(Put::Archive)),
        Target::Group(Place::Project(_)) => None,
        Target::Project { members, .. } => {
            (members.contains(&drag.project) && !matches!(from, Place::Project(_))).then(|| put(Put::Return))
        }
    }
}

/// The session's fields that `plan` changes, coming `from` there; None when it changes none, or when it archives
/// (archiving goes through `archive.rs` for its Undo). `snooze_at` is when a snooze ends (ms) when the plan names no
/// time.
pub(super) fn changes(id: &str, from: &Place, plan: &Plan, snooze_at: Option<i64>) -> Option<UpdateArgs> {
    let snoozed = *from == Place::Aside(Aside::Snoozed);
    let settled = *from == Place::Aside(Aside::Settled);
    let archived = *from == Place::Archived;
    // Out of where it was, unless it stays there.
    let leave = |args: UpdateArgs| UpdateArgs {
        snoozed_until: args.snoozed_until.or(snoozed.then_some(0)),
        settled: args.settled.or(settled.then_some(false)),
        archived: args.archived.or(archived.then_some(false)),
        ..args
    };
    let args = match plan.put {
        Put::Archive => return None,
        Put::Stay => UpdateArgs::default(),
        Put::Return => leave(UpdateArgs::default()),
        Put::Settle => leave(UpdateArgs {
            settled: Some(true),
            ..UpdateArgs::default()
        }),
        Put::Snooze(at) => leave(UpdateArgs {
            snoozed_until: Some(at.or(snooze_at)?),
            ..UpdateArgs::default()
        }),
    };
    let args = UpdateArgs {
        thread_id: id.to_string(),
        pinned: plan.pin,
        ..args
    };
    let changed =
        args.pinned.is_some() || args.snoozed_until.is_some() || args.settled.is_some() || args.archived.is_some();
    changed.then_some(args)
}

#[cfg(test)]
#[path = "session_drop_tests.rs"]
mod tests;
