//! Headless: a page that fails to load loads again, once, on its own; a second failure shows the failure card with
//! Try again; a notice of ours stays as it is.
use super::super::machine_source::MachineSource;
use super::super::navigation::test_backend::BackendProbe;
use super::super::{Auth, Workspace};
use super::{AfterFailure, Content, after_failure, expired};
use crate::gateway::{ChatText, PageFrame};
use crate::web::{WebEvent, WebView};
use gpui_kit::{Entity, EntityId, Modifiers, Task, TestAppContext, VisualTestContext};

const FRESH: &str = "https://pag-1.pages.test/p/pag_1/1?exp=4102444800&sig=s";

/// A Workspace on `backend`, a fixture: no live Gateway thread to wake the test's tasks from outside.
fn workspace<'a>(cx: &'a mut TestAppContext, backend: &BackendProbe) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    cx.update(gpui_kit::init);
    let gateway = backend.clone();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.code = None;
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        // Opens the Workbench on Artifacts; the page below replaces it.
        workspace.open_chat_text(
            ChatText {
                title: "Capability Sheet".into(),
                markdown: "# Capability Sheet".into(),
                path: "/artifacts/art_1".into(),
            },
            cx,
        );
        workspace
    });
    workspace.update_in(cx, |_, window, _| window.activate_window());
    draw(cx);
    cx.executor().advance_clock(crate::motion::PANEL * 2);
    draw(cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(workspace: &Entity<Workspace>, url: &str, cx: &mut VisualTestContext) {
    let frame = PageFrame {
        url: url.into(),
        title: Some("Growth".into()),
        path: "/pages/pag_1".into(),
        version_key: Some("pag_1@1".into()),
        mint: Some("/v1/pages/pag_1/versions/1/frame".into()),
    };
    workspace.update_in(cx, |workspace, window, cx| workspace.open_page(frame, window, cx));
    draw(cx);
}

/// The web view the panel shows, if it shows one.
fn shown(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<WebView>> {
    workspace.read_with(cx, |workspace, _| match &workspace.page.as_ref()?.content {
        Content::Web(view) => Some(view.clone()),
        _ => None,
    })
}

fn fail(view: &Entity<WebView>, status: Option<u16>, cx: &mut VisualTestContext) {
    view.update(cx, |_, cx| cx.emit(WebEvent::LoadFailed(status)));
    draw(cx);
}

fn id(view: &Option<Entity<WebView>>) -> Option<EntityId> {
    view.as_ref().map(|view| view.entity_id())
}

#[test]
fn only_an_engine_failure_is_tried_again_and_only_once() {
    assert_eq!(after_failure(None, false), AfterFailure::Retry);
    assert_eq!(after_failure(None, true), AfterFailure::Explain);
    for status in [403, 404, 502] {
        assert_eq!(
            after_failure(Some(status), false),
            AfterFailure::Keep,
            "{status} is a notice of ours"
        );
    }
}

#[test]
fn a_signed_address_expires_a_little_before_its_exp() {
    let url = "https://pag-1.pages.test/p/pag_1/1?exp=1000&sig=s";
    assert!(!expired(url, 900));
    assert!(expired(url, 980), "a load must get there before it expires");
    assert!(expired(url, 2000));
    assert!(
        !expired("https://example.test/page", 2000),
        "an address without exp never expires"
    );
}

#[gpui_kit::test]
fn a_page_that_fails_loads_again_once_then_says_so(cx: &mut TestAppContext) {
    let backend = BackendProbe::new();
    let (workspace, cx) = workspace(cx, &backend);
    open(&workspace, FRESH, cx);
    let first = shown(&workspace, cx);
    fail(first.as_ref().expect("the page loads"), None, cx);

    let second = shown(&workspace, cx);
    assert!(second.is_some(), "the page loads again on its own");
    assert_ne!(id(&first), id(&second), "in a new web view");

    fail(second.as_ref().unwrap(), None, cx);
    assert!(
        shown(&workspace, cx).is_none(),
        "a second failure does not load a third time"
    );
    assert!(workspace.read_with(cx, |workspace, _| matches!(
        workspace.page.as_ref().map(|panel| &panel.content),
        Some(Content::Failed)
    )));
    let retry = cx.debug_bounds("page-retry").expect("the card offers Try again");

    cx.simulate_click(retry.center(), Modifiers::none());
    draw(cx);
    let third = shown(&workspace, cx);
    assert!(third.is_some(), "Try again loads the page");
    assert_ne!(id(&second), id(&third));
}

#[gpui_kit::test]
fn a_notice_of_ours_is_not_tried_again(cx: &mut TestAppContext) {
    let backend = BackendProbe::new();
    let (workspace, cx) = workspace(cx, &backend);
    open(&workspace, FRESH, cx);
    let first = shown(&workspace, cx);
    fail(first.as_ref().unwrap(), Some(404), cx);
    assert_eq!(id(&first), id(&shown(&workspace, cx)), "the notice stays");
    assert!(cx.debug_bounds("page-retry").is_none());
}

#[gpui_kit::test]
fn an_expired_address_is_signed_again_before_the_next_try(cx: &mut TestAppContext) {
    let backend = BackendProbe::new();
    let (workspace, cx) = workspace(cx, &backend);
    open(&workspace, "https://pag-1.pages.test/p/pag_1/1?exp=1&sig=s", cx);
    let first = shown(&workspace, cx);
    fail(first.as_ref().unwrap(), None, cx);
    assert!(
        backend
            .reads
            .lock()
            .unwrap()
            .iter()
            .any(|path| path == "/v1/pages/pag_1/versions/1/frame"),
        "Gateway is asked to sign the same version again"
    );
    assert!(
        shown(&workspace, cx).is_none(),
        "the expired address is not loaded again"
    );
    // The fixture's answer names no address: the card says so rather than loading nothing.
    assert!(workspace.read_with(cx, |workspace, _| matches!(
        workspace.page.as_ref().map(|panel| &panel.content),
        Some(Content::Failed)
    )));
}
