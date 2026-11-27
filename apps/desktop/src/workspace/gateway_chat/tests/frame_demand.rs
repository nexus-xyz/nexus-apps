//! Passive frame callbacks never create animation demand.
use gpui_kit::{Context, IntoElement, Render, RequestFrameOptions, TestAppContext, Window, div};
use std::{cell::Cell, rc::Rc};

struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui_kit::test]
fn passive_callbacks_run_once_on_existing_draws_and_rearm_without_waking_idle_windows(cx: &mut TestAppContext) {
    cx.defer_draws_to_platform();
    let window = cx.add_window(|_, _| Empty);
    let native = cx.test_window(window.into());
    window.update(cx, |_, window, _| window.activate_window()).unwrap();
    cx.run_until_parked();
    for _ in 0..4 {
        native.simulate_scheduled_frame();
    }
    assert!(!native.frame_scheduled());
    let wakes = native.frame_wake_count();
    let calls = Rc::new(Cell::new(0));
    window
        .update(cx, {
            let calls = calls.clone();
            move |_, window, _| {
                window.before_next_native_draw(move |window, _| {
                    calls.set(calls.get() + 1);
                    window.before_next_native_draw(move |_, _| calls.set(calls.get() + 1));
                });
            }
        })
        .unwrap();
    assert!(!native.frame_scheduled(), "queuing passive work requests no frame");
    assert_eq!(native.frame_wake_count(), wakes);
    native.simulate_frame_request(RequestFrameOptions::default());
    assert_eq!(calls.get(), 0, "a clean native tick is not a draw");
    assert!(!native.frame_scheduled());
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    assert!(native.simulate_scheduled_frame());
    assert_eq!(calls.get(), 1, "the first callback runs on the requested draw");
    for _ in 0..4 {
        native.simulate_scheduled_frame();
    }
    assert_eq!(calls.get(), 1, "re-arming did not force another draw");
    assert!(
        !native.frame_scheduled(),
        "the re-armed callback leaves the loop parked"
    );
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    assert!(native.simulate_scheduled_frame());
    assert_eq!(calls.get(), 2, "re-armed work runs once when another draw is requested");
}
