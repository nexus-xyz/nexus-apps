//! Menus and popovers that leave as they came: a short fade back toward their trigger, instead of vanishing. GPUI
//! draws only what the view's state says, so a menu closed in the state is gone in the same frame. [`presence`] keeps
//! a closed one on screen for [`super::MENU_OUT`], and its caller draws it once more, leaving (Zeron's `Popup` and
//! `reap_popup`, `ui/src/popover.rs`).
//!
//! The record is per window view and key, kept here rather than in each menu's state, so a menu's `bool` or `Option`
//! stays as it is: the caller asks with what it has, and gets what to draw.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, EntityId, IntoElement, SharedString, Styled, div, px,
};

use super::{MENU_IN, MENU_OUT, ease_out};

/// What to draw of a menu now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Open, its `epoch`-th opening: it plays its entrance once per opening.
    Open { epoch: usize },
    /// Closed `since`, leaving: draw it once more, fading out.
    Leaving { since: Instant, epoch: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    open: bool,
    since: Instant,
    epoch: usize,
}

thread_local! {
    static SHOWN: RefCell<HashMap<(EntityId, SharedString), Shown>> = RefCell::default();
    /// What [`presence_of`]'s menus showed last, to draw them leaving after their state has let go of it.
    static LAST: RefCell<HashMap<(EntityId, SharedString), Box<dyn Any>>> = RefCell::default();
    /// The last opening's number, shared by every key: a menu's record goes once it has left, and its next opening
    /// still plays its entrance under an id no earlier opening used.
    static EPOCH: Cell<usize> = const { Cell::new(0) };
}

fn next_epoch() -> usize {
    EPOCH.with(|epoch| {
        let next = epoch.get().wrapping_add(1);
        epoch.set(next);
        next
    })
}

/// What changed for one key between two asks. `fresh` numbers a new opening. A menu that has left keeps no record.
fn step(
    shown: Option<Shown>,
    open: bool,
    now: Instant,
    reduced: bool,
    out: Duration,
    fresh: impl FnOnce() -> usize,
) -> (Option<Shown>, Option<Phase>) {
    match (shown, open) {
        (None, false) => (None, None),
        (None, true) => {
            let epoch = fresh();
            let shown = Shown {
                open: true,
                since: now,
                epoch,
            };
            (Some(shown), Some(Phase::Open { epoch }))
        }
        (Some(shown), true) => {
            let epoch = if shown.open { shown.epoch } else { fresh() };
            let shown = Shown {
                open: true,
                since: if shown.open { shown.since } else { now },
                epoch,
            };
            (Some(shown), Some(Phase::Open { epoch }))
        }
        (Some(shown), false) => {
            let shown = if shown.open {
                Shown {
                    open: false,
                    since: now,
                    epoch: shown.epoch,
                }
            } else {
                shown
            };
            if reduced || now.saturating_duration_since(shown.since) >= out {
                // Gone: drop the record (the next opening takes a new number from `fresh`).
                (None, None)
            } else {
                (
                    Some(shown),
                    Some(Phase::Leaving {
                        since: shown.since,
                        epoch: shown.epoch,
                    }),
                )
            }
        }
    }
}

/// Whether to draw the menu `key` of this view, which its state says is `open` or not. A menu that just closed
/// answers [`Phase::Leaving`] for [`MENU_OUT`]; the view hears once more when it has gone, so it draws the frame
/// without it.
pub fn presence<V: 'static>(key: impl Into<SharedString>, open: bool, cx: &mut Context<V>) -> Option<Phase> {
    presence_for(key, open, MENU_OUT, cx)
}

/// [`presence`] for a thing that leaves over `out` rather than a menu's [`MENU_OUT`] (a cue leaves over
/// [`super::UI_EXIT`]).
pub fn presence_for<V: 'static>(
    key: impl Into<SharedString>,
    open: bool,
    out: Duration,
    cx: &mut Context<V>,
) -> Option<Phase> {
    let key = (cx.entity_id(), key.into());
    let now = Instant::now();
    let reduced = cx.reduce_motion();
    let (before, phase) = SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        let before = shown.get(&key).copied();
        if before.is_none() && !open {
            return (None, None);
        }
        let (next, phase) = step(before, open, now, reduced, out, next_epoch);
        match next {
            Some(next) => shown.insert(key.clone(), next),
            None => shown.remove(&key),
        };
        (before, phase)
    });
    // The first frame of leaving: a frame after the end, so the menu is dropped even when nothing else moves.
    if before.is_some_and(|before| before.open) && matches!(phase, Some(Phase::Leaving { .. })) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(out + Duration::from_millis(16)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    phase
}

/// [`presence`] for a menu whose state is an `Option` of what it shows (a page, a row's id): while it leaves, the
/// value it showed last.
pub fn presence_of<V: 'static, T: Clone + 'static>(
    key: impl Into<SharedString>,
    value: Option<T>,
    cx: &mut Context<V>,
) -> Option<(Phase, T)> {
    presence_of_for(key, value, MENU_OUT, cx)
}

/// [`presence_of`] for a thing that leaves over `out` (a setup line leaves over [`super::UI_EXIT`]).
pub fn presence_of_for<V: 'static, T: Clone + 'static>(
    key: impl Into<SharedString>,
    value: Option<T>,
    out: Duration,
    cx: &mut Context<V>,
) -> Option<(Phase, T)> {
    let key = key.into();
    let slot = (cx.entity_id(), key.clone());
    let open = value.is_some();
    let Some(phase) = presence_for(key, open, out, cx) else {
        // Gone (or never shown): forget what it showed.
        LAST.with(|last| last.borrow_mut().remove(&slot));
        return None;
    };
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        match value {
            Some(value) => {
                last.insert(slot, Box::new(value.clone()));
                Some((phase, value))
            }
            None => {
                let value = last.get(&slot)?.downcast_ref::<T>()?.clone();
                Some((phase, value))
            }
        }
    })
}

impl Phase {
    pub fn open(self) -> bool {
        matches!(self, Phase::Open { .. })
    }

    /// Draw `menu` as this phase: an opening fades in while moving `travel` pixels (positive: up, as a menu over the
    /// composer rises from it; negative: down, as one under its trigger drops from it), a closing fades out going
    /// half of that back. A leaving menu takes no clicks: a layer over it takes them.
    ///
    /// Both phases wrap the menu in the same animation element, `(id, epoch)`, so the menu keeps its element ids while
    /// it leaves: its own state (a scroll offset, an animation inside it) carries on instead of starting over, and a
    /// menu closed half way through its entrance leaves from where it was.
    pub fn menu(self, id: impl Into<SharedString>, travel: f32, menu: impl IntoElement) -> AnyElement {
        let id = id.into();
        let (epoch, leaving) = match self {
            Phase::Open { epoch } => (epoch, None),
            Phase::Leaving { since, epoch } => (epoch, Some(since)),
        };
        let fade = leaving.map_or(0., |since| {
            ease_out((since.elapsed().as_secs_f32() / MENU_OUT.as_secs_f32()).clamp(0., 1.))
        });
        div()
            .relative()
            .child(menu)
            .when_some(leaving, |frame, since| {
                frame
                    .child(div().id("leaving").absolute().inset_0().occlude())
                    // The entrance has ended, so it asks for no frames: this one does, until the menu has gone. The
                    // fade itself reads the clock.
                    .child(div().absolute().size_0().with_animation(
                        SharedString::from(format!("{id}-out-{since:?}")),
                        Animation::new(MENU_OUT),
                        |pump, _| pump,
                    ))
            })
            .with_animation(
                (id, epoch),
                Animation::new(MENU_IN).with_easing(ease_out),
                move |frame, t| {
                    frame
                        .opacity(t * (1. - fade))
                        .top(px(travel * (1. - t) + travel * 0.5 * fade))
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A numbering for one test, as the shared counter numbers openings.
    fn counter() -> impl FnMut() -> usize {
        let mut n = 0;
        move || {
            n += 1;
            n
        }
    }

    #[test]
    fn a_closed_menu_that_never_opened_is_nothing() {
        let mut fresh = counter();
        assert_eq!(
            step(None, false, Instant::now(), false, MENU_OUT, &mut fresh),
            (None, None)
        );
    }

    #[test]
    fn closing_leaves_for_a_moment_then_is_gone_and_forgotten() {
        let mut fresh = counter();
        let start = Instant::now();
        let (shown, phase) = step(None, true, start, false, MENU_OUT, &mut fresh);
        assert_eq!(phase, Some(Phase::Open { epoch: 1 }));
        let closed = start + Duration::from_millis(500);
        let (shown, phase) = step(shown, false, closed, false, MENU_OUT, &mut fresh);
        assert_eq!(
            phase,
            Some(Phase::Leaving {
                since: closed,
                epoch: 1
            })
        );
        let (shown, phase) = step(shown, false, closed + MENU_OUT / 2, false, MENU_OUT, &mut fresh);
        assert!(matches!(phase, Some(Phase::Leaving { .. })));
        let (shown, phase) = step(shown, false, closed + MENU_OUT, false, MENU_OUT, &mut fresh);
        assert_eq!((shown, phase), (None, None), "a menu that has left keeps no record");
        // Opened again: a new entrance, under a number no earlier opening had.
        let (_, phase) = step(shown, true, closed + MENU_OUT * 3, false, MENU_OUT, &mut fresh);
        assert_eq!(phase, Some(Phase::Open { epoch: 2 }));
    }

    #[test]
    fn reopening_while_leaving_comes_back_with_a_new_entrance() {
        let mut fresh = counter();
        let start = Instant::now();
        let (shown, _) = step(None, true, start, false, MENU_OUT, &mut fresh);
        let (shown, _) = step(shown, false, start, false, MENU_OUT, &mut fresh);
        let (_, phase) = step(
            shown,
            true,
            start + Duration::from_millis(20),
            false,
            MENU_OUT,
            &mut fresh,
        );
        assert_eq!(phase, Some(Phase::Open { epoch: 2 }));
    }

    #[test]
    fn less_motion_closes_at_once() {
        let mut fresh = counter();
        let start = Instant::now();
        let (shown, _) = step(None, true, start, true, MENU_OUT, &mut fresh);
        assert_eq!(step(shown, false, start, true, MENU_OUT, &mut fresh), (None, None));
    }

    #[test]
    fn openings_share_one_count() {
        let a = next_epoch();
        let b = next_epoch();
        assert!(b > a);
    }
}
