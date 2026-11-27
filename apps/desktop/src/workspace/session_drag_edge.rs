//! What a session drag does near the sidebar's edges and over a folded group, with no UI.
//!
//! Near the list's top or bottom edge, the list scrolls, faster the nearer the pointer is to the edge, as Finder's and
//! VS Code's lists do. Held over a folded project or group for [`SPRING`], the group opens, as a spring-loaded folder
//! does.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;
use super::session_drop::{Place, Target};
use crate::motion;

/// How long a dragged session must rest over a folded group before it opens: long enough not to open every group
/// the pointer crosses.
pub(super) const SPRING: Duration = Duration::from_millis(600);
/// How close to the list's edge the scroll starts.
const EDGE: f32 = 40.;
/// The fastest scroll, in pixels a frame.
const MAX_SPEED: f32 = 14.;
/// How often the list scrolls while the pointer rests near its edge: 60 times a second.
pub(super) const SCROLL_FRAME: Duration = Duration::from_millis(16);

/// How far the list scrolls a frame with the pointer at `y`, in a list from `top` to `bottom`: up (below 0) near the
/// top, down (above 0) near the bottom, faster nearer the edge, and 0 elsewhere. Past the edge, it scrolls at its
/// fastest; further than one band past it, the pointer is elsewhere, and it does not scroll.
pub(super) fn edge_speed(y: f32, top: f32, bottom: f32) -> f32 {
    let band = EDGE.min((bottom - top) / 4.);
    if band <= 0. || y < top - band || y > bottom + band {
        return 0.;
    }
    if y < top + band {
        -MAX_SPEED * ((top + band - y) / band).min(1.)
    } else if y > bottom - band {
        MAX_SPEED * ((y - (bottom - band)) / band).min(1.)
    } else {
        0.
    }
}

/// The scroll offset after one frame at `speed`: GPUI's offsets go below 0 as the list scrolls down, and stay in
/// `-max..=0`.
pub(super) fn scrolled(offset: f32, speed: f32, max: f32) -> f32 {
    (offset - speed).clamp(-max.max(0.), 0.)
}

/// What opens if the drag rests over `target`: a folded project or a closed group. `folded` says whether that place
/// is folded now. "Snoozed" springs even when open: resting there shows the snooze presets as targets.
pub(super) fn spring(target: &Target, folded: impl Fn(&Place) -> bool) -> Option<Place> {
    let place = target.place()?;
    (folded(&place) || place == Place::Aside(crate::code::Aside::Snoozed)).then_some(place)
}

impl Workspace {
    /// Whether `place` is folded now: a project folded by hand, or a group closed.
    fn folded(&self, place: &Place) -> bool {
        match place {
            Place::Project(lead) => self.folded_projects.contains(lead),
            Place::Aside(group) => !self.open_aside.contains(group),
            Place::Archived => !self.archived_open,
        }
    }

    /// The drag's target changed: if it rests over a folded group, that group opens after [`SPRING`], unless the
    /// pointer leaves it first. Any other target lets go of the spring.
    pub(super) fn arm_spring(&mut self, cx: &mut Context<Self>) {
        let armed = self
            .session_drag
            .phase_over()
            .and_then(|(key, target)| spring(target, |place| self.folded(place)).map(|place| (key.to_string(), place)));
        self.session_drag.spring = armed.map(|(key, place)| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SPRING).await;
                let opened = this.update(cx, |this, cx| {
                    let still = this.session_drag.phase_over().is_some_and(|(over, _)| over == key);
                    if still && cx.has_active_drag() {
                        if this.folded(&place) {
                            this.unfold(&place, cx);
                        }
                        if place == Place::Aside(crate::code::Aside::Snoozed) {
                            this.session_drag.presets = Some(crate::snooze::presets(chrono::Local::now()));
                            cx.notify();
                        }
                    }
                    this.session_drag.spring = None;
                });
                if let Err(error) = opened {
                    eprintln!("nexus-desktop-gpui: a folded group could not spring open: {error}");
                }
            })
        });
    }

    /// Opens `place` as a click on its head does.
    fn unfold(&mut self, place: &Place, cx: &mut Context<Self>) {
        match place {
            Place::Project(lead) => {
                self.folded_projects.remove(lead);
                self.project_fold_motion
                    .entry(lead.clone())
                    .or_insert_with(|| super::folder_motion::Transition::settled(0.))
                    .retarget(1., motion::UI_LONG);
            }
            Place::Aside(group) => {
                self.open_aside.insert(*group);
                self.aside_fold_motion
                    .entry(*group)
                    .or_insert_with(|| super::folder_motion::Transition::settled(0.))
                    .retarget(1., motion::UI_LONG);
            }
            Place::Archived => {
                self.archived_open = true;
                self.load_archived(cx);
                self.archived_fold_motion.retarget(1., motion::UI_LONG);
            }
        }
        cx.notify();
    }

    /// The pointer moved during a session drag, at `position`, over the list in `bounds`. Near the list's edge, it
    /// scrolls until the pointer moves away, the drag ends, or the list can go no further.
    pub(super) fn drag_near_edge(&mut self, position: Point<Pixels>, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.session_drag.drag(cx).is_none() {
            return;
        }
        let across = position.x >= bounds.left() && position.x <= bounds.right();
        let speed = if across {
            edge_speed(
                f32::from(position.y),
                f32::from(bounds.top()),
                f32::from(bounds.bottom()),
            )
        } else {
            0.
        };
        self.session_drag.scroll_speed = speed;
        if speed == 0. || self.session_drag.scroll.is_some() {
            return;
        }
        self.session_drag.scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SCROLL_FRAME).await;
                let going = this.update(cx, |this, cx| {
                    let speed = this.session_drag.scroll_speed;
                    if speed == 0. || !cx.has_active_drag() {
                        this.session_drag.scroll = None;
                        return false;
                    }
                    let handle = &this.sidebar_filter.scroll;
                    let offset = handle.offset();
                    let max = f32::from(handle.max_offset().y);
                    let next = scrolled(f32::from(offset.y), speed, max);
                    if next != f32::from(offset.y) {
                        handle.set_offset(point(offset.x, px(next)));
                        // The rows moved under a still pointer: the target it was over is no longer under it, and
                        // GPUI says where it is only when it moves again.
                        let over = this
                            .session_drag
                            .phase_over()
                            .map(|(key, target)| (key.to_string(), target.clone()));
                        if let Some((key, target)) = over {
                            this.drag_step(
                                super::session_drag::Event::Move {
                                    key,
                                    inside: false,
                                    target,
                                },
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    true
                });
                if !going.unwrap_or(false) {
                    break;
                }
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;
    use crate::code::Aside;

    #[test]
    fn the_list_scrolls_only_near_its_edges_and_faster_nearer_them() {
        assert_eq!(edge_speed(300., 0., 600.), 0., "in the middle");
        assert!(edge_speed(30., 0., 600.) < 0., "near the top: up");
        assert!(edge_speed(570., 0., 600.) > 0., "near the bottom: down");
        assert!(
            edge_speed(5., 0., 600.) < edge_speed(30., 0., 600.),
            "faster nearer the edge"
        );
        assert_eq!(edge_speed(-10., 0., 600.), -MAX_SPEED, "just past the edge: fastest");
        assert_eq!(edge_speed(-200., 0., 600.), 0., "far past it: elsewhere");
        assert_eq!(edge_speed(10., 0., 0.), 0., "no list");
    }

    #[test]
    fn the_scroll_stays_inside_the_list() {
        assert_eq!(scrolled(0., -14., 500.), 0., "already at the top");
        assert_eq!(scrolled(-100., 14., 500.), -114.);
        assert_eq!(scrolled(-495., 14., 500.), -500., "at the bottom");
        assert_eq!(scrolled(0., 14., 0.), 0., "a list that fits");
    }

    #[test]
    fn only_a_folded_place_springs_open() {
        let settled = Target::Group(Place::Aside(Aside::Settled));
        assert_eq!(spring(&settled, |_| true), Some(Place::Aside(Aside::Settled)));
        assert_eq!(spring(&settled, |_| false), None, "open already");
        let snoozed = Target::Group(Place::Aside(Aside::Snoozed));
        assert_eq!(
            spring(&snoozed, |_| false),
            Some(Place::Aside(Aside::Snoozed)),
            "open, it still shows its presets"
        );
        let row = Target::Row(super::super::session_drop::Spot {
            project: "p".into(),
            members: ["p".to_string()].into_iter().collect(),
            listed: ["a".to_string()].into_iter().collect(),
            id: "a".into(),
            after: false,
            pinned: false,
        });
        assert_eq!(spring(&row, |_| true), None, "a row never folds");
    }
}
