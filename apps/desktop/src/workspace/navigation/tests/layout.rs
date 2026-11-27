//! Actual native Settings views, child-only deferred data and user input/layout lifecycle.
use super::{BackendProbe, Place, SettingsSection, View, Workspace, fixture};
use crate::pages::PageId;
use gpui_kit::{
    Modifiers, ScrollDelta, ScrollSource, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, point, px,
};
use std::time::Duration;
pub(super) fn draw(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
}
fn pending(
    workspace: &mut Workspace,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<Workspace>,
    offset: f32,
) {
    workspace.open_settings(SettingsSection::Appearance, window, cx);
    workspace.settings.shell.positions.insert(
        (workspace.settings.shell.account_scope.clone(), workspace.history_at),
        (point(px(0.), px(offset)), String::new(), 0, false),
    );
    workspace.restore_settings_position(workspace.history_at, window, cx);
}
#[gpui_kit::test]
fn deferred_native_members_child_restores_after_long_loading_and_another_primary_close(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let mut ready = BackendProbe::new();
    ready.member_count = 40;
    let (workspace, cx) = fixture(cx, &ready);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::Page(PageId::Members), window, cx)
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update(cx, |workspace, _| {
        assert!(workspace.settings.scroll.max_offset().y > px(600.));
        workspace.settings.scroll.set_offset(point(px(0.), px(-500.)));
    });
    draw(cx);
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("matching owner");
        window.remove_window();
        saved
    });
    let mut delayed = ready.clone();
    delayed.hold_members = true;
    let (workspace, cx) = fixture(cx, &delayed);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    draw(cx);
    let ids = delayed.member_ids();
    assert_eq!(ids.len(), 1);
    let old_read = ids[0];
    workspace.read_with(cx, |workspace, _| assert!(workspace.navigation_scroll.is_some()));
    cx.executor().advance_clock(Duration::from_secs(121));
    cx.run_until_parked();
    draw(cx);
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        assert_eq!(workspace.navigation_settings_offset().y, px(-500.));
        let saved = workspace.retain_primary_terminals(cx).expect("close while loading");
        window.remove_window();
        saved
    });
    let (workspace, cx) = fixture(cx, &delayed);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx))
    });
    cx.run_until_parked();
    draw(cx);
    let ids = delayed.member_ids();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], old_read);
    let current_read = ids[1];
    assert_ne!(old_read, current_read);
    // A retired page's network read may finish or be canceled; either way it cannot satisfy the new child.
    let _old_transport_received = delayed.release_members(old_read, 1);
    cx.run_until_parked();
    draw(cx);
    workspace.read_with(cx, |workspace, _| assert!(workspace.navigation_scroll.is_some()));
    assert_eq!(delayed.member_ids(), [current_read]);
    assert!(delayed.release_members(current_read, 40));
    cx.run_until_parked();
    draw(cx);
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.settings.scroll.offset().y, px(-500.));
        assert!(workspace.navigation_scroll.is_none());
    });
}
#[gpui_kit::test]
fn old_frame_never_consumes_new_ticket_and_native_wheel_or_query_cancels_restore(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        pending(workspace, window, cx, -5000.);
        let a = workspace.navigation_epoch;
        pending(workspace, window, cx, -6000.);
        assert_ne!(a, workspace.navigation_epoch);
    });
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        let ticket = workspace.navigation_scroll.as_ref().expect("B survives A's frame");
        assert_eq!(ticket.offset.y, px(-6000.));
        assert_eq!(ticket.epoch, workspace.navigation_epoch);
    });
    let bounds = cx.debug_bounds("settings").expect("native Settings scroll area");
    cx.simulate_event(ScrollWheelEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| assert!(workspace.navigation_scroll.is_none()));
    workspace.update_in(cx, |workspace, window, cx| pending(workspace, window, cx, -7000.));
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .settings
            .shell
            .query
            .update(cx, |input, cx| input.focus(window, cx))
    });
    cx.simulate_keystrokes("n e w");
    cx.run_until_parked();
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.navigation_scroll.is_none());
        assert_eq!(workspace.settings.shell.query_text, "new");
    });
}
#[gpui_kit::test]
fn explicit_same_section_and_field_paths_outrank_numeric_history_restore(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        pending(workspace, window, cx, -5000.);
        workspace.open_nexus_path("/settings/appearance", window, cx);
        assert!(workspace.navigation_scroll.is_none());
    });
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        pending(workspace, window, cx, -6000.);
        workspace.open_nexus_path("/settings/preferences#text-size", window, cx);
        assert!(workspace.navigation_scroll.is_none());
    });
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(matches!(
            workspace.history.get(workspace.history_at),
            Some(Place::SettingsField(SettingsSection::Appearance, "text-size"))
        ));
        assert!(
            workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        );
    });
}
#[gpui_kit::test]
fn old_field_scroll_focus_cannot_override_numeric_restore_or_new_explicit_field(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
        let palette = crate::pages::palette(cx);
        let _ = workspace.render_settings(SettingsSection::Appearance, palette, window, cx);
        workspace.settings.shell.positions.insert(
            (workspace.settings.shell.account_scope.clone(), workspace.history_at),
            (point(px(0.), px(-5000.)), String::new(), 0, false),
        );
        workspace.restore_settings_position(workspace.history_at, window, cx);
        workspace.focus_composer(window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            !workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        )
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
        let palette = crate::pages::palette(cx);
        let _ = workspace.render_settings(SettingsSection::Appearance, palette, window, cx);
        workspace.open_settings_field(SettingsSection::Appearance, "theme", window, cx);
        workspace.focus_composer(window, cx);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            !workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        )
    });
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            workspace
                .settings
                .shell
                .fields
                .focus("theme", cx)
                .contains_focused(window, cx)
        )
    });
}
#[gpui_kit::test]
fn matching_account_internal_rebind_keeps_completed_position_and_query(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let mut backend = BackendProbe::new();
    backend.member_count = 40;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::Page(PageId::Members), window, cx)
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.settings.scroll.set_offset(point(px(0.), px(-500.)));
        workspace.settings.shell.query_text = "kept query".into();
        workspace
            .settings
            .shell
            .query
            .update(cx, |query, cx| query.set_value("kept query", window, cx));
        workspace.settings.shell.searching = false;
        workspace.settings.shell.selected = 3;
        let at = workspace.history_at;
        workspace.accept_navigation_identity(&super::account("org-a", "person-a"), cx);
        workspace.rebind_navigation_settings(SettingsSection::Page(PageId::Members), window, cx);
        assert_eq!(workspace.history_at, at);
    });
    cx.run_until_parked();
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.settings.scroll.offset().y, px(-500.));
        assert_eq!(workspace.settings.shell.query_text, "kept query");
        assert_eq!(workspace.settings.shell.selected, 3);
    });
}
#[gpui_kit::test]
fn native_back_forward_clicks_drive_the_restored_journal_and_stop_at_ends(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = super::parked(cx, &backend);
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.restore_navigation(saved, None, window, cx)
    });
    draw(cx);
    let forward = cx.debug_bounds("forward").expect("native Forward control");
    cx.simulate_click(forward.center(), Modifiers::none());
    cx.run_until_parked();
    draw(cx);
    workspace.read_with(cx, |workspace, _| assert_eq!(workspace.current.as_deref(), Some("b")));
    let forward = cx.debug_bounds("forward").expect("disabled native Forward control");
    cx.simulate_click(forward.center(), Modifiers::none());
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| assert_eq!(workspace.history_at, 3));
    draw(cx);
    let back = cx.debug_bounds("back").expect("native Back control");
    cx.simulate_click(back.center(), Modifiers::none());
    cx.run_until_parked();
    draw(cx);
    workspace.read_with(cx, |workspace, _| {
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)))
    });
}

#[gpui_kit::test]
fn deferred_selected_search_reveal_cannot_scroll_a_different_query(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        workspace
            .settings
            .shell
            .query
            .update(cx, |input, cx| input.set_value("a", window, cx));
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update(cx, |workspace, cx| {
        let count = crate::workspace::settings_search::search("a", &workspace.settings_visibility()).len();
        assert!(count > 8);
        workspace.settings.shell.selected = count - 1;
        workspace.settings.shell.results_selected.set(None);
        cx.notify();
    });
    let observed = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let record = observed.clone();
    let view = workspace.clone();
    cx.update(|window, cx| {
        // This real draw lays out the old last row and queues its deferred ScrollAnchor reveal.
        window.draw(cx).clear(cx);
        view.update(cx, |workspace, cx| {
            // Query hydration, like a restored query, may keep the same selected numeric index.
            // The old reveal must compare the actual current query, not only that index.
            workspace.settings.shell.query_text = "different query".into();
            workspace
                .settings
                .shell
                .query
                .update(cx, |input, cx| input.set_value("different query", window, cx));
            workspace
                .settings
                .shell
                .results_scroll
                .set_offset(point(px(0.), px(-77.)));
        });
        cx.defer(move |cx| {
            record
                .borrow_mut()
                .push(view.read(cx).settings.shell.results_scroll.offset())
        });
    });
    cx.run_until_parked();
    assert_eq!(&*observed.borrow(), &[point(px(0.), px(-77.))]);
}

#[gpui_kit::test]
fn automatic_account_rebind_preserves_only_unfulfilled_explicit_field_intent(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
        let palette = crate::pages::palette(cx);
        let _ = workspace.render_settings(SettingsSection::Appearance, palette, window, cx);
        assert_eq!(workspace.settings.shell.fields.pending_target(), Some("text-size"));
        workspace.accept_navigation_identity(&super::account("org-a", "person-a"), cx);
        workspace.rebind_navigation_settings(SettingsSection::Appearance, window, cx);
        assert_eq!(workspace.settings.shell.fields.pending_target(), Some("text-size"));
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        );
        assert!(workspace.settings.shell.fields.pending_target().is_none());
        workspace.open_settings_field(SettingsSection::Appearance, "theme", window, cx);
        // A real user scroll/input cancel leaves no exact-field move for Account to resurrect.
        workspace.cancel_navigation_restore();
        workspace.rebind_navigation_settings(SettingsSection::Appearance, window, cx);
        assert!(workspace.settings.shell.fields.pending_target().is_none());
    });
}

#[gpui_kit::test]
fn first_account_preserves_fresh_explicit_field_before_registration(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.terminal_identity = None;
        workspace.account = None;
        workspace.settings.shell.account_scope = None;
        workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
        let account = super::account("org-a", "person-a");
        workspace.account = Some(account.clone());
        workspace.accept_navigation_identity(&account, cx);
        workspace.rebind_navigation_settings(SettingsSection::Appearance, window, cx);
        assert_eq!(workspace.settings.shell.fields.pending_target(), Some("text-size"));
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        );
        assert!(workspace.settings.shell.fields.pending_target().is_none());
    });
}

#[gpui_kit::test]
fn unmet_explicit_field_survives_primary_close_and_fresh_layout(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = {
        let (workspace, cx) = fixture(cx, &backend);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
            assert_eq!(workspace.settings.shell.fields.pending_target(), Some("text-size"));
            let saved = workspace.retain_primary_terminals(cx).expect("retain unmet field");
            window.remove_window();
            saved
        })
    };
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx));
        assert_eq!(workspace.settings.shell.fields.pending_target(), Some("text-size"));
        assert!(workspace.navigation_scroll.is_none());
    });
    cx.run_until_parked();
    draw(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(
            workspace
                .settings
                .shell
                .fields
                .focus("text-size", cx)
                .contains_focused(window, cx)
        );
        assert!(workspace.settings.shell.fields.pending_target().is_none());
    });
}

#[gpui_kit::test]
fn signed_in_preferences_load_without_a_sign_in_label_before_me_answers(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    cx.update(crate::pages::subscriptions::fixture_backend);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    for section in [
        SettingsSection::Appearance,
        SettingsSection::Notifications,
        SettingsSection::Page(PageId::Devices),
        SettingsSection::Page(PageId::ApiExplorer),
        SettingsSection::Page(PageId::Privacy),
        SettingsSection::Page(PageId::Worktrees),
        SettingsSection::Page(PageId::Agents),
        SettingsSection::Page(PageId::Servers),
    ] {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.auth = crate::workspace::Auth::SignedIn;
            workspace.account = None;
            assert!(workspace.page_ctx(cx).is_none());
            workspace.open_settings(section, window, cx);
            assert!(workspace.settings_account_status(section).is_none());
            assert!(matches!(workspace.view, View::Settings(open) if open == section));
            if matches!(section, SettingsSection::Page(id) if matches!(id.builder(), crate::pages::Builder::Mixed(_))) {
                assert!(
                    workspace
                        .settings
                        .shell
                        .views
                        .iter()
                        .any(|(open, view)| *open == section && view.is_some())
                );
            }
        });
        draw(cx);
        assert!(cx.debug_bounds("requires-sign-in-label").is_none());
        assert_eq!(
            cx.debug_bounds("settings-email-phone-loading").is_some(),
            section == SettingsSection::Notifications,
        );
    }
}

#[gpui_kit::test]
fn an_open_mixed_page_keeps_its_device_view_when_another_window_signs_in(cx: &mut TestAppContext) {
    let backend = BackendProbe::new();
    let _agents = crate::code_run::fixture_agents(vec![]);
    let (workspace, cx) = fixture(cx, &backend);
    let section = SettingsSection::Page(PageId::Privacy);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = crate::workspace::Auth::Local;
        workspace.account = None;
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.onboarding.reset_saved_state_for_test();
        workspace.onboarding_at_launch();
        workspace.open_settings(section, window, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("requires-sign-in-label").is_some());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.handle(crate::gateway::Event::SignedIn, window, cx);
        assert!(workspace.page_ctx(cx).is_none());
        assert!(matches!(workspace.view, View::Settings(open) if open == section));
        assert!(
            workspace
                .settings
                .shell
                .views
                .iter()
                .any(|(open, view)| *open == section && view.is_some())
        );
    });
    draw(cx);
    assert!(cx.debug_bounds("requires-sign-in-label").is_none());
}

#[gpui_kit::test]
fn local_settings_build_device_pages_and_keep_account_pages_inert(cx: &mut TestAppContext) {
    use crate::workspace::{Auth, settings_registry::Dest};

    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = Auth::Local;
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.account = None;
        workspace.terminal_identity = None;
        assert!(workspace.page_ctx(cx).is_none());
        backend.reads.lock().unwrap().clear();
        backend.calls.lock().unwrap().clear();

        for page in [
            PageId::Gateways,
            PageId::Worktrees,
            PageId::Hooks,
            PageId::DesktopApp,
            PageId::LocalSessions,
            PageId::SharedMemory,
            PageId::Mcp,
            PageId::Privacy,
        ] {
            let section = SettingsSection::Page(page);
            workspace.open_settings(section, window, cx);
            assert!(matches!(workspace.view, View::Settings(open) if open == section));
            assert!(
                workspace
                    .settings
                    .shell
                    .views
                    .iter()
                    .any(|(id, view)| *id == section && view.is_some()),
                "{page:?} must build without an account"
            );
        }
        workspace.open_settings(SettingsSection::Notifications, window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Notifications)));
        workspace.open_settings(SettingsSection::ApiKeys, window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::ApiKeys)));
        assert!(workspace.settings.shell.views.iter().all(|(_, view)| view.is_none()));
        workspace.open_nexus_path("/app/demo/settings/connectors/slack", window, cx);
        assert!(matches!(
            workspace.view,
            View::Settings(SettingsSection::GatewayLink(Dest::Slack))
        ));
        workspace.open_nexus_path("/app/demo/usage", window, cx);
        assert!(matches!(
            workspace.view,
            View::Settings(SettingsSection::Page(PageId::Usage))
        ));
        workspace.open_nexus_path("/app/demo/settings/budgets#usage-limits", window, cx);
        assert!(matches!(
            workspace.view,
            View::Settings(SettingsSection::Page(PageId::Usage))
        ));
        assert!(matches!(
            workspace.history.last(),
            Some(Place::SettingsField(_, "usage-limits"))
        ));
    });
    draw(cx);
    assert_eq!(*backend.reads.lock().unwrap(), Vec::<String>::new());
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn local_settings_keeps_gateway_rows_searchable_and_labels_disabled_controls(cx: &mut TestAppContext) {
    use crate::workspace::{Auth, settings_registry::Dest};

    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = Auth::Local;
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.account = None;
        workspace.terminal_identity = None;
        backend.calls.lock().unwrap().clear();
        let visibility = workspace.settings_visibility();
        let hits = crate::workspace::settings_search::search("provider", &visibility);
        assert!(
            hits.iter()
                .any(|hit| hit.dest == Dest::Providers && hit.requires_sign_in())
        );
        assert!(
            hits.iter()
                .any(|hit| hit.dest == Dest::LocalProviders && !hit.requires_sign_in())
        );
        workspace.open_settings(SettingsSection::ApiKeys, window, cx);
    });
    draw(cx);
    assert_eq!(crate::pages::ui::REQUIRES_SIGN_IN, "Requires sign-in");
    assert!(cx.debug_bounds("requires-sign-in-label").is_some());
    let create = cx
        .debug_bounds("settings-create-key-locked")
        .expect("Create key remains visible");
    cx.simulate_click(create.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(*backend.calls.lock().unwrap(), Vec::<&str>::new());
}

#[gpui_kit::test]
fn signed_out_policy_and_budgets_show_inert_native_actions(cx: &mut TestAppContext) {
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    for (page, selector) in [
        (PageId::Policy, "policy-patterns-locked"),
        (PageId::Usage, "usage-edit-budget-locked"),
    ] {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.auth = crate::workspace::Auth::Local;
            workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
            workspace.account = None;
            backend.calls.lock().unwrap().clear();
            backend.reads.lock().unwrap().clear();
            if page == PageId::Usage {
                workspace.open_settings_field(SettingsSection::Page(page), "usage-limits", window, cx);
            } else {
                workspace.open_settings(SettingsSection::Page(page), window, cx);
            }
        });
        draw(cx);
        assert!(cx.debug_bounds("requires-sign-in-label").is_some());
        let button = cx.debug_bounds(selector).expect("the native control is still visible");
        cx.simulate_click(button.center(), Modifiers::default());
        cx.run_until_parked();
        assert!(backend.calls.lock().unwrap().is_empty());
        assert!(backend.reads.lock().unwrap().is_empty());
        if page == PageId::Usage {
            workspace.update_in(cx, |workspace, _, _| {
                assert!(workspace.settings.shell.fields.pending_target().is_none());
            });
        }
    }
}

#[gpui_kit::test]
fn local_settings_gateway_change_keeps_the_local_scope_without_restoring_an_account(cx: &mut TestAppContext) {
    use crate::workspace::Auth;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.auth = Auth::Local;
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.account = None;
        workspace.terminal_identity = None;
        workspace.code_scope = Some(nexus_workbench::session_scope::SessionScope::local_text().to_string());
        let section = SettingsSection::Page(PageId::Gateways);
        workspace.open_settings(section, window, cx);
        let (gateway, events) = workspace.gateway.choose_gateway("https://another.example.test", false);
        backend.calls.lock().unwrap().clear();
        workspace.follow_gateway(gateway, events, window, cx, false);
        assert!(workspace.is_local());
        assert_eq!(
            workspace.code_scope.as_deref(),
            Some(nexus_workbench::session_scope::SessionScope::local_text())
        );
        assert!(matches!(workspace.view, View::Settings(open) if open == section));
        assert_eq!(
            workspace.device_page_ctx(cx).gateway_url,
            "https://another.example.test"
        );
    });
    assert!(backend.calls.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn settings_drops_the_previous_account_context_and_data_on_org_switch(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::ApiKeys, window, cx);
        workspace.settings.tokens = Some(Ok(vec![]));
        workspace.settings.token_note = Some("Previous account".into());
        assert!(workspace.settings.shell.account_ctx.is_some());
        workspace.leave_org();
        assert!(workspace.page_ctx(cx).is_none());
        assert!(workspace.settings.shell.account_ctx.is_none());
        assert!(workspace.settings.tokens.is_none());
        assert!(workspace.settings.token_note.is_none());
        assert!(workspace.settings.shell.views.is_empty());
    });
}

#[test]
fn local_settings_opens_the_available_section_of_a_mixed_destination() {
    use crate::workspace::settings_registry::{Dest, Visibility};
    let local = Visibility::signed_out(false);
    assert_eq!(
        Dest::Integrations.first_for(&local),
        Some(SettingsSection::Page(PageId::Mcp))
    );
    assert_eq!(
        Dest::Integrations.first(),
        Some(SettingsSection::Page(PageId::Connectors))
    );
    let hits = crate::workspace::settings_search::search("Connections", &local);
    let group = hits
        .iter()
        .find(|hit| hit.dest == Dest::Integrations && hit.anchor.is_none())
        .expect("Connections result");
    assert_eq!(group.section, Some(SettingsSection::Page(PageId::Mcp)));
    assert!(!group.requires_sign_in());
}
