use super::*;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathTaken {
    CancelBeforeLaunch,
    CancelAfterLaunch,
    PayerReadFailed,
    PayerRejected,
    SpawnFailed,
    SaveFailed,
    MissingPipes,
    Exited,
}

struct LocalHost {
    path: PathTaken,
    workbench: OnceLock<std::sync::Weak<Inner>>,
    root: PathBuf,
    launches: AtomicUsize,
    ended: Mutex<Vec<String>>,
    revoked: Notify,
}

impl LocalHost {
    fn cancel(&self, id: &str) {
        let inner = self
            .workbench
            .get()
            .expect("workbench")
            .upgrade()
            .expect("workbench still alive");
        let workbench = Workbench { inner };
        let live = workbench.live(id).expect("registered start");
        assert_eq!(workbench.stop_live(&live, StopReason::Stop, None), Some(false));
    }
}

impl Host for LocalHost {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        match self.path {
            PathTaken::CancelAfterLaunch => self.cancel(&request.thread_id),
            PathTaken::PayerReadFailed => {
                // The first payer read happened before launch. Fail only the second read.
                rusqlite::Connection::open(self.root.join("store.sqlite3"))
                    .expect("test store")
                    .execute_batch("drop table computer_required_payers")
                    .expect("fail the second payer read");
            }
            _ => {}
        }
        let program = self.root.join("fake-claude");
        let env = HashMap::from([("HOME".into(), self.root.to_string_lossy().into_owned())]);
        Box::pin(async move {
            Ok(Launch {
                spawn: Spawn {
                    program,
                    args: Vec::new(),
                    env,
                    cwd: request.cwd,
                },
                attribution: None,
                nexus_session: None,
                local_run: Some("local_test".into()),
                local_provider: None,
                model: None,
                session_mcp: mcp_servers::SessionMcp::default(),
            })
        })
    }

    fn end_local_run(&self, id: &str) {
        self.ended.lock().expect("revocations").push(id.into());
        self.revoked.notify_one();
    }

    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        panic!("a local run reached Gateway cleanup");
    }

    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }

    fn emit(&self, emitted: Emit) {
        if self.path == PathTaken::CancelBeforeLaunch
            && let Emit::Event(StoredEvent {
                thread_id,
                event: WbEvent::TurnStart { .. },
                ..
            }) = emitted
        {
            self.cancel(&thread_id);
        }
    }
}

async fn exercise(path: PathTaken) {
    let root = std::env::temp_dir().join(format!("nexus-local-run-{}", uuid::Uuid::new_v4()));
    let project_dir = root.join("project");
    std::fs::create_dir_all(&project_dir).expect("test directory");
    if path != PathTaken::SpawnFailed {
        let auth_method = if path == PathTaken::PayerRejected {
            "api_key"
        } else {
            "claude.ai"
        };
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = auth ]; then printf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"{auth_method}\"}}'; exit 0; fi\nexit 0\n"
        );
        process::write_fake_script(&root.join("fake-claude"), &script);
    }
    let db = root.join("store.sqlite3");
    let host = Arc::new(LocalHost {
        path,
        workbench: OnceLock::new(),
        root: root.clone(),
        launches: AtomicUsize::new(0),
        ended: Mutex::new(Vec::new()),
        revoked: Notify::new(),
    });
    let config = Config {
        scope: session_scope::SessionScope::local_text().into(),
        instructions: Some(root.join("instructions")),
        ..Config::default()
    };
    let workbench = Workbench::new(Store::open(&db).expect("store"), host.clone(), config).expect("workbench");
    host.workbench
        .set(Arc::downgrade(&workbench.inner))
        .expect("one workbench");
    if path == PathTaken::SaveFailed {
        rusqlite::Connection::open(db)
            .expect("test store")
            .execute_batch(
                "create trigger fail_session_write before insert on events
             when json_extract(new.body, '$.kind') = 'session'
             begin select raise(abort, 'test session save failure'); end;",
            )
            .expect("fail the spawned session's event write");
    }
    if path == PathTaken::MissingPipes {
        workbench.inner.close_launch_stderr.store(true, Ordering::SeqCst);
    }
    let project = workbench.add_project(&project_dir).expect("project");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        workbench.create_thread(NewThread {
            project_id: project.id,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Subscription,
            provider: None,
            text: "hello".into(),
            images: Vec::new(),
            worktree: None,
            parent: None,
            orchestration: false,
            name: None,
        }),
    )
    .await
    .expect("start should finish promptly");

    assert_eq!(
        host.launches.load(Ordering::SeqCst),
        usize::from(path != PathTaken::CancelBeforeLaunch)
    );
    if path == PathTaken::CancelBeforeLaunch {
        assert!(host.ended.lock().expect("revocations").is_empty());
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(5), host.revoked.notified())
            .await
            .expect("local run revoked");
        assert_eq!(*host.ended.lock().expect("revocations"), ["local_test"]);
    }
    if path == PathTaken::PayerReadFailed {
        let error = result.expect_err("payer read error propagates");
        assert!(error.contains("computer_required_payers"), "{error}");
    } else {
        let thread = result.expect("created session");
        let events = workbench.events(&thread.id, 0).expect("events");
        let errors: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.event {
                WbEvent::Error { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        match path {
            PathTaken::CancelBeforeLaunch | PathTaken::CancelAfterLaunch => {
                assert!(events.iter().any(|event| matches!(
                    event.event,
                    WbEvent::TurnEnd {
                        status: TurnStatus::Interrupted,
                        ..
                    }
                )));
                assert!(errors.is_empty(), "{errors:?}");
            }
            PathTaken::PayerRejected => assert_eq!(errors, ["Switch Claude Code to a subscription, then try again."]),
            PathTaken::SpawnFailed => assert!(
                errors.iter().any(|error| error.starts_with("Could not start ")),
                "{errors:?}"
            ),
            PathTaken::SaveFailed => assert_eq!(errors, [UNSAVED]),
            PathTaken::MissingPipes => assert_eq!(errors, ["Could not talk to the agent."]),
            PathTaken::Exited => assert!(
                events
                    .iter()
                    .any(|event| matches!(event.event, WbEvent::Session { .. }))
            ),
            PathTaken::PayerReadFailed => unreachable!(),
        }
        workbench.shutdown().await;
    }
    drop(workbench);
    std::fs::remove_dir_all(root).expect("remove test directory");
}

#[tokio::test]
async fn local_runs_cancel_before_a_grant_is_issued_or_revoke_it_after_launch() {
    exercise(PathTaken::CancelBeforeLaunch).await;
    exercise(PathTaken::CancelAfterLaunch).await;
}

#[tokio::test]
async fn local_runs_revoke_the_grant_on_payer_read_failure_or_rejection() {
    exercise(PathTaken::PayerReadFailed).await;
    exercise(PathTaken::PayerRejected).await;
}

#[tokio::test]
async fn local_runs_revoke_the_grant_on_spawn_save_or_pipe_failure() {
    exercise(PathTaken::SpawnFailed).await;
    exercise(PathTaken::SaveFailed).await;
    exercise(PathTaken::MissingPipes).await;
}

#[tokio::test]
async fn local_runs_revoke_the_grant_on_process_exit() {
    exercise(PathTaken::Exited).await;
}
