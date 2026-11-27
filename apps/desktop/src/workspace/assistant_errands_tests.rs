//! Each errand is its own thread (A12, A21·a, A23), on the production Workspace with a fixture backend
//! that answers the agent API from the test: the sidebar's rows, the chat's overview card, an errand's thread with its
//! reply and its approval card, the ⋯ menu, the History's search and New errand. The window draws after each answer,
//! so a panic in a render fails the test.
use super::super::navigation::test_backend::BackendProbe;
use super::super::search::Target;
use super::super::sidebar::NEW_ERRAND_HINT;
use super::super::{Auth, NewErrand, View, Workspace, machine_source::MachineSource};
use crate::agent::{Call, Chunk};
use crate::gateway::Account;
use crate::pages::PageId;
use crate::pages::agent::Phase;
use crate::pages::assistant_views::ErrandsPage;
use crate::workspace::overlay::OverlaySlot;
use gpui_kit::{Context, Entity, Task, TestAppContext, VisualTestContext, Window};
use nexus_desktop_core::agent_cards::Decision;
use nexus_desktop_core::agent_errands::{Edit, Entry, MenuItem};
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

fn calls(backend: &BackendProbe) -> Vec<Call> {
    backend
        .agent_calls
        .lock()
        .expect("agent calls")
        .iter()
        .map(|(call, _)| call.clone())
        .collect()
}

/// Answer the last call that `matches` with `status` and `body`, then let the window draw it.
fn answer(
    backend: &BackendProbe,
    matches: impl Fn(&Call) -> bool,
    status: u16,
    body: Value,
    cx: &mut VisualTestContext,
) {
    let sender = {
        let calls = backend.agent_calls.lock().expect("agent calls");
        let (_, sender) = calls
            .iter()
            .rev()
            .find(|(call, _)| matches(call))
            .expect("the call was made");
        sender.clone()
    };
    for chunk in [
        Chunk::Head {
            status,
            content_type: "application/json".into(),
        },
        Chunk::Data(body.to_string().into_bytes()),
        Chunk::End,
    ] {
        sender.unbounded_send(chunk).expect("the view reads its answer");
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn row(id: &str, goal: &str, status: &str, extra: Value) -> Value {
    let mut row = json!({ "id": id, "goal": goal, "status": status, "at": "2026-11-17T08:00:00Z" });
    if let (Value::Object(row), Value::Object(extra)) = (&mut row, extra) {
        row.extend(extra);
    }
    row
}

fn list() -> Value {
    json!({
        "errands": [
            row("i3", "Dinner Friday", "needs-you", json!({ "need": "approval", "detail": "Book Nopa" })),
            row("i4", "Gift for Ana", "running", json!({ "steps": 3 })),
        ],
        "history": [
            row("i1", "Car registration", "done", json!({})),
            row("i2", "Offsite venue", "done", json!({ "archived": true })),
        ]
    })
}

const CARD: &str = "card_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// Dinner Friday as its own thread: the goal, one step, a reply of the person's and the agent's, with a card.
fn dinner(extra: Value) -> Value {
    let mut view = json!({
        "errand": row("i3", "Dinner Friday", "needs-you", json!({ "need": "approval", "detail": "Book Nopa", "steps": 1 })),
        "constraints": [], "surface": "api", "createdAt": "2026-11-17T08:02:00Z",
        "steps": [{ "id": "s1", "text": "Compared 6 places", "urls": ["https://nopa.test/menu"], "at": "2026-11-17T08:10:00Z" }],
        "actions": ["done", "cancel"],
        "thread": [
            { "role": "user", "text": "near Divisadero", "at": "2026-11-17T08:15:00Z" },
            { "role": "assistant", "text": "Book Nopa?", "at": "2026-11-17T08:20:00Z", "messages": [
                { "kind": "answer", "text": "Nopa is the **best fit**: 4.6, 6 minutes, 7:30." },
                { "kind": "pending_action", "card": CARD, "inChat": "approve-or-decline", "approveUrl": "https://nexus.test/app/approvals/a1",
                  "detail": "d", "expiresAt": "2099-01-01T00:00:00Z",
                  "write": { "connector": "opentable", "operation": "book", "summary": "Book Nopa, Friday 7:30 PM, for 4", "effect": "reversible" },
                  "asks": { "tier": "asks-first", "to": ["Nopa · 560 Divisadero St"] } }
            ]}
        ]
    });
    if let (Value::Object(view), Value::Object(extra)) = (&mut view, extra) {
        view.extend(extra);
    }
    view
}

fn errands_page(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<ErrandsPage> {
    workspace.update(cx, |workspace, _| {
        assert!(
            matches!(workspace.view, View::Page(PageId::AgentErrands)),
            "Errands is open"
        );
        workspace
            .native_page
            .as_ref()
            .expect("a page")
            .1
            .clone()
            .downcast::<ErrandsPage>()
            .expect("Errands")
    })
}

/// A21·a and A12·c: the sidebar lists the open errands under the Assistant, what needs the person first, and the chat
/// opens with "1 errand needs you". A row opens that errand's thread alone, and its row is the current one.
#[gpui_kit::test]
fn the_sidebar_and_the_chat_follow_the_errands(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    // The account is known: its errands are read once.
    workspace.update(cx, |workspace, cx| workspace.assistant_account(cx));
    assert_eq!(calls(&backend).iter().filter(|call| **call == Call::Errands).count(), 1);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    workspace.read_with(cx, |workspace, _| {
        let rows: Vec<&str> = workspace
            .assistant
            .errand_rows
            .iter()
            .map(|errand| errand.id.as_str())
            .collect();
        assert_eq!(rows, ["i3", "i4"]);
    });

    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant_kept_page().expect("the chat")
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        page.read_with(cx, |page, _| page.errands_card()).as_deref(),
        Some("1 errand needs you")
    );

    // The row: Errands opens on that errand, its thread alone in the main column (A23·b).
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let errands = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    let read = Call::errand("i3").expect("its read");
    answer(&backend, |call| *call == read, 200, dinner(json!({})), cx);
    errands.read_with(cx, |page, _| {
        assert!(page.alone(), "opened on one errand, the thread is alone");
        assert_eq!(page.status(), Some("Opened Dinner Friday."));
    });
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.assistant.open_errand.as_deref(), Some("i3"))
    });

    // Errands off at the agent API: no rows, and no card.
    workspace.update(cx, |workspace, cx| workspace.refresh_errands(cx));
    answer(
        &backend,
        |call| *call == Call::Errands,
        503,
        json!({ "error": "errands_off" }),
        cx,
    );
    workspace.read_with(cx, |workspace, _| assert!(workspace.assistant.errand_rows.is_empty()));
    assert_eq!(page.read_with(cx, |page, _| page.errands_card()), None);
}

/// A12 and A23·b: an errand reads as its thread, a reply goes to its own thread and the answer shows there, and the
/// approval card inside it decides the write with the chat's call.
#[gpui_kit::test]
fn an_errand_is_a_thread_with_a_reply_and_its_card(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    let read = Call::errand("i3").expect("its read");
    answer(&backend, |call| *call == read, 200, dinner(json!({})), cx);
    page.read_with(cx, |page, _| {
        let kinds: Vec<&str> = page
            .entries()
            .iter()
            .map(|entry| match entry {
                Entry::Day(_) => "day",
                Entry::You { .. } => "you",
                Entry::Step { .. } => "step",
                Entry::Nexus { .. } => "nexus",
            })
            .collect();
        assert_eq!(kinds, ["day", "you", "step", "you", "nexus"]);
    });

    page.update_in(cx, |page, window, cx| {
        page.type_in("composer", "  book it  ", window, cx);
        page.press_enter("composer", window, cx);
    });
    let reply = Call::reply_errand("i3", "book it").expect("a reply");
    assert_eq!(calls(&backend).last(), Some(&reply));
    assert_eq!(
        reply,
        Call::ReplyErrand {
            path: "/agent/errands/i3/messages".into(),
            body: r#"{"message":"book it"}"#.into()
        }
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let mut after = dinner(json!({}));
    after["thread"]
        .as_array_mut()
        .expect("a thread")
        .extend([
            json!({ "role": "user", "text": "book it", "at": "2026-11-18T07:00:00Z" }),
            json!({ "role": "assistant", "text": "On it.", "at": "2026-11-18T07:00:05Z", "messages": [{ "kind": "notice", "text": "On it." }] }),
        ]);
    answer(
        &backend,
        |call| *call == reply,
        200,
        json!({ "messages": [{ "kind": "notice", "text": "On it." }], "errand": after }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Nexus answered."));
        assert_eq!(page.errand().map(|view| view.thread.len()), Some(4));
    });
    assert_eq!(calls(&backend).last(), Some(&Call::Errands), "the list moves with it");

    // The card in the thread: Approve sends the chat's own call, and the errand is read again after it.
    page.update_in(cx, |page, window, cx| {
        crate::pages::agent::Decides::decide(page, CARD, Decision::Approve, window, cx)
    });
    let decide = Call::Decide {
        path: format!("/agent/writes/{CARD}/approve"),
        body: "{}".into(),
    };
    assert!(calls(&backend).contains(&decide), "{:?}", calls(&backend));
    answer(
        &backend,
        |call| matches!(call, Call::Decide { .. }),
        200,
        json!({ "card": CARD, "decision": "approve", "outcome": "written", "text": "Booked Nopa for 7:30." }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert!(matches!(page.decision(CARD), Some(Phase::Ended(_))))
    });
    assert_eq!(calls(&backend).iter().filter(|call| **call == read).count(), 2);
}

/// A12·e: the ⋯ menu pins, renames and archives with one PATCH each, and the History's search finds a closed one.
#[gpui_kit::test]
fn the_menu_edits_the_errand_and_the_history_searches(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, None, window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    page.update(cx, |page, cx| page.show_history(cx));
    page.update_in(cx, |page, window, cx| page.type_in("search", "CAR reg", window, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    page.read_with(cx, |page, _| assert_eq!(page.found(), ["i1"]));

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    let read = Call::errand("i3").expect("its read");
    answer(&backend, |call| *call == read, 200, dinner(json!({})), cx);

    let pin = Edit {
        pinned: Some(true),
        ..Edit::default()
    };
    page.update_in(cx, |page, window, cx| page.choose_item(MenuItem::Pin, window, cx));
    let call = Call::edit_errand("i3", &pin).expect("a pin");
    assert_eq!(calls(&backend).last(), Some(&call));
    let mut pinned = dinner(json!({}));
    pinned["errand"]["pinned"] = json!(true);
    answer(&backend, |made| *made == call, 200, pinned.clone(), cx);
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Pinned."));
        assert!(page.errand().is_some_and(|view| view.errand.pinned));
    });

    page.update_in(cx, |page, window, cx| {
        page.choose_item(MenuItem::Rename, window, cx);
        page.type_in("rename", "Nopa on Friday", window, cx);
        page.press_enter("rename", window, cx);
    });
    let rename = Edit {
        title: Some(Some("Nopa on Friday".into())),
        ..Edit::default()
    };
    let call = Call::edit_errand("i3", &rename).expect("a rename");
    assert_eq!(calls(&backend).last(), Some(&call));
    let mut named = pinned.clone();
    named["errand"]["title"] = json!("Nopa on Friday");
    answer(&backend, |made| *made == call, 200, named, cx);
    page.read_with(cx, |page, _| assert_eq!(page.status(), Some("Renamed.")));

    page.update_in(cx, |page, window, cx| page.choose_item(MenuItem::Archive, window, cx));
    let archive = Edit {
        archived: Some(true),
        ..Edit::default()
    };
    assert_eq!(calls(&backend).last(), Call::edit_errand("i3", &archive).as_ref());
}

/// A12·e: Hand off to a session reads the errand's prompt from the agent API, opens a New session draft with it, sends
/// nothing, and records the step "Handed off to a session".
#[gpui_kit::test]
fn hand_off_opens_a_new_session_draft_and_records_the_step(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, list(), cx);
    let read = Call::errand("i3").expect("its read");
    answer(&backend, |call| *call == read, 200, dinner(json!({})), cx);

    page.update_in(cx, |page, window, cx| page.choose_item(MenuItem::HandOff, window, cx));
    let handoff = Call::errand_handoff("i3").expect("its hand-off");
    assert_eq!(calls(&backend).last(), Some(&handoff));
    answer(
        &backend,
        |call| *call == handoff,
        200,
        json!({ "prompt": "Continue my errand in this session.\n\nGoal: Dinner Friday" }),
        cx,
    );
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(workspace.read_with(cx, |workspace, _| matches!(workspace.view, View::NewCode)));
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(
            workspace.composer.read(cx).value().to_string(),
            "Continue my errand in this session.\n\nGoal: Dinner Friday"
        );
    });
    // The step, and no ask: nothing was sent to a session or to the Assistant.
    let step = Call::handed_off("i3").expect("its step");
    assert_eq!(calls(&backend).last(), Some(&step));
    assert!(!calls(&backend).iter().any(|call| matches!(call, Call::Ask { .. })));
    let mut handed = dinner(json!({}));
    handed["steps"] = json!([{ "id": "s2", "kind": "code", "text": "Handed off to a session", "urls": [], "at": "2026-11-17T09:00:00Z" }]);
    answer(&backend, |call| *call == step, 200, handed, cx);
    page.read_with(cx, |page, _| {
        assert_eq!(
            page.status(),
            Some("Your New session draft has the errand in it. Send it when it reads right.")
        );
        assert!(
            page.errand()
                .is_some_and(|view| view.steps.iter().any(|step| step.text == "Handed off to a session"))
        );
    });

    // A refusal says why, and opens no draft.
    page.update_in(cx, |page, window, cx| page.choose_item(MenuItem::HandOff, window, cx));
    let before = calls(&backend).len();
    answer(
        &backend,
        |call| *call == handoff,
        404,
        json!({ "error": "not_found" }),
        cx,
    );
    assert_eq!(calls(&backend).len(), before);
}

/// A21·e and A23·e: ⇧⌘J (or ⌘K's New errand) opens Errands with its box holding the text, and Start sends the goal
/// alone; the new errand opens as its thread.
#[gpui_kit::test]
fn a_new_errand_starts_from_the_shortcut(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    // ⌘K over typed text: New errand is the row under Hand off, with ⇧⌘J, and the key opens Errands with it.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_palette(window, cx);
        workspace.palette_typed("Sitter for Friday".into(), cx);
        let rows = workspace.palette_rows(cx);
        assert!(matches!(&rows[1].target, Target::NewErrand(text) if text == "Sitter for Friday"));
        assert_eq!(rows[1].title.as_ref(), "New errand “Sitter for Friday”");
        assert_eq!(rows[1].keys.as_ref().map(|keys| keys.as_ref()), Some(NEW_ERRAND_HINT));
        workspace.on_new_errand(&NewErrand, window, cx);
        assert!(workspace.overlay.palette().is_none());
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    page.read_with(cx, |page, cx| {
        assert_eq!(page.new_errand_text(cx).as_deref(), Some("Sitter for Friday"))
    });
    answer(
        &backend,
        |call| *call == Call::Errands,
        200,
        json!({ "errands": [] }),
        cx,
    );

    // ⇧⌘J again over Errands: the same view's box takes the new text.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_errand("Sitter, Friday night".into(), window, cx)
    });
    cx.run_until_parked();
    page.read_with(cx, |page, cx| {
        assert_eq!(page.new_errand_text(cx).as_deref(), Some("Sitter, Friday night"))
    });

    page.update_in(cx, |page, window, cx| page.press_enter("new", window, cx));
    let open = Call::open_errand("Sitter, Friday night", None).expect("a goal");
    assert_eq!(
        open,
        Call::OpenErrand {
            body: r#"{"goal":"Sitter, Friday night"}"#.into()
        }
    );
    assert_eq!(calls(&backend).last(), Some(&open));
    let mut view = dinner(json!({ "thread": [], "steps": [] }));
    view["errand"] = row("i9", "Sitter, Friday night", "running", json!({}));
    answer(
        &backend,
        |call| *call == open,
        201,
        json!({ "state": "opened", "errand": view }),
        cx,
    );
    page.read_with(cx, |page, cx| {
        assert_eq!(page.status(), Some("Started Sitter, Friday night."));
        assert_eq!(page.new_errand_text(cx), None);
        assert_eq!(page.errand().map(|view| view.errand.id.as_str()), Some("i9"));
    });
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.assistant.open_errand.as_deref(), Some("i9"))
    });

    // Too many open: the agent API's own words.
    page.update_in(cx, |page, window, cx| {
        page.open_new("One more", window, cx);
        page.press_enter("new", window, cx);
    });
    let more = Call::open_errand("One more", None).expect("a goal");
    answer(
        &backend,
        |call| *call == more,
        409,
        json!({ "error": "too_many", "detail": "" }),
        cx,
    );
    page.read_with(cx, |page, cx| {
        assert!(page.new_errand_text(cx).is_some(), "the box stays")
    });
}

/// The list with errands filed in two projects (A14), and one loose.
fn filed() -> Value {
    json!({
        "errands": [
            row("i3", "Dinner Friday", "needs-you", json!({ "need": "approval", "detail": "Book Nopa" })),
            row("i4", "Gift for Ana", "running", json!({ "project": "Fundraise", "steps": 2 })),
            row("i5", "Send the deck", "needs-you", json!({ "project": "Fundraise", "need": "question" })),
            row("i6", "Read the term sheet", "running", json!({ "project": "Summit Labs" })),
        ],
        "history": [row("i1", "Intro to Ana", "done", json!({ "project": "Fundraise" }))]
    })
}

/// A14: only the loose errands stay under the Assistant, with "3 in projects"; the Projects group lists
/// each project with its errands. The ⋯ menu's Move to project files an errand in a project, a new one too, and Keep
/// loose takes it out. A project's page lists its errands, ⇧⌘J goes to its Ask field, and the errand it starts is
/// filed there.
#[gpui_kit::test]
fn errands_file_into_projects_and_a_project_page_asks_nexus(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update(cx, |workspace, cx| workspace.assistant_account(cx));
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    workspace.read_with(cx, |workspace, _| {
        let ids = |rows: &[nexus_desktop_core::agent_errands::Errand]| {
            rows.iter().map(|errand| errand.id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(&workspace.assistant.errand_rows), ["i3"], "the loose one");
        assert_eq!(workspace.assistant.in_projects.as_deref(), Some("3 in projects"));
        let groups: Vec<(String, Vec<String>)> = workspace
            .assistant
            .errand_projects
            .iter()
            .map(|(name, rows)| (name.clone(), ids(rows)))
            .collect();
        assert_eq!(
            groups,
            [
                ("Fundraise".to_string(), vec!["i5".to_string(), "i4".to_string()]),
                ("Summit Labs".to_string(), vec!["i6".to_string()]),
            ]
        );
    });

    // Move to project, from the errand's ⋯ menu: the projects take the menu's place.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    let read = Call::errand("i3").expect("its read");
    answer(&backend, |call| *call == read, 200, dinner(json!({})), cx);
    page.update_in(cx, |page, window, cx| page.choose_item(MenuItem::Move, window, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        page.read_with(cx, |page, _| page.move_menu_projects().map(<[String]>::to_vec)),
        Some(vec!["Fundraise".to_string(), "Summit Labs".to_string()])
    );
    page.update_in(cx, |page, window, cx| {
        page.move_to(Some("Summit Labs".into()), window, cx)
    });
    let moved = Call::edit_errand(
        "i3",
        &Edit {
            project: Some(Some("Summit Labs".into())),
            ..Edit::default()
        },
    )
    .expect("a move");
    assert_eq!(calls(&backend).last(), Some(&moved));
    let filed_view = |project: Value| {
        dinner(
            json!({ "errand": row("i3", "Dinner Friday", "needs-you", json!({ "need": "approval", "detail": "Book Nopa", "project": project })) }),
        )
    };
    answer(
        &backend,
        |call| *call == moved,
        200,
        filed_view(json!("Summit Labs")),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Moved to Summit Labs."));
        assert_eq!(
            page.errand().and_then(|view| view.errand.project.as_deref()),
            Some("Summit Labs")
        );
    });

    // A new project: its name as the agent API reads it.
    page.update_in(cx, |page, window, cx| {
        page.choose_item(MenuItem::Move, window, cx);
        page.name_new_project(window, cx);
        page.type_in("naming", "  Board   prep ", window, cx);
        page.move_to_named(window, cx);
    });
    let named = Call::edit_errand(
        "i3",
        &Edit {
            project: Some(Some("Board prep".into())),
            ..Edit::default()
        },
    )
    .expect("a move");
    assert_eq!(calls(&backend).last(), Some(&named));
    answer(
        &backend,
        |call| *call == named,
        200,
        filed_view(json!("Board prep")),
        cx,
    );

    // Keep loose takes it out.
    page.update_in(cx, |page, window, cx| {
        page.choose_item(MenuItem::Move, window, cx);
        page.move_to(None, window, cx)
    });
    let loose = Call::edit_errand(
        "i3",
        &Edit {
            project: Some(None),
            ..Edit::default()
        },
    )
    .expect("a move");
    assert_eq!(calls(&backend).last(), Some(&loose));
    answer(&backend, |call| *call == loose, 200, dinner(json!({})), cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.status().map(String::from)).as_deref(),
        Some("Loose again, under the Assistant.")
    );

    // A project's page: its errands, open first, then closed; its row is the current one.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_errand_project("Fundraise".into(), window, cx)
    });
    cx.run_until_parked();
    let project = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    project.read_with(cx, |page, _| {
        assert_eq!(page.project(), Some("Fundraise"));
        assert_eq!(page.project_rows(), ["i5", "i4", "i1"]);
    });
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.assistant.open_project.as_deref(), Some("Fundraise"))
    });

    // ⇧⌘J here asks Nexus about the project: the Ask field takes the text, and the errand starts filed there.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.new_errand("Draft the board deck".into(), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        project.read_with(cx, |page, cx| page.asking_text(cx)).as_deref(),
        Some("Draft the board deck")
    );
    project.update_in(cx, |page, window, cx| page.ask_about(window, cx));
    let ask = Call::open_errand("Draft the board deck", Some("Fundraise")).expect("a goal");
    assert_eq!(calls(&backend).last(), Some(&ask));
    let started =
        dinner(json!({ "errand": row("i9", "Draft the board deck", "running", json!({ "project": "Fundraise" })) }));
    answer(
        &backend,
        |call| *call == ask,
        200,
        json!({ "state": "opened", "errand": started }),
        cx,
    );
    project.read_with(cx, |page, _| {
        assert_eq!(page.errand().map(|view| view.errand.id.as_str()), Some("i9"));
        assert_eq!(page.status(), Some("Started Draft the board deck."));
    });
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.assistant.open_errand.as_deref(), Some("i9"));
        assert_eq!(workspace.assistant.open_project, None);
    });
}

/// A Gateway project's page: Fundraise with its repository, a session its errand set up, and a note.
fn fundraise_view(extra: Value) -> Value {
    let mut view = json!({
        "project": { "name": "Fundraise", "id": "prj_fund", "repos": ["acme/data-room"], "errands": 3 },
        "gateway": "on",
        "errands": [], "history": [],
        "sessions": [{ "title": "Update the metrics model", "state": "ready", "where": "desktop", "repo": "acme/data-room",
                       "url": "nexus://new?repo=acme/data-room", "at": "2026-11-18T11:00:00Z", "errand": "i4" }],
        "files": { "source": "repositories", "repos": [{ "fullName": "acme/data-room", "url": "https://github.com/acme/data-room" }] },
        "notes": [{ "id": "n1", "text": "Raising a seed round.", "createdAt": "2026-11-18T00:00:00Z", "audience": "dm",
                    "project": { "id": "prj_fund", "name": "Fundraise" } }],
    });
    if let (Value::Object(view), Value::Object(extra)) = (&mut view, extra) {
        view.extend(extra);
    }
    view
}

/// A14: a project's page reads its view: its sessions join its chats, its repository is where its files
/// come from, and What Nexus knows here adds and forgets a note scoped to it. Ask Nexus starts the errand in the
/// Gateway project by id. A name-only project with a Gateway project of the same name offers Link.
#[gpui_kit::test]
fn a_project_page_shows_its_sessions_files_and_what_nexus_knows(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    workspace.update(cx, |workspace, cx| workspace.assistant_account(cx));
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_errand_project("Fundraise".into(), window, cx)
    });
    cx.run_until_parked();
    let page = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    let read = Call::project_view(None, "Fundraise").expect("its view");
    answer(&backend, |call| *call == read, 200, fundraise_view(json!({})), cx);
    page.read_with(cx, |page, _| {
        let view = page.project_view().expect("the view");
        assert_eq!(view.id.as_deref(), Some("prj_fund"));
        assert_eq!(view.sessions.len(), 1);
        assert_eq!(view.repos.len(), 1);
        assert_eq!(view.notes.len(), 1);
    });

    // Add a note: it is scoped to the Gateway project, and the view reads again.
    page.update_in(cx, |page, window, cx| {
        page.type_in("noting", "Northpoint prefers mornings.", window, cx);
        page.add_project_note(window, cx);
    });
    let view = page
        .read_with(cx, |page, _| page.project_view().cloned())
        .expect("the view");
    let add = Call::add_project_note("Northpoint prefers mornings.", &view).expect("a note");
    assert_eq!(calls(&backend).last(), Some(&add));
    answer(
        &backend,
        |call| *call == add,
        201,
        json!({ "note": { "id": "n2", "text": "Northpoint prefers mornings.", "createdAt": "t", "audience": "dm" } }),
        cx,
    );
    let reread = Call::project_view(Some("prj_fund"), "Fundraise").expect("its view by id");
    assert_eq!(calls(&backend).last(), Some(&reread));
    answer(&backend, |call| *call == reread, 200, fundraise_view(json!({})), cx);

    // Forget one.
    page.update_in(cx, |page, window, cx| page.forget_project_note("n1", window, cx));
    let forget = Call::forget(Some("n1")).expect("a forget");
    assert_eq!(calls(&backend).last(), Some(&forget));
    answer(&backend, |call| *call == forget, 200, json!({ "forgotten": 1 }), cx);

    // Ask Nexus about it: the errand starts in the Gateway project, by id.
    page.update_in(cx, |page, window, cx| {
        page.type_in("asking", "Draft the board deck", window, cx);
        page.ask_about(window, cx);
    });
    let ask = Call::open_errand_in("Draft the board deck", "prj_fund").expect("a goal");
    assert!(calls(&backend).contains(&ask));

    // A name-only project offers Link to the Gateway project of its name.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_errand_project("Summit Labs".into(), window, cx)
    });
    cx.run_until_parked();
    let summit = errands_page(&workspace, cx);
    answer(&backend, |call| *call == Call::Errands, 200, filed(), cx);
    let read = Call::project_view(None, "Summit Labs").expect("its view");
    let offer = json!({
        "project": { "name": "Summit Labs", "repos": [], "errands": 1, "link": { "id": "prj_summit", "name": "Summit Labs" } },
        "files": { "source": "repositories", "repos": [] }, "sessions": [], "notes": [],
    });
    answer(&backend, |call| *call == read, 200, fundraise_view(offer), cx);
    summit.update_in(cx, |page, window, cx| page.link_project(window, cx));
    let link = Call::link_project("Summit Labs", "prj_summit").expect("a link");
    assert_eq!(calls(&backend).last(), Some(&link));
}
