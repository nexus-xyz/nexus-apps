//! Esc with a menu or any other overlay open closes it, and nothing else: it never stops the running turn or denies an
//! approval under it. Opening one overlay closes the one before it (`overlay.rs`). Headless: no credentials or agent
//! processes.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource};
use gpui_kit::{Entity, Keystroke, TestAppContext, VisualTestContext};
use std::sync::Arc;

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = gpui_kit::Task::ready(());
        workspace
    });
    workspace.update_in(cx, |workspace, _, _| {
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Chat;
    });
    (workspace, cx)
}

fn escape() -> Keystroke {
    Keystroke::parse("escape").unwrap()
}

#[gpui_kit::test]
fn esc_closes_an_open_menu_before_it_stops_the_answer(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // An answer is streaming.
        workspace.asking = Some(7);
        // The sidebar's project menu (… or a right click on the project).
        workspace.open_overlay(Overlay::ProjectMenu("project".into()));
        workspace.on_keystroke(&escape(), window, cx);
        assert_eq!(workspace.overlay.project_menu(), None, "Esc closes the project menu");
        assert_eq!(workspace.asking, Some(7), "and the answer goes on");
        // The chat's team chip menu.
        workspace.open_overlay(Overlay::TeamMenu);
        workspace.on_keystroke(&escape(), window, cx);
        assert!(!workspace.overlay.team_menu());
        assert_eq!(workspace.asking, Some(7));
        // The composer's "+" menu: Esc reaches the workspace before the composer's own key handler.
        workspace.open_overlay(Overlay::AttachMenu);
        workspace.on_keystroke(&escape(), window, cx);
        assert!(!workspace.overlay.attach_menu());
        assert_eq!(workspace.asking, Some(7));
        // With nothing open, Esc stops the answer.
        workspace.on_keystroke(&escape(), window, cx);
        assert_eq!(workspace.asking, None);
    });
}

#[gpui_kit::test]
fn a_pick_whose_session_is_gone_does_not_take_the_esc(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // A session was picked (Ctrl-click, Cmd on a Mac), then deleted, or the organization changed: no bar shows.
        assert!(workspace.pick_click("gone", gpui_kit::Modifiers::secondary_key(cx), cx));
        assert!(
            workspace
                .render_bulk_bar(
                    crate::palette::Palette::for_choice(workspace.theme, window.appearance()),
                    cx
                )
                .is_none()
        );
        workspace.asking = Some(7);
        workspace.on_keystroke(&escape(), window, cx);
        assert_eq!(workspace.asking, None, "Esc stops the answer the first time");
    });
}

/// One of each overlay kind. The match in `kind` has no wildcard, so a new `Overlay` variant fails to build until it
/// is added here too.
fn every_overlay(window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Workspace>) -> Vec<Overlay> {
    use crate::workspace::{
        chat_menu::ChatMenu, chat_rows::RowMenu, code_title::TopPanel, image_viewer::ImageViewer, link_menu::LinkPopup,
        new_code::LaunchMenu, org_menu::OrgMenu, session_menu::MenuPage, session_menu::SessionMenu,
    };
    let every = vec![
        Overlay::PersonMenu,
        Overlay::OrgMenu(OrgMenu::Loading),
        Overlay::PanelMenu,
        Overlay::ProjectMenu("project".into()),
        Overlay::SessionMenu(SessionMenu {
            id: "session".into(),
            page: MenuPage::Main,
            in_title: false,
        }),
        Overlay::ChatMenu(ChatMenu::Main),
        Overlay::RowMenu(RowMenu {
            id: "chat".into(),
            archived: false,
            confirm: false,
            moving: false,
        }),
        Overlay::LaunchMenu(LaunchMenu::Machine),
        Overlay::LinkPopup(LinkPopup::sample(window, cx)),
        Overlay::ModelMenu,
        Overlay::AttachMenu,
        Overlay::MicMenu,
        Overlay::TeamMenu,
        Overlay::AddGroupMenu,
        Overlay::StashMenu,
        Overlay::UsageMeter,
        Overlay::TeamSharing,
        Overlay::CodeTop(TopPanel::Meter("session".into())),
        Overlay::Palette(None),
        Overlay::ImageViewer(ImageViewer::sample()),
        Overlay::Feedback,
        Overlay::AssistantMenu,
    ];
    let mut kinds: Vec<usize> = every.iter().map(kind).collect();
    kinds.sort();
    kinds.dedup();
    assert_eq!(kinds, (0..KINDS).collect::<Vec<_>>(), "one of each kind");
    every
}

const KINDS: usize = 22;

fn kind(overlay: &Overlay) -> usize {
    match overlay {
        Overlay::PersonMenu => 0,
        Overlay::OrgMenu(_) => 1,
        Overlay::PanelMenu => 2,
        Overlay::ProjectMenu(_) => 3,
        Overlay::SessionMenu(_) => 4,
        Overlay::ChatMenu(_) => 5,
        Overlay::RowMenu(_) => 6,
        Overlay::LaunchMenu(_) => 7,
        Overlay::LinkPopup(_) => 8,
        Overlay::ModelMenu => 9,
        Overlay::AttachMenu => 10,
        Overlay::MicMenu => 11,
        Overlay::TeamMenu => 12,
        Overlay::AddGroupMenu => 13,
        Overlay::StashMenu => 14,
        Overlay::UsageMeter => 15,
        Overlay::CodeTop(_) => 16,
        Overlay::Palette(_) => 17,
        Overlay::ImageViewer(_) => 18,
        Overlay::Feedback => 19,
        Overlay::AssistantMenu => 20,
        Overlay::TeamSharing => 21,
    }
}

#[gpui_kit::test]
fn esc_closes_every_overlay_before_it_stops_the_answer(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        for overlay in every_overlay(window, cx) {
            let which = kind(&overlay);
            workspace.asking = Some(7);
            workspace.open_overlay(overlay);
            workspace.on_keystroke(&escape(), window, cx);
            assert!(workspace.overlay.is_none(), "Esc closes overlay kind {which}");
            assert_eq!(workspace.asking, Some(7), "and the answer goes on (kind {which})");
        }
        workspace.on_keystroke(&escape(), window, cx);
        assert_eq!(workspace.asking, None, "with nothing open, Esc stops the answer");
    });
}

#[gpui_kit::test]
fn opening_an_overlay_closes_the_one_before_it(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        // The person menu is open, then a session's "…" opens its menu.
        workspace.toggle_overlay(Overlay::PersonMenu);
        assert!(workspace.overlay.person_menu());
        workspace.toggle_session_menu("session", false, window, cx);
        assert!(!workspace.overlay.person_menu(), "the person menu closed");
        assert!(
            workspace
                .overlay
                .session_menu()
                .is_some_and(|menu| menu.id == "session")
        );
        // The palette opens over it: the session menu closes.
        workspace.open_palette(window, cx);
        assert!(workspace.overlay.palette().is_some());
        assert!(workspace.overlay.session_menu().is_none(), "the session menu closed");
        // A late close of a kind no longer open (a mouse down outside the old menu) leaves the palette open.
        assert!(workspace.overlay.close_session_menu().is_none());
        assert!(!workspace.overlay.close_person_menu());
        assert!(workspace.overlay.palette().is_some());
        // Send Feedback… replaces the palette.
        workspace.open_feedback(window, cx);
        assert!(workspace.overlay.feedback());
        assert!(workspace.overlay.palette().is_none());
    });
}

#[gpui_kit::test]
fn a_typed_pull_request_form_outlives_a_popup_opened_over_it(cx: &mut TestAppContext) {
    use crate::workspace::{code_title::TopPanel, link_menu::LinkPopup};
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.archived_code = vec![crate::workspace::session_keys::tests::session("session")];
        // The person opens the pull request form, types a description and sends the create.
        workspace.open_pr_form("session", window, cx);
        let form = workspace.pr_form.as_mut().expect("the form opens");
        form.type_description("Why and what", window, cx);
        form.start_create();
        // The palette's shortcut opens it over the form: the form, its text and the create in flight stay.
        workspace.open_palette(window, cx);
        assert!(workspace.overlay.palette().is_some());
        let form = workspace.pr_form.as_ref().expect("the form stays under the palette");
        assert_eq!(form.description(cx), "Why and what");
        assert!(form.working(), "and its create is still in flight");
        // Esc closes the palette, not the form.
        workspace.on_keystroke(&escape(), window, cx);
        assert!(workspace.overlay.palette().is_none(), "Esc closes the palette");
        assert!(workspace.pr_form.is_some(), "and leaves the form");
        // A link's menu, the same.
        workspace.open_overlay(Overlay::LinkPopup(LinkPopup::sample(window, cx)));
        assert!(workspace.pr_form.is_some(), "a link menu leaves the form");
        workspace.on_keystroke(&escape(), window, cx);
        assert!(workspace.overlay.is_none(), "Esc closes the link menu");
        assert!(workspace.pr_form.is_some(), "and leaves the form");
        // With no overlay over it, Esc closes the form, as before.
        workspace.asking = Some(7);
        workspace.on_keystroke(&escape(), window, cx);
        assert!(workspace.pr_form.is_none(), "then Esc closes the form");
        assert_eq!(workspace.asking, Some(7), "and the answer goes on");
        // Another top bar panel replaces the form, as one panel replaced another before.
        workspace.open_pr_form("session", window, cx);
        assert!(workspace.pr_form.is_some());
        workspace.open_overlay(Overlay::CodeTop(TopPanel::Meter("session".into())));
        assert!(workspace.pr_form.is_none(), "the meter's card replaces the form");
    });
}
