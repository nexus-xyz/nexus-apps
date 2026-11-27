//! The edge cue's rule, and the cue in a drawn sidebar: it shows for a waiting session out of sight, hides while a
//! search narrows the list, and goes to the session when pressed.

use super::*;
use crate::code::{CodeProject, CodeSession};
use core::prelude::v1::test;
use gpui_kit::{Modifiers, MouseButton, TestAppContext, VisualTestContext};

fn row(top: f32) -> Spot {
    Spot {
        top,
        bottom: top + 14.,
        folded: false,
    }
}

fn folded(top: f32) -> Spot {
    Spot {
        folded: true,
        ..row(top)
    }
}

#[test]
fn no_cue_while_every_waiting_row_is_in_view() {
    assert_eq!(out_of_view(&[], 100., 500.), None);
    assert_eq!(out_of_view(&[row(120.), row(480.)], 100., 500.), None);
}

#[test]
fn a_row_below_the_view_puts_the_cue_on_the_bottom_edge_and_one_above_on_the_top() {
    assert_eq!(
        out_of_view(&[row(120.), row(700.)], 100., 500.),
        Some(Out {
            nearest: 1,
            edge: Edge::Bottom,
            count: 1
        })
    );
    assert_eq!(
        out_of_view(&[row(20.), row(300.)], 100., 500.),
        Some(Out {
            nearest: 0,
            edge: Edge::Top,
            count: 1
        })
    );
}

#[test]
fn the_nearest_of_several_out_of_sight_leads_and_all_are_counted() {
    let out = out_of_view(&[row(-400.), row(900.), row(540.), row(60.)], 100., 500.);
    assert_eq!(
        out,
        Some(Out {
            nearest: 3,
            edge: Edge::Top,
            count: 4
        })
    );
}

#[test]
fn a_row_half_cut_by_the_edge_is_in_view_when_its_middle_is() {
    // Its mark runs from 490 to 504: the middle, 497, is in view.
    assert_eq!(out_of_view(&[row(490.)], 100., 500.), None);
    assert!(out_of_view(&[row(494.)], 100., 500.).is_some());
}

#[test]
fn a_session_in_a_folded_project_is_out_of_sight_even_under_a_head_in_view() {
    assert_eq!(
        out_of_view(&[folded(200.)], 100., 500.),
        Some(Out {
            nearest: 0,
            edge: Edge::Bottom,
            count: 1
        })
    );
    // Its head in view is nearer than a row below the view.
    assert_eq!(
        out_of_view(&[row(600.), folded(200.)], 100., 500.).map(|out| (out.nearest, out.count)),
        Some((1, 2))
    );
    // A folded project above the view points up.
    assert_eq!(
        out_of_view(&[folded(10.)], 100., 500.).map(|out| out.edge),
        Some(Edge::Top)
    );
}

#[test]
fn going_to_a_row_brings_it_to_the_middle_of_the_view_as_far_as_the_list_scrolls() {
    // The row's middle is at 707; the view's middle at 300.
    assert_eq!(offset_to_show(0., 1000., row(700.), 100., 500.), -407.);
    assert_eq!(
        offset_to_show(-200., 1000., row(20.), 100., 500.),
        0.,
        "not past the top"
    );
    assert_eq!(
        offset_to_show(0., 150., row(700.), 100., 500.),
        -150.,
        "not past the end"
    );
}

#[test]
fn the_cue_sits_inside_the_list_on_its_edge() {
    let frame = Bounds::new(point(px(8.), px(40.)), size(px(260.), px(800.)));
    let list = Bounds::new(point(px(8.), px(240.)), size(px(260.), px(500.)));
    assert_eq!(
        place(Edge::Bottom, list, frame),
        Bounds::new(point(px(8.), px(700. - 8. - ROW)), size(px(244.), px(ROW)))
    );
    assert_eq!(
        place(Edge::Top, list, frame),
        Bounds::new(point(px(8.), px(208.)), size(px(244.), px(ROW)))
    );
}

fn session(id: &str, updated_at: i64, waiting: bool) -> CodeSession {
    let summary = serde_json::from_value(serde_json::json!({
        "id": id, "projectId": "p", "title": format!("Session {id}"), "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
        "nexusSessions": [], "contextTokens": null, "contextWindow": null,
        "createdAt": 0, "updatedAt": updated_at, "archived": false
    }))
    .unwrap();
    let mut session = CodeSession::of(summary);
    // A turn the plan's limit stopped needs the person (`seen.rs`).
    if waiting {
        session.limited = Some(nexus_workbench::event::LimitStop {
            resets_at: None,
            scheduled: false,
        });
    }
    session
}

/// A drawn Workspace with one project of 80 sessions; the last, far below the list's view, waits on the person.
fn fixture(cx: &mut TestAppContext, fold: bool) -> (Entity<Workspace>, &mut VisualTestContext) {
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
        let sessions = (0..80)
            .map(|ix| session(&format!("s{ix}"), 1000 - ix, ix == 79))
            .collect();
        workspace.projects = vec![CodeProject {
            id: "p".into(),
            name: "Gateway".into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: vec![],
            machine: None,
            added_at: 0,
            sessions,
        }];
        if fold {
            workspace.folded_projects.insert("p".into());
        }
        cx.notify();
    });
    settle(cx);
    (workspace, cx)
}

/// Lets the list draw, the cue read it and draw again, and any motion end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
    }
    cx.run_until_parked();
}

fn shown(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Cue> {
    let cue = workspace.read_with(cx, |workspace, _| workspace.sidebar_edge_cue.clone());
    cue.read_with(cx, |cue, _| cue.shown().cloned())
}

fn open_session(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
    workspace.read_with(cx, |workspace, _| match &workspace.view {
        crate::workspace::View::Code(view) => Some(view.id.clone()),
        _ => None,
    })
}

fn press(cx: &mut VisualTestContext) {
    let at = cx.debug_bounds("sidebar-edge-cue").expect("the cue is drawn").center();
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    settle(cx);
}

#[gpui_kit::test]
fn a_waiting_session_below_the_view_shows_the_cue_and_a_press_opens_it_in_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, false);
    let cue = shown(&workspace, cx).expect("the cue shows");
    assert_eq!(cue.edge, Edge::Bottom);
    assert_eq!(cue.count, 1);
    assert_eq!(cue.target.session, "s79");
    assert_eq!(cue.target.title.as_ref(), "Session s79");
    assert_eq!(cue.target.project.as_ref(), "Gateway");

    // A search narrows the list: no cue.
    let hidden = workspace.read_with(cx, |workspace, cx| {
        let list = workspace.sidebar_filter.scroll.bounds();
        let snapshot = Snapshot {
            marks: workspace.sidebar_marks.waiting(),
            list,
            frame: list,
            filtering: true,
        };
        workspace.edge_cue(&snapshot, cx)
    });
    assert_eq!(hidden, None, "no cue while a search narrows the list");

    press(cx);
    assert_eq!(
        open_session(&workspace, cx).as_deref(),
        Some("s79"),
        "the press opens it"
    );
    let scroll = workspace.read_with(cx, |workspace, _| workspace.sidebar_filter.scroll.offset());
    assert!(scroll.y < px(0.), "the list scrolled down to it: {scroll:?}");
    assert_eq!(shown(&workspace, cx), None, "open, it no longer waits: the cue leaves");
}

#[gpui_kit::test]
fn a_waiting_session_in_a_folded_project_unfolds_it_and_opens(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, true);
    let cue = shown(&workspace, cx).expect("the cue shows for the folded project");
    assert_eq!(cue.edge, Edge::Bottom);
    assert_eq!(cue.target.session, "s79");
    assert_eq!(cue.target.folded.as_deref(), Some("p"));

    press(cx);
    assert!(
        workspace.read_with(cx, |workspace, _| !workspace.folded_projects.contains("p")),
        "the project unfolds"
    );
    assert_eq!(
        open_session(&workspace, cx).as_deref(),
        Some("s79"),
        "and its session opens"
    );
    assert_eq!(shown(&workspace, cx), None);
}

#[gpui_kit::test]
fn the_cue_hides_once_the_waiting_row_scrolls_into_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx, false);
    assert!(shown(&workspace, cx).is_some());
    workspace.update(cx, |workspace, cx| {
        let max = workspace.sidebar_filter.scroll.max_offset();
        workspace.sidebar_filter.scroll.set_offset(point(px(0.), -max.y));
        cx.notify();
    });
    settle(cx);
    assert_eq!(shown(&workspace, cx), None, "the row is at the list's foot, in view");
}
