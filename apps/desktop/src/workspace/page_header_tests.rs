//! Headless: the Artifacts tab's page header draws its title menu, Full view and Close, and the Workbench's own
//! maximize gives way to the header's Full view, so the one state has one control.
use super::super::machine_source::MachineSource;
use super::super::side_pane::Tool;
use super::super::{Auth, Workspace};
use crate::gateway::ChatText;
use gpui_kit::{Entity, Modifiers, Task, TestAppContext, VisualTestContext};

fn open_page(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    open_page_titled(cx, "Capability Sheet")
}

fn open_page_titled<'a>(cx: &'a mut TestAppContext, title: &str) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let title = title.to_string();
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
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
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        workspace.side.fixture_width(640.);
        workspace.open_chat_text(
            ChatText {
                title,
                markdown: "# Capability Sheet".into(),
                path: "/artifacts/art_1".into(),
            },
            cx,
        );
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    draw(cx);
    // The Workbench slides in on the motion's clock: draw again once it has settled.
    cx.executor().advance_clock(crate::motion::PANEL * 2);
    draw(cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn the_page_header_has_its_controls_and_its_full_view_is_the_only_one(cx: &mut TestAppContext) {
    let (workspace, cx) = open_page(cx);
    assert!(workspace.read_with(cx, |workspace, _| workspace.side.showing(Tool::Artifacts)));
    assert!(
        cx.debug_bounds("page-title").is_some(),
        "the title opens the page's menu"
    );
    assert!(cx.debug_bounds("page-close").is_some(), "the header closes the page");
    assert!(
        cx.debug_bounds("side-maximize").is_none(),
        "on Artifacts the Workbench's maximize gives way to the header's Full view"
    );
    let full_view = cx.debug_bounds("page-full-view").expect("the header draws Full view");
    cx.simulate_click(full_view.center(), Modifiers::none());
    draw(cx);
    assert!(workspace.read_with(cx, |workspace, _| workspace.side.page_full()));
}

/// A title too long for the header ends before Full view instead of running under it.
#[gpui_kit::test]
fn a_long_title_stays_left_of_the_header_buttons(cx: &mut TestAppContext) {
    let long = "GPUI sidebar: session and project order vs Claude Code, ChatGPT and Codex, ".repeat(4);
    let (_, cx) = open_page_titled(cx, &long);
    let title = cx.debug_bounds("page-title").expect("the header draws the title");
    let full_view = cx.debug_bounds("page-full-view").expect("the header draws Full view");
    assert!(
        title.right() <= full_view.left(),
        "the title ends at {:?}, past Full view at {:?}",
        title.right(),
        full_view.left()
    );
    assert!(
        title.size.width > gpui_kit::px(200.),
        "the title keeps the header's room: {:?}",
        title.size.width
    );
}

/// A short title is as wide as its text, so its caret sits right after it.
#[gpui_kit::test]
fn a_short_title_keeps_its_own_width(cx: &mut TestAppContext) {
    let (_, cx) = open_page(cx);
    let title = cx.debug_bounds("page-title").expect("the header draws the title");
    let full_view = cx.debug_bounds("page-full-view").expect("the header draws Full view");
    assert!(
        title.right() < full_view.left() - gpui_kit::px(200.),
        "a short title does not stretch across the header: it ends at {:?}",
        title.right()
    );
}

/// One of your pages with a long title, at several Workbench widths: the title ends before Share, its lock included,
/// and Share ends before Full view. Above `SHARE_LABEL_MIN_WIDTH` Share has its label, below it its lock only.
#[gpui_kit::test]
fn a_long_title_stays_left_of_share_at_every_width(cx: &mut TestAppContext) {
    let long = "Quarterly roadmap review: setup and launch plan for the first four weeks";
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let pages = serde_json::json!({"pages": [{"id": "plan", "format": "md", "title": long}]});
    let lists =
        crate::library::Lists::from_responses(&serde_json::json!({}), &serde_json::json!({}), Some(&pages), None);
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
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        workspace.library.lists = Some(Ok(lists));
        workspace.library_opening = Some("page:plan".into());
        workspace.open_chat_text(
            ChatText {
                title: long.into(),
                markdown: "# Plan".into(),
                path: "/pages/plan".into(),
            },
            cx,
        );
        assert!(workspace.refresh_page_item(), "the page is one of yours");
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    draw(cx);
    cx.executor().advance_clock(crate::motion::PANEL * 2);
    for width in [900., 640., 520., 440., 400., 380., 340., 320.] {
        workspace.update(cx, |workspace, _| workspace.side.fixture_width(width));
        draw(cx);
        draw(cx);
        let title = cx.debug_bounds("page-title").expect("the header draws the title");
        let share = cx.debug_bounds("page-share").expect("your page has Share");
        let full_view = cx.debug_bounds("page-full-view").expect("the header draws Full view");
        assert!(
            title.right() <= share.left(),
            "at {width} px the title ends at {:?}, past Share at {:?}",
            title.right(),
            share.left()
        );
        assert!(
            share.right() <= full_view.left(),
            "at {width} px Share ends at {:?}, past Full view at {:?}",
            share.right(),
            full_view.left()
        );
    }
}
