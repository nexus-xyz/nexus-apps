//! A runner's hold on a native session ([`nexus_client::fence`]) against a stand-in Gateway that keeps one
//! session's runner as Gateway does: a take, a renewal every turn, a takeover after the lease, a handoff
//! with the token seen, and the connector changes that carry the token.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use nexus_client::fence::{Runner, SESSION_HEADER, Stopped, TOKEN_HEADER, conflict_token};
use nexus_client::{Client, ClientOptions, Error, Method};
use serde_json::{Value, json};

#[derive(Default)]
struct Session {
    token: i64,
    holder: Option<String>,
    /// Whether the holder's lease is live: the tests let it pass by hand.
    live: bool,
    ended: bool,
    renewals: usize,
    keys: Vec<String>,
    /// The changes Gateway let through: (session, token).
    written: Vec<(String, String)>,
}

type Shared = Arc<Mutex<Session>>;

fn refuse(status: StatusCode, code: &str, nexus: Value) -> Response {
    (
        status,
        axum::Json(json!({ "error": { "message": code, "code": code, "nexus": nexus } })),
    )
        .into_response()
}

fn runner(s: &Session) -> Value {
    json!({ "runnerId": s.holder, "fencingToken": s.token, "leaseExpiresAt": "2026-11-20T00:01:00Z" })
}

async fn take(State(shared): State<Shared>, headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
    let mut s = shared.lock().expect("session");
    s.keys.push(
        headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .into(),
    );
    if s.ended {
        return refuse(StatusCode::CONFLICT, "session_ended", Value::Null);
    }
    let allowed = match body["seenToken"].as_i64() {
        Some(seen) => seen == s.token,
        None => !s.live,
    };
    if !allowed {
        return refuse(
            StatusCode::CONFLICT,
            "takeover_conflict",
            json!({ "currentFencingToken": s.token }),
        );
    }
    s.token += 1;
    s.holder = body["runnerId"].as_str().map(String::from);
    s.live = true;
    axum::Json(runner(&s)).into_response()
}

async fn renew(State(shared): State<Shared>, headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
    let mut s = shared.lock().expect("session");
    s.keys.push(
        headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .into(),
    );
    if s.ended {
        return refuse(StatusCode::CONFLICT, "session_ended", Value::Null);
    }
    if body["fencingToken"].as_i64() != Some(s.token) {
        return refuse(
            StatusCode::CONFLICT,
            "lost_runner",
            json!({ "currentFencingToken": s.token }),
        );
    }
    s.renewals += 1;
    s.live = true;
    axum::Json(runner(&s)).into_response()
}

async fn read(State(shared): State<Shared>, Path(id): Path<String>) -> Response {
    let s = shared.lock().expect("session");
    axum::Json(json!({ "id": id, "fencingToken": s.token, "runner": runner(&s) })).into_response()
}

async fn write(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    let mut s = shared.lock().expect("session");
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let (session, token) = (header(SESSION_HEADER), header(TOKEN_HEADER));
    if token != s.token.to_string() {
        return refuse(StatusCode::CONFLICT, "lost_runner", Value::Null);
    }
    s.written.push((session, token));
    axum::Json(json!({ "ok": true })).into_response()
}

async fn gateway() -> (Client, Shared) {
    let shared = Shared::default();
    let app = Router::new()
        .route("/v1/sessions/{id}", get(read))
        .route("/v1/sessions/{id}/runner", post(take))
        .route("/v1/sessions/{id}/runner/renew", post(renew))
        .route("/v1/connectors/{name}/{operation}", post(write))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let options = ClientOptions {
        api_key: Some("nxc_runner".into()),
        ..ClientOptions::new(&url)
    };
    (Client::new(options), shared)
}

/// A connector change as the runner's MCP server sends it: through the fence.
async fn change(client: &Client, runner: &Runner) -> nexus_client::Result<Value> {
    let fence = runner.fence();
    let path = "/v1/connectors/github/pull_review";
    let mut headers = Vec::new();
    fence.apply(&Method::POST, path, &mut headers)?;
    let answer = client.call_with(Method::POST, path, &headers, Some(json!({}))).await;
    fence.observe(&answer);
    answer
}

fn code(error: &Error) -> Option<&str> {
    match error {
        Error::Core(error) => error.code.as_deref(),
        Error::Transport(_) => None,
    }
}

#[tokio::test]
async fn a_runner_takes_renews_and_fences_its_changes() {
    let (client, shared) = gateway().await;
    let a = Runner::take(&client, "cs_one", "host-a", None)
        .await
        .expect("A takes it");
    assert_eq!(a.fence().token(), 1);
    assert_eq!(a.fence().session(), "cs_one");
    change(&client, &a).await.expect("A writes");
    assert_eq!(
        shared.lock().expect("s").written,
        [("cs_one".to_string(), "1".to_string())]
    );

    a.renew().await.expect("renewed");
    let holding = tokio::spawn({
        let a = a.clone();
        async move { a.hold_every(Duration::from_millis(20)).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(shared.lock().expect("s").renewals >= 3, "the loop renews");
    assert!(!holding.is_finished());
    holding.abort();
    let keys = shared.lock().expect("s").keys.clone();
    assert!(
        keys.iter().all(|key| !key.is_empty()),
        "every take and renewal has an Idempotency-Key"
    );
}

#[tokio::test]
async fn a_second_runner_waits_for_the_lease_then_the_first_loses_its_changes_and_its_renewals() {
    let (client, shared) = gateway().await;
    let a = Runner::take(&client, "cs_one", "host-a", None)
        .await
        .expect("A takes it");
    let holding = tokio::spawn({
        let a = a.clone();
        async move { a.hold_every(Duration::from_millis(20)).await }
    });

    // B cannot take a session whose lease is live, and is told the token, nor resume another runner's session.
    let refused = Runner::take(&client, "cs_one", "host-b", None).await.expect_err("held");
    assert_eq!(conflict_token(&refused), Some(1));
    let refused = Runner::take_or_resume(&client, "cs_one", "host-b")
        .await
        .expect_err("not B's");
    assert_eq!(code(&refused), Some("takeover_conflict"));

    // The lease passes; B takes it over. A's next renewal stops A, and A's change is refused before it leaves.
    {
        let mut s = shared.lock().expect("s");
        s.live = false;
    }
    let b = Runner::take(&client, "cs_one", "host-b", None)
        .await
        .expect("B takes it over");
    assert_eq!(b.fence().token(), 2);
    let stopped = tokio::time::timeout(Duration::from_secs(5), holding)
        .await
        .expect("A stops")
        .expect("joined");
    assert_eq!(stopped, Stopped::LostRunner);
    let renewals = shared.lock().expect("s").renewals;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(shared.lock().expect("s").renewals, renewals, "A renews no more");
    let lost = change(&client, &a).await.expect_err("A lost it");
    assert_eq!(code(&lost), Some("lost_runner"));
    assert!(
        lost.to_string().contains("Another runner took this session over"),
        "{lost}"
    );
    change(&client, &b).await.expect("B writes");
    assert_eq!(
        shared.lock().expect("s").written,
        [("cs_one".to_string(), "2".to_string())]
    );
}

#[tokio::test]
async fn a_change_refused_as_lost_stops_the_fence_even_before_a_renewal_does() {
    let (client, _shared) = gateway().await;
    let a = Runner::take(&client, "cs_one", "host-a", None)
        .await
        .expect("A takes it");
    let b = Runner::take(&client, "cs_one", "host-b", Some(1))
        .await
        .expect("B's handoff");
    assert_eq!(b.fence().token(), 2);
    assert_eq!(
        code(&change(&client, &a).await.expect_err("stale")),
        Some("lost_runner")
    );
    assert_eq!(a.fence().stopped_by(), Some(Stopped::LostRunner));
    assert_eq!(code(&a.renew().await.expect_err("no renewal")), Some("lost_runner"));
    // A handoff with a token that is no longer the session's is refused.
    let stale = Runner::take(&client, "cs_one", "host-c", Some(1))
        .await
        .expect_err("stale handoff");
    assert_eq!(conflict_token(&stale), Some(2));
}

#[tokio::test]
async fn a_restarted_runner_resumes_its_own_session_and_an_ended_session_stops_it() {
    let (client, shared) = gateway().await;
    let first = Runner::take(&client, "cs_one", "host-a", None).await.expect("taken");
    // The same runner's next process, inside the lease: a handoff from itself.
    let again = Runner::take_or_resume(&client, "cs_one", "host-a")
        .await
        .expect("resumed");
    assert_eq!(again.fence().token(), 2);
    assert_eq!(
        code(&first.renew().await.expect_err("the old process lost it")),
        Some("lost_runner")
    );

    shared.lock().expect("s").ended = true;
    let ended = again.renew().await.expect_err("ended");
    assert_eq!(code(&ended), Some("session_ended"));
    assert_eq!(again.hold_every(Duration::from_millis(5)).await, Stopped::SessionEnded);
    assert_eq!(
        code(&change(&client, &again).await.expect_err("no change")),
        Some("session_ended")
    );
}
