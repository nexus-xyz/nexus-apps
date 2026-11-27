//! A veiled page draws no frame and no white until it shows, and says once that it shows.

use core::prelude::v1::test;
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

use super::{WebEvent, WebView, veil_state};

#[test]
fn a_veiled_page_draws_nothing_until_it_shows_and_never_the_white() {
    // (draws frames, white under them)
    assert_eq!(veil_state(false, false), (true, true), "a browser tab is as before");
    assert_eq!(veil_state(true, false), (false, false));
    assert_eq!(veil_state(true, true), (true, false));
}

#[gpui_kit::test]
fn a_clear_page_is_veiled_and_other_pages_are_not(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let startup = super::Startup {
        source: "",
        binding: None,
    };
    cx.add_window(|window, cx| {
        let clear = cx.new(|cx| WebView::clear_with_startup("about:blank", startup, window, cx));
        let plain = cx.new(|cx| WebView::with_startup("about:blank", startup, window, cx));
        clear.read_with(cx, |web, _| {
            assert!(web.is_clear());
            // No white under the page, ever: the app's surface shows where the page paints nothing.
            assert_eq!(veil_state(web.veiled, web.revealed).1, false);
        });
        plain.read_with(cx, |web, _| {
            assert!(!web.is_clear());
            assert!(!web.veiled, "an ordinary page draws as before");
        });
        gpui_kit::base::Root::new(clear, window, cx)
    });
}

#[gpui_kit::test]
fn a_veiled_page_says_once_that_it_shows(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let shown = Rc::new(Cell::new(0));
    let window = cx.add_window(|window, cx| {
        let web = cx.new(|cx| {
            let mut web = WebView::document("about:blank", window, cx);
            web.veil();
            web
        });
        let counted = shown.clone();
        cx.subscribe(&web, move |_, _, event: &WebEvent, _| {
            if matches!(event, WebEvent::Shown) {
                counted.set(counted.get() + 1);
            }
        })
        .detach();
        // Not drawn yet: the slow timer leaves it veiled.
        web.update(cx, |web, cx| web.reveal_if_drawn(cx));
        assert_eq!(shown.get(), 0);
        web.update(cx, |web, cx| {
            web.drew = true;
            web.reveal_if_drawn(cx);
            web.reveal(cx);
        });
        gpui_kit::base::Root::new(web, window, cx)
    });
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.run_until_parked();
    assert_eq!(shown.get(), 1);
}
