use super::*;
use std::collections::BTreeSet;

#[tokio::test]
async fn handover_v2_rotates_a_key_shared_with_a_holder_outside_corrected_trust() {
    let Some(gateway) = boot().await else { return };
    let client = Client::new(ClientOptions::new(&gateway.url()));
    let signup = client
        .post(
            "/v1/tenants",
            json!({ "name": "Handover upgrade", "email": "handover-upgrade@test.example" }),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    client
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("policy");
    let device = DeviceKey::generate();
    let id = register(&client, &device, "Upgraded runner").await;
    let keys = Arc::new(KeyStore::in_memory().expect("keys"));
    let backend = GatewayBackend::connect(client, "upgrade".into(), device, keys.clone(), true)
        .await
        .expect("runner");
    let thread = uuid::Uuid::new_v4().to_string();
    backend
        .key(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("initial key");
    let before = keys.latest(&thread).expect("read").expect("stored");
    keys.add_holders(&thread, before.epoch, &BTreeSet::from(["dev_legacy_injected".into()]))
        .expect("legacy sharing record");
    backend
        .key(&thread, Some(before.epoch), ContentPolicy::UserOnly)
        .await
        .expect("new writes");
    let after = keys.latest(&thread).expect("read").expect("rotated");
    assert!(after.epoch > before.epoch);
    assert_eq!(after.holders, BTreeSet::from([id]));
}

#[tokio::test]
async fn legacy_self_only_recipient_history_rotates_before_writes_and_retains_old_reads() {
    let Some(gateway) = boot().await else { return };
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Legacy key history","email":"legacy-history@test.example"}),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    let device = DeviceKey::generate();
    let id = register(&client, &device, "Runner").await;
    let keys = Arc::new(KeyStore::in_memory().expect("keys"));
    let thread = uuid::Uuid::new_v4().to_string();
    let legacy = e2e::ContentKey::generate(0);
    let locked = legacy
        .lock_for(&thread, &id, &device.encryption_key())
        .expect("legacy copy");
    let signature = device.sign(&e2e::thread_key_message(&thread, 0, &id, &locked).expect("message"));
    client.put(&format!("/v1/threads/{thread}/keys"), json!({"keyVersion":1,"senderDeviceId":id,"keys":[{"deviceId":id,"lockedKey":e2e::b64u(&locked),"signature":e2e::b64u(&signature)}]})).await.expect("legacy publication");
    keys.insert(
        &thread,
        &keystore::StoredKey {
            epoch: 0,
            holder: id.clone(),
            locked,
            holders: BTreeSet::from([id]),
        },
    )
    .expect("legacy row with incomplete history");
    let reader = DeviceKey::from_secret_bytes(&device.secret_bytes());
    let backend = GatewayBackend::connect(client, "legacy".into(), device, keys.clone(), true)
        .await
        .expect("runner");
    let next = backend
        .key(&thread, Some(0), ContentPolicy::UserOnly)
        .await
        .expect("new writes");
    assert!(
        next.epoch() > 0,
        "a self-only legacy record cannot prove prior recipients"
    );
    let history = keys.all(&thread).expect("retained keys");
    let old = history.iter().find(|key| key.epoch == 0).expect("old row");
    let readable = reader
        .unlock_content_key(&thread, 0, &old.holder, &old.locked)
        .expect("old key still decrypts");
    assert_eq!(readable.epoch(), 0);
    assert_eq!(
        keys.all(&thread)
            .expect("readable history")
            .iter()
            .map(|row| row.epoch)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[tokio::test]
async fn an_adopted_epoch_is_preserved_for_reads_but_never_reused_for_writes() {
    let Some(gateway) = boot().await else { return };
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({"name":"Adopted key history","email":"adopted-history@test.example"}),
        )
        .await
        .expect("signup");
    let client = person(&gateway.url(), signup["apiKey"].as_str().expect("key"));
    let device = DeviceKey::generate();
    let id = register(&client, &device, "Runner").await;
    let thread = uuid::Uuid::new_v4().to_string();
    let adopted = e2e::ContentKey::generate(0);
    let locked = adopted
        .lock_for(&thread, &id, &device.encryption_key())
        .expect("remote copy");
    let signature = device.sign(&e2e::thread_key_message(&thread, 0, &id, &locked).expect("message"));
    client.put(&format!("/v1/threads/{thread}/keys"), json!({"keyVersion":1,"senderDeviceId":id,"keys":[{"deviceId":id,"lockedKey":e2e::b64u(&locked),"signature":e2e::b64u(&signature)}]})).await.expect("existing Gateway epoch");
    let keys = Arc::new(KeyStore::in_memory().expect("keys"));
    let reader = DeviceKey::from_secret_bytes(&device.secret_bytes());
    let backend = GatewayBackend::connect(client, "adopted".into(), device, keys.clone(), true)
        .await
        .expect("runner");
    let next = backend
        .key(&thread, None, ContentPolicy::UserOnly)
        .await
        .expect("new writes");
    assert!(next.epoch() > 0, "a signed copy does not attest its recipient history");
    let history = keys.all(&thread).expect("retained keys");
    let old = history.iter().find(|key| key.epoch == 0).expect("old row");
    let readable = reader
        .unlock_content_key(&thread, 0, &old.holder, &old.locked)
        .expect("old key still decrypts");
    assert_eq!(readable.epoch(), 0);
    assert_eq!(
        keys.all(&thread)
            .expect("readable history")
            .iter()
            .map(|row| row.epoch)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}
