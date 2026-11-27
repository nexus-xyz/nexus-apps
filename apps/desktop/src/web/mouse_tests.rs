//! The buttons a page is told are held follow the real mouse, wherever the button goes up.

use core::prelude::v1::test;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::WebView;
use super::keys::{LEFT_BUTTON, MIDDLE_BUTTON, RIGHT_BUTTON};
use super::protocol::{Button, MouseAction};

const HELD: u32 = LEFT_BUTTON | MIDDLE_BUTTON | RIGHT_BUTTON;

/// A sidebar 200px wide, then the page.
struct Probe {
    page: Entity<WebView>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(div().id("probe-side").w(px(200.)).h_full())
            .child(div().flex_1().h_full().child(self.page.clone()))
    }
}

const PAGE: Point<Pixels> = Point {
    x: px(400.),
    y: px(100.),
};
const PAGE_LOWER: Point<Pixels> = Point {
    x: px(400.),
    y: px(300.),
};
const SIDE: Point<Pixels> = Point {
    x: px(50.),
    y: px(100.),
};
/// Past the window's left edge: a display to its left.
const OTHER_DISPLAY: Point<Pixels> = Point {
    x: px(-600.),
    y: px(100.),
};

fn probe(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<WebView>) {
    cx.update(gpui_kit::init);
    let mut page = None;
    let window = cx.add_window(|window, cx| {
        let web = cx.new(|cx| WebView::document("about:blank", window, cx));
        page = Some(web.clone());
        let probe = cx.new(|_| Probe { page: web });
        gpui_kit::base::Root::new(probe, window, cx)
    });
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (cx, page.expect("the page"))
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn sent(cx: &mut VisualTestContext, page: &Entity<WebView>) -> Vec<(MouseAction, u32)> {
    cx.update(|_, cx| page.update(cx, |page, _| std::mem::take(&mut page.sent)))
}

fn ups(sent: &[(MouseAction, u32)]) -> Vec<Button> {
    sent.iter()
        .filter_map(|(action, _)| match action {
            MouseAction::Up { button, .. } => Some(*button),
            _ => None,
        })
        .collect()
}

fn moves_held(sent: &[(MouseAction, u32)]) -> Vec<u32> {
    sent.iter()
        .filter(|(action, _)| matches!(action, MouseAction::Move))
        .map(|(_, flags)| flags & HELD)
        .collect()
}

/// Presses on the page and drags off it, as on the page's scrollbar.
fn press_and_drag_off(cx: &mut VisualTestContext, page: &Entity<WebView>, to: Point<Pixels>) {
    cx.simulate_mouse_down(PAGE, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(PAGE_LOWER, Some(MouseButton::Left), Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    draw(cx);
    let sent = sent(cx, page);
    assert!(
        matches!(
            sent.first(),
            Some((
                MouseAction::Down {
                    button: Button::Left,
                    ..
                },
                _
            ))
        ),
        "{sent:?}"
    );
    assert_eq!(
        moves_held(&sent),
        vec![LEFT_BUTTON],
        "the drag on the page holds the left button"
    );
}

#[gpui_kit::test]
fn a_release_on_another_display_reaches_the_page(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    press_and_drag_off(cx, &page, OTHER_DISPLAY);
    // The platform hands the window the release, off it (an X11, Wayland or macOS press keeps the pointer).
    cx.simulate_mouse_up(OTHER_DISPLAY, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(ups(&sent(cx, &page)), vec![Button::Left]);
    // Back over the page with no button held: a hover, not a drag.
    cx.simulate_mouse_move(PAGE, None, Modifiers::none());
    cx.simulate_mouse_move(PAGE_LOWER, None, Modifiers::none());
    let sent = sent(cx, &page);
    assert_eq!(ups(&sent), vec![]);
    assert_eq!(moves_held(&sent), vec![0, 0], "{sent:?}");
}

#[gpui_kit::test]
fn a_release_over_the_sidebar_reaches_the_page(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    press_and_drag_off(cx, &page, SIDE);
    cx.simulate_mouse_up(SIDE, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(ups(&sent(cx, &page)), vec![Button::Left]);
}

#[gpui_kit::test]
fn a_release_the_window_never_saw_ends_the_drag_on_the_next_move(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    press_and_drag_off(cx, &page, OTHER_DISPLAY);
    // No release at all (Windows hands a release outside to the app under the pointer once capture is lost, and a
    // compositor's window move keeps it): the next move says no button is held.
    cx.simulate_mouse_move(PAGE, None, Modifiers::none());
    let sent = sent(cx, &page);
    let up = sent.iter().position(|(action, _)| {
        matches!(
            action,
            MouseAction::Up {
                button: Button::Left,
                ..
            }
        )
    });
    let first_move = sent.iter().position(|(action, _)| matches!(action, MouseAction::Move));
    assert!(
        up.is_some() && up < first_move,
        "the release goes before the move: {sent:?}"
    );
    assert_eq!(moves_held(&sent), vec![0], "{sent:?}");
}

#[gpui_kit::test]
fn a_second_press_without_a_release_releases_first(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    press_and_drag_off(cx, &page, OTHER_DISPLAY);
    cx.simulate_mouse_down(PAGE, MouseButton::Left, Modifiers::none());
    let sent = sent(cx, &page);
    let kinds: Vec<_> = sent
        .iter()
        .filter_map(|(action, _)| match action {
            MouseAction::Up { .. } => Some("up"),
            MouseAction::Down { .. } => Some("down"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, vec!["up", "down"], "{sent:?}");
}

#[gpui_kit::test]
fn a_drag_on_the_page_keeps_its_button(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    cx.simulate_mouse_down(PAGE, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(PAGE_LOWER, Some(MouseButton::Left), Modifiers::none());
    draw(cx);
    cx.simulate_mouse_up(PAGE_LOWER, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(PAGE, None, Modifiers::none());
    let sent = sent(cx, &page);
    assert_eq!(ups(&sent), vec![Button::Left]);
    assert_eq!(moves_held(&sent), vec![LEFT_BUTTON, 0], "{sent:?}");
}

#[gpui_kit::test]
fn a_release_off_the_page_after_a_press_off_it_is_not_sent(cx: &mut TestAppContext) {
    let (cx, page) = probe(cx);
    cx.simulate_mouse_down(SIDE, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_up(SIDE, MouseButton::Left, Modifiers::none());
    draw(cx);
    let sent = sent(cx, &page);
    assert!(sent.is_empty(), "{sent:?}");
}
