//! Per-window settings targets. Native controls register their stable registry anchor with the shell's scroll view.

use gpui_kit::{component::v_flex, prelude::*, *};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

/// The search anchor of a control drawn only with the signed-in page context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AccountField(pub(crate) &'static str);

#[derive(Clone)]
pub struct SettingsFields(Rc<RefCell<State>>);
struct State {
    window: AnyWindowHandle,
    scroll: ScrollHandle,
    entries: HashMap<&'static str, (ScrollAnchor, FocusHandle)>,
    pending: Option<&'static str>,
    generation: u64,
    intent: Option<&'static str>,
    highlighted: Option<(&'static str, Instant)>,
}
impl SettingsFields {
    pub fn new(window: &Window, scroll: ScrollHandle) -> Self {
        Self(Rc::new(RefCell::new(State {
            window: window.window_handle(),
            scroll,
            entries: HashMap::new(),
            pending: None,
            generation: 0,
            intent: None,
            highlighted: None,
        })))
    }
    pub fn focus(&self, id: &'static str, cx: &mut App) -> FocusHandle {
        let mut state = self.0.borrow_mut();
        let scroll = state.scroll.clone();
        state
            .entries
            .entry(id)
            .or_insert_with(|| (ScrollAnchor::for_handle(scroll), cx.focus_handle().tab_stop(true)))
            .1
            .clone()
    }
    /// A place with internal tabs can reveal the requested panel before its row is registered.
    pub fn pending(&self) -> Option<&'static str> {
        self.0.borrow().pending
    }
    /// Take a request for a place, such as a whole tab, that is not one control: no scroll, focus or highlight.
    pub fn take_place(&self, id: &'static str) -> bool {
        let mut state = self.0.borrow_mut();
        if state.pending != Some(id) {
            return false;
        }
        state.pending = None;
        state.intent = None;
        true
    }
    pub fn scroll(&self) -> ScrollHandle {
        self.0.borrow().scroll.clone()
    }
    pub fn request(&self, anchor: &'static str) {
        let mut state = self.0.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        state.pending = Some(anchor);
        state.intent = Some(anchor);
    }
    /// An explicit exact-field move not yet applied by its deferred layout/focus callback.
    pub(crate) fn pending_target(&self) -> Option<&'static str> {
        self.0.borrow().intent
    }
    /// The row a request still waits to draw (`pending`), unlike `pending_target`'s focus intent.
    #[cfg(test)]
    pub(crate) fn requested_target(&self) -> Option<&'static str> {
        self.0.borrow().pending
    }
    #[cfg(test)]
    pub(crate) fn highlighted_target(&self) -> Option<&'static str> {
        self.0.borrow().highlighted.map(|(id, _)| id)
    }
    pub fn clear(&self) {
        let mut state = self.0.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        state.intent = None;
        state.highlighted = None;
    }
    /// Register an exact row. Text fields supply their own focus handle; read-only rows use a neutral row focus.
    pub fn field(&self, id: &'static str, body: impl IntoElement, primary: Option<FocusHandle>, cx: &mut App) -> Div {
        let (anchor, focus, window, requested, arrival) = {
            let mut state = self.0.borrow_mut();
            let scroll = state.scroll.clone();
            let entry = state
                .entries
                .entry(id)
                .or_insert_with(|| (ScrollAnchor::for_handle(scroll), cx.focus_handle()));
            let (anchor, focus) = entry.clone();
            let requested = state.pending == Some(id);
            if requested {
                state.pending = None;
                state.highlighted = Some((id, Instant::now()));
            }
            let arrival = state
                .highlighted
                .filter(|(key, at)| *key == id && at.elapsed() < ARRIVAL)
                .map(|(_, at)| at);
            (anchor, focus, state.window, requested, arrival)
        };
        if requested {
            let enter_control = primary.is_none();
            let target = primary.unwrap_or_else(|| focus.clone());
            let scroll = anchor.clone();
            let state = Rc::downgrade(&self.0);
            let generation = self.0.borrow().generation;
            cx.defer(move |cx| {
                if state
                    .upgrade()
                    .is_none_or(|state| state.borrow().generation != generation)
                {
                    return;
                }

                let _ = window.update(cx, |_, window, cx| {
                    scroll.scroll_to(window, cx);
                    target.focus(window, cx);
                    if enter_control {
                        window.focus_next(cx);
                        if !target.contains_focused(window, cx) {
                            target.focus(window, cx);
                        }
                    }
                    if let Some(state) = state.upgrade() {
                        let mut state = state.borrow_mut();
                        if state.generation == generation {
                            state.intent = None;
                        }
                    }
                    window.refresh();
                });
            });
            cx.spawn(async move |cx| {
                cx.background_executor().timer(ARRIVAL).await;
                let _ = window.update(cx, |_, window, _| window.refresh());
            })
            .detach();
        }
        let palette = crate::pages::palette(cx);
        let row = v_flex()
            .id(SharedString::from(format!("settings-field-{id}")))
            .w_full()
            .min_w(px(0.))
            .track_focus(&focus)
            .tab_group()
            .tab_index(0)
            .tab_stop(false)
            .anchor_scroll(Some(anchor))
            .border_2()
            .rounded(px(8.))
            .border_color(transparent_black())
            .focus_visible(|style| style.border_color(palette.muted))
            .child(body);
        let row = match arrival {
            None => row.into_any_element(),
            // Less motion keeps the fill and the ring, still, and drops the fade.
            Some(_) if cx.reduce_motion() => arrived(row, palette, 1.).into_any_element(),
            Some(at) => {
                let hold = HOLD.as_secs_f32();
                let total = ARRIVAL.as_secs_f32();
                row.with_animation(
                    ElementId::NamedInteger(format!("settings-arrival-{id}").into(), arrival_id(at)),
                    Animation::new(ARRIVAL),
                    move |row, t| {
                        let into = t * total;
                        let shown = if into < hold {
                            1.
                        } else {
                            1. - crate::motion::ease_out((into - hold) / crate::motion::SLOW.as_secs_f32())
                        };
                        arrived(row, palette, shown)
                    },
                )
                .into_any_element()
            }
        };
        div().w_full().min_w(px(0.)).child(row)
    }
}

/// How long the arrived row holds its fill before it fades.
const HOLD: Duration = Duration::from_millis(450);
/// A row's whole arrival: the hold, then the fade over `motion::SLOW`.
const ARRIVAL: Duration = Duration::from_millis(450 + 600);

/// The row a request (or a setup fix) opened on: the `selected` fill and a 1.5 px ring, at `shown` of their strength.
fn arrived(row: Stateful<Div>, palette: crate::palette::Palette, shown: f32) -> Stateful<Div> {
    row.bg(palette.selected.opacity(shown)).shadow(vec![BoxShadow {
        color: palette.muted.opacity(shown),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(1.5),
        inset: false,
    }])
}

/// One arrival's animation, by its start: the next request on the same row plays again.
fn arrival_id(at: Instant) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    at.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod arrival_tests {
    #[test]
    fn the_arrival_holds_then_fades_over_slow() {
        assert_eq!(super::ARRIVAL, super::HOLD + crate::motion::SLOW);
    }
}
