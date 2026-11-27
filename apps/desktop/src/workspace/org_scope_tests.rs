//! Every way out of an organization forgets what the window read for it (`org_scope.rs`).
use super::gateway_chat::tests::{Probe, account, ask, fixture, named};
use super::org_menu::Org;
use crate::code_run::{AgentAccess, CodeEvent};
use crate::gateway::Event;
use crate::model::SessionSummary;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

/// Org A's reads, as a window holds them after a while in it.
fn read_org_a(workspace: &Entity<super::Workspace>, cx: &mut VisualTestContext) {
    workspace.update(cx, |workspace, _| {
        workspace.sessions = vec![SessionSummary {
            id: "a-chat".into(),
            title: "A's chat".into(),
            day: None,
            at_ms: None,
        }];
        workspace.sessions_loaded = true;
        workspace.archived_chats = Some(Ok(vec![]));
        workspace.free_start_max = Some(5.0);
        workspace.inbox = 3;
        workspace.palette_searching = true;
        workspace.had_account = true;
    });
}

/// What Gateway sends every window and every chat side (`Gateway::send_all`): the window reads it apart from its chats.
fn deliver(workspace: &Entity<super::Workspace>, cx: &mut VisualTestContext, probe: &Probe, side: u64, event: Event) {
    probe.emit(side, event.clone());
    workspace.update_in(cx, |workspace, window, cx| workspace.handle(event, window, cx));
    cx.run_until_parked();
}

/// A runner for the window to hold, so an account read does not start the app's own: that one runs the process-wide
/// pull request watch, which reaches every later window test from the `nexus-code` thread (`CodeRunner` `pull_watch`).
fn hold_test_runner(
    workspace: &Entity<super::Workspace>,
    cx: &mut VisualTestContext,
    scope: &str,
) -> std::path::PathBuf {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let data = std::env::temp_dir().join(format!("ngs-org-scope-{}", &id[..8]));
    std::fs::create_dir_all(&data).expect("a temporary folder");
    let (runner, _events) = crate::code_run::test_runner(data.clone(), scope);
    workspace.update(cx, |workspace, _| workspace.runner = Some(std::rc::Rc::new(runner)));
    data
}

fn forgot_org_a(workspace: &Entity<super::Workspace>, cx: &mut VisualTestContext) {
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.sessions.is_empty(), "org A's chats");
        assert!(!workspace.sessions_loaded);
        assert!(workspace.archived_chats.is_none(), "org A's archived chats");
        assert_eq!(workspace.free_start_max, None, "org A's spend cap");
        assert_eq!(workspace.inbox, 0, "org A's inbox count");
        assert!(!workspace.palette_searching, "a search for org A");
    });
}

/// Another window switched organization: the sign-in moves under this one, and org A's lists go with it.
#[gpui_kit::test]
fn a_sign_in_over_an_account_forgets_the_old_organization(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    read_org_a(&workspace, cx);
    deliver(&workspace, cx, &probe, a.side, Event::SignedIn);
    forgot_org_a(&workspace, cx);
    assert!(workspace.read_with(cx, |workspace, _| workspace.account.is_none()));
    assert!(probe.stops().contains(&(a.side, a.turn)), "org A's answer stops");
}

/// The switcher in this window: it used to drop the account first, so the next account read saw no switch and left
/// org A's chats running.
#[gpui_kit::test]
fn the_switcher_stops_the_old_organizations_chats(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    read_org_a(&workspace, cx);
    let org_b = Org {
        id: "org-b".into(),
        slug: Some("org-b".into()),
        name: "B".into(),
        personal: false,
        current: false,
        logo: None,
        role: None,
        provider_org_id: None,
    };
    workspace.update_in(cx, |workspace, window, cx| workspace.switch_org(&org_b, window, cx));
    cx.run_until_parked();
    forgot_org_a(&workspace, cx);
    assert!(probe.stops().contains(&(a.side, a.turn)), "org A's answer stops");
}

/// An account read that names org B with no sign-in before it: the open org A chat closes, because a message sent in
/// it would go to org A's session.
#[gpui_kit::test]
fn an_account_of_another_organization_closes_the_old_chat(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let a = ask(&workspace, cx, &probe, "A");
    named(&probe, &a, "a");
    cx.run_until_parked();
    read_org_a(&workspace, cx);
    let _data = hold_test_runner(&workspace, cx, "http://127.0.0.1:9|org-a");
    assert_eq!(
        workspace
            .read_with(cx, |workspace, _| workspace.current.clone())
            .as_deref(),
        Some("a")
    );
    deliver(
        &workspace,
        cx,
        &probe,
        a.side,
        Event::Account(account("org-b", "person-a")),
    );
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.current, None, "org A's chat is not open in org B");
        assert_eq!(workspace.free_start_max, None);
        assert!(workspace.archived_chats.is_none());
        assert_eq!(
            workspace.account.as_ref().map(|account| account.org.as_str()),
            Some("org-b")
        );
    });
}

/// Who may pay for a new session is read with a credential: an answer for an earlier one is dropped.
#[gpui_kit::test]
fn code_access_of_another_sign_in_is_dropped(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let (workspace, cx) = fixture(cx, &probe);
    let access = || AgentAccess {
        subscriptions: true,
        answer: None,
    };
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.on_code_events(
            vec![CodeEvent::Access {
                credential: Some("fixture-identity-old".into()),
                access: access(),
            }],
            window,
            cx,
        );
        assert!(workspace.code_form.access.is_none(), "org A's answer");
        workspace.on_code_events(
            vec![CodeEvent::Access {
                credential: Some("fixture-identity-a".into()),
                access: access(),
            }],
            window,
            cx,
        );
        assert!(workspace.code_form.access.is_some(), "the current sign-in's answer");
    });
}
