//! The project drag's state, then a real drag through the drawn sidebar.

use std::rc::Rc;

use gpui_kit::{Entity, Modifiers, MouseButton, Pixels, Point, Task, TestAppContext, VisualTestContext, point, px};

use super::{ProjectDragState, ProjectSlot, Spot};
use crate::code::CodeProject;
use crate::workspace::Workspace;

fn slot(key: &str, pinned: bool) -> ProjectSlot {
    ProjectSlot {
        key: key.into(),
        pinned,
        listed: ["pin", "a", "b", "c"]
            .map(String::from)
            .into_iter()
            .collect::<Rc<[String]>>(),
    }
}

#[test]
fn a_project_lands_before_or_after_another_pinned_as_it_is() {
    let mut state = ProjectDragState::default();
    state.lift("a".into(), false);
    assert!(state.moved(&slot("c", false), true, true));
    assert_eq!(
        state.over,
        Some(Spot {
            key: "c".into(),
            after: true
        })
    );
    assert_eq!(
        state.dropped(&slot("c", false)).unwrap(),
        ["pin", "b", "c", "a"],
        "after c"
    );
    assert!(state.lifted.is_none(), "the drag is over");

    state.lift("c".into(), false);
    state.moved(&slot("a", false), true, false);
    assert_eq!(
        state.dropped(&slot("a", false)).unwrap(),
        ["pin", "c", "a", "b"],
        "before a"
    );
}

#[test]
fn a_pinned_project_or_itself_never_takes_the_drag() {
    let mut state = ProjectDragState::default();
    state.lift("a".into(), false);
    assert!(!state.moved(&slot("pin", true), true, false), "pinned stays above");
    assert!(!state.moved(&slot("a", false), true, true), "on itself");
    assert_eq!(state.over, None);
}

#[test]
fn leaving_one_project_never_clears_another_s_spot() {
    let mut state = ProjectDragState::default();
    state.lift("a".into(), false);
    state.moved(&slot("b", false), true, true);
    assert!(
        !state.moved(&slot("c", false), false, true),
        "out of c, which had nothing"
    );
    assert_eq!(state.over.as_ref().map(|spot| spot.key.as_str()), Some("b"));
    assert!(state.moved(&slot("b", false), false, true), "out of b");
    assert_eq!(state.over, None);
}

#[test]
fn a_drop_where_it_is_or_after_a_cancel_keeps_no_order() {
    let mut state = ProjectDragState::default();
    state.lift("a".into(), false);
    state.moved(&slot("b", false), true, false);
    assert_eq!(state.dropped(&slot("b", false)), None, "before b, where it is");
    state.lift("a".into(), false);
    state.moved(&slot("c", false), true, true);
    assert!(state.cancel());
    assert_eq!(state.dropped(&slot("c", false)), None);
    assert!(!state.cancel(), "nothing left to cancel");
}

fn project(id: &str, added_at: i64) -> CodeProject {
    CodeProject {
        id: id.into(),
        name: format!("Project {id}"),
        branch: Some("main".into()),
        pinned: false,
        remotes: vec![],
        machine: None,
        added_at,
        sessions: vec![],
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _gateway_events) = crate::gateway::Gateway::start();
    let (_send, events) = futures::channel::mpsc::unbounded();
    let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            crate::workspace::machine_source::MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace
    });
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.auth = crate::workspace::Auth::SignedIn;
        workspace.view = crate::workspace::View::Chat;
        // The newest added first: "p1", then "p2".
        workspace.projects = vec![project("p1", 2), project("p2", 1)];
        cx.notify();
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn bounds_center(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center()
}

/// Presses at `from`, moves past the drag threshold and on to `to`.
fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(from + point(px(0.), px(12.)), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_project_dragged_below_another_lands_there_and_keeps_its_place(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let from = bounds_center(cx, "project-drag-code:p1");
    let p2 = cx.debug_bounds("project-drop-code:p2").unwrap();
    let to = point(p2.center().x, p2.bottom() - px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(
            workspace.project_drag.over,
            Some(Spot {
                key: "code:p2".into(),
                after: true
            }),
            "the line shows under p2"
        );
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.groups.groups.project_order(), ["code:p2", "code:p1"]);
        assert!(workspace.project_drag.lifted.is_none(), "the drag is over");
        assert!(
            !workspace.folded_projects.contains("p1"),
            "a drag never folds the project it moved"
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let p1 = bounds_center(cx, "project-drag-code:p1").y;
    let p2 = bounds_center(cx, "project-drag-code:p2").y;
    assert!(p2 < p1, "p2 now shows above p1");
}

#[gpui_kit::test]
fn esc_cancels_a_project_drag_and_nothing_lands(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let from = bounds_center(cx, "project-drag-code:p1");
    let p2 = cx.debug_bounds("project-drop-code:p2").unwrap();
    let to = point(p2.center().x, p2.bottom() - px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.project_drag.over.is_some());
        assert!(workspace.cancel_project_drag(window, cx), "a drag went on");
        assert!(!cx.has_active_drag(), "GPUI let go of it too");
        assert!(!workspace.cancel_project_drag(window, cx), "nothing left to cancel");
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert!(workspace.groups.groups.project_order().is_empty(), "nothing moved");
    });
}
