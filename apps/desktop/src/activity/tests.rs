use super::*;
use core::prelude::v1::test;
use state::{PEEK, RETRACT, Reveal};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn brief_hovers_never_open_and_pinning_survives_pointer_exit() {
    let now = std::time::Instant::now();
    let mut reveal = Reveal::default();
    reveal.hover(true, now);
    assert!(!reveal.tick(now + PEEK / 2));
    reveal.hover(false, now + PEEK / 2);
    reveal.tick(now + PEEK + RETRACT);
    assert!(!reveal.open);
    reveal.hover(true, now + RETRACT * 2);
    assert!(reveal.tick(now + RETRACT * 2 + PEEK));
    reveal.toggle();
    reveal.hover(false, now + RETRACT * 3);
    reveal.tick(now + RETRACT * 4);
    assert!(reveal.open);
    reveal.retract();
    reveal.tick(now + RETRACT * 5);
    assert!(!reveal.open);
}

#[test]
fn an_unpinned_peek_retracts_after_leaving_and_reentry_cancels_it() {
    let now = std::time::Instant::now();
    let mut reveal = Reveal::default();
    reveal.hover(true, now);
    reveal.tick(now + PEEK);
    reveal.hover(false, now + PEEK);
    assert!(!reveal.tick(now + PEEK + RETRACT / 2));
    reveal.hover(true, now + PEEK + RETRACT / 2);
    reveal.tick(now + PEEK + RETRACT);
    assert!(reveal.open);
    reveal.hover(false, now + RETRACT * 2);
    assert!(reveal.tick(now + RETRACT * 3));
    assert!(!reveal.open);
}

#[test]
fn tool_copy_never_includes_arguments_or_unknown_action_text() {
    assert_eq!(action_label(Some("browser_screenshot")), "Capturing the browser tab");
    assert_eq!(
        action_label(Some("https://user:secret@host/?token=secret")),
        "Browser access active"
    );
}

#[gpui_kit::test]
fn revocation_cleans_the_indicator_and_resume_cannot_regrant_access(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
    let (browser, cx) = cx.add_window_view(|window, cx| {
        let mut browser = Browser::new(
            "activity-revoke".into(),
            None,
            nexus_browser::profile::Profile::new("activity-test", "revoke", false),
            None,
            window,
            cx,
        );
        browser.allow_agent();
        browser
    });
    browser.update(cx, |browser, cx| {
        assert!(browser.activity_status().is_some());
        browser.pause_agent(true, cx);
        assert!(browser.activity_status().unwrap().paused);
        browser.take_over_agent(cx);
        browser.pause_agent(false, cx);
        assert!(browser.activity_status().is_none());
    });
}

#[gpui_kit::test]
fn concurrent_runs_survive_each_other_and_expiry_revokes_a_paused_grant(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
    let (browser, cx) = cx.add_window_view(|window, cx| {
        let mut browser = Browser::new(
            "activity-runs".into(),
            None,
            nexus_browser::profile::Profile::new("activity-test", "runs", false),
            None,
            window,
            cx,
        );
        browser.allow_agent();
        browser
    });
    let other = cx.update(|window, cx| {
        cx.new(|cx| {
            let mut browser = Browser::new(
                "activity-other".into(),
                None,
                nexus_browser::profile::Profile::new("activity-test", "other", false),
                None,
                window,
                cx,
            );
            browser.allow_agent();
            browser
        })
    });
    let first = Arc::new(AtomicBool::new(true));
    let second = Arc::new(AtomicBool::new(true));
    let entity = browser.clone();
    let other_entity = other.clone();
    let flag = first.clone();
    let second_flag = second.clone();
    browser.update_in(cx, |_, window, cx| {
        let owner = window.window_handle();
        cx.defer(move |cx| {
            using(&entity, "one", Arc::new(move || flag.load(Ordering::SeqCst)), owner, cx);
            using(
                &entity,
                "two",
                Arc::new(move || second_flag.load(Ordering::SeqCst)),
                owner,
                cx,
            );
            using(&other_entity, "three", Arc::new(|| true), owner, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let activity = cx.global::<Current>().0.clone();
        assert_eq!(activity.read(cx).sessions.len(), 2);
        assert_eq!(activity.read(cx).sessions[browser.read(cx).activity_id()].runs.len(), 2);
        first.store(false, Ordering::SeqCst);
        activity.update(cx, |activity, cx| activity.refresh(cx));
        assert_eq!(activity.read(cx).sessions[browser.read(cx).activity_id()].runs.len(), 1);
        browser.update(cx, |browser, cx| browser.pause_agent(true, cx));
        second.store(false, Ordering::SeqCst);
        activity.update(cx, |activity, cx| activity.refresh(cx));
        assert_eq!(activity.read(cx).sessions.len(), 1);
        assert!(
            browser.read(cx).activity_status().is_none(),
            "expiry must revoke a paused grant"
        );
        other.update(cx, |browser, cx| browser.take_over_agent(cx));
        activity.update(cx, |activity, cx| activity.refresh(cx));
        assert!(activity.read(cx).sessions.is_empty());
        assert!(activity.read(cx).window.is_none());
    });
}

#[gpui_kit::test]
fn pill_controls_expand_by_click_and_retract_with_escape(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
    let (pill, cx) = cx.add_window_view(|window, cx| {
        pill::Pill::new(
            vec![Item {
                id: "activity-ui".into(),
                status: Status {
                    agent: "Codex".into(),
                    title: "Check settings".into(),
                    action: "Reading the page".into(),
                    site: "localhost".into(),
                    paused: false,
                },
            }],
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert!(!pill.read_with(cx, |pill, _| pill.reveal.open));
    let bounds = cx
        .debug_bounds("activity-expand")
        .expect("the retracted pill has a keyboard-accessible trigger");
    cx.simulate_click(bounds.center(), Modifiers::default());
    assert!(pill.read_with(cx, |pill, _| pill.reveal.open));
    cx.update(|window, _| {
        assert_eq!(
            window.bounds().size,
            size(px(placement::EXPANDED), px(placement::HEADER + 144.))
        );
    });
    cx.simulate_keystrokes("escape");
    assert!(!pill.read_with(cx, |pill, _| pill.reveal.open));
    cx.update(|window, _| {
        assert_eq!(
            window.bounds().size,
            size(px(placement::COMPACT), px(placement::HEADER))
        );
    });
}

#[gpui_kit::test]
fn transfer_keeps_paused_grants_and_rejects_old_or_expired_controls(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
    let live = Arc::new(AtomicBool::new(true));
    let (saved, old_id, _old_browser) = {
        let (page, _) = crate::web::fixture_page(601);
        let (browser, cx) = cx.add_window_view(|window, cx| {
            let mut browser = Browser::fixture_retained("same-session", page, window, cx);
            browser.allow_agent();
            browser
        });
        let flag = live.clone();
        let call = browser.update_in(cx, |browser, window, cx| {
            browser.agent(
                "browser_tabs".into(),
                serde_json::json!({}),
                "run".into(),
                Arc::new(move || flag.load(Ordering::SeqCst)),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(futures::FutureExt::now_or_never(call).expect("tool completes").is_ok());
        let old_id = browser.read_with(cx, |browser, _| browser.activity_id().to_string());
        let saved = browser.update(cx, |browser, cx| {
            browser.pause_agent(true, cx);
            browser.retain(cx)
        });
        (saved, old_id, browser)
    };
    let (restored, cx) = cx.add_window_view(|window, cx| {
        // A failed native constructor retains its replacement before deferred registration can run.
        let intermediate = cx.new(|cx| Browser::restore(saved, window, cx));
        let saved = intermediate.update(cx, |browser, cx| browser.retain(cx));
        Browser::restore(saved, window, cx)
    });
    cx.run_until_parked();
    let new_id = restored.read_with(cx, |browser, _| browser.activity_id().to_string());
    assert_ne!(old_id, new_id);
    cx.update(|_, cx| {
        assert!(restored.read(cx).agent_paused());
        control(&old_id, Control::Pause, cx);
        assert!(restored.read(cx).agent_paused());
        control(&new_id, Control::Pause, cx);
        assert!(!restored.read(cx).agent_paused());
        restored.update(cx, |browser, cx| browser.pause_agent(true, cx));
        live.store(false, Ordering::SeqCst);
        // No poll has run: a click must recheck credentials itself.
        control(&new_id, Control::Pause, cx);
        assert!(restored.read(cx).agent_paused());
        let activity = cx.global::<Current>().0.clone();
        activity.update(cx, |activity, cx| activity.refresh(cx));
        assert!(restored.read(cx).activity_status().is_none());
        assert!(activity.read(cx).sessions.is_empty());
    });
}

#[gpui_kit::test]
fn duplicate_session_ids_and_deferred_retired_registration_do_not_share_controls(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
    });
    let (first, cx) = cx.add_window_view(|window, cx| {
        let mut browser = Browser::new(
            "duplicate".into(),
            None,
            nexus_browser::profile::Profile::new("activity-test", "first", false),
            None,
            window,
            cx,
        );
        browser.allow_agent();
        browser
    });
    let second = cx.update(|window, cx| {
        cx.new(|cx| {
            let mut browser = Browser::new(
                "duplicate".into(),
                None,
                nexus_browser::profile::Profile::new("activity-test", "second", false),
                None,
                window,
                cx,
            );
            browser.allow_agent();
            browser
        })
    });
    let source_live = Arc::new(AtomicBool::new(true));
    let source = source_live.clone();
    first.update(cx, |browser, _| {
        browser.bind_activity_source(Arc::new(move || source.load(Ordering::SeqCst)));
    });
    let a = first.clone();
    let b = second.clone();
    first.update_in(cx, |_, window, cx| {
        let owner = window.window_handle();
        cx.defer(move |cx| {
            using(&a, "run", Arc::new(|| true), owner, cx);
            using(&b, "run", Arc::new(|| true), owner, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let activity = cx.global::<Current>().0.clone();
        assert_eq!(activity.read(cx).sessions.len(), 2);
        let id = first.read(cx).activity_id().to_string();
        control(&id, Control::Pause, cx);
        assert!(first.read(cx).agent_paused());
        assert!(!second.read(cx).agent_paused());
        // Source retirement must block an old pill callback before the registry's next sweep.
        source_live.store(false, Ordering::SeqCst);
        control(&id, Control::Pause, cx);
        assert!(first.read(cx).agent_paused());
    });
    let a = first.clone();
    first.update_in(cx, |browser, window, cx| {
        let owner = window.window_handle();
        drop(browser.retain(cx));
        cx.defer(move |cx| using(&a, "late", Arc::new(|| true), owner, cx));
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let activity = cx.global::<Current>().0.clone();
        assert_eq!(activity.read(cx).sessions.len(), 1);
        assert!(activity.read(cx).sessions.contains_key(second.read(cx).activity_id()));
    });
}
