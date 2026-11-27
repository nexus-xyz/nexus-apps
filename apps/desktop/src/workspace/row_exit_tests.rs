use super::*;
use crate::workspace::sidebar::ROW;
use core::prelude::v1::test;

#[test]
fn a_deleted_row_starts_to_leave_on_the_click() {
    let frame = leaving(Duration::ZERO, false);
    assert_eq!(frame.height, 1.);
    assert!(frame.content > 0.99);
    assert_eq!(frame.tint, 1.);
    assert!(frame.moving && frame.leaving);
}

#[test]
fn the_content_fades_first_then_the_slot_folds() {
    // At the end of the fade the content is gone and the slot still holds its place.
    let faded = leaving(LEAVE, false);
    assert!(faded.content < 0.01);
    assert!((faded.shift + SLIDE).abs() < 0.01);
    assert_eq!(faded.height, 1.);
    // Half way through the fold the slot is part closed.
    let folding = leaving(LEAVE + FOLD / 2, false);
    assert!(folding.height > 0. && folding.height < 1.);
    // Then it rests at no height, still leaving, and asks for no more frames.
    let folded = leaving(LEAVE + FOLD, false);
    assert!(folded.height.abs() < 0.001);
    assert!(!folded.moving && folded.leaving);
}

#[test]
fn less_motion_folds_the_row_at_once_and_asks_for_no_frame() {
    let frame = leaving(Duration::ZERO, true);
    assert_eq!(frame.height, 0.);
    assert!(!frame.moving);
    let back = returning(Duration::ZERO, 0., true);
    assert_eq!(back.height, 1.);
    assert!(!back.moving);
}

#[test]
fn a_failed_delete_brings_the_row_back_from_where_it_was() {
    let start = Instant::now();
    let mut exits = RowExits::default();
    exits.start("a", start);
    let failed = start + LEAVE + FOLD / 2;
    let was = exits.frame("a", failed, false).unwrap().height;
    exits.bring_back("a", failed, false);
    let now = exits.frame("a", failed, false).unwrap();
    assert!((now.height - was).abs() < 0.001, "no jump when it turns back");
    assert!(!now.leaving);
    assert_eq!(now.ring, 1.);
    let open = exits.frame("a", failed + FOLD, false).unwrap();
    assert!((open.height - 1.).abs() < 0.001);
    // Once the ring has faded, the row is drawn as usual again.
    assert_eq!(exits.frame("a", failed + RING, false), None);
}

#[test]
fn the_group_height_follows_the_folding_rows() {
    let start = Instant::now();
    let mut exits = RowExits::default();
    let ids = [("a", ROW), ("b", ROW + 16.), ("c", ROW)];
    assert_eq!(exits.height(ids, start, false), 3. * ROW + 16.);
    exits.start("b", start);
    assert_eq!(exits.height(ids, start, false), 3. * ROW + 16.);
    let folded = start + LEAVE + FOLD;
    assert!((exits.height(ids, folded, false) - 2. * ROW).abs() < 0.01);
}

#[test]
fn the_refresh_forgets_rows_that_left_the_list_and_rows_that_came_back() {
    let start = Instant::now();
    let mut exits = RowExits::default();
    exits.start("gone", start);
    exits.start("slow", start);
    exits.start("failed", start);
    exits.bring_back("failed", start, false);
    let later = start + Duration::from_millis(2_000);
    // "gone" left the list; "slow" is still listed while its delete waits; "failed" came back and rests.
    exits.forget(|id| id != "gone", later, false);
    assert_eq!(exits.frame("gone", later, false), None);
    assert!(exits.frame("slow", later, false).is_some_and(|frame| frame.leaving));
    assert!(!exits.rows.contains_key("failed"));
}

#[test]
fn the_notice_names_the_session() {
    assert_eq!(deleted_text("Fix the build"), "Deleted \u{201c}Fix the build\u{201d}");
    assert_eq!(
        delete_failed_text("Fix the build", "busy"),
        "Couldn\u{2019}t delete \u{201c}Fix the build\u{201d}: busy"
    );
}
