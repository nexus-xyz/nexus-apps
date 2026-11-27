//! The composer's Up and Down step (`Workspace::recall_sent`) on an open Code session: it fills the composer from the
//! session's sent messages, puts the caret where the step says, and leaves a typed draft alone.

use super::super::code_view::CodeView;
use super::super::machine_source::MachineSource;
use super::super::{Auth, View, Workspace};
use crate::code::Block;
use core::prelude::v1::test;
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

const SESSION: &str = "prompt-history-test";

fn user(text: &str) -> Block {
    Block::User {
        seq: 0,
        at: 0,
        text: text.into(),
        from: None,
        rewound: false,
        images: 0,
        image_data: Vec::new(),
    }
}

fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace.auth = Auth::SignedIn;
        // No store read: the test sets the transcript itself.
        workspace._code_poll = Task::ready(());
        let mut view = CodeView::new(SESSION.into(), window, cx);
        view.loaded = true;
        view.blocks = vec![user("one"), Block::Text("answer".into()), user("two\nlines")];
        workspace.view = View::Code(view);
        workspace
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// One Up (`down` false) or Down: whether it was a step, and the composer's text, selection and emptiness after it.
fn step(down: bool, workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (bool, String, Range<usize>, bool) {
    workspace.update_in(cx, |workspace, window, cx| {
        let stepped = workspace.recall_sent(down, window, cx);
        let composer = workspace.composer.read(cx);
        (
            stepped,
            composer.value().to_string(),
            composer.selected_range(),
            workspace.composer_empty,
        )
    })
}

#[gpui_kit::test]
fn up_and_down_fill_the_composer_from_the_sent_messages(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    assert_eq!(step(false, &workspace, cx), (true, "two\nlines".into(), 0..0, false));
    assert_eq!(step(false, &workspace, cx), (true, "one".into(), 0..0, false));
    assert!(!step(false, &workspace, cx).0, "nothing is older");
    assert_eq!(step(true, &workspace, cx), (true, "two\nlines".into(), 9..9, false));
    assert_eq!(
        step(true, &workspace, cx),
        (true, String::new(), 0..0, true),
        "past the newest, empty"
    );
    assert!(!step(true, &workspace, cx).0, "the walk is over");
}

#[gpui_kit::test]
fn a_typed_draft_or_no_open_session_has_no_walk(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |state, cx| state.set_value("draft", window, cx));
    });
    assert!(!step(false, &workspace, cx).0);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        workspace.view = View::NewCode;
    });
    assert!(!step(false, &workspace, cx).0);
}
