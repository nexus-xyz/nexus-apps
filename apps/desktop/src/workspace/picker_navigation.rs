//! Shared cursor and search transitions for the Code and Chat model pickers.

use gpui_kit::component::input::InputState;
use gpui_kit::{App, Entity, Window};

/// Empty lists use zero as a sentinel; a nonempty list always has a valid cursor.
pub(super) fn clamp(cursor: usize, count: usize) -> usize {
    cursor.min(count.saturating_sub(1))
}

/// Move from a valid current row, including after the list has shrunk.
pub(super) fn step(cursor: usize, count: usize, down: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let cursor = clamp(cursor, count);
    if down {
        if cursor == count - 1 { 0 } else { cursor + 1 }
    } else if cursor == 0 {
        count - 1
    } else {
        cursor - 1
    }
}

/// Catalog and Favorites updates preserve the highlighted identity when it still exists.
pub(super) fn reconcile(cursor: usize, old: &[String], new: &[String]) -> usize {
    old.get(cursor)
        .and_then(|key| new.iter().position(|candidate| candidate == key))
        .unwrap_or_else(|| clamp(cursor, new.len()))
}

/// Queued duplicate input notifications must not reset a restored cursor.
pub(super) fn query_changed(query: &mut String, value: &str) -> bool {
    if query == value {
        return false;
    }
    query.clear();
    query.push_str(value);
    true
}

/// Keep the stored query, visible input, and focus in the same state.
pub(super) fn clear_search(query: &mut String, input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    query.clear();
    input.update(cx, |state, cx| {
        state.set_value("", window, cx);
        state.focus(window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::{clamp, clear_search, query_changed, reconcile, step};
    use core::prelude::v1::test;
    use gpui_kit::component::input::{Input, InputEvent, InputState};
    use gpui_kit::{
        AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
        ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, TestAppContext,
        VisualTestContext, Window, div, point, px,
    };

    #[test]
    fn cursor_navigation_covers_empty_singleton_shrunk_and_wrapped_lists() {
        for count in 0..12 {
            for cursor in [0, 1, 2, 8, 20, usize::MAX] {
                let valid = clamp(cursor, count);
                if count == 0 {
                    assert_eq!(valid, 0);
                    assert_eq!(step(cursor, count, true), 0);
                    assert_eq!(step(cursor, count, false), 0);
                } else {
                    assert!(valid < count);
                    assert_eq!(step(step(cursor, count, true), count, false), valid);
                    assert_eq!(step(step(cursor, count, false), count, true), valid);
                    assert!(step(cursor, count, true) < count);
                    assert!(step(cursor, count, false) < count);
                }
            }
        }
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(9, 2, true), 0);
    }

    #[test]
    fn catalog_reordering_preserves_identity_and_removal_clamps_the_cursor() {
        let keys = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        let old = keys(&["opus", "sonnet", "haiku"]);
        assert_eq!(reconcile(1, &old, &keys(&["haiku", "opus", "sonnet"])), 2);
        assert_eq!(reconcile(2, &old, &keys(&["opus", "sonnet"])), 1);
        assert_eq!(reconcile(0, &old, &keys(&["sonnet", "haiku"])), 0);
        assert_eq!(reconcile(2, &old, &[]), 0);
        assert_eq!(reconcile(1, &[], &old), 1);
        assert_eq!(reconcile(usize::MAX, &old, &old), 2);
    }

    #[test]
    fn only_changed_input_resets_navigation_even_for_whitespace() {
        let mut query = "opus".to_string();
        assert!(!query_changed(&mut query, "opus"));
        assert!(query_changed(&mut query, "  "));
        assert!(query.trim().is_empty());
        assert!(query_changed(&mut query, ""));
        assert!(!query_changed(&mut query, ""));
    }

    struct ScrollingRows {
        cursor: usize,
        count: usize,
        scroll: ScrollHandle,
        focus: FocusHandle,
    }

    impl Render for ScrollingRows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("picker-scroll-test")
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let down = match event.keystroke.key.as_str() {
                        "down" => true,
                        "up" => false,
                        _ => return,
                    };
                    this.cursor = step(this.cursor, this.count, down);
                    this.scroll.scroll_to_item(this.cursor);
                    cx.notify();
                }))
                .w(px(240.))
                .h(px(110.))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children((0..self.count).map(|ix| {
                    div()
                        .id(("picker-scroll-row", ix))
                        .debug_selector(move || format!("picker-scroll-row-{ix}"))
                        .flex_none()
                        .w_full()
                        .h(px(if ix % 2 == 0 { 28. } else { 52. }))
                }))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn native_variable_height_rows_stay_visible_after_arrows_wrap_and_reset(cx: &mut TestAppContext) {
        let (rows, cx) = cx.add_window_view(|_, cx| ScrollingRows {
            cursor: 0,
            count: 8,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        });
        cx.update(|window, cx| rows.update(cx, |rows, cx| rows.focus.focus(window, cx)));
        draw(cx);
        for key in [
            "up", "down", "down", "down", "down", "down", "down", "down", "down", "down", "up",
        ] {
            cx.simulate_keystrokes(key);
            draw(cx);
            let (cursor, scroll) = rows.read_with(cx, |rows, _| (rows.cursor, rows.scroll.clone()));
            let item = scroll
                .bounds_for_item(cursor)
                .expect("the active row is a direct scroll child");
            let viewport = scroll.bounds();
            let offset = scroll.offset();
            assert!(
                item.top() + offset.y >= viewport.top(),
                "{key}: row {cursor} starts above the viewport"
            );
            assert!(
                item.bottom() + offset.y <= viewport.bottom(),
                "{key}: row {cursor} ends below the viewport"
            );
        }
        rows.update(cx, |rows, cx| {
            rows.cursor = 0;
            rows.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            rows.read_with(cx, |rows, _| rows.scroll.offset()),
            point(px(0.), px(0.))
        );
    }

    struct SearchControl {
        input: Entity<InputState>,
        query: String,
        cursor: usize,
        _subscription: Subscription,
    }

    impl Render for SearchControl {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(240.)).child(Input::new(&self.input))
        }
    }

    #[gpui_kit::test]
    fn native_input_clear_keeps_focus_and_ignores_queued_duplicate_notifications(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (control, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let subscription = cx.subscribe_in(&input, window, |this: &mut SearchControl, state, event, _, cx| {
                if let InputEvent::Change = event {
                    let value = state.read(cx).value().to_string();
                    if query_changed(&mut this.query, &value) {
                        this.cursor = 0;
                    }
                }
            });
            SearchControl {
                input,
                query: String::new(),
                cursor: 3,
                _subscription: subscription,
            }
        });
        cx.update(|window, cx| {
            control.update(cx, |control, cx| {
                control.input.update(cx, |input, cx| {
                    input.focus(window, cx);
                });
            })
        });
        draw(cx);
        cx.simulate_input("opus");
        draw(cx);
        assert_eq!(
            control.read_with(cx, |control, _| (control.query.clone(), control.cursor)),
            ("opus".into(), 0)
        );
        cx.update(|window, cx| {
            control.update(cx, |control, cx| {
                // A notification from a preceding edit can arrive after the rail reset.
                control.input.update(cx, |_, cx| cx.emit(InputEvent::Change));
                clear_search(&mut control.query, &control.input, window, cx);
                control.cursor = 2;
            })
        });
        draw(cx);
        let (query, value, cursor, focused) = cx.update(|window, cx| {
            control.read_with(cx, |control, cx| {
                let input = control.input.read(cx);
                (
                    control.query.clone(),
                    input.value().to_string(),
                    control.cursor,
                    input.focus_handle(cx).is_focused(window),
                )
            })
        });
        assert_eq!((query.as_str(), value.as_str(), cursor), ("", "", 2));
        assert!(focused, "rail selection keeps the search input focused");
    }
}
