//! Pairing after a session has gone idle must share its history without a synthetic event.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newly_paired_phone_opens_idle_history_without_another_event() {
    let Some(mut gateway) = boot().await else { return };
    let root = std::env::temp_dir().join(format!("nexus-idle-keys-{}", uuid::Uuid::new_v4()));
    let signup = Client::new(ClientOptions::new(&gateway.url()))
        .post(
            "/v1/tenants",
            json!({ "name": "Idle history", "email": "idle@acme.test" }),
        )
        .await
        .expect("sign up");
    let direct = person(&gateway.url(), signup["apiKey"].as_str().expect("API key"));
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .await
        .expect("content policy");
    let a_key = DeviceKey::generate();
    let a = register(&direct, &a_key, "laptop").await;
    let scope = format!("{}|acme", gateway.url());
    let backend = Arc::new(
        GatewayBackend::connect(
            direct.clone(),
            scope.clone(),
            DeviceKey::from_secret_bytes(&a_key.secret_bytes()),
            Arc::new(KeyStore::open(&root.join("keys.sqlite3")).expect("key store")),
            true,
        )
        .await
        .expect("backend"),
    );
    let path = root.join("workbench.sqlite3");
    let wb = Workbench::new(
        Store::open(&path).expect("store"),
        Arc::new(E2eHost(backend.clone())),
        Config {
            scope: scope.clone(),
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("workbench");
    wb.store()
        .insert_thread(&super::super::tests::record("idle", &scope))
        .expect("thread");
    wb.register_session_sync("idle", Some(backend.identity()));
    wb.store()
        .append("idle", 1_762_000_000_000, &user("existing private history"))
        .expect("event");
    assert_eq!(wb.sync_sessions_once().await.confirmed_events, 1);
    let target = wb.store().sync_target("idle").expect("target").expect("owned");
    let b_key = DeviceKey::generate();
    let b = register(&direct, &b_key, "phone paired after idle").await;
    let copy_path = format!("/v1/threads/{}/keys?device={b}&version=1", target.sync_id);
    approve(&direct, (&a, &a_key), &b, &b_key).await;
    assert!(
        direct.get(&copy_path).await.is_err(),
        "pairing alone did not run the desktop worker"
    );
    assert!(wb.sync_sessions_once().await.failures.is_empty());
    assert!(
        direct.get(&copy_path).await.is_err(),
        "a fresh idle session is not polled every upload tick"
    );

    // Persisted scheduling is already overdue on a restarted desktop; avoid a real thirty-second test sleep.
    rusqlite::Connection::open(&path)
        .expect("scheduling clock")
        .execute(
            "update session_sync_threads set last_attempt = 0 where thread_id = 'idle'",
            [],
        )
        .expect("due idle maintenance");
    drop(wb);
    let wb = Workbench::new(
        Store::open(&path).expect("reopen store"),
        Arc::new(E2eHost(backend)),
        Config {
            scope,
            shell: crate::process::Shell::sh(),
            ..Config::default()
        },
    )
    .expect("restart desktop");
    let report = wb.sync_sessions_once().await;
    assert!(
        report.failures.is_empty(),
        "idle key maintenance failed: {:?}",
        report.failures
    );
    assert!(
        direct.get(&copy_path).await.is_ok(),
        "a newly paired phone gets the idle session key"
    );
    let opened = read_as(&direct, &target.sync_id, (&b, &b_key)).await;
    assert_eq!(opened.len(), 1);
    let event: Value = serde_json::from_str(&opened[0].0.event_json).expect("event");
    assert_eq!(event["text"], "existing private history");
    assert_eq!(report.confirmed_events, 0, "maintenance appended no synthetic event");
    assert_eq!(
        wb.store()
            .sync_target("idle")
            .expect("target")
            .expect("owned")
            .confirmed,
        1
    );
    assert_eq!(
        direct
            .get(&format!("/v1/threads/{}", target.sync_id))
            .await
            .expect("head")["lastSeq"],
        0
    );
    gateway.stop().await;
    let _ = std::fs::remove_dir_all(root);
}
