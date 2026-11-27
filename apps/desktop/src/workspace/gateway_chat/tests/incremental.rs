//! A streamed token applies only the messages it changed; every change still reaches the transcript.
use super::{Probe, ask, chat, complete, delta, fixture, named};
use crate::gateway::Event;
use crate::model::{Meta, Stored};
use crate::workspace::{Entry, Workspace};
use gpui_kit::{Entity, EntityId, TestAppContext, VisualTestContext};

/// Seed four stored messages: two questions, two answers with Gateway's meta.
fn seed(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |workspace, cx| {
        workspace.entries = (1..=4)
            .map(|id| {
                if id % 2 == 1 {
                    Entry::user(id, format!("Question {id}"))
                } else {
                    let mut entry = Entry::answer(id, &format!("Answer {id}"), false, cx);
                    entry.meta = Meta {
                        model: Some("anthropic/claude-sonnet-5.5".into()),
                        cost_usd: Some(0.01),
                        ..Meta::default()
                    };
                    entry
                }
            })
            .collect();
        workspace.next_entry = 4;
    });
}

fn answer_id(workspace: &Workspace, index: usize) -> EntityId {
    workspace.entries[index]
        .answer
        .as_ref()
        .expect("native answer")
        .entity_id()
}

fn rendered(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, index: usize) -> String {
    workspace.read_with(cx, |workspace, cx| {
        let answer = workspace.entries[index].answer.as_ref().expect("native answer");
        answer.read(cx).rendered_text().as_str().to_string()
    })
}

#[gpui_kit::test]
fn a_token_leaves_unchanged_history_alone_and_grows_the_same_answer(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    seed(&workspace, cx);
    let a = ask(&workspace, cx, &probe, "Question 5");
    named(&probe, &a, "a");
    delta(&probe, &a, "one ");
    cx.run_until_parked();
    let (history, streaming) = workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.entries.len(), 6);
        assert_eq!(workspace.entries[5].streamed, "one ");
        // The run still holds 0.01 for this answer: re-applying every message on a token would put it back.
        workspace.entries[1].meta.cost_usd = Some(9.0);
        (answer_id(workspace, 1), answer_id(workspace, 5))
    });
    delta(&probe, &a, "two ");
    delta(&probe, &a, "three");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries.len(), 6);
        assert_eq!(workspace.entries[1].meta.cost_usd, Some(9.0));
        assert_eq!(workspace.entries[1].streamed, "Answer 2");
        assert_eq!(workspace.entries[5].streamed, "one two three");
        assert!(workspace.entries[5].streaming);
        assert_eq!(answer_id(workspace, 1), history);
        assert_eq!(
            answer_id(workspace, 5),
            streaming,
            "a growing answer keeps its native text"
        );
        assert_eq!(workspace.sessions.iter().filter(|session| session.id == "a").count(), 1);
    });
    assert!(rendered(&workspace, cx, 5).contains("one two three"));
}

#[gpui_kit::test]
fn a_changed_message_anywhere_in_the_run_reaches_its_entry(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    seed(&workspace, cx);
    let a = ask(&workspace, cx, &probe, "Question 5");
    named(&probe, &a, "a");
    delta(&probe, &a, "partial");
    cx.run_until_parked();
    probe.reply(
        &a,
        Event::Asked {
            turn: a.turn,
            question: Stored {
                id: Some("question-5".into()),
                ..Stored::default()
            },
        },
    );
    cx.run_until_parked();
    let history = workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries[4].stored.id.as_deref(), Some("question-5"));
        answer_id(workspace, 1)
    });
    // A historical answer replaced in the run, as any later change to an old message would be.
    workspace.update(cx, |workspace, cx| {
        let binding = workspace.chat_execution.clone().expect("execution");
        let key = workspace.chat_execution_at.expect("projected run");
        {
            let mut state = binding.state.borrow_mut();
            let message = &mut state.runs.get_mut(&key).expect("run").entries[1];
            message.streamed = "Edited answer".into();
            message.touch();
        }
        binding.owner.update(cx, |_, cx| cx.notify());
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.entries[1].streamed, "Edited answer");
        assert_eq!(
            answer_id(workspace, 1),
            history,
            "a replaced answer keeps its native text"
        );
        assert_eq!(workspace.entries[5].streamed, "partial");
    });
    assert!(rendered(&workspace, cx, 1).contains("Edited answer"));
}

#[gpui_kit::test]
fn finishing_a_turn_settles_the_answer_and_copy_reads_its_final_text(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    seed(&workspace, cx);
    let a = ask(&workspace, cx, &probe, "Question 5");
    named(&probe, &a, "a");
    delta(&probe, &a, "draft text");
    cx.run_until_parked();
    let streaming = workspace.read_with(cx, |workspace, _| answer_id(workspace, 5));
    // Gateway's final content replaces what streamed.
    complete(&probe, &a, "Final text");
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        let answer = &workspace.entries[5];
        assert_eq!(answer.streamed, "Final text");
        assert!(!answer.streaming);
        assert!(workspace.asking.is_none());
        assert_eq!(answer_id(workspace, 5), streaming);
        let (question, answer) = (workspace.entries[4].id, workspace.entries[5].id);
        workspace.copy_entry(answer, cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some("Final text")
        );
        assert_eq!(workspace.copied, Some(answer));
        workspace.copy_entry(question, cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some("Question 5")
        );
    });
    assert!(rendered(&workspace, cx, 5).contains("Final text"));
}

#[gpui_kit::test]
fn switching_sessions_shows_each_run_and_its_tokens(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    delta(&probe, &a, "a1 ");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| workspace.new_chat(window, cx));
    let b = ask(&workspace, cx, &probe, "B");
    named(&probe, &b, "b");
    delta(&probe, &b, "b1");
    // A streams while B shows.
    delta(&probe, &a, "a2 ");
    cx.run_until_parked();
    let texts = |workspace: &Workspace| {
        workspace
            .entries
            .iter()
            .map(|entry| format!("{}{}", entry.text, entry.streamed))
            .collect::<Vec<_>>()
    };
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.current.as_deref(), Some("b"));
        assert_eq!(texts(workspace), ["B", "b1"]);
    });
    workspace.update_in(cx, |workspace, window, cx| workspace.open(&chat("a"), window, cx));
    delta(&probe, &a, "a3");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.current.as_deref(), Some("a"));
        assert_eq!(texts(workspace), ["A", "a1 a2 a3"]);
    });
    delta(&probe, &b, " b2");
    workspace.update_in(cx, |workspace, window, cx| workspace.open(&chat("b"), window, cx));
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(texts(workspace), ["B", "b1 b2"]);
    });
}

#[gpui_kit::test]
fn queue_changes_reach_the_workspace_while_tokens_stream(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    seed(&workspace, cx);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    delta(&probe, &a, "a1");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.enqueue_chat("B".into(), Default::default(), false, window, cx)
    });
    delta(&probe, &a, " a2");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        let queue = workspace.chat_queue.queues.get("a").expect("queue");
        assert_eq!(queue.items().len(), 1);
        assert_eq!(workspace.entries.last().expect("answer").streamed, "a1 a2");
    });
    // The Owner takes B from the queue when A ends: both the queue and the new messages reach the transcript.
    complete(&probe, &a, "A final");
    cx.run_until_parked();
    let b = probe.asks().last().expect("queued B").clone();
    assert_eq!(b.prompt, "B");
    delta(&probe, &b, "b1");
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(
            workspace
                .chat_queue
                .queues
                .get("a")
                .is_none_or(|queue| queue.items().is_empty())
        );
        let entries = &workspace.entries;
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[5].streamed, "A final");
        assert!(!entries[5].streaming);
        assert_eq!(entries[6].text.as_ref(), "B");
        assert_eq!(entries[7].streamed, "b1");
        assert_eq!(workspace.asking, Some(b.turn));
    });
}
