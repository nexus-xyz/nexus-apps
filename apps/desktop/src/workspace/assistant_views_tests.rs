//! The Assistant's views beside its chat (A9, A12, A15, A16) on the production Workspace: the strip that moves between
//! them, Account › Assistant, and each view's calls and answers, with a fixture backend that answers the agent API from
//! the test. Each view is drawn after each answer, so a panic in its render fails the test.
use super::super::navigation::test_backend::BackendProbe;
use super::super::settings_registry::Dest;
use super::super::{Auth, View, Workspace, machine_source::MachineSource};
use crate::agent::{Call, Chunk};
use crate::gateway::Account;
use crate::pages::PageId;
use crate::pages::assistant_views::{
    ApprovalsPage, Asking, DiscoverPage, ErrandsPage, LineMode, MemoryPage, PrivacyPage, STRIP, Section, VaultPage,
    VaultPart,
};
use gpui_kit::{AnyView, Context, Entity, Task, TestAppContext, VisualTestContext, Window};
use nexus_desktop_core::agent_errands::Action;
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

pub(super) fn fixture<'a>(
    cx: &'a mut TestAppContext,
    backend: &BackendProbe,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| make(window, cx, backend))
}

/// The agent API calls so far, in order.
pub(super) fn calls(backend: &BackendProbe) -> Vec<Call> {
    backend
        .agent_calls
        .lock()
        .expect("agent calls")
        .iter()
        .map(|(call, _)| call.clone())
        .collect()
}

/// Answer the last call that `matches` with `status` and `body`, then let the view draw it.
pub(super) fn answer(
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
}

/// Open `id` as the strip or a link does, and return its view.
pub(super) fn open(workspace: &Entity<Workspace>, id: PageId, cx: &mut VisualTestContext) -> AnyView {
    workspace.update_in(cx, |workspace, window, cx| workspace.open_native(id, window, cx));
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        assert!(
            matches!(workspace.view, View::Page(open) if open == id),
            "{id:?} is open"
        );
        assert!(workspace.assistant_strip(id, cx), "the strip is over {id:?}");
        workspace.native_page.as_ref().expect("a page").1.clone()
    })
}

fn approvals_body(rules: Value) -> Value {
    json!({ "tiers": "on", "spendLimitUsd": "50.00", "ownLine": false, "defaultSpendLimitUsd": "50.00",
            "maxSpendLimitUsd": "200.00", "expiresAfterMinutes": 30, "rules": rules })
}

const RULE: &str = "allow_0123456789abcdef01234567";

/// A9: Approvals reads the settings, draws the three tiers and the learned rules, and Reset sends that rule's DELETE.
#[gpui_kit::test]
fn approvals_reads_the_tiers_and_resets_a_learned_rule(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentApprovals, cx)
        .downcast::<ApprovalsPage>()
        .expect("Approvals");
    assert_eq!(calls(&backend), [Call::Approvals, Call::Proactivity]);
    let rule = json!([{ "id": RULE, "kind": "send", "to": "Dana K.", "since": "2026-11-03T09:00:00Z" }]);
    answer(&backend, |call| *call == Call::Approvals, 200, approvals_body(rule), cx);
    page.read_with(cx, |page, _| {
        let view = page.view().expect("read");
        assert_eq!(view.expires_after_minutes, Some(30));
        assert_eq!(view.rules.len(), 1);
        // A7·b: the two voices, with the person's own names in their samples.
        assert_eq!(
            page.voices()
                .each_ref()
                .map(|voice| (voice.title, voice.sample.as_str())),
            [("As your assistant", "Sent by Nexus, A's assistant"), ("As you", "A")]
        );
    });

    page.update_in(cx, |page, window, cx| page.reset(Some(RULE.into()), window, cx));
    let path = format!("/agent/approvals/rules/{RULE}");
    assert_eq!(calls(&backend).last(), Some(&Call::Delete { path: path.clone() }));
    answer(
        &backend,
        |call| *call == Call::Delete { path: path.clone() },
        200,
        json!({ "revoked": 1 }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert!(page.view().expect("kept").rules.is_empty());
        assert_eq!(page.status(), Some("Nexus asks first again for that write."));
    });
}

/// A16: What Nexus knows draws the notes as files by kind, deletes one line of a note by editing the rest, and opens
/// the errand that taught a line.
#[gpui_kit::test]
fn what_nexus_knows_shows_files_edits_a_line_away_and_opens_its_errand(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentMemory, cx)
        .downcast::<MemoryPage>()
        .expect("What Nexus knows");
    let body = json!({ "savedModel": "auto", "controls": true, "learning": { "paused": false }, "notes": [
        { "id": "n2", "text": "- Dana is slow\n- nudge after 3 days", "createdAt": "2026-11-04T09:00:00Z", "audience": "dm", "kind": "people",
          "source": { "kind": "told", "surface": "api", "errand": { "id": "i7", "goal": "Summit Labs SOW" } } },
        { "id": "n1", "text": "Aisle seats.", "createdAt": "2026-11-03T09:00:00Z", "audience": "dm", "kind": "preferences" }
    ]});
    answer(&backend, |call| *call == Call::Memory, 200, body, cx);
    page.read_with(cx, |page, _| {
        let paths: Vec<&str> = page.files().iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["preferences.md", "errands/summit-labs-sow.md"]);
    });
    page.update(cx, |page, cx| {
        page.open_file("errands/summit-labs-sow.md", cx);
        page.pick((0, 0), LineMode::Where, cx);
    });
    cx.run_until_parked();

    page.update_in(cx, |page, window, cx| page.delete((0, 0), window, cx));
    let edited = Call::edit_note("n2", "nudge after 3 days").expect("a note's edit");
    assert_eq!(calls(&backend).last(), Some(&edited));
    let note = json!({ "note": { "id": "n2", "text": "nudge after 3 days", "createdAt": "2026-11-04T09:00:00Z", "audience": "dm", "kind": "people",
        "source": { "kind": "told", "errand": { "id": "i7", "goal": "Summit Labs SOW" } } } });
    answer(&backend, |call| *call == edited, 200, note, cx);
    page.read_with(cx, |page, _| {
        assert_eq!(page.files()[1].lines.len(), 1);
        assert_eq!(page.status(), Some("Nexus uses the new note from its next message."));
    });

    // Open the errand: Errands opens on it once its list is read.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i7".into()), window, cx)
    });
    cx.run_until_parked();
    let _errands = workspace
        .update(cx, |workspace, _| {
            workspace.native_page.as_ref().expect("a page").1.clone()
        })
        .downcast::<ErrandsPage>()
        .expect("Errands");
    let list = json!({ "errands": [{ "id": "i7", "goal": "Summit Labs SOW", "status": "running", "steps": 2, "at": "2026-11-04T09:00:00Z" }] });
    answer(&backend, |call| *call == Call::Errands, 200, list, cx);
    assert_eq!(calls(&backend).last(), Call::errand("i7").as_ref(), "the errand opens");
}

fn errand(status: &str, need: Option<&str>, actions: Value) -> Value {
    json!({
        "errand": { "id": "i3", "goal": "Dinner Friday", "status": status, "need": need, "steps": 1, "at": "2026-11-17T08:00:00Z" },
        "constraints": ["Near Divisadero"], "surface": "slack", "createdAt": "2026-11-17T08:00:00Z",
        "steps": [{ "id": "s1", "text": "Compared 6 places", "urls": ["https://nopa.test"], "at": "2026-11-17T08:10:00Z" }],
        "actions": actions
    })
}

/// A12: Errands lists the errands, opens one, and Confirm sends its action, shows what it did and reads the list again.
#[gpui_kit::test]
fn errands_opens_one_and_confirms_its_draft(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentErrands, cx)
        .downcast::<ErrandsPage>()
        .expect("Errands");
    let list = json!({ "errands": [{ "id": "i3", "goal": "Dinner Friday", "status": "needs-you", "need": "confirm", "at": "2026-11-17T08:00:00Z" }] });
    answer(&backend, |call| *call == Call::Errands, 200, list.clone(), cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_assistant(PageId::AgentErrands, Some("i3".into()), window, cx)
    });
    cx.run_until_parked();
    let page = {
        let _old = page;
        workspace
            .update(cx, |workspace, _| {
                workspace.native_page.as_ref().expect("a page").1.clone()
            })
            .downcast::<ErrandsPage>()
            .expect("Errands")
    };
    answer(&backend, |call| *call == Call::Errands, 200, list, cx);
    let read = Call::errand("i3").expect("an errand's read");
    answer(
        &backend,
        |call| *call == read,
        200,
        errand("needs-you", Some("confirm"), json!(["confirm", "cancel"])),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.errand().map(|view| view.actions.len()), Some(2))
    });

    page.update_in(cx, |page, window, cx| page.act_on(Action::Confirm, window, cx));
    let confirm = Call::act("i3", Action::Confirm).expect("an action");
    assert_eq!(calls(&backend).last(), Some(&confirm));
    answer(
        &backend,
        |call| *call == confirm,
        200,
        json!({ "state": "confirmed", "errand": errand("running", None, json!(["done", "cancel"])) }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Confirmed. Nexus works on it now."));
        assert_eq!(
            page.errand().map(|view| view.actions.clone()),
            Some(vec![Action::Done, Action::Cancel])
        );
    });
    assert_eq!(calls(&backend).last(), Some(&Call::Errands), "the list moves with it");
}

/// A12: with errands off at the agent API, the view says so instead of failing.
#[gpui_kit::test]
fn errands_off_reads_as_one_line(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let _page = open(&workspace, PageId::AgentErrands, cx);
    answer(
        &backend,
        |call| *call == Call::Errands,
        503,
        json!({ "error": "errands_off", "detail": "Off." }),
        cx,
    );
}

/// A15: Discover reads the memory and the approvals, draws each section, and Try it hands the line to the chat.
#[gpui_kit::test]
fn discover_draws_each_section_and_try_it_hands_the_line_off(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentDiscover, cx)
        .downcast::<DiscoverPage>()
        .expect("Discover");
    assert!(calls(&backend).contains(&Call::Memory) && calls(&backend).contains(&Call::Approvals));
    answer(
        &backend,
        |call| *call == Call::Memory,
        200,
        json!({ "savedModel": "auto", "controls": true, "notes": [] }),
        cx,
    );
    answer(&backend, |call| *call == Call::Approvals, 500, json!({}), cx);
    for section in [
        Section::Try,
        Section::Connect,
        Section::Autonomy,
        Section::Teach,
        Section::New,
    ] {
        page.update(cx, |page, cx| page.show(section, cx));
        cx.run_until_parked();
    }
    page.read_with(cx, |page, _| {
        assert!(page.rows().is_empty(), "the fixture offers no connector")
    });

    let line = nexus_desktop_core::agent_discover::IDEAS[0].line;
    page.update_in(cx, |page, window, cx| page.try_line(line, window, cx));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(matches!(workspace.view, View::Page(PageId::Agent)))
    });
    let asked = calls(&backend)
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::Ask { body } => Some(body),
            _ => None,
        })
        .expect("Nexus is asked");
    assert!(asked.contains(line), "{asked}");
}

/// The strip moves between the chat and each view, and Account › Assistant opens What Nexus knows here, not in the
/// browser. Without an agent API, a view is not there. A24·c draws no strip over the thread: its title bar is the
/// Assistant's, with ⋯.
#[gpui_kit::test]
fn the_strip_and_account_assistant_open_the_views_natively(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    for id in STRIP {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_assistant(id, None, window, cx)
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            assert!(matches!(workspace.view, View::Page(open) if open == id), "{id:?}");
            let thread = id == PageId::Agent;
            assert_eq!(workspace.assistant_strip(id, cx), !thread, "the strip over {id:?}");
            assert_eq!(
                workspace.assistant_titled(id, cx),
                thread,
                "the Assistant's title over {id:?}"
            );
        });
    }
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_dest(Dest::Assistant, window, cx)
    });
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(matches!(workspace.view, View::Page(PageId::AgentMemory)))
    });

    workspace.update_in(cx, |workspace, window, cx| {
        workspace.agent = false;
        assert!(!workspace.assistant_strip(PageId::AgentMemory, cx));
        workspace.open_native(PageId::AgentApprovals, window, cx);
        assert!(!matches!(workspace.view, View::Page(PageId::AgentApprovals)));
    });
}

fn privacy_body(paused: bool) -> Value {
    json!({
        "pause": { "paused": paused, "pausedAt": "2026-11-17T14:02:00Z" },
        "links": [{ "id": "l1", "agent": "slack", "label": "Slack", "createdAt": "2026-11-01T09:00:00Z" }],
        "accounts": [
            { "connector": "gmail", "account": "ann@gmail.test", "scope": "user", "reads": ["Read mail"], "writes": ["Send mail"], "never": [] },
            { "connector": "linear", "account": "Acme", "scope": "org", "reads": ["Read issues"], "writes": [], "never": ["Delete issues"] },
            { "connector": "github", "account": "ann", "scope": "user", "reads": [], "writes": [], "never": [], "error": "Gateway did not answer" }
        ]
    })
}

/// A11: Privacy reads what Nexus can see, pauses after the dialog's yes (the bar and the sidebar's row say Paused),
/// and Revoke all access names what ends, sends the confirm and reads the view again.
#[gpui_kit::test]
fn privacy_pauses_with_a_confirm_and_revokes_all_access(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_privacy::{self as words, Request};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentPrivacy, cx)
        .downcast::<PrivacyPage>()
        .expect("Privacy");
    let read = Call::Privacy(Request::Read);
    assert_eq!(calls(&backend), std::slice::from_ref(&read));
    answer(&backend, |call| *call == read, 200, privacy_body(false), cx);
    page.read_with(cx, |page, _| {
        let view = page.view().expect("read");
        assert_eq!(view.accounts.len(), 3);
        assert!(!view.pause.paused);
    });

    page.update_in(cx, |page, window, cx| page.ask(Asking::Pause, window, cx));
    cx.run_until_parked();
    page.read_with(cx, |page, _| assert_eq!(page.asking(), Some(Asking::Pause)));
    page.update_in(cx, |page, window, cx| page.set_paused(true, window, cx));
    let pause = Call::Privacy(Request::set_pause(true).expect("a pause"));
    assert_eq!(calls(&backend).last(), Some(&pause));
    answer(
        &backend,
        |call| *call == pause,
        200,
        json!({ "paused": true, "pausedAt": "2026-11-17T14:02:00Z" }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.asking(), None);
        assert_eq!(page.status(), Some(words::PAUSED_STATUS));
        assert!(page.paused_line().expect("the bar").starts_with("Paused at "));
    });
    workspace.update(cx, |workspace, _| {
        assert!(workspace.assistant.paused, "the row says Paused")
    });

    page.update_in(cx, |page, window, cx| page.ask(Asking::Revoke, window, cx));
    cx.run_until_parked();
    page.read_with(cx, |page, _| {
        let text = page.dialog_text().expect("the dialog");
        assert!(text.contains("Ends your agent link: Slack."), "{text}");
        assert!(text.contains("Gmail (ann@gmail.test) and GitHub (ann)"), "{text}");
        assert!(text.contains("Your organization's account stays connected."), "{text}");
    });
    page.update_in(cx, |page, window, cx| page.revoke(window, cx));
    let revoke = Call::Privacy(Request::revoke(true).expect("a revoke"));
    assert_eq!(calls(&backend).last(), Some(&revoke));
    let ended = json!({
        "links": { "ended": [{ "id": "l1", "agent": "slack", "label": "Slack", "createdAt": "2026-11-01T09:00:00Z" }], "failed": [] },
        "accounts": { "disconnected": [{ "connector": "gmail", "account": "ann@gmail.test" }], "failed": [] }
    });
    answer(&backend, |call| *call == revoke, 200, ended, cx);
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Ended 1 agent link and disconnected 1 account."))
    });
    assert_eq!(calls(&backend).last(), Some(&read), "the view reads again");
    answer(&backend, |call| *call == read, 200, privacy_body(true), cx);

    // Resume from the bar: the row no longer says Paused.
    page.update_in(cx, |page, window, cx| page.set_paused(false, window, cx));
    let resume = Call::Privacy(Request::set_pause(false).expect("a resume"));
    answer(&backend, |call| *call == resume, 200, json!({ "paused": false }), cx);
    workspace.update(cx, |workspace, _| assert!(!workspace.assistant.paused));
}

/// A11: an agent API without Privacy (404) says so in one line; the row reads the pause once the account is known.
#[gpui_kit::test]
fn privacy_off_reads_as_one_line_and_the_row_reads_the_pause(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_privacy::Request;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentPrivacy, cx)
        .downcast::<PrivacyPage>()
        .expect("Privacy");
    let read = Call::Privacy(Request::Read);
    answer(&backend, |call| *call == read, 404, json!({ "error": "not_found" }), cx);
    page.read_with(cx, |page, _| assert!(page.view().is_none()));

    workspace.update(cx, |workspace, cx| workspace.assistant_account(cx));
    let pause = Call::Privacy(Request::Pause);
    assert_eq!(calls(&backend).last(), Some(&pause));
    answer(&backend, |call| *call == pause, 200, json!({ "paused": true }), cx);
    workspace.update(cx, |workspace, _| assert!(workspace.assistant.paused));
}

fn vault_body() -> Value {
    json!({
        "manager": { "kind": "1password", "vault": "Maya’s Personal vault" },
        "logins": [
            { "id": "united", "name": "United", "site": "united.com", "grant": { "kind": "each_use" }, "lastUsed": "2026-11-17T07:41:00Z" },
            { "id": "marriott", "name": "Marriott Bonvoy", "site": "marriott.com", "grant": { "kind": "until", "until": "2026-11-22T12:00:00Z" }, "lastUsed": null }
        ],
        "card": { "last4": "4021", "state": "active", "spent": "142", "limit": "500", "period": "month" },
        "merchants": [
            { "id": "doordash", "merchant": "DoorDash", "amount": "60", "per": "order", "approval": "asks_first" },
            { "id": "united", "merchant": "United", "amount": null, "per": "booking", "approval": "face_id" },
            { "id": "amazon", "merchant": "Amazon", "amount": null, "per": "order", "approval": "off" }
        ],
        "applePay": true,
        "asks": [{ "id": "ask_1", "login": { "id": "united", "name": "United", "account": "MileagePlus ••• 772" }, "manager": "1password",
                   "site": "united.com", "task": "Check in for UA 1532", "until": "2026-11-22T12:00:00Z", "expiresAt": "2026-11-17T08:00:00Z" }]
    })
}

/// A8: Passwords and payments draws the logins, the card and the grant sheet; Revoke, Freeze and Approve until a date
/// each send their checked call, and the log and the Nexus vault open in the same view.
#[gpui_kit::test]
fn vault_revokes_freezes_approves_and_opens_its_log(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_vault::{self as words, For, LoginChange, Request};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentVault, cx)
        .downcast::<VaultPage>()
        .expect("Passwords and payments");
    let read = Call::Vault(Request::Read);
    // The page reads the vault, and sends this device's index (empty here: nothing is kept on it).
    assert_eq!(calls(&backend)[0], read);
    assert!(
        calls(&backend)[1..]
            .iter()
            .all(|call| matches!(call, Call::Vault(Request::Index { .. })))
    );
    answer(&backend, |call| *call == read, 200, vault_body(), cx);
    page.read_with(cx, |page, _| {
        let view = page.view().expect("read");
        assert_eq!((view.logins.len(), view.merchants.len(), view.asks.len()), (2, 3, 1));
    });

    page.update(cx, |page, cx| page.open_menu("marriott", cx));
    cx.run_until_parked();
    page.update_in(cx, |page, window, cx| page.revoke("united", window, cx));
    let revoke = Call::Vault(Request::login("united", &LoginChange::Revoke).expect("a revoke"));
    assert_eq!(calls(&backend).last(), Some(&revoke));
    let mut after = vault_body();
    after["logins"][0]["grant"] = Value::Null;
    answer(
        &backend,
        |call| *call == revoke,
        200,
        json!({ "vault": after, "stopped": 1 }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(
            page.status(),
            Some("United revoked. It stopped one errand that used it.")
        );
        assert!(page.view().expect("kept").logins[0].grant.is_none());
    });

    page.update_in(cx, |page, window, cx| page.freeze(window, cx));
    let freeze = Call::Vault(Request::card(true).expect("a freeze"));
    assert_eq!(calls(&backend).last(), Some(&freeze));
    let mut frozen = vault_body();
    frozen["card"]["state"] = json!("frozen");
    answer(&backend, |call| *call == freeze, 200, json!({ "vault": frozen }), cx);
    page.read_with(cx, |page, _| assert_eq!(page.status(), Some(words::frozen_text(true))));

    page.update(cx, |page, cx| page.choose("ask_1", For::Until, cx));
    page.update_in(cx, |page, window, cx| page.decide("ask_1", true, window, cx));
    let approve = Call::Vault(Request::decide("ask_1", Some(For::Until)).expect("an approval"));
    assert_eq!(calls(&backend).last(), Some(&approve));
    let mut decided = vault_body();
    decided["asks"] = json!([]);
    answer(
        &backend,
        |call| *call == approve,
        200,
        json!({ "vault": decided, "decided": "confirm_on_device" }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(page.status(), Some("Confirm with Face ID on your phone to finish."));
        assert!(page.view().expect("kept").asks.is_empty());
    });

    // A failed write says so and keeps the vault on screen.
    page.update_in(cx, |page, window, cx| page.freeze(window, cx));
    answer(
        &backend,
        |call| matches!(call, Call::Vault(Request::Card { .. })),
        410,
        json!({ "error": "ask_gone" }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert_eq!(
            page.status(),
            Some("That request is no longer waiting. Ask Nexus again.")
        );
        assert!(page.view().is_some());
    });

    page.update_in(cx, |page, window, cx| page.show(VaultPart::Activity, window, cx));
    let activity = Call::Vault(Request::Activity);
    assert_eq!(calls(&backend).last(), Some(&activity));
    let entries = json!({ "entries": [
        { "at": "2026-10-12T08:30:00Z", "what": "Paid $38.60 · Souvla", "with": "Nexus card ••4021", "approved": "In the app · Mac", "result": "Done", "outcome": "done" },
        { "at": "2026-10-11T18:55:00Z", "what": "Tried to pay $94 · Amazon", "with": "Nexus card ••4021", "approved": null, "result": "Refused: Amazon is off", "outcome": "refused" }
    ]});
    answer(&backend, |call| *call == activity, 200, entries, cx);
    page.read_with(cx, |page, _| {
        let log = page.log().expect("the log");
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].1, "Paid $38.60 · Souvla");
    });

    page.update_in(cx, |page, window, cx| page.show(VaultPart::Nexus, window, cx));
    cx.run_until_parked();
    page.update_in(cx, |page, window, cx| page.open_import(window, cx));
    page.update_in(cx, |page, window, cx| {
        page.import_bytes(
            b"name,url,username,password\nDelta,https://delta.com,maya,Pw-Import-Test-1\n".to_vec(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    page.read_with(cx, |page, _| {
        assert!(
            page.import_said()
                .is_some_and(|said| said.starts_with("Imported 1 logins from Chrome."))
        );
    });
    let made = calls(&backend);
    assert!(
        made.iter().all(|call| matches!(call, Call::Vault(_))),
        "every call is the vault's: {made:?}"
    );
}

/// A8: without a vault source (503 `vault_off`) the page says nothing is linked; an answer shaped like a secret shows
/// nothing at all.
#[gpui_kit::test]
fn vault_off_reads_as_nothing_linked_and_a_secret_is_never_drawn(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_vault::Request;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentVault, cx)
        .downcast::<VaultPage>()
        .expect("Passwords and payments");
    let read = Call::Vault(Request::Read);
    answer(
        &backend,
        |call| *call == read,
        503,
        json!({ "error": "vault_off", "detail": "Off." }),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert!(page.off());
        assert_eq!(page.view(), Some(&Default::default()));
    });
    page.update_in(cx, |page, window, cx| page.show(VaultPart::Nexus, window, cx));
    cx.run_until_parked();
    page.update_in(cx, |page, window, cx| page.show(VaultPart::Activity, window, cx));
    let activity = Call::Vault(Request::Activity);
    answer(
        &backend,
        |call| *call == activity,
        503,
        json!({ "error": "vault_off" }),
        cx,
    );

    let page = open(&workspace, PageId::AgentVault, cx)
        .downcast::<VaultPage>()
        .expect("Passwords and payments");
    let mut leaked = vault_body();
    leaked["logins"][0]["password"] = json!("hunter2");
    answer(&backend, |call| *call == read, 200, leaked, cx);
    page.read_with(cx, |page, _| assert!(page.view().is_none(), "nothing of it is kept"));
}

/// A8·b over the chat: a login Nexus asks to use waits above the thread, as the web's `AgentVaultAsks`.
/// Approve sends the page's own call, the sheet goes and its line stays. The vault is read again after each answer,
/// and with the vault off nothing shows.
#[gpui_kit::test]
fn the_grant_sheet_waits_over_the_chat_and_decides_there(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_vault::{For, Request};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant_kept_page().expect("kept")
    });
    cx.run_until_parked();
    let vault = page.read_with(cx, |page, _| page.vault());
    let read = Call::Vault(Request::Read);
    answer(&backend, |call| *call == read, 200, vault_body(), cx);
    assert_eq!(vault.read_with(cx, |vault, _| vault.sheets()), ["United"]);

    vault.update_in(cx, |vault, window, cx| vault.decide("ask_1", true, window, cx));
    let approve = Call::Vault(Request::decide("ask_1", Some(For::Once)).expect("an approval"));
    assert_eq!(calls(&backend).last(), Some(&approve));
    let mut decided = vault_body();
    decided["asks"] = json!([]);
    answer(
        &backend,
        |call| *call == approve,
        200,
        json!({ "vault": decided, "decided": "confirm_on_device" }),
        cx,
    );
    vault.read_with(cx, |vault, _| {
        assert!(vault.sheets().is_empty());
        assert_eq!(vault.status(), Some("Confirm with Face ID on your phone to finish."));
    });

    // An answer ends: the vault is read again, once. Off at the agent API, nothing shows over the chat.
    let reads = || calls(&backend).iter().filter(|call| **call == read).count();
    let before = reads();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.hand_off("check me in".into(), window, cx)
    });
    cx.run_until_parked();
    let ask = {
        let calls = backend.agent_calls.lock().expect("agent calls");
        let (_, sender) = calls
            .iter()
            .rev()
            .find(|(call, _)| matches!(call, Call::Ask { .. }))
            .expect("an ask");
        sender.clone()
    };
    for chunk in [
        Chunk::Head {
            status: 200,
            content_type: "text/event-stream".into(),
        },
        Chunk::Data(
            b"event: done\ndata: {\"status\":200,\"conversation\":\"c1\",\"messages\":[{\"kind\":\"answer\",\"text\":\"On it.\"}]}\n\n"
                .to_vec(),
        ),
        Chunk::End,
    ] {
        ask.unbounded_send(chunk).expect("the page reads its answer");
    }
    cx.run_until_parked();
    assert_eq!(reads(), before + 1);
    answer(&backend, |call| *call == read, 503, json!({ "error": "vault_off" }), cx);
    assert!(vault.read_with(cx, |vault, _| vault.sheets()).is_empty());
}

/// A device for the next Passwords page, with `auth` for Touch ID. Its Keychain is in memory.
fn vault_device(
    auth: std::sync::Arc<dyn nexus_desktop_core::vault_local::Authenticator>,
) -> std::sync::Arc<crate::vault_device::Device> {
    let device = std::sync::Arc::new(crate::vault_device::tests::device(auth, None));
    crate::vault_device::tests::NEXT.with(|next| *next.borrow_mut() = Some(device.clone()));
    device
}

/// The bodies of every index sent so far.
fn index_bodies(backend: &BackendProbe) -> Vec<String> {
    calls(backend)
        .into_iter()
        .filter_map(|call| match call {
            Call::Vault(nexus_desktop_core::agent_vault::Request::Index { body }) => Some(body),
            _ => None,
        })
        .collect()
}

/// The Nexus vault adds, imports and deletes logins in the Keychain, and each index the page sends holds no
/// password and no username, nor does any call it makes.
#[gpui_kit::test]
fn the_vault_keeps_logins_on_the_device_and_sends_only_an_index(cx: &mut TestAppContext) {
    use nexus_desktop_core::agent_vault::Request;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let device = vault_device(std::sync::Arc::new(crate::vault_device::tests::Approve));
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentVault, cx)
        .downcast::<VaultPage>()
        .expect("Passwords and payments");
    let read = Call::Vault(Request::Read);
    answer(&backend, |call| *call == read, 200, vault_body(), cx);
    // Opening sends the device's index: nothing in it yet.
    assert_eq!(index_bodies(&backend).len(), 1);
    assert!(index_bodies(&backend)[0].contains("\"logins\":[]"));

    page.update_in(cx, |page, window, cx| page.open_login(None, window, cx));
    page.update_in(cx, |page, window, cx| {
        page.fill(
            "Delta",
            "https://www.delta.com/login",
            "maya@mail.com",
            "Pw-Typed-Secret-77",
            window,
            cx,
        );
        page.save_login(window, cx);
    });
    cx.run_until_parked();
    page.update_in(cx, |page, window, cx| {
        page.import_bytes(
            b"name,url,username,password\nBank,https://bank.example,sam,Pw-Csv-Secret-88\nDelta,https://delta.com,maya@mail.com,x\n".to_vec(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    let kept = page.read_with(cx, |page, _| page.local_logins());
    assert_eq!(
        kept.iter().map(|login| login.site.as_str()).collect::<Vec<_>>(),
        ["delta.com", "bank.example"]
    );
    assert!(kept.iter().all(|login| !login.granted()), "an import allows nothing");
    // The passwords are in the Keychain item and nowhere in a call.
    let everything = format!("{:?}", calls(&backend));
    for secret in ["Pw-Typed-Secret-77", "Pw-Csv-Secret-88", "maya@mail.com", "sam"] {
        assert!(!everything.contains(secret), "{secret} crossed to the network");
    }
    let last = index_bodies(&backend).pop().expect("an index");
    assert!(last.contains("delta.com") && last.contains("bank.example") && last.contains("\"account\":\"m•••\""));
    assert!(Request::index(last).is_some());
    assert_eq!(device.vault.secrets().unwrap().len(), 2);

    // Allow one login: the grant is kept here and sent in the next index.
    let id = kept[0].id.clone();
    page.update_in(cx, |page, window, cx| page.allow(&id, true, window, cx));
    cx.run_until_parked();
    assert!(
        index_bodies(&backend)
            .pop()
            .expect("an index")
            .contains("\"grant\":{\"kind\":\"each_use\"}")
    );

    // Delete all: the Keychain item goes and the next index is empty.
    page.update_in(cx, |page, window, cx| page.delete_all(window, cx));
    cx.run_until_parked();
    assert!(device.vault.list().unwrap().is_empty());
    assert!(
        index_bodies(&backend)
            .pop()
            .expect("an index")
            .contains("\"sources\":[]")
    );
}

/// A grant for a login on this device needs Touch ID. Declined, nothing is sent. Approved, Nexus is told, and
/// the use ends at "ready on this device".
#[gpui_kit::test]
fn a_grant_for_a_login_on_this_device_needs_touch_id(cx: &mut TestAppContext) {
    use crate::vault_device::tests::{Approve, Decline};
    use nexus_desktop_core::agent_vault::{For, Request};
    use nexus_desktop_core::vault_local::{NewLogin, Secret};
    for approves in [false, true] {
        let _agents = crate::code_run::fixture_agents(vec![]);
        let decline = std::sync::Arc::new(Decline::default());
        let device = if approves {
            vault_device(std::sync::Arc::new(Approve))
        } else {
            vault_device(decline.clone())
        };
        let added = device
            .vault
            .add(NewLogin {
                name: "Delta".into(),
                site: "delta.com".into(),
                username: "maya".into(),
                password: Secret::new("Pw-Never-Typed-9"),
            })
            .unwrap();
        let backend = BackendProbe::new();
        let (workspace, cx) = fixture(cx, &backend);
        let page = open(&workspace, PageId::AgentVault, cx)
            .downcast::<VaultPage>()
            .expect("Passwords and payments");
        let mut body = vault_body();
        body["asks"][0]["login"] = json!({ "id": added.id, "name": "Delta", "account": "m•••" });
        body["asks"][0]["site"] = json!("delta.com");
        body["asks"][0]["manager"] = json!("nexus");
        let read = Call::Vault(Request::Read);
        answer(&backend, |call| *call == read, 200, body.clone(), cx);
        page.update_in(cx, |page, window, cx| page.decide("ask_1", true, window, cx));
        cx.run_until_parked();
        let approve = Call::Vault(Request::decide("ask_1", Some(For::Once)).expect("an approval"));
        if !approves {
            assert!(!calls(&backend).contains(&approve), "no Touch ID, no approval sent");
            assert_eq!(*decline.0.lock().unwrap(), 1);
            page.read_with(cx, |page, _| {
                assert!(
                    page.status().is_some_and(|status| status.starts_with("Not approved.")),
                    "{:?}",
                    page.status()
                );
            });
            assert!(device.vault.state_of(&added.id).unwrap().1.is_none());
            continue;
        }
        assert!(calls(&backend).contains(&approve));
        let mut decided = body;
        decided["asks"] = json!([]);
        answer(
            &backend,
            |call| *call == approve,
            200,
            json!({ "vault": decided, "decided": "approved" }),
            cx,
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.status().is_some_and(
                |status| status.contains("ready on this device") && status.contains("nothing was filled in")
            ));
        });
        assert!(
            device.vault.state_of(&added.id).unwrap().1.is_some(),
            "the use is noted"
        );
        assert_eq!(device.vault.activity().unwrap()[0].result, "Ready on this device");
        let last = index_bodies(&backend).pop().expect("an index");
        assert!(!last.contains("Pw-Never-Typed-9") && !last.contains("\"lastUsed\":null"));
    }
}

/// Touch ID is not an approval. When Nexus answers "declined", or the ask expired (410), or the call failed,
/// the device keeps no grant, no last use and no "Approved" line, and the next index carries no grant.
#[gpui_kit::test]
fn a_declined_or_expired_ask_after_touch_id_leaves_the_device_with_no_grant(cx: &mut TestAppContext) {
    use crate::vault_device::tests::Approve;
    use nexus_desktop_core::agent_vault::{For, Request};
    use nexus_desktop_core::vault_local::{NewLogin, Secret};
    // The answer Nexus gives, and its status.
    for (status, answer_body) in [
        (200, Some("declined")),
        (200, Some("confirm_on_device")),
        (410, None),
        (500, None),
    ] {
        let _agents = crate::code_run::fixture_agents(vec![]);
        let device = vault_device(std::sync::Arc::new(Approve));
        let added = device
            .vault
            .add(NewLogin {
                name: "Delta".into(),
                site: "delta.com".into(),
                username: "maya".into(),
                password: Secret::new("Pw-Declined-Test-3"),
            })
            .unwrap();
        let backend = BackendProbe::new();
        let (workspace, cx) = fixture(cx, &backend);
        let page = open(&workspace, PageId::AgentVault, cx)
            .downcast::<VaultPage>()
            .expect("Passwords and payments");
        let mut body = vault_body();
        body["asks"][0]["login"] = json!({ "id": added.id, "name": "Delta", "account": "m•••" });
        body["asks"][0]["site"] = json!("delta.com");
        body["asks"][0]["manager"] = json!("nexus");
        let read = Call::Vault(Request::Read);
        answer(&backend, |call| *call == read, 200, body.clone(), cx);
        page.update(cx, |page, cx| page.choose("ask_1", For::Until, cx));
        page.update_in(cx, |page, window, cx| page.decide("ask_1", true, window, cx));
        cx.run_until_parked();
        let approve = Call::Vault(Request::decide("ask_1", Some(For::Until)).expect("an approval"));
        assert!(calls(&backend).contains(&approve), "Touch ID passed, so Nexus is asked");
        let reply = match answer_body {
            Some(decided) => {
                let mut view = body.clone();
                view["asks"] = json!([]);
                json!({ "vault": view, "decided": decided })
            }
            None => json!({ "error": "ask_gone" }),
        };
        answer(&backend, |call| *call == approve, status, reply, cx);
        cx.run_until_parked();
        let (grant, used) = device.vault.state_of(&added.id).unwrap();
        assert!(
            grant.is_none() && used.is_none(),
            "{status} {answer_body:?}: no grant, no use"
        );
        assert!(
            !device
                .vault
                .activity()
                .unwrap()
                .iter()
                .any(|line| line.what.starts_with("Approved") || line.result.contains("Ready")),
            "{status} {answer_body:?}: no approval line"
        );
        for sent in index_bodies(&backend) {
            assert!(sent.contains("\"grant\":null") && !sent.contains("\"until\""), "{sent}");
        }
        page.read_with(cx, |page, _| {
            assert!(
                !page.status().unwrap_or("").contains("ready on this device"),
                "{:?}",
                page.status()
            );
        });
    }
}

/// Show password asks Touch ID first. Declined shows nothing; approved shows it, then Hide takes it away.
#[gpui_kit::test]
fn show_password_asks_touch_id_and_hides_again(cx: &mut TestAppContext) {
    use crate::vault_device::tests::{Approve, Decline};
    use nexus_desktop_core::vault_local::{NewLogin, Secret};
    for approves in [false, true] {
        let _agents = crate::code_run::fixture_agents(vec![]);
        let device = if approves {
            vault_device(std::sync::Arc::new(Approve))
        } else {
            vault_device(std::sync::Arc::new(Decline::default()))
        };
        let added = device
            .vault
            .add(NewLogin {
                name: "Delta".into(),
                site: "delta.com".into(),
                username: "maya".into(),
                password: Secret::new("Pw-Shown-Test-5"),
            })
            .unwrap();
        let backend = BackendProbe::new();
        let (workspace, cx) = fixture(cx, &backend);
        let page = open(&workspace, PageId::AgentVault, cx)
            .downcast::<VaultPage>()
            .expect("Passwords and payments");
        cx.run_until_parked();
        page.update_in(cx, |page, window, cx| page.reveal(&added.id, window, cx));
        cx.run_until_parked();
        let shown = page.read_with(cx, |page, _| page.shown());
        assert_eq!(shown.as_deref(), approves.then_some("Pw-Shown-Test-5"));
        page.update(cx, |page, cx| page.hide(cx));
        assert!(page.read_with(cx, |page, _| page.shown()).is_none());
        assert!(!format!("{:?}", calls(&backend)).contains("Pw-Shown-Test-5"));
    }
}

/// Settings search finds Passwords and payments and Privacy under Account › Assistant.
#[test]
fn settings_search_finds_the_assistants_vault_and_privacy() {
    use super::super::settings_registry::Visibility;
    use super::super::settings_search::search;
    let visibility = Visibility::all().with_assistant(true);
    for (query, anchor) in [("passwords", "vault"), ("revoke all", "privacy")] {
        let hits = search(query, &visibility);
        assert!(
            hits.iter()
                .any(|hit| hit.dest == Dest::Assistant && hit.anchor == Some(anchor)),
            "{query}"
        );
    }
}
