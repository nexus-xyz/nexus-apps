//! A page opened ahead (`ready.rs`): its startup goes in before the page is taken, and the page it becomes shows only
//! once its own document loaded.

use core::prelude::v1::test;

use gpui_kit::*;

use super::{Loading, SCRIPT_ID, Startup, WebView, startup_messages};

#[test]
fn the_startup_goes_in_first_and_the_navigation_takes_the_next_id() {
    let message = |raw: &String| serde_json::from_str::<serde_json::Value>(raw).unwrap();
    let with_binding = startup_messages(Startup {
        source: "window.ready = 1",
        binding: Some("nexus"),
    });
    let methods: Vec<_> = with_binding.iter().map(|raw| message(raw)["method"].clone()).collect();
    assert_eq!(
        methods,
        [
            "Page.enable",
            "Runtime.enable",
            "Runtime.addBinding",
            "Page.addScriptToEvaluateOnNewDocument"
        ]
    );
    let ids: Vec<_> = with_binding
        .iter()
        .map(|raw| message(raw)["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, (SCRIPT_ID..SCRIPT_ID + 4).collect::<Vec<_>>());
    assert_eq!(message(&with_binding[3])["params"]["source"], "window.ready = 1");
    let plain = startup_messages(Startup {
        source: "",
        binding: None,
    });
    assert_eq!(plain.len(), 2, "no binding: the page domain and the script");
}

#[gpui_kit::test]
fn a_page_opened_ahead_shows_once_its_own_document_loaded(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let made = std::rc::Rc::new(std::cell::RefCell::new(None));
    let kept = made.clone();
    let window = cx.add_window(|window, cx| {
        let web = cx.new(|cx| {
            let mut web = WebView::document("about:blank", window, cx);
            web.held = true;
            web.hidden = true;
            web
        });
        *kept.borrow_mut() = Some(web.clone());
        gpui_kit::base::Root::new(web, window, cx)
    });
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    let web: Entity<WebView> = made.borrow_mut().take().unwrap();
    let report = |address: &str, loading: bool, cx: &mut VisualTestContext| {
        web.update_in(cx, |web, window, cx| {
            {
                let mut shared = web.shared.lock().unwrap();
                shared.frame = Some(super::Frame {
                    pixels: vec![255; 4],
                    width: 1,
                    height: 1,
                    ..Default::default()
                });
                shared.address = Some(address.into());
                shared.loading = Some(Loading {
                    loading,
                    ..Default::default()
                });
                shared.changed();
            }
            web.take_changes(window, cx);
            (web.held, web.hidden)
        })
    };
    assert_eq!(
        report("about:blank", false, cx),
        (true, true),
        "the blank page it waited on"
    );
    assert_eq!(
        report("https://page.example/", true, cx),
        (true, true),
        "its document, loading"
    );
    assert_eq!(
        report("https://page.example/", false, cx),
        (false, false),
        "loaded: it shows"
    );
    web.update_in(cx, |web, _, _| {
        assert!(web.image.is_none(), "held paints must never show the prepared blank");
        assert!(!web.drew, "the new document must provide its own visible paint");
    });
    report("https://page.example/", false, cx);
    web.update_in(cx, |web, _, _| {
        assert!(web.image.is_some(), "the unhidden document now paints");
        assert!(web.drew);
    });
}
