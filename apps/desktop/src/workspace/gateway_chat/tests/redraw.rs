//! A streamed token draws the main area again, never the sidebar: nothing the sidebar shows changed.
use super::{Probe, ask, delta, fixture, named};
use crate::workspace::pane::{Region, drawn};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
fn a_streamed_token_draws_the_main_area_and_not_the_sidebar(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "Question");
    named(&probe, &a, "a");
    delta(&probe, &a, "first ");
    cx.run_until_parked();
    let (sidebar, center) = (drawn::count(Region::Sidebar), drawn::count(Region::Center));
    assert!(sidebar > 0 && center > 0, "both panes drew once the chat opened");
    for token in ["two ", "three ", "four"] {
        delta(&probe, &a, token);
        cx.run_until_parked();
    }
    let (sidebar_after, center_after) = (drawn::count(Region::Sidebar), drawn::count(Region::Center));
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.entries.last().expect("answer").streamed,
            "first two three four"
        );
    });
    assert!(
        center_after >= center + 3,
        "each token draws the main area: {center} -> {center_after}"
    );
    assert_eq!(sidebar_after, sidebar, "a token never draws the sidebar again");
}

#[gpui_kit::test]
fn the_answering_chats_spinner_turns_without_drawing_the_sidebar(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    // The sidebar lists the chats once they load: the fixture's list shows the loading rows.
    workspace.update(cx, |workspace, _| workspace.sessions_loaded = true);
    let a = ask(&workspace, cx, &probe, "Question");
    named(&probe, &a, "a");
    delta(&probe, &a, "first ");
    cx.run_until_parked();
    let shown = workspace.read_with(cx, |workspace, _| workspace.sidebar_marks.shown());
    assert_eq!(
        shown,
        vec![("a".into(), crate::code::Status::Running)],
        "the answering chat's row spins, over the sidebar"
    );
    // The first frame settles what the chat's start left pending; the frames after it are the spinner's alone.
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let sidebar = drawn::count(Region::Sidebar);
    let mut frames = 0;
    for _ in 0..5 {
        frames += cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    assert!(frames >= 5, "the spinner asks for each next frame");
    assert_eq!(
        drawn::count(Region::Sidebar),
        sidebar,
        "a spinner's frame never draws the sidebar again"
    );
}
