//! The Assistant in the window: the kept page, its sidebar row's states, the palette's Hand off and the thread's send
//! states, on the production Workspace with a fixture backend that answers the agent API from the test.
use super::super::navigation::test_backend::BackendProbe;
use super::super::search::Target;
use super::super::sidebar::{ASSISTANT_HINT, Trailing, assistant_trailing};
use super::super::{Auth, OpenAssistant, View, Workspace, machine_source::MachineSource};
use crate::agent::{Call, Chunk, Sent};
use crate::gateway::Account;
use crate::pages::PageId;
use crate::pages::agent::RowState;
use crate::workspace::overlay::OverlaySlot;
use gpui_kit::{Context, Entity, Task, TestAppContext, VisualTestContext, Window};
use serde_json::json;
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn account(personal: bool) -> Account {
    let kind = if personal { "personal" } else { "company" };
    Account::from_json(&json!({"tenant":{"id":"org-a","slug":"org-a","name":"Acme","kind":kind},"user":{"id":"person-a","personId":"person-a","name":"A","email":"a@example.test"},"role":"member","permissions":[]})).expect("account wire fixture")
}

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
    let account = account(false);
    workspace.accept_navigation_identity(&account, cx);
    workspace.account = Some(account);
    // An agent API, as `NEXUS_AGENTS_API_URL` or the setting gives one.
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

const ANSWER: &str = r#"{"kind":"answer","text":"Done.","model":"nexus/auto"}"#;
const PENDING: &str = r##"{"kind":"pending_action","card":"c1","approveUrl":"https://nexus.test/app/approvals/a1","detail":"Approve it in Nexus.","write":{"connector":"slack","operation":"post_message","summary":"Post in #general","effect":"irreversible","arguments":{}}}"##;

/// The last agent API call's body, and its answer's sender.
fn last_ask(backend: &BackendProbe) -> (String, futures::channel::mpsc::UnboundedSender<Chunk>) {
    let calls = backend.agent_calls.lock().expect("agent calls");
    let (call, answer) = calls.last().expect("an agent call");
    let Call::Ask { body } = call else {
        panic!("an ask: {call:?}")
    };
    (body.clone(), answer.clone())
}

fn asks(backend: &BackendProbe) -> usize {
    backend
        .agent_calls
        .lock()
        .expect("agent calls")
        .iter()
        .filter(|(call, _)| matches!(call, Call::Ask { .. }))
        .count()
}

/// Nexus answers the last ask with `message`, as the agent API streams it.
fn answer(backend: &BackendProbe, message: &str, cx: &mut VisualTestContext) {
    let (_, answer) = last_ask(backend);
    for chunk in [
        head(200, "text/event-stream"),
        sse("started", r#"{"conversation":"c1"}"#),
        sse("text", r#"{"delta":"Done."}"#),
        sse(
            "done",
            &format!(r#"{{"status":200,"conversation":"c1","messages":[{message}]}}"#),
        ),
        Chunk::End,
    ] {
        answer.unbounded_send(chunk).expect("the page reads its answer");
    }
    cx.run_until_parked();
}

fn row(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> RowState {
    workspace.update(cx, |workspace, _| workspace.assistant.row)
}

/// Leave the Assistant for the Inbox, as the window draws it: the page learns the person left.
fn leave(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_inbox(window, cx);
        workspace.sync_assistant(cx);
    });
    cx.run_until_parked();
}

/// A21·c: the row breathes while an answer streams, counts the replies that land while the person is elsewhere, and
/// says Needs you while a write waits. The page is kept, so the answer goes on while the person is away.
#[gpui_kit::test]
fn the_row_breathes_counts_unread_replies_and_says_needs_you(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    assert_eq!(
        assistant_trailing(row(&workspace, cx)),
        Trailing::Hint,
        "idle: the ⌘J hint"
    );

    let first = workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("hi".into(), window, cx);
        assert!(matches!(workspace.view, View::Page(PageId::Agent)));
        workspace.assistant.kept.as_ref().expect("kept").page.entity_id()
    });
    cx.run_until_parked();
    assert!(row(&workspace, cx).thinking, "the mark breathes while Nexus answers");

    leave(&workspace, cx);
    answer(&backend, ANSWER, cx);
    let state = row(&workspace, cx);
    assert_eq!((state.unread, state.thinking), (1, false));
    assert_eq!(assistant_trailing(state), Trailing::Unread(1));

    // Coming back shows the same thread, and reads the reply.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        assert_eq!(workspace.assistant.kept.as_ref().expect("kept").page.entity_id(), first);
    });
    cx.run_until_parked();
    assert_eq!(row(&workspace, cx).unread, 0);

    // A write that waits needs the person, over the count.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("post it".into(), window, cx)
    });
    cx.run_until_parked();
    leave(&workspace, cx);
    answer(&backend, PENDING, cx);
    let state = row(&workspace, cx);
    assert!(state.needs_you && state.unread == 1);
    assert_eq!(assistant_trailing(state), Trailing::NeedsYou);

    // Another account drops the page and its row.
    workspace.update(cx, |workspace, cx| {
        let mut other = account(true);
        other.org = "org-b".into();
        workspace.account = Some(other);
        workspace.assistant_account(cx);
        assert!(workspace.assistant.kept.is_none());
        assert_eq!(workspace.assistant.row, RowState::default());
    });
}

/// A21·e: over typed text the palette's first row is Hand off, with ⌘J; choosing it, or Cmd+J in the field, opens
/// the Assistant and sends the text.
#[gpui_kit::test]
fn the_palette_hands_typed_text_off_to_the_assistant(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_palette(window, cx);
        workspace.palette_typed("dinner".into(), cx);
        let rows = workspace.palette_rows(cx);
        assert!(matches!(&rows[0].target, Target::HandOff(text) if text == "dinner"));
        assert_eq!(rows[0].title.as_ref(), "Hand off “dinner”");
        assert_eq!(rows[0].keys.as_ref().map(|keys| keys.as_ref()), Some(ASSISTANT_HINT));
        // A scope keeps to its own rows.
        workspace.palette_typed(">dinner".into(), cx);
        assert!(
            !workspace
                .palette_rows(cx)
                .first()
                .is_some_and(|row| matches!(row.target, Target::HandOff(_)))
        );
        workspace.palette_typed("dinner".into(), cx);
        assert!(workspace.palette_key("enter", false, window, cx));
        assert!(!workspace.overlay.palette().is_some());
        assert!(matches!(workspace.view, View::Page(PageId::Agent)));
    });
    cx.run_until_parked();
    assert_eq!(last_ask(&backend).0, r#"{"message":"dinner","stream":true}"#);
    assert_eq!(asks(&backend), 1);

    // Cmd+J while the field has text does the same.
    answer(&backend, ANSWER, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_palette(window, cx);
        workspace.palette_typed("book nopa".into(), cx);
        workspace.on_open_assistant(&OpenAssistant, window, cx);
        assert!(!workspace.overlay.palette().is_some());
    });
    cx.run_until_parked();
    assert_eq!(
        last_ask(&backend).0,
        r#"{"message":"book nopa","stream":true,"conversation":"c1"}"#
    );

    // Without the Assistant there is no Hand off.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.agent = false;
        workspace.open_palette(window, cx);
        workspace.palette_typed("dinner".into(), cx);
        assert!(workspace.hand_off_text().is_none());
        assert!(!matches!(workspace.palette_rows(cx).first(), Some(row) if matches!(row.target, Target::HandOff(_))));
    });
}

/// A1·C: Delivered, then Read once Nexus starts; Not sent when the agent service does not answer, with the offline
/// line, and the message goes again by itself after a while.
#[gpui_kit::test]
fn the_thread_reads_delivered_read_and_not_sent_and_resends_when_offline(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("hi".into(), window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    assert_eq!(
        page.read_with(cx, |page, _| page.sent()),
        (Some(Sent::Delivered), false)
    );
    let (_, answer_to) = last_ask(&backend);
    answer_to.unbounded_send(head(200, "text/event-stream")).unwrap();
    answer_to
        .unbounded_send(sse("started", r#"{"conversation":"c1"}"#))
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        page.read_with(cx, |page, _| page.sent()),
        (Some(Sent::Read(_)), false)
    ));
    answer(&backend, ANSWER, cx);

    // The agent service does not answer: not sent, offline.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("make it 8".into(), window, cx)
    });
    cx.run_until_parked();
    let (_, answer_to) = last_ask(&backend);
    let (status, error, detail) = nexus_desktop_core::agent::UNREACHABLE;
    for chunk in [
        head(status, "application/json"),
        Chunk::Data(nexus_desktop_core::agent::failure_body(error, detail).into_bytes()),
        Chunk::End,
    ] {
        answer_to.unbounded_send(chunk).unwrap();
    }
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.sent()), (Some(Sent::NotSent), true));
    assert_eq!(asks(&backend), 2);
    // After a while it goes again by itself.
    cx.executor().advance_clock(std::time::Duration::from_secs(16));
    cx.run_until_parked();
    assert_eq!(asks(&backend), 3);
    assert_eq!(
        last_ask(&backend).0,
        r#"{"message":"make it 8","stream":true,"conversation":"c1"}"#
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.sent()),
        (Some(Sent::Delivered), false)
    );
}

/// The agent API's open answer to the last call that `kind` picks.
fn last_call(backend: &BackendProbe, kind: fn(&Call) -> bool) -> futures::channel::mpsc::UnboundedSender<Chunk> {
    let calls = backend.agent_calls.lock().expect("agent calls");
    let (_, answer) = calls.iter().rev().find(|(call, _)| kind(call)).expect("the call");
    answer.clone()
}

/// A19, A6·e and A9 in the window: the first open reads `GET /agent/intro`, shows, keeps it seen, and a link sends its
/// line; then a Connect card and an approval card draw in the thread, and Approve decides it there.
#[gpui_kit::test]
fn the_first_open_shows_sends_a_line_and_the_cards_draw(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    let intro = last_call(&backend, |call| matches!(call, Call::Intro));
    for chunk in [
        head(200, "application/json"),
        Chunk::Data(
            br#"{"firstOpen":true,"line":"Hand me anything.","starters":["Summarise my inbox"],"reads":[],"accounts":true,"discover":true}"#
                .to_vec(),
        ),
        Chunk::End,
    ] {
        intro.unbounded_send(chunk).expect("the page reads the first open");
    }
    cx.run_until_parked();
    assert!(page.read_with(cx, |page, _| page.first_open().is_some()));
    let seen = backend
        .agent_calls
        .lock()
        .expect("agent calls")
        .iter()
        .any(|(call, _)| *call == Call::IntroSeen);
    assert!(seen, "kept seen once it shows");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // "See what I can do" sends `discover` as the first question, and the first open goes.
    page.update_in(cx, |page, window, cx| {
        page.start_with(nexus_desktop_core::agent_cards::DISCOVER_LINE, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(last_ask(&backend).0, r#"{"message":"discover","stream":true}"#);
    assert!(page.read_with(cx, |page, _| page.first_open().is_none()));

    // The answer needed Linear: its Connect card draws under it. Then a write waits: the approval card draws.
    answer(
        &backend,
        r#"{"kind":"answer","text":"Connect Linear first.","connect":[{"connector":"linear","label":"Linear","url":"https://nexus.test/app/org-a/settings/connectors#linear"}]}"#,
        cx,
    );
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("text them".into(), window, cx)
    });
    cx.run_until_parked();
    answer(
        &backend,
        r##"{"kind":"pending_action","card":"card_cccccccccccccccccccccccccccccccc","inChat":"approve-or-decline","approveUrl":"https://nexus.test/app/approvals/a2","detail":"d","expiresAt":"2099-01-01T00:00:00Z","write":{"connector":"slack","operation":"post_message","summary":"Text Ana","effect":"irreversible"},"asks":{"tier":"asks-first","to":["Ana R."],"text":"Dinner at 7:30.","amountUsd":"31.4","why":{"said":"tell them","at":"2026-11-09T08:24:00Z"},"always":true}}"##,
        cx,
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(row(&workspace, cx).needs_you);

    // Approve in the thread: the agent API's own route, with "Always allow this kind"; then how it ended.
    let card = "card_cccccccccccccccccccccccccccccccc";
    page.update_in(cx, |page, window, cx| {
        page.decide(card, nexus_desktop_core::agent_cards::Decision::Approve, window, cx)
    });
    cx.run_until_parked();
    let decide = {
        let calls = backend.agent_calls.lock().expect("agent calls");
        let (call, answer) = calls.last().expect("a decision");
        assert_eq!(
            *call,
            Call::Decide {
                path: format!("/agent/writes/{card}/approve"),
                body: "{}".into()
            }
        );
        answer.clone()
    };
    assert!(matches!(
        page.read_with(cx, |page, _| page.decision(card).cloned()),
        Some(crate::pages::agent::Phase::Working(_))
    ));
    let done = format!(r#"{{"card":"{card}","decision":"approve","outcome":"written","text":"Sent to Ana."}}"#);
    for chunk in [
        head(200, "application/json"),
        Chunk::Data(done.into_bytes()),
        Chunk::End,
    ] {
        decide.unbounded_send(chunk).expect("the page reads the decision");
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(matches!(
        page.read_with(cx, |page, _| page.decision(card).cloned()),
        Some(crate::pages::agent::Phase::Ended(decided)) if decided.text == "Sent to Ana."
    ));
    assert!(!row(&workspace, cx).needs_you, "decided: nothing waits");
}

/// The thread reads like texting. A time line opens it, each message is its own row as on the board,
/// a tapback sticks to a bubble and is kept with the account's marks, and the contact card opens and Esc
/// closes it, before Esc would stop an answer.
#[gpui_kit::test]
fn the_thread_has_time_lines_keeps_tapbacks_and_opens_the_contact_card(cx: &mut TestAppContext) {
    use crate::pages::agent::{Item, Tapback};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("dinner friday?".into(), window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    answer(&backend, ANSWER, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let items = page.read_with(cx, |page, _| page.items().to_vec());
    assert!(
        matches!(&items[..], [Item::Separator(label), Item::Entry(0), Item::Entry(1)] if label.starts_with("Today ")),
        "{items:?}"
    );

    // A tapback on Nexus's answer: on the bubble, and kept with the account's marks for this conversation.
    page.update(cx, |page, cx| page.react("1.0", Tapback::Heart, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let kept = workspace.update(cx, |workspace, _| workspace.assistant.marks.reactions.get("c1", "1.0"));
    assert_eq!(kept, Some(Tapback::Heart));
    page.update(cx, |page, cx| page.react("1.0", Tapback::Heart, cx));
    cx.run_until_parked();
    let kept = workspace.update(cx, |workspace, _| workspace.assistant.marks.reactions.get("c1", "1.0"));
    assert_eq!(kept, None, "the same tapback again takes it off");

    // The header opens Nexus's card; Esc closes it, and does not stop the answer under way.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("and saturday?".into(), window, cx)
    });
    cx.run_until_parked();
    page.update(cx, |page, cx| page.toggle_card(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(page.read_with(cx, |page, _| page.card_open()));
    cx.simulate_keystrokes("escape");
    assert!(!page.read_with(cx, |page, _| page.card_open()));
    assert!(row(&workspace, cx).thinking, "the first Esc only closed the card");
    cx.simulate_keystrokes("escape");
    assert!(!row(&workspace, cx).thinking, "the next Esc stops the answer");
}

/// The Assistant goes by the name the person gave its profile, in the chat and its views.
/// An email to approve draws its subject, body and signature apart (A7·a), and a written result with a title ends
/// as its own row with Open (A7·f).
#[gpui_kit::test]
fn the_assistant_goes_by_its_name_and_the_card_shows_the_exact_text_in_parts(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    *backend.profiles.lock().expect("profiles") = Some(json!({ "data": [
        { "id": "agp_team", "owner": "team", "name": "Support Bot" },
        { "id": "agp_person", "owner": "person", "name": " Ada " }
    ] }));
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update(cx, |workspace, cx| {
        workspace.forget_assistant();
        workspace.assistant_account(cx);
    });
    cx.run_until_parked();
    assert!(
        backend
            .reads
            .lock()
            .expect("reads")
            .iter()
            .any(|path| path == "/v1/agents/profiles")
    );
    let name = workspace.update(cx, |workspace, _| workspace.assistant.name.clone());
    assert_eq!(name.as_deref(), Some("Ada"));

    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.name().clone()), "Ada");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // A name read later reaches the kept chat.
    workspace.update(cx, |workspace, cx| workspace.set_assistant_name("Cedar".into(), cx));
    assert_eq!(page.read_with(cx, |page, _| page.name().clone()), "Cedar");

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("nudge dana about the SOW".into(), window, cx)
    });
    cx.run_until_parked();
    let card = "card_dddddddddddddddddddddddddddddddd";
    answer(
        &backend,
        &json!({ "kind": "pending_action", "card": card, "inChat": "approve-or-decline", "approveUrl": "https://nexus.test/app/approvals/a3",
                 "detail": "d", "expiresAt": "2099-01-01T00:00:00Z",
                 "write": { "connector": "outlook", "operation": "send", "summary": "Email Dana K.", "effect": "reversible",
                            "arguments": { "to": "dana@acme.test", "subject": "SOW v3, two questions", "body": "…" } },
                 "asks": { "tier": "asks-first", "to": ["Dana K. · dana@acme.test"],
                           "text": "Hi Dana, Maya asked me to follow up on the SOW.\n\nSent by Nexus, Maya's assistant",
                           "why": { "said": "nudge dana about the SOW", "at": "2026-11-09T09:02:00Z" }, "always": true } })
        .to_string(),
        cx,
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));

    page.update_in(cx, |page, window, cx| {
        page.decide(card, nexus_desktop_core::agent_cards::Decision::Approve, window, cx)
    });
    cx.run_until_parked();
    let decide = {
        let calls = backend.agent_calls.lock().expect("agent calls");
        calls.last().expect("a decision").1.clone()
    };
    let done = json!({ "card": card, "decision": "approve", "outcome": "written", "text": "Sent. It's on both calendars.",
                       "result": { "title": "SOW review with Dana K.", "url": "https://calendar.test/e/1" } });
    for chunk in [
        head(200, "application/json"),
        Chunk::Data(done.to_string().into_bytes()),
        Chunk::End,
    ] {
        decide.unbounded_send(chunk).expect("the page reads the decision");
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(matches!(
        page.read_with(cx, |page, _| page.decision(card).cloned()),
        Some(crate::pages::agent::Phase::Ended(decided))
            if decided.title.as_deref() == Some("SOW review with Dana K.")
                && decided.url.as_deref() == Some("https://calendar.test/e/1")
    ));
}

/// A Gateway without profiles, or a failed read, keeps Nexus.
#[gpui_kit::test]
fn without_a_profile_the_assistant_is_nexus(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update(cx, |workspace, cx| {
        workspace.forget_assistant();
        workspace.assistant_account(cx);
    });
    cx.run_until_parked();
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    assert_eq!(page.read_with(cx, |page, _| page.name().clone()), "Nexus");
}

type EarsSender = futures::channel::mpsc::UnboundedSender<crate::voice::ears::Heard>;

/// Voice mode's ears for the window test: the test speaks into them. Its asks reach the fixture's agent API.
struct TestEars(Rc<RefCell<Option<EarsSender>>>, BackendProbe);

impl crate::voice::pane::Senses for TestEars {
    fn hear(&self) -> crate::voice::Hearing {
        let (out, events) = futures::channel::mpsc::unbounded();
        *self.0.borrow_mut() = Some(out);
        crate::voice::Hearing { events, _stop: None }
    }

    fn ask(&self, call: Call) -> crate::agent::Stream {
        crate::backend::Backend::agent(&self.1, Some("https://agents.test".into()), call)
    }
}

/// A voice that says nothing and is done at once.
struct Quiet;

impl crate::voice::speaker::Speaker for Quiet {
    fn speak(&mut self, _: &str) -> bool {
        false
    }
    fn stop(&mut self) {}
    fn speaking(&self) -> bool {
        false
    }
}

/// A18·e: a voice call asks in the Assistant's conversation; when it ends, its card lands in the thread, then the
/// write it asked for as its approval card, so the row says Needs you.
#[gpui_kit::test]
fn a_voice_call_lands_its_card_and_its_write_in_the_thread(cx: &mut TestAppContext) {
    use crate::voice::ears::Heard;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| workspace.open_agent(None, window, cx));
    cx.run_until_parked();
    let page = workspace.update(cx, |workspace, _| {
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    let entries = |page: &Entity<crate::pages::agent::AgentPage>, cx: &mut VisualTestContext| {
        page.read_with(cx, |page, _| {
            page.items()
                .iter()
                .filter(|item| matches!(item, crate::pages::agent::Item::Entry(_)))
                .count()
        })
    };
    let before = entries(&page, cx);

    let ears = Rc::new(RefCell::new(None));
    let senses = Rc::new(TestEars(ears.clone(), backend.clone()));
    page.update_in(cx, |page, window, cx| {
        page.start_voice_with(senses, Box::new(Quiet), window, cx)
    });
    let speak = |heard: Heard| {
        let ears = ears.borrow();
        ears.as_ref()
            .expect("open ears")
            .unbounded_send(heard)
            .expect("the pane listens");
    };
    speak(Heard::Opened);
    speak(Heard::Turn("Post it in general.".into()));
    cx.run_until_parked();
    let (body, _) = last_ask(&backend);
    assert!(body.contains("Post it in general."), "{body}");
    answer(&backend, PENDING, cx);

    let voice = page.read_with(cx, |page, _| page.voice()).expect("the voice pane");
    voice.update(cx, |voice, cx| voice.end(cx));
    cx.run_until_parked();
    assert!(page.read_with(cx, |page, _| page.voice()).is_none(), "the pane closed");
    assert_eq!(entries(&page, cx), before + 2, "the card, then the write");
    let conversation = page.read_with(cx, |page, _| page.conversation().map(String::from));
    assert_eq!(conversation.as_deref(), Some("c1"));
    assert!(page.read_with(cx, |page, _| page.row()).needs_you);
}

/// A24·e: ⌘K matches the Assistant's settings ("Assistant › Approvals"), each opening its native view; without the
/// Assistant there are none.
#[gpui_kit::test]
fn the_palette_matches_the_assistants_settings(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_palette(window, cx);
        for (typed, title, view) in [
            ("approvals", "Assistant › Approvals", PageId::AgentApprovals),
            ("proactivity", "Assistant › Proactivity", PageId::AgentApprovals),
            ("privacy", "Assistant › Privacy", PageId::AgentPrivacy),
            ("passwords", "Assistant › Passwords and payments", PageId::AgentVault),
            ("memory", "Assistant › Memory", PageId::AgentMemory),
            ("discover", "Assistant › Discover", PageId::AgentDiscover),
            ("errands", "Assistant › Errands", PageId::AgentErrands),
            ("projects", "Assistant › Projects", PageId::AgentErrands),
        ] {
            workspace.palette_typed(typed.into(), cx);
            let rows = workspace.palette_rows(cx);
            let row = rows
                .iter()
                .find(|row| row.title.as_ref() == title)
                .unwrap_or_else(|| panic!("{title} for {typed}"));
            assert_eq!(row.target, Target::AssistantView(view));
        }
        workspace.agent = false;
        workspace.palette_typed("approvals".into(), cx);
        assert!(
            !workspace
                .palette_rows(cx)
                .iter()
                .any(|row| matches!(row.target, Target::AssistantView(_)))
        );
    });
}

fn other_org(id: &str, name: &str, personal: bool) -> super::super::org_menu::Org {
    super::super::org_menu::Org {
        id: id.into(),
        slug: Some(id.into()),
        name: name.into(),
        personal,
        current: false,
        logo: None,
        role: None,
        provider_org_id: None,
    }
}

/// A24·g: a switch with an Assistant view open lands on the same view of the new account, and ⌘[ goes back to the
/// account it left while that view is on screen.
#[gpui_kit::test]
fn a_switch_lands_on_the_same_assistant_view_and_back_returns(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.accounts.orgs = vec![other_org("org-a", "Acme", false), other_org("me", "Personal", true)];
        workspace.open_assistant(PageId::AgentErrands, None, window, cx);
        assert!(matches!(workspace.view, View::Page(PageId::AgentErrands)));

        // The switch leaves for Personal: the view is remembered, and the old account's late answer lands nothing.
        workspace.before_switch();
        workspace.new_chat(window, cx);
        assert!(!workspace.land_after_switch(window, cx), "still the old account");
        let mut personal = account(true);
        personal.org = "me".into();
        workspace.account = Some(personal);
        assert!(workspace.land_after_switch(window, cx));
        assert!(matches!(workspace.view, View::Page(PageId::AgentErrands)));
        let (back, view) = workspace.accounts.back_target().expect("a way back");
        assert_eq!((back.id.as_str(), view), ("org-a", PageId::AgentErrands));

        // Anywhere else, ⌘[ is the history's.
        workspace.open_inbox(window, cx);
        assert!(!workspace.switch_back(window, cx));
        assert!(workspace.accounts.back_target().is_none());

        // A switch from the inbox opens nothing.
        workspace.before_switch();
        assert!(!workspace.land_after_switch(window, cx));
    });
}

/// A24·f: the account on screen is live; another account shows what was kept, and nothing once it is a day old.
#[gpui_kit::test]
fn the_switcher_shows_each_accounts_assistant(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update(cx, |workspace, _| {
        let now = crate::code::now_ms();
        let peek = |needs_you: bool, ago: i64| super::super::account_states::Peek {
            needs_you,
            unread: 0,
            paused: false,
            off: false,
            at: now - ago,
        };
        workspace.accounts.set_peek("other", peek(true, 3_600_000));
        workspace.accounts.set_peek("old", peek(true, 25 * 3_600_000));
        let (shown, live) = workspace
            .account_peek(&other_org("other", "Other", false))
            .expect("kept");
        assert!(shown.needs_you && !live);
        assert!(workspace.account_peek(&other_org("old", "Old", false)).is_none());
        assert!(workspace.account_peek(&other_org("never", "Never", false)).is_none());
        let (_, live) = workspace
            .account_peek(&other_org("org-a", "Acme", false))
            .expect("live");
        assert!(live);
        workspace.agent = false;
        assert!(workspace.account_peek(&other_org("other", "Other", false)).is_none());
    });
}
