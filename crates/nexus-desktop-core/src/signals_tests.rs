use super::*;

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-signals-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn a_heartbeat_is_due_once_a_minute_while_focused_and_active() {
    let start = Instant::now();
    let mut activity = Activity::default();
    activity.view("main", Surface::Code);
    activity.input("main", start);
    assert_eq!(activity.due(start), None, "no window has the focus");
    activity.focus("main", true);
    assert_eq!(activity.due(start), Some(Surface::Code));
    activity.sent(start);
    assert_eq!(activity.due(start + Duration::from_secs(30)), None, "one a minute");
    activity.input("main", start + Duration::from_secs(50));
    assert_eq!(activity.due(start + Duration::from_secs(60)), Some(Surface::Code));
    activity.sent(start + Duration::from_secs(60));
    assert_eq!(
        activity.due(start + Duration::from_secs(150)),
        None,
        "idle for more than 60 s"
    );
    activity.focus("main", false);
    activity.input("main", start + Duration::from_secs(151));
    assert_eq!(
        activity.due(start + Duration::from_secs(152)),
        None,
        "in the background"
    );
}

#[test]
fn the_surface_is_the_focused_window_the_person_acted_in() {
    let now = Instant::now();
    let mut activity = Activity::default();
    activity.view("main", Surface::Code);
    activity.view("panel", Surface::Chat);
    activity.focus("panel", true);
    activity.input("panel", now);
    assert_eq!(activity.due(now), Some(Surface::Chat));
    activity.closed("panel");
    assert_eq!(activity.due(now), None);
    activity.focus("other-window", true);
    assert_eq!(
        activity.due(now),
        Some(Surface::Other),
        "a window that never said what it shows"
    );
}

#[test]
fn values_outside_the_contracts_lists_are_mapped_before_they_leave() {
    assert_eq!(name("diff_view"), "diff_view");
    assert_eq!(name("agent_spawn.claude"), "agent_spawn.claude");
    for bad in ["", "Diff View", "/home/someone/x", "a".repeat(65).as_str(), "nope:1"] {
        assert_eq!(name(bad), "other", "{bad}");
    }
    assert_eq!(app_version("0.31.0+abc1234"), "0.31.0+abc1234");
    assert_eq!(app_version("0.31.0 (dev build)"), "unknown");
    assert_eq!(Surface::parse("settings"), Surface::Other);
    assert!(["macos", "windows", "linux", "other"].contains(&os()));
    if let Some(version) = os_version() {
        assert!(version.len() <= 32 && !version.contains(' '), "{version}");
    }
}

#[test]
fn a_heartbeat_carries_only_the_contracts_fields() {
    let body = heartbeat_body(App::DesktopGpui, Surface::Review);
    let keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    for key in &keys {
        assert!(
            ["surface", "app", "appVersion", "os", "osVersion"].contains(key),
            "{key}"
        );
    }
    assert_eq!(body["app"], "desktop-gpui");
    assert_eq!(body["surface"], "review");
}

#[test]
fn writes_utc_times() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(1_709_164_800_123), "2024-02-29T00:00:00.123Z");
    assert_eq!(rfc3339(1_792_613_399_868), "2026-10-21T20:09:59.868Z");
}

#[test]
fn a_panic_names_its_crate_never_its_path() {
    assert_eq!(
        panic_component(Some("crates/nexus-workbench/src/lib.rs:3546:13")),
        "nexus-workbench"
    );
    assert_eq!(
        panic_component(Some(
            "/home/someone/nexus/eng/factory/rs/crates/nexus-client/src/client.rs:1:1"
        )),
        "nexus-client"
    );
    assert_eq!(panic_component(Some("src/workspace/mod.rs:1:1")), "app");
    assert_eq!(
        panic_component(Some(
            "/home/someone/.cargo/registry/src/index/tokio-1.53.3/src/lib.rs:1:1"
        )),
        "dependency"
    );
    assert_eq!(panic_component(None), "unknown");
}

#[test]
fn the_queue_is_bounded_and_hands_out_batches_of_twenty() {
    let dir = temp_dir("queue");
    let queue = Queue::new(&dir);
    for n in 0..(MAX_QUEUED + 5) {
        queue.push(json!({ "n": n }));
    }
    assert_eq!(queue.len(), MAX_QUEUED, "the oldest go first");
    let batch = queue.take();
    assert_eq!(batch.len(), BATCH);
    assert_eq!(batch[0]["n"], 5);
    assert_eq!(queue.len(), MAX_QUEUED - BATCH);
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn a_crash_is_queued_once_at_the_next_start() {
    let dir = temp_dir("crash");
    let crash = crate::crash::SeenCrash {
        name: "panic in main".into(),
        message: "a message that never leaves".into(),
        location: Some("crates/nexus-workbench/src/lib.rs:1:2".into()),
        occurred_at_ms: 1_792_613_399_868,
        app_version: "0.31.0".into(),
    };
    std::fs::write(dir.join("last-crash.json"), serde_json::to_vec(&crash).expect("json")).expect("write");
    queue_last_crash(&dir, App::DesktopGpui);
    queue_last_crash(&dir, App::DesktopGpui);
    let queued = Queue::new(&dir).take();
    assert_eq!(queued.len(), 1, "a crash left for the prompt is counted once");
    let entry = &queued[0];
    assert_eq!(entry["kind"], "crash");
    assert_eq!(entry["component"], "nexus-workbench");
    assert_eq!(entry["code"], "panic");
    assert_eq!(entry["fatal"], true);
    assert_eq!(entry["appVersion"], "0.31.0", "the version that crashed");
    assert_eq!(entry["occurredAt"], "2026-10-21T20:09:59.868Z");
    assert!(!entry.to_string().contains("never leaves"));
    assert!(dir.join("last-crash.json").exists(), "the prompt still gets it");
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn a_gateway_errors_code_is_a_name_never_its_message() {
    let core = nexus_client::error::CoreError {
        status: 403,
        message: "someone@example.com is not allowed".into(),
        code: Some("role_required".into()),
        interval: None,
        details: None,
    };
    assert_eq!(error_code(&core.clone().into()), "role_required");
    let odd = nexus_client::error::CoreError {
        code: Some("Not A Name".into()),
        ..core
    };
    assert_eq!(error_code(&odd.into()), "status_403");
    let unreachable = nexus_client::Error::Transport(nexus_client::error::Transport::Unreachable("host".into()));
    assert_eq!(error_code(&unreachable), "unreachable");
}

/// A fake Gateway: answers every request with `status` and keeps each request's head and body.
async fn fake_gateway(status: u16) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(read) = socket.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            kept.lock()
                .expect("lock")
                .push(String::from_utf8_lossy(&request).to_string());
            let answer = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
            );
            let _ = socket.write_all(answer.as_bytes()).await;
        }
    });
    (url, seen)
}

fn signed_in(url: &str) -> Client {
    Client::new(ClientOptions {
        api_key: Some("nx_synthetic".into()),
        client: Some("desktop-gpui/0.31.0".into()),
        ..ClientOptions::new(url)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_batch_is_dropped_and_never_retried() {
    let (url, seen) = fake_gateway(503).await;
    let dir = temp_dir("send");
    let queue = Queue::new(&dir);
    for n in 0..25 {
        queue.push(error_entry(
            App::Desktop,
            "0.31.0",
            Kind::Update,
            "updater",
            &format!("code_{n}"),
            false,
            0,
        ));
    }
    assert_eq!(send_errors(&signed_in(&url), &queue).await, BATCH);
    let requests = seen.lock().expect("lock").clone();
    assert_eq!(requests.len(), 1, "one attempt, no retry on a 503");
    let request = &requests[0];
    assert!(
        request.starts_with(&format!("POST {ERRORS_PATH} HTTP/1.1\r\n")),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-nexus-client: desktop-gpui/0.31.0")
    );
    // A fresh key, so the Gateway does not count it as missing; one attempt, so it is never reused.
    assert!(request.to_ascii_lowercase().contains("idempotency-key: "));
    let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["errors"].as_array().expect("errors").len(), BATCH);
    assert_eq!(queue.len(), 5, "the failed batch is gone, the rest waits");
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_heartbeat_goes_once_to_its_route() {
    let (url, seen) = fake_gateway(500).await;
    send_heartbeat(&signed_in(&url), App::Desktop, Surface::Chat).await;
    let requests = seen.lock().expect("lock").clone();
    assert_eq!(requests.len(), 1, "no retry");
    assert!(requests[0].starts_with(&format!("POST {HEARTBEAT_PATH} HTTP/1.1\r\n")));
    let body: Value = serde_json::from_str(requests[0].split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["surface"], "chat");
    assert_eq!(body["app"], "desktop");
}

/// The `idempotency-key` header of a raw request.
fn key_of(request: &str) -> Option<String> {
    request.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .strip_prefix("idempotency-key: ")
            .map(String::from)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn each_heartbeat_has_its_own_key() {
    let (url, seen) = fake_gateway(200).await;
    let client = signed_in(&url);
    send_heartbeat(&client, App::Desktop, Surface::Chat).await;
    send_heartbeat(&client, App::Desktop, Surface::Chat).await;
    let keys: Vec<_> = seen
        .lock()
        .expect("lock")
        .iter()
        .map(|request| key_of(request))
        .collect();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(Option::is_some), "{keys:?}");
    assert_ne!(keys[0], keys[1], "a key is never reused across heartbeats");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gateway_that_does_not_answer_costs_five_seconds_at_most() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    // Accepts and never answers.
    let _hold = tokio::spawn(async move {
        let mut sockets = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            sockets.push(socket);
        }
    });
    let started = Instant::now();
    send_heartbeat(&signed_in(&url), App::Desktop, Surface::Code).await;
    assert!(
        started.elapsed() < TIMEOUT + Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_goes_only_with_the_sign_in_it_was_recorded_under() {
    let (url, seen) = fake_gateway(200).await;
    let dir = temp_dir("owner");
    let queue = Queue::new(&dir);
    let client = signed_in(&url);
    let mine = owner_of(&client).expect("signed in");
    let entry = |code: &str, owner: Option<&str>| {
        let mut entry = error_entry(
            App::Desktop,
            "0.31.0",
            Kind::Agent,
            "agent_spawn.claude",
            code,
            false,
            0,
        );
        if let (Some(owner), Some(fields)) = (owner, entry.as_object_mut()) {
            fields.insert(OWNER_FIELD.into(), Value::from(owner));
        }
        entry
    };
    queue.push(entry("mine", Some(&mine)));
    queue.push(entry("someone_elses", Some("desktop:0000000000000000")));
    queue.push(entry("signed_out", None));
    assert_eq!(send_errors(&client, &queue).await, 3);
    let requests = seen.lock().expect("lock").clone();
    let body: Value = serde_json::from_str(requests[0].split_once("\r\n\r\n").expect("body").1).expect("json");
    let codes: Vec<&str> = body["errors"]
        .as_array()
        .expect("errors")
        .iter()
        .filter_map(|entry| entry["code"].as_str())
        .collect();
    assert_eq!(codes, ["mine", "signed_out"], "another sign-in's error is dropped");
    assert!(
        !requests[0].contains(OWNER_FIELD),
        "the owner never leaves the computer"
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}
