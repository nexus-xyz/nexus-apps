//! Isolated accepted-account/local-store helpers for real Files window lifecycle fixtures.
use super::{
    Key,
    model::{Presentation, Registry},
};
use crate::{
    side::{browser::agent::LocalOrigin, files::Files},
    workspace::{
        Workspace,
        gateway_chat::tests::{Probe, fixture},
        runs::{Runs, Source},
    },
};
use gpui_kit::{AnyWindowHandle, App, Context, Entity, TestAppContext, VisualTestContext, Window};
use nexus_workbench::store::{Store, ThreadRecord};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
mod combined_sources;
mod combined_tools;
mod lifecycle;
mod refresh;
mod returns;
mod source;
mod source_ingress;

const THREAD: &str = "files-original-session";
const OTHER: &str = "files-other-session";

struct SourceStore {
    data: PathBuf,
    scope: String,
    project: String,
    folder: PathBuf,
}
impl SourceStore {
    fn new(probe: &Probe) -> Self {
        let data = std::env::temp_dir().join(format!("nexus-files-detach-{}", uuid::Uuid::new_v4()));
        let folder = data.join("project with spaces");
        std::fs::create_dir_all(&folder).expect("isolated Files project folder");
        std::fs::write(
            folder.join("hello.rs"),
            concat!(r#"fn original() { println!("source A"); }"#, "\n"),
        )
        .expect("actual fixture file bytes");
        let scope = format!("{}|org-a", probe.url);
        let store = Store::open(&data.join("workbench.sqlite3")).expect("private SQLite source");
        let project = store
            .add_project(&folder, nexus_workbench::now_ms())
            .expect("actual project")
            .id;
        let source = Self {
            data,
            scope,
            project,
            folder,
        };
        source.insert(THREAD);
        source.insert(OTHER);
        source
    }
    fn insert(&self, id: &str) {
        let store = Store::open(&self.data.join("workbench.sqlite3")).expect("actual private source");
        let now = nexus_workbench::now_ms();
        let summary = serde_json::from_value(json!({
            "id": id, "projectId": self.project, "title": format!("Files {id}"), "agent": "claude",
            "model": null, "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0,
            "nexusSessions": [], "contextTokens": null, "contextWindow": null, "createdAt": now,
            "updatedAt": now, "archived": false
        }))
        .expect("production ThreadSummary");
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
                calls.lock().expect("private RPC calls").push((name, args));
                let (send, read) = futures::channel::oneshot::channel();
                let _ = send.send(Ok(Value::Null));
                read
            }),
        )
    }
}
fn theme(cx: &mut App) {
    cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
        crate::palette::ThemeChoice::Light,
        gpui_kit::WindowAppearance::Light,
    )));
}
fn install_source(workspace: &mut Workspace, source: &Source, rpc: &Rpc) {
    workspace.fixture_terminal_views();
    let Source::Local { data, scope } = source else {
        panic!("local fixture source")
    };
    workspace.code = crate::code::CodeStore::open_in(data, scope.clone()).expect("actual qualified reader");
    workspace.files_fixture_runs = Some(rpc.runs(source.clone()));
    workspace.browser_detach_runs = Some(rpc.runs(source.clone()));
    workspace
        .navigation_place_sink
        .get_or_insert_with(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));
    workspace.fixture_files_navigation(THREAD);
    workspace.fixture_files_navigation(OTHER);
}
fn select(workspace: &mut Workspace, session: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.fixture_select_files_session(session, window, cx);
}
fn setup<'a>(
    cx: &'a mut TestAppContext,
    probe: &Probe,
    source: &SourceStore,
    rpc: &Rpc,
) -> (Entity<Workspace>, Entity<Files>, &'a mut VisualTestContext) {
    cx.update(theme);
    let (workspace, cx) = fixture(cx, probe);
    let (window, files) = workspace.update_in(cx, |workspace, window, cx| {
        install_source(workspace, &source.source(), rpc);
        select(workspace, THREAD, window, cx);
        let files = workspace
            .fixture_local_files(THREAD, window, cx)
            .expect("guarded local Files");
        (window.window_handle(), files)
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), window, workspace.clone(), false, cx)
    });
    cx.run_until_parked();
    (workspace, files, cx)
}
fn key(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Key {
    workspace.read_with(cx, |workspace, _| {
        workspace.files_key(THREAD).expect("accepted source Files key")
    })
}
fn detached(owner: u64, cx: &mut VisualTestContext) -> (AnyWindowHandle, Entity<Files>) {
    cx.cx.update(
        |cx| match &cx.global::<Registry>().id(owner).expect("Files owner").presentation {
            Presentation::Detached { window, files } => (*window, files.clone()),
            _ => panic!("expected native detached Files"),
        },
    )
}
fn detach(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> (u64, AnyWindowHandle, Entity<Files>) {
    let key = key(workspace, cx);
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_files(window, cx))
        .expect("actual detach");
    cx.run_until_parked();
    let owner = cx
        .cx
        .update(|cx| cx.global::<Registry>().key(&key).expect("registered Files owner").id);
    let (window, files) = detached(owner, cx);
    (owner, window, files)
}

fn attached(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<Files>> {
    workspace.read_with(cx, |workspace, _| workspace.fixture_files())
}
