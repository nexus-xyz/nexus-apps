//! Production Workspace loops with deferred fake HTTP/SSE and platform visibility events. The Workspace is
//! deliberately not rendered and its visibility subscription is removed: task wakeups must clean up by themselves.
#[path = "test_backend.rs"]
mod test_backend;
use super::{
    Connection, Viewer, Workspace,
    model::{Page, Session},
};
use crate::{
    gateway::{Account, team_sessions::News},
    workspace::{Auth, View, machine_source::MachineSource},
};
use futures::channel::mpsc;
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, IntoElement, Render, Task, TestAppContext, Window, WindowVisibility,
    div,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use test_backend::{BackendFixture, Pending};

struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
fn header() -> Value {
    json!({"threadId":"fixture-thread","personId":"owner","private":false,"agent":"codex","status":"running","lastSeq":null,"followers":[]})
}
fn fixture(cx: &mut TestAppContext) -> (Entity<Workspace>, Arc<BackendFixture>, AnyWindowHandle) {
    cx.update(gpui_kit::init);
    let backend = BackendFixture::new();
    let (_, events) = mpsc::unbounded();
    let (_, arrivals) = mpsc::unbounded();
    let mut entity = None;
    let window=cx.add_window(|window,cx| {
        entity=Some(cx.new(|cx| {
            let mut workspace=Workspace::with_machine_source(backend.clone(),events,arrivals,false,MachineSource::isolated(),window,cx);
            workspace._code_poll=Task::ready(());
            workspace._subscriptions.clear();
            workspace.auth=Auth::SignedIn;
            workspace.account=Account::from_json(&json!({"tenant":{"id":"org-a","slug":"org-a","kind":"organization"},"user":{"id":"reader","email":"reader@example.test"}}));
            workspace.view=View::TeamSessions;
            workspace.team_presence.sessions=true;
            workspace.team_presence.viewer=Viewer::default();
            let scope=workspace.team_scope();
            workspace.team_presence.viewer.scope=scope;
            workspace.team_presence.viewer.transcript.open("fixture-thread".into());
            workspace.team_presence.viewer.transcript.session=Some(serde_json::from_value::<Session>(header()).unwrap());
            workspace
        }));
        Empty
    });
    let workspace = entity.unwrap();
    workspace.update(cx, |workspace, cx| {
        workspace.team_presence.viewer.scope = workspace.team_scope();
        cx.notify();
    });
    (workspace, backend, window.into())
}
fn start(workspace: &Entity<Workspace>, window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.sync_team_sessions(window, cx))
    })
    .unwrap();
    cx.run_until_parked();
}
fn take_poll(backend: &BackendFixture) -> Vec<Pending> {
    [
        "/v1/threads?scope=team&limit=50",
        "/v1/orgs/settings",
        "/v1/threads/fixture-thread",
        "/v1/threads/fixture-thread/presence",
    ]
    .into_iter()
    .map(|path| backend.take(path))
    .collect()
}
fn reply_poll(pending: Vec<Pending>, cx: &mut TestAppContext) {
    let consumed: Vec<_> = pending.iter().map(|pending| pending.consumed.clone()).collect();
    for pending in pending {
        let value = match pending.path.as_str() {
            "/v1/threads?scope=team&limit=50" => json!({"threads":[header()]}),
            "/v1/orgs/settings" => json!({"multiplayerSessions":true}),
            "/v1/threads/fixture-thread" => header(),
            _ => json!({"ok":true}),
        };
        assert!(pending.answer.send(Ok(value)).is_ok());
    }
    cx.run_until_parked();
    assert!(
        consumed
            .iter()
            .all(|consumed| consumed.load(std::sync::atomic::Ordering::SeqCst))
    );
}
fn suspended(workspace: &Entity<Workspace>, cx: &TestAppContext) {
    workspace.read_with(cx, |workspace, _| {
        let viewer = &workspace.team_presence.viewer;
        assert!(
            viewer.poll.is_none()
                && viewer.stream.is_none()
                && viewer.read.is_none()
                && viewer.older.is_none()
                && viewer.following.is_none()
        );
        assert_eq!(viewer.transcript.connection, Connection::Reconnecting);
    });
}

#[gpui_kit::test]
fn viewer_poll_reply_while_hidden_clears_slots_without_a_render_and_restarts(cx: &mut TestAppContext) {
    let (workspace, backend, window) = fixture(cx);
    start(&workspace, window, cx);
    let pending = take_poll(&backend);
    workspace.update(cx, |workspace, cx| {
        workspace.team_presence.viewer.read = Some(cx.spawn(async move |_, _| futures::future::pending::<()>().await));
        workspace.team_presence.viewer.older = Some(cx.spawn(async move |_, _| futures::future::pending::<()>().await));
    });
    let old = workspace.read_with(cx, |workspace, _| workspace.team_presence.viewer.transcript.incarnation);
    cx.simulate_window_visibility_change(window, WindowVisibility::Hidden);
    reply_poll(pending, cx);
    suspended(&workspace, cx);
    assert!(backend.pending.lock().unwrap().iter().any(|request| {
        request
            .body
            .as_ref()
            .is_some_and(|body| body["leave"] == true && body["client"] == old.to_string())
    }));
    cx.simulate_window_visibility_change(window, WindowVisibility::Visible);
    start(&workspace, window, cx);
    assert_eq!(backend.streams.lock().unwrap().len(), 2);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.team_presence.viewer.poll.is_some());
        assert_ne!(workspace.team_presence.viewer.transcript.incarnation, old);
    });
    assert_eq!(take_poll(&backend).len(), 4);
}

#[gpui_kit::test]
fn viewer_poll_timer_while_hidden_clears_completed_handle_without_a_render(cx: &mut TestAppContext) {
    let (workspace, backend, window) = fixture(cx);
    start(&workspace, window, cx);
    reply_poll(take_poll(&backend), cx);
    cx.simulate_window_visibility_change(window, WindowVisibility::Hidden);
    cx.background_executor.advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    suspended(&workspace, cx);
    cx.simulate_window_visibility_change(window, WindowVisibility::Visible);
    start(&workspace, window, cx);
    assert_eq!(backend.streams.lock().unwrap().len(), 2);
}

#[gpui_kit::test]
fn viewer_stream_wakeup_while_hidden_clears_slots_and_old_callback_cannot_clear_replacements(cx: &mut TestAppContext) {
    let (workspace, backend, window) = fixture(cx);
    start(&workspace, window, cx);
    let (scope, incarnation) = workspace.read_with(cx, |workspace, _| {
        (
            workspace.team_scope(),
            workspace.team_presence.viewer.transcript.incarnation,
        )
    });
    cx.simulate_window_visibility_change(window, WindowVisibility::Hidden);
    backend.streams.lock().unwrap()[0].unbounded_send(News::Closed).unwrap();
    cx.run_until_parked();
    suspended(&workspace, cx);
    cx.simulate_window_visibility_change(window, WindowVisibility::Visible);
    start(&workspace, window, cx);
    // A queued completion from the suspended incarnation must not cancel the replacement loops.
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(!workspace.team_viewer_visible(&scope, incarnation, window.is_visible(), cx));
            assert!(workspace.team_presence.viewer.poll.is_some());
            assert!(workspace.team_presence.viewer.stream.is_some());
        })
    })
    .unwrap();
}

#[gpui_kit::test]
fn viewer_follow_reply_while_hidden_releases_busy_and_a_new_follow_can_start(cx: &mut TestAppContext) {
    let (workspace, backend, window) = fixture(cx);
    start(&workspace, window, cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.toggle_team_follow(window, cx))
    })
    .unwrap();
    let pending = backend.take("/v1/threads/fixture-thread/follow");
    cx.simulate_window_visibility_change(window, WindowVisibility::Hidden);
    let mut followed = header();
    followed["followers"] = json!(["reader"]);
    pending.answer.send(Ok(followed)).unwrap();
    cx.run_until_parked();
    suspended(&workspace, cx);
    cx.simulate_window_visibility_change(window, WindowVisibility::Visible);
    start(&workspace, window, cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.toggle_team_follow(window, cx))
    })
    .unwrap();
    let _pending = backend.take("/v1/threads/fixture-thread/follow");
    assert!(workspace.read_with(cx, |workspace, _| workspace.team_presence.viewer.following.is_some()));
}

#[gpui_kit::test]
fn viewer_old_account_follow_reply_is_not_published(cx: &mut TestAppContext) {
    let (workspace, backend, window) = fixture(cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.toggle_team_follow(window, cx))
    })
    .unwrap();
    let pending = backend.take("/v1/threads/fixture-thread/follow");
    *backend.marker.lock().unwrap() = Some("account-b".into());
    let consumed = pending.consumed.clone();
    let mut followed = header();
    followed["followers"] = json!(["reader"]);
    pending.answer.send(Ok(followed)).unwrap();
    cx.run_until_parked();
    assert!(consumed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(workspace.read_with(cx, |workspace, _| {
        workspace
            .team_presence
            .viewer
            .transcript
            .session
            .as_ref()
            .unwrap()
            .followers
            .is_empty()
    }));
}

#[test]
fn viewer_successful_page_restores_live_only_while_the_stream_is_open() {
    let mut viewer = Viewer::default();
    viewer.transcript.open("fixture-thread".into());
    let ticket = viewer.transcript.ticket().unwrap();
    let page = || serde_json::from_value::<Page>(json!({"thread":header(),"events":[],"more":false})).unwrap();
    viewer.stream_live = true;
    viewer.transcript.connection = Connection::Reconnecting;
    assert!(viewer.apply_page(&ticket, page(), "reader", false));
    assert_eq!(viewer.transcript.connection, Connection::Live);
    viewer.stream_live = false;
    viewer.transcript.connection = Connection::Reconnecting;
    assert!(viewer.apply_page(&ticket, page(), "reader", false));
    assert_eq!(viewer.transcript.connection, Connection::Reconnecting);
}
