//! Actual qualified SQLite readers and held guard continuations, never global runtime/test authority.
use super::super::{
    Key,
    model::{self, SourceProof},
};
use super::{Probe, SourceStore, THREAD};
use crate::{
    backend::ExecutionAuthority, code::CodeStore, gateway::Account, side::shared_terminals::Guard,
    windows::retention::Identity,
};
use nexus_workbench::store::Store;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

fn account(org: &str) -> Account {
    Account::from_json(
        &json!({"tenant":{"id":org,"slug":org,"name":"Commands fixture","kind":"company"},
        "user":{"id":"person-a","personId":"person-a","name":"A","email":"a@example.test"},
        "role":"member","permissions":[]}),
    )
    .expect("actual Account wire shape")
}
fn qualified(
    source: &SourceStore,
    probe: &Probe,
    runner: Guard,
    after_read: Option<Arc<dyn Fn(bool) + Send + Sync>>,
) -> (Key, Guard) {
    let reader = CodeStore::open_in(&source.data, source.scope.clone())
        .expect("actual original reader")
        .expect("source exists");
    let summary = reader
        .summary(THREAD)
        .expect("scoped row")
        .expect("source-qualified session");
    let folder = reader.folder(THREAD).expect("original folder");
    let identity = Identity::current(probe, &account("org-a")).expect("accepted identity");
    let key = Key {
        identity,
        source: source.source(),
        session: THREAD.into(),
        folder: folder.path.clone(),
        base: folder.base.clone(),
    };
    let guard = model::source_guard(
        &key,
        &folder,
        reader.reader(),
        summary.created_at,
        summary.project_id,
        SourceProof {
            account: ExecutionAuthority::from_read(probe.epoch.clone(), probe.epoch.load(Ordering::SeqCst)),
            backend: crate::workspace::gateway_access::AccountGateway::fixture(Arc::new(probe.clone())),
            runner,
            after_read,
        },
    )
    .expect("qualified guard");
    (key, guard)
}
#[test]
fn original_session_deletion_permanently_revokes_even_after_exact_row_restore() {
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (_, guard) = qualified(&source, &probe, Arc::new(|| true), None);
    assert!(guard());
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("original writer");
    let original = store.thread(THREAD).expect("actual row").expect("existing session");
    store.delete_thread(THREAD).expect("real deletion");
    assert!(!guard(), "no removed session may read its old folder");
    store
        .insert_thread(&original)
        .expect("restore identical record, creation and scope");
    assert!(!guard(), "the original guard must not become current again");
    let (_, fresh) = qualified(&source, &probe, Arc::new(|| true), None);
    assert!(fresh(), "a newly admitted exact source remains usable");
}
#[test]
fn same_project_worktree_folder_change_revokes_and_does_not_rebind() {
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (_, guard) = qualified(&source, &probe, Arc::new(|| true), None);
    assert!(guard());
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("source writer");
    let mut original = store.thread(THREAD).expect("actual source row").expect("session");
    let moved = source.data.join("other worktree");
    std::fs::create_dir_all(&moved).expect("real alternate directory");
    original.summary.worktree = Some(
        serde_json::from_value(json!({"path":moved,"branch":"fixture","base":"main"})).expect("worktree metadata"),
    );
    store
        .save_thread(&original)
        .expect("real folder change under same project and creation");
    assert!(!guard());
    original.summary.worktree = None;
    store.save_thread(&original).expect("restore exact original folder");
    assert!(!guard());
    assert!(qualified(&source, &probe, Arc::new(|| true), None).1());
}
#[test]
fn valid_reader_held_across_another_observed_deletion_cannot_resurrect_guard() {
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (seen, observed) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let released = Mutex::new(released);
    let calls = AtomicUsize::new(0);
    let hook = Arc::new(move |valid: bool| {
        if valid && calls.fetch_add(1, Ordering::SeqCst) == 0 {
            seen.send(()).expect("held original observation");
            released
                .lock()
                .expect("private release")
                .recv_timeout(Duration::from_secs(3))
                .expect("release held guard");
        }
    });
    let (_, guard) = qualified(&source, &probe, Arc::new(|| true), Some(hook));
    let held = guard.clone();
    let thread = std::thread::spawn(move || held());
    observed
        .recv_timeout(Duration::from_secs(3))
        .expect("actual row read completed without holding reader mutex");
    let store = Store::open(&source.data.join("workbench.sqlite3")).expect("source writer");
    let original = store.thread(THREAD).expect("original row").expect("session");
    store.delete_thread(THREAD).expect("actual deletion");
    assert!(!guard(), "a separate invocation observes deletion and revokes");
    store
        .insert_thread(&original)
        .expect("restore exact row before older invocation finishes");
    release.send(()).expect("release original valid observation");
    assert!(
        !thread.join().expect("held guard completed"),
        "shared sticky revocation must win"
    );
    assert!(!guard());
}
#[test]
fn account_revocation_while_final_runner_check_waits_refuses_the_read() {
    let probe = Probe::new();
    let source = SourceStore::new(&probe);
    let (seen, observed) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let released = Mutex::new(released);
    let count = AtomicUsize::new(0);
    let runner: Guard = Arc::new(move || {
        if count.fetch_add(1, Ordering::SeqCst) == 1 {
            seen.send(()).expect("final runner gate entered");
            released
                .lock()
                .expect("per-call release")
                .recv_timeout(Duration::from_secs(3))
                .expect("release final gate");
        }
        true
    });
    let (_, guard) = qualified(&source, &probe, runner, None);
    let held = guard.clone();
    let task = std::thread::spawn(move || held());
    observed
        .recv_timeout(Duration::from_secs(3))
        .expect("real guard reached post-read runner callback");
    probe.epoch.fetch_add(2, Ordering::SeqCst);
    release.send(()).expect("release original callback");
    assert!(
        !task.join().expect("guard completed"),
        "proof must be checked after the blocking callback"
    );
    assert!(!guard());
}
