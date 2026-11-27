//! Acceptance test: `nexus-host` pairs as a trusted device, a session run on it appears, decryptable, on
//! the person's other device, and after a revoke it uploads nothing more and logs why, with no content.
//!
//! The host is device A: its key file, `pair`, `HostSync` and the workbench's engine, with the stand-in `claude`. Device
//! B is the person's laptop, which approves the host by its code and reads the thread as Nexus UI does. Gateway is a
//! separate server binary, as in nexus-workbench's `gateway_e2e.rs`. It runs when
//! `NEXUS_SYNC_E2E_GATEWAY` names a Gateway binary and `TEST_DATABASE_URL` a Postgres; otherwise it returns early.
//!
//! ```sh
//! NEXUS_SYNC_E2E_GATEWAY=/path/to/gateway \
//!   TEST_DATABASE_URL=postgres://postgres@127.0.0.1:55445/postgres \
//!   cargo test --locked -p nexus-host --test session_sync -- --nocapture
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use nexus_client::e2e::{self, DeviceKey};
use nexus_client::{Client, ClientOptions};
use nexus_host::files::Paths;
use nexus_host::session_sync::{self, HostSync, REVOKED, SYNC_ON};
use nexus_workbench::event::{PermissionMode, SessionAgent, Usage, Via};
use nexus_workbench::process::Spawn;
use nexus_workbench::session_sync::{Backend, Batch, ContentPolicy};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, NewThread, ProcessEnd, Workbench};
use serde_json::{Value, json};

struct Gateway {
    child: tokio::process::Child,
    url: String,
}

impl Gateway {
    async fn start(bin: &str, database: &str) -> Gateway {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let schema = format!("host_sync_e2e_{}_{}", std::process::id(), nexus_workbench::now_ms());
        let child = nexus_workbench::process::async_command(bin)
            .env("HOST", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("DATABASE_URL", database)
            .env("NEXUS_DB_SCHEMA", schema)
            .env("NEXUS_PROVIDER", "mock")
            .env("NEXUS_SECRET", "host-sync-e2e-secret")
            .env("NEXUS_UI_SECRET", "ui-secret")
            .env("NEXUS_RATE_LIMIT_PER_MINUTE", "100000")
            .env("NEXUS_LOG", "warn")
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("start nexus-gateway");
        let mut gateway = Gateway {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let client = Client::new(ClientOptions::new(&gateway.url));
        for _ in 0..1800 {
            if client.get("/readyz").await.is_ok() {
                return gateway;
            }
            if let Ok(Some(status)) = gateway.child.try_wait() {
                panic!("nexus-gateway exited before it was ready: {status}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("nexus-gateway did not become ready in three minutes");
    }
}

/// The host's `Host::session_sync` is `HostSync`'s, as `HeadlessHost`'s; its agent is the stand-in `claude`.
struct SyncHost {
    sync: Arc<HostSync>,
    memory: PathBuf,
}

impl Host for SyncHost {
    fn session_sync(&self) -> Option<Arc<dyn Backend>> {
        self.sync.current()
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let env: HashMap<String, String> =
            HashMap::from([("FAKE_CLAUDE_DIR".into(), self.memory.to_string_lossy().into_owned())]);
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
                    args: request.args.clone(),
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some("ns_1".into()),
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn top_price(&self, _: &Usage) -> Option<f64> {
        Some(0.0)
    }
    fn emit(&self, _: Emit) {}
}

fn person(gateway: &str, api_key: &str) -> Client {
    Client::new(ClientOptions {
        api_key: Some(api_key.to_string()),
        ..ClientOptions::new(gateway)
    })
}

fn b64u(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .expect("base64url")
}

fn key32(text: &str) -> [u8; 32] {
    b64u(text).try_into().expect("32 bytes")
}

/// The synced thread's id and its last event number in Gateway.
async fn synced(client: &Client) -> Option<(String, i64)> {
    let list = client.get("/v1/threads").await.expect("threads");
    let thread = list["threads"].as_array()?.first()?.clone();
    Some((thread["threadId"].as_str()?.to_string(), thread["lastSeq"].as_i64()?))
}

/// Every event's `event_json`, opened as device B: its own copy of each key version, then `nexus_client::e2e`.
async fn read_as(client: &Client, thread: &str, me: (&str, &DeviceKey)) -> Vec<Value> {
    let page = client
        .get(&format!("/v1/threads/{thread}/events?after=-1&limit=500"))
        .await
        .expect("events");
    let mut opened = Vec::new();
    for event in page["events"].as_array().expect("events") {
        let version = event["keyVersion"].as_i64().expect("sealed");
        let copy = client
            .get(&format!("/v1/threads/{thread}/keys?device={}&version={version}", me.0))
            .await
            .expect("B's key copy");
        let epoch = e2e::epoch_of(version as i32).expect("epoch");
        let locked = b64u(copy["lockedKey"].as_str().expect("locked"));
        let key =
            me.1.unlock_content_key(thread, epoch, me.0, &locked)
                .expect("B unlocks the key");
        let seq = event["seq"].as_u64().expect("seq");
        let plaintext = key
            .open_event(thread, seq, &b64u(event["sealed"].as_str().expect("sealed")))
            .expect("B opens the event");
        let plaintext: Value = serde_json::from_slice(&plaintext).expect("plaintext");
        opened.push(serde_json::from_str(plaintext["event_json"].as_str().expect("event json")).expect("event"));
    }
    opened
}

fn turns_ended(wb: &Workbench, thread: &str) -> usize {
    wb.events(thread, 0)
        .expect("events")
        .iter()
        .filter(|stored| serde_json::to_value(&stored.event).expect("event")["kind"] == "turnEnd")
        .count()
}

async fn until(what: &str, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..240 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paired_host_syncs_its_sessions_until_it_is_revoked() {
    let (Ok(bin), Ok(database)) = (
        std::env::var("NEXUS_SYNC_E2E_GATEWAY"),
        std::env::var("TEST_DATABASE_URL"),
    ) else {
        eprintln!("skipped: set NEXUS_SYNC_E2E_GATEWAY and TEST_DATABASE_URL to run the host's two-device test");
        return;
    };
    let gateway = Gateway::start(&bin, &database).await;
    let signup = Client::new(ClientOptions::new(&gateway.url))
        .post("/v1/tenants", json!({ "name": "Acme", "email": "ada@acme.test" }))
        .await
        .expect("sign up");
    let api_key = signup["apiKey"].as_str().expect("api key").to_string();
    let laptop = person(&gateway.url, &api_key);
    laptop
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("content policy");

    // B, the laptop, is the person's first device.
    let b_key = DeviceKey::generate();
    let b = laptop
        .post(
            "/v1/devices",
            json!({ "name": "laptop", "encryptionKey": e2e::b64u(&b_key.encryption_key()),
                    "signingKey": e2e::b64u(&b_key.signing_key()) }),
        )
        .await
        .expect("register B")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // The host pairs: it prints its code, and B approves the pending device whose keys give that code.
    let root = std::env::temp_dir().join(format!("nh-sync-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths::at(root.join("host"));
    paths.create().expect("host folder");
    let host_client = person(&gateway.url, &api_key);
    let printed: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut say = {
        let printed = printed.clone();
        move |line: &str| printed.lock().expect("lock").push(line.to_string())
    };
    // Then the person checks that the approver it names, B, shows the code it prints.
    let b_code = e2e::short_code(&b_key.fingerprint());
    let mut confirm = |shown: &nexus_workbench::session_sync::approver::Approver| shown.id == b && shown.code == b_code;
    let pairing = session_sync::pair(
        &host_client,
        &paths,
        "nexus-host on e2e",
        Duration::from_millis(200),
        &mut say,
        &mut confirm,
    );
    let approving = async {
        let mut pending = None;
        until("the host's pending device", async || {
            let listed = laptop.get("/v1/devices").await.expect("devices");
            pending = listed["devices"]
                .as_array()
                .expect("devices")
                .iter()
                .find(|device| device["state"] == "pending")
                .cloned();
            pending.is_some()
        })
        .await;
        let device = pending.expect("pending");
        let (encryption, signing) = (
            key32(device["encryptionKey"].as_str().expect("key")),
            key32(device["signingKey"].as_str().expect("key")),
        );
        // As Settings › Devices: the code is checked against the device's own keys, then the approval is signed.
        let code = e2e::short_code(&e2e::fingerprint(&encryption, Some(&signing)));
        let shown = printed.lock().expect("lock").concat().replace(' ', "");
        assert!(
            shown.contains(&code),
            "the host printed the code of the keys it registered"
        );
        let id = device["id"].as_str().expect("id").to_string();
        let message = e2e::approval_message(&id, &encryption, &signing).expect("message");
        laptop
            .post(
                &format!("/v1/devices/{id}/approve"),
                json!({ "approverDeviceId": b, "signature": e2e::b64u(&b_key.sign(&message)) }),
            )
            .await
            .expect("approve");
        id
    };
    let (paired, host_device) = tokio::join!(pairing, approving);
    assert_eq!(paired.expect("paired").id, host_device);

    // `serve`'s check turns sync on.
    let logged: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = logged.clone();
    let sync = Arc::new(HostSync::new(Arc::new(move |line: &str| {
        sink.lock().expect("lock").push(line.to_string())
    })));
    let scope = format!("{}|acme", gateway.url);
    sync.refresh(&host_client, &paths, &scope).await;
    assert!(sync.current().is_some(), "{:?}", logged.lock().expect("lock"));

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    let memory = root.join("memory");
    std::fs::create_dir_all(&memory).expect("memory");
    let wb = Workbench::new(
        Store::open(&paths.store()).expect("store"),
        Arc::new(SyncHost {
            sync: sync.clone(),
            memory,
        }),
        Config {
            scope: scope.clone(),
            shell: nexus_workbench::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    let worker = {
        let wb = wb.clone();
        tokio::spawn(async move { wb.run_session_sync_until_shutdown().await })
    };
    let project = wb.add_project(&repo).expect("project");
    let thread = wb
        .create_thread(NewThread {
            name: None,
            project_id: project.id,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default,
            effort: None,
            via: Via::Nexus,
            provider: None,
            text: "secret prompt".into(),
            images: vec![],
            worktree: None,
            parent: None,
            orchestration: false,
        })
        .await
        .expect("thread")
        .id;
    until("the first turn", async || turns_ended(&wb, &thread) == 1).await;
    let local = wb.events(&thread, 0).expect("events").len() as i64;
    let mut sync_id = String::new();
    until("the session in Gateway", async || match synced(&laptop).await {
        Some((id, last)) if last + 1 == local => {
            sync_id = id;
            true
        }
        _ => false,
    })
    .await;

    // B reads the session the host ran: every event, decrypted.
    let opened = read_as(&laptop, &sync_id, (&b, &b_key)).await;
    assert_eq!(opened.len() as i64, local);
    assert!(opened.iter().any(|event| event["text"] == "secret prompt"));

    // B revokes the host. The next turn's events stay on the host.
    let live = sync.current().expect("live");
    let message = e2e::revocation_message(&host_device).expect("message");
    laptop
        .post(
            &format!("/v1/devices/{host_device}/revoke"),
            json!({ "revokerDeviceId": b, "signature": e2e::b64u(&b_key.sign(&message)) }),
        )
        .await
        .expect("revoke");
    wb.send(&thread, "another secret").await.expect("send");
    until("the second turn", async || turns_ended(&wb, &thread) == 2).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        synced(&laptop).await.map(|(_, last)| last + 1),
        Some(local),
        "nothing new uploaded"
    );

    // Even ciphertext already staged is refused before it leaves: the host checks its device before each upload.
    let staged = Batch {
        thread: sync_id.clone(),
        policy: ContentPolicy::UserOnly,
        events: vec![],
    };
    assert!(live.append(staged).await.is_err());
    assert!(sync.current().is_none(), "the slot is empty: the engine makes no call");
    sync.refresh(&host_client, &paths, &scope).await;
    assert!(sync.current().is_none());
    let lines = logged.lock().expect("lock").clone();
    assert_eq!(lines, [SYNC_ON, REVOKED], "each said once");
    for line in &lines {
        for secret in ["secret", &thread, &sync_id, root.to_string_lossy().as_ref()] {
            assert!(!line.contains(secret), "{line}");
        }
    }

    wb.shutdown().await;
    let _ = worker.await;
    let _ = std::fs::remove_dir_all(root);
}
