//! A real drag through the drawn sidebar: the pointer presses on a session row, moves, and lets go.

use gpui_kit::{Entity, Modifiers, MouseButton, Pixels, Point, Task, TestAppContext, VisualTestContext, point, px};

use super::{Phase, Place, Target};
use crate::code::{CodeProject, CodeSession};
use crate::workspace::Workspace;

fn session(id: &str, updated_at: i64) -> CodeSession {
    let summary = serde_json::from_value(serde_json::json!({
        "id": id, "projectId": "p", "title": format!("Session {id}"), "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
        "nexusSessions": [], "contextTokens": null, "contextWindow": null,
        "createdAt": 0, "updatedAt": updated_at, "archived": false
    }))
    .unwrap();
    CodeSession::of(summary)
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    fixture_with(cx, |_| {})
}

/// The fixture, with its sessions changed by `change` before the sidebar draws them.
fn fixture_with(
    cx: &mut TestAppContext,
    change: impl FnOnce(&mut Workspace),
) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let gateway = crate::gateway::Gateway::for_view_test(crate::code_run::runtime());
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
        workspace.projects = vec![CodeProject {
            id: "p".into(),
            name: "Gateway".into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: vec![],
            machine: None,
            added_at: 0,
            sessions: vec![session("a", 3), session("b", 2), session("c", 1)],
        }];
        change(workspace);
        cx.notify();
    });
    cx.run_until_parked();
    (workspace, cx)
}

fn center(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center()
}

/// Presses at `from`, moves past the drag threshold and on to `to`, and lets go there.
fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(from + point(px(0.), px(12.)), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_session_dragged_below_another_lands_there_and_keeps_its_place(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let from = center(cx, "session-drag-a");
    let c = cx.debug_bounds("session-drag-c").unwrap();
    // The lower half of "c": "a" goes after it.
    let to = point(c.center().x, c.bottom() - px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, _, _| {
        assert!(
            matches!(
                workspace.session_drag.phase.over(),
                Some((_, Target::Row(spot))) if spot.id == "c" && spot.after
            ),
            "the line shows under c"
        );
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.groups.groups.session_order("p"), ["b", "c", "a"]);
        assert_eq!(workspace.session_drag.phase, Phase::Idle, "the drag is over");
        assert!(
            !matches!(&workspace.view, crate::workspace::View::Code(_)),
            "a drag never opens the session it moved"
        );
    });
    // Drawn again once the rows have glided to their places (`sidebar_hold.rs`), the project keeps the order it was
    // given.
    cx.executor().advance_clock(crate::motion::UI_LONG * 2);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let order = ["a", "b", "c"].map(|id| {
        let selector: &'static str = match id {
            "a" => "session-drag-a",
            "b" => "session-drag-b",
            _ => "session-drag-c",
        };
        (center(cx, selector).y, id)
    });
    let mut sorted = order;
    sorted.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    assert_eq!(sorted.map(|(_, id)| id), ["b", "c", "a"]);
}

#[gpui_kit::test]
fn a_session_dropped_among_the_pinned_ones_lands_there_pinned(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture_with(cx, |workspace| {
        workspace.projects[0].sessions[0].pinned = true;
    });
    // "a" is pinned, on top; "c" goes over it.
    let from = center(cx, "session-drag-c");
    let a = cx.debug_bounds("session-drag-a").unwrap();
    let to = point(a.center().x, a.top() + px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, _, _| {
        let over = workspace.session_drag.phase.over().map(|(_, target)| target.clone());
        let Some(Target::Row(spot)) = over else {
            panic!("over a's row, not {over:?}");
        };
        assert_eq!((spot.id.as_str(), spot.after, spot.pinned), ("a", false, true));
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.groups.groups.session_order("p"), ["c", "a", "b"]);
        assert_eq!(workspace.session_drag.phase, Phase::Idle);
    });
}

#[gpui_kit::test]
fn an_open_settled_group_takes_a_session_dropped_on_its_rows(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture_with(cx, |workspace| {
        workspace.projects[0].sessions[2].settled = true;
        workspace.open_aside.insert(crate::code::Aside::Settled);
    });
    let from = center(cx, "session-drag-a");
    let settled_row = center(cx, "session-drag-c");
    drag(cx, from, settled_row);
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(
            workspace.session_drag.phase.over().map(|(_, target)| target),
            Some(&Target::Group(Place::Aside(crate::code::Aside::Settled))),
            "a settled row is part of its group's target"
        );
    });
}

#[gpui_kit::test]
fn a_closed_group_springs_open_while_a_session_rests_over_it(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture_with(cx, |workspace| {
        workspace.projects[0].sessions[2].settled = true;
    });
    let from = center(cx, "session-drag-a");
    let head = center(cx, "session-drop-zone-Aside(Settled)");
    drag(cx, from, head);
    workspace.update_in(cx, |workspace, _, _| {
        assert!(
            !workspace.open_aside.contains(&crate::code::Aside::Settled),
            "not at once"
        );
    });
    cx.executor()
        .advance_clock(crate::workspace::session_drag_edge::SPRING * 2);
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert!(
            workspace.open_aside.contains(&crate::code::Aside::Settled),
            "it opened while the session rested over it"
        );
    });
}

#[gpui_kit::test]
fn esc_cancels_a_drag_and_nothing_lands(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let from = center(cx, "session-drag-a");
    let c = cx.debug_bounds("session-drag-c").unwrap();
    let to = point(c.center().x, c.bottom() - px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, window, cx| {
        assert!(workspace.session_drag.phase.over().is_some());
        assert!(workspace.cancel_session_drag(window, cx), "a drag went on");
        assert_eq!(workspace.session_drag.phase, Phase::Idle);
        assert!(!cx.has_active_drag(), "GPUI let go of it too");
        assert!(!workspace.cancel_session_drag(window, cx), "nothing left to cancel");
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert!(
            workspace.groups.groups.session_order("p").is_empty(),
            "the cancelled drag moved nothing"
        );
    });
}

#[gpui_kit::test]
fn a_picked_row_carries_the_other_picked_sessions(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let chord = if cfg!(target_os = "macos") {
        Modifiers::command()
    } else {
        Modifiers::control()
    };
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.pick_click("a", chord, cx);
        workspace.pick_click("b", chord, cx);
    });
    cx.run_until_parked();
    let from = center(cx, "session-drag-a");
    let c = cx.debug_bounds("session-drag-c").unwrap();
    let to = point(c.center().x, c.bottom() - px(4.));
    drag(cx, from, to);
    workspace.update_in(cx, |workspace, _, cx| {
        let drag = workspace.session_drag.drag(cx).expect("a drag goes on");
        let carried: Vec<&str> = drag.all().map(|one| one.id.as_str()).collect();
        assert_eq!(carried, ["a", "b"], "the dragged one first, then the picked one");
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.groups.groups.session_order("p"), ["c", "a", "b"]);
        assert!(
            !workspace.sidebar_filter.picked.has("a"),
            "the drop let go of the picked rows"
        );
    });
}

#[gpui_kit::test]
fn resting_over_snoozed_shows_its_presets_as_targets(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let from = center(cx, "session-drag-a");
    drag(cx, from, from + point(px(0.), px(40.)));
    cx.executor().advance_clock(crate::motion::COLLAPSE * 2);
    cx.run_until_parked();
    let head = center(cx, "session-drop-zone-Aside(Snoozed)");
    cx.simulate_mouse_move(head, MouseButton::Left, Modifiers::default());
    cx.executor()
        .advance_clock(crate::workspace::session_drag_edge::SPRING * 2);
    cx.executor().advance_clock(crate::motion::COLLAPSE * 2);
    cx.run_until_parked();
    let (label, at) = workspace.update_in(cx, |workspace, _, _| {
        let presets = workspace.session_drag.presets.clone().expect("the presets opened");
        presets[0]
    });
    let selector: &'static str = Box::leak(format!("session-drop-zone-snooze-{label}").into_boxed_str());
    let preset = center(cx, selector);
    cx.simulate_mouse_move(preset, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(
            workspace.session_drag.phase.over().map(|(_, target)| target),
            Some(&Target::SnoozeUntil(at))
        );
    });
    cx.simulate_mouse_up(preset, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.session_drag.phase, Phase::Idle);
        assert!(
            workspace.session_drag.presets.is_none(),
            "they close when the drag ends"
        );
    });
}

#[gpui_kit::test]
fn an_empty_settled_group_opens_as_a_target_only_during_a_drag(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let zone = "session-drop-zone-Aside(Settled)";
    let closed = cx.debug_bounds(zone).map(|bounds| bounds.size.height);
    assert!(
        closed.is_none_or(|height| height < px(1.)),
        "at rest an empty Settled takes no room"
    );
    let from = center(cx, "session-drag-b");
    drag(cx, from, from + point(px(0.), px(40.)));
    // The zone folds open over `motion::COLLAPSE`.
    cx.executor().advance_clock(crate::motion::COLLAPSE * 2);
    cx.run_until_parked();
    let open = cx.debug_bounds(zone).expect("Settled opens during the drag");
    assert!(open.size.height > px(1.));
    let target = open.center();
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(
            workspace.session_drag.phase.over().map(|(_, target)| target),
            Some(&Target::Group(Place::Aside(crate::code::Aside::Settled)))
        );
    });
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, _, _| {
        assert_eq!(workspace.session_drag.phase, Phase::Idle);
        assert!(
            workspace.groups.groups.session_order("p").is_empty(),
            "settling moves no session in its project"
        );
    });
}
