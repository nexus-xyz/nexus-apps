//! The thread's live step line, quick replies and link cards (brand v5 A1), on the production Workspace
//! with a fixture backend that answers the agent API from the test. The window draws after each answer, so a panic in
//! a render fails the test.
use super::super::navigation::test_backend::BackendProbe;
use super::super::{Auth, View, Workspace, machine_source::MachineSource};
use crate::agent::{Call, Chunk};
use crate::gateway::Account;
use crate::pages::agent::AgentPage;
use gpui_kit::{Context, Entity, Task, TestAppContext, VisualTestContext, Window};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn make(window: &mut Window, cx: &mut Context<Workspace>, backend: &BackendProbe) -> Workspace {
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let mut workspace = Workspace::with_machine_source(
        Arc::new(backend.clone()),
        events,
        arrivals,
        false,
        MachineSource::isolated(),
        window,
        cx,
    );
    workspace._code_poll = Task::ready(());
    workspace.auth = Auth::SignedIn;
    workspace.code_scope = Some(format!("{}|org-a", backend.url));
    workspace.side.open = false;
    workspace.navigation_place_sink = Some(Rc::new(RefCell::new(vec![])));
    let account = Account::from_json(&json!({"tenant":{"id":"org-a","slug":"org-a","name":"Acme","kind":"company"},"user":{"id":"person-a","personId":"person-a","name":"A","email":"a@example.test"},"role":"member","permissions":[]})).expect("account wire fixture");
    workspace.accept_navigation_identity(&account, cx);
    workspace.account = Some(account);
    workspace.agent = true;
    workspace
}

fn fixture<'a>(cx: &'a mut TestAppContext, backend: &BackendProbe) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| make(window, cx, backend))
}

fn sse(event: &str, data: &str) -> Chunk {
    Chunk::Data(format!("event: {event}\ndata: {data}\n\n").into_bytes())
}

fn head(status: u16, content_type: &str) -> Chunk {
    Chunk::Head {
        status,
        content_type: content_type.into(),
    }
}

/// The sender of the last call that `matches`.
fn sender(backend: &BackendProbe, matches: impl Fn(&Call) -> bool) -> futures::channel::mpsc::UnboundedSender<Chunk> {
    let calls = backend.agent_calls.lock().expect("agent calls");
    let (_, sender) = calls
        .iter()
        .rev()
        .find(|(call, _)| matches(call))
        .expect("the call was made");
    sender.clone()
}

fn send(sender: &futures::channel::mpsc::UnboundedSender<Chunk>, chunks: Vec<Chunk>, cx: &mut VisualTestContext) {
    for chunk in chunks {
        sender.unbounded_send(chunk).expect("the page reads its answer");
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_ask(call: &Call) -> bool {
    matches!(call, Call::Ask { .. })
}

fn last_ask(backend: &BackendProbe) -> String {
    let calls = backend.agent_calls.lock().expect("agent calls");
    calls
        .iter()
        .rev()
        .find_map(|(call, _)| match call {
            Call::Ask { body } => Some(body.clone()),
            _ => None,
        })
        .expect("an ask")
}

fn errand(status: &str, extra: Value) -> Value {
    let mut row =
        json!({ "id": "i7", "goal": "Book the dentist", "status": status, "steps": 2, "at": "2026-11-18T09:00:00Z" });
    if let (Value::Object(row), Value::Object(extra)) = (&mut row, extra) {
        row.extend(extra);
    }
    row
}

fn open(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<AgentPage> {
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant_kept_page().expect("kept")
    });
    cx.run_until_parked();
    page
}

/// A1·C b and A1·D: while Nexus thinks the thread shows "Thinking", then each step line as the stream sends it. The
/// answer that asks has its quick replies under it, and a tap sends one as the person's next message; the replies
/// go while it is answered.
#[gpui_kit::test]
fn the_step_line_follows_the_stream_and_a_quick_reply_sends(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("plan my day".into(), window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        page.read_with(cx, |page, cx| page.step_line(cx)).as_deref(),
        Some("Thinking")
    );

    let ask = sender(&backend, is_ask);
    send(
        &ask,
        vec![
            head(200, "text/event-stream"),
            sse("started", r#"{"conversation":"c1"}"#),
            sse("model", r#"{"model":"nexus/auto"}"#),
            sse("step", r#"{"line":"Reading your calendar"}"#),
        ],
        cx,
    );
    assert_eq!(
        page.read_with(cx, |page, cx| page.step_line(cx)).as_deref(),
        Some("Reading your calendar")
    );
    // A line that is not one plain line is read as nothing: the last good line stays.
    send(&ask, vec![sse("step", r#"{"line":"two\nlines"}"#)], cx);
    send(&ask, vec![sse("step", r#"{"line":"Searching Linear"}"#)], cx);
    assert_eq!(
        page.read_with(cx, |page, cx| page.step_line(cx)).as_deref(),
        Some("Searching Linear")
    );
    // Under Reduce motion the line still draws, still.
    cx.update(|window, cx| {
        cx.set_reduce_motion(true);
        window.draw(cx).clear(cx);
        cx.set_reduce_motion(false);
    });

    send(
        &ask,
        vec![
            sse("text", r#"{"delta":"Two meetings. Tell Ana you are late?"}"#),
            sse(
                "done",
                r#"{"status":200,"conversation":"c1","messages":[{"kind":"answer","text":"Two meetings. Tell Ana you are late?","suggestions":["Tell them","Not now","tell them","Ask me later\nplease"]}]}"#,
            ),
            Chunk::End,
        ],
        cx,
    );
    assert_eq!(page.read_with(cx, |page, cx| page.step_line(cx)), None, "answered");
    assert_eq!(
        page.read_with(cx, |page, _| page.quick_replies()),
        vec!["Tell them".to_string(), "Not now".to_string()]
    );

    // A tap sends the reply's words, and the replies go while it is answered.
    page.update_in(cx, |page, window, cx| page.start_with("Tell them", window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        last_ask(&backend),
        r#"{"message":"Tell them","stream":true,"conversation":"c1"}"#
    );
    assert!(page.read_with(cx, |page, _| page.quick_replies()).is_empty());
}

/// A1·5: the "I opened an errand" notice draws its card with its state now, by shape: a running one turns, and once
/// the errands list says it failed, its card says why and stops turning. A click opens its thread. A session card
/// draws under its answer, and a desktop one opens a New session draft from its `nexus://new?` link.
#[gpui_kit::test]
fn the_errand_and_session_cards_show_their_state_and_open(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("book the dentist and fix the decoder".into(), window, cx)
    });
    cx.run_until_parked();
    let notice = json!({ "kind": "notice", "topic": "errands", "state": "opened", "text": "I opened an errand.", "errand": errand("running", json!({})) });
    let answer = json!({ "kind": "answer", "text": "It's a bigger job, so I set up a session.", "sessions": [
        { "where": "desktop", "state": "ready", "title": "Fix the decoder", "url": "nexus://new?prompt=fix%20the%20decoder", "repo": "acme/sim", "branch": "decoder-gru" },
        { "where": "synced", "state": "running", "title": "Codex session", "url": "https://nexus.test/app/org-a/sessions/t1" },
        { "where": "synced", "state": "done", "title": "Unsafe", "url": "javascript:alert(1)" }
    ]});
    let done = json!({ "status": 200, "conversation": "c1", "messages": [notice, answer] }).to_string();
    send(
        &sender(&backend, is_ask),
        vec![head(200, "text/event-stream"), sse("done", &done), Chunk::End],
        cx,
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.link_cards()),
        vec![
            "errand Book the dentist · Running · 2 steps so far".to_string(),
            "session Fix the decoder · Ready to open · on decoder-gru".to_string(),
            "session Codex session · Running".to_string(),
        ]
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.turning()),
        2,
        "the running errand and session"
    );

    // The answer ended, so the window reads the errands again: the errand failed since, with the agent's reason.
    let list =
        json!({ "errands": [], "history": [errand("failed", json!({ "detail": "The clinic's site was down" }))] });
    send(
        &sender(&backend, |call| matches!(call, Call::Errands)),
        vec![
            head(200, "application/json"),
            Chunk::Data(list.to_string().into_bytes()),
            Chunk::End,
        ],
        cx,
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.link_cards())[0],
        "errand Book the dentist · Failed · The clinic's site was down"
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.turning()),
        1,
        "only the session turns"
    );

    // The desktop session card fills a New session draft; nothing is sent.
    let asks = backend.agent_calls.lock().expect("agent calls").len();
    page.update_in(cx, |page, window, cx| {
        page.open_draft("nexus://new?prompt=fix%20the%20decoder", window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(workspace.read_with(cx, |workspace, _| matches!(workspace.view, View::NewCode)));
    assert_eq!(backend.agent_calls.lock().expect("agent calls").len(), asks);
}

/// A1 and A24·c: the composer's Auto chip shows the saved model and saves a pick, `@` lists the person's context and a
/// pick becomes a chip the ask carries, `/` picks a skill, and Enter without a menu still sends.
#[gpui_kit::test]
fn the_composer_has_the_chip_the_at_menu_and_the_skills(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, cx);
    let model = json!({ "model": "auto", "models": [{ "id": "anthropic/claude-haiku-4-5", "name": "Haiku 4.5", "maker": "anthropic" }] });
    let answer = |call: fn(&Call) -> bool, status: u16, body: Value, cx: &mut VisualTestContext| {
        let chunks = vec![
            head(status, "application/json"),
            Chunk::Data(body.to_string().into_bytes()),
            Chunk::End,
        ];
        send(&sender(&backend, call), chunks, cx);
    };
    answer(|call| matches!(call, Call::Model), 200, model, cx);
    assert_eq!(page.read_with(cx, |page, _| page.chip_label()).as_deref(), Some("Auto"));

    // A pick is saved and shown.
    page.update_in(cx, |page, window, cx| page.pick_model("fast", window, cx));
    cx.run_until_parked();
    answer(
        |call| matches!(call, Call::SaveModel { body } if body == r#"{"model":"fast"}"#),
        200,
        json!({ "model": "fast", "models": [], "saved": true }),
        cx,
    );
    assert_eq!(page.read_with(cx, |page, _| page.chip_label()).as_deref(), Some("Fast"));

    // `@` reads what it may pick, filters as the person types, and Enter makes a chip.
    page.update_in(cx, |page, window, cx| page.type_text("plan @", window, cx));
    cx.run_until_parked();
    answer(
        |call| matches!(call, Call::Context),
        200,
        json!({ "items": [
            { "kind": "connector", "id": "gmail", "label": "Gmail" },
            { "kind": "connector", "id": "linear", "label": "Linear" },
            { "kind": "errand", "id": "i7", "label": "Book the dentist" }
        ] }),
        cx,
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.menu_labels()),
        vec!["Gmail", "Linear", "Book the dentist"]
    );
    assert!(page.update_in(cx, |page, window, cx| page.press("down", window, cx)));
    assert!(page.update_in(cx, |page, window, cx| page.press("enter", window, cx)));
    assert_eq!(page.read_with(cx, |page, _| page.chips()), vec!["@Linear"]);
    assert_eq!(page.read_with(cx, |page, cx| page.typed(cx)), "plan ");

    // Esc closes a menu; with none open, the keys are the text field's.
    page.update_in(cx, |page, window, cx| page.type_text("plan @bo", window, cx));
    cx.run_until_parked();
    assert_eq!(
        page.read_with(cx, |page, _| page.menu_labels()),
        vec!["Book the dentist"]
    );
    assert!(page.update_in(cx, |page, window, cx| page.press("escape", window, cx)));
    assert!(page.read_with(cx, |page, _| page.menu_labels()).is_empty());
    assert!(!page.update_in(cx, |page, window, cx| page.press("enter", window, cx)));

    // Enter sends the words with the pick as context.
    page.update_in(cx, |page, window, cx| {
        page.type_text("plan dinner", window, cx);
        page.submit(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        last_ask(&backend),
        r#"{"message":"plan dinner","stream":true,"context":[{"kind":"connector","id":"linear"}]}"#
    );
    assert!(
        page.read_with(cx, |page, _| page.chips()).is_empty(),
        "the chips go with the words"
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A24·c: `/` opens only at the start, a skill is a chip, and a skill that needs words does not send without them.
#[gpui_kit::test]
fn a_skill_is_a_chip_and_needs_its_words(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, cx);
    page.update_in(cx, |page, window, cx| page.type_text("see /br", window, cx));
    assert!(page.read_with(cx, |page, _| page.menu_labels()).is_empty());
    page.update_in(cx, |page, window, cx| page.type_text("/we", window, cx));
    assert_eq!(page.read_with(cx, |page, _| page.menu_labels()), vec!["/web"]);
    assert!(page.update_in(cx, |page, window, cx| page.press("tab", window, cx)));
    assert_eq!(page.read_with(cx, |page, _| page.chips()), vec!["/web"]);
    // No words: nothing is sent.
    let before = backend.agent_calls.lock().expect("agent calls").len();
    page.update_in(cx, |page, window, cx| page.submit(window, cx));
    cx.run_until_parked();
    assert_eq!(backend.agent_calls.lock().expect("agent calls").len(), before);
    page.update_in(cx, |page, window, cx| {
        page.type_text("cheap flights", window, cx);
        page.submit(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        last_ask(&backend),
        r#"{"message":"cheap flights","stream":true,"skill":"web"}"#
    );
}
