//! The async client against a stand-in Gateway: headers, idempotent retries, errors, raw exchanges, a streamed chat
//! answer and the device login, on one connection pool.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use nexus_client::device::{self, LoginError};
use nexus_client::{Client, ClientOptions, Error, Method};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Seen {
    writes: Arc<AtomicUsize>,
    keys: Arc<std::sync::Mutex<Vec<String>>>,
    polls: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
    starts: Arc<std::sync::Mutex<Vec<Value>>>,
}

async fn me(headers: HeaderMap) -> Response {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let client = headers
        .get("x-nexus-client")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != "Bearer nxc_test" {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": { "message": "invalid API key", "code": "authentication_error" } })),
        )
            .into_response();
    }
    axum::Json(json!({ "tenant": { "slug": "acme" }, "client": client })).into_response()
}

async fn flaky_write(State(seen): State<Seen>, headers: HeaderMap) -> Response {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    seen.keys.lock().expect("keys").push(key);
    if seen.writes.fetch_add(1, Ordering::SeqCst) == 0 {
        return (StatusCode::SERVICE_UNAVAILABLE, "busy").into_response();
    }
    axum::Json(json!({ "ok": true })).into_response()
}

async fn flaky_read(State(seen): State<Seen>) -> Response {
    if seen.reads.fetch_add(1, Ordering::SeqCst) == 0 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "1")],
            "draining",
        )
            .into_response();
    }
    axum::Json(json!({ "ok": true })).into_response()
}

async fn chat() -> Response {
    let events = [
        json!({ "nexus": { "question": { "id": "q_1", "content": "hi" } } }),
        json!({ "model": "mock", "choices": [{ "delta": { "content": "Hel" } }] }),
        json!({ "choices": [{ "delta": { "content": "lo" } }] }),
        json!({ "nexus": { "session": { "id": "chs_1", "title": "hi" }, "message": { "id": "m_1", "content": "Hello" } } }),
    ];
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .expect("response")
}

async fn device_start(State(seen): State<Seen>, axum::Json(body): axum::Json<Value>) -> axum::Json<Value> {
    seen.starts.lock().expect("starts").push(body);
    axum::Json(json!({ "user_code": "ABCD-EFGH", "device_code": "dev_1", "interval": 0, "expires_in": 60 }))
}

async fn device_token(State(seen): State<Seen>, axum::Json(body): axum::Json<Value>) -> Response {
    assert_eq!(body["grant_type"], device::GRANT_TYPE);
    let poll = seen.polls.fetch_add(1, Ordering::SeqCst);
    let answer = |status: StatusCode, code: &str| {
        (
            status,
            axum::Json(json!({ "error": { "message": code, "code": code }, "interval": 0 })),
        )
            .into_response()
    };
    let refuse = |code: &str| answer(StatusCode::BAD_REQUEST, code);
    match (body["device_code"].as_str(), poll) {
        (Some("denied"), _) => refuse("access_denied"),
        (_, 0) => refuse("authorization_pending"),
        (_, 1) => refuse("slow_down"),
        // A rate limit, a draining deploy and a proxy's 502 do not end the login.
        (_, 2) => answer(StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        (_, 3) => answer(StatusCode::SERVICE_UNAVAILABLE, "draining"),
        (_, 4) => (StatusCode::BAD_GATEWAY, "bad gateway").into_response(),
        _ => axum::Json(json!({
            "access_token": "nxc_test",
            "tenant": { "slug": "acme", "id": "t_1" },
            "user": { "email": "owner@acme.test" },
            "role": "owner"
        }))
        .into_response(),
    }
}

async fn serve() -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/tenants/me", get(me))
        .route("/v1/flaky", post(flaky_write))
        .route("/v1/flaky-read", get(flaky_read))
        .route("/v1/chat/sessions/{id}/messages", post(chat))
        .route("/v1/auth/device", post(device_start))
        .route("/v1/auth/token", post(device_token))
        .route(
            "/v1/proxy/html",
            get(|| async { (StatusCode::BAD_GATEWAY, "<html><h1>502 Server Error</h1></html>") }),
        )
        .route(
            "/v1/proxy/text",
            get(|| async { (StatusCode::UNAUTHORIZED, "Unauthorized") }),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (url, seen)
}

#[tokio::test]
async fn redirects_never_forward_gateway_credentials_or_request_bodies() {
    let received = Arc::new(AtomicUsize::new(0));
    let sink_received = received.clone();
    let sink = Router::new().route(
        "/capture",
        post(move || {
            sink_received.fetch_add(1, Ordering::SeqCst);
            async { axum::Json(json!({ "captured": true })) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("sink bind");
    let destination = format!("http://{}/capture", listener.local_addr().expect("sink address"));
    let sink_task = tokio::spawn(async move { axum::serve(listener, sink).await.expect("sink serve") });

    for status in [StatusCode::TEMPORARY_REDIRECT, StatusCode::PERMANENT_REDIRECT] {
        let destination = destination.clone();
        let gateway = Router::new().route(
            "/v1/keys",
            post(move || {
                let destination = destination.clone();
                async move { (status, [(header::LOCATION, destination)]) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("gateway bind");
        let base = format!("http://{}", listener.local_addr().expect("gateway address"));
        let gateway_task = tokio::spawn(async move { axum::serve(listener, gateway).await.expect("gateway serve") });
        let client = Client::new(ClientOptions {
            api_key: Some("nx_test_credential".into()),
            headers: vec![("x-nexus-test-secret".into(), "private-header".into())],
            ..ClientOptions::new(&base)
        });
        let answer = client
            .exchange(
                Method::POST,
                "/v1/keys",
                &[],
                Some(
                    serde_json::to_vec(&json!({ "apiKey": "private-provider-key" }))
                        .expect("body")
                        .into(),
                ),
            )
            .await
            .expect("redirect response");
        assert_eq!(
            answer.status,
            status.as_u16(),
            "the original Gateway response stays visible"
        );
        assert_eq!(
            received.load(Ordering::SeqCst),
            0,
            "no redirected request reaches another origin"
        );
        gateway_task.abort();
    }
    sink_task.abort();
}

#[tokio::test]
async fn desktop_attribution_rechecks_org_membership_and_refuses_unverified_signins() {
    let reads = Arc::new(AtomicUsize::new(0));
    let endpoint_reads = reads.clone();
    let app = Router::new().route(
        "/v1/tenants/me",
        get(move || {
            let read = endpoint_reads.fetch_add(1, Ordering::SeqCst);
            async move {
                match read {
                    0 => axum::Json(json!({
                        "tenant": {"slug": "nexus-xyz"},
                        "user": {"email": "contractor@example.com"},
                        "role": "member"
                    }))
                    .into_response(),
                    1 => axum::Json(json!({
                        "tenant": {"slug": "personal-contractor"},
                        "user": {"email": "contractor@example.com"}
                    }))
                    .into_response(),
                    _ => StatusCode::UNAUTHORIZED.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let client = Client::new(ClientOptions::new(&url));
    assert!(nexus_client::commit_trailer::desktop_eligible(&client).await);
    assert!(!nexus_client::commit_trailer::desktop_eligible(&client).await);
    assert!(!nexus_client::commit_trailer::desktop_eligible(&client).await);
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn calls_retries_and_streams() {
    let (url, seen) = serve().await;
    let client = Client::new(ClientOptions {
        api_key: Some("nxc_test".into()),
        client: Some("desktop/test".into()),
        ..ClientOptions::new(&format!("{url}/"))
    });
    assert!(client.warm().await.is_ok());
    let me = client.get("/v1/tenants/me").await.expect("me");
    assert_eq!(me["client"], "desktop/test");

    // A signed-out client on the same pool gets Gateway's error.
    let Err(Error::Core(error)) = client.with_api_key(None).get("/v1/tenants/me").await else {
        panic!("401")
    };
    assert_eq!(
        (error.status, error.code.as_deref()),
        (401, Some("authentication_error"))
    );

    // An error body that is not JSON (a proxy's HTML page, plain text) keeps its status (the 502 after
    // three tries).
    for (path, status) in [("/v1/proxy/html", 502), ("/v1/proxy/text", 401)] {
        let Err(Error::Core(error)) = client.get(path).await else {
            panic!("{path}")
        };
        assert_eq!(
            (error.status, error.message.clone()),
            (status, format!("HTTP {status}"))
        );
    }

    // A write answered 503 is retried once with the same key.
    assert_eq!(
        client.post("/v1/flaky", json!({})).await.expect("write"),
        json!({ "ok": true })
    );
    let keys = seen.keys.lock().expect("keys").clone();
    assert_eq!(keys.len(), 2);
    assert!(!keys[0].is_empty() && keys[0] == keys[1]);

    // A read answered 503 is retried.
    let started = std::time::Instant::now();
    assert_eq!(client.get("/v1/flaky-read").await.expect("read"), json!({ "ok": true }));
    // It waited the answer's `Retry-After: 1`, not the 250 ms default.
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    assert_eq!(seen.reads.load(Ordering::SeqCst), 2);

    // A raw exchange passes the status and body through.
    let raw = client
        .exchange(Method::GET, "/v1/missing", &[], None)
        .await
        .expect("raw");
    assert_eq!(raw.status, 404);

    // A streamed answer: every chunk in order, then the outcome.
    let mut text = String::new();
    let outcome = client
        .stream_chat_message("chs_1", json!({ "content": "hi" }), |chunk| {
            text.push_str(nexus_client::sse::delta(chunk).unwrap_or(""));
        })
        .await
        .expect("stream");
    assert_eq!(text, "Hello");
    assert_eq!(outcome.message["id"], "m_1");
    assert_eq!(outcome.session["id"], "chs_1");
}

#[tokio::test]
async fn signs_in_with_a_device_code() {
    let (url, seen) = serve().await;
    let client = Client::new(ClientOptions {
        idempotency: false,
        ..ClientOptions::new(&url)
    });
    let code = device::start(
        &client,
        "Nexus for desktop on test",
        Some(device::DESKTOP_CLIENT_ID),
        "http://ui.test/app",
    )
    .await
    .expect("start");
    assert_eq!(
        *seen.starts.lock().expect("starts"),
        [json!({ "client": "Nexus for desktop on test", "client_id": device::DESKTOP_CLIENT_ID })]
    );
    assert_eq!(code.link, "http://ui.test/app/device?code=ABCD-EFGH");
    let grant = device::wait(&client, &code, || false).await.expect("grant");
    assert_eq!(
        (grant.token.as_str(), grant.org().as_deref()),
        ("nxc_test", Some("acme"))
    );

    let denied = device::DeviceCode {
        device_code: "denied".into(),
        ..code.clone()
    };
    assert_eq!(device::wait(&client, &denied, || false).await, Err(LoginError::Denied));
    assert_eq!(device::wait(&client, &code, || true).await, Err(LoginError::Cancelled));
}

/// An observed client sees each answer once, in full for a call and with its head for a stream, and a client made from
/// it with another key keeps the observer.
#[tokio::test]
async fn an_observer_sees_every_answer_of_a_call_and_a_stream() {
    let refused = || {
        (
            StatusCode::UPGRADE_REQUIRED,
            [(header::UPGRADE, "nexus-desktop")],
            r#"{"error":{"code":"desktop_revoked","message":"withdrawn"}}"#,
        )
    };
    let gateway = Router::new()
        .route("/v1/orgs", get(move || async move { refused() }))
        .route("/v1/anthropic/v1/messages", post(move || async move { refused() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move { axum::serve(listener, gateway).await.expect("serve") });
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observer = {
        let seen = seen.clone();
        nexus_client::Observer::new(move |answer| {
            let upgrade = answer
                .headers
                .iter()
                .find(|(name, _)| name == "upgrade")
                .map(|(_, value)| value.clone());
            let body = answer.body.map(|body| String::from_utf8_lossy(body).to_string());
            seen.lock()
                .expect("seen")
                .push((answer.path.to_string(), answer.status, upgrade, body));
        })
    };
    let client = Client::new(ClientOptions::new(&base))
        .observed(observer)
        .with_api_key(Some("key".into()));
    let error = client.get("/v1/orgs").await.expect_err("refused");
    assert!(matches!(error, Error::Core(error) if error.status == 426));
    let (status, _, _) = client
        .stream(Method::POST, "/v1/anthropic/v1/messages", &[], None)
        .await
        .expect("an answer");
    assert_eq!(status, 426);
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].0, "/v1/orgs");
    assert_eq!(seen[0].1, 426);
    assert_eq!(seen[0].2.as_deref(), Some("nexus-desktop"));
    assert!(
        seen[0]
            .3
            .as_deref()
            .is_some_and(|body| body.contains("desktop_revoked"))
    );
    assert_eq!(seen[1].0, "/v1/anthropic/v1/messages");
    assert_eq!(seen[1].2.as_deref(), Some("nexus-desktop"));
    assert_eq!(seen[1].3, None);
    server.abort();
}
