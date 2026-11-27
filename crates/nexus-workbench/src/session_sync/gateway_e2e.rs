//! Acceptance test: a thread run on device A appears, decryptable, on device B, through a real Gateway on
//! Postgres, after the connection drops mid-session and after Gateway itself restarts. And the way back: A
//! syncs with a recovery code set up, A is lost, and a new device B unlocks the history with the code alone, proves
//! the code with Gateway's challenge, becomes the root and approves the code again.
//!
//! It drives the real engine, `GatewayBackend` and `nexus_client::e2e`. Gateway is a separate server binary, started as a process:
//! this crate's Cargo workspace cannot link Gateway's crates. It runs when `NEXUS_SYNC_E2E_GATEWAY` names a Gateway
//! binary and `TEST_DATABASE_URL` a Postgres; otherwise it returns early.
//!
//! ```sh
//! NEXUS_SYNC_E2E_GATEWAY=/path/to/gateway \
//!   TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55445/postgres \
//!   cargo test --locked -p nexus-workbench gateway_e2e -- --nocapture
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexus_client::e2e::{self, DeviceKey};
use nexus_client::{Client, ClientOptions};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::trust::{self, Keys, Listed, Memory};
use super::*;
use crate::event::WbEvent;
use crate::store::Store;
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd};

/// The Gateway process, restartable on the same port and schema.
struct Gateway {
    bin: String,
    database: String,
    schema: String,
    port: u16,
    child: Option<tokio::process::Child>,
}

impl Gateway {
    async fn start(&mut self) {
        let child = crate::process::async_command(&self.bin)
            .env("HOST", "127.0.0.1")
            .env("PORT", self.port.to_string())
            .env("DATABASE_URL", &self.database)
            .env("NEXUS_DB_SCHEMA", &self.schema)
            .env("NEXUS_PROVIDER", "mock")
            .env("NEXUS_SECRET", "session-sync-e2e-secret")
            .env("NEXUS_UI_SECRET", "ui-secret")
            .env("NEXUS_RATE_LIMIT_PER_MINUTE", "100000")
            .env("NEXUS_LOG", "warn")
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start nexus-gateway");
        self.child = Some(child);
        let client = Client::new(ClientOptions::new(&self.url()));
        // Three minutes: under a loaded machine (the whole suite at once), migrating a fresh schema takes a while.
        for _ in 0..1800 {
            if client.get("/readyz").await.is_ok() {
                return;
            }
            if let Some(status) = self.child.as_mut().and_then(|child| child.try_wait().ok().flatten()) {
                panic!("nexus-gateway exited before it was ready: {status}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("nexus-gateway did not become ready in three minutes");
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().await.expect("stop nexus-gateway");
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// A fault-injecting transport: a TCP proxy in front of Gateway. Down, it cuts every open connection, pooled ones
/// included, and refuses new ones, as a laptop that went offline.
struct Proxy {
    port: u16,
    down: Arc<AtomicBool>,
    links: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn start(upstream: u16) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        let down = Arc::new(AtomicBool::new(false));
        let links: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let (accept_down, accept_links) = (down.clone(), links.clone());
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                if accept_down.load(Ordering::SeqCst) {
                    continue;
                }
                let link = tokio::spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(("127.0.0.1", upstream)).await {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                    }
                });
                accept_links.lock().expect("links").push(link);
            }
        });
        Proxy { port, down, links }
    }

    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
        if down {
            for link in self.links.lock().expect("links").drain(..) {
                link.abort();
            }
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

struct E2eHost(Arc<GatewayBackend>);

impl Host for E2eHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        Some(self.0.clone())
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        Box::pin(async { Err("no process".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &crate::event::Usage) -> Option<f64> {
        None
    }
    fn top_price(&self, _: &crate::event::Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}

fn person(gateway: &str, api_key: &str) -> Client {
    Client::new(ClientOptions {
        api_key: Some(api_key.to_string()),
        ..ClientOptions::new(gateway)
    })
}

async fn register(client: &Client, device: &DeviceKey, name: &str) -> String {
    let answer = client
        .post(
            "/v1/devices",
            json!({
                "name": name,
                "encryptionKey": e2e::b64u(&device.encryption_key()),
                "signingKey": e2e::b64u(&device.signing_key()),
            }),
        )
        .await
        .expect("register a device");
    answer["id"].as_str().expect("device id").to_string()
}

async fn approve(client: &Client, approver: (&str, &DeviceKey), id: &str, device: &DeviceKey) {
    let message = e2e::approval_message(id, &device.encryption_key(), &device.signing_key()).expect("message");
    client
        .post(
            &format!("/v1/devices/{id}/approve"),
            json!({ "approverDeviceId": approver.0, "signature": e2e::b64u(&approver.1.sign(&message)) }),
        )
        .await
        .expect("approve");
}

async fn revoke(client: &Client, revoker: (&str, &DeviceKey), id: &str) {
    let message = e2e::revocation_message(id).expect("message");
    client
        .post(
            &format!("/v1/devices/{id}/revoke"),
            json!({ "revokerDeviceId": revoker.0, "signature": e2e::b64u(&revoker.1.sign(&message)) }),
        )
        .await
        .expect("revoke");
}

fn b64u(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .expect("base64url")
}

/// Device B reads the thread as Nexus UI would: its own key copy per version, checked against its own verification of
/// the directory, then every event opened with `nexus_client::e2e`. Returns each event's plaintext and epoch.
async fn read_as(client: &Client, thread: &str, me: (&str, &DeviceKey)) -> Vec<(Plaintext, u64)> {
    let listed: Vec<Listed> =
        serde_json::from_value(client.get("/v1/devices").await.expect("devices")["devices"].clone()).expect("list");
    let keys = Keys {
        encryption: me.1.encryption_key(),
        signing: Some(me.1.signing_key()),
    };
    // B's person compared its approver's code when B was paired.
    let trusted = trust::verify(&listed, &keys, &mut trust::confirmed(&listed, &keys)).expect("B trusts its directory");
    let mut opened = Vec::new();
    let mut content_keys: HashMap<i64, e2e::ContentKey> = HashMap::new();
    let mut after = -1;
    loop {
        let page = client
            .get(&format!("/v1/threads/{thread}/events?after={after}&limit=500"))
            .await
            .expect("events");
        for event in page["events"].as_array().expect("events") {
            let version = event["keyVersion"].as_i64().expect("sealed");
            if let std::collections::hash_map::Entry::Vacant(slot) = content_keys.entry(version) {
                let copy = client
                    .get(&format!("/v1/threads/{thread}/keys?device={}&version={version}", me.0))
                    .await
                    .expect("B's key copy");
                let epoch = e2e::epoch_of(version as i32).expect("epoch");
                let locked = b64u(copy["lockedKey"].as_str().expect("locked"));
                let sender = copy["senderDeviceId"].as_str().expect("sender");
                let holder = trusted
                    .holders
                    .iter()
                    .find(|holder| holder.id == sender)
                    .expect("a trusted sender");
                let message = e2e::thread_key_message(thread, epoch, me.0, &locked).expect("message");
                assert!(e2e::verify(
                    &holder.keys.signing.expect("a device signs"),
                    &message,
                    &b64u(copy["signature"].as_str().expect("signature"))
                ));
                let key = me.1.unlock_content_key(thread, epoch, me.0, &locked).expect("unlock");
                slot.insert(key);
            }
            let key = &content_keys[&version];
            let seq = event["seq"].as_u64().expect("seq");
            let plaintext = key
                .open_event(thread, seq, &b64u(event["sealed"].as_str().expect("sealed")))
                .expect("B opens the event");
            opened.push((serde_json::from_slice(&plaintext).expect("plaintext"), key.epoch()));
            after = seq as i64;
        }
        if !page["more"].as_bool().unwrap_or(false) {
            return opened;
        }
    }
}

/// A Gateway on a new schema, started; `None` (the test skips) without `NEXUS_SYNC_E2E_GATEWAY` and `TEST_DATABASE_URL`.
async fn boot() -> Option<Gateway> {
    let (Ok(bin), Ok(database)) = (
        std::env::var("NEXUS_SYNC_E2E_GATEWAY"),
        std::env::var("TEST_DATABASE_URL"),
    ) else {
        eprintln!("skipped: set NEXUS_SYNC_E2E_GATEWAY and TEST_DATABASE_URL to run the Gateway end-to-end tests");
        return None;
    };
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port")
        .local_addr()
        .expect("address")
        .port();
    let mut gateway = Gateway {
        bin,
        database,
        schema: format!("session_sync_e2e_{}", uuid::Uuid::new_v4().simple()),
        port,
        child: None,
    };
    gateway.start().await;
    Some(gateway)
}

fn user(text: &str) -> WbEvent {
    WbEvent::User {
        text: text.into(),
        images: vec![],
        from: None,
    }
}

#[path = "gateway_e2e/computer.rs"]
mod computer;
#[path = "gateway_e2e/control.rs"]
mod control;
#[path = "gateway_e2e/handover_upgrade.rs"]
mod handover_upgrade;
#[path = "gateway_e2e/idle_keys.rs"]
mod idle_keys;
#[path = "gateway_e2e/key_publication.rs"]
mod key_publication;
#[path = "gateway_e2e/org_key.rs"]
mod org_key;
#[path = "gateway_e2e/team.rs"]
mod team;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_run_on_a_reaches_paired_b_across_an_outage_and_a_gateway_restart() {
    let Some(mut gateway) = boot().await else {
        return;
    };
    let proxy = Proxy::start(gateway.port).await;
    let root = std::env::temp_dir().join(format!("nexus-session-sync-e2e-{}", uuid::Uuid::new_v4()));

    // A person whose organization lets content sync for them alone (`user`).
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post("/v1/tenants", json!({ "name": "Acme", "email": "ada@acme.test" }))
        .await
        .expect("sign up");
    let api_key = signup["apiKey"].as_str().expect("api key").to_string();
    let direct = person(&gateway.url(), &api_key);
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("content policy");

    // Device A, the runner, is the person's first device; C is paired now and revoked during the outage.
    let a_key = DeviceKey::generate();
    let a = register(&direct, &a_key, "laptop").await;
    let c_key = DeviceKey::generate();
    let c = register(&direct, &c_key, "old phone").await;
    approve(&direct, (&a, &a_key), &c, &c_key).await;

    let scope = format!("{}|acme", gateway.url());
    let keys = Arc::new(KeyStore::open(&root.join("session-sync-keys.sqlite3")).expect("key store"));
    let runner = person(&proxy.url(), &api_key);
    let backend = Arc::new(
        GatewayBackend::connect(
            runner,
            scope.clone(),
            DeviceKey::from_secret_bytes(&a_key.secret_bytes()),
            keys.clone(),
            true,
        )
        .await
        .expect("A's backend"),
    );
    let wb = Workbench::new(
        Store::open(&root.join("workbench.sqlite3")).expect("store"),
        Arc::new(E2eHost(backend.clone())),
        Config {
            scope: scope.clone(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    wb.store()
        .insert_thread(&super::tests::record("t_one", &scope))
        .expect("thread");
    wb.register_session_sync("t_one", Some(backend.identity()));
    let sync_id = wb
        .store()
        .sync_target("t_one")
        .expect("target")
        .expect("registered")
        .sync_id;
    let mut written = Vec::new();
    let mut write = |text: &str| {
        let event = user(text);
        wb.store().append("t_one", 1_762_000_000_000, &event).expect("event");
        written.push(text.to_string());
    };
    let sync = || async {
        let report = wb.sync_sessions_once().await;
        wb.inner.session_sync.lock().await.retry.clear();
        report
    };
    // Online, passes repeat until `events` more are confirmed: on a loaded machine a request can pass the engine's
    // five-second limit, and the next pass resumes it.
    let sync_online = |events: usize| async move {
        let mut confirmed = 0;
        for _ in 0..40 {
            let report = sync().await;
            confirmed += report.confirmed_events;
            if confirmed >= events && report.failures.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        confirmed
    };

    write("first prompt");
    write("second prompt");
    assert_eq!(sync_online(2).await, 2);

    // The connection drops mid-session: events keep arriving, and every pass fails without losing them.
    proxy.set_down(true);
    write("written offline");
    write("still offline");
    for _ in 0..3 {
        let report = sync().await;
        assert_eq!(report.confirmed_events, 0);
        assert!(!report.failures.is_empty());
    }
    let head = direct.get(&format!("/v1/threads/{sync_id}")).await.expect("head");
    assert_eq!(head["lastSeq"], 1, "nothing new reached Gateway while offline");

    // Meanwhile the person pairs B with A's key, and revokes C, through another connection.
    let b_key = DeviceKey::generate();
    let b = register(&direct, &b_key, "desktop").await;
    approve(&direct, (&a, &a_key), &b, &b_key).await;
    revoke(&direct, (&a, &a_key), &c).await;

    proxy.set_down(false);
    assert_eq!(sync_online(2).await, 2);

    // Gateway itself restarts mid-session.
    gateway.stop().await;
    write("during the restart");
    assert!(!sync().await.failures.is_empty());
    gateway.start().await;
    write("after the restart");
    assert_eq!(sync_online(2).await, 2);

    // B decrypts every event, in order, from both epochs: C's revocation rotated the key.
    let opened = read_as(&direct, &sync_id, (&b, &b_key)).await;
    let texts: Vec<String> = opened
        .iter()
        .map(|(plaintext, _)| {
            let event: Value = serde_json::from_str(&plaintext.event_json).expect("event json");
            event["text"].as_str().expect("text").to_string()
        })
        .collect();
    assert_eq!(texts, written);
    assert!(
        opened
            .iter()
            .all(|(plaintext, _)| plaintext.thread.title == "secret title")
    );
    let epochs: Vec<u64> = opened.iter().map(|(_, epoch)| *epoch).collect();
    assert_eq!(
        epochs,
        [0, 0, 1, 1, 1, 1],
        "the revoke rotated the key before the next batch"
    );
    let rotated = keys.latest(&sync_id).expect("key store").expect("a key");
    assert_eq!(rotated.epoch, 1);
    assert!(
        rotated.holders.contains(&b) && !rotated.holders.contains(&c),
        "{:?}",
        rotated.holders
    );

    // Gateway stores ciphertext only.
    let raw = direct
        .get(&format!("/v1/threads/{sync_id}/events?after=-1&limit=500"))
        .await
        .expect("events")
        .to_string();
    for secret in ["prompt", "secret title", "t_one"] {
        assert!(!raw.contains(secret), "{secret}");
    }

    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}

/// The events of `thread`, opened with `unlock`'s content key per key version, after checking each copy's signature
/// with its sender's listed signing key. Returns each event's text.
async fn read_with(
    client: &Client,
    thread: &str,
    holder_id: &str,
    unlock: impl Fn(u64, &[u8]) -> e2e::ContentKey,
) -> Vec<String> {
    let listed: Vec<Listed> =
        serde_json::from_value(client.get("/v1/devices").await.expect("devices")["devices"].clone()).expect("list");
    let mut texts = Vec::new();
    let mut content_keys: HashMap<i64, e2e::ContentKey> = HashMap::new();
    let page = client
        .get(&format!("/v1/threads/{thread}/events?after=-1&limit=500"))
        .await
        .expect("events");
    for event in page["events"].as_array().expect("events") {
        let version = event["keyVersion"].as_i64().expect("sealed");
        if let std::collections::hash_map::Entry::Vacant(slot) = content_keys.entry(version) {
            let copy = client
                .get(&format!(
                    "/v1/threads/{thread}/keys?device={holder_id}&version={version}"
                ))
                .await
                .expect("the holder's key copy");
            let epoch = e2e::epoch_of(version as i32).expect("epoch");
            let locked = b64u(copy["lockedKey"].as_str().expect("locked"));
            let sender = listed
                .iter()
                .find(|device| device.id == copy["senderDeviceId"].as_str().expect("sender"))
                .and_then(|device| device.signing_key.as_deref())
                .expect("a listed sender with a signing key");
            let message = e2e::thread_key_message(thread, epoch, holder_id, &locked).expect("message");
            assert!(e2e::verify(
                &b64u(sender).try_into().expect("32 bytes"),
                &message,
                &b64u(copy["signature"].as_str().expect("signature"))
            ));
            slot.insert(unlock(epoch, &locked));
        }
        let seq = event["seq"].as_u64().expect("seq");
        let plaintext = content_keys[&version]
            .open_event(thread, seq, &b64u(event["sealed"].as_str().expect("sealed")))
            .expect("the event opens");
        let plaintext: Plaintext = serde_json::from_slice(&plaintext).expect("plaintext");
        let event: Value = serde_json::from_str(&plaintext.event_json).expect("event json");
        texts.push(event["text"].as_str().expect("text").to_string());
    }
    texts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_is_lost_b_unlocks_the_history_with_the_recovery_code_and_becomes_the_root() {
    let Some(mut gateway) = boot().await else {
        return;
    };
    let root = std::env::temp_dir().join(format!("nexus-session-sync-way-back-{}", uuid::Uuid::new_v4()));
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post("/v1/tenants", json!({ "name": "Acme", "email": "ada@acme.test" }))
        .await
        .expect("sign up");
    let api_key = signup["apiKey"].as_str().expect("api key").to_string();
    let direct = person(&gateway.url(), &api_key);
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("content policy");
    let person_id = direct.get("/v1/auth/whoami").await.expect("whoami")["userId"]
        .as_str()
        .expect("a person")
        .to_string();

    // Setup on A, the first device: a recovery code, shown once, registered as a holder and approved by A.
    let a_key = DeviceKey::generate();
    let a = register(&direct, &a_key, "laptop").await;
    let printed = e2e::RecoveryCode::generate().display().to_string();
    let setup_holder =
        e2e::HolderKey::from_recovery_code(&e2e::RecoveryCode::parse(&printed).expect("the code"), &person_id)
            .expect("holder");
    let holder = direct
        .post(
            "/v1/devices",
            json!({
                "name": "Recovery code",
                "kind": "recovery_code",
                "encryptionKey": e2e::b64u(&setup_holder.encryption_key()),
            }),
        )
        .await
        .expect("register the code")["id"]
        .as_str()
        .expect("holder id")
        .to_string();
    let approval = e2e::holder_approval_message(&holder, e2e::HolderKind::RecoveryCode, &setup_holder.encryption_key())
        .expect("message");
    direct
        .post(
            &format!("/v1/devices/{holder}/approve"),
            json!({ "approverDeviceId": a, "signature": e2e::b64u(&a_key.sign(&approval)) }),
        )
        .await
        .expect("A approves the code");
    drop(setup_holder);

    // A's runner syncs a thread. It locks the content key for the code too.
    let scope = format!("{}|acme", gateway.url());
    let keys = Arc::new(KeyStore::open(&root.join("session-sync-keys.sqlite3")).expect("key store"));
    let backend = Arc::new(
        GatewayBackend::connect(
            person(&gateway.url(), &api_key),
            scope.clone(),
            DeviceKey::from_secret_bytes(&a_key.secret_bytes()),
            keys.clone(),
            true,
        )
        .await
        .expect("A's backend"),
    );
    let wb = Workbench::new(
        Store::open(&root.join("workbench.sqlite3")).expect("store"),
        Arc::new(E2eHost(backend.clone())),
        Config {
            scope: scope.clone(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    wb.store()
        .insert_thread(&super::tests::record("t_one", &scope))
        .expect("thread");
    wb.register_session_sync("t_one", Some(backend.identity()));
    let sync_id = wb
        .store()
        .sync_target("t_one")
        .expect("target")
        .expect("registered")
        .sync_id;
    let written = ["first prompt", "second prompt"];
    for text in written {
        wb.store()
            .append("t_one", 1_762_000_000_000, &user(text))
            .expect("event");
    }
    let mut confirmed = 0;
    for _ in 0..40 {
        let report = wb.sync_sessions_once().await;
        wb.inner.session_sync.lock().await.retry.clear();
        confirmed += report.confirmed_events;
        if confirmed >= written.len() && report.failures.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(confirmed, written.len());
    let stored = keys.latest(&sync_id).expect("key store").expect("a key");
    assert!(stored.holders.contains(&holder), "{:?}", stored.holders);

    // A is lost, unrevoked. On B, the person types the printed code, in lower case and without hyphens.
    drop(wb);
    let typed = printed.to_lowercase().replace('-', "");
    let unlocked = e2e::HolderKey::from_recovery_code(&e2e::RecoveryCode::parse(&typed).expect("parses"), &person_id)
        .expect("holder");
    let listed: Vec<Listed> =
        serde_json::from_value(direct.get("/v1/devices").await.expect("devices")["devices"].clone()).expect("list");
    let found = listed
        .iter()
        .find(|device| {
            device.kind == "recovery_code"
                && device.state == "trusted"
                && device.encryption_key == e2e::b64u(&unlocked.encryption_key())
        })
        .expect("the code's holder is listed");
    assert_eq!(found.id, holder);
    // B reads the history with the code's copies alone.
    let texts = read_with(&direct, &sync_id, &holder, |epoch, locked| {
        unlocked
            .unlock_content_key(&sync_id, epoch, &holder, locked)
            .expect("the code unlocks")
    })
    .await;
    assert_eq!(texts, written);

    // B proves the code: it opens the challenge Gateway sealed to the code's holder. Without the proof, a sign-in alone
    // resets nothing.
    let b_key = DeviceKey::generate();
    let b_body = |proof: Option<&str>| {
        let mut body = json!({
            "name": "new laptop",
            "encryptionKey": e2e::b64u(&b_key.encryption_key()),
            "signingKey": e2e::b64u(&b_key.signing_key()),
        });
        if let Some(proof) = proof {
            body["holderId"] = json!(holder);
            body["challenge"] = json!(proof);
        }
        body
    };
    assert!(
        direct.post("/v1/devices/recover", b_body(None)).await.is_err(),
        "no proof, no reset"
    );
    let challenges = direct
        .post("/v1/devices/recover/challenge", json!({}))
        .await
        .expect("a challenge");
    let sealed = challenges["challenges"]
        .as_array()
        .expect("challenges")
        .iter()
        .find(|entry| entry["holderId"] == holder.as_str())
        .and_then(|entry| entry["sealed"].as_str())
        .expect("the code's challenge");
    let opened = unlocked
        .open_recovery_challenge(&person_id, &holder, &b64u(sealed))
        .expect("the code opens its challenge");
    // B recovers: A is revoked, and B is the new root.
    let recovered = direct
        .post("/v1/devices/recover", b_body(Some(&e2e::b64u(&*opened))))
        .await
        .expect("recover");
    assert_eq!(recovered["state"], "trusted");
    assert_eq!(recovered["approvedBy"], Value::Null);
    let b = recovered["id"].as_str().expect("B's id").to_string();
    // B approves the code again, and copies the history's keys for itself.
    direct
        .post(
            &format!("/v1/devices/{holder}/approve"),
            json!({ "approverDeviceId": b, "signature": e2e::b64u(&b_key.sign(&approval)) }),
        )
        .await
        .expect("B approves the code again");
    let copy = direct
        .get(&format!("/v1/threads/{sync_id}/keys?device={holder}"))
        .await
        .expect("the code's newest copy");
    let version = copy["keyVersion"].as_i64().expect("version") as i32;
    let epoch = e2e::epoch_of(version).expect("epoch");
    let content = unlocked
        .unlock_content_key(
            &sync_id,
            epoch,
            &holder,
            &b64u(copy["lockedKey"].as_str().expect("locked")),
        )
        .expect("unlock");
    let mine = content
        .lock_for(&sync_id, &b, &b_key.encryption_key())
        .expect("lock for B");
    let message = e2e::thread_key_message(&sync_id, epoch, &b, &mine).expect("message");
    direct
        .put(
            &format!("/v1/threads/{sync_id}/keys"),
            json!({
                "keyVersion": version,
                "senderDeviceId": b,
                "keys": [{ "deviceId": b, "lockedKey": e2e::b64u(&mine), "signature": e2e::b64u(&b_key.sign(&message)) }],
            }),
        )
        .await
        .expect("B's own copy");

    // B's directory: A revoked, B the root, the code trusted through B. B reads the thread with its own key.
    let listed: Vec<Listed> =
        serde_json::from_value(direct.get("/v1/devices").await.expect("devices")["devices"].clone()).expect("list");
    assert_eq!(
        listed
            .iter()
            .find(|device| device.id == a)
            .map(|device| device.state.as_str()),
        Some("revoked")
    );
    let b_keys = Keys {
        encryption: b_key.encryption_key(),
        signing: Some(b_key.signing_key()),
    };
    let trusted = trust::verify(&listed, &b_keys, &mut Memory::default()).expect("B trusts its directory");
    let ids: Vec<&str> = trusted.holders.iter().map(|holder| holder.id.as_str()).collect();
    assert_eq!(ids, [b.as_str(), holder.as_str()]);
    let opened = read_as(&direct, &sync_id, (&b, &b_key)).await;
    let texts: Vec<String> = opened
        .iter()
        .map(|(plaintext, _)| {
            let event: Value = serde_json::from_str(&plaintext.event_json).expect("event json");
            event["text"].as_str().expect("text").to_string()
        })
        .collect();
    assert_eq!(texts, written);

    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
