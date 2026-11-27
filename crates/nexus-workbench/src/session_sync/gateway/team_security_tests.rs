use super::*;
use crate::session_sync::{ContentPolicy, keystore::KeyStore};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use nexus_client::{Client, ClientOptions, e2e::DeviceKey};
use std::convert::Infallible;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

struct Hostile {
    own: Value,
    invented: Value,
    team: Mutex<Value>,
    thread_copy: Value,
    pause_team: AtomicBool,
    team_entered: tokio::sync::Notify,
    team_resume: tokio::sync::Notify,
    pause_directory: AtomicBool,
    directory_entered: tokio::sync::Notify,
    directory_resume: tokio::sync::Notify,
    calls: Mutex<Vec<(String, String, Value)>>,
    missing: AtomicBool,
}

impl Hostile {
    async fn answer(&self, request: Request<Incoming>) -> Response<Full<Bytes>> {
        let method = request.method().to_string();
        let path = request.uri().path().to_string();
        let body = request.into_body().collect().await.expect("body").to_bytes();
        let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
        self.calls
            .lock()
            .expect("calls")
            .push((method.clone(), path.clone(), body));
        if path == "/v1/orgs/members/invented/holders" && self.pause_directory.swap(false, Ordering::SeqCst) {
            self.directory_entered.notify_one();
            self.directory_resume.notified().await;
        }
        if path == "/v1/orgs/team-key" && self.pause_team.swap(false, Ordering::SeqCst) {
            self.team_entered.notify_one();
            self.team_resume.notified().await;
        }
        let (status, answer) = match (method.as_str(), path.as_str()) {
            ("GET", "/v1/auth/whoami") => (200, json!({ "kind": "person", "userId": "victim", "tenantId": "acme" })),
            ("GET", "/v1/devices") => (200, json!({ "devices": [self.own.clone()] })),
            ("GET", "/v1/orgs/settings") => (200, json!({ "sessionContent": "user", "multiplayerSessions": true })),
            ("GET", "/v1/orgs/members") => (
                200,
                json!({ "members": [{ "userId": "victim" }, { "userId": "invented" }] }),
            ),
            ("GET", "/v1/orgs/members/victim/holders") => (200, json!({ "holders": [self.own.clone()] })),
            ("GET", "/v1/orgs/members/invented/holders") => (200, json!({ "holders": [self.invented.clone()] })),
            ("GET", "/v1/orgs/team-key") => (200, self.team.lock().expect("team").clone()),
            ("GET", _) if path.ends_with("/team-keys") => (200, json!({ "keys": [self.thread_copy.clone()] })),
            ("GET", _) if path.ends_with("/keys") => (404, json!({ "code": "not_found" })),
            ("GET", _) if path.starts_with("/v1/threads/") && !self.missing.load(Ordering::SeqCst) => {
                (200, json!({ "private": false, "lastSeq": null }))
            }
            ("GET", _) => (404, json!({ "code": "not_found" })),
            _ => (200, json!({})),
        };
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(answer.to_string())))
            .expect("response")
    }
}

struct Fixture {
    runner: Arc<GatewayBackend>,
    thread: String,
    hostile: Arc<Hostile>,
    root: std::path::PathBuf,
    listener: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.listener.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fixture {
    async fn new() -> Self {
        let device = DeviceKey::generate();
        let attacker = DeviceKey::generate();
        let team = HolderKey::generate();
        let listed = |id: &str, key: &DeviceKey| {
            json!({
                "id": id, "kind": "device", "state": "trusted", "encryptionKey": e2e::b64u(&key.encryption_key()),
                "signingKey": e2e::b64u(&key.signing_key()), "approvedBy": null, "approvalSignature": null, "hasCurrentTeamKey": false,
            })
        };
        let locked = team
            .lock_team_key_for("acme", 1, "dev_victim", &device.encryption_key())
            .expect("locked");
        let signature = attacker.sign(&e2e::team_key_message("acme", 1, "dev_victim", &locked).expect("message"));
        let thread = uuid::Uuid::new_v4().to_string();
        let content = e2e::ContentKey::generate(0);
        let content_copy = content
            .lock_for(&thread, "tmk_attacker", &team.encryption_key())
            .expect("thread copy");
        let content_signature = attacker
            .sign(&e2e::team_thread_key_message(&thread, 0, "tmk_attacker", &content_copy).expect("thread message"));
        let hostile = Arc::new(Hostile {
            own: listed("dev_victim", &device),
            invented: listed("dev_invented", &attacker),
            team: Mutex::new(
                json!({ "key": { "id": "tmk_attacker", "epoch": 1, "encryptionKey": e2e::b64u(&team.encryption_key()), "retiredAt": null },
                "copies": [{ "teamKeyId": "tmk_attacker", "epoch": 1, "holderId": "dev_victim", "lockedKey": e2e::b64u(&locked), "senderDeviceId": "dev_invented", "signature": e2e::b64u(&signature) }] }),
            ),
            thread_copy: json!({ "keyVersion": 1, "teamKeyId": "tmk_attacker", "lockedKey": e2e::b64u(&content_copy), "senderDeviceId": "dev_invented", "signature": e2e::b64u(&content_signature) }),
            pause_team: AtomicBool::new(false),
            team_entered: tokio::sync::Notify::new(),
            team_resume: tokio::sync::Notify::new(),
            pause_directory: AtomicBool::new(false),
            directory_entered: tokio::sync::Notify::new(),
            directory_resume: tokio::sync::Notify::new(),
            calls: Mutex::new(Vec::new()),
            missing: AtomicBool::new(false),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let serving = hostile.clone();
        let listener = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let serving = serving.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let serving = serving.clone();
                        async move { Ok::<_, Infallible>(serving.answer(request).await) }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let root = std::env::temp_dir().join(format!("nexus-team-trust-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("folder");
        let keys = Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("keys"));
        let mut runner = GatewayBackend::connect(
            Client::new(ClientOptions::new(&url)),
            "scope".into(),
            device,
            keys,
            true,
        )
        .await
        .expect("runner");
        runner.team_policy_path = root.join("managed.json");
        Self {
            runner: Arc::new(runner),
            thread,
            hostile,
            root,
            listener,
        }
    }

    fn opt_in(&self, admit: bool) {
        let members = if admit {
            vec![
                json!({ "personId": "invented", "deviceId": "dev_invented", "encryptionKey": self.hostile.invented["encryptionKey"], "signingKey": self.hostile.invented["signingKey"] }),
            ]
        } else {
            vec![]
        };
        std::fs::write(&self.runner.team_policy_path, json!({ "sessionTeamSync": [{ "gateway": self.runner.client.base(), "tenantId": "acme", "members": members }] }).to_string()).expect("managed policy");
    }
}

#[tokio::test]
async fn gateway_settings_cannot_opt_in_any_team_path() {
    let fixture = Fixture::new().await;
    let thread = uuid::Uuid::new_v4().to_string();
    let trusted = fixture.runner.directory().await.expect("personal directory");
    fixture
        .runner
        .prepare(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("personal sync");
    fixture.runner.hand_over(&trusted).await.expect("no handover");
    assert!(
        fixture
            .runner
            .team_content_key(&thread, 1, "invented", &trusted)
            .await
            .expect("reader gate")
            .is_none()
    );
    assert!(matches!(
        fixture.runner.team_epoch(&trusted).await.expect("epoch gate"),
        TeamState::Off
    ));
    let calls = fixture.hostile.calls.lock().expect("calls");
    assert!(
        !calls
            .iter()
            .any(|(_, path, _)| path.contains("team-key") || path.starts_with("/v1/orgs/members"))
    );
    assert!(calls.iter().any(|(method, path, body)| method == "PUT"
        && path.ends_with("/keys")
        && body["keys"][0]["deviceId"] == "dev_victim"));
}

#[tokio::test]
async fn invented_members_and_their_team_keys_count_only_after_local_admission() {
    let fixture = Fixture::new().await;
    fixture.opt_in(false);
    let thread = uuid::Uuid::new_v4().to_string();
    let trusted = fixture.runner.directory().await.expect("personal directory");
    assert!(matches!(
        fixture.runner.team_epoch(&trusted).await.expect("refused fake copy"),
        TeamState::Waiting
    ));
    fixture
        .runner
        .prepare(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("personal sync continues");
    assert!(
        !fixture
            .hostile
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|(method, path, _)| method == "PUT" && path.contains("team-key"))
    );
    fixture.opt_in(true);
    assert!(matches!(
        fixture.runner.team_epoch(&trusted).await.expect("admitted signer"),
        TeamState::Held(_)
    ));
    fixture
        .runner
        .prepare(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("explicit sharing");
    let shared = fixture.runner.keys.latest(&thread).expect("stored").expect("key");
    assert!(shared.holders.contains("tmk_attacker"));
    fixture.hostile.missing.store(true, Ordering::SeqCst);
    fixture
        .runner
        .prepare(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("404 is private");
    let private = fixture.runner.keys.latest(&thread).expect("stored").expect("key");
    assert!(private.epoch > shared.epoch);
    assert!(!private.holders.contains("tmk_attacker"));
    std::fs::remove_file(&fixture.runner.team_policy_path).expect("opt out");
    assert!(matches!(
        fixture.runner.team_epoch(&trusted).await.expect("removed policy"),
        TeamState::Off
    ));
}

#[tokio::test]
async fn legacy_revocations_survive_tenant_scoped_admission() {
    let fixture = Fixture::new().await;
    fixture.opt_in(true);
    fixture
        .runner
        .keys
        .remember(
            &format!("{}/team/invented", fixture.runner.owner),
            &trust::Memory {
                revoked: std::collections::HashSet::from(["dev_invented".into()]),
                ..Default::default()
            },
        )
        .expect("legacy observation");
    let trusted = fixture.runner.directory().await.expect("personal directory");
    assert!(matches!(
        fixture
            .runner
            .team_epoch(&trusted)
            .await
            .expect("replayed revoked signer"),
        TeamState::Waiting
    ));
    let migrated = fixture
        .runner
        .keys
        .memory(&format!("{}/team/acme/invented", fixture.runner.owner))
        .expect("migrated memory");
    assert!(migrated.revoked.contains("dev_invented"));
}

#[tokio::test]
async fn policy_changes_during_directory_reads_stop_epoch_creation_and_handover() {
    for create in [true, false] {
        for opt_out in [true, false] {
            let fixture = Fixture::new().await;
            fixture.opt_in(true);
            if create {
                *fixture.hostile.team.lock().expect("team") = json!({"key": null, "copies": []});
            }
            let trusted = fixture.runner.directory().await.expect("personal directory");
            fixture.hostile.pause_directory.store(true, Ordering::SeqCst);
            let runner = fixture.runner.clone();
            let pending = tokio::spawn(async move {
                if create {
                    runner.team_epoch(&trusted).await.map(|_| ())
                } else {
                    runner.hand_over(&trusted).await
                }
            });
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                fixture.hostile.directory_entered.notified(),
            )
            .await
            .expect("directory paused");
            if opt_out {
                std::fs::remove_file(&fixture.runner.team_policy_path).expect("opt out");
            } else {
                fixture.opt_in(false);
            }
            fixture.hostile.directory_resume.notify_one();
            assert!(
                pending.await.expect("team task").is_err(),
                "changed authorization refuses publication"
            );
            assert!(
                !fixture
                    .hostile
                    .calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .any(|(method, path, _)| method != "GET" && path.starts_with("/v1/orgs/team-key"))
            );
        }
    }
}

#[tokio::test]
async fn a_policy_change_during_the_final_team_read_opens_no_content_key() {
    for opt_out in [true, false] {
        let fixture = Fixture::new().await;
        fixture.opt_in(true);
        let trusted = fixture.runner.directory().await.expect("personal directory");
        assert!(
            fixture
                .runner
                .team_content_key(&fixture.thread, 1, "invented", &trusted)
                .await
                .expect("authorized read")
                .is_some()
        );
        fixture.hostile.pause_team.store(true, Ordering::SeqCst);
        let runner = fixture.runner.clone();
        let thread = fixture.thread.clone();
        let pending = tokio::spawn(async move { runner.team_content_key(&thread, 1, "invented", &trusted).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            fixture.hostile.team_entered.notified(),
        )
        .await
        .expect("last network read paused");
        if opt_out {
            std::fs::remove_file(&fixture.runner.team_policy_path).expect("opt out");
        } else {
            fixture.opt_in(false);
        }
        fixture.hostile.team_resume.notify_one();
        assert!(
            pending
                .await
                .expect("reader")
                .expect("personal reads continue")
                .is_none(),
            "stale local authorization must not open a team content key"
        );
    }
}
