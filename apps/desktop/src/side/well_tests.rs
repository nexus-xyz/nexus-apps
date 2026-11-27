//! The well's sizes and its drag, fit and maximize, without a window.
use core::prelude::v1::test;

use super::*;

/// Wells in a pane whose foot is at 900px, with well "a" painted at 100px: room for 788px.
fn painted() -> Wells {
    let wells = Wells::default();
    wells.foot.set(Some(900.));
    wells.tops.borrow_mut().insert("a".into(), 100.);
    wells
}

#[test]
fn a_well_is_sized_in_whole_lines_of_its_terminal() {
    // 12 lines of 18px and the grid's 8px above and below.
    assert_eq!(height(DEFAULT_LINES), 232.);
    assert_eq!(lines_in(232.), 12);
    assert_eq!(lines_in(240.), 12);
    assert_eq!(lines_in(40.), MIN_LINES);
    assert_eq!(nearest(240., 40), 12);
    assert_eq!(nearest(244., 40), 13);
    assert_eq!(nearest(2_000., 40), 40);
    assert_eq!(nearest(10., 40), MIN_LINES);
}

#[test]
fn a_drag_follows_the_pointer_and_lets_go_on_the_nearest_line() {
    let mut wells = painted();
    let key: SharedString = "a".into();
    assert_eq!(wells.target("a"), height(12));
    wells.begin(key.clone(), 500.);
    // 47px down: the well follows the pointer, between lines.
    assert!(wells.drag_to("a", 547.));
    assert_eq!(wells.drag.as_ref().unwrap().now, height(12) + 47.);
    // Another well's drag is not this one's.
    assert!(!wells.drag_to("b", 600.));
    assert!(wells.release());
    // 47px is 2.6 lines: it rests on 15, and eases there from where it was let go, as quick as an exit.
    assert_eq!(wells.lines("a"), 15);
    assert_eq!(wells.course("a"), (height(15), SETTLE));
    assert!(!wells.release(), "a second mouse up ends nothing");
}

#[test]
fn a_drag_stops_at_six_lines_and_at_the_pane() {
    let mut wells = painted();
    wells.begin("a".into(), 500.);
    wells.drag_to("a", -2_000.);
    assert_eq!(wells.drag.as_ref().unwrap().now, height(MIN_LINES));
    wells.drag_to("a", 5_000.);
    // 788px of room holds 42 lines.
    assert_eq!(wells.drag.as_ref().unwrap().now, height(42));
    wells.release();
    assert_eq!(wells.lines("a"), 42);
}

#[test]
fn a_double_click_on_the_grip_fits_the_pane_and_then_goes_back() {
    let mut wells = painted();
    let key: SharedString = "a".into();
    wells.fit(&key);
    assert_eq!(wells.lines("a"), 42);
    assert_eq!(wells.course("a").1, PANEL);
    wells.fit(&key);
    assert_eq!(wells.lines("a"), DEFAULT_LINES);
    // Before the pane was painted, nothing is known to fit: it stays at 12.
    let mut unpainted = Wells::default();
    unpainted.fit(&key);
    assert_eq!(unpainted.lines("a"), DEFAULT_LINES);
}

#[test]
fn maximize_gives_the_well_the_pane_and_restore_gives_it_back() {
    let mut wells = painted();
    let key: SharedString = "a".into();
    assert_eq!(wells.restores(), None);
    wells.toggle_maximized(&key);
    assert!(wells.is_maximized("a"));
    assert_eq!(wells.target("a"), 788.);
    wells.toggle_maximized(&key);
    assert!(!wells.is_maximized("a"));
    assert_eq!(wells.target("a"), height(DEFAULT_LINES));
    assert_eq!(wells.restores(), Some(1));
    assert!(!wells.restore(), "nothing was maximized");
}

#[test]
fn only_the_well_under_the_pointer_shows_its_grip() {
    let mut wells = Wells::default();
    let (a, b): (SharedString, SharedString) = ("a".into(), "b".into());
    assert!(wells.hover(&a, true));
    assert!(!wells.hover(&a, true));
    // Leaving a well that is not the hovered one changes nothing.
    assert!(!wells.hover(&b, false));
    assert!(wells.hover(&b, true));
    assert!(wells.hover(&b, false));
    assert_eq!(wells.hovered, None);
}
