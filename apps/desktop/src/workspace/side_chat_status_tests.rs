use core::prelude::v1::test;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use nexus_workbench::attention::{Activity, Attention};
use nexus_workbench::event::{LimitStop, ThreadStatus};

use super::*;

fn side(status: ThreadStatus, updated_at: i64) -> SideChatActivity {
    SideChatActivity {
        id: "side".into(),
        activity: Activity {
            status,
            updated_at,
            listed_at: Some(updated_at),
        },
        limited: None,
    }
}

#[test]
fn a_side_chat_off_screen_tells_how_it_stands() {
    assert_eq!(
        state_of(&side(ThreadStatus::Running, 20), Some(10), false),
        Some(Attention::Working)
    );
    assert_eq!(
        state_of(&side(ThreadStatus::Starting, 20), Some(10), false),
        Some(Attention::Working)
    );
    assert_eq!(
        state_of(&side(ThreadStatus::Approval, 20), Some(10), false),
        Some(Attention::Needs)
    );
    assert_eq!(
        state_of(&side(ThreadStatus::Error, 20), Some(10), false),
        Some(Attention::Error)
    );
}

#[test]
fn a_finished_turn_is_unread_until_the_person_sees_it() {
    let finished = side(ThreadStatus::Idle, 20);
    assert_eq!(state_of(&finished, Some(10), false), Some(Attention::Unread));
    assert_eq!(state_of(&finished, Some(20), false), None);
    // Never seen: the first read counts it as seen, so nothing lights up on a fresh install.
    assert_eq!(state_of(&finished, None, false), None);
    assert_eq!(state_of(&side(ThreadStatus::Interrupted, 20), Some(10), false), None);
}

#[test]
fn a_turn_stopped_at_the_usage_limit_says_limited_while_nothing_runs() {
    let mut stopped = side(ThreadStatus::Idle, 20);
    stopped.limited = Some(LimitStop {
        resets_at: None,
        scheduled: false,
    });
    assert_eq!(state_of(&stopped, Some(20), false), Some(Attention::Limited));
}

#[test]
fn the_side_chat_on_screen_shows_no_mark() {
    for status in [
        ThreadStatus::Running,
        ThreadStatus::Approval,
        ThreadStatus::Error,
        ThreadStatus::Idle,
    ] {
        assert_eq!(state_of(&side(status, 20), Some(10), true), None, "{status:?}");
    }
}

#[test]
fn only_what_asks_the_person_to_act_lifts_the_name() {
    assert!(loud(Some(Attention::Needs)));
    assert!(loud(Some(Attention::Error)));
    for quiet in [
        None,
        Some(Attention::Working),
        Some(Attention::Unread),
        Some(Attention::Limited),
    ] {
        assert!(!loud(quiet), "{quiet:?}");
    }
}

#[test]
fn the_tooltip_names_the_entry_then_its_state() {
    assert_eq!(tooltip("Side chat", None), "Side chat");
    assert_eq!(tooltip("Side chat", Some(Attention::Needs)), "Side chat · Needs you");
    assert_eq!(
        tooltip("Side chat", Some(Attention::Unread)),
        "Side chat · Finished, unread"
    );
}

/// A window with the arc, and a count of the times it asked to draw again.
fn arc(cx: &mut gpui_kit::TestAppContext) -> (Rc<Cell<usize>>, &mut gpui_kit::VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, _| TurningMark);
    let asks = Rc::new(Cell::new(0));
    let count = asks.clone();
    cx.update(|_, cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)).detach());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (asks, cx)
}

fn asks_over(asks: &Cell<usize>, span: Duration, cx: &mut gpui_kit::VisualTestContext) -> usize {
    let before = asks.get();
    let step = Duration::from_millis(1);
    let mut elapsed = Duration::ZERO;
    while elapsed < span {
        cx.executor().advance_clock(step);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        elapsed += step;
    }
    asks.get() - before
}

#[gpui_kit::test]
fn the_arc_redraws_at_thirty_frames_a_second(cx: &mut gpui_kit::TestAppContext) {
    let (asks, cx) = arc(cx);
    let frames = asks_over(&asks, Duration::from_secs(1), cx);
    assert!((27..=31).contains(&frames), "{frames} frames in a second");
}

#[gpui_kit::test]
fn under_less_motion_the_arc_stands_still(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (asks, cx) = arc(cx);
    assert_eq!(asks_over(&asks, Duration::from_secs(1), cx), 0);
}
