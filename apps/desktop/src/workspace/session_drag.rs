//! Dragging a session in the sidebar. A session row lifts into a raised copy under the pointer,
//! and its place in the list dims. Over a row of its project, a line shows where it lands; over a group or its own
//! project, the group lights up. An empty "Settled" or "Snoozed" opens as a target only while a drag could land in
//! it. The row then lands with a fill that fades.
//!
//! The drag is a state machine (`session_drag_machine.rs`), and what a drop does is a plain rule (`session_drop.rs`).
//! This file draws them, tells the machine what the pointer does, and applies what lands with what the menus call.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::folder_motion::Transition;
pub(super) use super::session_drag_machine::{Event, Phase, Step};
pub(super) use super::session_drop::{Facts, Place, Plan, Put, Spot, Target};
use super::sidebar::{ROW, ROW_TEXT};
use super::{Workspace, glyph};
use crate::code::CodeSession;
use crate::motion;
use crate::palette::Palette;

/// How long a dropped row's fill takes to fade: long enough to find the row in its new place.
const LAND: Duration = Duration::from_millis(600);
/// How far the dragged row's place dims: still there, so the list does not jump while the drag goes on.
const LIFTED: f32 = 0.4;

/// The session being dragged, and the raised copy drawn under the pointer.
#[derive(Clone)]
pub(super) struct SessionDrag {
    facts: Facts,
    title: SharedString,
    /// How many sessions the drag carries: the dragged one and the picked ones with it.
    count: usize,
    palette: Palette,
}

/// Where a project's row sits, for a session dropped on it.
pub(super) struct RowSlot {
    /// The project, by its lead's id, and its Code projects (one per copy of the repository).
    pub project: String,
    pub members: Rc<[String]>,
    /// The project's sessions in their order on screen.
    pub listed: Rc<[String]>,
    /// The row's family (`session_tree::families`): its top-level session, and its last row.
    pub top: String,
    pub last: String,
    /// Whether the family's top session is pinned.
    pub pinned: bool,
}

impl RowSlot {
    /// The spot a session lands at with the pointer on row `id`: before the family in its top row's upper half, after
    /// the family anywhere else.
    fn spot(&self, id: &str, lower: bool) -> Spot {
        Spot {
            project: self.project.clone(),
            members: self.members.clone(),
            listed: self.listed.clone(),
            id: self.top.clone(),
            after: id != self.top || lower,
            pinned: self.pinned,
        }
    }

    /// Where row `id` draws the line for `spot`: over its top edge (false), under its bottom one (true), or not at all.
    fn line(&self, id: &str, spot: &Spot) -> Option<bool> {
        if spot.id != self.top || spot.project != self.project {
            return None;
        }
        match spot.after {
            false => (id == self.top).then_some(false),
            true => (id == self.last).then_some(true),
        }
    }
}

/// The sidebar's drag: the machine, the lifted row's dimming, the lit targets and the row that landed last.
#[derive(Default)]
pub(super) struct DragState {
    phase: Phase,
    lift: Transition,
    lit: HashMap<Place, Transition>,
    landed: Option<(String, Transition)>,
    /// Opens a folded group the drag rests over, once `session_drag_edge::SPRING` passes.
    pub(super) spring: Option<Task<()>>,
    /// Scrolls the list while the pointer rests near its edge, at `scroll_speed` pixels a frame.
    pub(super) scroll: Option<Task<()>>,
    pub(super) scroll_speed: f32,
    /// The snooze presets shown under "Snoozed" as targets, with their times, read once when the drag rested over it:
    /// a time read on each frame would make a new target on each frame.
    pub(super) presets: Option<Vec<(&'static str, i64)>>,
}

impl DragState {
    /// Stops the spring and the edge scroll, and hides the snooze presets: the drag ended.
    pub(super) fn stop_assists(&mut self) {
        self.spring = None;
        self.scroll = None;
        self.scroll_speed = 0.;
        self.presets = None;
    }

    /// The target the drag is over and its element's key, whether or not GPUI still has the drag.
    pub(super) fn phase_over(&self) -> Option<(&str, &Target)> {
        self.phase.over()
    }

    /// Moves the machine on, and starts the motions its step asks for.
    fn step(&mut self, event: Event) -> Step {
        let lifting = matches!(event, Event::Lift(_));
        let before = self.lit_place();
        let step = self.phase.step(event);
        if lifting {
            self.stop_assists();
            self.lit.clear();
            self.lift = Transition::settled(0.);
            self.lift.retarget(1., motion::UI);
        }
        let after = self.lit_place();
        if before != after {
            if let Some(place) = before {
                self.lit.entry(place).or_default().retarget(0., motion::HOVER);
            }
            if let Some(place) = after {
                self.lit.entry(place).or_default().retarget(1., motion::HOVER);
            }
        }
        if self.phase == Phase::Idle {
            self.lit.clear();
        }
        if let Step::Land(landing) = &step {
            let mut fill = Transition::settled(1.);
            fill.retarget(0., LAND);
            self.landed = Some((landing.drag.id.clone(), fill));
        }
        step
    }

    fn lit_place(&self) -> Option<Place> {
        self.phase.over().and_then(|(_, target)| target.place())
    }

    /// The dragged session, while GPUI still has the drag. A drag let go outside every target ends with no event:
    /// GPUI only stops it.
    pub(super) fn drag(&self, cx: &App) -> Option<&Facts> {
        self.phase.drag().filter(|_| cx.has_active_drag())
    }

    /// What the pointer is over, while a drag goes on.
    pub(super) fn over(&self, cx: &App) -> Option<&Target> {
        self.phase
            .over()
            .map(|(_, target)| target)
            .filter(|_| cx.has_active_drag())
    }
}

impl Render for SessionDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        h_flex()
            .id("session-drag-ghost")
            .w(px(248.))
            .h(px(ROW))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .shadow_md()
            .text_size(px(ROW_TEXT))
            .text_color(palette.foreground)
            .child(glyph(Lucide::GripVertical, palette.faint).size(px(14.)))
            .child(div().flex_1().min_w(px(0.)).truncate().child(self.title.clone()))
            // Several sessions: the dragged one's title, and how many go with it.
            .when(self.count > 1, |ghost| {
                ghost.child(
                    div()
                        .flex_none()
                        .px(px(6.))
                        .rounded_full()
                        .bg(palette.primary)
                        .text_color(palette.primary_foreground)
                        .text_size(px(11.))
                        .child(SharedString::from(self.count.to_string())),
                )
            })
            // It lifts off the list: up a little and to full strength, as a menu opens.
            .with_animation(
                "session-drag-lift",
                Animation::new(motion::UI).with_easing(motion::ease_out),
                |ghost, delta| ghost.opacity(0.7 + 0.3 * delta).mt(px(2. * (1. - delta))),
            )
    }
}

impl Workspace {
    /// `row`, the row of `session` in `place`, made draggable. `depth` is its depth in the project's tree of linked
    /// sessions. `slot`, given for a project's row, makes the row a target: a session dropped on it lands next to its
    /// family.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn drag_row(
        &self,
        row: AnyElement,
        session: &CodeSession,
        place: Place,
        depth: usize,
        slot: Option<RowSlot>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = session.id.clone();
        let drag = SessionDrag {
            facts: Facts {
                id: id.clone(),
                project: session.project_id.clone(),
                from: place,
                pinned: session.pinned,
                linked: depth > 0,
                with: Vec::new(),
            },
            title: session.title.clone().into(),
            count: 1,
            palette,
        };
        let workspace = cx.entity().downgrade();
        // The dragged row and the picked ones it carries dim in their places.
        let dragged = self
            .session_drag
            .drag(cx)
            .is_some_and(|drag| drag.all().any(|one| one.id == id));
        let line = match (self.session_drag.over(cx), &slot) {
            (Some(Target::Row(spot)), Some(slot)) => slot.line(&id, spot),
            _ => None,
        };
        let selector = format!("session-drag-{id}");
        let frame = div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .relative()
            .rounded(px(8.))
            .child(row)
            .on_drag(drag, move |drag, _, _, cx| {
                let mut drag = drag.clone();
                let begun = workspace.update(cx, |this, cx| {
                    this.peek.dismiss();
                    this.end_project_drag();
                    // A picked row carries the other picked sessions.
                    drag.facts.with = this.carried_with(&drag.facts.id);
                    drag.count = 1 + drag.facts.with.len();
                    this.drag_step(Event::Lift(drag.facts.clone()), cx);
                });
                if let Err(error) = begun {
                    eprintln!("nexus-desktop-gpui: a session drag had no window: {error}");
                }
                cx.new(|_| drag)
            })
            .when_some(line, |frame, after| frame.child(drop_line(&id, after, palette)));
        let frame = match slot {
            Some(slot) => {
                let key = format!("row-{id}");
                let row_id = id.clone();
                let dropped_key = key.clone();
                frame
                    .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<SessionDrag>, _, cx| {
                        let lower = event.event.position.y > event.bounds.center().y;
                        this.drag_step(
                            Event::Move {
                                key: key.clone(),
                                inside: event.bounds.contains(&event.event.position),
                                target: Target::Row(slot.spot(&row_id, lower)),
                            },
                            cx,
                        );
                    }))
                    .on_drop(cx.listener(move |this, _: &SessionDrag, _, cx| {
                        this.drag_step(
                            Event::Drop {
                                key: dropped_key.clone(),
                            },
                            cx,
                        );
                    }))
            }
            None => frame,
        };
        let frame = match &self.session_drag.landed {
            Some((landed, fill)) if *landed == id => fill.draw(
                div().rounded(px(8.)).child(frame),
                format!("session-landed-{id}"),
                move |frame, value| frame.bg(palette.selected.opacity(value)),
            ),
            _ => frame.into_any_element(),
        };
        if dragged {
            self.session_drag
                .lift
                .draw(div().child(frame), format!("session-lifted-{id}"), |frame, value| {
                    frame.opacity(1. - (1. - LIFTED) * value)
                })
        } else {
            frame
        }
    }

    /// `content` (a group, or a project's head or foot) as a target for a dragged session: it lights up while the
    /// pointer is in it and `target` would take the session. `key` names it among the targets.
    pub(super) fn drop_zone(
        &self,
        content: AnyElement,
        key: String,
        target: Target,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lit = match (self.session_drag.drag(cx), target.place()) {
            (Some(_), Some(place)) => self.session_drag.lit.get(&place).copied().unwrap_or_default(),
            // A target of no group (a snooze preset) lights while the pointer is on it.
            (Some(_), None) if self.session_drag.phase_over().is_some_and(|(over, _)| over == key) => {
                Transition::settled(1.)
            }
            _ => Transition::settled(0.),
        };
        let selector = format!("session-drop-zone-{key}");
        let fill = lit.draw(
            div().absolute().inset_0().rounded(px(8.)).border_1(),
            format!("{selector}-fill"),
            move |fill, value| {
                fill.bg(palette.selected.opacity(value))
                    .border_color(palette.primary.opacity(0.45 * value))
            },
        );
        let moved_key = key.clone();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .relative()
            .child(fill)
            .child(content)
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<SessionDrag>, _, cx| {
                this.drag_step(
                    Event::Move {
                        key: moved_key.clone(),
                        inside: event.bounds.contains(&event.event.position),
                        target: target.clone(),
                    },
                    cx,
                );
            }))
            .on_drop(cx.listener(move |this, _: &SessionDrag, _, cx| {
                this.drag_step(Event::Drop { key: key.clone() }, cx);
            }))
            .into_any_element()
    }

    /// Whether a drag going on now would land in `place`'s group: an empty one opens as a target only then.
    pub(super) fn drag_could_land(&self, place: &Place, cx: &App) -> bool {
        self.session_drag
            .drag(cx)
            .is_some_and(|drag| super::session_drop::plan(drag, &Target::Group(place.clone())).is_some())
    }

    /// Moves the drag's machine on by `event`, and does what its step asks. A step that changes the target may arm the
    /// spring of a folded group (`session_drag_edge.rs`).
    pub(super) fn drag_step(&mut self, event: Event, cx: &mut Context<Self>) {
        match self.session_drag.step(event) {
            Step::Same => {}
            Step::Changed => {
                self.arm_spring(cx);
                cx.notify();
            }
            Step::Land(landing) => {
                self.session_drag.stop_assists();
                self.land_session(&landing.drag, &landing.target, &landing.plan, cx);
                cx.notify();
            }
        }
    }

    /// Esc during a session drag: GPUI lets go of it, and nothing lands. False when no drag goes on.
    pub(super) fn cancel_session_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.session_drag.drag(cx).is_none() {
            return false;
        }
        cx.stop_active_drag(window);
        self.session_drag.stop_assists();
        self.drag_step(Event::Cancel, cx);
        true
    }

    /// The other picked sessions a drag of `id` carries: none unless `id` is picked with others. In the
    /// sidebar's order, then the picked ones it does not draw (put aside, archived).
    fn carried_with(&self, id: &str) -> Vec<Facts> {
        let picked = &self.sidebar_filter.picked;
        if !picked.has(id) || self.picked_count() < 2 {
            return Vec::new();
        }
        let drawn = self.drawn_code_ids();
        let mut ids: Vec<String> = drawn.iter().filter(|one| picked.has(one)).cloned().collect();
        let mut rest: Vec<String> = self.picked_ids().into_iter().filter(|one| !ids.contains(one)).collect();
        rest.sort();
        ids.extend(rest);
        let now = crate::code::now_ms();
        ids.iter()
            .filter(|one| *one != id)
            .filter_map(|one| self.session_by_id(one))
            .map(|session| Facts {
                id: session.id.clone(),
                project: session.project_id.clone(),
                from: if session.archived {
                    Place::Archived
                } else {
                    match session.aside(now) {
                        Some(aside) => Place::Aside(aside),
                        None => Place::Project(session.project_id.clone()),
                    }
                },
                pinned: session.pinned,
                linked: session.parent.is_some(),
                with: Vec::new(),
            })
            .collect()
    }

    /// `drag` landed: its project's new order, then what the menus do to each session it carries, with one Undo for
    /// what they changed.
    fn land_session(&mut self, drag: &Facts, target: &Target, plan: &Plan, cx: &mut Context<Self>) {
        let count = drag.all().count();
        if count > 1 {
            self.sidebar_filter.picked.clear();
        }
        let title = self
            .session_by_id(&drag.id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        let mut order_before = None;
        if let Some(spot) = &plan.spot {
            let listed: Vec<&str> = spot.listed.iter().map(String::as_str).collect();
            let dragged: Vec<&str> = drag.all().map(|one| one.id.as_str()).collect();
            if let Some(order) = super::session_order::moved_all(&listed, &dragged, &spot.id, spot.after) {
                let kept = self.groups.groups.session_order(&spot.project).to_vec();
                order_before = Some((spot.project.clone(), kept));
                self.groups.groups.set_session_order(&spot.project, order);
                self.save_groups();
            }
        }
        // Each session goes where it goes from where it is; one that is there already stays.
        let plans: Vec<(&Facts, Plan)> = drag
            .all()
            .filter_map(|one| Some((one, super::session_drop::plan_one(one, target)?)))
            .collect();
        if plan.put == Put::Archive {
            // `archive.rs` offers its own Undo.
            self.archive_with_undo(plans.iter().map(|(one, _)| one.id.clone()).collect(), cx);
            return;
        }
        let now = chrono::Local::now();
        // A drop on "Snoozed" itself has no time: the snooze menu's "Until tomorrow morning".
        let default_at = crate::snooze::presets(now)
            .into_iter()
            .find(|(label, _)| *label == "Until tomorrow morning")
            .map(|(_, at)| at);
        let mut undo = Vec::new();
        for (one, own) in &plans {
            let Some(args) = super::session_drop::changes(&one.id, &one.from, own, default_at) else {
                continue;
            };
            if let Some(session) = self.session_by_id(&one.id) {
                let before = super::session_drop_undo::Before {
                    pinned: session.pinned,
                    settled: session.settled,
                    snoozed_until: session.snoozed_until,
                    archived: session.archived,
                };
                undo.push(super::session_drop_undo::DropUndo::of(
                    &args,
                    before,
                    order_before.take(),
                ));
            }
            self.change_session(args, cx);
        }
        let at = match plan.put {
            Put::Snooze(at) => at.or(default_at),
            _ => None,
        };
        let wake = at.map(|at| crate::snooze::wake_text(at, now));
        let text = super::session_drop_undo::notice_text(plan, &title, count, wake.as_deref());
        if let Some(text) = text.filter(|_| !undo.is_empty()) {
            self.show_notice(
                text,
                Some(("Undo", super::notice_host::NoticeDo::UndoDrop(undo))),
                nexus_desktop_core::notice::UNDO,
                cx,
            );
        }
    }
}

/// The line where a dropped session lands: over the row's top edge, or under its bottom one. It grows in from the
/// title's start, so the eye finds it where the title will be. The session lands at the top level, so the line does
/// too.
fn drop_line(id: &str, after: bool, palette: Palette) -> impl IntoElement {
    div()
        .absolute()
        .left(px(8.))
        .right(px(8.))
        .h(px(2.))
        .when(after, |line| line.bottom(px(-1.)))
        .when(!after, |line| line.top(px(-1.)))
        .rounded_full()
        .bg(palette.primary)
        .with_animation(
            SharedString::from(format!("session-drop-line-{id}-{after}")),
            Animation::new(motion::UI).with_easing(motion::ease_out),
            |line, delta| line.opacity(delta).mr(relative(1. - delta)),
        )
}

#[cfg(test)]
#[path = "session_drag_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn slot(top: &str, last: &str) -> RowSlot {
        RowSlot {
            project: "repo".into(),
            members: ["repo".to_string()].into_iter().collect(),
            listed: ["lead", "child", "other"].map(String::from).into_iter().collect(),
            top: top.into(),
            last: last.into(),
            pinned: false,
        }
    }

    #[test]
    fn a_family_takes_a_drop_before_its_top_or_after_its_last_row() {
        let family = slot("lead", "child");
        let above = family.spot("lead", false);
        assert_eq!((above.id.as_str(), above.after), ("lead", false));
        assert_eq!(family.line("lead", &above), Some(false), "over the top row's top edge");
        assert_eq!(family.line("child", &above), None);
        for (row, lower) in [("lead", true), ("child", false), ("child", true)] {
            let below = family.spot(row, lower);
            assert_eq!((below.id.as_str(), below.after), ("lead", true), "{row} {lower}");
            assert_eq!(family.line("child", &below), Some(true), "under the last row");
            assert_eq!(family.line("lead", &below), None, "never inside the family");
        }
        let other = slot("other", "other");
        assert_eq!(other.line("other", &above), None, "another family's spot");
    }
}
