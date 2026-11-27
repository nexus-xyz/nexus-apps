//! Actual Gateway transport composed with the runner and a stand-in agent.

use super::*;
use crate::event::SessionAgent;

pub(super) struct ControlHost {
    pub(super) backend: Arc<dyn Backend>,
    pub(super) agent: std::path::PathBuf,
    pub(super) memory: std::path::PathBuf,
}

/// Fail only the receipt transport: commands still reach the real Gateway and agent.
struct HoldReceipts {
    backend: Arc<GatewayBackend>,
    held: AtomicBool,
}

impl Backend for HoldReceipts {
    fn identity(&self) -> &Identity {
        self.backend.identity()
    }
    fn allow_local_cache(&self) -> bool {
        self.backend.allow_local_cache()
    }
    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        self.backend.policy()
    }
    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        self.backend.head(thread)
    }
    fn key(
        &self,
        thread: &str,
        epoch: Option<u64>,
        policy: ContentPolicy,
    ) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        self.backend.key(thread, epoch, policy)
    }
    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        self.backend.append(batch)
    }
    fn delete(&self, thread: &str) -> BoxFuture<'_, Result<()>> {
        self.backend.delete(thread)
    }
    fn supports_control(&self) -> bool {
        true
    }
    fn control_lease(&self, thread: &str, enabled: bool) -> BoxFuture<'_, Result<()>> {
        self.backend.control_lease(thread, enabled)
    }
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<crate::session_sync::control::Command>>> {
        self.backend.control_commands()
    }
    fn control_claim(&self, thread: &str, id: &str) -> BoxFuture<'_, Result<bool>> {
        self.backend.control_claim(thread, id)
    }
    fn control_finish(&self, thread: &str, id: &str, completed: bool) -> BoxFuture<'_, Result<()>> {
        if self.held.load(Ordering::SeqCst) {
            return Box::pin(async { Err("receipt connection unavailable".into()) });
        }
        self.backend.control_finish(thread, id, completed)
    }
}

impl Host for ControlHost {
    fn computer_choices(
        &self,
        agents: Vec<SessionAgent>,
        _: Option<std::path::PathBuf>,
    ) -> BoxFuture<'static, Result<crate::computer::settings::NativeChoices>> {
        use crate::computer::{AgentChoices, Availability, BillingPayer, RouteChoices};
        Box::pin(async move {
            Ok(crate::computer::settings::NativeChoices {
                agents: agents
                    .into_iter()
                    .map(|agent| AgentChoices {
                        agent,
                        name: agent.as_str().into(),
                        routes: vec![RouteChoices {
                            via: crate::event::Via::Nexus,
                            payer: BillingPayer::Gateway,
                            label: "Test Gateway".into(),
                            detail: None,
                            state: Availability::Usable,
                            models: vec![],
                            default_model: None,
                        }],
                    })
                    .collect(),
                logins: vec![],
            })
        })
    }
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        Some(self.backend.clone())
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        let (agent, memory) = (self.agent.clone(), self.memory.to_string_lossy().into_owned());
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                model: None,
                nexus_session: None,
                local_run: None,
                local_provider: None,
                spawn: crate::process::Spawn {
                    program: agent,
                    args: request.args,
                    cwd: request.cwd,
                    env: HashMap::from([("FAKE_CLAUDE_DIR".into(), memory)]),
                },
            })
        })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, usage: &crate::event::Usage) -> Option<f64> {
        Some(usage.output_tokens as f64 * 0.0001)
    }
    fn top_price(&self, _: &crate::event::Usage) -> Option<f64> {
        Some(0.001)
    }
    fn emit(&self, _: Emit) {}
}

pub(super) async fn control_wait(
    wb: &Workbench,
    id: &str,
    done: impl Fn(&[crate::event::StoredEvent]) -> bool,
) -> Vec<crate::event::StoredEvent> {
    for _ in 0..400 {
        let events = wb.events(id, 0).expect("events");
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("agent did not reach the expected state: {:?}", wb.events(id, 0));
}

fn phone_action(thread: &str, phone: (&str, &DeviceKey), runner: (&str, &DeviceKey), action: Value) -> Value {
    let id = uuid::Uuid::new_v4().to_string();
    let key = e2e::ContentKey::generate(0);
    let expires = crate::now_ms() + 60_000;
    let plaintext = serde_json::to_vec(&json!({"version":1,"threadId":thread,"action":action})).expect("payload");
    let locked = e2e::b64u(&key.lock_for(&id, runner.0, &runner.1.encryption_key()).expect("lock"));
    let sealed = e2e::b64u(&key.seal_event(&id, 0, &plaintext).expect("seal"));
    let message = format!(
        "nexus-code-command-v1\n{thread}\n{id}\n{}\n{}\n{expires}\n{locked}\n{sealed}",
        phone.0, runner.0
    );
    json!({"id":id,"senderDeviceId":phone.0,"runnerDeviceId":runner.0,"expiresAt":expires,
        "lockedKey":locked,"sealed":sealed,"signature":e2e::b64u(&phone.1.sign(&message))})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_gateway_delivers_phone_prompts_approval_answers_and_stop_to_the_runner() {
    use crate::event::{PermissionMode, SessionAgent, TurnStatus, Via};
    let Some(mut gateway) = boot().await else { return };
    let root = std::env::temp_dir().join(format!("nexus-phone-control-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("repo")).expect("repo");
    std::fs::create_dir_all(root.join("memory")).expect("agent state");
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Phone control","email":"phone-control@example.test"}),
        )
        .await
        .expect("sign up");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    client
        .put("/v1/orgs/settings", json!({"sessionContent":"user"}))
        .await
        .expect("policy");
    let computer = DeviceKey::generate();
    let runner = register(&client, &computer, "Computer").await;
    let phone = DeviceKey::generate();
    let sender = register(&client, &phone, "Phone").await;
    approve(&client, (&runner, &computer), &sender, &phone).await;
    let scope = format!("{}|phone-control", gateway.url());
    let backend = Arc::new(
        GatewayBackend::connect(
            client.clone(),
            scope.clone(),
            DeviceKey::from_secret_bytes(&computer.secret_bytes()),
            Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("keys")),
            true,
        )
        .await
        .expect("backend"),
    );
    let agent = std::env::var_os("NEXUS_SYNC_E2E_AGENT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .expect("test binary")
                .parent()
                .expect("deps")
                .parent()
                .expect("debug")
                .join("fake-claude")
        });
    assert!(
        agent.is_file(),
        "Build the fake agent with cargo build -p nexus-workbench --bins, or set NEXUS_SYNC_E2E_AGENT"
    );
    let receipts = Arc::new(HoldReceipts {
        backend,
        held: AtomicBool::new(false),
    });
    let host = Arc::new(ControlHost {
        backend: receipts.clone(),
        agent,
        memory: root.join("memory"),
    });
    let wb = Workbench::new(
        Store::open(&root.join("workbench.sqlite3")).expect("store"),
        host.clone(),
        Config {
            scope: scope.clone(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let project = wb.add_project(&root.join("repo")).expect("project");
    let local = wb
        .create_thread(crate::NewThread {
            name: None,
            project_id: project.id.clone(),
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Nexus,
            provider: None,
            text: "Hello".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        })
        .await
        .expect("local session");
    let turn_count = |events: &[crate::event::StoredEvent]| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnEnd { .. }))
            .count()
    };
    control_wait(&wb, &local.id, |events| turn_count(events) == 1).await;
    let synced = wb.sync_sessions_once().await;
    assert!(synced.failures.is_empty(), "initial sync: {synced:?}");
    let thread = wb
        .store()
        .sync_target(&local.id)
        .expect("target")
        .expect("owned")
        .sync_id;
    wb.set_code_control(true).expect("local opt-in");
    wb.control_sessions_once().await;
    assert_eq!(
        client
            .get(&format!("/v1/threads/{thread}/control"))
            .await
            .expect("lease")["enabled"],
        true
    );
    let send = phone_action(
        &thread,
        (&sender, &phone),
        (&runner, &computer),
        json!({"kind":"send","text":"tool"}),
    );
    client
        .post(&format!("/v1/threads/{thread}/commands"), send.clone())
        .await
        .expect("encrypted prompt");
    wb.control_sessions_once().await;
    control_wait(&wb, &local.id, |events| {
        events
            .iter()
            .any(|event| matches!(event.event, WbEvent::Approval { .. }))
    })
    .await;
    assert!(wb.sync_sessions_once().await.failures.is_empty());
    let opened = read_as(&client, &thread, (&sender, &phone)).await;
    let request = opened
        .iter()
        .find_map(
            |(event, _)| match serde_json::from_str::<WbEvent>(&event.event_json).expect("event") {
                WbEvent::Approval { request_id, .. } => Some(request_id),
                _ => None,
            },
        )
        .expect("phone decrypts the approval request");
    let approval = phone_action(
        &thread,
        (&sender, &phone),
        (&runner, &computer),
        json!({"kind":"approve","requestId":request,"decision":"allow","message":null,"answers":null}),
    );
    client
        .post(&format!("/v1/threads/{thread}/commands"), approval.clone())
        .await
        .expect("encrypted approval");
    wb.control_sessions_once().await;
    control_wait(&wb, &local.id, |events| turn_count(events) == 2).await;
    let slow = phone_action(
        &thread,
        (&sender, &phone),
        (&runner, &computer),
        json!({"kind":"send","text":"slow"}),
    );
    client
        .post(&format!("/v1/threads/{thread}/commands"), slow.clone())
        .await
        .expect("next prompt");
    wb.control_sessions_once().await;
    control_wait(&wb, &local.id, |events| {
        events
            .iter()
            .filter(|event| matches!(event.event, WbEvent::TurnStart { .. }))
            .count()
            == 3
    })
    .await;
    let stop = phone_action(&thread, (&sender, &phone), (&runner, &computer), json!({"kind":"stop"}));
    client
        .post(&format!("/v1/threads/{thread}/commands"), stop.clone())
        .await
        .expect("stop");
    wb.control_sessions_once().await;
    let events = control_wait(&wb, &local.id, |events| turn_count(events) == 3).await;
    assert!(matches!(
        events.last().map(|event| &event.event),
        Some(WbEvent::TurnEnd {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    let prompts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            WbEvent::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, ["Hello", "tool", "slow"]);
    for command in [&send, &approval, &slow, &stop] {
        let id = command["id"].as_str().expect("id");
        assert_eq!(
            client
                .get(&format!("/v1/threads/{thread}/commands/{id}"))
                .await
                .expect("receipt")["state"],
            "completed"
        );
    }
    client
        .post(&format!("/v1/threads/{thread}/commands"), send)
        .await
        .expect("identical retry");
    wb.control_sessions_once().await;
    assert_eq!(
        wb.events(&local.id, 0).expect("events").len(),
        events.len(),
        "retry cannot execute the prompt twice"
    );
    // A connection outage leaves a full local receipt page pending after real actions have run. Deleting that
    // Gateway session makes their delivery permanently impossible, while the computer retains its dedup rows.
    let stale_target = wb.store().sync_target(&local.id).expect("target").expect("owned");
    receipts.held.store(true, Ordering::SeqCst);
    let mut stale_ids = Vec::new();
    for _ in 0..20 {
        let command = phone_action(&thread, (&sender, &phone), (&runner, &computer), json!({"kind":"stop"}));
        stale_ids.push(command["id"].as_str().expect("id").to_string());
        client
            .post(&format!("/v1/threads/{thread}/commands"), command)
            .await
            .expect("stop while receipt transport is down");
        wb.control_sessions_once().await;
    }
    assert_eq!(
        wb.store()
            .control_results(receipts.identity(), None)
            .expect("pending receipts")
            .len(),
        20
    );
    client
        .delete(&format!("/v1/threads/{thread}"))
        .await
        .expect("delete Gateway session");
    receipts.held.store(false, Ordering::SeqCst);
    wb.control_sessions_once().await;
    assert!(
        wb.store()
            .control_results(receipts.identity(), None)
            .expect("retired receipts")
            .is_empty(),
        "deleted targets cannot monopolize the first receipt page"
    );

    let fresh = wb
        .create_thread(crate::NewThread {
            name: None,
            project_id: project.id,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Nexus,
            provider: None,
            text: "A fresh session".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        })
        .await
        .expect("fresh session");
    control_wait(&wb, &fresh.id, |events| turn_count(events) == 1).await;
    wb.sync_sessions_once().await;
    let fresh_thread = wb
        .store()
        .sync_target(&fresh.id)
        .expect("target")
        .expect("owned")
        .sync_id;
    wb.control_sessions_once().await;
    let continued = phone_action(
        &fresh_thread,
        (&sender, &phone),
        (&runner, &computer),
        json!({"kind":"send","text":"After receipt recovery"}),
    );
    let continued_id = continued["id"].as_str().expect("id").to_string();
    client
        .post(&format!("/v1/threads/{fresh_thread}/commands"), continued)
        .await
        .expect("later prompt");
    wb.control_sessions_once().await;
    control_wait(&wb, &fresh.id, |events| turn_count(events) == 2).await;
    assert_eq!(
        client
            .get(&format!("/v1/threads/{fresh_thread}/commands/{continued_id}"))
            .await
            .expect("later receipt")["state"],
        "completed"
    );
    let before_reopen = wb.events(&fresh.id, 0).expect("events").len();
    wb.shutdown().await;
    let reopened = Workbench::new(
        Store::open(&root.join("workbench.sqlite3")).expect("reopen"),
        host,
        Config {
            scope,
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("reopened workbench");
    for id in stale_ids {
        assert!(
            !reopened
                .store()
                .control_reserve(&stale_target.identity, &stale_target.sync_id, &id)
                .expect("durable dedup")
        );
    }
    reopened.control_sessions_once().await;
    assert_eq!(
        reopened.events(&fresh.id, 0).expect("events after reopen").len(),
        before_reopen,
        "retiring delivery never removes replay protection"
    );
    reopened.set_code_control(false).expect("disable locally");
    reopened.control_sessions_once().await;
    assert_eq!(
        client
            .get(&format!("/v1/threads/{fresh_thread}/control"))
            .await
            .expect("disabled")["enabled"],
        false
    );
    reopened.shutdown().await;
    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
