//! The real workspace's Code bar at several window widths: it sheds in steps as the window narrows, keeps every part
//! inside the window, and leaves the title its floor.
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};

use super::{Step, TITLE_FLOOR};
use crate::code::CodeProject;
use crate::workspace::agents_tool::AgentsTool;
use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::session_keys::tests::session;
use crate::workspace::{Auth, View, Workspace};

const TITLE: &str = "Make the dark theme the default for everyone, and ship it in the next build";
const BRANCH: &str = "nexus/we-want-to-make-the-dark-theme-the-default-theme";

/// A signed-in workspace with one Code session open: a long title, a long branch and a cost. No store, no agent.
fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = gpui_kit::Task::ready(());
        workspace.auth = Auth::SignedIn;
        workspace.title = TITLE.into();
        let mut open = session("fit");
        open.title = TITLE.into();
        open.branch = Some(BRANCH.into());
        open.cost_usd = 211.58;
        workspace.projects = vec![CodeProject {
            id: "project".into(),
            name: "project".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: vec![open],
            machine: None,
            added_at: 0,
        }];
        workspace.side.fixture_session("fit");
        workspace.side.agents = Some(AgentsTool::resting("fit".into(), cx));
        crate::project_commands::saved("project", Vec::new(), cx);
        let mut view = CodeView::new("fit".into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// Resize the window to `width` and draw until the bar's measures settle.
fn at_width(width: f32, cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(width), px(800.)));
    for _ in 0..4 {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }
    // One last frame, its bounds kept for the assertions.
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn the_bar_sheds_in_steps_and_stays_inside_the_window(cx: &mut TestAppContext) {
    let (workspace, cx) = workspace(cx);
    let mut last = Step::FULL;
    for width in [
        2400., 1600., 1300., 1150., 1050., 980., 900., 850., 800., 760., 720., 640., 600.,
    ] {
        at_width(width, cx);
        let step = workspace.read_with(cx, |workspace, _| workspace.title_fit.step);
        let git = cx.debug_bounds("code-git").expect("the git button is drawn");
        let title = cx.debug_bounds("code-title").expect("the title is drawn");
        let viewport = cx.update(|window, _| window.viewport_size().width);
        let (row, facts, tags) = workspace.read_with(cx, |workspace, _| {
            let fit = &workspace.title_fit;
            (fit.row.get(), fit.facts.get(), fit.tags.get())
        });
        eprintln!(
            "{width}px (viewport {viewport:?}, row {row}, facts {facts}, tags {tags}): step {step:?}, git {git:?}, title {title:?}"
        );
        assert!(
            step >= last,
            "{width}px: a narrower window never shows more ({step:?} after {last:?})"
        );
        assert!(
            git.right() <= px(width),
            "{width}px: the git button stays inside the window: {git:?}"
        );
        if step < Step::TIGHTEST {
            assert!(
                f32::from(title.size.width) + 1. >= TITLE_FLOOR,
                "{width}px: the title keeps its floor until the bar is at its tightest: {title:?}"
            );
        }
        last = step;
    }
    // A wide window shows everything; the narrowest sheds.
    at_width(2400., cx);
    assert_eq!(
        workspace.read_with(cx, |workspace, _| workspace.title_fit.step),
        Step::FULL
    );
    at_width(600., cx);
    assert_eq!(
        workspace.read_with(cx, |workspace, _| workspace.title_fit.step),
        Step::TIGHTEST,
        "the narrowest window reaches the tightest step"
    );
}
