use core::prelude::v1::test;
use std::time::Instant;

use super::*;

fn pane() -> Pane {
    let (mut pane, _) = Pane::new(None);
    pane.surface = Some(SURFACE);
    pane
}

#[test]
fn the_bar_asks_from_the_desktop_surface_in_the_threads_conversation() {
    let mut pane = pane();
    pane.conversation = Some("c1".into());
    let call = pane.ask("Summarise this", Instant::now()).expect("asks");
    let crate::agent::Call::Ask { body } = call else {
        panic!("an ask");
    };
    let body: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(body["surface"], "desktop");
    assert_eq!(body["conversation"], "c1");
    assert_eq!(body["message"], "Summarise this");
    assert_eq!(body["stream"], true);
}

#[test]
fn the_body_follows_the_ask_from_the_step_line_to_the_failure() {
    let mut pane = pane();
    assert_eq!(body_of(&pane), Body::Empty);
    pane.ask("hi", Instant::now());
    assert_eq!(body_of(&pane), Body::Thinking("Thinking".into()));
    pane.draft.as_mut().unwrap().step = Some("Searching Linear".into());
    assert_eq!(body_of(&pane), Body::Thinking("Searching Linear".into()));
    pane.draft.as_mut().unwrap().text = "The rig".into();
    assert_eq!(body_of(&pane), Body::Writing);
    pane.draft = None;
    pane.problem = Some(nexus_desktop_core::agent::problem(502, None, None));
    assert!(matches!(body_of(&pane), Body::Failed { .. }));
}

#[test]
fn the_window_grows_with_the_answer_up_to_its_limit() {
    let answered = Body::Answered { needs_you: false };
    let short = height(&answered, "Yes.");
    let long = height(&answered, &"word ".repeat(100));
    let huge = height(&answered, &"word ".repeat(5000));
    assert!(height(&Body::Empty, "") < short);
    assert!(short < long);
    assert!(long < huge);
    assert_eq!(
        huge,
        height(&answered, &"x\n".repeat(100)),
        "both stop at the scroll limit"
    );
}

#[test]
fn lines_count_wrapping_and_breaks() {
    assert_eq!(lines("short"), 1);
    assert_eq!(lines(&"a".repeat(67)), 2);
    assert_eq!(lines("one\n\nthree"), 3);
}

#[test]
fn the_bar_is_a_mac_feature() {
    if !cfg!(target_os = "macos") {
        assert!(!enabled());
    }
}
