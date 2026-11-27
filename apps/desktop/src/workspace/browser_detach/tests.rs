//! Registered native windows move real fake-CEF owners; no user stores, runners or keychain are touched.
use super::{
    Key,
    model::{Origin, Presentation, Registry},
    returning,
};
use crate::{
    side::browser::{Browser, BrowserEvent, agent::LocalOrigin},
    workspace::{
        View, Workspace,
        gateway_chat::tests::{Probe, fixture},
        runs::{Runs, Source},
    },
};
use gpui_kit::{AnyWindowHandle, App, AppContext, Context, Entity, TestAppContext, VisualTestContext, Window};
use nexus_workbench::store::{Store, ThreadRecord};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
};

mod delivery;
mod lifecycle;
mod return_readiness;
mod returns;
mod source_ingress;

const THREAD: &str = "detached-original-session";
const OTHER: &str = "detached-other-session";

struct SourceStore {
    data: PathBuf,
    scope: String,
    project: String,
}
impl SourceStore {
    fn new(probe: &Probe) -> Self {
        let data = std::env::temp_dir().join(format!("nexus-browser-detach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data).expect("isolated source folder");
        let scope = format!("{}|org-a", probe.url);
        let store = Store::open(&data.join("workbench.sqlite3")).expect("real local store");
        let project = store
            .add_project(&data, nexus_workbench::now_ms())
            .expect("stored project")
            .id;
        let source = Self { data, scope, project };
        source.insert(THREAD);
        source.insert(OTHER);
        source
    }
    fn insert(&self, id: &str) {
        let store = Store::open(&self.data.join("workbench.sqlite3")).expect("real local store");
        let now = nexus_workbench::now_ms();
        let summary = serde_json::from_value(json!({
            "id": id, "projectId": self.project, "title": format!("Original {id}"),
            "agent": "claude", "model": null, "permissionMode": "default", "via": "nexus",
            "status": "idle", "costUsd": 0.0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": now, "updatedAt": now, "archived": false
        }))
        .expect("production summary shape");
        store
            .insert_thread(&ThreadRecord {
                summary,
                native_id: format!("native-{id}"),
                native_started: false,
                scope: self.scope.clone(),
                fork_from: None,
                seed: None,
                automatic_title: false,
                delivered_seq: 0,
                halt: None,
            })
            .expect("source-qualified session");
    }
    fn source(&self) -> Source {
        Source::Local {
            data: self.data.clone(),
            scope: self.scope.clone(),
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

#[derive(Clone, Default)]
struct Rpc {
    calls: Arc<Mutex<Vec<(&'static str, Value)>>>,
}
impl Rpc {
    fn runs(&self, source: Source) -> Runs {
        let calls = self.calls.clone();
        Runs::deferred_source(
            source,
            Arc::new(move |name, args| {
                calls.lock().expect("recorded transport").push((name, args));
                let (send, read) = futures::channel::oneshot::channel();
                let _ = send.send(Ok(Value::Null));
                read
            }),
        )
    }
    fn calls(&self) -> Vec<(&'static str, Value)> {
        self.calls.lock().expect("recorded transport").clone()
    }
}

fn theme(cx: &mut App) {
    cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
        crate::palette::ThemeChoice::Light,
        gpui_kit::WindowAppearance::Light,
    )));
}
fn install_source(workspace: &mut Workspace, source: &Source, rpc: &Rpc) {
    let Source::Local { data, scope } = source else {
        panic!("local fixture source")
    };
    workspace.code = crate::code::CodeStore::open_in(data, scope.clone()).expect("scoped reader");
    assert!(
        workspace
            .code
            .as_ref()
            .expect("concrete reader")
            .summary(THREAD)
            .expect("membership read")
            .is_some()
    );
    workspace.browser_detach_runs = Some(rpc.runs(source.clone()));
    assert!(
        workspace.navigation_place_sink.is_some(),
        "fixture navigation must stay in memory"
    );
}
fn select(workspace: &mut Workspace, session: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.fixture_select_code_browser(session, window, cx);
}
fn setup<'a>(
    cx: &'a mut TestAppContext,
    probe: &Probe,
    source: &SourceStore,
    rpc: &Rpc,
    page: crate::web::RetainedPage,
) -> (Entity<Workspace>, Entity<Browser>, &'a mut VisualTestContext) {
    cx.update(theme);
    let (workspace, cx) = fixture(cx, probe);
    let handle = workspace.update_in(cx, |workspace, window, cx| {
        install_source(workspace, &source.source(), rpc);
        select(workspace, THREAD, window, cx);
        window.window_handle()
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), handle, workspace.clone(), false, cx)
    });
    let browser = workspace.update_in(cx, |workspace, window, cx| {
        workspace.fixture_local_browser(THREAD, page, window, cx)
    });
    (workspace, browser, cx)
}
fn key(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Key {
    workspace.read_with(cx, |workspace, _| {
        workspace.browser_detach_key(THREAD).expect("accepted source key")
    })
}
fn owner(key: &Key, cx: &mut VisualTestContext) -> u64 {
    cx.cx
        .update(|cx| cx.global::<Registry>().key(key).expect("registered resource owner").id)
}
fn detached(id: u64, cx: &mut VisualTestContext) -> (AnyWindowHandle, Entity<Browser>) {
    cx.cx.update(
        |cx| match &cx.global::<Registry>().id(id).expect("retained owner").presentation {
            Presentation::Detached { window, browser } => (*window, browser.clone()),
            _ => panic!("expected detached presentation"),
        },
    )
}
fn detach(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (u64, AnyWindowHandle, Entity<Browser>) {
    let key = key(workspace, cx);
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_browser(window, cx))
        .expect("detach active local CEF owner");
    cx.run_until_parked();
    let id = owner(&key, cx);
    let (window, browser) = detached(id, cx);
    (id, window, browser)
}
fn origin(id: u64, cx: &mut VisualTestContext) -> Origin {
    cx.cx
        .update(|cx| cx.global::<Registry>().id(id).expect("original owner").origin.clone())
}
fn emit(browser: &Entity<Browser>, event: BrowserEvent, cx: &mut VisualTestContext) {
    browser.update(cx, |_, cx| cx.emit(event));
    cx.run_until_parked();
}
fn pending(id: u64, cx: &mut VisualTestContext) -> usize {
    cx.cx
        .update(|cx| cx.global::<Registry>().id(id).expect("pending owner").pending.len())
}
fn snapshot(id: u64, cx: &mut VisualTestContext) -> crate::workspace::drafts::ParkedDraft {
    let origin = origin(id, cx);
    cx.cx.update(|cx| origin.draft.snapshot(cx))
}

fn agent_tabs(source: &LocalOrigin, cx: &mut VisualTestContext) -> (AnyWindowHandle, Entity<Browser>, Value) {
    let valid: crate::side::browser::agent::Guard = Arc::new(|| true);
    let (window, browser, guard) = cx
        .cx
        .update(|cx| crate::windows::agent_browser(source, THREAD, &valid, cx))
        .expect("source-authorized browser route");
    let task = window
        .update(&mut cx.cx, |_, window, cx| {
            browser.update(cx, |browser, cx| {
                browser.agent(
                    "browser_tabs".into(),
                    json!({}),
                    "detached-fixture-run".into(),
                    guard,
                    window,
                    cx,
                )
            })
        })
        .expect("live native browser window");
    cx.run_until_parked();
    let answer = futures::FutureExt::now_or_never(task)
        .expect("native tabs request settled")
        .expect("current browser answers");
    (window, browser, answer)
}
