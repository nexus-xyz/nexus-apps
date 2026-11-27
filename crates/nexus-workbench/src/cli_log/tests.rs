//! A CLI conversation's log: bound to sync at creation, continued on resume, and read once.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::*;
use crate::session_sync::{Backend, Batch, ContentKey, ContentPolicy, Identity, Policy};
use crate::store::Store;
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd};

const SCOPE: &str = "https://gateway.example|org";

/// Content never leaves (`never`): no key is needed, and Gateway gets usage-only rows.
struct Usage {
    identity: Identity,
    stored: Mutex<Vec<Batch>>,
}

impl Backend for Usage {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn allow_local_cache(&self) -> bool {
        true
    }
    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        Box::pin(async {
            Ok(Policy {
                content: ContentPolicy::NeverLeaves,
            })
        })
    }
    fn head(&self, _: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        let head = self
            .stored
            .lock()
            .expect("stored")
            .iter()
            .flat_map(|batch| batch.events.iter().map(|event| event.seq))
            .max();
        Box::pin(async move { Ok(head) })
    }
    fn key(&self, _: &str, _: Option<u64>, _: ContentPolicy) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        Box::pin(async { Err("no key under never".into()) })
    }
    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        let last = batch.events.last().map_or(0, |event| event.seq);
        self.stored.lock().expect("stored").push(batch);
        Box::pin(async move { Ok(last) })
    }
    fn delete(&self, _: &str) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// The CLI's host: the signed-in person's backend, or none.
struct CliHost(Mutex<Option<Arc<Usage>>>);

impl Host for CliHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        self.0
            .lock()
            .expect("backend")
            .clone()
            .map(|backend| backend as Arc<dyn Backend>)
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        Box::pin(async { Err("the CLI starts its agents itself".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &crate::event::Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}

fn backend(user: &str) -> Arc<Usage> {
    Arc::new(Usage {
        identity: Identity {
            scope: SCOPE.into(),
            user: user.into(),
        },
        stored: Mutex::new(Vec::new()),
    })
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn workbench(store: Store, host: Arc<CliHost>) -> Workbench {
    Workbench::new(
        store,
        host,
        Config {
            scope: SCOPE.into(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench")
}

fn run<'a>(native_id: &'a str, cwd: &'a Path) -> CliRun<'a> {
    CliRun {
        agent: SessionAgent::Claude,
        native_id,
        cwd,
        model: Some("claude-opus-5-5"),
        via: Via::Subscription,
    }
}

fn user(text: &str) -> WbEvent {
    WbEvent::User {
        text: text.into(),
        images: vec![],
        from: None,
    }
}

#[tokio::test]
async fn a_conversation_logs_once_continues_on_resume_and_syncs() {
    let dir = Dir(std::env::temp_dir().join(format!("nexus-cli-log-{}", uuid::Uuid::new_v4())));
    let cwd = dir.0.join("acme");
    let ada = backend("usr_ada");
    let host = Arc::new(CliHost(Mutex::new(Some(ada.clone()))));
    let wb = workbench(Store::open(&dir.0.join("log.sqlite3")).expect("store"), host.clone());

    let thread = wb.cli_thread(&run("conv-1", &cwd)).expect("thread").expect("bound");
    assert_eq!(thread.read_to, 0);
    wb.cli_append(
        &thread.id,
        &[(1, user("Fix the flaky test\nthen push")), (2, user("and the docs"))],
        120,
    )
    .expect("append");
    let record = wb.store().thread(&thread.id).expect("read").expect("thread");
    assert_eq!(record.summary.title, "Fix the flaky test", "the first prompt names it");
    assert_eq!(record.summary.agent, SessionAgent::Claude);

    // The same conversation, resumed: the same thread, read on from where it stopped.
    let resumed = wb.cli_thread(&run("conv-1", &cwd)).expect("thread").expect("bound");
    assert_eq!(
        resumed,
        CliThread {
            id: thread.id.clone(),
            read_to: 120
        }
    );
    assert!(
        wb.cli_append(&thread.id, &[(3, user("again"))], 100).is_err(),
        "a line is never read twice"
    );
    assert_eq!(
        wb.store().events_after(&thread.id, 0).expect("events").len(),
        2,
        "all or nothing"
    );
    wb.cli_append(&thread.id, &[(3, user("go on"))], 200).expect("append");

    assert_eq!(wb.cli_unsynced().expect("unsynced"), 3);
    let report = wb.sync_sessions_once().await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.confirmed_events, 3);
    assert_eq!(wb.cli_unsynced().expect("unsynced"), 0);
    let stored = ada.stored.lock().expect("stored").clone();
    assert!(
        stored
            .iter()
            .flat_map(|batch| &batch.events)
            .all(|event| event.sealed.is_none())
    );

    // Another person signed in: Ada's thread is not theirs, and a new conversation binds to them.
    *host.0.lock().expect("backend") = Some(backend("usr_bob"));
    assert_eq!(wb.cli_thread(&run("conv-1", &cwd)).expect("thread"), None);
    assert!(wb.cli_thread(&run("conv-2", &cwd)).expect("thread").is_some());
}

#[test]
fn nothing_is_logged_without_sync() {
    let dir = Dir(std::env::temp_dir().join(format!("nexus-cli-log-{}", uuid::Uuid::new_v4())));
    let cwd = dir.0.join("acme");
    let off = workbench(
        Store::open(&dir.0.join("log.sqlite3")).expect("store"),
        Arc::new(CliHost(Mutex::new(None))),
    );
    assert_eq!(off.cli_thread(&run("conv-1", &cwd)).expect("thread"), None);
    assert!(off.store().threads(SCOPE).expect("threads").is_empty());
    let memory = workbench(
        Store::in_memory().expect("store"),
        Arc::new(CliHost(Mutex::new(Some(backend("usr_ada"))))),
    );
    assert_eq!(memory.cli_thread(&run("conv-1", &cwd)).expect("thread"), None);
}
