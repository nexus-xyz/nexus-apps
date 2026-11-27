//! A withdrawn build against a fake Gateway on loopback (`revoked.rs`): every client of the Gateway reports the
//! refusal, the first one starts one update check, an ordinary error starts none, and the calls Gateway exempts still
//! go out.

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{Gateway, Verb};
use crate::revoked::Latch;

/// How the fake answers a refused path.
#[derive(Clone, Copy)]
enum Refusal {
    /// Gateway's 426 `desktop_revoked`, with its `Upgrade` header, in the path's route family's shape.
    Revoked,
    /// A 426 from something else: no `Upgrade: nexus-desktop`, no `desktop_revoked`.
    Ordinary426,
    /// An ordinary failure.
    ServerError,
}

const EXEMPT: &[&str] = &[
    "/v1/auth/device",
    "/v1/auth/token",
    "/v1/auth/logout",
    "/v1/desktop/download-access",
];

/// Gateway's body for the refusal on `path`, as `ErrorShape::for_path` picks it.
fn refusal_body(path: &str) -> String {
    let message = "This version of the Nexus app (0.11.0) was withdrawn. Update Nexus to continue.";
    let body = if path.starts_with("/v1/anthropic/") {
        serde_json::json!({ "type": "error", "error": {
            "type": "invalid_request_error", "message": message, "nexus": { "version": "0.11.0", "code": "desktop_revoked" },
        }})
    } else if path.starts_with("/v1/openai/") {
        serde_json::json!({ "error": {
            "message": message, "type": "invalid_request_error", "code": "desktop_revoked", "param": null,
            "nexus": { "version": "0.11.0" },
        }})
    } else {
        serde_json::json!({ "error": {
            "message": message, "type": "invalid_request_error", "code": "desktop_revoked", "version": "0.11.0",
        }})
    };
    body.to_string()
}

/// A fake Gateway: an exempt path answers 200 `{}`, any other `/v1` path `refusal`. It keeps each request's path and
/// `x-nexus-client`, and runs until the test ends.
/// Each request's path and `x-nexus-client`, in order.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn fake_gateway(refusal: Refusal) -> (String, Seen) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            let head = loop {
                let Ok(read) = stream.read(&mut buffer) else {
                    break None;
                };
                if read == 0 {
                    break None;
                }
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    break Some(text[..end].to_string());
                }
            };
            let Some(head) = head else {
                continue;
            };
            let path = head.split(' ').nth(1).unwrap_or_default().to_string();
            let client = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("x-nexus-client:")
                        .map(str::to_string)
                })
                .unwrap_or_default()
                .trim()
                .to_string();
            let bare = path.split('?').next().unwrap_or_default().to_string();
            log.lock().expect("log").push((bare.clone(), client));
            let (status, headers, body) = if EXEMPT.contains(&bare.as_str()) {
                ("200 OK", "", "{}".to_string())
            } else {
                match refusal {
                    Refusal::Revoked => (
                        "426 Upgrade Required",
                        "upgrade: nexus-desktop\r\ncache-control: no-store\r\n",
                        refusal_body(&bare),
                    ),
                    Refusal::Ordinary426 => (
                        "426 Upgrade Required",
                        "upgrade: h2c\r\n",
                        r#"{"error":{"message":"Upgrade required","code":"upgrade_required"}}"#.to_string(),
                    ),
                    Refusal::ServerError => (
                        "500 Internal Server Error",
                        "",
                        r#"{"error":{"message":"boom","type":"api_error"}}"#.to_string(),
                    ),
                }
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (url, seen)
}

/// A signed-in Gateway at `url` whose latch counts the update checks it starts.
fn gateway(runtime: &tokio::runtime::Runtime, url: &str) -> (Gateway, Arc<AtomicUsize>) {
    let checks = Arc::new(AtomicUsize::new(0));
    let latch = {
        let checks = checks.clone();
        Latch::new(move |_| {
            checks.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (gateway, _events) = Gateway::at_with(runtime.handle().clone(), url, latch);
    gateway.set_token(Some("nxc_test".into()));
    (gateway, checks)
}

fn runtime() -> tokio::runtime::Runtime {
    nexus_client::install_tls();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime")
}

/// Read an agent stream to its end, for its head's status.
fn agent_status(runtime: &tokio::runtime::Runtime, stream: crate::agent::Stream) -> Option<u16> {
    let mut chunks = stream.chunks;
    runtime.block_on(async move {
        use futures::StreamExt as _;
        let mut status = None;
        while let Some(chunk) = chunks.next().await {
            if let crate::agent::Chunk::Head { status: head, .. } = chunk {
                status = Some(head);
            }
        }
        status
    })
}

#[test]
fn every_call_reports_the_refusal_once_and_the_exempt_calls_still_go_out() {
    let runtime = runtime();
    let (url, seen) = fake_gateway(Refusal::Revoked);
    let (gateway, checks) = gateway(&runtime, &url);

    // A JSON call in Gateway's own shape.
    let me = runtime.block_on(gateway.request(Verb::Get, "/v1/tenants/me", None));
    assert!(me.is_err());
    let revoked = gateway.revoked().current().expect("the refusal is kept");
    assert_eq!(revoked.version, "0.11.0");
    assert!(revoked.message.contains("was withdrawn"), "{}", revoked.message);
    assert_eq!(checks.load(Ordering::SeqCst), 1);

    // More refusals, in every route family's shape and on the agent API's stream: still one refusal, one check.
    for path in ["/v1/orgs", "/v1/openai/v1/responses", "/v1/chat/sessions"] {
        let answer = runtime.block_on(gateway.request(Verb::Get, path, None));
        assert!(answer.is_err(), "{path}");
    }
    let call = crate::agent::Call::Ask {
        body: nexus_desktop_core::agent::ask_body("hi", None),
    };
    assert_eq!(
        agent_status(&runtime, gateway.agent(Some(url.clone()), call)),
        Some(426)
    );
    assert_eq!(checks.load(Ordering::SeqCst), 1);

    // What Gateway exempts still goes out after the refusal, with this build's header: sign-in, sign-out and the
    // internal channel's check.
    for path in ["/v1/auth/device", "/v1/auth/logout"] {
        let answer = runtime.block_on(gateway.request(Verb::Post, path, Some(serde_json::json!({}))));
        assert!(answer.is_ok(), "{path}: {answer:?}");
    }
    let access = runtime.block_on(gateway.request(Verb::Get, "/v1/desktop/download-access", None));
    assert!(access.is_ok(), "{access:?}");
    let seen = seen.lock().expect("seen").clone();
    for path in ["/v1/auth/device", "/v1/auth/logout", "/v1/desktop/download-access"] {
        assert!(seen.iter().any(|(seen, _)| seen == path), "{path} not sent: {seen:?}");
    }
    let client = format!("desktop/{}", crate::channel::VERSION).to_ascii_lowercase();
    assert!(seen.iter().all(|(_, sent)| *sent == client), "{seen:?}");
    assert_eq!(checks.load(Ordering::SeqCst), 1);
}

#[test]
fn an_ordinary_426_or_a_server_error_is_not_a_refusal() {
    for refusal in [Refusal::Ordinary426, Refusal::ServerError] {
        let runtime = runtime();
        let (url, _) = fake_gateway(refusal);
        let (gateway, checks) = gateway(&runtime, &url);
        for path in ["/v1/tenants/me", "/v1/anthropic/v1/messages"] {
            let answer = runtime.block_on(gateway.request(Verb::Get, path, None));
            assert!(answer.is_err(), "{path}");
        }
        let call = crate::agent::Call::Ask {
            body: nexus_desktop_core::agent::ask_body("hi", None),
        };
        assert!(agent_status(&runtime, gateway.agent(Some(url.clone()), call)).is_some());
        assert_eq!(gateway.revoked().current(), None);
        assert_eq!(checks.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn another_gateway_keeps_the_refusal_this_build_got() {
    let runtime = runtime();
    let (url, _) = fake_gateway(Refusal::Revoked);
    let (gateway, checks) = gateway(&runtime, &url);
    assert!(runtime.block_on(gateway.request(Verb::Get, "/v1/orgs", None)).is_err());
    let (other, _events) = gateway.switched("http://127.0.0.1:9");
    assert!(other.revoked().current().is_some());
    assert_eq!(checks.load(Ordering::SeqCst), 1);
}
