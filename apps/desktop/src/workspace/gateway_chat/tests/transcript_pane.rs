//! The chat's transcript is a cached pane of its own: a caret blink in the composer leaves it be, and a change to
//! the chat draws it again.
use super::{Probe, ask, complete, delta, fixture, named};
use crate::workspace::View;
use crate::workspace::pane::{Region, drawn};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
fn a_caret_blink_draws_the_composer_and_not_the_transcript(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "Question");
    named(&probe, &a, "a");
    delta(&probe, &a, "An answer");
    complete(&probe, &a, "An answer");
    cx.run_until_parked();
    let composer = workspace.read_with(cx, |workspace, _| {
        assert!(matches!(workspace.view, View::Chat));
        workspace.composer.clone()
    });
    let transcript = drawn::count(Region::ChatTranscript);
    let center = drawn::count(Region::Center);
    assert!(transcript > 0, "the chat drew its transcript pane");

    // A blink: the composer's text box draws again.
    composer.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(
        drawn::count(Region::Center) > center,
        "the main area around the composer draws again"
    );
    assert_eq!(
        drawn::count(Region::ChatTranscript),
        transcript,
        "a blink keeps the transcript"
    );

    // A change to the Workspace draws the transcript again.
    workspace.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let transcript_after_change = drawn::count(Region::ChatTranscript);
    assert!(
        transcript_after_change > transcript,
        "a Workspace change draws the transcript"
    );

    // So does the main area told to draw again on its own, as a streamed answer is.
    let center = workspace.read_with(cx, |workspace, _| workspace.panes.clone().expect("panes").1);
    center.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(
        drawn::count(Region::ChatTranscript) > transcript_after_change,
        "the main area's own redraw reaches its transcript"
    );
}
