// Named, not `super::*`: the kit's glob brings its own `test` attribute.
use super::{NO_UPDATE, Offer, RevokedNotice, content};
use crate::revoked::Revoked;
use nexus_desktop_updates::{
    App, Coordinator, DOWNLOAD_PAGE, Disabled, Failure, INTERNAL_INSTALL, Phase, Snapshot, Trigger,
};

const MESSAGE: &str = "This version of the Nexus app (0.11.0) was withdrawn. Update Nexus to continue.";

fn revoked() -> Revoked {
    Revoked {
        version: "0.11.0".into(),
        message: MESSAGE.into(),
    }
}

fn idle() -> Snapshot {
    Coordinator::of(App::Desktop, "0.11.0".into(), None, 0).snapshot
}

fn checked(result: Result<Option<String>, Failure>) -> Snapshot {
    let mut coordinator = Coordinator::of(App::Desktop, "0.11.0".into(), None, 0);
    assert!(coordinator.begin(Trigger::Manual, 0));
    coordinator.finish(result, 0, 0);
    coordinator.snapshot
}

#[test]
fn it_says_gateways_words_and_offers_the_download_page_without_an_updater() {
    let notice = content(&revoked(), None, false, None, None);
    assert_eq!(notice.title, MESSAGE);
    assert_eq!(notice.note, NO_UPDATE);
    assert_eq!(notice.offer, Offer::Download(DOWNLOAD_PAGE.into()));
    assert!(!notice.check_again);
    assert!(!notice.problem);
}

#[test]
fn a_verified_update_offers_restart_and_a_check_in_progress_waits() {
    let checking = Snapshot {
        phase: Phase::Checking,
        ..idle()
    };
    let notice = content(&revoked(), Some(&checking), false, None, None);
    assert_eq!(notice.offer, Offer::Waiting);
    assert_eq!(notice.note, "Checking for updates…");
    let ready = checked(Ok(Some("0.11.1".into())));
    assert_eq!(ready.phase, Phase::Ready);
    let notice = content(&revoked(), Some(&ready), true, None, None);
    assert_eq!(notice.offer, Offer::Restart);
    assert!(notice.note.contains("0.11.1"), "{}", notice.note);
    // Ready without verified bytes is never a restart.
    assert_ne!(
        content(&revoked(), Some(&ready), false, None, None).offer,
        Offer::Restart
    );
    let installing = Snapshot {
        phase: Phase::Installing,
        ..ready.clone()
    };
    assert_eq!(
        content(&revoked(), Some(&installing), true, None, None).offer,
        Offer::Restarting
    );
    // A platform that only announces the release offers its download.
    let url = "https://downloads.nexus.inc/desktop/stable/Nexus.dmg".to_string();
    assert_eq!(
        content(&revoked(), Some(&ready), false, Some(url.clone()), None).offer,
        Offer::Download(url)
    );
}

#[test]
fn no_update_or_a_failed_check_offers_the_download_page_and_another_check() {
    let none = checked(Ok(None));
    let notice = content(&revoked(), Some(&none), false, None, None);
    assert_eq!(notice.offer, Offer::Download(DOWNLOAD_PAGE.into()));
    assert_eq!(notice.note, NO_UPDATE);
    assert!(notice.check_again);
    let failed = checked(Err(Failure::new(
        "network",
        "Nexus could not reach the update feed.",
        true,
    )));
    let notice = content(&revoked(), Some(&failed), false, None, None);
    assert_eq!(notice.note, "Nexus could not reach the update feed.");
    assert_eq!(notice.offer, Offer::Download(DOWNLOAD_PAGE.into()));
    assert!(notice.check_again);
}

#[test]
fn a_build_that_cannot_update_itself_offers_its_channels_manual_way() {
    let stable = Coordinator::of(App::Desktop, "0.11.0".into(), Some(Disabled::MissingKey), 0).snapshot;
    let notice = content(&revoked(), Some(&stable), false, None, None);
    assert_eq!(notice.offer, Offer::Download(DOWNLOAD_PAGE.into()));
    assert!(!notice.check_again);
    let internal = Coordinator::of(App::Desktop, "0.11.0-internal.42".into(), Some(Disabled::MissingKey), 0).snapshot;
    let notice = content(&revoked(), Some(&internal), false, None, None);
    // Release builds carry the internal installer (`NEXUS_DESKTOP_INTERNAL_INSTALL`); other builds offer the page.
    let expected = INTERNAL_INSTALL.map_or(Offer::Download(DOWNLOAD_PAGE.into()), |command| {
        Offer::Command(command.into())
    });
    assert_eq!(notice.offer, expected);
}

#[test]
fn a_refused_restart_says_why_in_red() {
    let ready = checked(Ok(Some("0.11.1".into())));
    let notice = content(
        &revoked(),
        Some(&ready),
        true,
        None,
        Some("Stop the running sessions first."),
    );
    assert_eq!(notice.note, "Stop the running sessions first.");
    assert!(notice.problem);
    assert_eq!(notice.offer, Offer::Restart);
}

#[test]
fn the_notice_shows_the_first_refusal_once() {
    let mut notice = RevokedNotice::default();
    assert!(!notice.shown());
    assert!(!notice.accept(None));
    assert!(notice.accept(Some(revoked())));
    assert!(notice.shown());
    // A later refusal, or the same one again, changes nothing: no second notice, no redraw.
    let later = Revoked {
        message: "later".into(),
        ..revoked()
    };
    assert!(!notice.accept(Some(later)));
    assert!(!notice.accept(Some(revoked())));
    assert_eq!(
        notice.revoked.as_ref().map(|revoked| revoked.message.as_str()),
        Some(MESSAGE)
    );
}

/// An app window (or the panel's) on `gateway`, with no machines and no Gateway events.
fn open_window(
    gateway: std::sync::Arc<dyn crate::backend::Backend>,
    panel: bool,
    cx: &mut gpui_kit::TestAppContext,
) -> (
    gpui_kit::Entity<crate::workspace::Workspace>,
    &mut gpui_kit::VisualTestContext,
) {
    use crate::workspace::{Workspace, machine_source::MachineSource};
    // Background HTTP replies must not wake GPUI's deterministic test scheduler.
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    cx.add_window_view(move |window, cx| {
        Workspace::with_machine_source(gateway, events, arrivals, panel, MachineSource::isolated(), window, cx)
    })
}

/// Whether `cx`'s window, drawn now, shows the card.
fn drawn(cx: &mut gpui_kit::VisualTestContext) -> bool {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.debug_bounds("revoked-notice").is_some()
}

/// The first refusal reaches the app window, drawn as one card, and a later refusal changes nothing.
#[gpui_kit::test]
fn a_refusal_draws_one_card_in_the_app_window(cx: &mut gpui_kit::TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    let latch = gateway.revoked();
    let (workspace, cx) = open_window(std::sync::Arc::new(gateway), false, cx);
    assert!(!drawn(cx), "a working build shows no card");
    assert!(latch.note(revoked()));
    assert!(drawn(cx));
    assert!(!latch.note(Revoked {
        message: "later".into(),
        ..revoked()
    }));
    assert!(drawn(cx));
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace
                .revoked_notice
                .revoked
                .as_ref()
                .map(|revoked| revoked.message.as_str()),
            Some(MESSAGE)
        );
    });
}

/// A window opened after the refusal shows it at once. The panel's window shows none.
#[gpui_kit::test]
fn a_later_window_shows_the_kept_refusal_and_the_panel_none(cx: &mut gpui_kit::TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    assert!(gateway.revoked().note(revoked()));
    let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
    let (_, window) = open_window(gateway.clone(), false, cx);
    assert!(drawn(window));
    let (_, panel) = open_window(gateway, true, cx);
    assert!(!drawn(panel));
}
