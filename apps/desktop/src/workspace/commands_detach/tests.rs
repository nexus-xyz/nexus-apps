//! Native owner callbacks use isolated SQLite stores and per-runner fake terminals.
use super::{
    model::{Presentation, Registry},
    *,
};
use crate::{
    side::shared_terminals::SharedTerminals,
    workspace::gateway_chat::tests::{Probe, fixture},
};
use gpui_kit::{AnyWindowHandle, App, AppContext, Entity, TestAppContext, VisualTestContext};
use nexus_workbench::store::{Store, ThreadRecord};
use serde_json::json;
use std::{path::PathBuf, rc::Rc, sync::Arc};
const THREAD: &str = "commands-original-session";
const OTHER: &str = "commands-other-session";
mod source;
struct SourceStore {
    data: PathBuf,
    scope: String,
    project: String,
    folder: PathBuf,
}
impl SourceStore {
    fn new(probe: &Probe) -> Self {
        let data = std::env::temp_dir().join(format!("nexus-commands-detach-{}", uuid::Uuid::new_v4()));
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
            "id": id, "projectId": self.project, "title": format!("Commands {id}"), "agent": "claude",
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
}
impl Drop for SourceStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn theme(cx: &mut App) {
    cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
        crate::palette::ThemeChoice::Light,
        gpui_kit::WindowAppearance::Light,
    )));
}
// Auxiliary panes use this deterministic transport. Commands retain the real original runner below.
fn auxiliary_runs(source: &SourceStore) -> super::super::runs::Runs {
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("isolated source snapshot");
    let threads = [THREAD, OTHER]
        .into_iter()
        .map(|id| store.thread(id).unwrap().unwrap().summary)
        .collect::<Vec<_>>();
    super::super::runs::Runs::deferred_source(
        source.source(),
        Arc::new(move |name, _args| {
            let (send, read) = futures::channel::oneshot::channel();
            let answer = match name {
                "wb_overview" => Ok(json!({"threads": threads, "projects": []})),
                "wb_thread_events" => Ok(json!([])),
                _ => Err(format!("No auxiliary fixture response for {name}")),
            };
            let _ = send.send(answer);
            read
        }),
    )
}
fn setup<'a>(
    cx: &'a mut TestAppContext,
    probe: &Probe,
    source: &SourceStore,
) -> (Entity<Workspace>, Entity<SharedTerminals>, &'a mut VisualTestContext) {
    cx.update(theme);
    let (workspace, cx) = fixture(cx, probe);
    let (handle, commands) = workspace.update_in(cx, |workspace, window, cx| {
        workspace.fixture_terminal_views();
        workspace.navigation_place_sink = Some(Rc::new(std::cell::RefCell::new(vec![])));
        workspace.code = crate::code::CodeStore::open_in(&source.data, source.scope.clone()).expect("original reader");
        workspace.fixture_commands_auxiliary_runs(THREAD, OTHER, &source.project, auxiliary_runs(source));
        let (runner, _) = crate::code_run::test_runner(source.data.clone(), &source.scope);
        runner.fixture_terminal(|_thread, _name, _args, valid| {
            let (send, read) = futures::channel::oneshot::channel();
            let _ = send.send(if valid() { Ok(json!([])) } else { Err("revoked".into()) });
            read
        });
        workspace.runner = Some(Rc::new(runner));
        workspace.fixture_select_files_session(THREAD, window, cx);
        let (key, runner, guard) = workspace
            .commands_attached_source(THREAD, window.window_handle().window_id())
            .expect("accepted original owner");
        let commands = cx.new(|cx| SharedTerminals::new_guarded(runner, THREAD.into(), guard, window, cx));
        workspace.fixture_bind_commands(key, commands.clone());
        (window.window_handle(), commands)
    });
    cx.cx.update(|cx| {
        crate::windows::retention_fixtures::register(Arc::new(probe.clone()), handle, workspace.clone(), false, cx)
    });
    cx.run_until_parked();
    (workspace, commands, cx)
}
fn detach(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
) -> (u64, AnyWindowHandle, Entity<SharedTerminals>) {
    let key = workspace.read_with(cx, |workspace, _| workspace.commands_key(THREAD).expect("original key"));
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_commands(window, cx))
        .expect("move local Commands");
    cx.run_until_parked();
    cx.cx.update(|cx| {
        let entry = cx.global::<Registry>().key(&key).expect("registered owner");
        match &entry.presentation {
            Presentation::Detached { window, commands } => (entry.id, *window, commands.clone()),
            _ => panic!("native Commands presentation"),
        }
    })
}
fn attached(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<SharedTerminals>> {
    workspace.read_with(cx, |workspace, _| workspace.fixture_attached_commands(THREAD))
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}
#[gpui_kit::test]
fn native_move_refocus_and_registered_close_restore_one_fresh_view(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, original, cx) = setup(cx, &probe, &source);
    let (owner, window, moved) = detach(&workspace, cx);
    assert_ne!(original.entity_id(), moved.entity_id());
    original.read_with(cx, |view, _| assert!(!view.is_current()));
    workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_commands(window, cx))
        .expect("repeat focuses existing owner");
    assert_eq!(cx.cx.update(|cx| cx.global::<Registry>().entries.len()), 1);
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual native close");
    draw(cx);
    let returned = attached(&workspace, cx).expect("installed close observer returns Commands");
    assert_ne!(returned.entity_id(), moved.entity_id());
    moved.read_with(cx, |view, _| assert!(!view.is_current()));
    cx.cx
        .update(|cx| returning::return_from(owner, window.window_id(), true, cx));
    draw(cx);
    assert_eq!(attached(&workspace, cx).unwrap().entity_id(), returned.entity_id());
}
fn rollback(after: bool, cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, original, cx) = setup(cx, &probe, &source);
    cx.cx.update(|cx| {
        init(cx);
        if after {
            cx.global_mut::<Registry>().fail_after_construct = true;
        } else {
            cx.global_mut::<Registry>().fail_next_open = true;
        }
    });
    let error = workspace
        .update_in(cx, |workspace, window, cx| workspace.detach_commands(window, cx))
        .expect_err("injected constructor failure");
    assert!(error.contains("returned"));
    let fresh = attached(&workspace, cx).expect("fresh rollback presentation");
    assert_ne!(original.entity_id(), fresh.entity_id());
    fresh.read_with(cx, |view, _| assert!(view.is_current()));
    original.read_with(cx, |view, _| assert!(!view.is_current()));
}
#[gpui_kit::test]
fn constructor_failure_before_consuming_plain_state_rolls_back(cx: &mut TestAppContext) {
    rollback(false, cx);
}
#[gpui_kit::test]
fn constructor_failure_after_consuming_plain_state_recovers_it_once(cx: &mut TestAppContext) {
    rollback(true, cx);
}
#[gpui_kit::test]
fn explicit_return_retires_native_view_and_quit_never_reopens(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, _, cx) = setup(cx, &probe, &source);
    let (owner, window, moved) = detach(&workspace, cx);
    cx.cx
        .update(|cx| returning::return_from(owner, window.window_id(), false, cx));
    draw(cx);
    assert!(attached(&workspace, cx).is_some());
    moved.read_with(cx, |view, _| assert!(!view.is_current()));
    let (owner, window, _) = detach(&workspace, cx);
    cx.cx.update(|cx| {
        quit(cx);
        returning::return_from(owner, window.window_id(), true, cx);
        assert!(!keeps_app_alive(cx));
        assert!(cx.global::<Registry>().entries.is_empty());
    });
}

#[gpui_kit::test]
fn source_native_close_keeps_original_commands_and_both_failed_opens_keep_plain_state(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, _, cx) = setup(cx, &probe, &source);
    let source_window = workspace.update_in(cx, |_, window, _| window.window_handle());
    let (owner, window, moved) = detach(&workspace, cx);
    source_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual source close");
    cx.run_until_parked();
    moved.read_with(cx, |commands, _| assert!(commands.is_current()));
    cx.cx.update(|cx| {
        assert!(current(owner, cx));
        assert!(keeps_app_alive(cx));
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(|_| None));
        cx.global_mut::<Registry>().fail_next_open = true;
    });
    window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .expect("actual native Commands close");
    cx.run_until_parked();
    cx.cx.update(|cx| {
        assert!(current(owner, cx));
        assert!(keeps_app_alive(cx));
        let Presentation::Returning { saved } = presentation(owner, cx).expect("recoverable owner") else {
            panic!("plain returning state")
        };
        assert!(saved.borrow().is_some());
    });
    moved.read_with(cx, |commands, _| assert!(!commands.is_current()));
}

#[gpui_kit::test]
fn duplicate_session_ids_in_two_stores_never_reserve_or_return_to_each_other(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let original = SourceStore::new(&probe);
    let other = SourceStore::new(&probe);
    let (workspace, _, cx) = setup(cx, &probe, &original);
    let (owner, _, _) = detach(&workspace, cx);
    let original_key = cx.cx.update(|cx| origin(owner, cx).unwrap().key);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.code =
            crate::code::CodeStore::open_in(&other.data, other.scope.clone()).expect("independent original reader");
        let other_key = workspace.commands_key(THREAD).expect("same id in exact second store");
        assert!(other_key != original_key);
        assert!(!reserved(&other_key, window.window_handle().window_id(), cx));
        assert!(!workspace.commands_matches_key(&original_key));
        assert!(!workspace.commands_return_target(&original_key));
        assert!(current(owner, cx), "navigation cannot replace detached authority");
    });
}

#[gpui_kit::test]
fn deleted_original_session_permanently_retires_strong_controls(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, original, cx) = setup(cx, &probe, &source);
    let (owner, _, moved) = detach(&workspace, cx);
    let continuation = cx.cx.update(|cx| guard(owner, cx).expect("frozen original guard"));
    Store::open(&source.data.join("workbench.sqlite3"))
        .unwrap()
        .delete_thread(THREAD)
        .unwrap();
    assert!(!continuation());
    source.insert(THREAD);
    assert!(
        !continuation(),
        "observed deletion cannot revive under equal session id"
    );
    cx.cx.update(|cx| {
        revoke_stale(cx);
        assert!(!current(owner, cx));
    });
    original.read_with(cx, |commands, _| assert!(!commands.is_current()));
    moved.read_with(cx, |commands, _| assert!(!commands.is_current()));
}

#[gpui_kit::test]
fn pending_return_grants_no_access_and_exact_reader_restores_without_replacement_runner(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, _, cx) = setup(cx, &probe, &source);
    let (identity, account, proof) = workspace.read_with(cx, |workspace, _| {
        (
            workspace.terminal_identity.clone(),
            workspace.account.clone(),
            workspace.gateway_account_authority.clone(),
        )
    });
    let source_window = workspace.update_in(cx, |_, window, _| window.window_handle());
    let (owner, native, _) = detach(&workspace, cx);
    let created = Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = created.clone();
    let accepted = probe.clone();
    let auxiliary = auxiliary_runs(&source);
    let project = source.project.clone();
    cx.cx.update(|cx| {
        cx.global_mut::<Registry>().app_window_factory = Some(Rc::new(move |cx| {
            let (window, workspace) = gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workspace = crate::workspace::gateway_chat::tests::fixture_model(window, cx, &accepted);
                    workspace.fixture_terminal_views();
                    workspace.fixture_commands_auxiliary_runs(THREAD, OTHER, &project, auxiliary.clone());
                    workspace.navigation_place_sink = Some(Rc::new(std::cell::RefCell::new(vec![])));
                    workspace.account = None;
                    workspace.terminal_identity = None;
                    workspace.gateway_account_authority = None;
                    workspace.code = None;
                    workspace.runner = None;
                    workspace
                })
            })
            .ok()?;
            crate::windows::retention_fixtures::register(
                Arc::new(accepted.clone()),
                window,
                workspace.clone(),
                false,
                cx,
            );
            record.borrow_mut().push((window, workspace.clone()));
            Some((window, workspace))
        }));
    });
    source_window
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .unwrap();
    native
        .update(&mut cx.cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(created.borrow().len(), 1);
    let (destination, workspace) = created.borrow()[0].clone();
    let mut visual = VisualTestContext::from_window(destination, &cx.cx);
    workspace.read_with(&mut visual, |workspace, _| {
        assert!(workspace.commands_return.is_some());
        assert!(workspace.fixture_commands_empty());
        assert!(workspace.commands_key(THREAD).is_none());
    });
    let key = visual.cx.update(|cx| origin(owner, cx).unwrap().key);
    let browser = super::super::browser_detach::Key {
        identity: key.identity.clone(),
        source: key.source.clone(),
        session: key.session.clone(),
        tool: "browser",
    };
    let files = super::super::files_detach::Key {
        identity: key.identity.clone(),
        source: key.source.clone(),
        session: key.session.clone(),
        folder: key.folder.clone(),
        base: key.base.clone(),
    };
    visual.cx.update(|cx| {
        let browser_target = crate::windows::browser_return_window(&browser, source_window.window_id(), cx)
            .expect("Browser coalesces pending Commands destination");
        let files_target = crate::windows::files_return_window(&files, source_window.window_id(), cx)
            .expect("Files coalesces pending Commands destination");
        assert_eq!(browser_target.0.window_id(), destination.window_id());
        assert!(!browser_target.2);
        assert_eq!(files_target.0.window_id(), destination.window_id());
        assert!(!files_target.2);
    });
    workspace.update_in(&mut visual, |workspace, window, cx| {
        workspace.account = account;
        workspace.terminal_identity = identity;
        workspace.gateway_account_authority = proof;
        workspace.code = crate::code::CodeStore::open_in(&source.data, source.scope.clone())
            .expect("original concrete reader arrives");
        assert!(workspace.runner.is_none(), "return must keep the original saved runner");
        workspace.sync_commands_return(window, cx);
    });
    visual.run_until_parked();
    let returned = attached(&workspace, &mut visual).expect("exact original reader accepts plain Commands owner");
    returned.read_with(&mut visual, |commands, _| assert!(commands.is_current()));
    workspace.read_with(&mut visual, |workspace, _| assert!(workspace.commands_return.is_none()));
    assert_eq!(created.borrow().len(), 1);
}

#[gpui_kit::test]
fn only_explicit_reopen_can_replace_a_revoked_attached_presentation(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (workspace, original, cx) = setup(cx, &probe, &source);
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    original.read_with(cx, |commands, _| assert!(!commands.is_current()));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.gateway_account_authority = Some(crate::backend::ExecutionAuthority::from_read(
            probe.epoch.clone(),
            probe.epoch.load(Ordering::SeqCst),
        ));
        assert!(
            workspace.fixture_local_commands(window, cx).is_none(),
            "sync cannot silently replace stale controls"
        );
        workspace.navigation_restoring = true;
        workspace.open_code(THREAD, window, cx);
        assert!(
            workspace.fixture_local_commands(window, cx).is_none(),
            "automatic route restoration cannot recapture authority"
        );
        workspace.navigation_restoring = false;
        workspace.open_code(THREAD, window, cx);
        let fresh = workspace
            .fixture_local_commands(window, cx)
            .expect("explicit reopen admits a fresh original source proof");
        assert_ne!(fresh.entity_id(), original.entity_id());
        assert!(fresh.read(cx).is_current());
        assert!(
            !original.read(cx).is_current(),
            "strong stale clone remains permanently inert"
        );
    });
}
