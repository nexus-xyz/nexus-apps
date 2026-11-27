//! The hold's rule, its moves, and a session that arrives under the pointer in the drawn sidebar.

// `gpui_kit::*` brings GPUI's own `test`; the plain tests here take the standard one.
use core::prelude::v1::test;
use std::cell::Cell;
use std::time::{Duration, Instant};

use super::*;
use crate::code::{CodeProject, CodeSession};

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn held_rows_keep_their_order_and_a_new_one_waits() {
    let placed = place(&ids(&["a", "b", "c"]), &["d", "c", "a", "b"], true);
    assert_eq!(placed.shown, ["a", "b", "c"]);
    assert!(
        placed.moved.is_empty() && placed.arrived.is_empty(),
        "nothing moves while held"
    );
}

#[test]
fn a_session_that_goes_leaves_at_once_while_held() {
    let placed = place(&ids(&["a", "b", "c"]), &["c", "a"], true);
    assert_eq!(placed.shown, ["a", "c"]);
}

#[test]
fn released_rows_glide_by_the_rows_they_moved_and_a_new_one_arrives() {
    let placed = place(&ids(&["a", "b", "c"]), &["c", "d", "a", "b"], false);
    assert_eq!(placed.shown, ["c", "d", "a", "b"]);
    // Among the rows on screen before and after: c was third and is first; a and b each go down one.
    assert_eq!(
        placed.moved,
        [("c".to_string(), 2.), ("a".to_string(), -1.), ("b".to_string(), -1.)]
    );
    assert_eq!(placed.arrived, ["d"]);
}

#[test]
fn a_row_that_arrives_or_leaves_moves_no_other() {
    let placed = place(&ids(&["a", "b", "c"]), &["new", "a", "c"], false);
    assert!(placed.moved.is_empty(), "{:?}", placed.moved);
    assert_eq!(placed.arrived, ["new"]);
}

#[test]
fn the_same_order_changes_nothing() {
    let placed = place(&ids(&["a", "b"]), &["a", "b"], false);
    assert_eq!(
        placed,
        Placed {
            shown: ids(&["a", "b"]),
            ..Placed::default()
        }
    );
}

#[test]
fn a_released_row_starts_where_it_was_and_rests_in_its_place() {
    let hold = SidebarHold::default();
    let start = Instant::now();
    hold.order("p", &["a", "b"], &[], false, start, false);
    assert_eq!(
        hold.order("p", &["b", "a"], &[], true, start, false),
        ["a", "b"],
        "held"
    );
    assert!(!hold.moving("p", start));
    let shown = hold.order("p", &["b", "a"], &[], false, start, false);
    assert_eq!(shown, ["b", "a"], "released");
    let projects = hold.projects.borrow();
    let shown = &projects["p"];
    assert_eq!(shown.offset("b", start), ROW, "b starts a row down, where it was");
    assert_eq!(shown.offset("a", start), -ROW);
    let half = shown.offset("b", start + GLIDE / 2);
    assert!(half > 0. && half < ROW, "{half}");
    assert_eq!(shown.offset("b", start + GLIDE), 0.);
    assert!(!shown.moving(start + GLIDE), "at rest after the glide");
}

#[test]
fn an_arriving_row_opens_from_no_height() {
    let hold = SidebarHold::default();
    let start = Instant::now();
    hold.order("p", &["a"], &[], false, start, false);
    hold.order("p", &["new", "a"], &[], false, start, false);
    assert_eq!(hold.shortfall("p", start), ROW);
    let half = hold.shortfall("p", start + GLIDE / 2);
    assert!(half > 0. && half < ROW, "{half}");
    assert_eq!(hold.shortfall("p", start + GLIDE), 0.);
}

#[test]
fn less_motion_puts_each_row_in_its_place_at_once() {
    let hold = SidebarHold::default();
    let start = Instant::now();
    hold.order("p", &["a", "b"], &[], false, start, true);
    hold.order("p", &["new", "b", "a"], &[], false, start, true);
    assert!(!hold.moving("p", start));
    assert_eq!(hold.shortfall("p", start), 0.);
}

#[test]
fn a_drop_shows_while_held_and_the_dropped_row_lands_in_place() {
    let hold = SidebarHold::default();
    let start = Instant::now();
    hold.order("p", &["a", "b", "c"], &[], true, start, false);
    // "a" dropped after "c": the drop stores the project's order, and the pointer still holds the rows.
    let dragged = ids(&["b", "c", "a"]);
    assert_eq!(
        hold.order("p", &["b", "c", "a"], &dragged, true, start, false),
        ["b", "c", "a"]
    );
    let projects = hold.projects.borrow();
    assert_eq!(projects["p"].offset("a", start), 0., "the dropped row does not glide");
    assert_eq!(projects["p"].offset("b", start), ROW, "the others make room");
    drop(projects);
    // Once taken, the hold applies again.
    assert_eq!(
        hold.order("p", &["c", "b", "a"], &dragged, true, start, false),
        ["b", "c", "a"]
    );
}

#[test]
fn the_dropped_row_is_the_one_that_moved_furthest() {
    assert_eq!(
        dropped(&ids(&["a", "b", "c"]), &ids(&["b", "c", "a"])),
        Some("a".into())
    );
    assert_eq!(
        dropped(&ids(&["a", "b", "c"]), &ids(&["c", "a", "b"])),
        Some("c".into())
    );
    assert_eq!(
        dropped(&ids(&["a", "b"]), &ids(&["b", "a"])),
        None,
        "a swap of neighbours"
    );
}

/// A view that draws one project's rows through a hold.
struct Rows {
    hold: SidebarHold,
    order: Vec<&'static str>,
}

impl Render for Rows {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let shown = self.hold.order("p", &self.order, &[], false, now, cx.reduce_motion());
        let rows: Vec<AnyElement> = shown
            .iter()
            .map(|id| {
                let row = div().h(px(ROW)).into_any_element();
                self.hold.draw("p", id, row, now, window)
            })
            .collect();
        div().children(rows)
    }
}

/// How many frames the rows ask for over `span` of the test clock, drawn again each millisecond as a display would.
fn asks_over(rows: &Entity<Rows>, span: Duration, cx: &mut VisualTestContext) -> usize {
    let before = ASKED.with(Cell::get);
    let step = Duration::from_millis(1);
    let mut elapsed = Duration::ZERO;
    while elapsed < span {
        cx.executor().advance_clock(step);
        cx.run_until_parked();
        rows.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        elapsed += step;
    }
    ASKED.with(Cell::get) - before
}

#[gpui_kit::test]
fn rows_ask_for_frames_only_while_they_glide(cx: &mut TestAppContext) {
    let (rows, cx) = cx.add_window_view(|_, _| Rows {
        hold: SidebarHold::default(),
        order: vec!["a", "b", "c"],
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(asks_over(&rows, Duration::from_secs(1), cx), 0, "at rest: no frames");

    rows.update(cx, |rows, cx| {
        rows.order = vec!["c", "a", "b"];
        cx.notify();
    });
    let gliding = asks_over(&rows, GLIDE + Duration::from_millis(50), cx);
    assert!(gliding > 100, "{gliding} frames while it glides");
    assert_eq!(
        asks_over(&rows, Duration::from_secs(1), cx),
        0,
        "rows at rest after a glide ask for no frames"
    );
}

#[gpui_kit::test]
fn under_less_motion_a_reorder_asks_for_no_frames(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (rows, cx) = cx.add_window_view(|_, _| Rows {
        hold: SidebarHold::default(),
        order: vec!["a", "b"],
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    rows.update(cx, |rows, cx| {
        rows.order = vec!["new", "b", "a"];
        cx.notify();
    });
    assert_eq!(asks_over(&rows, Duration::from_millis(300), cx), 0);
}

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
        cx.notify();
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// Draws again once every move is over.
fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(GLIDE * 2);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn selector(id: &str) -> &'static str {
    Box::leak(format!("session-drag-{id}").into_boxed_str())
}

/// The rows of `ids` drawn now, top to bottom.
fn drawn(cx: &mut VisualTestContext, ids: &[&'static str]) -> Vec<&'static str> {
    let mut rows: Vec<(Pixels, &'static str)> = ids
        .iter()
        .filter_map(|id| cx.debug_bounds(selector(id)).map(|bounds| (bounds.top(), *id)))
        .collect();
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    rows.into_iter().map(|(_, id)| id).collect()
}

#[gpui_kit::test]
fn a_session_that_arrives_under_the_pointer_waits_until_the_pointer_leaves(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let (workspace, cx) = fixture(cx);
    let row = cx.debug_bounds(selector("a")).expect("a is drawn");
    cx.simulate_mouse_move(row.center(), None, Modifiers::default());
    workspace.update_in(cx, |workspace, _, cx| {
        workspace.projects[0].sessions.insert(0, session("d", 4));
        cx.notify();
    });
    settle(cx);
    assert_eq!(
        drawn(cx, &["a", "b", "c", "d"]),
        ["a", "b", "c"],
        "the new session waits while the pointer is on the list"
    );

    cx.simulate_mouse_move(
        point(row.right() + px(400.), row.center().y),
        None,
        Modifiers::default(),
    );
    settle(cx);
    assert_eq!(
        drawn(cx, &["a", "b", "c", "d"]),
        ["d", "a", "b", "c"],
        "it shows once the pointer leaves"
    );
}
