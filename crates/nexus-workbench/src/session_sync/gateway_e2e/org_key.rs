//! The organization's recovery key through a real Gateway. The organization
//! chooses `user_and_org`. Ada's runner syncs, and locks nothing for the organization's key until she confirms its
//! fingerprint on this computer. Then every epoch is locked for it, and the vault's private half opens the thread. The
//! organization replaces the key: the runner locks nothing for the new one, which waits for Ada, and rotates the content
//! key. Ada confirms it, and the new key gets the whole history. The policy goes back to `user`: the key rotates again,
//! and the organization's key gets nothing new.
use super::*;
use crate::session_sync::approver;
use crate::session_sync::org_key::{self, OrgKeyState};

/// One copy of a thread's content key locked for an organization key, as its auditor reads it.
struct OrgCopy {
    version: i64,
    org: String,
    locked: Vec<u8>,
    sender: String,
    signature: Vec<u8>,
}

/// The organization's copies of `thread`, as its auditor reads them.
async fn org_copies(client: &Client, thread: &str) -> Vec<OrgCopy> {
    let list = client
        .get(&format!("/v1/threads/{thread}/org-keys"))
        .await
        .expect("org copies");
    list["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .map(|copy| OrgCopy {
            version: copy["keyVersion"].as_i64().expect("version"),
            org: copy["orgKeyId"].as_str().expect("org key id").to_string(),
            locked: b64u(copy["lockedKey"].as_str().expect("locked")),
            sender: copy["senderDeviceId"].as_str().expect("sender").to_string(),
            signature: b64u(copy["signature"].as_str().expect("signature")),
        })
        .collect()
}

/// Registers, or replaces, the organization's recovery key; its id.
async fn register_org_key(client: &Client, key: &e2e::HolderKey) -> String {
    let answer = client
        .put(
            "/v1/orgs/recovery-key",
            json!({ "encryptionKey": e2e::b64u(&key.encryption_key()) }),
        )
        .await
        .expect("register the org key");
    answer["key"]["id"].as_str().expect("key id").to_string()
}

/// Every event of `thread`, opened with the copies locked for `org` (its id and the vault's private half), after
/// checking each copy's signature with `sender`'s signing key. Returns each event's text.
async fn read_as_org(client: &Client, thread: &str, org: (&str, &e2e::HolderKey), sender: &DeviceKey) -> Vec<String> {
    let copies = org_copies(client, thread).await;
    let page = client
        .get(&format!("/v1/threads/{thread}/events?after=-1&limit=500"))
        .await
        .expect("events");
    let mut texts = Vec::new();
    for event in page["events"].as_array().expect("events") {
        let version = event["keyVersion"].as_i64().expect("sealed");
        let copy = copies
            .iter()
            .find(|copy| copy.version == version && copy.org == org.0)
            .unwrap_or_else(|| panic!("no copy of version {version} for {}", org.0));
        let (locked, signature) = (&copy.locked, &copy.signature);
        let epoch = e2e::epoch_of(version as i32).expect("epoch");
        let message = e2e::org_thread_key_message(thread, epoch, org.0, locked).expect("message");
        assert!(e2e::verify(&sender.signing_key(), &message, signature));
        let key = org
            .1
            .unlock_content_key(thread, epoch, org.0, locked)
            .expect("the vault's key opens its copy");
        let seq = event["seq"].as_u64().expect("seq");
        let plaintext = key
            .open_event(thread, seq, &b64u(event["sealed"].as_str().expect("sealed")))
            .expect("the event opens");
        let plaintext: Plaintext = serde_json::from_slice(&plaintext).expect("plaintext");
        let event: Value = serde_json::from_str(&plaintext.event_json).expect("event json");
        texts.push(event["text"].as_str().expect("text").to_string());
    }
    texts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_runner_locks_for_the_org_key_only_once_its_person_confirms_it() {
    let Some(mut gateway) = boot().await else { return };
    let root = std::env::temp_dir().join(format!("nexus-org-key-{}", uuid::Uuid::new_v4()));
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post("/v1/tenants", json!({ "name": "Acme", "email": "ada@acme.test" }))
        .await
        .expect("sign up");
    // Ada owns the organization: she registers its key, chooses the policy and audits the copies.
    let direct = person(&gateway.url(), signup["apiKey"].as_str().expect("API key"));
    let vault = e2e::HolderKey::generate();
    let first = register_org_key(&direct, &vault).await;
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user_and_org" }))
        .await
        .expect("content policy");
    let whoami = direct.get("/v1/auth/whoami").await.expect("whoami");
    let tenant = whoami["tenantId"].as_str().expect("tenant").to_string();
    let owner = approver::owner(&gateway.url(), whoami["userId"].as_str().expect("person"));

    let a_key = DeviceKey::generate();
    let a = register(&direct, &a_key, "laptop").await;
    let scope = format!("{}|acme", gateway.url());
    let keys = Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("key store"));
    let backend = Arc::new(
        GatewayBackend::connect(
            direct.clone(),
            scope.clone(),
            DeviceKey::from_secret_bytes(&a_key.secret_bytes()),
            keys.clone(),
            true,
        )
        .await
        .expect("backend"),
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
        .insert_thread(&super::super::tests::record("t_org", &scope))
        .expect("thread");
    wb.register_session_sync("t_org", Some(backend.identity()));
    let thread = wb
        .store()
        .sync_target("t_org")
        .expect("target")
        .expect("registered")
        .sync_id;
    let write = |text: &str| {
        wb.store()
            .append("t_org", 1_762_000_000_000, &user(text))
            .expect("event");
    };
    let sync = || async {
        let report = wb.sync_sessions_once().await;
        wb.inner.session_sync.lock().await.retry.clear();
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        report
    };
    let versions = |copies: &[OrgCopy], org: &str| {
        let mut found: Vec<i64> = copies
            .iter()
            .filter(|copy| copy.org == org)
            .map(|copy| copy.version)
            .collect();
        found.sort();
        found
    };

    // No pin: the thread syncs for Ada's devices, and nothing is locked for the organization.
    write("before the pin");
    assert_eq!(sync().await.confirmed_events, 1);
    assert!(org_copies(&direct, &thread).await.is_empty());
    let answer = direct.get("/v1/orgs/recovery-key").await.expect("org key");
    assert!(matches!(
        org_key::state(&keys, &owner, &tenant, &answer),
        Ok(OrgKeyState::Unconfirmed(_))
    ));

    // Ada compares the fingerprint with the vault's and confirms it: the next pass locks the epoch for the org key.
    let print = e2e::org_key_fingerprint(&vault.encryption_key());
    org_key::confirm(&keys, &owner, &tenant, &answer, &print).expect("confirmed");
    write("after the pin");
    assert_eq!(sync().await.confirmed_events, 1);
    let copies = org_copies(&direct, &thread).await;
    assert_eq!(versions(&copies, &first), [1]);
    assert!(copies.iter().all(|copy| copy.sender == a), "signed by Ada's device");
    assert_eq!(
        read_as_org(&direct, &thread, (&first, &vault), &a_key).await,
        ["before the pin", "after the pin"]
    );

    // The organization replaces its key. The runner has not confirmed it: it locks nothing for it, and rotates.
    let replaced = e2e::HolderKey::generate();
    let second = register_org_key(&direct, &replaced).await;
    let answer = direct.get("/v1/orgs/recovery-key").await.expect("org key");
    assert!(matches!(
        org_key::state(&keys, &owner, &tenant, &answer),
        Ok(OrgKeyState::Changed { .. })
    ));
    write("after the replacement");
    assert_eq!(sync().await.confirmed_events, 1);
    assert_eq!(keys.latest(&thread).expect("key store").expect("a key").epoch, 1);
    let copies = org_copies(&direct, &thread).await;
    assert_eq!(versions(&copies, &first), [1], "the old key gets nothing new");
    assert!(versions(&copies, &second).is_empty(), "the new key waits for Ada");

    // Ada confirms the new key: it gets every epoch, so it opens the whole thread.
    let print = e2e::org_key_fingerprint(&replaced.encryption_key());
    org_key::confirm(&keys, &owner, &tenant, &answer, &print).expect("confirmed again");
    write("after the second pin");
    assert_eq!(sync().await.confirmed_events, 1);
    assert_eq!(versions(&org_copies(&direct, &thread).await, &second), [1, 2]);
    assert_eq!(
        read_as_org(&direct, &thread, (&second, &replaced), &a_key).await,
        [
            "before the pin",
            "after the pin",
            "after the replacement",
            "after the second pin"
        ]
    );

    // Back to `user`: the key the organization holds rotates, and its next epoch is Ada's alone.
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("content policy");
    write("back to the person alone");
    assert_eq!(sync().await.confirmed_events, 1);
    assert_eq!(keys.latest(&thread).expect("key store").expect("a key").epoch, 2);
    let copies = org_copies(&direct, &thread).await;
    assert_eq!(versions(&copies, &second), [1, 2], "nothing for epoch 2");
    let opened = read_as(&direct, &thread, (&a, &a_key)).await;
    assert_eq!(opened.last().map(|(_, epoch)| *epoch), Some(2));

    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
