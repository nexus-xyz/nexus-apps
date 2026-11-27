//! The boat client against the fake boat server: what it sends (field names from boat's OpenAPI), how it reads the
//! answers, the states it waits through, and the errors a person sees.

use std::time::Duration;

use nexus_cloud::boat::Boat;
use nexus_cloud::fake::{Config, FakeBoat};
use nexus_cloud::{Error, Provider, Size, Spec, State, wait_ready};
use serde_json::{Value, json};

async fn fake(config: Config) -> (FakeBoat, Boat) {
    let fake = FakeBoat::start(config).await.unwrap_or_else(|e| panic!("{e}"));
    let boat = Boat::new(Some(&fake.url()), "boat_fake", None).unwrap_or_else(|e| panic!("{e}"));
    (fake, boat)
}

fn spec() -> Spec {
    Spec {
        name: "ada-code".into(),
        labels: [("nexus.user".to_string(), "ada".to_string())].into(),
        ..Spec::default()
    }
}

#[tokio::test]
async fn creates_without_the_accounts_secrets_or_a_timer_and_waits_until_ready() {
    let fake = FakeBoat::start(Config::default())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let boat = Boat::new(Some(&fake.url()), "boat_fake", Some("acme".into())).unwrap_or_else(|e| panic!("{e}"));
    let id = boat.create(spec()).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(id.starts_with("bx_"), "{id}");
    let created = fake
        .calls()
        .into_iter()
        .find(|c| c.path == "/sandboxes")
        .unwrap_or_else(|| panic!("no create"));
    assert_eq!(
        created.body,
        json!({ "name": "ada-code", "type": "large", "labels": { "nexus.user": "ada" },
            "noEnv": true, "ttlSeconds": null })
    );
    assert!(created.idempotency_key.is_some_and(|k| k.len() >= 32));
    assert_eq!(created.org.as_deref(), Some("acme"));

    let first = boat.get(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(first.state, State::Creating);
    assert_eq!(first.size, Some(Size::Large));
    assert_eq!(first.labels.get("nexus.user").map(String::as_str), Some("ada"));
    let mut seen = Vec::new();
    let ready = wait_ready(&boat, &id, Duration::from_secs(10), |s| seen.push(s.clone()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ready.state, State::Ready);
    assert_eq!(ready.provider_state, "ready");
    assert_eq!(seen, vec![State::Creating, State::Ready]);
}

#[tokio::test]
async fn a_create_whose_answer_was_lost_is_retried_with_the_same_key() {
    let (fake, boat) = fake(Config::default()).await;
    fake.fail_next("POST", "/sandboxes", 502, "bad_gateway", "Upstream hiccup.", json!({}));
    let id = boat.create(spec()).await.unwrap_or_else(|e| panic!("{e}"));
    let creates: Vec<_> = fake.calls().into_iter().filter(|c| c.path == "/sandboxes").collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0].idempotency_key, creates[1].idempotency_key);
    assert!(boat.get(&id).await.is_ok());
    // Out of machines is said, not retried.
    fake.fail_next(
        "POST",
        "/sandboxes",
        503,
        "out_of_capacity",
        "Paused.",
        json!({ "retryAfterSeconds": 120 }),
    );
    let refused = boat.create(spec()).await;
    assert!(
        matches!(&refused, Err(Error::Api { code, .. }) if code == "out_of_capacity"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn sleeps_wakes_and_says_why_it_cannot() {
    let (fake, boat) = fake(Config::default()).await;
    let id = fake.insert("ready");
    boat.sleep(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(boat.get(&id).await.map(|m| m.state), Ok(State::Stopping));
    // Asleep already, or on its way: not an error.
    boat.sleep(&id).await.unwrap_or_else(|e| panic!("{e}"));
    tokio::time::sleep(Duration::from_millis(400)).await;
    let asleep = boat.get(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(asleep.state, State::Sleeping);
    assert!(asleep.stopped_at.is_some());

    fake.fail_next(
        "POST",
        "/resume",
        429,
        "rate_limited",
        "5 starts per minute",
        json!({ "window": "minute", "limit": 5, "nextStartAt": "2026-11-01T12:00:30Z", "retryAfterSeconds": 30 }),
    );
    let limited = boat.wake(&id, None).await;
    assert_eq!(
        limited,
        Err(Error::RateLimited {
            message: "boat's limit on machine starts was reached: 5 starts per minute".into(),
            next_start_at: Some("2026-11-01T12:00:30Z".into()),
            retry_after_secs: Some(30),
        })
    );
    assert!(
        limited
            .err()
            .is_some_and(|e| e.to_string().contains("2026-11-01T12:00:30Z"))
    );

    boat.wake(&id, Some(Size::XLarge))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let resume = fake.calls().into_iter().rfind(|c| c.path.ends_with("/resume"));
    assert_eq!(resume.map(|c| c.body), Some(json!({ "type": "xlarge" })));
    // A wake has a disk to restore: it reads as starting, not creating.
    assert_eq!(boat.get(&id).await.map(|m| m.state), Ok(State::Starting));
    let woken = wait_ready(&boat, &id, Duration::from_secs(10), |_| {})
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(woken.size, Some(Size::XLarge));
    // Awake already: not an error.
    boat.wake(&id, None).await.unwrap_or_else(|e| panic!("{e}"));

    boat.sleep(&id).await.unwrap_or_else(|e| panic!("{e}"));
    tokio::time::sleep(Duration::from_millis(400)).await;
    fake.expire(&id);
    let expired = boat.wake(&id, None).await;
    assert_eq!(expired, Err(Error::DataExpired));
    assert_eq!(
        Error::DataExpired.to_string(),
        "The machine's disk expired after 14 days asleep, so it cannot be woken. Delete it and create a new one."
    );
}

#[tokio::test]
async fn ssh_preview_desktop_and_commands() {
    let (fake, boat) = fake(Config::default()).await;
    let id = fake.insert("ready");
    let access = boat
        .authorize_ssh(&id, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample nexus-cloud")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (access.host.as_str(), access.port, access.user.as_str()),
        ("203.0.113.10", 22, "user")
    );
    assert!(access.host_key.starts_with("ssh-ed25519 "));
    let sent = fake.calls().into_iter().rfind(|c| c.path.ends_with("/sshkey"));
    assert_eq!(
        sent.map(|c| c.body),
        Some(json!({ "key": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample nexus-cloud" }))
    );
    assert!(
        boat.authorize_ssh(&id, "-----BEGIN OPENSSH PRIVATE KEY-----")
            .await
            .is_err()
    );

    let private = boat.preview(&id, 3000, false).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(private.contains("-3000.") && private.contains("_token="), "{private}");
    let public = boat.preview(&id, 5173, true).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(!public.contains("_token="), "{public}");
    let hosted = fake.calls().into_iter().rfind(|c| c.path.ends_with("/host"));
    assert_eq!(
        hosted.map(|c| c.body),
        Some(json!({ "port": 5173, "public": true, "localhost": true }))
    );

    let desktop = boat.desktop(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert!(desktop.starts_with("https://"), "{desktop}");

    let out = boat
        .exec(&id, "echo out; echo err >&2; exit 3", Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out.exit_code, Some(3));
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
    assert!(!out.timed_out);
    let sent = fake.calls().into_iter().rfind(|c| c.path.ends_with("/commands"));
    assert_eq!(sent.map(|c| c.body["timeoutSeconds"].clone()), Some(Value::from(5)));
    let slow = boat
        .exec(&id, "sleep 3", Duration::from_secs(1))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(slow.timed_out);
    assert_eq!(slow.exit_code, None);

    // A sleeping machine cannot take a key yet: boat says so, and the link retries that.
    boat.sleep(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let early = boat.authorize_ssh(&id, "ssh-ed25519 AAAA x").await;
    assert!(matches!(&early, Err(e) if e.is_retryable()), "{early:?}");
}

#[tokio::test]
async fn a_stop_key_reads_and_stops_its_own_machine_only() {
    let (fake, boat) = fake(Config::default()).await;
    let id = fake.insert("ready");
    let other = fake.insert("ready");
    let key = boat
        .mint_stop_key(&id)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("no key"));
    let minted = fake.calls().into_iter().rfind(|c| c.path == "/api-keys/scoped");
    assert_eq!(
        minted.map(|c| c.body),
        Some(json!({ "name": format!("nexus-cloud-stop-{id}"), "ttl": "365d",
            "actions": ["sandbox.read", "sandbox.stop"], "sandboxIds": [id] }))
    );
    let scoped = Boat::new(Some(&fake.url()), key, None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(scoped.get(&id).await.map(|m| m.state), Ok(State::Ready));
    assert!(matches!(scoped.get(&other).await, Err(Error::Api { status: 403, .. })));
    assert!(matches!(scoped.delete(&id).await, Err(Error::Api { status: 403, .. })));
    scoped.sleep(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fake.state(&id).as_deref(), Some("archiving"));

    // A key that may not make keys: no stop key, and no error (the app sleeps the machine instead).
    let (_fake, refusing) = self::fake(Config {
        scoped_keys: false,
        ..Config::default()
    })
    .await;
    assert_eq!(refusing.mint_stop_key("bx_23456789").await, Ok(None));
}

#[tokio::test]
async fn deletes_with_the_confirm_header_and_reports_gone() {
    let (fake, boat) = fake(Config::default()).await;
    let id = fake.insert("archived");
    boat.delete(&id).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fake.state(&id), None);
    assert_eq!(boat.get(&id).await.map(|m| m.state), Ok(State::Gone));
    boat.delete(&id).await.unwrap_or_else(|e| panic!("{e}"));
    let gone = wait_ready(&boat, &id, Duration::from_secs(1), |_| {}).await;
    assert_eq!(gone.err(), Some(Error::NotFound));
}

#[tokio::test]
async fn a_wrong_key_is_said_plainly_and_never_shown() {
    let fake = FakeBoat::start(Config::default())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let boat = Boat::new(Some(&fake.url()), "boat_wrong_secret", None).unwrap_or_else(|e| panic!("{e}"));
    let refused = boat.get("bx_23456789").await;
    assert!(matches!(&refused, Err(Error::Unauthorized(_))), "{refused:?}");
    assert!(!format!("{refused:?}").contains("boat_wrong_secret"));
    let unreachable = Boat::new(Some("http://127.0.0.1:9/api/v1"), "boat_wrong_secret", None)
        .unwrap_or_else(|e| panic!("{e}"))
        .get("bx_23456789")
        .await;
    assert!(matches!(&unreachable, Err(Error::Network(_))), "{unreachable:?}");
    assert!(!format!("{unreachable:?}").contains("boat_wrong_secret"));
}
