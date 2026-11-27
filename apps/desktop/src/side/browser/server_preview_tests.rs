//! A server session's Browser routes its local addresses through a forward to the server (`server_preview.rs`); a
//! local session's never does.

use core::prelude::v1::test;
use std::sync::{Arc, Mutex};

use gpui_kit::*;
use nexus_browser::place::Place;

use super::Browser;
use super::server_preview::{Opener, ServerPreview};

fn theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
}

/// A stand-in for the server's forwards: it records the ports asked for and answers `answer`.
fn opener(asked: &Arc<Mutex<Vec<u16>>>, answer: Result<u16, String>) -> Opener {
    let asked = asked.clone();
    Arc::new(move |port| {
        asked.lock().expect("asked").push(port);
        let answer = answer.clone();
        Box::pin(async move { answer })
    })
}

fn browser(place: Place, opener: Option<Opener>, cx: &mut TestAppContext) -> (Entity<Browser>, &mut VisualTestContext) {
    cx.add_window_view(|window, cx| {
        let mut browser = Browser::new(
            "server-preview-test".into(),
            Some(place),
            nexus_browser::profile::Profile::new("server-preview-test", "scope", false),
            None,
            window,
            cx,
        );
        if let Some(opener) = opener {
            browser.server = Some(ServerPreview::with_opener("box".into(), opener));
        }
        browser
    })
}

/// Runs the UI until `done`, while the forward answers on the app's Tokio runtime.
fn settle(cx: &mut VisualTestContext, browser: &Entity<Browser>, done: impl Fn(&Browser) -> bool) {
    for _ in 0..200 {
        cx.run_until_parked();
        if browser.read_with(cx, |browser, _| done(browser)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the forward never answered");
}

#[gpui_kit::test]
fn a_server_sessions_local_address_opens_through_a_forward_and_shows_the_servers_address(cx: &mut TestAppContext) {
    theme(cx);
    cx.executor().allow_parking();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (browser, cx) = browser(Place::Server("m1".into()), Some(opener(&asked, Ok(41234))), cx);
    let url = "http://localhost:3000/some/path?q=1#top";
    browser.update_in(cx, |browser, window, cx| browser.open(url, window, cx));
    settle(cx, &browser, |browser| !browser.pages.is_empty());
    assert_eq!(*asked.lock().expect("asked"), [3000]);
    browser.update(cx, |browser, cx| {
        let tab = browser.tabs.current().expect("a tab").clone();
        assert_eq!(tab.url, url, "the tab keeps the server's address");
        assert_eq!(browser.address.read(cx).value().to_string(), url, "and so does the bar");
        let server = browser.server.as_ref().expect("a server's browser");
        assert_eq!(
            server.page_url(&tab.url).as_deref(),
            Some("http://127.0.0.1:41234/some/path?q=1#top"),
            "the page loads it through the forward"
        );
        assert!(browser.pages.contains_key(&tab.id));
        assert_eq!(
            server.on_server("http://127.0.0.1:41234/next?x=2"),
            "http://localhost:3000/next?x=2",
            "where the page goes next shows as the server's"
        );
        assert!(browser.problem.is_none());
    });

    // A website is not forwarded.
    browser.update_in(cx, |browser, window, cx| {
        browser.open_new("https://example.com/", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(*asked.lock().expect("asked"), [3000]);
}

#[gpui_kit::test]
fn a_forward_that_fails_says_why_and_loads_nothing(cx: &mut TestAppContext) {
    theme(cx);
    cx.executor().allow_parking();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let refused = Err("Permission denied (publickey).".to_string());
    let (browser, cx) = browser(Place::Server("m1".into()), Some(opener(&asked, refused)), cx);
    browser.update_in(cx, |browser, window, cx| {
        browser.open("http://0.0.0.0:5173/", window, cx)
    });
    settle(cx, &browser, |browser| browser.problem.is_some());
    browser.read_with(cx, |browser, _| {
        assert_eq!(
            browser.problem.as_ref().map(|problem| problem.to_string()).as_deref(),
            Some("Could not reach port 5173 on box: Permission denied (publickey).")
        );
        assert!(browser.pages.is_empty());
    });
    assert_eq!(*asked.lock().expect("asked"), [5173]);
}

#[gpui_kit::test]
fn a_local_sessions_browser_never_forwards(cx: &mut TestAppContext) {
    theme(cx);
    let (browser, cx) = browser(Place::Local, None, cx);
    browser.update_in(cx, |browser, window, cx| {
        assert!(browser.server.is_none());
        browser.open("http://localhost:3000/app", window, cx);
        // Loaded at once, on this computer: no forward to wait for.
        let tab = browser.tabs.current().expect("a tab").clone();
        assert_eq!(tab.url, "http://localhost:3000/app");
        assert!(browser.pages.contains_key(&tab.id));
        assert!(browser.forwarding.is_none());
    });
}
