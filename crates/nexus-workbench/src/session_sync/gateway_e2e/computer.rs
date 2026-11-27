//! Zero-session computer RPC through the actual Gateway, crypto, Api and native agent lifecycle.
use super::*;
use crate::api::{AgentInfo, Api, Machine};
use crate::computer::{self as c, Transport};
use crate::event::SessionAgent;
use sha2::Digest as _;
// Its test runs a fake agent written by the unix-only `write_fake_script`.
#[cfg(unix)]
#[path = "computer/settings.rs"]
mod settings;

struct AgentMachine(std::path::PathBuf, std::path::PathBuf);
impl Machine for AgentMachine {
    fn tool_path(&self) -> String {
        std::env::var("PATH").unwrap_or_default()
    }
    fn home(&self) -> Result<std::path::PathBuf> {
        Ok(self.1.clone())
    }
    fn agent(&self, a: SessionAgent) -> AgentInfo {
        AgentInfo {
            agent: a,
            path: (a == SessionAgent::Claude).then(|| self.0.to_string_lossy().into_owned()),
            version: None,
            models: None,
            problem: None,
        }
    }
    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}
struct ClaimFault {
    inner: Arc<GatewayBackend>,
    lose: AtomicBool,
    lose_reply: AtomicBool,
    lose_decision: AtomicBool,
    pair_gate: AtomicBool,
    pair_entered: tokio::sync::Notify,
    pair_resume: tokio::sync::Notify,
}
impl Backend for ClaimFault {
    fn identity(&self) -> &Identity {
        self.inner.identity()
    }
    fn computer(&self) -> Option<&dyn Transport> {
        Some(self)
    }
    fn allow_local_cache(&self) -> bool {
        true
    }
    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        self.inner.policy()
    }
    fn head(&self, t: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        self.inner.head(t)
    }
    fn key(&self, t: &str, e: Option<u64>, p: ContentPolicy) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        self.inner.key(t, e, p)
    }
    fn append(&self, b: Batch) -> BoxFuture<'_, Result<u64>> {
        self.inner.append(b)
    }
    fn delete(&self, t: &str) -> BoxFuture<'_, Result<()>> {
        self.inner.delete(t)
    }
    fn supports_control(&self) -> bool {
        true
    }
    fn control_lease(&self, t: &str, e: bool) -> BoxFuture<'_, Result<()>> {
        self.inner.control_lease(t, e)
    }
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<crate::session_sync::control::Command>>> {
        self.inner.control_commands()
    }
    fn control_claim(&self, t: &str, id: &str) -> BoxFuture<'_, Result<bool>> {
        self.inner.control_claim(t, id)
    }
    fn control_finish(&self, t: &str, id: &str, e: bool) -> BoxFuture<'_, Result<()>> {
        self.inner.control_finish(t, id, e)
    }
}
impl c::ClientTransport for ClaimFault {
    fn computers(&self) -> BoxFuture<'_, Result<Value>> {
        c::ClientTransport::computers(self.inner.as_ref())
    }
    fn seal_request(
        &self,
        computer: String,
        id: String,
        operation: c::Operation,
    ) -> BoxFuture<'_, Result<c::Outbound>> {
        c::ClientTransport::seal_request(self.inner.as_ref(), computer, id, operation)
    }
    fn send_request(&self, outbound: c::Outbound) -> BoxFuture<'_, Result<c::CallResult>> {
        c::ClientTransport::send_request(self.inner.as_ref(), outbound)
    }
    fn get_request(&self, outbound: c::Outbound) -> BoxFuture<'_, Result<c::CallResult>> {
        c::ClientTransport::get_request(self.inner.as_ref(), outbound)
    }
    fn synced_page(
        &self,
        thread: String,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'_, Result<Value>> {
        c::ClientTransport::synced_page(self.inner.as_ref(), thread, before, after, limit)
    }
    fn seal_command(
        &self,
        thread: String,
        id: String,
        action: crate::session_sync::control::Action,
    ) -> BoxFuture<'_, Result<c::Outbound>> {
        c::ClientTransport::seal_command(self.inner.as_ref(), thread, id, action)
    }
    fn send_command(&self, outbound: c::Outbound) -> BoxFuture<'_, Result<Value>> {
        c::ClientTransport::send_command(self.inner.as_ref(), outbound)
    }
    fn get_command(&self, thread: String, id: String) -> BoxFuture<'_, Result<Value>> {
        c::ClientTransport::get_command(self.inner.as_ref(), thread, id)
    }
}
impl Transport for ClaimFault {
    fn seal_join(&self, invitation: c::Invitation, name: String) -> BoxFuture<'_, Result<c::Outbound>> {
        self.inner.seal_join(invitation, name)
    }

    fn poll_join(&self, outbound: c::Outbound) -> BoxFuture<'_, Result<(c::JoinStatus, String)>> {
        self.inner.poll_join(outbound)
    }

    fn admission_sender(&self, sender: String) -> BoxFuture<'_, Result<bool>> {
        self.inner.admission_sender(sender)
    }

    fn device(&self) -> BoxFuture<'_, Result<c::Device>> {
        Transport::device(self.inner.as_ref())
    }
    fn presence(&self, i: c::Info, e: bool) -> BoxFuture<'_, Result<()>> {
        self.inner.presence(i, e)
    }
    fn requests<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Vec<c::Request>>> {
        self.inner.requests(id)
    }
    fn claim(&self, r: c::RequestTarget) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let claimed = self.inner.claim(r).await?;
            if self.lose.swap(false, Ordering::SeqCst) {
                Err("lost claim response".into())
            } else {
                Ok(claimed)
            }
        })
    }
    fn seal_reply(&self, r: c::RequestTarget, o: c::Outcome) -> BoxFuture<'_, Result<c::Reply>> {
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err("process stopped before persisting reply".into()) });
        }
        self.inner.seal_reply(r, o)
    }
    fn deliver(&self, r: c::Reply) -> BoxFuture<'_, Result<()>> {
        self.inner.deliver(r)
    }
    fn invitation(&self, i: c::Info, signed_decisions: bool) -> BoxFuture<'_, Result<c::Invitation>> {
        self.inner.invitation(i, signed_decisions)
    }
    fn pairings(&self, i: c::InvitationRecord) -> BoxFuture<'_, Result<Vec<c::Pairing>>> {
        self.inner.pairings(i)
    }
    fn approve_pairing(&self, p: c::Pairing, admission: c::PairingAdmission) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.pair_gate.swap(false, Ordering::SeqCst) {
                self.pair_entered.notify_one();
                self.pair_resume.notified().await;
            }
            self.inner.approve_pairing(p, admission).await
        })
    }
    fn publish_pairing_decision(&self, invitation: c::InvitationRecord) -> BoxFuture<'_, Result<()>> {
        if self.lose_decision.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err("pairing relay unavailable".into()) });
        }
        self.inner.publish_pairing_decision(invitation)
    }
}
fn frame(domain: &str, scope: &c::Scope, parts: &[String]) -> String {
    let mut text = format!("{domain}\n");
    for s in [&scope.gateway, &scope.tenant_id, &scope.person_id]
        .into_iter()
        .chain(parts)
    {
        text.push_str(&format!("{}:{s}", s.len()));
    }
    text
}
fn request(info: &c::Info, phone: (&str, &DeviceKey), runner: (&str, &DeviceKey), operation: Value) -> Value {
    let id = uuid::Uuid::new_v4().to_string();
    let scope = info.scope.as_ref().expect("scope");
    let now = crate::now_ms();
    let expiry = now
        + if operation["kind"] == "startSession" {
            15_000
        } else {
            30_000
        };
    let payload = json!({"version":1,"kind":"computerRequest","scope":scope,"computerId":info.computer_id,"id":id,"senderDeviceId":phone.0,"runnerDeviceId":runner.0,"generation":info.generation,"issuedAt":now,"expiresAt":expiry,"operation":operation});
    let key = e2e::ContentKey::generate(0);
    let locked = e2e::b64u(&key.lock_for(&id, runner.0, &runner.1.encryption_key()).expect("wrap"));
    let sealed = e2e::b64u(
        &key.seal_event(&id, 0, &serde_json::to_vec(&payload).expect("encode"))
            .expect("seal"),
    );
    let m = frame(
        "nexus-computer-request-v1",
        scope,
        &[
            info.computer_id.clone(),
            id.clone(),
            phone.0.into(),
            runner.0.into(),
            info.generation.to_string(),
            now.to_string(),
            expiry.to_string(),
            locked.clone(),
            sealed.clone(),
        ],
    );
    json!({"version":1,"scope":scope,"computerId":info.computer_id,"id":id,"senderDeviceId":phone.0,"runnerDeviceId":runner.0,"generation":info.generation,"issuedAt":now,"expiresAt":expiry,"lockedKey":locked,"sealed":sealed,"signature":e2e::b64u(&phone.1.sign(&m))})
}
async fn result(client: &Client, info: &c::Info, id: &str, phone: (&str, &DeviceKey)) -> Value {
    let row = client
        .get(&format!("/v1/computers/{}/requests/{id}", info.computer_id))
        .await
        .expect("request");
    assert_eq!(row["state"], "replied", "runner persisted a verifiable reply");
    let r = &row["reply"];
    let key = phone
        .1
        .unlock_content_key(id, 0, phone.0, &b64u(r["lockedKey"].as_str().expect("wrapped")))
        .expect("unwrap result");
    let opened = key
        .open_event(id, 0, &b64u(r["sealed"].as_str().expect("sealed")))
        .expect("open result");
    serde_json::from_slice::<Value>(&opened).expect("reply payload")["result"].clone()
}
async fn submit(client: &Client, info: &c::Info, envelope: Value) -> String {
    let id = envelope["id"].as_str().expect("id").to_string();
    client
        .post(&format!("/v1/computers/{}/requests", info.computer_id), envelope)
        .await
        .expect("enqueue");
    id
}
fn pairing(i: &c::Invitation, phone: (&str, &DeviceKey)) -> Value {
    let id = uuid::Uuid::new_v4().to_string();
    let expiry = i.expires_at;
    let encryption = e2e::b64u(&phone.1.encryption_key());
    let signing = e2e::b64u(&phone.1.signing_key());
    let payload = json!({"version":1,"kind":"phonePairing","scope":i.scope,"id":id,"inviteId":i.invite_id,"computerId":i.computer_id,"inviterDeviceId":i.inviter_device_id,"senderDeviceId":phone.0,"expiresAt":expiry,"secret":i.secret,"encryptionKey":encryption,"signingKey":signing});
    let public: [u8; 32] = b64u(&i.encryption_key).try_into().expect("public");
    let key = e2e::ContentKey::generate(0);
    let locked = e2e::b64u(&key.lock_for(&id, &i.inviter_device_id, &public).expect("wrap"));
    let sealed = e2e::b64u(
        &key.seal_event(&id, 0, &serde_json::to_vec(&payload).expect("encode"))
            .expect("seal"),
    );
    let m = frame(
        "nexus-phone-pairing-v1",
        &i.scope,
        &[
            id.clone(),
            i.invite_id.clone(),
            i.inviter_device_id.clone(),
            phone.0.into(),
            expiry.to_string(),
            locked.clone(),
            sealed.clone(),
        ],
    );
    json!({"version":1,"scope":i.scope,"id":id,"inviteId":i.invite_id,"inviterDeviceId":i.inviter_device_id,"senderDeviceId":phone.0,"expiresAt":expiry,"lockedKey":locked,"sealed":sealed,"signature":e2e::b64u(&phone.1.sign(&m))})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_session_computer_creates_once_and_lost_claim_is_uncertain_after_reopen() {
    let Some(mut gateway) = boot().await else { return };
    let root = std::env::temp_dir().join(format!("nexus-computer-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("repo")).expect("repo");
    std::fs::create_dir_all(root.join("memory")).expect("memory");
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Computer RPC","email":"computer-rpc@test.example"}),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    client
        .put("/v1/orgs/settings", json!({"sessionContent":"user"}))
        .await
        .expect("policy");
    let desktop_key = DeviceKey::generate();
    let desktop = register(&client, &desktop_key, "Desktop").await;
    let phone_key = DeviceKey::generate();
    let phone = register(&client, &phone_key, "Phone").await;
    approve(&client, (&desktop, &desktop_key), &phone, &phone_key).await;
    let scope = format!("{}|computer-rpc", gateway.url());
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
    let fault = Arc::new(ClaimFault {
        inner: backend,
        lose: AtomicBool::new(false),
        lose_reply: AtomicBool::new(false),
        lose_decision: AtomicBool::new(false),
        pair_gate: AtomicBool::new(false),
        pair_entered: tokio::sync::Notify::new(),
        pair_resume: tokio::sync::Notify::new(),
    });
    let agent = std::env::var_os("NEXUS_SYNC_E2E_AGENT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .expect("binary")
                .parent()
                .expect("deps")
                .parent()
                .expect("debug")
                .join("fake-claude")
        });
    assert!(agent.exists(), "build fake-claude for native execution proof");
    let host = Arc::new(super::control::ControlHost {
        backend: fault.clone(),
        agent: agent.clone(),
        memory: root.join("memory"),
    });
    let machine = Arc::new(AgentMachine(agent, root.clone()));
    let path = root.join("workbench.sqlite3");
    let wb = Workbench::new(
        Store::open(&path).expect("store"),
        host.clone(),
        Config {
            scope: scope.clone(),
            remote_pairing_preview: true,
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let project = wb.add_project(&root.join("repo")).expect("project");
    let api = Api::new(wb.clone(), machine.clone());

    let stable = Workbench::new(
        Store::open(&root.join("stable-workbench.sqlite3")).expect("stable store"),
        host.clone(),
        Config {
            scope: scope.clone(),
            remote_pairing_preview: false,
            ..Config::default()
        },
    )
    .expect("workbench with preview off");
    let stable_api = Api::new(stable.clone(), machine.clone());
    let legacy = stable_api.phone_invitation_begin().await.expect("stable invitation");
    assert_eq!(legacy.version, 1);
    assert!(e2e::verify(
        &desktop_key.signing_key(),
        &frame(
            "nexus-phone-invitation-v1",
            &legacy.scope,
            &[
                legacy.invite_id.clone(),
                legacy.computer_id.clone(),
                legacy.computer_name.clone(),
                legacy.inviter_device_id.clone(),
                legacy.encryption_key.clone(),
                legacy.signing_key.clone(),
                legacy.expires_at.to_string(),
                e2e::b64u(&sha2::Sha256::digest(b64u(&legacy.secret))),
            ]
        ),
        &b64u(&legacy.signature),
    ));
    let legacy_request = pairing(&legacy, (&phone, &phone_key));
    let route = format!("/v1/devices/{desktop}/pairing-requests");
    client
        .post(&route, legacy_request.clone())
        .await
        .expect("scan from legacy phone");
    let candidate = stable_api
        .phone_invitation_poll(legacy.invite_id.clone())
        .await
        .expect("legacy scan");
    stable_api
        .phone_invitation_approve(
            legacy.invite_id.clone(),
            phone.clone(),
            candidate.code.expect("legacy code"),
        )
        .await
        .expect("approve on stable desktop");
    let receipt = client.post(&route, legacy_request).await.expect("legacy receipt");
    assert_eq!(receipt["state"], "finished");
    assert!(receipt.get("decision").is_none());
    assert!(!stable.code_control().expect("stable control setting").enabled);
    assert!(matches!(
        stable_api.computer_join_poll(legacy.invite_id).await,
        Err(message) if message == "Desktop-to-desktop control is unavailable."
    ));
    let refused = stable_api
        .computer_request(
            legacy.computer_id,
            uuid::Uuid::new_v4().to_string(),
            c::Operation::DescribeProjects,
        )
        .await;
    assert!(matches!(refused, Err(message) if message == "Desktop-to-desktop control is unavailable."));
    stable.shutdown().await;

    wb.set_code_control(true).expect("local opt-in");
    let mut info = api.computer_info().await.expect("info");
    info.generation = wb
        .store()
        .computer_generation(&scope, &fault.identity().user, true, true)
        .expect("generation");
    fault.presence(info.clone(), true).await.expect("zero-thread presence");
    assert!(wb.threads().expect("threads").is_empty());
    assert_eq!(
        client.get("/v1/computers").await.expect("computers")["computers"]
            .as_array()
            .expect("list")
            .len(),
        1
    );
    let describe = submit(
        &client,
        &info,
        request(
            &info,
            (&phone, &phone_key),
            (&desktop, &desktop_key),
            json!({"kind":"describeProjects"}),
        ),
    )
    .await;
    api.computer_pass().await;
    let projects = result(&client, &info, &describe, (&phone, &phone_key)).await;
    assert_eq!(projects["kind"], "projects");
    let choice = &projects["projects"][0];
    assert_eq!(choice["id"], project.id);
    let start=submit(&client,&info,request(&info,(&phone,&phone_key),(&desktop,&desktop_key),json!({"kind":"startSession","projectId":project.id,"revision":choice["revision"],"text":"Phone-created first prompt"}))).await;
    api.computer_pass().await;
    let made = result(&client, &info, &start, (&phone, &phone_key)).await;
    assert_eq!(made["kind"], "created");
    let local = crate::creation_thread_id(&start).expect("local id");
    super::control::control_wait(&wb, &local, |events| {
        events.iter().any(|e| matches!(e.event, WbEvent::TurnEnd { .. }))
    })
    .await;
    assert_eq!(wb.sync_sessions_once().await.failures.len(), 0);
    let opened = read_as(
        &client,
        made["threadId"].as_str().expect("sync id"),
        (&phone, &phone_key),
    )
    .await;
    assert!(opened.iter().any(
        |(e, _)| serde_json::from_str::<Value>(&e.event_json).expect("event")["text"] == "Phone-created first prompt"
    ));
    api.computer_pass().await;
    assert_eq!(
        wb.events(&local, 0)
            .expect("events")
            .iter()
            .filter(|e| matches!(e.event, WbEvent::User { .. }))
            .count(),
        1
    );
    let recover=submit(&client,&info,request(&info,(&phone,&phone_key),(&desktop,&desktop_key),json!({"kind":"startSession","projectId":project.id,"revision":choice["revision"],"text":"Creation exists after interrupted reply persistence"}))).await;
    fault.lose_reply.store(true, Ordering::SeqCst);
    api.computer_pass().await;
    let recover_local = crate::creation_thread_id(&recover).expect("local id");
    super::control::control_wait(&wb, &recover_local, |events| {
        events.iter().any(|e| matches!(e.event, WbEvent::TurnEnd { .. }))
    })
    .await;
    api.computer_pass().await;
    assert_eq!(
        result(&client, &info, &recover, (&phone, &phone_key)).await["kind"],
        "created",
        "a matching first-prompt digest proves a real creation after a crash"
    );
    assert_eq!(
        wb.events(&recover_local, 0)
            .expect("events")
            .iter()
            .filter(|e| matches!(e.event, WbEvent::User { .. }))
            .count(),
        1
    );
    let lost=submit(&client,&info,request(&info,(&phone,&phone_key),(&desktop,&desktop_key),json!({"kind":"startSession","projectId":project.id,"revision":choice["revision"],"text":"Must never start after a lost claim acknowledgement"}))).await;
    fault.lose.store(true, Ordering::SeqCst);
    api.computer_pass().await;
    assert!(!wb.has_thread(&crate::creation_thread_id(&lost).expect("local id")));
    api.computer_pass().await;
    assert_eq!(
        result(&client, &info, &lost, (&phone, &phone_key)).await["kind"],
        "uncertain"
    );
    let reopened = Workbench::new(
        Store::open(&path).expect("reopen"),
        host.clone(),
        Config {
            scope: scope.clone(),
            remote_pairing_preview: true,
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let restarted = Api::new(reopened.clone(), machine.clone());
    restarted.computer_pass().await;
    assert!(!reopened.has_thread(&crate::creation_thread_id(&lost).expect("local id")));
    assert_eq!(reopened.threads().expect("threads").len(), 2);
    // The actual shared creator waits on its gate while the real receiving desktop toggles its grant.
    for reenable in [false, true] {
        wb.set_code_control(true).expect("grant on");
        let generation = wb
            .store()
            .computer_info(&fault.identity().scope, &fault.identity().user)
            .expect("generation")
            .generation;
        let creation = uuid::Uuid::new_v4().to_string();
        let local = crate::creation_thread_id(&creation).expect("local id");
        let gate = wb.creation_gate(&local);
        let held = gate.lock().await;
        let new = crate::NewThread {
            name: None,
            project_id: project.id.clone(),
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: crate::event::PermissionMode::Default,
            effort: None,
            via: crate::event::Via::Nexus,
            provider: None,
            text: "This admission must be invalidated while the actual gate is held".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        };
        let guard = crate::CreationGuard {
            identity: fault.identity().clone(),
            expires_at: crate::now_ms() + 60_000,
            generation,
            sender: phone.clone(),
            selection: None,
        };
        let mut blocked =
            Box::pin(wb.create_thread_once_mentioning_guarded(new, &creation, String::new(), Some(guard)));
        assert!(futures::poll!(&mut blocked).is_pending());
        wb.set_code_control(false).expect("grant off");
        if reenable {
            wb.set_code_control(true).expect("grant back on");
        }
        drop(held);
        assert!(blocked.await.is_err());
        assert!(
            !wb.has_thread(&local),
            "off/on may not revive a creation's old generation"
        );
    }
    wb.set_code_control(true).expect("restore grant");
    info.generation = wb
        .store()
        .computer_info(&fault.identity().scope, &fault.identity().user)
        .expect("generation")
        .generation;
    fault
        .presence(info.clone(), true)
        .await
        .expect("renew fresh generation");
    let invite = api.phone_invitation_begin().await.expect("desktop invitation");
    let pending_key = DeviceKey::generate();
    let pending = register(&client, &pending_key, "New scanned phone").await;
    client
        .post(
            &format!("/v1/devices/{desktop}/pairing-requests"),
            pairing(&invite, (&pending, &pending_key)),
        )
        .await
        .expect("encrypted scan proof");
    let candidate = api
        .phone_invitation_poll(invite.invite_id.clone())
        .await
        .expect("scan candidate");
    assert_eq!(candidate.state, "approving");
    assert_eq!(
        candidate.code.as_deref(),
        Some(e2e::short_code(&pending_key.fingerprint()).as_str())
    );
    let listed = client.get("/v1/devices").await.expect("pending directory");
    assert!(
        listed["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["id"] == pending && device["state"] == "pending")
    );
    assert!(
        api.phone_invitation_approve(invite.invite_id.clone(), pending.clone(), "wrong code".into())
            .await
            .is_err()
    );
    let connected = api
        .phone_invitation_approve(
            invite.invite_id.clone(),
            pending.clone(),
            candidate.code.expect("match code"),
        )
        .await
        .expect("explicit desktop approval");
    assert_eq!(connected.state, "connected");
    assert_eq!(connected.device_id.as_deref(), Some(pending.as_str()));
    let listed = client.get("/v1/devices").await.expect("directory");
    assert!(
        listed["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|d| d["id"] == pending && d["state"] == "trusted" && d["approvedBy"] == desktop)
    );
    // Cancellation and real clock expiry while approval is awaiting transport cannot grant trust later.
    for expire in [false, true] {
        let mut delayed = api.phone_invitation_begin().await.expect("delayed invitation");
        if expire {
            let mut record = wb
                .store()
                .phone_invitation(&fault.identity().scope, &fault.identity().user, &delayed.invite_id)
                .expect("record")
                .expect("exists");
            record.expires_at = crate::now_ms() + 1_500;
            delayed.expires_at = record.expires_at;
            wb.store()
                .phone_invitation_save(&fault.identity().scope, &fault.identity().user, &record)
                .expect("last moments of invitation lifetime");
        }
        let key = DeviceKey::generate();
        let pending_id = register(&client, &key, if expire { "Expired proof" } else { "Cancelled proof" }).await;
        client
            .post(
                &format!("/v1/devices/{desktop}/pairing-requests"),
                pairing(&delayed, (&pending_id, &key)),
            )
            .await
            .expect("delayed scan proof");
        let candidate = api
            .phone_invitation_poll(delayed.invite_id.clone())
            .await
            .expect("delayed candidate");
        fault.pair_gate.store(true, Ordering::SeqCst);
        let polling = api.clone();
        let invite_id = delayed.invite_id.clone();
        let device_id = pending_id.clone();
        let task = tokio::spawn(async move {
            polling
                .phone_invitation_approve(invite_id, device_id, candidate.code.expect("match code"))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), fault.pair_entered.notified())
            .await
            .expect("approval reached the awaited transport");
        if expire {
            let left = delayed.expires_at.saturating_sub(crate::now_ms()).max(0) as u64;
            tokio::time::sleep(Duration::from_millis(left + 10)).await;
        } else {
            api.phone_invitation_cancel(delayed.invite_id)
                .await
                .expect("cancel before signature authorization");
        }
        fault.pair_resume.notify_one();
        assert!(task.await.expect("poll").is_err());
        let listed = client.get("/v1/devices").await.expect("directory");
        assert!(
            listed["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .any(|d| d["id"] == pending_id && d["state"] == "pending"),
            "cancelled or expired invitation never signs a phone approval"
        );
    }
    // Reverse trust direction: an already trusted phone scans and approves a still-pending desktop.
    let other_key = DeviceKey::generate();
    let other_backend = Arc::new(
        GatewayBackend::connect(
            client.clone(),
            fault.identity().scope.clone(),
            DeviceKey::from_secret_bytes(&other_key.secret_bytes()),
            Arc::new(KeyStore::open(&root.join("other-keys.sqlite3")).expect("keys")),
            true,
        )
        .await
        .expect("pending backend"),
    );
    let other_memory = root.join("other-memory");
    std::fs::create_dir_all(&other_memory).expect("second desktop memory");
    let other_wb = Workbench::new(
        Store::open(&root.join("other-workbench.sqlite3")).expect("store"),
        Arc::new(super::control::ControlHost {
            backend: other_backend.clone(),
            agent: host.agent.clone(),
            memory: other_memory,
        }),
        Config {
            scope: fault.identity().scope.clone(),
            remote_pairing_preview: true,
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("pending desktop");
    let other_api = Api::new(other_wb.clone(), machine.clone());
    let reverse = other_api
        .phone_invitation_begin()
        .await
        .expect("normal device registration for Connect phone");
    let listed = client.get("/v1/devices").await.expect("directory");
    assert!(
        listed["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|d| d["id"] == reverse.inviter_device_id && d["state"] == "pending")
    );
    // The phone confirmed the desktop when it paired. Seal that trust to the new computer.
    let devices = listed["devices"].as_array().expect("directory");
    let phone_store = KeyStore::in_memory().expect("phone keys");
    crate::session_sync::approver::confirm(&phone_store, "phone", devices, &phone_key, &desktop)
        .expect("phone confirmed the original desktop");
    let other_encryption = e2e::b64u(&other_key.encryption_key());
    let handover = crate::session_sync::approver::handover(
        &phone_store,
        "phone",
        devices,
        &phone_key,
        &reverse.inviter_device_id,
        &other_encryption,
    )
    .expect("prepare handover")
    .expect("trusted phone handover");
    let approval = e2e::approval_message(
        &reverse.inviter_device_id,
        &other_key.encryption_key(),
        &other_key.signing_key(),
    )
    .expect("approval message");
    client.post(
        &format!("/v1/devices/{}/approve", reverse.inviter_device_id),
        json!({ "approverDeviceId": phone, "signature": e2e::b64u(&phone_key.sign(&approval)), "handover": handover }),
    ).await.expect("phone approves the new computer");
    client
        .post(
            &format!("/v1/devices/{}/pairing-requests", reverse.inviter_device_id),
            pairing(&reverse, (&phone, &phone_key)),
        )
        .await
        .expect("existing phone scan proof");
    let candidate = other_api
        .phone_invitation_poll(reverse.invite_id.clone())
        .await
        .expect("trusted phone candidate");
    assert_eq!(candidate.state, "approving");
    let joined = other_api
        .phone_invitation_approve(reverse.invite_id, phone.clone(), candidate.code.expect("match code"))
        .await
        .expect("desktop confirmed its trusted phone");
    assert_eq!(joined.state, "connected");
    assert_eq!(joined.device_id.as_deref(), Some(phone.as_str()));
    let listed = client.get("/v1/devices").await.expect("directory");
    assert!(
        listed["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|d| d["id"] == phone && d["encryptionKey"] == e2e::b64u(&phone_key.encryption_key()))
    );
    // The phone's remote control grant on the second desktop does not extend to an unlinked desktop.
    let other_project = other_wb
        .add_project(&root.join("repo"))
        .expect("second desktop project");
    let other_info = other_api.computer_info().await.expect("second desktop info");
    assert!(other_info.enabled);
    other_backend
        .presence(other_info.clone(), true)
        .await
        .expect("second desktop presence");
    let unauthorized = uuid::Uuid::new_v4().to_string();
    api.computer_request(
        other_info.computer_id.clone(),
        unauthorized.clone(),
        c::Operation::DescribeProjects,
    )
    .await
    .expect("queue request from another trusted device");
    other_api.computer_pass().await;
    let answer = api
        .computer_request_get(other_info.computer_id.clone(), unauthorized)
        .await
        .expect("signed rejection for an unlinked device");
    assert!(matches!(answer.result, Some(c::Outcome::Refused { code, .. }) if code == "device_not_connected"));

    let link = |invite: &c::Invitation| {
        format!(
            "xyz.nexus.app://connect#invite={}",
            e2e::b64u(&serde_json::to_vec(invite).expect("encode invitation"))
        )
    };
    let invite_to_cancel = api.phone_invitation_begin().await.expect("new invitation");
    let cancelled = other_api
        .computer_join_begin(link(&invite_to_cancel))
        .await
        .expect("request link");
    assert_eq!(cancelled.state, c::JoinState::Pending);
    fault.lose_decision.store(true, Ordering::SeqCst);
    assert!(api.phone_invitation_cancel(cancelled.invite_id.clone()).await.is_err());
    api.computer_pass().await;
    assert_eq!(
        other_api
            .computer_join_poll(cancelled.invite_id)
            .await
            .expect("verify cancellation")
            .state,
        c::JoinState::Cancelled,
    );

    let desktop_invite = api
        .phone_invitation_begin()
        .await
        .expect("desktop-to-desktop invitation");
    let pairing = other_api
        .computer_join_begin(link(&desktop_invite))
        .await
        .expect("request desktop link");
    assert_eq!(pairing.state, c::JoinState::Pending);
    assert!(other_api.computer_join_allow(pairing.invite_id.clone()).await.is_err());
    let candidate = api
        .phone_invitation_poll(pairing.invite_id.clone())
        .await
        .expect("second desktop candidate");
    api.phone_invitation_approve(
        pairing.invite_id.clone(),
        candidate.device_id.expect("desktop candidate id"),
        candidate.code.expect("desktop match code"),
    )
    .await
    .expect("approve second desktop");
    assert_eq!(
        other_api
            .computer_join_poll(pairing.invite_id.clone())
            .await
            .expect("verify desktop approval")
            .state,
        c::JoinState::Approved,
    );
    assert!(
        !other_wb
            .store()
            .control_allows(other_backend.identity(), &desktop)
            .expect("separate permission")
    );
    other_api
        .computer_join_allow(pairing.invite_id)
        .await
        .expect("grant reverse control on the second desktop");

    assert!(
        other_wb
            .store()
            .control_allows(other_backend.identity(), &desktop)
            .expect("reverse grant exists")
    );
    let reverse_request = uuid::Uuid::new_v4().to_string();
    api.computer_request(
        other_info.computer_id.clone(),
        reverse_request.clone(),
        c::Operation::DescribeProjects,
    )
    .await
    .expect("request work on the second desktop");
    other_api.computer_pass().await;
    let answer = api
        .computer_request_get(other_info.computer_id.clone(), reverse_request)
        .await
        .expect("second desktop replies");
    let Some(c::Outcome::Projects { projects, .. }) = answer.result else {
        panic!(
            "second desktop projects: {}",
            serde_json::to_string(&answer).expect("format response")
        )
    };
    let remote = projects
        .iter()
        .find(|p| p.id == other_project.id)
        .expect("saved project on second desktop");
    let start_reverse = uuid::Uuid::new_v4().to_string();
    api.computer_request(
        other_info.computer_id.clone(),
        start_reverse.clone(),
        c::Operation::StartSession {
            base_branch: None,
            handoff: None,
            project_id: remote.id.clone(),
            revision: remote.revision.clone(),
            text: "Start work from the first desktop".into(),
            selection: None,
        },
    )
    .await
    .expect("send prompt to the second desktop");
    other_api.computer_pass().await;
    let answer = api
        .computer_request_get(other_info.computer_id.clone(), start_reverse)
        .await
        .expect("second desktop accepts prompt");
    assert!(matches!(answer.result, Some(c::Outcome::Created { .. })));
    assert_eq!(other_wb.threads().expect("second desktop runs local work").len(), 1);
    let native_request = uuid::Uuid::new_v4().to_string();
    let queued = other_api
        .computer_request(
            info.computer_id.clone(),
            native_request.clone(),
            c::Operation::DescribeProjects,
        )
        .await
        .expect("native encrypted sender");
    assert_eq!(queued.state, "queued");
    api.computer_pass().await;
    let answer = other_api
        .computer_request_get(info.computer_id.clone(), native_request.clone())
        .await
        .expect("native verified reply");
    assert_eq!(answer.state, "replied");
    let Some(c::Outcome::Projects { projects, .. }) = answer.result else {
        panic!("first desktop projects")
    };
    let remote = projects
        .iter()
        .find(|p| p.id == project.id)
        .expect("saved project on first desktop");
    let start = uuid::Uuid::new_v4().to_string();
    other_api
        .computer_request(
            info.computer_id.clone(),
            start.clone(),
            c::Operation::StartSession {
                base_branch: None,
                handoff: None,
                project_id: remote.id.clone(),
                revision: remote.revision.clone(),
                text: "Start work from the second desktop".into(),
                selection: None,
            },
        )
        .await
        .expect("send prompt to the first desktop");
    api.computer_pass().await;
    let answer = other_api
        .computer_request_get(info.computer_id.clone(), start)
        .await
        .expect("first desktop accepts prompt");
    assert!(matches!(answer.result, Some(c::Outcome::Created { .. })));
    assert!(
        other_api
            .computer_request(
                info.computer_id.clone(),
                native_request,
                c::Operation::StartSession {
                    base_branch: None,
                    handoff: None,
                    selection: None,
                    project_id: project.id.clone(),
                    revision: choice["revision"].as_str().expect("revision").into(),
                    text: "A changed request must not be sent".into()
                }
            )
            .await
            .is_err()
    );
    rusqlite::Connection::open(&path)
        .expect("clock")
        .execute("update session_sync_threads set last_attempt=0", [])
        .expect("idle refresh due");
    wb.inner.session_sync.lock().await.next_keys = None;
    assert!(wb.sync_sessions_once().await.failures.is_empty());
    let synced = made["threadId"].as_str().expect("sync id").to_string();
    let read = other_api
        .synced_thread_page(synced.clone(), None, Some(-1), Some(100))
        .await
        .expect("native decrypted reader");
    assert!(
        read["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["state"] == "open" && e["event"]["text"] == "Phone-created first prompt")
    );
    wb.control_sessions_once().await;
    let command = uuid::Uuid::new_v4().to_string();
    other_api
        .synced_command(
            synced.clone(),
            command.clone(),
            crate::session_sync::control::Action::Send {
                text: "Continue through the native remote renderer".into(),
            },
        )
        .await
        .expect("native encrypted command");
    wb.control_sessions_once().await;
    assert_eq!(
        other_api
            .synced_command_get(synced, command)
            .await
            .expect("native receipt")["state"],
        "completed"
    );
    super::control::control_wait(&wb, &local, |events| {
        events
            .iter()
            .any(|e| matches!(&e.event,WbEvent::User{text,..}if text=="Continue through the native remote renderer"))
    })
    .await;
    let fresh_backend = Arc::new(
        GatewayBackend::connect(
            client.clone(),
            scope.clone(),
            DeviceKey::generate(),
            Arc::new(KeyStore::open(&root.join("fresh-keys.sqlite3")).expect("fresh keys")),
            true,
        )
        .await
        .expect("fresh desktop backend"),
    );
    let fresh_wb = Workbench::new(
        Store::open(&root.join("fresh-workbench.sqlite3")).expect("fresh store"),
        Arc::new(E2eHost(fresh_backend)),
        Config {
            scope,
            remote_pairing_preview: true,
            ..Config::default()
        },
    )
    .expect("fresh desktop");
    let fresh_api = Api::new(fresh_wb.clone(), machine);
    let fresh_invite = api
        .phone_invitation_begin()
        .await
        .expect("invite a new desktop installation");
    let joining = fresh_api
        .computer_join_begin(link(&fresh_invite))
        .await
        .expect("register and join");
    assert_eq!(joining.state, c::JoinState::Pending);
    let candidate = api
        .phone_invitation_poll(joining.invite_id.clone())
        .await
        .expect("fresh desktop candidate");
    api.phone_invitation_approve(
        joining.invite_id.clone(),
        candidate.device_id.expect("new desktop id"),
        candidate.code.expect("new desktop code"),
    )
    .await
    .expect("approve new desktop");
    let joined = fresh_api
        .computer_join_poll(joining.invite_id.clone())
        .await
        .expect("anchor the host from the invitation");
    assert_eq!(joined.state, c::JoinState::Approved);
    assert!(
        !fresh_wb
            .code_control()
            .expect("joining alone grants outbound control")
            .enabled
    );
    fresh_api
        .computer_join_allow(joining.invite_id)
        .await
        .expect("grant reciprocal control when requested");

    assert!(
        !other_wb
            .store()
            .control_allows(other_backend.identity(), &pending)
            .expect("earlier phone grant")
    );
    other_wb.set_code_control(false).expect("pause remote control");
    other_wb.set_code_control(true).expect("explicit account-wide opt-in");
    assert!(
        other_wb
            .store()
            .control_allows(other_backend.identity(), &pending)
            .expect("earlier phone can control again")
    );
    fresh_wb.shutdown().await;
    other_wb.shutdown().await;
    let stored = std::fs::read(&path).expect("db bytes");
    assert!(
        !stored
            .windows(invite.secret.len())
            .any(|w| w == invite.secret.as_bytes()),
        "invitation secret is never stored in native SQLite"
    );
    reopened.shutdown().await;
    wb.shutdown().await;
    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
