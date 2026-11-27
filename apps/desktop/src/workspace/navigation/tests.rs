//! Production Workspace journal and native Settings lifecycle, isolated from the person's backend/preferences.
use super::test_backend::BackendProbe;
use super::{Retained, Source};
use crate::gateway::{Account, Event};
use crate::model::SessionSummary;
use crate::workspace::{Auth, Place, SettingsSection, View, Workspace, machine_source::MachineSource};
use gpui_kit::{Context, Entity, Task, TestAppContext, VisualTestContext, Window, point, px};
use serde_json::json;
use std::{cell::RefCell, rc::Rc, sync::Arc};
mod artifact_refs;
mod layout;
mod open_focus;
fn account(org: &str, person: &str) -> Account {
    Account::from_json(&json!({"tenant":{"id":org,"slug":org,"name":"Fixture organization","kind":"company"},"user":{"id":person,"personId":person,"name":"A","email":format!("{person}@example.test")},"role":"member","permissions":[]})).expect("actual account wire fixture")
}
fn make(window: &mut Window, cx: &mut Context<Workspace>, backend: &BackendProbe) -> Workspace {
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let mut workspace = Workspace::with_machine_source(
        Arc::new(backend.clone()),
        events,
        arrivals,
        false,
        MachineSource::isolated(),
        window,
        cx,
    );
    workspace._code_poll = Task::ready(());
    workspace.auth = Auth::SignedIn;
    workspace.code_scope = Some(format!("{}|org-a", backend.url));
    workspace.side.open = false;
    workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])));
    let account = account("org-a", "person-a");
    workspace.accept_navigation_identity(&account, cx);
    workspace.account = Some(account);
    workspace
}
fn fixture<'a>(cx: &'a mut TestAppContext, backend: &BackendProbe) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| make(window, cx, backend))
}
fn chat(id: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: format!("Chat {id}"),
        day: None,
        at_ms: None,
    }
}
fn parked(cx: &mut TestAppContext, backend: &BackendProbe) -> Retained {
    let (workspace, cx) = fixture(cx, backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        workspace.open_settings_field(SettingsSection::Appearance, "text-size", window, cx);
        workspace.open(&chat("b"), window, cx);
        workspace.go(-1, window, cx);
        let saved = workspace.retain_navigation();
        window.remove_window();
        saved
    })
}
#[gpui_kit::test]
fn primary_journal_restores_middle_cursor_then_real_back_forward_and_branching(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = parked(cx, &backend);
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.restore_navigation(saved, None, window, cx);
        assert_eq!(workspace.history.len(), 4);
        assert_eq!(workspace.history_at, 2);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)));
        workspace.go(-1, window, cx);
        assert_eq!(workspace.current.as_deref(), Some("a"));
        workspace.go(1, window, cx);
        workspace.go(1, window, cx);
        assert_eq!(workspace.current.as_deref(), Some("b"));
        workspace.go(-1, window, cx);
        workspace.open(&chat("c"), window, cx);
        assert_eq!(workspace.history.len(), 4);
        assert!(matches!(&workspace.history[3],Place::Chat(Some(chat)) if chat.id=="c"));
        workspace.go(1, window, cx);
        assert_eq!(workspace.current.as_deref(), Some("c"));
    });
    assert!(
        backend
            .opens
            .lock()
            .expect("actual chat opens")
            .iter()
            .any(|id| id == "a")
    );
}
#[gpui_kit::test]
fn explicit_new_chat_and_tray_chat_outrank_parked_route_even_before_first_identity(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = parked(cx, &backend);
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.terminal_identity = None;
        workspace.new_chat(window, cx);
        workspace.accept_navigation_identity(&account("org-a", "person-a"), cx);
        workspace.restore_navigation(saved, None, window, cx);
        assert!(matches!(workspace.view, View::Chat));
        assert!(workspace.current.is_none());
        assert!(matches!(workspace.history.last(), Some(Place::Chat(None))));
    });
    let saved = workspace.update_in(cx, |workspace, window, _| {
        let saved = workspace.retain_navigation();
        window.remove_window();
        saved
    });
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.terminal_identity = None;
        workspace.open(&chat("tray-request"), window, cx);
        workspace.accept_navigation_identity(&account("org-a", "person-a"), cx);
        workspace.restore_navigation(saved, None, window, cx);
        assert_eq!(workspace.current.as_deref(), Some("tray-request"));
        assert!(matches!(workspace.history.last(),Some(Place::Chat(Some(chat))) if chat.id=="tray-request"));
    });
}
#[gpui_kit::test]
fn explicit_settings_merge_never_overwrites_the_retained_entrys_query_or_position(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = {
        let (workspace, cx) = fixture(cx, &backend);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_settings(SettingsSection::Appearance, window, cx);
            workspace.settings.shell.query_text = "old search".into();
            workspace.settings.shell.selected = 3;
            workspace.settings.scroll.set_offset(point(px(0.), px(-240.)));
            let saved = workspace.retain_navigation();
            window.remove_window();
            saved
        })
    };
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_settings(SettingsSection::Account, window, cx);
        workspace.settings.shell.query_text = "explicit search".into();
        workspace.settings.shell.selected = 1;
        workspace.restore_navigation(saved, None, window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Account)));
        workspace.go(-1, window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)));
        assert_eq!(workspace.settings.shell.query_text, "old search");
        assert_eq!(workspace.settings.shell.selected, 3);
        assert_eq!(workspace.navigation_settings_offset().y, px(-240.));
    });
}
#[gpui_kit::test]
fn settings_close_the_workbench_and_its_keys_cannot_open_it_there(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        assert!(!workspace.workbench_hidden());
        // A real shell's reader thread would wake the test scheduler from outside it.
        workspace.fixture_terminal_views();
        workspace.side.open = true;
        workspace.side.maximized = true;
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        assert!(workspace.workbench_hidden());
        assert!(!workspace.side.open && !workspace.side.maximized);
        workspace.on_toggle_side_pane(&crate::workspace::ToggleSidePane, window, cx);
        workspace.on_toggle_maximize_workbench(&crate::workspace::ToggleMaximizeWorkbench, window, cx);
        assert!(!workspace.side.open && !workspace.side.maximized);
        // Back on the chat, the Workbench stays closed until the person opens it.
        workspace.go(-1, window, cx);
        assert!(!workspace.workbench_hidden() && !workspace.side.open);
        workspace.on_toggle_side_pane(&crate::workspace::ToggleSidePane, window, cx);
        assert!(workspace.side.open);
    });
}
#[gpui_kit::test]
fn matching_account_preserves_journal_but_person_org_credential_and_gateway_changes_revoke_it(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("private"), window, cx);
        workspace.accept_navigation_identity(&account("org-a", "person-a"), cx);
        assert_eq!(workspace.history.len(), 2);
        let mut changed = account("org-a", "person-a");
        changed.person_id = Some("person-b".into());
        workspace.accept_navigation_identity(&changed, cx);
        assert_eq!(workspace.history, [Place::Chat(None)]);
        workspace.open(&chat("b"), window, cx);
        workspace.accept_navigation_identity(&account("org-b", "person-b"), cx);
        assert_eq!(workspace.history, [Place::Chat(None)]);
        workspace.open(&chat("c"), window, cx);
        *backend.marker.lock().expect("fixture marker") = Some("fixture-identity-b".into());
        assert!(!workspace.navigation_allowed());
        workspace.accept_navigation_identity(&account("org-b", "person-b"), cx);
        assert_eq!(workspace.history, [Place::Chat(None)]);
        workspace.open(&chat("d"), window, cx);
        let mut other = backend.clone();
        other.url = "https://other.example.test".into();
        workspace.gateway = crate::workspace::gateway_access::GatewayAccess::new(Arc::new(other));
        assert!(!workspace.navigation_allowed());
        workspace.accept_navigation_identity(&account("org-b", "person-b"), cx);
        assert_eq!(workspace.history, [Place::Chat(None)]);
    });
}
#[gpui_kit::test]
fn stale_account_envelope_and_retired_workspace_cannot_change_the_new_journal(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("current"), window, cx);
        let epoch = Arc::new(std::sync::atomic::AtomicU64::new(2));
        workspace.handle(
            Event::ReadSnapshot {
                epoch,
                expected: 1,
                revision: None,
                event: Box::new(Event::Account(account("old-org", "old-person"))),
            },
            window,
            cx,
        );
        assert_eq!(workspace.history.len(), 2);
        let saved = workspace.retain_navigation();
        let reads = backend.opens.lock().expect("recorded chat reads").len();
        workspace.open(&chat("retired"), window, cx);
        assert_eq!(backend.opens.lock().expect("retired chat reads").len(), reads);
        assert!(workspace.history.is_empty());
        assert!(workspace.navigation_retired);
        drop(saved);
    });
}
#[gpui_kit::test]
fn changed_local_store_binding_and_unknown_code_sources_never_fall_back_on_history_navigation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.visit(Place::Code("private-code".into()));
        let at = workspace.history_at;
        workspace.navigation_sources.insert(
            at,
            Source::Local {
                data: "/another/store".into(),
                scope: "different-scope".into(),
            },
        );
        workspace.open_navigation_place(window, cx);
        assert!(matches!(workspace.view, View::Chat));
        assert!(workspace.composer_note.is_some());
        workspace
            .navigation_sources
            .insert(at, Source::Machine("old-machine".into()));
        workspace.open_navigation_place(window, cx);
        assert!(matches!(workspace.view, View::Chat));
        workspace.navigation_sources.remove(&at);
        workspace.open_navigation_place(window, cx);
        assert!(matches!(workspace.view, View::Chat));
    });
}

#[gpui_kit::test]
fn actual_primary_envelope_rebinds_fresh_views_and_rejects_a_different_identity(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = {
        let (workspace, cx) = fixture(cx, &backend);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open(&chat("a"), window, cx);
            workspace.open_settings(SettingsSection::Appearance, window, cx);
            let saved = workspace
                .retain_primary_terminals(cx)
                .expect("signed-in primary envelope");
            window.remove_window();
            saved
        })
    };
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx));
        assert_eq!(workspace.history.len(), 3);
        workspace.go(-1, window, cx);
        assert_eq!(workspace.current.as_deref(), Some("a"));
    });
    let saved = workspace.update_in(cx, |workspace, window, cx| {
        let saved = workspace.retain_primary_terminals(cx).expect("retained matching owner");
        window.remove_window();
        saved
    });
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.accept_navigation_identity(&account("org-a", "different-person"), cx);
        assert!(!workspace.apply_primary_retained(saved, window, cx));
        assert_eq!(workspace.history, [Place::Chat(None)]);
    });
}

#[gpui_kit::test]
fn repeated_settings_sections_keep_distinct_per_entry_search_selection_and_scroll(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let saved = {
        let (workspace, cx) = fixture(cx, &backend);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_settings(SettingsSection::Appearance, window, cx);
            workspace.settings.shell.query_text = "first".into();
            workspace.settings.shell.selected = 3;
            workspace.settings.scroll.set_offset(point(px(0.), px(-240.)));
            workspace.open(&chat("between"), window, cx);
            workspace.open_settings(SettingsSection::Appearance, window, cx);
            workspace.settings.shell.query_text = "second".into();
            workspace.settings.shell.selected = 1;
            workspace.settings.scroll.set_offset(point(px(0.), px(-480.)));
            workspace.open(&chat("last"), window, cx);
            workspace.go(-1, window, cx);
            let saved = workspace.retain_primary_terminals(cx).expect("middle Settings history");
            window.remove_window();
            saved
        })
    };
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.apply_primary_retained(saved, window, cx));
        assert_eq!(workspace.history_at, 3);
        assert_eq!(workspace.settings.shell.query_text, "second");
        assert_eq!(workspace.settings.shell.selected, 1);
        assert_eq!(workspace.navigation_settings_offset().y, px(-480.));
        workspace.go(-1, window, cx);
        workspace.go(-1, window, cx);
        assert_eq!(workspace.settings.shell.query_text, "first");
        assert_eq!(workspace.settings.shell.selected, 3);
        assert_eq!(workspace.navigation_settings_offset().y, px(-240.));
        workspace.go(1, window, cx);
        workspace.go(1, window, cx);
        assert_eq!(workspace.settings.shell.query_text, "second");
        assert_eq!(workspace.settings.shell.selected, 1);
        assert_eq!(workspace.navigation_settings_offset().y, px(-480.));
    });
}

#[gpui_kit::test]
fn settings_opens_with_the_keys_in_its_search_field_and_a_move_inside_keeps_them_in_the_nav(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    use gpui_kit::Focusable as _;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        let search = workspace.settings.shell.query.read(cx).focus_handle(cx);
        workspace.open(&chat("a"), window, cx);
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        assert!(search.is_focused(window), "Settings opens in its search field");

        // A result opened from the search keeps the keys there.
        workspace.open_settings(SettingsSection::Account, window, cx);
        assert!(search.is_focused(window), "a search result keeps the search field");

        // A move from the nav keeps the keys in the nav.
        window.focus(&workspace.settings.shell.nav_focus, cx);
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        assert!(
            workspace.settings.shell.nav_focus.is_focused(window),
            "a move inside Settings stays in the nav"
        );

        workspace.open(&chat("b"), window, cx);
        workspace.open_settings_from_keys(window, cx);
        assert!(search.is_focused(window), "Ctrl+, opens Settings in its search field");

        // Esc in the empty field is not the field's: the window's Esc leaves Settings.
        let escape = gpui_kit::Keystroke::parse("escape").expect("escape keystroke");
        assert!(!workspace.settings_keystroke(&escape, window, cx));
    });
}

#[gpui_kit::test]
fn the_settings_key_lands_on_the_last_section_and_the_find_key_goes_to_the_search_field(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    use gpui_kit::Focusable as _;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open(&chat("a"), window, cx);
        // The first time: Account.
        workspace.open_settings_from_keys(window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Account)));

        // Then the section last used, also from a session.
        workspace.open_settings(SettingsSection::Notifications, window, cx);
        workspace.open(&chat("b"), window, cx);
        assert!(!matches!(workspace.view, View::Settings(_)));
        workspace.open_settings_from_keys(window, cx);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Notifications)));

        // Find in Settings is the search field's; a place key (New session) leaves Settings.
        let search = workspace.settings.shell.query.read(cx).focus_handle(cx);
        window.focus(&workspace.settings.shell.nav_focus, cx);
        workspace.open_find(window, cx);
        assert!(search.is_focused(window), "the find key focuses Search settings");
        workspace.start_new_session(window, cx);
        assert!(
            !matches!(workspace.view, View::Settings(_)),
            "a place key leaves Settings"
        );
    });
}

#[gpui_kit::test]
fn local_workspace_uses_the_normal_sidebar_and_settings_without_gateway_calls(cx: &mut TestAppContext) {
    use crate::pages::PageId;
    use crate::workspace::{REQUIRES_SIGN_IN, local_mode::Entry};

    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.marker.lock().unwrap() = None;
    let local_settings = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::pages::subscriptions::fixture_backend(cx);
        crate::pages::local_providers::fixture_store(local_settings.path(), cx);
    });
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let (_, events) = futures::channel::mpsc::unbounded();
        let (_, arrivals) = futures::channel::mpsc::unbounded();
        let mut workspace = Workspace::with_machine_source(
            Arc::new(backend.clone()),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.auth = Auth::SignedOut { error: None };
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.local_mode.remembered = true;
        workspace
    });
    layout::draw(cx);
    assert!(
        backend.calls.lock().unwrap().is_empty(),
        "the signed-out card makes no request"
    );
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enter_local(Entry::Launch, window, cx);
        assert!(workspace.is_local());
        assert_eq!(workspace.code_scope.as_deref(), Some("|"));
        assert!(matches!(workspace.view, View::NewCode));
        workspace.sidebar_open = true;
        cx.notify();
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.projects = vec![crate::code::CodeProject {
            id: "local-fixture".into(),
            name: "Local project".into(),
            branch: None,
            pinned: false,
            remotes: vec![],
            sessions: vec![],
            machine: None,
            added_at: 0,
        }];
        workspace.notify_main(cx);
        cx.notify();
    });
    layout::draw(cx);
    for id in [
        "new-session",
        "sidebar-groups",
        "person",
        "project-local-fixture",
        "toggle-side-pane",
    ] {
        assert!(cx.debug_bounds(id).is_some(), "{id} must be drawn");
    }
    for id in ["inbox", "artifacts", "your-agent", "org"] {
        assert!(cx.debug_bounds(id).is_none(), "{id} must stay hidden");
    }

    let native_settings = PageId::ALL
        .into_iter()
        .filter(|id| !id.is_place() && *id != PageId::Subscriptions)
        .map(SettingsSection::Page);
    let built_in_settings = [
        SettingsSection::Appearance,
        SettingsSection::Notifications,
        SettingsSection::Shortcuts,
        SettingsSection::LocalProviders,
        SettingsSection::Account,
        SettingsSection::ApiKeys,
    ];
    for section in native_settings.chain(built_in_settings) {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_settings(section, window, cx);
            assert!(
                matches!(workspace.view, View::Settings(open) if open == section),
                "{section:?}"
            );
            if let SettingsSection::Page(id) = section {
                let built = workspace
                    .settings
                    .shell
                    .views
                    .iter()
                    .any(|(open, view)| *open == section && view.is_some());
                let builder = id.builder();
                assert_eq!(
                    built,
                    id.is_native() && (!builder.needs_account() || builder.has_locked_view()),
                    "{id:?}"
                );
            }
        });
        layout::draw(cx);
    }
    workspace.update_in(cx, |workspace, window, cx| {
        let cloud = SettingsSection::Page(PageId::Cloud);
        assert_eq!(workspace.settings_account_status(cloud), Some(REQUIRES_SIGN_IN));
        workspace.start_new_session(window, cx);
        workspace.open_palette(window, cx);
        workspace.palette_typed("hello".into(), cx);
        workspace.palette_typed("@org".into(), cx);
    });
    cx.run_until_parked();
    assert!(
        backend.calls.lock().unwrap().is_empty(),
        "calls: {:?}; requests: {:?}",
        backend.calls,
        backend.reads
    );
    assert!(backend.reads.lock().unwrap().is_empty());
    workspace.update_in(cx, |workspace, window, cx| workspace.sign_in_from_local(window, cx));
    assert_eq!(*backend.calls.lock().unwrap(), ["sign_in"]);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.handle(Event::SignedIn, window, cx);
        assert!(matches!(workspace.auth, Auth::SignedIn));
        assert!(matches!(workspace.view, View::NewCode));
        assert!(!workspace.local_mode.signing_in);
        assert!(!workspace.code_form.local_subscription);
    });
}

#[gpui_kit::test]
fn sign_out_only_returns_to_local_after_the_choice(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
        workspace.sessions.push(chat("old"));
        *backend.marker.lock().unwrap() = None;
        workspace.handle(Event::SignedOut, window, cx);
        assert!(matches!(workspace.auth, Auth::SignedOut { .. }));
        assert!(workspace.sessions.is_empty());
        workspace.local_mode.remembered = true;
        workspace.handle(Event::SignedOut, window, cx);
        assert!(workspace.is_local());
        assert!(workspace.account.is_none());
        assert_eq!(workspace.code_scope.as_deref(), Some("|"));
        workspace.handle(Event::SignInFailed("cancelled request".into()), window, cx);
        assert!(workspace.is_local());
        workspace.sign_in_from_local(window, cx);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        workspace.handle(
            Event::SignInCode {
                cancelled: cancelled.clone(),
                user_code: "OLD-CODE".into(),
                link: "http://gateway.invalid".into(),
                expires_in: 90.,
            },
            window,
            cx,
        );
        workspace.handle(
            Event::DeviceSignInFailed {
                cancelled,
                error: "expired".into(),
            },
            window,
            cx,
        );
        assert!(matches!(workspace.auth, Auth::Checking));
    });
}

fn enter_local_fixture(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.account = None;
    workspace.auth = Auth::SignedOut { error: None };
    workspace.local_mode.opt_in = crate::local_providers_opt_in::enabled_for_test();
    workspace.local_mode.remembered = true;
    workspace.enter_local(crate::workspace::local_mode::Entry::Launch, window, cx);
    assert!(workspace.is_local());
}

#[gpui_kit::test]
fn cancelling_denying_or_expiring_local_sign_in_preserves_the_open_settings(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.marker.lock().unwrap() = None;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        enter_local_fixture(workspace, window, cx);
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        workspace.sign_in_from_local(window, cx);
        workspace.cancel_sign_in_attempt(window, cx);
        assert!(backend.calls.lock().unwrap().contains(&"cancel_sign_in"));
        assert!(workspace.is_local());
        assert!(!workspace.local_mode.signing_in);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)));
        for error in ["Sign-in was denied.", "The code expired."] {
            workspace.sign_in_from_local(window, cx);
            workspace.handle(
                Event::DeviceSignInFailed {
                    cancelled: Default::default(),
                    error: error.into(),
                },
                window,
                cx,
            );
            assert!(workspace.is_local());
            assert!(!workspace.local_mode.signing_in);
            assert_eq!(workspace.code_scope.as_deref(), Some("|"));
            assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)));
            assert_eq!(workspace.notices.current().unwrap().text, error);
        }
    });
}

#[gpui_kit::test]
fn a_local_sign_in_code_timing_out_returns_to_the_same_workspace(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.marker.lock().unwrap() = None;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        enter_local_fixture(workspace, window, cx);
        workspace.open_settings(SettingsSection::Appearance, window, cx);
        workspace.sign_in_from_local(window, cx);
        workspace.sign_in_code(
            "CODE-FOR-TEST".into(),
            "https://gateway.example.test/device".into(),
            1.,
            window,
            cx,
        );
        assert!(matches!(workspace.auth, Auth::Waiting { .. }));
    });
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.is_local());
        assert!(!workspace.local_mode.signing_in);
        assert!(matches!(workspace.view, View::Settings(SettingsSection::Appearance)));
        assert_eq!(workspace.notices.current().unwrap().text, "The sign-in code expired.");
    });
}

#[gpui_kit::test]
fn prompt_links_in_local_explain_the_account_requirement_without_waiting_for_sign_in(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.marker.lock().unwrap() = None;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.pending_links.push("nexus://new?prompt=cold".into());
        enter_local_fixture(workspace, window, cx);
        assert!(
            workspace.pending_links.is_empty(),
            "an initial link must not survive Local entry"
        );
        assert_eq!(
            workspace.notices.current().map(|notice| notice.text.as_str()),
            Some("Sign in to Nexus, then open the link again.")
        );
        let before = backend.calls.lock().unwrap().len();
        workspace.arrived(crate::links::Arrival::Link("nexus://new?prompt=old".into()), window, cx);
        assert_eq!(
            workspace.notices.current().map(|notice| notice.text.as_str()),
            Some("Sign in to Nexus, then open the link again.")
        );
        assert!(workspace.pending_links.is_empty());
        assert_eq!(backend.calls.lock().unwrap().len(), before);
    });
}

#[gpui_kit::test]
fn first_sign_in_from_local_starts_onboarding_without_waiting_for_account_details(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.marker.lock().unwrap() = None;
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        enter_local_fixture(workspace, window, cx);
        workspace.onboarding.reset_saved_state_for_test();
        assert!(!workspace.onboarding.active());
        workspace.sign_in_from_local(window, cx);
        *backend.marker.lock().unwrap() = Some("fixture-identity-a".into());
        workspace.handle(Event::SignedIn, window, cx);
        assert!(matches!(workspace.auth, Auth::SignedIn));
        assert!(workspace.onboarding.active());
        assert!(workspace.account.is_none());
        assert_eq!(workspace.code_scope.as_deref(), Some("|"));
        assert!(backend.calls.lock().unwrap().contains(&"load_account"));
    });
}

#[gpui_kit::test]
fn sign_in_with_byo_off_keeps_the_open_settings_views(cx: &mut TestAppContext) {
    use crate::pages::PageId;

    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.local_mode.opt_in = Default::default();
        let section = SettingsSection::Page(PageId::DesktopApp);
        workspace.open_settings(section, window, cx);
        assert!(
            workspace
                .settings
                .shell
                .views
                .iter()
                .any(|(id, view)| *id == section && view.is_some())
        );
        workspace.handle(Event::SignedIn, window, cx);
        assert!(matches!(workspace.view, View::Settings(open) if open == section));
        assert!(
            workspace
                .settings
                .shell
                .views
                .iter()
                .any(|(id, view)| *id == section && view.is_some())
        );
    });
}
