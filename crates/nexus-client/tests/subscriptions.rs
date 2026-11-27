//! Subscription policy failures must not look like an administrator disabled personal plans.

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use nexus_client::agent_setup::subscriptions_allowed;
use nexus_client::{Client, ClientOptions};
use serde_json::{Value, json};

async fn serve(app: Router) -> Client {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    Client::new(ClientOptions::new(&url))
}

async fn settings(status: StatusCode, body: Value) -> Client {
    serve(Router::new().route(
        "/v1/orgs/settings",
        get(move || {
            let body = body.clone();
            async move { (status, axum::Json(body)) }
        }),
    ))
    .await
}

#[tokio::test]
async fn only_an_explicit_policy_can_allow_or_deny_subscriptions() {
    for allowed in [true, false] {
        let client = settings(StatusCode::OK, json!({ "agentSubscriptions": allowed })).await;
        assert_eq!(subscriptions_allowed(&client).await, Ok(allowed));
    }
}

#[tokio::test]
async fn missing_or_invalid_policy_is_an_error() {
    for body in [
        json!({}),
        json!({ "agentSubscriptions": null }),
        json!({ "agentSubscriptions": "false" }),
        json!({ "agentSubscriptions": 0 }),
    ] {
        let client = settings(StatusCode::OK, body).await;
        let error = subscriptions_allowed(&client).await.expect_err("unknown policy");
        assert!(error.contains(client.base()), "{error}");
        assert!(error.contains("agentSubscriptions must be a boolean"), "{error}");
    }
}

#[tokio::test]
async fn gateway_failures_keep_the_status_reason_and_gateway() {
    for (status, reason) in [
        (StatusCode::UNAUTHORIZED, "Sign in again"),
        (StatusCode::FORBIDDEN, "Settings access denied"),
        (StatusCode::NOT_FOUND, "Settings route unavailable"),
        (StatusCode::TOO_MANY_REQUESTS, "Too many requests"),
        (StatusCode::SERVICE_UNAVAILABLE, "Database unavailable"),
    ] {
        let client = settings(status, json!({ "error": { "message": reason } })).await;
        let error = subscriptions_allowed(&client).await.expect_err("failed request");
        assert!(error.contains(client.base()), "{error}");
        assert!(error.contains(&format!("HTTP {}", status.as_u16())), "{error}");
        assert!(error.contains(reason), "{error}");
    }
}

#[tokio::test]
async fn a_timeout_is_an_error() {
    let client = serve(Router::new().route("/v1/orgs/settings", get(std::future::pending::<Response>))).await;
    let client = Client::with_http(
        ClientOptions {
            timeout: Duration::from_millis(50),
            ..client.options().clone()
        },
        client.http().clone(),
    );
    let error = subscriptions_allowed(&client).await.expect_err("timed out");
    assert!(error.contains(client.base()), "{error}");
    assert!(error.contains("did not answer in time"), "{error}");
}

#[tokio::test]
async fn an_unreachable_gateway_is_an_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let client = Client::new(ClientOptions::new(&format!(
        "http://{}",
        listener.local_addr().expect("addr")
    )));
    drop(listener);
    let error = subscriptions_allowed(&client).await.expect_err("unreachable");
    assert!(error.contains(client.base()), "{error}");
    assert!(error.contains("unreachable"), "{error}");
}
