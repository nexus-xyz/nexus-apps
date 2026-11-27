//! The tray's rows in an app window (`shell.rs`): where each one lands, and a window that is still signing in.
use super::gateway_chat::tests::{Probe, account, fixture};
use super::shell::TrayPlace;
use super::{SettingsSection, View, Workspace};
use crate::gateway::Event;
use crate::model::SessionSummary;
use crate::pages::PageId;
use crate::tray::Command;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

const COMPUTER: SettingsSection = SettingsSection::Page(PageId::DesktopApp);

fn tray(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, command: Command) {
    workspace.update_in(cx, |workspace, window, cx| workspace.on_tray(command, window, cx));
    cx.run_until_parked();
}

/// Whether the window shows Settings › This computer.
fn on_computer(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    workspace.read_with(cx, |workspace, _| matches!(workspace.view, View::Settings(COMPUTER)))
}

fn on_chat(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> bool {
    workspace.read_with(cx, |workspace, _| matches!(workspace.view, View::Chat))
}

/// A runner for the window to hold, so the account read below does not start the app's own (`org_scope_tests.rs`).
fn hold_test_runner(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> std::path::PathBuf {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let data = std::env::temp_dir().join(format!("ngs-tray-{}", &id[..8]));
    std::fs::create_dir_all(&data).expect("a temporary folder");
    let (runner, _events) = crate::code_run::test_runner(data.clone(), "http://127.0.0.1:9|org-a");
    workspace.update(cx, |workspace, _| workspace.runner = Some(std::rc::Rc::new(runner)));
    data
}

#[gpui_kit::test]
fn settings_and_check_for_updates_open_the_windows_settings_not_the_panel(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    tray(&workspace, cx, Command::Settings);
    assert!(on_computer(&workspace, cx));
    tray(&workspace, cx, Command::NewChat);
    assert!(on_chat(&workspace, cx));
    tray(&workspace, cx, Command::CheckForUpdates);
    assert!(on_computer(&workspace, cx));
}

#[gpui_kit::test]
fn new_chat_leaves_the_open_chat_for_a_new_one(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    tray(&workspace, cx, Command::Settings);
    tray(&workspace, cx, Command::NewChat);
    assert!(on_chat(&workspace, cx));
    assert!(
        workspace.read_with(cx, |workspace, _| workspace.current.is_none()),
        "a new chat"
    );
}

/// A window opened for the row has no account yet: the row waits for it, and goes in place of the window's landing.
#[gpui_kit::test]
fn a_window_still_signing_in_does_the_row_once_its_account_arrives(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let _data = hold_test_runner(&workspace, cx);
    workspace.update(cx, |workspace, _| workspace.account = None);
    tray(&workspace, cx, Command::Settings);
    assert!(on_chat(&workspace, cx), "nothing to show without the account");
    assert!(workspace.read_with(cx, |workspace, _| workspace.tray_pending.is_some()));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.handle(Event::Account(account("org-a", "person-a")), window, cx)
    });
    cx.run_until_parked();
    assert!(on_computer(&workspace, cx));
    assert!(workspace.read_with(cx, |workspace, _| workspace.tray_pending.is_none()));
}

#[test]
fn the_window_rows_have_a_place_and_the_app_rows_do_not() {
    assert_eq!(TrayPlace::of(Command::NewChat), Some(TrayPlace::NewChat));
    assert_eq!(TrayPlace::of(Command::Settings), Some(TrayPlace::Settings));
    assert_eq!(TrayPlace::of(Command::CheckForUpdates), Some(TrayPlace::Updates));
    assert_eq!(TrayPlace::of(Command::RestartToUpdate), Some(TrayPlace::Updates));
    let chat = TrayPlace::of(Command::Session {
        id: "s1".into(),
        title: "Chat".into(),
    });
    assert!(matches!(chat, Some(TrayPlace::Session(session)) if session.id == "s1"));
    for app in [
        Command::Feedback,
        Command::Open,
        Command::NewWindow,
        Command::Panel,
        Command::TogglePanel,
        Command::Quit,
    ] {
        assert_eq!(TrayPlace::of(app.clone()), None, "{app:?}");
    }
}

#[test]
fn a_chat_waits_for_the_sign_in_and_the_others_for_the_account() {
    let chat = TrayPlace::Session(SessionSummary {
        id: "s1".into(),
        title: "Chat".into(),
        day: None,
        at_ms: None,
    });
    assert!(!chat.ready(false, false));
    assert!(chat.ready(true, false));
    for place in [TrayPlace::NewChat, TrayPlace::Settings, TrayPlace::Updates] {
        assert!(!place.ready(false, false), "{place:?}");
        assert!(!place.ready(true, false), "{place:?}");
        assert!(place.ready(true, true), "{place:?}");
    }
}

#[gpui_kit::test]
fn tray_feedback_explains_why_an_account_is_needed_in_local_and_signed_out(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    for auth in [super::Auth::SignedOut { error: None }, super::Auth::Local] {
        workspace.update(cx, |workspace, _| workspace.auth = auth);
        tray(&workspace, cx, Command::Feedback);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.notices.current().unwrap().text, "Sign in to send feedback.");
            use super::overlay::OverlaySlot as _;
            assert!(!workspace.overlay.feedback());
        });
    }
}
