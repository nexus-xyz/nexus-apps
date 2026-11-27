//! The team key through a real Gateway. Ada's runner syncs a team-visible thread: her device makes
//! the team key and locks the thread to it. Dee joins later, Ada's device hands the key over at its next refresh, and
//! Dee's synced reader opens the whole thread through it. Cy is removed: the next epoch is made without him, the
//! thread rotates, and his old team key opens nothing written after. Ada makes the thread private: its next key gets no
//! team copy.
use super::*;
use crate::computer::ClientTransport;

const UI_SECRET: &str = "ui-secret";
const ORG: &str = "org_acme";

/// Nexus UI's server headers for `person` in Acme.
fn ui(gateway: &str, person: &str) -> Client {
    Client::new(ClientOptions {
        headers: vec![
            ("x-nexus-ui-secret".into(), UI_SECRET.into()),
            ("x-nexus-clerk-user-id".into(), format!("clerk_{person}")),
            ("x-nexus-clerk-org-id".into(), ORG.into()),
        ],
        ..ClientOptions::new(gateway)
    })
}

/// `person` signs in to Acme with `role`. Returns their user id and a client with their own API key.
async fn member(gateway: &str, person: &str, role: &str) -> (String, Client) {
    let session = Client::new(ClientOptions {
        headers: vec![("x-nexus-ui-secret".into(), UI_SECRET.into())],
        ..ClientOptions::new(gateway)
    })
    .post(
        "/v1/ui/session",
        json!({
            "org": { "clerkOrgId": ORG, "name": "acme", "slug": "acme" },
            "user": { "clerkUserId": format!("clerk_{person}"), "email": format!("{person}@acme.test"), "name": person },
            "clerkRole": role,
        }),
    )
    .await
    .expect("UI session");
    let user = session["user"]["id"].as_str().expect("user").to_string();
    let token = ui(gateway, person)
        .post("/v1/auth/tokens", json!({ "label": "e2e" }))
        .await
        .expect("API key");
    (user, super::person(gateway, token["token"].as_str().expect("token")))
}

async fn backend(client: Client, scope: &str, key: &DeviceKey, root: &std::path::Path) -> Arc<GatewayBackend> {
    let keys = KeyStore::open(&root.join(format!("{}.sqlite3", uuid::Uuid::new_v4()))).expect("key store");
    let backend = GatewayBackend::connect(
        client,
        scope.to_string(),
        DeviceKey::from_secret_bytes(&key.secret_bytes()),
        Arc::new(keys),
        true,
    )
    .await
    .expect("backend");
    Arc::new(backend.with_team_policy_file(root.join("team-managed.json")))
}

async fn admit(client: &Client, gateway: &str, root: &std::path::Path, members: &[(&str, &str, &DeviceKey)]) {
    let path = root.join("team-managed.json");
    let whoami = client.get("/v1/auth/whoami").await.expect("whoami");
    let members: Vec<Value> = members.iter().map(|(person, device, key)| json!({
        "personId": person, "deviceId": device, "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
    })).collect();
    std::fs::write(
        path,
        json!({ "sessionTeamSync": [{
        "gateway": gateway, "tenantId": whoami["tenantId"], "members": members,
    }] })
        .to_string(),
    )
    .expect("admin admission");
}

/// What a reader opens on the thread: each event's text, or its state when it did not open.
async fn read(reader: &GatewayBackend, thread: &str) -> Vec<String> {
    let page = reader
        .synced_page(thread.to_string(), None, Some(-1), 100)
        .await
        .expect("synced page");
    page["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| match event["state"].as_str() {
            Some("open") => event["event"]["text"].as_str().expect("text").to_string(),
            state => state.unwrap_or_default().to_string(),
        })
        .collect()
}

/// The thread's team copies, as `(keyVersion, teamKeyId, lockedKey)`.
async fn team_copies(client: &Client, thread: &str) -> Vec<(i64, String, Vec<u8>)> {
    let list = client
        .get(&format!("/v1/threads/{thread}/team-keys"))
        .await
        .expect("team copies");
    list["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .map(|copy| {
            (
                copy["keyVersion"].as_i64().expect("version"),
                copy["teamKeyId"].as_str().expect("team key").to_string(),
                b64u(copy["lockedKey"].as_str().expect("locked")),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_member_opens_the_team_key_a_removed_one_does_not_and_a_private_thread_has_no_team_copy() {
    let Some(mut gateway) = boot().await else {
        return;
    };
    let url = gateway.url();
    let root = std::env::temp_dir().join(format!("nexus-team-key-e2e-{}", uuid::Uuid::new_v4()));
    let (ada_id, ada) = member(&url, "ada", "org:admin").await;
    let (cy_id, cy) = member(&url, "cy", "org:member").await;
    for settings in [
        json!({ "sessionContent": "user" }),
        json!({ "multiplayerSessions": true }),
    ] {
        ada.put("/v1/orgs/settings", settings).await.expect("settings");
    }
    let ada_key = DeviceKey::generate();
    let ada_device = register(&ada, &ada_key, "laptop").await;
    let cy_key = DeviceKey::generate();
    let cy_device = register(&cy, &cy_key, "laptop").await;

    let scope = format!("{url}|acme");
    let runner = backend(ada.clone(), &scope, &ada_key, &root).await;
    admit(
        &ada,
        &url,
        &root,
        &[(&ada_id, &ada_device, &ada_key), (&cy_id, &cy_device, &cy_key)],
    )
    .await;
    let wb = Workbench::new(
        Store::open(&root.join("workbench.sqlite3")).expect("store"),
        Arc::new(E2eHost(runner.clone())),
        Config {
            scope: scope.clone(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    wb.store()
        .insert_thread(&super::super::tests::record("t_team", &scope))
        .expect("thread");
    wb.register_session_sync("t_team", Some(runner.identity()));
    let thread = wb
        .store()
        .sync_target("t_team")
        .expect("target")
        .expect("registered")
        .sync_id;
    let write_and_sync = |text: &str| {
        wb.store()
            .append("t_team", 1_762_000_000_000, &user(text))
            .expect("event");
        async {
            for _ in 0..40 {
                let report = wb.sync_sessions_once().await;
                wb.inner.session_sync.lock().await.retry.clear();
                if report.confirmed_events >= 1 && report.failures.is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            panic!("the event did not sync");
        }
    };

    write_and_sync("private bootstrap").await;
    ada.put(&format!("/v1/threads/{thread}/private"), json!({ "private": false }))
        .await
        .expect("explicit visibility");
    // Ada's device makes epoch 1 for Ada and Cy, and locks the thread to it.
    write_and_sync("before Dee").await;
    let first = team_copies(&ada, &thread).await;
    assert_eq!(first.len(), 1, "one team copy after explicit sharing");
    let cy_state = cy
        .get(&format!("/v1/orgs/team-key?holder={cy_device}"))
        .await
        .expect("Cy's copies");
    let cy_copy = &cy_state["copies"][0];
    let whoami = cy.get("/v1/auth/whoami").await.expect("whoami");
    let tenant = whoami["tenantId"].as_str().expect("tenant");
    let cy_team = cy_key
        .unlock_team_key(
            tenant,
            1,
            &cy_device,
            &b64u(cy_copy["lockedKey"].as_str().expect("locked")),
        )
        .expect("Cy holds epoch 1");

    // Dee joins later. Ada's next refresh hands her every epoch, and Dee opens the thread from its first event.
    let (dee_id, dee) = member(&url, "dee", "org:member").await;
    let dee_key = DeviceKey::generate();
    let dee_device = register(&dee, &dee_key, "laptop").await;
    admit(
        &ada,
        &url,
        &root,
        &[
            (&ada_id, &ada_device, &ada_key),
            (&cy_id, &cy_device, &cy_key),
            (&dee_id, &dee_device, &dee_key),
        ],
    )
    .await;
    runner.refresh_team_now().await;
    write_and_sync("after Dee").await;
    let dee_reader = backend(dee.clone(), &scope, &dee_key, &root).await;
    assert_eq!(read(&dee_reader, &thread).await, ["no-key", "before Dee", "after Dee"]);

    // Cy is removed: Gateway retires epoch 1. Ada's device makes epoch 2 without him, and the thread rotates.
    ui(&url, "ada")
        .delete(&format!("/v1/orgs/members/{cy_id}"))
        .await
        .expect("remove Cy");
    admit(
        &ada,
        &url,
        &root,
        &[(&ada_id, &ada_device, &ada_key), (&dee_id, &dee_device, &dee_key)],
    )
    .await;
    // Another device creates epoch 2 with only its own copy, before Ada receives the handover.
    let next_team = e2e::HolderKey::generate();
    let copy_for = |device: &str, key: &DeviceKey| {
        let locked = next_team
            .lock_team_key_for(tenant, 2, device, &key.encryption_key())
            .expect("team copy");
        let message = e2e::team_key_message(tenant, 2, device, &locked).expect("message");
        json!({ "epoch": 2, "holderId": device, "lockedKey": e2e::b64u(&locked),
            "signature": e2e::b64u(&dee_key.sign(&message)) })
    };
    let next = dee
        .post(
            "/v1/orgs/team-key",
            json!({
                "epoch": 2, "encryptionKey": e2e::b64u(&next_team.encryption_key()),
                "senderDeviceId": dee_device, "copies": [copy_for(&dee_device, &dee_key)],
            }),
        )
        .await
        .expect("Dee creates epoch 2");
    runner.refresh_team_now().await;
    write_and_sync("waiting for handover").await;
    let waiting_head = ada.get(&format!("/v1/threads/{thread}")).await.expect("waiting head");
    let waiting_version = waiting_head["keyVersion"].as_i64().expect("waiting version");
    assert!(
        waiting_version > first[0].0,
        "waiting must rotate past the key that Cy's retired epoch opens"
    );
    assert!(
        team_copies(&ada, &thread)
            .await
            .iter()
            .all(|(version, _, _)| *version < waiting_version),
        "the waiting key has no team copy, so Cy's old team key cannot open it"
    );
    write_and_sync("still waiting").await;
    let still_waiting = ada.get(&format!("/v1/threads/{thread}")).await.expect("still waiting");
    assert_eq!(
        still_waiting["keyVersion"].as_i64(),
        Some(waiting_version),
        "the unshared key stays while waiting"
    );
    dee.put(
        "/v1/orgs/team-key/copies",
        json!({
            "senderDeviceId": dee_device, "copies": [copy_for(&ada_device, &ada_key)],
        }),
    )
    .await
    .expect("handover to Ada");
    runner.refresh_team_now().await;
    write_and_sync("after Cy left").await;
    let after = team_copies(&ada, &thread).await;
    let (newest_version, newest_team, newest_locked) = after.last().expect("a copy").clone();
    assert_ne!(newest_team, first[0].1, "the new key is locked to the next epoch");
    assert_eq!(newest_team, next["id"].as_str().expect("new team id"));
    let epoch = e2e::epoch_of(newest_version as i32).expect("epoch");
    for team_id in [&newest_team, &first[0].1] {
        assert!(
            cy_team
                .unlock_content_key(&thread, epoch, team_id, &newest_locked)
                .is_err(),
            "Cy's old team key opens nothing written after the rotation"
        );
    }
    assert!(cy.get(&format!("/v1/threads/{thread}/events?after=-1")).await.is_err());
    dee_reader.refresh_team_now().await;
    assert_eq!(
        read(&dee_reader, &thread).await,
        ["no-key", "before Dee", "after Dee", "no-key", "no-key", "after Cy left"]
    );

    // Ada makes the thread private: its next key rotates and gets no team copy.
    ada.put(&format!("/v1/threads/{thread}/private"), json!({ "private": true }))
        .await
        .expect("private");
    write_and_sync("private now").await;
    let head = ada.get(&format!("/v1/threads/{thread}")).await.expect("head");
    let private_version = head["keyVersion"].as_i64().expect("version");
    assert!(private_version > newest_version, "making it private rotated the key");
    assert!(
        team_copies(&ada, &thread)
            .await
            .iter()
            .all(|(version, _, _)| *version < private_version),
        "a private thread gets no new team copy"
    );

    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
