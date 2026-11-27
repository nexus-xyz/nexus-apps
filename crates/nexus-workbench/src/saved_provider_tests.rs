use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use nexus_providers::{KeyRef, ProviderError, Secret};

#[derive(Default)]
pub(crate) struct Keys(AtomicUsize);
impl nexus_providers::KeySource for Keys {
    fn read(&self, _: &KeyRef) -> std::result::Result<Option<Secret>, ProviderError> {
        let generation = self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Some(Secret::new(format!(
            "byo-fixture-credential-generation-{generation}"
        ))))
    }
    fn write(&self, _: &KeyRef, _: Secret) -> std::result::Result<(), ProviderError> {
        panic!("launch cannot write credentials")
    }
    fn delete(&self, _: &KeyRef) -> std::result::Result<(), ProviderError> {
        panic!("launch cannot delete credentials")
    }
}

const SIGN_OUT: &str = "Sign out of Nexus to use this provider on this computer.";
const MISSING: &str = "This provider was removed. Choose a provider in Settings.";
const DISABLED: &str = "This computer cannot start a local provider session.";
const SIGNED_IN_SCOPE: &str = "https://gateway.invalid|org_test";
const NOTICE: &str = "These MCP servers may receive the provider key: native-db.";
const NOTICE_CHILD_STORE: &str = "NEXUS_TEST_NOTICE_CHILD_STORE";
const NOTICE_CHILD_THREAD: &str = "NEXUS_TEST_NOTICE_CHILD_THREAD";
const NOTICE_CHILD_READY: &str = "NEXUS_TEST_NOTICE_CHILD_READY";

#[derive(Default)]
struct TestHost {
    local_providers_opt_in: provider::LocalProvidersOptIn,
    sync_lookups: AtomicUsize,
    launches: AtomicUsize,
    keyed_spawn: Option<PathBuf>,
    revoked: AtomicUsize,
    source_generation: AtomicUsize,
    launch_pause: Option<Arc<LaunchPause>>,
}

#[derive(Default)]
struct LaunchPause {
    reached: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

impl Host for TestHost {
    fn local_providers_opt_in(&self) -> &provider::LocalProvidersOptIn {
        &self.local_providers_opt_in
    }

    fn while_saved_provider_allowed(&self, action: &mut dyn FnMut()) -> Result<()> {
        action();
        Ok(())
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        assert_eq!(request.scope, SessionScope::Local);
        assert_eq!(
            request.provider.as_ref().map(nexus_providers::InstanceId::as_str),
            Some("key:test")
        );
        self.launches.fetch_add(1, Ordering::Relaxed);
        let Some(root) = &self.keyed_spawn else {
            let pause = self.launch_pause.clone();
            return Box::pin(async move {
                if let Some(pause) = pause {
                    pause.reached.notify_one();
                    pause.resume.notified().await;
                }
                Err(MISSING.into())
            });
        };
        let mut native_mcp = local_provider::NativeMcp::default();
        native_mcp.include("native-db");
        let launch = Launch {
            attribution: None,
            spawn: Spawn {
                program: std::env::current_exe().expect("test binary"),
                args: vec![
                    "--exact".into(),
                    "saved_provider_tests::keyed_launch_notice_child".into(),
                ],
                env: HashMap::from([
                    ("ANTHROPIC_API_KEY".into(), "fixture-provider-key".into()),
                    (
                        NOTICE_CHILD_STORE.into(),
                        root.join("store.sqlite3").display().to_string(),
                    ),
                    (NOTICE_CHILD_THREAD.into(), request.thread_id),
                    (
                        NOTICE_CHILD_READY.into(),
                        root.join("child-ready").display().to_string(),
                    ),
                ]),
                cwd: request.cwd,
            },
            nexus_session: None,
            local_run: Some("keyed-run".into()),
            local_provider: Some(local_provider::Run {
                id: request.provider.expect("provider"),
                kind: nexus_providers::ProviderKind::ApiKey,
                native_mcp,
            }),
            model: None,
            session_mcp: Default::default(),
        };
        Box::pin(async { Ok(launch) })
    }

    fn saved_server_models(&self, provider: &nexus_providers::InstanceId) -> Result<Vec<String>> {
        assert_eq!(provider.as_str(), "server:test");
        Ok(vec!["team/replacement".into()])
    }

    fn end_local_run(&self, run: &str) {
        assert_eq!(run, "keyed-run");
        self.revoked.fetch_add(1, Ordering::Relaxed);
    }

    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        panic!("a saved provider reached Gateway cleanup");
    }

    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }

    fn emit(&self, event: Emit) {
        // With a bound source in the fixture, change it as soon as creation announces the new row.
        if matches!(event, Emit::Thread(_)) && self.source_generation.load(Ordering::Relaxed) != 0 {
            self.source_generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn session_choice_identity(&self) -> Option<String> {
        let generation = self.source_generation.load(Ordering::Relaxed);
        (generation != 0).then(|| generation.to_string())
    }

    fn session_sync(&self) -> Option<Arc<dyn session_sync::Backend>> {
        self.sync_lookups.fetch_add(1, Ordering::Relaxed);
        None
    }
}

impl api::Machine for TestHost {
    fn tool_path(&self) -> String {
        panic!("a refused request looked up the tool path");
    }

    fn home(&self) -> Result<PathBuf> {
        panic!("a refused request looked up the home directory");
    }

    fn agent(&self, _: SessionAgent) -> api::AgentInfo {
        panic!("a refused request looked up an agent");
    }

    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        panic!("a refused request looked up subscription policy");
    }
}

fn refused<T>(result: Result<T>, expected: &str) {
    assert_eq!(result.err().expect("request must be refused"), expected);
}

async fn assert_launches_refused(wb: &Workbench, api: &api::Api, id: &str, reason: &str) {
    let record = wb.record(id).expect("saved provider record");
    refused(
        wb.start(
            record,
            Say::User {
                text: "go".into(),
                images: vec![],
            },
            1,
            true,
            None,
        )
        .await,
        reason,
    );
    refused(api.send_files(id, "go", vec![], vec![]).await, reason);
    refused(
        wb.fork_thread(id, None).await,
        "This build cannot fork a session with a saved local provider.",
    );
    refused(
        wb.side_chat(id).await,
        "This build cannot start a side chat with a saved local provider.",
    );
    refused(
        wb.switch_agent(
            id,
            agent_switch::AgentSwitch {
                expected_agent: SessionAgent::Claude,
                agent: SessionAgent::Codex,
                model: None,
                effort: None,
                via: Via::Subscription,
                account: None,
            },
        )
        .await,
        "This build cannot switch the agent on a saved local provider.",
    );
    assert!(!wb.is_live(id));
    assert!(wb.store().sync_target(id).expect("sync target").is_none());
}

#[tokio::test]
async fn saved_provider_session_admission_and_launches() {
    // Use separate test processes for the real environment-backed flag.
    const CHILD: &str = "NEXUS_TEST_SAVED_PROVIDER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for enabled in ["0", "1", " TRUE ", "on", "false"] {
            let status = process::command(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "saved_provider_tests::saved_provider_session_admission_and_launches",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("NEXUS_BYO_PROVIDERS", enabled)
                .status()
                .expect("test subprocess");
            assert!(status.success(), "provider flag {enabled}");
        }
        return;
    }

    let root = std::env::temp_dir().join(format!("nexus-provider-session-{}", uuid::Uuid::new_v4()));
    let project_dir = root.join("project");
    std::fs::create_dir_all(&project_dir).expect("project directory");
    let mode = provider::LocalProvidersOptIn::from_env(|| Arc::new(Keys::default()));
    let host = Arc::new(TestHost {
        local_providers_opt_in: mode.clone(),
        ..Default::default()
    });
    let wb = Workbench::new(
        Store::open(&root.join("store.sqlite3")).expect("store"),
        host.clone(),
        Config {
            scope: session_scope::SessionScope::local_text().into(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let api = api::Api::new(wb.clone(), host.clone());
    let project = wb.add_project(&project_dir).expect("project");
    let new = || NewThread {
        project_id: project.id.clone(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::Default,
        effort: None,
        via: Via::Subscription,
        provider: Some(nexus_providers::InstanceId::new("key:test".into())),
        text: "first message".into(),
        images: vec![],
        worktree: None,
        parent: None,
        orchestration: false,
        name: None,
    };
    let guard = CreationGuard {
        identity: session_sync::Identity {
            scope: session_scope::SessionScope::local_text().into(),
            user: "test".into(),
        },
        expires_at: i64::MAX,
        generation: 1,
        sender: "test".into(),
        selection: None,
    };
    let before = host.sync_lookups.load(Ordering::Relaxed);
    if mode.enabled() {
        refused(wb.check_saved_provider_creation(Some(&guard)), DISABLED);
        host.source_generation.store(1, Ordering::Relaxed);
        let source = api::StartAuthority {
            scope: session_scope::SessionScope::local_text().into(),
            identity: host.session_choice_identity(),
        };
        refused(
            wb.create_thread_in_scope(new(), String::new(), Some(&source)).await,
            "The account changed before the session could start.",
        );
        assert!(
            wb.store()
                .threads(session_scope::SessionScope::local_text())
                .expect("list Local sessions")
                .is_empty()
        );
        assert_eq!(host.launches.load(Ordering::Relaxed), 0);
        host.source_generation.store(0, Ordering::Relaxed);

        let saved = wb.create_thread(new()).await.expect("saved provider session");
        assert_eq!(saved.provider, new().provider);
        assert_eq!(host.launches.load(Ordering::Relaxed), 1);
        wb.send(&saved.id, "fresh message").await.expect("send a fresh message");
        assert_eq!(host.launches.load(Ordering::Relaxed), 2);
        let events = wb.events(&saved.id, 0).expect("saved events");
        assert!(
            events
                .iter()
                .any(|entry| matches!(&entry.event, WbEvent::User { text, .. } if text == "fresh message"))
        );
        assert_eq!(
            events
                .iter()
                .filter(|entry| matches!(&entry.event, WbEvent::Error { text, .. } if text == MISSING))
                .count(),
            2,
        );
        assert_eq!(host.sync_lookups.load(Ordering::Relaxed), before);
        #[cfg(feature = "opencode")]
        prove_saved_server_model_updates(&wb, &saved.id);
        prove_credential_preparation(&project_dir).await;
        prove_bound_start_rollback(&root, &project_dir, new()).await;
        prove_keyed_launch_and_scope_stop(&root, new()).await;

        wb.set_scope(SIGNED_IN_SCOPE);
        refused(wb.create_thread(new()).await, SIGN_OUT);
        let mut signed_in = wb.record(&saved.id).expect("source");
        signed_in.scope = SIGNED_IN_SCOPE.into();
        signed_in.summary.id = "signed-in-provider-record".into();
        wb.store().insert_thread(&signed_in).expect("seed a legacy record");
        assert_launches_refused(&wb, &api, &signed_in.summary.id, SIGN_OUT).await;
    } else {
        refused(wb.create_thread(new()).await, DISABLED);
        refused(wb.check_saved_provider_creation(Some(&guard)), DISABLED);
    }
    wb.shutdown().await;
    drop(api);
    drop(wb);
    std::fs::remove_dir_all(root).expect("clean fixture");
}

#[cfg(feature = "opencode")]
fn prove_saved_server_model_updates(wb: &Workbench, source: &str) {
    let mut saved = wb.record(source).expect("key session fixture");
    saved.summary.id = "saved-server-session".into();
    saved.summary.agent = SessionAgent::Opencode;
    saved.summary.provider = Some(nexus_providers::InstanceId::new("server:test".into()));
    // The model on this record has since left the saved list.
    saved.summary.model = Some("team/original".into());
    wb.store().insert_thread(&saved).expect("server session");
    let id = &saved.summary.id;
    for model in [Some("zai/glm-5.3".into()), Some(String::new()), None] {
        refused(
            wb.update(
                id,
                ThreadPatch {
                    model: Some(model),
                    ..Default::default()
                },
            ),
            "Choose a model saved for this server.",
        );
        assert_eq!(
            wb.record(id).expect("server session").summary.model.as_deref(),
            Some("team/original")
        );
    }
    let patched = wb
        .update(
            id,
            ThreadPatch {
                model: Some(Some("team/replacement".into())),
                ..Default::default()
            },
        )
        .expect("saved choice");
    assert_eq!(patched.model.as_deref(), Some("team/replacement"));
}

#[test]
fn keyed_launch_notice_child() {
    let Some(path) = std::env::var_os(NOTICE_CHILD_STORE) else {
        return;
    };
    // A separate process must already be able to read the notice when the keyed child starts.
    assert_eq!(
        std::env::var("ANTHROPIC_API_KEY").as_deref(),
        Ok("fixture-provider-key")
    );
    let store = Store::open_reading(Path::new(&path)).expect("read-only store");
    let thread_id = std::env::var(NOTICE_CHILD_THREAD).expect("thread id");
    let events = store.events_after(&thread_id, 0).expect("stored events");
    assert!(
        events
            .iter()
            .any(|entry| matches!(&entry.event, WbEvent::Notice { text } if text == NOTICE))
    );
    std::fs::write(std::env::var_os(NOTICE_CHILD_READY).expect("ready marker"), "").expect("write ready marker");
    // Hold the keyed process open until the workbench stops it.
    std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).expect("read until stopped");
}

async fn prove_bound_start_rollback(root: &Path, project: &Path, mut new: NewThread) {
    let pause = Arc::new(LaunchPause::default());
    let host = Arc::new(TestHost {
        local_providers_opt_in: provider::LocalProvidersOptIn::Enabled(provider::LocalProviders {
            keys: Arc::new(Keys::default()),
        }),
        launch_pause: Some(pause.clone()),
        ..Default::default()
    });
    let wb = Workbench::new(
        Store::open(&root.join("concurrent-send.sqlite3")).expect("store"),
        host,
        Config {
            scope: session_scope::SessionScope::local_text().into(),
            ..Config::default()
        },
    )
    .expect("workbench");
    new.project_id = wb.add_project(project).expect("project").id;
    let source = api::StartAuthority {
        scope: session_scope::SessionScope::local_text().into(),
        identity: None,
    };
    let request = uuid::Uuid::new_v4().to_string();
    let id = creation_thread_id(&request).expect("creation id");
    let create = wb.create_thread_once_in_scope(new.clone(), &request, String::new(), None, Some(&source));
    let send = async {
        tokio::time::timeout(Duration::from_secs(5), pause.reached.notified())
            .await
            .expect("the first launch reached the host");
        assert!(wb.record(&id).is_ok());
        let result = wb.send(&id, "another draft").await;
        pause.resume.notify_one();
        result
    };
    let (create, send) = tokio::join!(create, send);
    refused(
        send,
        "This session is still starting. Send the message again when it opens.",
    );
    refused(create, "The local provider could not start. Check it in Settings.");
    assert!(
        wb.store()
            .threads(session_scope::SessionScope::local_text())
            .expect("list Local sessions")
            .is_empty()
    );

    let cancel = async {
        tokio::time::timeout(Duration::from_secs(5), pause.reached.notified())
            .await
            .expect("the next launch reached the host");
        wb.set_scope(SIGNED_IN_SCOPE);
        pause.resume.notify_one();
    };
    let (create, ()) = tokio::join!(wb.create_thread_in_scope(new, String::new(), Some(&source)), cancel);
    refused(create, "The account changed before the session could start.");
    assert!(
        wb.store()
            .threads(session_scope::SessionScope::local_text())
            .expect("list Local sessions")
            .is_empty()
    );
    wb.shutdown().await;
}

async fn prove_keyed_launch_and_scope_stop(root: &Path, mut new: NewThread) {
    let root = root.join("keyed-spawn");
    std::fs::create_dir(&root).expect("fixture directory");
    let host = Arc::new(TestHost {
        local_providers_opt_in: provider::LocalProvidersOptIn::Enabled(provider::LocalProviders {
            keys: Arc::new(Keys::default()),
        }),
        keyed_spawn: Some(root.clone()),
        ..Default::default()
    });
    let wb = Workbench::new(
        Store::open(&root.join("store.sqlite3")).expect("store"),
        host.clone(),
        Config {
            scope: session_scope::SessionScope::local_text().into(),
            ..Config::default()
        },
    )
    .expect("workbench");
    new.project_id = wb.add_project(&root).expect("project").id;

    let saved = wb.create_thread(new).await.expect("keyed session");
    let ready = root.join("child-ready");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("keyed child saw the persisted MCP notice");
    let events = wb.events(&saved.id, 0).expect("stored events");
    let notice = events
        .iter()
        .position(|entry| matches!(&entry.event, WbEvent::Notice { text } if text == NOTICE));
    let spawn = events
        .iter()
        .position(|entry| matches!(entry.event, WbEvent::Session { .. }));
    assert!(notice.expect("MCP notice") < spawn.expect("session start event"));

    let live = wb.live(&saved.id).expect("keyed process");
    let mut exited = live.exited.clone();
    wb.set_scope(SIGNED_IN_SCOPE);
    assert_eq!(live.state().stop, Some(StopReason::Stop));
    assert_eq!(host.revoked.load(Ordering::Relaxed), 1);
    tokio::time::timeout(Duration::from_secs(10), exited.wait_for(|done| *done))
        .await
        .expect("scope change stopped the child")
        .expect("exit status");
    wb.shutdown().await;
}

async fn prove_credential_preparation(cwd: &Path) {
    use nexus_providers::{ApiKeySetting, InstanceId, KeySource, KeyVendor, ProviderSettings, ServerSetting};

    let keys = Arc::new(Keys::default());
    let source: Arc<dyn KeySource> = keys.clone();
    let api_key = |id, vendor| ApiKeySetting {
        id: InstanceId::new(id),
        vendor,
        key: KeyRef::new("fixture-reference".into()),
    };
    let settings = ProviderSettings {
        api_keys: vec![
            api_key("key:claude".into(), KeyVendor::Anthropic),
            api_key("key:codex".into(), KeyVendor::Openai),
        ],
        servers: vec![ServerSetting {
            id: InstanceId::new("server:test".into()),
            name: "Loopback".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            key: Some(KeyRef::new("server-reference".into())),
            models: vec!["test-model".into()],
        }],
        ..Default::default()
    };
    let request = |id: &str, agent| LaunchRequest {
        thread_id: "thread".into(),
        agent,
        cwd: cwd.into(),
        model: None,
        via: Via::Subscription,
        provider: Some(InstanceId::new(id.into())),
        required_payer: None,
        account_dir: None,
        args: Vec::new(),
        scope: session_scope::SessionScope::local_text().into(),
        prompt_counts: None,
    };
    async fn prepare(
        request: &LaunchRequest,
        settings: &ProviderSettings,
        keys: &Arc<dyn KeySource>,
        servers: &[mcp_servers::McpServer],
        check_owner: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Launch> {
        let mut native = local_provider::NativeMcp::default();
        native.include("native-db");
        let env = HashMap::from([
            ("HOME".into(), request.cwd.display().to_string()),
            ("ANTHROPIC_API_KEY".into(), "old-credential".into()),
            ("OPENAI_API_KEY".into(), "old-credential".into()),
            ("NEXUS_OPENCODE_API_KEY".into(), "old-credential".into()),
        ]);
        let local = provider::LocalProviders { keys: keys.clone() };
        local_provider::prepare(
            request,
            "fixture-agent".into(),
            env,
            local_provider::Source {
                settings,
                local: &local,
                check_owner,
            },
            local_provider::Mcp {
                program: None,
                servers,
                values: &Default::default(),
                native,
            },
            |_, _| panic!("there is no launcher program"),
        )
        .await
    }

    let permit = || Ok(());
    let claude = request("key:claude", SessionAgent::Claude);
    let codex = request("key:codex", SessionAgent::Codex);
    let wrong_agent = request("key:claude", SessionAgent::Codex);
    assert!(prepare(&wrong_agent, &settings, &source, &[], &permit).await.is_err());
    let collision = serde_json::from_value::<Vec<mcp_servers::McpServer>>(serde_json::json!([{
        "name": "db", "command": "db", "secrets": ["NEXUS_API_KEY"]
    }]))
    .expect("a server with an unset declaration");
    assert!(prepare(&codex, &settings, &source, &collision, &permit).await.is_err());
    let checks = AtomicUsize::new(0);
    let sign_in_during_prepare = || {
        if checks.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(())
        } else {
            Err(SIGN_OUT.into())
        }
    };
    refused(
        prepare(&claude, &settings, &source, &[], &sign_in_during_prepare).await,
        SIGN_OUT,
    );
    assert_eq!(keys.0.load(Ordering::Relaxed), 0);
    for (i, (request, variable)) in [
        (&claude, "ANTHROPIC_API_KEY"),
        (&codex, "NEXUS_API_KEY"),
        (&claude, "ANTHROPIC_API_KEY"),
    ]
    .into_iter()
    .enumerate()
    {
        let launch = prepare(request, &settings, &source, &[], &permit)
            .await
            .expect("prepare keyed launch");
        let secret = format!("byo-fixture-credential-generation-{i}");
        assert_eq!(launch.spawn.env.get(variable), Some(&secret));
        assert!(!launch.spawn.env.values().any(|value| value == "old-credential"));
        assert!(!format!("{launch:?}").contains(&secret));
        assert!(!launch.spawn.args.iter().any(|arg| arg.contains(&secret)));
        assert!(launch.nexus_session.is_none());
        assert_eq!(
            launch
                .local_provider
                .as_ref()
                .and_then(local_provider::Run::mcp_warning)
                .as_deref(),
            Some("These MCP servers may receive the provider key: native-db.")
        );
    }
    #[cfg(feature = "opencode")]
    {
        let server = request("server:test", SessionAgent::Opencode);
        let keyed = prepare(&server, &settings, &source, &[], &permit)
            .await
            .expect("prepare keyed server");
        assert_eq!(
            keyed.spawn.env.get("NEXUS_OPENCODE_API_KEY").map(String::as_str),
            Some("byo-fixture-credential-generation-3")
        );
        let mut settings = settings;
        settings.servers[0].key = None;
        let keyless = prepare(&server, &settings, &source, &[], &permit)
            .await
            .expect("prepare keyless server");
        assert!(!keyless.spawn.env.contains_key("NEXUS_OPENCODE_API_KEY"));
        assert!(
            keyless
                .local_provider
                .as_ref()
                .and_then(local_provider::Run::mcp_warning)
                .is_none()
        );
        assert_eq!(keys.0.load(Ordering::Relaxed), 4);
    }
}
