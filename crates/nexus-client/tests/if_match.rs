//! Both clients send `If-Match` on an edit of an editable resource, against a stand-in Gateway that
//! versions `/v1/orgs/settings` and a chat session as the real one does: 428 without `If-Match`, 412 with a stale one,
//! and a chat message raises its session's version.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use nexus_client::{Client, ClientOptions};
use serde_json::json;

fn etag(version: u64) -> String {
    format!("\"{version}\"")
}

async fn read(State(version): State<Arc<AtomicU64>>) -> Response {
    let current = version.load(Ordering::SeqCst);
    (
        [(header::ETAG, etag(current))],
        axum::Json(json!({ "version": current })),
    )
        .into_response()
}

async fn edit(State(version): State<Arc<AtomicU64>>, headers: HeaderMap) -> Response {
    let Some(sent) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) else {
        let error = json!({ "error": { "message": "send If-Match", "code": "if_match_required" } });
        return (StatusCode::PRECONDITION_REQUIRED, axum::Json(error)).into_response();
    };
    let current = version.load(Ordering::SeqCst);
    if sent != etag(current) {
        let error = json!({ "error": { "message": "it changed", "code": "version_mismatch" } });
        return (
            StatusCode::PRECONDITION_FAILED,
            [(header::ETAG, etag(current))],
            axum::Json(error),
        )
            .into_response();
    }
    version.store(current + 1, Ordering::SeqCst);
    (
        [(header::ETAG, etag(current + 1))],
        axum::Json(json!({ "version": current + 1 })),
    )
        .into_response()
}

/// A chat message: it raises its session's version, as the Gateway's message count does.
async fn message(State(version): State<Arc<AtomicU64>>) -> Response {
    version.fetch_add(1, Ordering::SeqCst);
    axum::Json(json!({ "ok": true })).into_response()
}

async fn serve() -> String {
    let session = Router::new()
        .route(SESSION, get(read).patch(edit))
        .route("/v1/chat/sessions/s1/messages", axum::routing::post(message))
        .with_state(Arc::new(AtomicU64::new(1)));
    let app = Router::new()
        .route("/v1/orgs/settings", get(read).put(edit))
        .with_state(Arc::new(AtomicU64::new(1)))
        .merge(session);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    url
}

const PATH: &str = "/v1/orgs/settings";
const SESSION: &str = "/v1/chat/sessions/s1";

#[tokio::test]
async fn an_edit_carries_the_version_its_caller_read() {
    let url = serve().await;
    let (mine, theirs) = (
        Client::new(ClientOptions::new(&url)),
        Client::new(ClientOptions::new(&url)),
    );
    // A one-shot edit reads the version first.
    assert_eq!(mine.put(PATH, json!({})).await.expect("first edit")["version"], 2);
    // Another client edits in between.
    assert_eq!(theirs.put(PATH, json!({})).await.expect("their edit")["version"], 3);
    // Mine still holds version 2: refused, not overwritten.
    let refused = mine.put(PATH, json!({})).await.expect_err("stale");
    assert!(refused.is_version_mismatch(), "{refused:?}");
    // The refusal is forgotten, so the next edit reads again and goes through.
    assert_eq!(mine.put(PATH, json!({})).await.expect("after a reload")["version"], 4);
    // A read keeps the version, and a clone shares it.
    mine.get(PATH).await.expect("read");
    assert_eq!(mine.clone().put(PATH, json!({})).await.expect("clone")["version"], 5);
}

#[tokio::test]
async fn a_message_does_not_make_the_next_rename_stale() {
    let url = serve().await;
    let client = Client::new(ClientOptions::new(&url));
    client.get(SESSION).await.expect("read the session");
    client
        .post("/v1/chat/sessions/s1/messages", json!({ "content": "hi" }))
        .await
        .expect("message");
    // The message raised the version past the one read: the rename reads it again instead of sending a stale one.
    let renamed = client.patch(SESSION, json!({ "title": "t" })).await.expect("rename");
    assert_eq!(renamed["version"], 3);
}

#[cfg(feature = "blocking")]
#[test]
fn the_blocking_client_carries_it_too() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let url = runtime.block_on(serve());
    let call = |client: nexus_client::blocking::Client| std::thread::spawn(move || client.put(PATH, json!({})));
    let mine = nexus_client::blocking::Client::new(ClientOptions::new(&url));
    let theirs = nexus_client::blocking::Client::new(ClientOptions::new(&url));
    assert_eq!(
        call(mine.clone()).join().expect("join").expect("first edit")["version"],
        2
    );
    assert_eq!(call(theirs).join().expect("join").expect("their edit")["version"], 3);
    let refused = call(mine.clone()).join().expect("join").expect_err("stale");
    assert!(refused.is_version_mismatch(), "{refused:?}");
    assert_eq!(call(mine).join().expect("join").expect("after a reload")["version"], 4);
}
