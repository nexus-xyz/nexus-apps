//! Real mailbox delivery, native auth probes and native model ACK semantics composed together.
use super::*;
use crate::computer::settings::{NativeChoices, check_launch_payer, native_choices};
use crate::event::{Via, WbEvent};
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
struct PreparedLaunch {
    via: Via,
    payer: Option<c::BillingPayer>,
}

struct SettingsHost {
    inner: Arc<super::super::control::ControlHost>,
    client: Client,
    program: PathBuf,
    env: HashMap<String, String>,
    launches: Arc<Mutex<Vec<PreparedLaunch>>>,
    delay: Arc<AtomicBool>,
    launch_delay: Arc<AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}
impl Host for SettingsHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        Some(self.inner.backend.clone())
    }
    fn computer_choices(
        &self,
        agents: Vec<SessionAgent>,
        account: Option<PathBuf>,
    ) -> BoxFuture<'static, Result<NativeChoices>> {
        let (client, program, env, delay, entered, resume) = (
            self.client.clone(),
            self.program.clone(),
            self.env.clone(),
            self.delay.clone(),
            self.entered.clone(),
            self.resume.clone(),
        );
        Box::pin(async move {
            if delay.swap(false, Ordering::SeqCst) {
                entered.notify_one();
                resume.notified().await;
            }
            let infos = agents
                .into_iter()
                .map(|agent| AgentInfo {
                    agent,
                    path: (agent == SessionAgent::Claude).then(|| program.to_string_lossy().into_owned()),
                    version: None,
                    models: None,
                    problem: None,
                })
                .collect();
            native_choices(Some(client), infos, env, account).await
        })
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        let (inner, program, env, launches, delay, entered, resume) = (
            self.inner.clone(),
            self.program.clone(),
            self.env.clone(),
            self.launches.clone(),
            self.launch_delay.clone(),
            self.entered.clone(),
            self.resume.clone(),
        );
        Box::pin(async move {
            let self_env = env.clone();
            check_launch_payer(&request, program, env).await?;
            if delay.swap(false, Ordering::SeqCst) {
                entered.notify_one();
                resume.notified().await;
            }
            launches.lock().expect("launches").push(PreparedLaunch {
                via: request.via,
                payer: request.required_payer,
            });
            let mut launch = inner.launch(request).await?;
            launch.spawn.env.extend(self_env);
            launch.spawn.env.insert("FAKE_CLAUDE_IGNORE_CHANGES".into(), "1".into());
            Ok(launch)
        })
    }
    fn emit(&self, event: Emit) {
        self.inner.emit(event)
    }
    fn ended(&self, id: String, end: ProcessEnd) -> BoxFuture<'static, bool> {
        self.inner.ended(id, end)
    }
    fn price(&self, model: &str, usage: &crate::event::Usage) -> Option<f64> {
        self.inner.price(model, usage)
    }
}
async fn rpc(
    api: &Api,
    client: &Client,
    info: &c::Info,
    phone: (&str, &DeviceKey),
    desktop: (&str, &DeviceKey),
    operation: Value,
) -> Value {
    let id = submit(client, info, request(info, phone, desktop, operation)).await;
    api.computer_pass().await;
    result(client, info, &id, phone).await
}
async fn status(wb: &Workbench, id: &str, wanted: crate::event::ThreadStatus) {
    super::super::control::control_wait(wb, id, |_| wb.record(id).is_ok_and(|r| r.summary.status == wanted)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_settings_preserve_billing_and_native_pending_ack() {
    let Some(mut gateway) = boot().await else { return };
    let root = std::env::temp_dir().join(format!("nexus-computer-settings-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("repo")).expect("repo");
    std::fs::create_dir_all(root.join("memory")).expect("memory");
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Settings RPC","email":"settings-rpc@test.example"}),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    client
        .put(
            "/v1/orgs/settings",
            json!({"sessionContent":"user","agentSubscriptions":true}),
        )
        .await
        .expect("policy");
    let desktop_key = DeviceKey::generate();
    let desktop = register(&client, &desktop_key, "Desktop").await;
    let phone_key = DeviceKey::generate();
    let phone = register(&client, &phone_key, "Phone").await;
    approve(&client, (&desktop, &desktop_key), &phone, &phone_key).await;
    let scope = format!("{}|settings-rpc", gateway.url());
    let backend = Arc::new(
        GatewayBackend::connect(
            client.clone(),
            scope.clone(),
            DeviceKey::from_secret_bytes(&desktop_key.secret_bytes()),
            Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("keys")),
            true,
        )
        .await
        .expect("backend"),
    );
    let agent = std::env::current_exe()
        .expect("binary")
        .parent()
        .expect("deps")
        .parent()
        .expect("debug")
        .join("fake-claude");
    assert!(agent.exists());
    let program = root.join("agent");
    crate::process::write_fake_script(
        &program,
        r#"#!/bin/sh
if [ "$1" = auth ]; then /bin/cat "$NEXUS_TEST_AUTH"; exit; fi
case " $* " in
  *" --no-session-persistence "*) ;;
  *) exec "$NEXUS_TEST_AGENT" "$@" ;;
esac
read -r line
printf '%s\n' '{"type":"control_response","response":{"request_id":"models","subtype":"success","response":{"models":[{"value":"fixture-model","displayName":"Fixture model","supportedEffortLevels":["low","high"]}]}}}'
read -r extra
"#,
    );
    let auth = root.join("auth.json");
    std::fs::write(
        &auth,
        r#"{"loggedIn":true,"authMethod":"claude.ai","email":"NEVER_SEND@example.test"}"#,
    )
    .expect("plan status");
    let env = HashMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        ("HOME".into(), root.to_string_lossy().into_owned()),
        ("NEXUS_TEST_AUTH".into(), auth.to_string_lossy().into_owned()),
        ("NEXUS_TEST_AGENT".into(), agent.to_string_lossy().into_owned()),
    ]);
    let host = Arc::new(SettingsHost {
        inner: Arc::new(super::super::control::ControlHost {
            backend: backend.clone(),
            agent: program.clone(),
            memory: root.join("memory"),
        }),
        client: client.clone(),
        program,
        env,
        launches: Arc::default(),
        delay: Arc::new(AtomicBool::new(false)),
        launch_delay: Arc::new(AtomicBool::new(false)),
        entered: Arc::default(),
        resume: Arc::default(),
    });
    let path = root.join("workbench.sqlite3");
    let wb = Workbench::new(
        Store::open(&path).expect("store"),
        host.clone(),
        Config {
            scope: scope.clone(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let project = wb.add_project(&root.join("repo")).expect("project");
    let machine = Arc::new(AgentMachine(agent, root.clone()));
    let api = Api::new(wb.clone(), machine.clone());
    wb.set_code_control(true).expect("opt in");
    let info = api.computer_info().await.expect("info");
    backend.presence(info.clone(), true).await.expect("presence");
    let describe = || json!({"kind":"describeProjects"});
    let phone = (&phone[..], &phone_key);
    let desktop = (&desktop[..], &desktop_key);
    let projects = rpc(&api, &client, &info, phone, desktop, describe()).await;
    assert_eq!(projects["agents"][0]["routes"][1]["state"], "usable");
    assert_eq!(projects["agents"][0]["routes"][2]["state"], "unavailable");
    assert_eq!(
        projects["agents"][2]["routes"].as_array().expect("nexus routes").len(),
        1
    );
    assert!(!projects.to_string().contains("NEVER_SEND"));
    let selected = json!({"via":"subscription","payer":"plan","model":"anthropic/fixture-model","effort":"high"});
    let start=rpc(&api,&client,&info,phone,desktop,json!({"kind":"startSession","projectId":project.id,"revision":projects["projects"][0]["revision"],"text":"slow ignore-changes","selection":selected})).await;
    assert_eq!(start["kind"], "created", "{start}");
    let sync = start["threadId"].as_str().expect("sync").to_string();
    let target = wb
        .store()
        .control_by_sync(backend.identity(), &sync)
        .expect("binding")
        .expect("owned");
    status(&wb, &target.local, crate::event::ThreadStatus::Running).await;
    assert_eq!(
        host.launches.lock().expect("launches").as_slice(),
        &[PreparedLaunch {
            via: Via::Subscription,
            payer: Some(c::BillingPayer::Plan)
        }]
    );
    let snapshot = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":sync}),
    )
    .await;
    assert_eq!(snapshot["session"]["payer"]["kind"], "plan");
    let updated=rpc(&api,&client,&info,phone,desktop,json!({"kind":"updateSession","threadId":sync,"revision":snapshot["session"]["revision"],"settings":{"model":"anthropic/fixture-model","effort":"low"}})).await;
    assert_eq!(updated["kind"], "sessionUpdated", "{updated}");
    assert_eq!(
        updated["session"]["effort"], "high",
        "unacknowledged effort remains saved"
    );
    assert_eq!(updated["session"]["pending"]["effort"], "low");
    let stale=rpc(&api,&client,&info,phone,desktop,json!({"kind":"updateSession","threadId":sync,"revision":snapshot["session"]["revision"],"settings":{"model":null,"effort":null}})).await;
    assert_eq!(stale["kind"], "sessionChanged");
    let foreign = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(foreign["kind"], "refused");
    wb.stop(&target.local).expect("stop");
    status(&wb, &target.local, crate::event::ThreadStatus::Interrupted).await;
    let settled = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":sync}),
    )
    .await;
    assert_eq!(
        settled["session"]["effort"], "low",
        "normal native finish saves pending settings"
    );
    assert!(settled["session"]["pending"].is_null());

    // The paired phone can read this session’s checkpoints. A later, unrecorded edit stays on Desktop.
    wb.checkpoint_written(&target.local).await;
    let dir = wb
        .workdir(&wb.record(&target.local).expect("session"))
        .expect("session folder");
    assert!(!dir.join(".git").exists());
    let git = |args: &[&str]| crate::git::git(&dir, args).expect("fixture git");
    git(&["init"]);
    let file = "changes with spaces.txt";
    let prefix = crate::changes_source::checkpoint_prefix(&target.local);
    for (seq, text) in [(0, "before\n"), (1, "recorded after\n")] {
        std::fs::write(dir.join(file), text).expect("file");
        git(&["add", "--", file]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "checkpoint",
        ]);
        git(&["update-ref", &format!("{prefix}{seq:010}"), "HEAD"]);
    }
    std::fs::write(dir.join(file), "unrecorded after\n").expect("file");
    let changes = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"readSessionChanges","threadId":sync}),
    )
    .await;
    assert_eq!(changes["kind"], "sessionChanges", "{changes}");
    assert_eq!(changes["threadId"], sync);
    assert_eq!(changes["files"], json!([file]));
    let patch = changes["patch"].as_str().expect("patch");
    assert!(
        patch.contains("+recorded after") && !patch.contains("unrecorded after"),
        "{patch}"
    );
    let foreign = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"readSessionChanges","threadId":uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(foreign["kind"], "refused");
    std::fs::remove_dir_all(dir.join(".git")).expect("remove fixture checkpoints");
    std::fs::remove_file(dir.join(file)).expect("remove fixture file");

    // Reopen retains the explicit payer. A changed CLI sign-in cannot silently bill an API key.
    std::fs::write(&auth, r#"{"loggedIn":true,"authMethod":"api_key"}"#).expect("key status");
    drop(api);
    drop(wb);
    let wb = Workbench::new(
        Store::open(&path).expect("reopen"),
        host.clone(),
        Config {
            scope: scope.clone(),
            ..Config::default()
        },
    )
    .expect("workbench");
    assert_eq!(
        wb.store()
            .computer_required_payer(&target.local, &scope)
            .expect("durable payer"),
        Some(c::BillingPayer::Plan)
    );
    wb.send(&target.local, "A changed login must not change who pays")
        .await
        .expect("record prompt");
    status(&wb, &target.local, crate::event::ThreadStatus::Error).await;
    assert_eq!(
        host.launches.lock().expect("launches").len(),
        1,
        "no resumed provider process"
    );
    assert!(
        wb.events(&target.local, 0)
            .expect("events")
            .iter()
            .any(|e| matches!(&e.event,WbEvent::Error{text,..} if text.contains("sign-in changed")))
    );
    let api = Api::new(wb.clone(), machine);
    let key_projects = rpc(&api, &client, &info, phone, desktop, describe()).await;
    assert_eq!(key_projects["agents"][0]["routes"][1]["state"], "unavailable");
    assert_eq!(key_projects["agents"][0]["routes"][2]["state"], "usable");
    let old_start=rpc(&api,&client,&info,phone,desktop,json!({"kind":"startSession","projectId":project.id,"revision":projects["projects"][0]["revision"],"text":"No payer fallback","selection":selected})).await;
    assert_eq!(old_start["kind"], "choicesChanged");
    let api_key_session = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":sync}),
    )
    .await;
    assert_eq!(
        api_key_session["session"]["payer"]["kind"], "plan",
        "explicit payer remains fixed when current CLI login changes"
    );
    let legacy = wb
        .create_thread(crate::NewThread {
            name: None,
            project_id: project.id.clone(),
            agent: SessionAgent::Claude,
            model: Some("anthropic/fixture-model".into()),
            permission_mode: crate::event::PermissionMode::Default,
            effort: None,
            via: Via::Subscription,
            provider: None,
            text: "A preexisting direct account remains accurate".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        })
        .await
        .expect("ordinary native creation");
    status(&wb, &legacy.id, crate::event::ThreadStatus::Idle).await;
    let legacy = wb.store().sync_target(&legacy.id).expect("binding").expect("owned");
    let legacy = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":legacy.sync_id}),
    )
    .await;
    assert_eq!(
        legacy["session"]["payer"]["kind"], "apiKey",
        "a legacy direct route is not falsely described as a subscription plan"
    );
    std::fs::write(&auth, r#"{"loggedIn":false}"#).expect("signed out");
    let out = rpc(&api, &client, &info, phone, desktop, describe()).await;
    assert_eq!(out["agents"][0]["routes"][1]["state"], "unavailable");
    std::fs::write(&auth, "unrecognized status").expect("unknown");
    let unknown = rpc(&api, &client, &info, phone, desktop, describe()).await;
    assert_eq!(unknown["agents"][0]["routes"][1]["state"], "unknown");
    // The login changes after the host's initial check, while launch preparation is awaiting transport.
    std::fs::write(&auth, r#"{"loggedIn":true,"authMethod":"claude.ai"}"#).expect("plan status");
    let fresh = rpc(&api, &client, &info, phone, desktop, describe()).await;
    host.launch_delay.store(true, Ordering::SeqCst);
    let delayed=submit(&client,&info,request(&info,phone,desktop,json!({"kind":"startSession","projectId":project.id,"revision":fresh["projects"][0]["revision"],"text":"No process under a changed payer","selection":selected}))).await;
    let pass = api.clone();
    let processing = tokio::spawn(async move { pass.computer_pass().await });
    tokio::time::timeout(Duration::from_secs(5), host.entered.notified())
        .await
        .expect("initial launch check passed");
    std::fs::write(&auth, r#"{"loggedIn":true,"authMethod":"api_key"}"#).expect("changed during transport");
    host.resume.notify_one();
    processing.await.expect("worker");
    let delayed = result(&client, &info, &delayed, phone).await;
    assert_eq!(delayed["kind"], "created");
    assert_eq!(
        delayed["launch"], "error",
        "final spawn admission refuses changed payer"
    );
    let delayed = wb
        .store()
        .control_by_sync(backend.identity(), delayed["threadId"].as_str().expect("sync"))
        .expect("binding")
        .expect("target");
    assert!(
        !wb.events(&delayed.local, 0)
            .expect("events")
            .iter()
            .any(|e| matches!(e.event, WbEvent::Session { .. })),
        "no agent process started after the delayed preparation"
    );
    client
        .put("/v1/orgs/settings", json!({"agentSubscriptions":false}))
        .await
        .expect("deny plans");
    let disabled = rpc(&api, &client, &info, phone, desktop, describe()).await;
    assert_eq!(disabled["agents"][0]["routes"][1]["state"], "unavailable");
    client
        .put("/v1/orgs/settings", json!({"agentSubscriptions":true}))
        .await
        .expect("allow plans");
    std::fs::write(&auth, r#"{"loggedIn":true,"authMethod":"claude.ai"}"#).expect("plan restored");
    let current = rpc(
        &api,
        &client,
        &info,
        phone,
        desktop,
        json!({"kind":"describeSession","threadId":sync}),
    )
    .await;
    host.delay.store(true, Ordering::SeqCst);
    let guarded=submit(&client,&info,request(&info,phone,desktop,json!({"kind":"updateSession","threadId":sync,"revision":current["session"]["revision"],"settings":{"model":null,"effort":null}}))).await;
    let pass = api.clone();
    let processing = tokio::spawn(async move { pass.computer_pass().await });
    tokio::time::timeout(Duration::from_secs(5), host.entered.notified())
        .await
        .expect("native probe held");
    wb.set_code_control(false).expect("disable");
    wb.set_code_control(true).expect("re-enable");
    host.resume.notify_one();
    processing.await.expect("worker");
    assert_eq!(result(&client, &info, &guarded, phone).await["kind"], "refused");
    assert_eq!(
        wb.record(&target.local).expect("record").summary.model.as_deref(),
        Some("anthropic/fixture-model"),
        "a stale generation cannot update after discovery"
    );
    drop(api);
    drop(wb);
    gateway.stop().await;
    let unavailable = host
        .computer_choices(vec![SessionAgent::Claude], None)
        .await
        .expect("unknown access is described");
    assert_eq!(
        unavailable.agents[0].routes[1].state,
        c::Availability::Unknown,
        "unreachable policy is unknown rather than a policy denial"
    );
    let _ = std::fs::remove_dir_all(root);
}
