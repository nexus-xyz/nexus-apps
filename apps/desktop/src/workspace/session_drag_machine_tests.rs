use core::prelude::v1::test;

use super::*;
use crate::code::Aside;
use crate::workspace::session_drop::{Place, Put};

fn drag() -> Facts {
    Facts {
        id: "a".into(),
        project: "repo".into(),
        from: Place::Project("repo".into()),
        pinned: false,
        linked: false,
        with: Vec::new(),
    }
}

fn project() -> Target {
    Target::Project {
        lead: "repo".into(),
        members: ["repo".to_string()].into_iter().collect(),
    }
}

fn settled() -> Target {
    Target::Group(Place::Aside(Aside::Settled))
}

fn enter(key: &str, target: Target) -> Event {
    Event::Move {
        key: key.into(),
        inside: true,
        target,
    }
}

fn leave(key: &str, target: Target) -> Event {
    Event::Move {
        key: key.into(),
        inside: false,
        target,
    }
}

#[test]
fn a_drag_lifts_reaches_a_target_and_lands_on_it() {
    let mut phase = Phase::default();
    assert_eq!(phase.step(enter("settled", settled())), Step::Same, "no drag yet");
    assert_eq!(phase.step(Event::Lift(drag())), Step::Changed);
    assert_eq!(phase.drag().map(|drag| drag.id.as_str()), Some("a"));
    assert_eq!(phase.step(enter("settled", settled())), Step::Changed);
    assert_eq!(
        phase.step(enter("settled", settled())),
        Step::Same,
        "the same target: no redraw"
    );
    match phase.step(Event::Drop { key: "settled".into() }) {
        Step::Land(landing) => {
            assert_eq!(landing.drag, drag());
            assert_eq!(landing.target, settled());
            assert_eq!(landing.plan.put, Put::Settle);
        }
        step => panic!("it should land, not {step:?}"),
    }
    assert_eq!(phase, Phase::Idle);
}

#[test]
fn a_target_that_would_not_take_it_never_lights() {
    let mut phase = Phase::default();
    phase.step(Event::Lift(drag()));
    assert_eq!(
        phase.step(enter("head", project())),
        Step::Same,
        "it is in this project already"
    );
    assert_eq!(phase.over(), None);
    assert_eq!(phase.step(Event::Drop { key: "head".into() }), Step::Same);
    assert!(phase.drag().is_some(), "GPUI ends that drag; nothing lands");
}

#[test]
fn leaving_one_element_never_clears_the_target_of_another() {
    let mut phase = Phase::default();
    phase.step(Event::Lift(drag()));
    phase.step(enter("settled-head", settled()));
    // The pointer moves to the group's rows: the rows' element hears it first, then the head hears it left.
    assert_eq!(phase.step(enter("settled-rows", settled())), Step::Changed);
    assert_eq!(phase.step(leave("settled-head", settled())), Step::Same);
    assert_eq!(phase.over().map(|(key, _)| key), Some("settled-rows"));
    assert_eq!(phase.step(leave("settled-rows", settled())), Step::Changed);
    assert_eq!(phase.over(), None);
    assert!(phase.drag().is_some(), "still lifted");
}

#[test]
fn only_the_element_it_is_over_takes_the_drop() {
    let mut phase = Phase::default();
    phase.step(Event::Lift(drag()));
    phase.step(enter("settled", settled()));
    assert_eq!(
        phase.step(Event::Drop {
            key: "elsewhere".into()
        }),
        Step::Same
    );
    assert!(matches!(phase, Phase::Over { .. }));
}

#[test]
fn a_cancel_drops_nothing_and_a_new_lift_starts_over() {
    let mut phase = Phase::default();
    phase.step(Event::Lift(drag()));
    phase.step(enter("settled", settled()));
    assert_eq!(phase.step(Event::Cancel), Step::Changed);
    assert_eq!(phase, Phase::Idle);
    assert_eq!(phase.step(Event::Cancel), Step::Same);
    phase.step(Event::Lift(drag()));
    phase.step(enter("settled", settled()));
    let other = Facts {
        id: "b".into(),
        ..drag()
    };
    phase.step(Event::Lift(other));
    assert_eq!(phase.over(), None, "a new drag starts over no target");
    assert_eq!(phase.drag().map(|drag| drag.id.as_str()), Some("b"));
}
