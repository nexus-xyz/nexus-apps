//! Session rows that never move under the pointer, and glide when they move. While the pointer is on the
//! sidebar's card, a sidebar menu is open or a drag goes on, each Code project keeps the sessions it showed last, in
//! that order: their marks, titles and ages still change, a session that goes leaves at once, and a new one waits.
//! When the hold ends, the project shows its order now: a row that changed place glides there from where it was, and a
//! new one opens from no height as it fades in. The same glide plays on any change of order with no hold. A drop the
//! person makes shows at once, hold or not: the dropped row lands in place and the others glide.
//!
//! The rule is [`place`]. Each frame of a move is a plain function of the time since it started, as `row_exit.rs`
//! draws, so the rows and the project's fixed-height group read the same height in the same frame. Nothing moves at
//! rest, and less motion puts every row in its place at once.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use super::Workspace;
use super::overlay::Overlay;
use super::sidebar::ROW;
use crate::code::CodeSession;
use crate::motion;

/// How long a row takes to glide to its new place, or to open.
const GLIDE: Duration = motion::UI_LONG;
/// How long a drop may wait for the session list to show what it did, before the hold applies again. A drop stores the
/// project's order (`session_order.rs`), so a change of that order is how the hold knows the person moved a row.
const ADOPT: Duration = Duration::from_secs(3);

/// What a project shows, by the rule [`place`].
#[derive(Debug, Default, PartialEq)]
pub(super) struct Placed {
    /// The session ids on screen, in order.
    pub shown: Vec<String>,
    /// The rows that changed place, each with how many rows below (positive) or above (negative) its new place it
    /// was.
    pub moved: Vec<(String, f32)>,
    /// The rows that were not on screen before.
    pub arrived: Vec<String>,
}

/// What a project shows when it showed `held` last and its order is `current` now. Held, it keeps `held`, less the
/// sessions that are gone. Free, it shows `current`. A row's move counts only among the rows on screen both before and
/// after, so a row that arrives (it opens from no height) or one that leaves moves no other row.
pub(super) fn place(held: &[String], current: &[&str], holding: bool) -> Placed {
    if held.iter().map(String::as_str).eq(current.iter().copied()) {
        return Placed {
            shown: held.to_vec(),
            ..Placed::default()
        };
    }
    let now: HashSet<&str> = current.iter().copied().collect();
    if holding {
        return Placed {
            shown: held.iter().filter(|id| now.contains(id.as_str())).cloned().collect(),
            ..Placed::default()
        };
    }
    let before: HashMap<&str, usize> = held
        .iter()
        .map(String::as_str)
        .filter(|id| now.contains(id))
        .enumerate()
        .map(|(at, id)| (id, at))
        .collect();
    let moved = current
        .iter()
        .filter(|id| before.contains_key(*id))
        .enumerate()
        .filter_map(|(at, id)| {
            let was = before[id];
            (was != at).then(|| (id.to_string(), was as f32 - at as f32))
        })
        .collect();
    let arrived = current
        .iter()
        .filter(|id| !before.contains_key(*id))
        .map(|id| id.to_string())
        .collect();
    Placed {
        shown: current.iter().map(|id| id.to_string()).collect(),
        moved,
        arrived,
    }
}

/// The share of a move done `elapsed` after it started, eased; 1 once it is over.
fn progress(elapsed: Duration) -> f32 {
    motion::ease_out((elapsed.as_secs_f32() / GLIDE.as_secs_f32()).min(1.))
}

/// One project's rows as last shown, and their moves.
#[derive(Default)]
struct Shown {
    ids: Vec<String>,
    /// A gliding row's start, in pixels from its place, and when it left.
    glides: HashMap<String, (f32, Instant)>,
    /// When an arriving row started to open.
    arrivals: HashMap<String, Instant>,
    /// The order the person's drops stored, as last seen.
    dragged: Vec<String>,
    /// A drop the person just made here: the next change of order shows at once, until when, and the dropped row.
    dropped: Option<(Instant, Option<String>)>,
}

impl Shown {
    fn offset(&self, id: &str, now: Instant) -> f32 {
        self.glides.get(id).map_or(0., |(from, at)| {
            from * (1. - progress(now.saturating_duration_since(*at)))
        })
    }

    /// An arriving row's height and opacity, 0 to 1; None once it is open.
    fn opening(&self, id: &str, now: Instant) -> Option<f32> {
        let at = self.arrivals.get(id)?;
        let elapsed = now.saturating_duration_since(*at);
        (elapsed < GLIDE).then(|| progress(elapsed))
    }

    fn moving(&self, now: Instant) -> bool {
        let live = |at: &Instant| now.saturating_duration_since(*at) < GLIDE;
        self.glides.values().any(|(_, at)| live(at)) || self.arrivals.values().any(live)
    }

    /// Show `placed`: the moved rows start from where they are drawn now.
    fn take(&mut self, placed: Placed, now: Instant, reduced: bool, still: Option<&str>) {
        if reduced {
            self.glides.clear();
            self.arrivals.clear();
        } else {
            self.glides
                .retain(|_, (_, at)| now.saturating_duration_since(*at) < GLIDE);
            self.arrivals.retain(|_, at| now.saturating_duration_since(*at) < GLIDE);
            for (id, rows) in placed.moved {
                if still == Some(id.as_str()) {
                    self.glides.remove(&id);
                    continue;
                }
                let from = rows * ROW + self.offset(&id, now);
                self.glides.insert(id, (from, now));
            }
            for id in placed.arrived {
                self.arrivals.insert(id, now);
            }
        }
        self.ids = placed.shown;
    }
}

/// The row that a drop moved: the one that moved furthest from `shown`, the order on screen, to `dragged`, the order
/// the drop stored. None when no row moved more than one place: a swap of neighbours glides both.
fn dropped(shown: &[String], dragged: &[String]) -> Option<String> {
    let was: HashMap<&str, usize> = shown.iter().enumerate().map(|(at, id)| (id.as_str(), at)).collect();
    dragged
        .iter()
        .enumerate()
        .filter_map(|(at, id)| Some((was.get(id.as_str())?.abs_diff(at), id)))
        .filter(|(distance, _)| *distance > 1)
        .max_by_key(|(distance, _)| *distance)
        .map(|(_, id)| id.clone())
}

/// The sidebar's hold: whether the pointer is on the card, and each Code project's rows as last shown. The rows are
/// kept as the list is drawn (`order`), the one place that knows the order on screen; the rest only reads them.
#[derive(Default)]
pub(super) struct SidebarHold {
    /// Set by the card's pointer watch ([`SidebarHold::watch`]), outside any update.
    inside: Rc<Cell<bool>>,
    projects: RefCell<HashMap<String, Shown>>,
}

impl SidebarHold {
    /// The session ids `project` shows now, given its order `current` and `dragged`, the order the person's drops
    /// stored (`session_order.rs`). The first time it shows `current`. A new `dragged` is the person's own drop: the
    /// project's next change of order shows at once, hold or not, and the dropped row lands in place.
    pub fn order(
        &self,
        project: &str,
        current: &[&str],
        dragged: &[String],
        holding: bool,
        now: Instant,
        reduced: bool,
    ) -> Vec<String> {
        let mut projects = self.projects.borrow_mut();
        let Some(shown) = projects.get_mut(project) else {
            let ids: Vec<String> = current.iter().map(|id| id.to_string()).collect();
            projects.insert(
                project.to_string(),
                Shown {
                    ids: ids.clone(),
                    dragged: dragged.to_vec(),
                    ..Shown::default()
                },
            );
            return ids;
        };
        if shown.dragged != dragged {
            shown.dropped = Some((now + ADOPT, dropped(&shown.ids, dragged)));
            shown.dragged = dragged.to_vec();
        }
        if shown.ids.iter().map(String::as_str).eq(current.iter().copied()) {
            return shown.ids.clone();
        }
        let drop = shown.dropped.take().filter(|(until, _)| now < *until);
        let placed = place(&shown.ids, current, holding && drop.is_none());
        let still = drop.and_then(|(_, still)| still);
        shown.take(placed, now, reduced, still.as_deref());
        shown.ids.clone()
    }

    /// Whether a row of `project` moves now.
    pub fn moving(&self, project: &str, now: Instant) -> bool {
        self.projects
            .borrow()
            .get(project)
            .is_some_and(|shown| shown.moving(now))
    }

    /// The height `project`'s opening rows do not take yet, in pixels.
    pub fn shortfall(&self, project: &str, now: Instant) -> f32 {
        let projects = self.projects.borrow();
        let Some(shown) = projects.get(project) else {
            return 0.;
        };
        shown
            .arrivals
            .keys()
            .filter_map(|id| shown.opening(id, now))
            .map(|open| (1. - open) * ROW)
            .sum()
    }

    /// `row`, session `id`'s row in `project`, where it is now: gliding to its place, or opening. A row that moves asks
    /// for the next frame; one at rest is drawn as it is.
    pub fn draw(&self, project: &str, id: &str, row: AnyElement, now: Instant, window: &mut Window) -> AnyElement {
        let (offset, opening) = match self.projects.borrow().get(project) {
            Some(shown) => (shown.offset(id, now), shown.opening(id, now)),
            None => return row,
        };
        if offset == 0. && opening.is_none() {
            return row;
        }
        ask_frame(window);
        let row = if offset == 0. {
            row
        } else {
            div().relative().top(px(offset)).child(row).into_any_element()
        };
        match opening {
            Some(open) => div()
                .flex_none()
                .h(px(ROW * open))
                .overflow_hidden()
                .opacity(open)
                .child(row)
                .into_any_element(),
            None => row,
        }
    }

    /// Watches the pointer over the card it is drawn in (absolute, filling it). It reads every move, as a menu over
    /// the card does not hide the card from it, and draws the sidebar again when the pointer leaves.
    pub fn watch(&self, workspace: WeakEntity<Workspace>) -> impl IntoElement {
        let inside = self.inside.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (moved, left) = ((inside.clone(), workspace.clone()), (inside.clone(), workspace.clone()));
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        pointer(&moved.0, bounds.contains(&event.position), &moved.1, cx);
                    }
                });
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        pointer(&left.0, false, &left.1, cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

#[cfg(test)]
thread_local! {
    /// How many frames the rows asked for: a test window runs no frames of its own, so a test counts the asks.
    static ASKED: Cell<usize> = const { Cell::new(0) };
}

/// A moving row asks for the next frame.
fn ask_frame(window: &mut Window) {
    #[cfg(test)]
    ASKED.with(|asked| asked.set(asked.get() + 1));
    window.request_animation_frame();
}

/// The pointer is `now` on the card or off it. Leaving it ends the hold: the sidebar draws again.
fn pointer(inside: &Cell<bool>, now: bool, workspace: &WeakEntity<Workspace>, cx: &mut App) {
    if inside.replace(now)
        && !now
        && let Err(error) = workspace.update(cx, |_, cx| cx.notify())
    {
        eprintln!("nexus-desktop-gpui: the sidebar's hold had no window: {error}");
    }
}

impl Workspace {
    /// Whether the sidebar holds its rows: the pointer is on its card, one of its menus is open, or a drag goes on.
    pub(super) fn sidebar_held(&self, cx: &App) -> bool {
        self.sidebar_hold.inside.get()
            || cx.has_active_drag()
            || match &self.overlay {
                Some(Overlay::ProjectMenu(_) | Overlay::RowMenu(_) | Overlay::AddGroupMenu) => true,
                Some(Overlay::SessionMenu(menu)) => !menu.in_title,
                _ => false,
            }
    }

    /// `sessions`, a project's in their order now, as the sidebar shows them: in tree order (`session_tree.rs`),
    /// held while the sidebar holds.
    pub(super) fn hold_sessions<'a>(
        &self,
        project: &str,
        sessions: Vec<&'a CodeSession>,
        cx: &App,
    ) -> Vec<&'a CodeSession> {
        let tree = super::session_tree::tree(&sessions);
        let current: Vec<&str> = tree.iter().map(|(session, _)| session.id.as_str()).collect();
        let now = cx.background_executor().now();
        let dragged = self.groups.groups.session_order(project);
        let held = self.sidebar_held(cx);
        let shown = self
            .sidebar_hold
            .order(project, &current, dragged, held, now, cx.reduce_motion());
        let by_id: HashMap<&str, &'a CodeSession> = tree
            .iter()
            .map(|(session, _)| (session.id.as_str(), *session))
            .collect();
        shown.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect()
    }
}

#[cfg(test)]
#[path = "sidebar_hold_tests.rs"]
mod tests;
