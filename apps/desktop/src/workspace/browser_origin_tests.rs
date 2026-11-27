//! Real registered-window browser lookup uses authenticated store provenance, not the front window.
use super::Workspace;
use super::gateway_chat::tests::{Probe, fixture};
use crate::side::browser::agent::LocalOrigin;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use nexus_workbench::store::{Store, ThreadRecord};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const THREAD: &str = "same-browser-session";
struct SourceStore {
    data: PathBuf,
    scope: String,
}
impl SourceStore {
    fn new(scope: &str) -> Self {
        let data = std::env::temp_dir().join(format!("nexus-browser-origin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data).expect("isolated local store folder");
        let store = Store::open(&data.join("workbench.sqlite3")).expect("real local workbench store");
        let now = nexus_workbench::now_ms();
        let project = store.add_project(&data, now).expect("real stored project");
        let summary = serde_json::from_value(json!({
            "id": THREAD, "projectId": project.id, "title": "Original browser session",
            "agent": "claude", "model": null, "permissionMode": "default", "via": "nexus",
            "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": now, "updatedAt": now, "archived": false
        }))
        .expect("production thread summary wire shape");
        store
            .insert_thread(&ThreadRecord {
                summary,
                native_id: "native-browser-session".into(),
                native_started: false,
                scope: scope.into(),
                fork_from: None,
                seed: None,
                automatic_title: false,
                delivered_seq: 0,
                halt: None,
            })
            .expect("stored scoped thread");
        Self {
            data,
            scope: scope.into(),
        }
    }
    fn origin(&self) -> LocalOrigin {
        LocalOrigin {
            data: self.data.clone(),
            scope: self.scope.clone(),
            machine: None,
        }
    }
}
impl Drop for SourceStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data);
    }
}
fn register(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    probe: &Probe,
    source: &SourceStore,
) -> gpui_kit::AnyWindowHandle {
    let handle = workspace.update_in(cx, |workspace, window, _| {
        workspace.code = crate::code::CodeStore::open_in(&source.data, source.scope.clone()).expect("reader opens");
        assert!(
            workspace
                .code
                .as_ref()
                .expect("concrete reader")
                .summary(THREAD)
                .expect("scoped lookup")
                .is_some()
        );
        window.window_handle()
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), handle, workspace.clone(), false, cx)
    });
    handle
}
fn valid() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| true)
}

#[gpui_kit::test]
fn source_lookup_creates_in_matching_background_window_instead_of_wrong_front(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&format!("{}|org-a", probe.url));
    let b = SourceStore::new(&a.scope);
    let (first, cx) = fixture(cx, &probe);
    let first_window = register(&first, cx, &probe, &a);
    let (front, cx) = fixture(cx, &probe);
    let front_window = register(&front, cx, &probe, &b);
    assert_ne!(first_window.window_id(), front_window.window_id());
    let (routed, _, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&a.origin(), THREAD, &valid(), cx))
        .expect("supported matching background store");
    assert_eq!(routed.window_id(), first_window.window_id());
    first.read_with(cx, |workspace, _| assert!(workspace.has_browser(THREAD)));
    front.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn two_matching_views_reuse_existing_browser_even_when_other_matching_view_takes_focus(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (first, cx) = fixture(cx, &probe);
    let first_window = register(&first, cx, &probe, &source);
    let (second, cx) = fixture(cx, &probe);
    let second_window = register(&second, cx, &probe, &source);
    let (selected, browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("two matching views are supported");
    assert_eq!(selected.window_id(), second_window.window_id());
    cx.cx.update(|cx| crate::windows::focused(cx, first_window.window_id()));
    let (selected_again, browser_again, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("existing exact browser stays canonical");
    assert_eq!(selected_again.window_id(), second_window.window_id());
    assert_eq!(browser.entity_id(), browser_again.entity_id());
    first.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn wrong_scope_missing_thread_or_queued_revocation_never_creates_a_browser(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    let wrong = LocalOrigin {
        data: source.data.clone(),
        scope: "another-gateway|another-org".into(),
        machine: None,
    };
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&wrong, THREAD, &valid(), cx))
            .is_err()
    );
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&source.origin(), "missing-session", &valid(), cx))
            .is_err()
    );
    let checks = Arc::new(AtomicU64::new(0));
    let queued_valid: Arc<dyn Fn() -> bool + Send + Sync> = {
        let checks = checks.clone();
        Arc::new(move || checks.fetch_add(1, Ordering::SeqCst) == 0)
    };
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &queued_valid, cx))
            .is_err()
    );
    assert!(
        checks.load(Ordering::SeqCst) >= 2,
        "eventual window update rechecks the original live guard"
    );
    workspace.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn parked_same_thread_from_another_store_does_not_block_supported_active_store(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&format!("{}|org-a", probe.url));
    let b = SourceStore::new(&a.scope);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &a);
    cx.cx
        .update(|cx| crate::windows::agent_browser(&a.origin(), THREAD, &valid(), cx))
        .expect("primary browser created");
    let (other, cx) = fixture(cx, &probe);
    let other_window = register(&other, cx, &probe, &b);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&a.origin(), THREAD, &valid(), cx))
            .is_err()
    );
    let (selected, _, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&b.origin(), THREAD, &valid(), cx))
        .expect("another local store's same thread is independent of parked source");
    assert_eq!(selected.window_id(), other_window.window_id());
    other.read_with(cx, |workspace, _| assert!(workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn account_epoch_aba_refuses_creation_before_any_signed_in_event_is_consumed(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    probe.epoch.fetch_add(2, Ordering::SeqCst); // Same final credential marker; captured Account generation is stale.
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
            .is_err()
    );
    workspace.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
}

#[gpui_kit::test]
fn parked_primary_does_not_hide_an_existing_same_source_secondary_browser(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    cx.cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("primary browser");
    let (secondary, cx) = fixture(cx, &probe);
    let secondary_window = register(&secondary, cx, &probe, &source);
    // Existing UI flows permit two views of this exact source/session. Exercise the production creation path.
    let browser = secondary
        .update_in(cx, |workspace, window, cx| workspace.agent_browser(THREAD, window, cx))
        .expect("secondary's existing browser");
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let (selected, selected_browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("already-live matching browser remains supported");
    assert_eq!(selected.window_id(), secondary_window.window_id());
    assert_eq!(selected_browser.entity_id(), browser.entity_id());
}

const SERVER_THREAD: &str = "server-browser-session";

/// Session SERVER_THREAD on SSH server `m_server`, which this window's machines know.
fn server_session(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    let now = nexus_workbench::now_ms();
    let summary = serde_json::from_value(json!({
        "id": SERVER_THREAD, "projectId": "p_server", "title": "Server browser session",
        "agent": "claude", "model": null, "permissionMode": "default", "via": "nexus",
        "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
        "contextWindow": null, "createdAt": now, "updatedAt": now, "archived": false
    }))
    .expect("production thread summary wire shape");
    workspace.read_with(cx, |workspace, _| {
        workspace.machine_source.fixture_server_session("m_server", summary)
    });
}

#[gpui_kit::test]
fn a_server_sessions_call_opens_its_browser_in_a_window_of_its_organization(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    server_session(&workspace, cx);
    let server = LocalOrigin {
        machine: Some("m_server".into()),
        ..source.origin()
    };
    // The same call labelled as this computer's store finds no session: the machine is part of the origin.
    assert_eq!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&source.origin(), SERVER_THREAD, &valid(), cx))
            .err(),
        Some(super::side_pane::BROWSER_NO_SESSION.to_string())
    );
    let (_, browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&server, SERVER_THREAD, &valid(), cx))
        .expect("the server's session is admitted");
    workspace.read_with(cx, |workspace, _| assert!(workspace.has_browser(SERVER_THREAD)));
    // The browser's permission owner is the server's source: its requests stay that session's.
    let reused = workspace
        .update_in(cx, |workspace, window, cx| {
            workspace.agent_browser(SERVER_THREAD, window, cx)
        })
        .expect("the same browser again");
    assert_eq!(browser.entity_id(), reused.entity_id());
    // Another server, or another organization, never opens it.
    let other_server = LocalOrigin {
        machine: Some("m_other".into()),
        ..source.origin()
    };
    assert_eq!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&other_server, SERVER_THREAD, &valid(), cx))
            .err(),
        Some(super::side_pane::BROWSER_NO_SERVER_SESSION.to_string())
    );
    let other_org = LocalOrigin {
        scope: "another-gateway|another-org".into(),
        ..server.clone()
    };
    assert_eq!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&other_org, SERVER_THREAD, &valid(), cx))
            .err(),
        Some(super::side_pane::BROWSER_OTHER_ORG.to_string())
    );
    // A server's origin never takes a session of this computer's store with the same id.
    let local_as_server = LocalOrigin {
        machine: Some("m_server".into()),
        ..source.origin()
    };
    assert!(
        cx.cx
            .update(|cx| crate::windows::agent_browser(&local_as_server, THREAD, &valid(), cx))
            .is_err()
    );
}

fn browser_for(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<crate::side::browser::Browser> {
    workspace
        .update_in(cx, |workspace, window, cx| workspace.agent_browser(THREAD, window, cx))
        .expect("real source owns this browser")
}
fn read_request(
    browser: &Entity<crate::side::browser::Browser>,
    guard: crate::side::browser::agent::Guard,
    cx: &mut VisualTestContext,
) -> gpui_kit::Task<Result<serde_json::Value, String>> {
    let tab = browser.update_in(cx, |browser, _, _| browser.fixture_untrusted_tab());
    browser.update_in(cx, |browser, window, cx| {
        browser.agent(
            "browser_read".into(),
            json!({"tab":tab}),
            "same-run".into(),
            guard,
            window,
            cx,
        )
    })
}
fn poll_permissions(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(std::time::Duration::from_millis(60));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn same_thread_in_two_stores_cannot_decline_or_always_grant_the_other_request(cx: &mut TestAppContext) {
    use crate::side::browser::agent::{Answer, REQUESTS};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let a = SourceStore::new(&format!("{}|org-a", probe.url));
    let b = SourceStore::new(&a.scope);
    let (first, cx) = fixture(cx, &probe);
    register(&first, cx, &probe, &a);
    let (_, browser_a, guard_a) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&a.origin(), THREAD, &valid(), cx))
        .expect("A admitted");
    let (second, cx) = fixture(cx, &probe);
    register(&second, cx, &probe, &b);
    let (_, browser_b, guard_b) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&b.origin(), THREAD, &valid(), cx))
        .expect("B admitted");
    let call_a = read_request(&browser_a, guard_a, cx);
    cx.run_until_parked();
    second.update_in(cx, |workspace, _, cx| {
        let palette = cx.global::<crate::pages::Theme>().0;
        assert!(
            workspace.render_browser_permission(THREAD, palette, cx).is_none(),
            "wrong-source conversation has no permission card"
        );
    });
    let call_b = read_request(&browser_b, guard_b, cx);
    cx.run_until_parked();
    let request_a = browser_a
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("actual A request");
    let request_b = browser_b
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("actual B request");
    assert_eq!(request_a.thread_id, THREAD);
    assert_eq!(request_b.thread_id, THREAD);
    assert_ne!(request_a.id, request_b.id);
    // A wrong-source card ID cannot consume A or authorize B, even though both real session/run IDs match.
    browser_b.update_in(cx, |browser, _, cx| {
        browser.answer_request_id(&request_a.id, Answer::Decline, cx);
        browser.answer_request_id(&request_a.id, Answer::Always, cx);
        assert!(browser.tabs().0.iter().all(|tab| !tab.agent_allowed));
    });
    assert!(REQUESTS.contains(&request_a.id) && REQUESTS.contains(&request_b.id));
    browser_a.update_in(cx, |browser, _, cx| {
        browser.answer_request_id(&request_a.id, Answer::Decline, cx)
    });
    browser_b.update_in(cx, |browser, _, cx| {
        browser.answer_request_id(&request_b.id, Answer::Always, cx)
    });
    poll_permissions(cx);
    let error_a = futures::FutureExt::now_or_never(call_a)
        .expect("A decline consumed")
        .expect_err("A refused");
    assert_eq!(error_a, nexus_browser::access::DECLINED);
    // No CEF process is started: the actual read proceeds past permission and encounters the intentionally absent page.
    let error_b = futures::FutureExt::now_or_never(call_b)
        .expect("B grant consumed")
        .expect_err("fixture has no page");
    assert_eq!(error_b, "This browser tab is not open yet.");
    browser_a.read_with(cx, |browser, _| {
        assert!(browser.tabs().0.iter().all(|tab| !tab.agent_allowed))
    });
    browser_b.read_with(cx, |browser, _| {
        assert!(browser.tabs().0.iter().all(|tab| tab.agent_allowed))
    });
    assert!(!REQUESTS.contains(&request_a.id) && !REQUESTS.contains(&request_b.id));
}

#[gpui_kit::test]
fn matching_source_second_view_can_answer_permission_for_original_browser(cx: &mut TestAppContext) {
    use crate::side::browser::agent::Answer;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (first, cx) = fixture(cx, &probe);
    register(&first, cx, &probe, &source);
    let (_, original, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("original admitted");
    let (second, cx) = fixture(cx, &probe);
    register(&second, cx, &probe, &source);
    let matching = browser_for(&second, cx);
    matching.update_in(cx, |browser, _, _| {
        browser.fixture_untrusted_tab();
    });
    let call = read_request(&original, guard, cx);
    cx.run_until_parked();
    let request = original
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("actual request");
    let other = matching
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("same-source permission visible");
    assert_eq!(request.id, other.id);
    assert_eq!(other.thread_id, THREAD);
    matching.update_in(cx, |browser, _, cx| {
        browser.answer_request_id(&request.id, Answer::Always, cx)
    });
    poll_permissions(cx);
    assert_eq!(
        futures::FutureExt::now_or_never(call)
            .expect("matching approval consumed")
            .expect_err("no engine fixture"),
        "This browser tab is not open yet."
    );
    original.read_with(cx, |browser, _| {
        assert!(browser.tabs().0.iter().all(|tab| tab.agent_allowed))
    });
    matching.read_with(cx, |browser, _| {
        assert!(
            browser.tabs().0.iter().all(|tab| !tab.agent_allowed),
            "approval applies to requester, not another view"
        )
    });
}

#[gpui_kit::test]
fn admitted_account_aba_before_task_start_has_no_permission_or_tab_effect(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    let (_, browser, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("admitted before ABA");
    let events = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = events.clone();
    let _subscription = workspace.update_in(cx, |_, _, cx| {
        cx.subscribe(&browser, move |_, _, event, _| {
            if matches!(event, crate::side::browser::BrowserEvent::Permission) {
                count.set(count.get() + 1);
            }
        })
    });
    let call = read_request(&browser, guard.clone(), cx);
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    assert!(valid()(), "producer fingerprint still matches after ABA");
    assert!(!guard(), "captured Account proof cannot be recaptured");
    cx.run_until_parked();
    assert_eq!(
        futures::FutureExt::now_or_never(call)
            .expect("revoked task answered")
            .expect_err("revoked"),
        "This browser tool credential is invalid or expired."
    );
    browser.read_with(cx, |browser, _| {
        assert!(browser.permission_request().is_none());
        assert_eq!(browser.tabs().0.len(), 1);
        assert!(!browser.tabs().0[0].agent_allowed);
    });
    assert_eq!(
        events.get(),
        0,
        "no permission notification or action publication before act"
    );
}

#[gpui_kit::test]
fn waiting_permission_is_removed_on_account_revocation_without_grant(cx: &mut TestAppContext) {
    use crate::side::browser::agent::{Answer, REQUESTS};
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    let (_, browser, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("admitted");
    let call = read_request(&browser, guard, cx);
    cx.run_until_parked();
    let request = browser
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("actual waiting request");
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    browser.update_in(cx, |browser, _, cx| {
        browser.answer_request_id(&request.id, Answer::Always, cx)
    });
    assert!(
        !REQUESTS.contains(&request.id),
        "revoked permission cannot remain pending"
    );
    poll_permissions(cx);
    assert!(
        futures::FutureExt::now_or_never(call)
            .expect("revocation consumed")
            .is_err()
    );
    browser.read_with(cx, |browser, _| {
        assert!(browser.tabs().0.iter().all(|tab| !tab.agent_allowed))
    });
}

#[gpui_kit::test]
fn canceled_permission_task_releases_request_while_browser_is_still_alive(cx: &mut TestAppContext) {
    use crate::side::browser::agent::REQUESTS;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let (workspace, cx) = fixture(cx, &probe);
    register(&workspace, cx, &probe, &source);
    let (_, browser, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("admitted");
    let call = read_request(&browser, guard, cx);
    cx.run_until_parked();
    let request = browser
        .read_with(cx, |browser, _| browser.permission_request())
        .expect("actual waiting request");
    drop(call);
    cx.run_until_parked();
    assert!(!REQUESTS.contains(&request.id));
    browser.read_with(cx, |browser, _| {
        assert!(browser.tabs().0.iter().all(|tab| !tab.agent_allowed))
    });
}

fn primary_browser_page(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    page: crate::web::RetainedPage,
) -> Entity<crate::side::browser::Browser> {
    workspace.update_in(cx, |workspace, window, cx| {
        let browser = workspace
            .agent_browser(THREAD, window, cx)
            .expect("real scoped browser");
        browser.update(cx, |browser, cx| browser.fixture_add_retained_page(page, window, cx));
        browser
    })
}

#[gpui_kit::test]
fn captured_account_aba_eagerly_releases_parked_pages_with_stale_entities_held(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8111);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let marker = primary.read_with(cx, |workspace, _| workspace.gateway.marker());
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        page_probe.alive() && page_probe.parked(),
        "real primary close parked its HostPage"
    );
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    primary.read_with(cx, |workspace, _| {
        assert_eq!(workspace.gateway.marker(), marker, "ABA leaves fingerprint unchanged")
    });
    let auth = cx
        .cx
        .update(|cx| crate::windows::retention_fixtures::watch_identity(Arc::new(probe.clone()), cx));
    auth.unbounded_send(crate::gateway::Event::SignedIn)
        .expect("actual watcher event");
    cx.run_until_parked();
    assert!(
        !page_probe.alive(),
        "eager watcher cleanup releases resources, even with old UI clones alive"
    );
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}

#[gpui_kit::test]
fn primary_adoption_rechecks_captured_authority_before_restoring_any_parked_page(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8112);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let old_id = old_browser.entity_id();
    let marker = primary.read_with(cx, |workspace, _| workspace.gateway.marker());
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(page_probe.alive() && page_probe.parked());
    // No auth event or watcher cleanup is consumed before this fresh accepted Account.
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    let (fresh, cx) = fixture(cx, &probe);
    register(&fresh, cx, &probe, &source);
    fresh.read_with(cx, |workspace, _| assert_eq!(workspace.gateway.marker(), marker));
    let restored = fresh.update_in(cx, |workspace, window, cx| {
        workspace.restore_primary_terminals(window, cx)
    });
    assert!(restored, "unrelated retained primary state still adopts");
    assert!(!page_probe.alive(), "the old HostPage never reaches a fresh view");
    let (_, browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("fresh authority may create a fresh empty browser");
    assert_ne!(browser.entity_id(), old_id);
    browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}

#[gpui_kit::test]
fn current_account_event_releases_pages_from_an_older_accepted_epoch(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8113);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let account = primary.read_with(cx, |workspace, _| {
        workspace.account.clone().expect("accepted fixture account")
    });
    let marker = primary.read_with(cx, |workspace, _| workspace.gateway.marker());
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    let auth = cx
        .cx
        .update(|cx| crate::windows::retention_fixtures::watch_identity(Arc::new(probe.clone()), cx));
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(page_probe.alive() && page_probe.parked());
    let expected = probe.epoch.fetch_add(2, Ordering::SeqCst) + 2;
    primary.read_with(cx, |workspace, _| assert_eq!(workspace.gateway.marker(), marker));
    auth.unbounded_send(crate::gateway::Event::ReadSnapshot {
        epoch: probe.epoch.clone(),
        expected,
        revision: None,
        event: Box::new(crate::gateway::Event::Account(account)),
    })
    .expect("actual current Account envelope");
    cx.run_until_parked();
    assert!(
        !page_probe.alive(),
        "the Account branch must prune too, without a SignedIn event"
    );
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}

#[gpui_kit::test]
fn an_already_taken_bundle_rechecks_authority_at_page_consumption(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8114);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    let fresh_window = register(&fresh, cx, &probe, &source);
    let identity = fresh.read_with(cx, |workspace, _| {
        workspace.terminal_identity.clone().expect("current identity")
    });
    let saved = cx
        .cx
        .update(|cx| crate::windows::take_primary_terminals(fresh_window.window_id(), &identity, cx))
        .expect("healthy take must preserve the original bundle");
    assert!(page_probe.alive() && page_probe.parked());
    // Shell no longer owns this bundle. Neither its watcher nor another take can revoke it.
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.restore_private_terminals(saved.terminals, window, cx)
    });
    assert!(
        !page_probe.alive(),
        "actual page consumption must reject an expired extracted bundle"
    );
    fresh.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}

#[gpui_kit::test]
fn parked_pages_wait_for_the_exact_store_through_a_second_primary_close(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8121);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let before = old_browser.read_with(cx, |browser, _| {
        browser.tabs().0.iter().map(|tab| tab.id.clone()).collect::<Vec<_>>()
    });
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    register(&fresh, cx, &probe, &other_source);
    assert!(fresh.update_in(cx, |workspace, window, cx| {
        workspace.code = None;
        workspace.restore_primary_terminals(window, cx)
    }));
    assert!(
        page_probe.alive() && page_probe.parked(),
        "a missing reader keeps plain owners"
    );
    fresh.read_with(cx, |workspace, _| assert!(!workspace.has_browser(THREAD)));
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.code = crate::code::CodeStore::open_in(&other_source.data, other_source.scope.clone()).unwrap();
        workspace.sync_side(window, cx);
        assert!(
            !workspace.has_browser(THREAD),
            "same ID in another store cannot adopt these pages"
        );
        assert!(
            workspace.agent_browser(THREAD, window, cx).is_err(),
            "same-ID tool lookup cannot relabel parked pages"
        );
    });
    assert!(page_probe.alive() && page_probe.parked());
    fresh.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        page_probe.alive() && page_probe.parked(),
        "a second close preserves the original source"
    );
    let (last, cx) = fixture(cx, &probe);
    let last_window = register(&last, cx, &probe, &other_source);
    assert!(last.update_in(cx, |workspace, window, cx| {
        workspace.code = None;
        workspace.restore_primary_terminals(window, cx)
    }));
    last.update_in(cx, |workspace, window, cx| {
        workspace.code = crate::code::CodeStore::open_in(&source.data, source.scope.clone()).unwrap();
        workspace.sync_side(window, cx);
    });
    assert!(page_probe.alive() && !page_probe.parked());
    let (routed, browser, _) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(&source.origin(), THREAD, &valid(), cx))
        .expect("the exact original source adopts and drives its pages");
    assert_eq!(routed.window_id(), last_window.window_id());
    assert_ne!(browser.entity_id(), old_browser.entity_id());
    browser.read_with(cx, |browser, _| {
        let tabs = browser.tabs().0;
        assert_eq!(tabs.len(), before.len());
        assert_eq!(tabs.iter().map(|tab| tab.id.clone()).collect::<Vec<_>>(), before);
    });
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}

#[gpui_kit::test]
fn deferred_store_readiness_never_refreshes_a_parked_pages_account_proof(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&format!("{}|org-a", probe.url));
    let other_source = SourceStore::new(&source.scope);
    let (page, page_probe) = crate::web::fixture_page(8122);
    let (primary, cx) = fixture(cx, &probe);
    register(&primary, cx, &probe, &source);
    let old_browser = primary_browser_page(&primary, cx, page);
    let (other, cx) = fixture(cx, &probe);
    register(&other, cx, &probe, &other_source);
    primary.update_in(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    let (fresh, cx) = fixture(cx, &probe);
    register(&fresh, cx, &probe, &source);
    assert!(fresh.update_in(cx, |workspace, window, cx| {
        workspace.code = None;
        workspace.restore_primary_terminals(window, cx)
    }));
    assert!(page_probe.alive() && page_probe.parked());
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    fresh.update_in(cx, |workspace, window, cx| workspace.sync_side(window, cx));
    assert!(
        !page_probe.alive(),
        "proof cleanup cannot wait for a missing store reader"
    );
    fresh.update_in(cx, |workspace, window, cx| {
        workspace.code = crate::code::CodeStore::open_in(&source.data, source.scope.clone()).unwrap();
        workspace.sync_side(window, cx);
        assert!(!workspace.has_browser(THREAD));
    });
    old_browser.read_with(cx, |browser, _| assert!(browser.tabs().0.is_empty()));
}
