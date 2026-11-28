//! The cue on the sidebar list's edge that points to a session that needs the person out of the list's sight: below
//! or above its scrolled view, or inside a folded project. A session
//! that needs the person keeps its place in the list, so its row can sit below the fold.
//!
//! The list already records where each waiting mark lands (`sidebar_marks.rs`): a waiting session's row, and a folded
//! project's head that shows a wait inside it. A probe in this view reads those places after the list's prepaint, and
//! asks for an update only when they changed. The rule ([`out_of_view`]) is a plain function. The cue is a view of its
//! own, so its entrance redraws only itself.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sidebar::{ROW, ROW_TEXT};
use super::sidebar_marks::Marks;
use super::{Workspace, glyph};
use crate::code::{CodeSession, Status};
use crate::i18n::tr;
use crate::motion;
use crate::palette::Palette;

#[cfg(test)]
#[path = "sidebar_edge_cue_tests.rs"]
mod tests;

/// The cue's inset from the list's sides and from the edge it sits on.
const INSET: f32 = 8.;
/// How far the cue rises as it comes in.
const RISE: f32 = 4.;
/// The still dot before "Needs you".
const DOT: f32 = 6.;
/// The mark id a folded project's head gives its state (`sidebar.rs`, `render_projects`).
const PROJECT_MARK: &str = "project-";
/// How long a folded project may take to unfold before its session opens anyway.
const UNFOLD_WAIT: Duration = Duration::from_millis(600);

/// The edge of the list the cue sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    Top,
    Bottom,
}

/// Where a waiting session is in the window, top to bottom: its row's mark, or its folded project's head.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spot {
    pub top: f32,
    pub bottom: f32,
    pub folded: bool,
}

impl Spot {
    fn of(bounds: Bounds<Pixels>, folded: bool) -> Self {
        Spot {
            top: bounds.top().into(),
            bottom: bounds.bottom().into(),
            folded,
        }
    }

    fn middle(self) -> f32 {
        (self.top + self.bottom) / 2.
    }
}

/// The waiting sessions out of sight: how many, the nearest one's index, and the edge it lies beyond.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Out {
    pub nearest: usize,
    pub edge: Edge,
    pub count: usize,
}

/// Which waiting sessions the list's view (`top` to `bottom`) does not show, and which is nearest. A row is in view
/// when its mark's middle is. A session in a folded project is out of view wherever its head is: it lies beyond the
/// edge its head lies beyond, and below when its head is in view. Nearest is the least distance from the view; the
/// first in list order wins a tie.
pub(super) fn out_of_view(spots: &[Spot], top: f32, bottom: f32) -> Option<Out> {
    let mut count = 0;
    let mut nearest: Option<(f32, usize, Edge)> = None;
    for (ix, spot) in spots.iter().enumerate() {
        let middle = spot.middle();
        let (distance, edge) = if middle < top {
            (top - middle, Edge::Top)
        } else if middle > bottom {
            (middle - bottom, Edge::Bottom)
        } else if spot.folded {
            (0., Edge::Bottom)
        } else {
            continue;
        };
        count += 1;
        if nearest.is_none_or(|(best, _, _)| distance < best) {
            nearest = Some((distance, ix, edge));
        }
    }
    nearest.map(|(_, nearest, edge)| Out { nearest, edge, count })
}

/// The list's scroll offset (`offset`, 0 down to `-max`) that brings a session's row (`spot`) to the middle of the
/// view (`top` to `bottom`), or as near as the list scrolls.
pub(super) fn offset_to_show(offset: f32, max: f32, spot: Spot, top: f32, bottom: f32) -> f32 {
    (offset + (top + bottom) / 2. - spot.middle()).clamp(-max.max(0.), 0.)
}

/// The cue's box in its view, whose origin is `frame`'s: a row inset from the list's sides, on the list's `edge`.
pub(super) fn place(edge: Edge, list: Bounds<Pixels>, frame: Bounds<Pixels>) -> Bounds<Pixels> {
    let left = list.left() - frame.left() + px(INSET);
    let width = (list.size.width - px(INSET * 2.)).max(px(0.));
    let top = match edge {
        Edge::Top => list.top() - frame.top() + px(INSET),
        Edge::Bottom => list.bottom() - frame.top() - px(INSET + ROW),
    };
    Bounds::new(point(left, top), size(width, px(ROW)))
}

/// A waiting session the cue can point to.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Target {
    pub session: String,
    pub title: SharedString,
    pub project: SharedString,
    /// The folded project it is hidden in, to unfold.
    pub folded: Option<String>,
    pub spot: Spot,
}

/// What the cue shows: the nearest waiting session out of sight, how many are, and where the cue goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cue {
    pub edge: Edge,
    pub count: usize,
    pub target: Target,
    place: Bounds<Pixels>,
}

/// What the list showed at its last prepaint, as far as the cue reads it.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    marks: Vec<(SharedString, Bounds<Pixels>)>,
    list: Bounds<Pixels>,
    frame: Bounds<Pixels>,
    filtering: bool,
}

/// The cue's view, drawn over the sidebar's pane after its moving marks (`sidebar.rs`, `render_sidebar`).
pub(super) struct EdgeCue {
    workspace: WeakEntity<Workspace>,
    marks: Marks,
    list: ScrollHandle,
    /// The last snapshot the probe took, so a frame that changes nothing asks for no update.
    seen: Rc<RefCell<Option<Snapshot>>>,
    shown: Option<Cue>,
    /// The cue shown last, drawn while it leaves.
    last: Option<Cue>,
    /// A session in a folded project the person went to: it opens once its row is in the list.
    unfolding: Option<Target>,
    /// Opens `unfolding` if its row never shows.
    _unfold_wait: Task<()>,
}

impl EdgeCue {
    /// `marks` are the sidebar list's, and `list` its scroll.
    pub(super) fn new(workspace: &Entity<Workspace>, marks: &Marks, list: &ScrollHandle, cx: &mut App) -> Entity<Self> {
        let workspace = workspace.downgrade();
        let (marks, list) = (marks.clone(), list.clone());
        cx.new(|_| EdgeCue {
            workspace,
            marks,
            list,
            seen: Rc::default(),
            shown: None,
            last: None,
            unfolding: None,
            _unfold_wait: Task::ready(()),
        })
    }

    /// What the cue shows now; None while hidden.
    #[cfg(test)]
    pub(super) fn shown(&self) -> Option<&Cue> {
        self.shown.as_ref()
    }

    /// The list drew its waiting marks somewhere else: work out the cue again.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.seen.borrow().clone() else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        // An unfolded project's session opens once its row is in the list, brought into view.
        if let Some(target) = self.unfolding.clone()
            && let Some((_, bounds)) = snapshot.marks.iter().find(|(id, _)| id.as_ref() == target.session)
        {
            let target = Target {
                spot: Spot::of(*bounds, false),
                folded: None,
                ..target
            };
            self.unfolding = None;
            self._unfold_wait = Task::ready(());
            let list = self.list.clone();
            workspace.update(cx, |workspace, cx| workspace.go_to_waiting(&target, &list, window, cx));
            return;
        }
        let next = if self.unfolding.is_some() {
            None
        } else {
            let app: &App = cx;
            workspace.read(app).edge_cue(&snapshot, app)
        };
        if next != self.shown {
            if next.is_some() {
                self.last = next.clone();
            }
            self.shown = next;
            cx.notify();
        }
    }

    /// The person pressed the cue: go to the session it names.
    fn go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cue) = self.shown.clone() else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let target = cue.target;
        let Some(project) = target.folded.clone() else {
            let list = self.list.clone();
            workspace.update(cx, |workspace, cx| workspace.go_to_waiting(&target, &list, window, cx));
            return;
        };
        // Its row shows once the project has unfolded: it opens then (`refresh`), or after a while if it never shows.
        workspace.update(cx, |workspace, cx| workspace.fold_project(&project, false, cx));
        self.unfolding = Some(target);
        self.shown = None;
        self._unfold_wait = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(UNFOLD_WAIT).await;
            let opened = this.update_in(cx, |cue, window, cx| {
                let Some(target) = cue.unfolding.take() else {
                    return;
                };
                let list = cue.list.clone();
                if let Some(workspace) = cue.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| workspace.go_to_waiting(&target, &list, window, cx));
                }
            });
            if let Err(error) = opened {
                eprintln!("nexus-desktop-gpui: the waiting session did not open: {error:#}");
            }
        });
        cx.notify();
    }

    /// Reads where the list put its waiting marks, after the list's prepaint, and asks for an update when that changed.
    fn probe(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (marks, list, seen) = (self.marks.clone(), self.list.clone(), self.seen.clone());
        let (workspace, cue) = (self.workspace.clone(), cx.entity().downgrade());
        canvas(
            move |frame, window, cx| {
                let filtering = workspace
                    .upgrade()
                    .is_some_and(|workspace| workspace.read(cx).filtering());
                let snapshot = Snapshot {
                    marks: marks.waiting(),
                    list: list.bounds(),
                    frame,
                    filtering,
                };
                if seen.borrow().as_ref() == Some(&snapshot) {
                    return;
                }
                *seen.borrow_mut() = Some(snapshot);
                window.defer(cx, move |window, cx| {
                    if let Err(error) = cue.update(cx, |cue, cx| cue.refresh(window, cx)) {
                        eprintln!("nexus-desktop-gpui: the sidebar's edge cue was not updated: {error:#}");
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The cue's row as `phase` draws it: it rises as it comes in, and fades as it leaves.
    fn draw(&self, phase: motion::Phase, cue: Cue, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let text: SharedString = if cue.count > 1 {
            tr("{count} need you").replace("{count}", &cue.count.to_string()).into()
        } else {
            tr("Needs you")
        };
        let tip: SharedString = tr("In {project}").replace("{project}", &cue.target.project).into();
        let arrow = match cue.edge {
            Edge::Top => Lucide::ArrowUp,
            Edge::Bottom => Lucide::ArrowDown,
        };
        let row = h_flex()
            .id("sidebar-edge-cue")
            .debug_selector(|| "sidebar-edge-cue".into())
            .size_full()
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.pop)
            .shadow(palette.pop_shadow(palette.pop_ring()))
            .text_size(px(ROW_TEXT))
            .cursor_pointer()
            .occlude()
            // Still: a wait on the person never moves (render rule 6).
            .child(div().size(px(DOT)).flex_none().rounded_full().bg(palette.waiting()))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.foreground)
                    .child(text),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(palette.muted)
                    .when(cue.count == 1, |title| title.child(cue.target.title.clone())),
            )
            .child(glyph(arrow, palette.icon).size(px(14.)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .when(phase.open(), |row| {
                row.on_click(cx.listener(|cue, _, window, cx| cue.go(window, cx)))
            });
        let (epoch, leaving) = match phase {
            motion::Phase::Open { epoch } => (epoch, None),
            motion::Phase::Leaving { since, epoch } => (epoch, Some(since)),
        };
        let fade = leaving.map_or(0., |since| {
            motion::ease_out((since.elapsed().as_secs_f32() / motion::UI_EXIT.as_secs_f32()).clamp(0., 1.))
        });
        let place = cue.place;
        div()
            .absolute()
            .left(place.left())
            .top(place.top())
            .w(place.size.width)
            .h(place.size.height)
            .child(
                div()
                    .relative()
                    .size_full()
                    .child(row)
                    .when_some(leaving, |frame, since| {
                        // The entrance has ended and asks for no frames: this asks for them while the cue leaves.
                        frame.child(div().absolute().size_0().with_animation(
                            SharedString::from(format!("sidebar-edge-cue-out-{since:?}")),
                            Animation::new(motion::UI_EXIT),
                            |pump, _| pump,
                        ))
                    })
                    .with_animation(
                        ("sidebar-edge-cue", epoch),
                        Animation::new(motion::UI).with_easing(motion::ease_out),
                        move |frame, t| {
                            frame
                                .opacity(t * (1. - fade))
                                .top(px(RISE * (1. - t) + RISE * 0.5 * fade))
                        },
                    ),
            )
            .into_any_element()
    }
}

impl Render for EdgeCue {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = motion::presence_for("sidebar-edge-cue", self.shown.is_some(), motion::UI_EXIT, cx);
        let theme = self.workspace.upgrade().map(|workspace| workspace.read(cx).theme);
        let cue = phase
            .zip(self.shown.clone().or_else(|| self.last.clone()))
            .zip(theme)
            .map(|((phase, cue), theme)| {
                let palette = Palette::for_choice(theme, window.appearance());
                self.draw(phase, cue, palette, cx)
            });
        div().absolute().inset_0().child(self.probe(cx)).children(cue)
    }
}

impl Workspace {
    /// The cue for what the list showed at its last prepaint; None when no waiting session is out of sight, or while a
    /// search narrows the list.
    fn edge_cue(&self, snapshot: &Snapshot, cx: &App) -> Option<Cue> {
        let list = snapshot.list;
        if snapshot.filtering || list.size.height <= px(0.) {
            return None;
        }
        let targets = self.waiting_targets(&snapshot.marks, cx);
        let spots: Vec<Spot> = targets.iter().map(|target| target.spot).collect();
        let out = out_of_view(&spots, list.top().into(), list.bottom().into())?;
        Some(Cue {
            edge: out.edge,
            count: out.count,
            target: targets[out.nearest].clone(),
            place: place(out.edge, list, snapshot.frame),
        })
    }

    /// The Code sessions that need the person behind the list's waiting marks, in list order: a session's row, or each
    /// waiting session of a folded project, at its head. Other waiting marks (a server's) name no session.
    fn waiting_targets(&self, marks: &[(SharedString, Bounds<Pixels>)], cx: &App) -> Vec<Target> {
        let mut targets = Vec::new();
        let mut taken = HashSet::new();
        let waiting = |session: &CodeSession| self.session_mark(session, cx) == Status::NeedsYou;
        for (id, bounds) in marks {
            let folded = id
                .strip_prefix(PROJECT_MARK)
                .filter(|project| self.folded_projects.contains(*project));
            if let Some(project) = folded {
                let name = self.project_name(project);
                for session in self.repo_sessions(project).filter(|session| waiting(session)) {
                    if taken.insert(session.id.clone()) {
                        targets.push(Target {
                            session: session.id.clone(),
                            title: session.title.clone().into(),
                            project: name.clone(),
                            folded: Some(project.to_string()),
                            spot: Spot::of(*bounds, true),
                        });
                    }
                }
            } else if let Some(session) = self.session_by_id(id).filter(|session| waiting(session))
                && taken.insert(session.id.clone())
            {
                targets.push(Target {
                    session: session.id.clone(),
                    title: session.title.clone().into(),
                    project: self.project_name(&session.project_id),
                    folded: None,
                    spot: Spot::of(*bounds, false),
                });
            }
        }
        targets
    }

    /// The listed sessions of the repository whose sidebar project `lead` leads (`repo_groups.rs`): every copy's.
    fn repo_sessions<'a>(&'a self, lead: &str) -> impl Iterator<Item = &'a CodeSession> + 'a {
        let key = self
            .projects
            .iter()
            .find(|project| project.id == lead)
            .and_then(|project| super::repo_groups::repo_key(&project.remotes));
        let lead = lead.to_string();
        self.projects
            .iter()
            .filter(move |project| {
                project.id == lead
                    || key
                        .as_ref()
                        .is_some_and(|key| super::repo_groups::repo_key(&project.remotes).as_ref() == Some(key))
            })
            .flat_map(|project| project.sessions.iter())
            .filter(|session| !session.archived && !session.settled && session.snoozed_until.is_none())
    }

    pub(super) fn project_name(&self, id: &str) -> SharedString {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| SharedString::from(project.name.clone()))
            .unwrap_or_default()
    }

    /// Bring a waiting session's row into the list's view and open it, as a click on its row does.
    fn go_to_waiting(&mut self, target: &Target, list: &ScrollHandle, window: &mut Window, cx: &mut Context<Self>) {
        if target.folded.is_none() {
            let view = list.bounds();
            let offset = list.offset();
            let y = offset_to_show(
                offset.y.into(),
                list.max_offset().y.into(),
                target.spot,
                view.top().into(),
                view.bottom().into(),
            );
            list.set_offset(point(offset.x, px(y)));
        }
        self.open_code(&target.session, window, cx);
        cx.notify();
    }
}
