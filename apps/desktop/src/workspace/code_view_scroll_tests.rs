//! The transcript stays where the reader is while blocks above them change: a sub-agent left running in the background
//! keeps adding steps under its row, above the answer the session wrote since.

use super::*;
use crate::workspace::machine_source::MachineSource;
use core::prelude::v1::test;
use nexus_workbench::event::{TaskStatus, WbEvent};
use serde_json::json;
use std::sync::Arc;

const SESSION: &str = "scroll-test";

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
        workspace.auth = crate::workspace::Auth::SignedIn;
        // No store read: the test sets the transcript itself.
        workspace._code_poll = Task::ready(());
        let mut view = CodeView::new(SESSION.into(), window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace.side.fixture_session(SESSION);
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

fn view(workspace: &mut Workspace) -> &mut CodeView {
    match &mut workspace.view {
        View::Code(view) => view,
        _ => unreachable!("the fixture opens a Code session"),
    }
}

/// A session whose agent started a background review, then wrote twelve answers while the review took `steps` steps.
fn session(steps: usize) -> Transcript {
    session_state(steps, false)
}

fn session_state(steps: usize, finished: bool) -> Transcript {
    let mut events = vec![
        WbEvent::User {
            text: "Review the diff, then go on".into(),
            images: Vec::new(),
            from: None,
        },
        WbEvent::TurnStart { turn_id: "turn".into() },
        WbEvent::Tool {
            id: "review".into(),
            name: "Agent".into(),
            input: json!({"description": "Review the diff", "prompt": "Review it", "run_in_background": true}),
            parent: None,
        },
        WbEvent::ToolResult {
            id: "review".into(),
            output: "Async agent launched".into(),
            images: Vec::new(),
            is_error: false,
            truncated: false,
            parent: None,
        },
    ];
    for answer in 0..12 {
        events.push(WbEvent::Tool {
            id: format!("read-{answer}"),
            name: "Read".into(),
            input: json!({"file_path": format!("src/part_{answer}.rs")}),
            parent: None,
        });
        events.push(WbEvent::Text {
            id: format!("answer-{answer}"),
            text: format!(
                "Answer {answer}.\n\n{}",
                "A paragraph the reader is reading. ".repeat(40)
            ),
            parent: None,
        });
    }
    for step in 0..steps {
        events.push(WbEvent::Tool {
            id: format!("review-step-{step}"),
            name: "Bash".into(),
            input: json!({"command": format!("cargo test --step {step}")}),
            parent: Some("review".into()),
        });
    }
    if finished {
        events.push(WbEvent::Task {
            task_id: "review-task".into(),
            tool_use_id: Some("review".into()),
            task_type: Some("local_agent".into()),
            agent_type: None,
            status: TaskStatus::Completed,
            description: "Review finished".into(),
            tool_count: Some(steps as u64),
            total_tokens: None,
            duration_ms: Some(1000),
            detail: None,
        });
    }
    Transcript::of(
        events
            .into_iter()
            .enumerate()
            .map(|(ix, event)| (ix as i64 + 1, ix as i64 + 1, event))
            .collect(),
    )
}

/// Take `transcript` as the store poll does, and draw the window.
fn show(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, transcript: Transcript) {
    workspace.update(cx, |workspace, cx| {
        view(workspace).set(transcript, cx);
        cx.notify();
    });
    draw(cx);
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

fn answer_at(view: &CodeView, ix: usize) -> Option<String> {
    match view.blocks.get(ix) {
        Some(Block::Text(text)) => text.lines().next().map(str::to_string),
        _ => None,
    }
}

#[gpui_kit::test]
fn background_sub_agent_steps_leave_the_reader_where_they_are(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    show(&workspace, cx, session(1));
    workspace.update(cx, |workspace, _| view(workspace).toggle_agent("review"));
    draw(cx);
    // The reader scrolls up into answer 6, below the review's row.
    let (row, reading) = workspace.update(cx, |workspace, cx| {
        cx.notify();
        let view = view(workspace);
        let row = view.agents["review"].row;
        let reading = (0..view.blocks.len())
            .find(|ix| answer_at(view, *ix).as_deref() == Some("Answer 6."))
            .expect("answer 6 is in the transcript");
        assert!(row < reading, "the review's row is above the answers it ran beside");
        view.list.scroll_to(ListOffset {
            item_ix: reading,
            offset_in_item: px(40.),
        });
        (row, reading)
    });
    draw(cx);
    let before = workspace.update(cx, |workspace, _| view(workspace).list.logical_scroll_top());
    assert_eq!(before.item_ix, reading);
    assert!(before.offset_in_item > px(0.));

    for steps in 2..=4 {
        show(&workspace, cx, session(steps));
        let (top, answer, still_row) = workspace.update(cx, |workspace, _| {
            let view = view(workspace);
            let top = view.list.logical_scroll_top();
            (top, answer_at(view, top.item_ix), view.agents["review"].row)
        });
        assert_eq!(still_row, row, "the review's row does not move");
        assert_eq!(
            answer.as_deref(),
            Some("Answer 6."),
            "after step {steps} the reader is still on answer 6, not at {top:?}"
        );
        assert_eq!(top.offset_in_item, before.offset_in_item, "and at the same line of it");
    }
}

#[gpui_kit::test]
fn running_and_completed_sub_agents_start_collapsed(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    for finished in [false, true] {
        show(&workspace, cx, session_state(2, finished));
        workspace.update(cx, |workspace, _| {
            let view = view(workspace);
            assert_eq!(view.agents["review"].running, !finished);
            assert!(!view.agent_open("review"));
            let body: Vec<_> = view.blocks.iter().filter(|block| !block.path().is_empty()).collect();
            assert!(!body.is_empty(), "the fixture includes the sub-agent's steps");
            assert!(body.iter().all(|block| !view.shows(block)));
        });
    }
}

#[gpui_kit::test]
fn explicit_agent_choices_survive_streaming_refreshes_and_completion(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    show(&workspace, cx, session(1));
    workspace.update(cx, |workspace, _| view(workspace).toggle_agent("review"));
    for finished in [false, true] {
        show(&workspace, cx, session_state(3, finished));
        workspace.update(cx, |workspace, _| {
            let view = view(workspace);
            view.stream_agents(vec![("review".into(), "Checking another file".into())]);
            assert!(view.agent_open("review"));
            assert!(
                view.blocks
                    .iter()
                    .filter(|block| !block.path().is_empty())
                    .all(|block| view.shows(block))
            );
        });
    }
    workspace.update(cx, |workspace, _| view(workspace).toggle_agent("review"));
    show(&workspace, cx, session_state(4, true));
    workspace.update(cx, |workspace, _| assert!(!view(workspace).agent_open("review")));
}

#[gpui_kit::test]
fn nested_agent_rows_open_independently_and_a_closed_parent_hides_the_subtree(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture(cx);
    let events = vec![
        json!({"kind":"tool", "id":"outer", "name":"Agent", "input":{"description":"Outer"}, "parent":null}),
        json!({"kind":"tool", "id":"inner", "name":"Agent", "input":{"description":"Inner"}, "parent":"outer"}),
        json!({"kind":"text", "id":"inner-answer", "text":"Nested answer", "parent":"inner"}),
    ];
    show(
        &workspace,
        cx,
        Transcript::of(
            events
                .into_iter()
                .enumerate()
                .map(|(ix, event)| (ix as i64 + 1, ix as i64 + 1, serde_json::from_value(event).unwrap()))
                .collect(),
        ),
    );
    workspace.update(cx, |workspace, _| {
        let view = view(workspace);
        assert!(!view.agent_open("outer") && !view.agent_open("inner"));
        view.toggle_agent("outer");
        let child = &view.blocks[view.agents["inner"].row];
        assert!(
            view.shows(child),
            "opening the parent exposes the child's collapsed row"
        );
        let answer = view.blocks.iter().position(|block| block.path().len() == 2).unwrap();
        assert!(!view.shows(&view.blocks[answer]));
        view.toggle_agent("inner");
        assert!(view.shows(&view.blocks[answer]));
        view.toggle_agent("outer");
        assert!(!view.shows(&view.blocks[answer]));
        assert!(
            view.agent_open("inner"),
            "closing the parent preserves the child's choice"
        );
    });
}
