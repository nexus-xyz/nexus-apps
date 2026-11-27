//! A cloud machine through Gateway's relay, end to end, against a fake Gateway in this
//! process: the host's line out ([`nexus_host::relay`]) dials its control connection, the app's [`RelayLink`] dials
//! `/connect`, the fake Gateway sends `open`, the host dials the stream back, and the fake joins the two. Then the app
//! makes real calls on the real host server, and `host_provision` writes its files.

// The upgrade's callback answers tungstenite's own `ErrorResponse`, as src/server.rs's does.
#![allow(clippy::result_large_err)]

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::start;
use futures::{SinkExt, StreamExt};
use nexus_host::files::Credentials;
use nexus_host::relay::{Sources, Waits};
use nexus_remote::client::Pushed;
use nexus_remote::machine::{Backoff, Remote, Sink, Status};
use nexus_remote::relay::{RelayLink, dial};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

const MACHINE: &str = "m_1";
const MACHINE_TOKEN: &str = "nx_machine_token";
const PERSON_TOKEN: &str = "nx_person_token";

type Ws = WebSocketStream<TcpStream>;

/// The fake Gateway's state: the machine's control connection, and the app connections waiting for their stream.
#[derive(Default)]
struct State {
    control: Option<mpsc::UnboundedSender<Message>>,
    waiting: HashMap<String, oneshot::Sender<Ws>>,
    /// Every upgrade's path and `Authorization`, in order.
    seen: Vec<(String, String)>,
}

/// What one upgrade asked for.
enum Route {
    Host,
    Stream(String),
    Connect,
}

fn refused(status: StatusCode, code: &str) -> ErrorResponse {
    let body = json!({ "error": { "code": code, "message": code } }).to_string();
    let mut response = ErrorResponse::new(Some(body));
    *response.status_mut() = status;
    response
}

/// Gateway's three routes, as the plan's contract has them.
async fn fake_gateway(state: Arc<Mutex<State>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap_or_else(|e| panic!("{e}"));
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let next = Arc::new(AtomicU64::new(1));
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (state, next) = (state.clone(), next.clone());
            tokio::spawn(async move {
                let mut route = None;
                let decide = |request: &Request, response: Response| {
                    let path = request.uri().path().to_string();
                    let auth = request
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state.seen.push((path.clone(), auth.clone()));
                    let base = format!("/v1/cloud/machines/{MACHINE}");
                    let machine = auth == format!("Bearer {MACHINE_TOKEN}");
                    if path == format!("{base}/host") {
                        if !machine {
                            return Err(refused(StatusCode::FORBIDDEN, "not_machine_token"));
                        }
                        route = Some(Route::Host);
                    } else if let Some(id) = path.strip_prefix(&format!("{base}/host/streams/")) {
                        if !machine {
                            return Err(refused(StatusCode::FORBIDDEN, "not_machine_token"));
                        }
                        route = Some(Route::Stream(id.to_string()));
                    } else if path == format!("{base}/connect") {
                        if auth != format!("Bearer {PERSON_TOKEN}") {
                            return Err(refused(StatusCode::UNAUTHORIZED, "unauthorized"));
                        }
                        if state.control.as_ref().is_none_or(|c| c.is_closed()) {
                            return Err(refused(StatusCode::CONFLICT, "host_offline"));
                        }
                        route = Some(Route::Connect);
                    } else {
                        return Err(refused(StatusCode::NOT_FOUND, "not_found"));
                    }
                    Ok(response)
                };
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, decide).await else {
                    return;
                };
                match route {
                    Some(Route::Host) => control(ws, &state).await,
                    Some(Route::Stream(id)) => {
                        let waiting = state.lock().ok().and_then(|mut s| s.waiting.remove(&id));
                        if let Some(waiting) = waiting {
                            let _ = waiting.send(ws);
                        }
                    }
                    Some(Route::Connect) => {
                        let id = format!("st_{}", next.fetch_add(1, Ordering::SeqCst));
                        let (found, stream) = oneshot::channel();
                        let control = {
                            let Ok(mut state) = state.lock() else {
                                return;
                            };
                            state.waiting.insert(id.clone(), found);
                            state.control.clone()
                        };
                        let open = json!({ "type": "open", "stream": id }).to_string();
                        if control.is_none_or(|c| c.send(Message::text(open)).is_err()) {
                            return;
                        }
                        if let Ok(Ok(host)) = tokio::time::timeout(Duration::from_secs(10), stream).await {
                            splice(ws, host).await;
                        }
                    }
                    None => {}
                }
            });
        }
    });
    port
}

/// The machine's control connection: `open` frames out, its pings answered.
async fn control(ws: Ws, state: &Arc<Mutex<State>>) {
    let (out, mut outgoing) = mpsc::unbounded_channel();
    if let Ok(mut state) = state.lock() {
        state.control = Some(out);
    }
    let (mut sink, mut source) = ws.split();
    loop {
        tokio::select! {
            message = outgoing.recv() => match message {
                Some(message) => if sink.send(message).await.is_err() { break },
                None => break,
            },
            message = source.next() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    if let Ok(mut state) = state.lock() {
        state.control = None;
    }
}

/// Every text, binary and close frame passed through as it is, both ways.
async fn splice(app: Ws, host: Ws) {
    let (mut app_out, mut app_in) = app.split();
    let (mut host_out, mut host_in) = host.split();
    let pass = |message: &Message| matches!(message, Message::Text(_) | Message::Binary(_) | Message::Close(_));
    let up = async {
        while let Some(Ok(message)) = app_in.next().await {
            if pass(&message) && host_out.send(message).await.is_err() {
                break;
            }
        }
    };
    let down = async {
        while let Some(Ok(message)) = host_in.next().await {
            if pass(&message) && app_out.send(message).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

struct Statuses(Mutex<Vec<Status>>);

impl Sink for Statuses {
    fn push(&self, _: &Pushed) {}
    fn status(&self, status: &Status) {
        if let Ok(mut statuses) = self.0.lock() {
            statuses.push(status.clone());
        }
    }
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn sources(root: &std::path::Path, port: u16) -> Sources {
    sources_with(root, port, MACHINE, MACHINE_TOKEN)
}

fn sources_with(root: &std::path::Path, port: u16, machine: &str, token: &str) -> Sources {
    let dir: PathBuf = root.join("cloud");
    let _ = std::fs::create_dir_all(&dir);
    let gateway = format!("http://127.0.0.1:{port}");
    let cloud = dir.join("cloud.json");
    let _ = std::fs::write(
        &cloud,
        json!({ "machineId": machine, "gatewayUrl": gateway }).to_string(),
    );
    let credentials = dir.join("credentials");
    Credentials {
        gateway,
        token: token.into(),
        org: Some("org".into()),
        label: "box".into(),
    }
    .save(&credentials)
    .unwrap_or_else(|e| panic!("{e}"));
    Sources { cloud, credentials }
}

fn fast() -> Waits {
    Waits {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy: Duration::from_secs(60),
        refused: Duration::from_secs(60),
        idle: Duration::from_millis(100),
    }
}

#[tokio::test]
async fn the_app_reaches_the_host_through_gateway() {
    let running = start("relay");
    let state = Arc::new(Mutex::new(State::default()));
    let port = fake_gateway(state.clone()).await;
    let gateway = format!("http://127.0.0.1:{port}");

    // Before the host dials out, Gateway says it is offline: an error that heals.
    let link = RelayLink::new(&gateway, MACHINE, PERSON_TOKEN).unwrap_or_else(|e| panic!("{e}"));
    let offline = dial(&link.url, &link.headers).await.err();
    assert!(
        offline
            .as_ref()
            .is_some_and(|e| e.status == Some(409) && e.code.as_deref() == Some("host_offline") && e.heals()),
        "{offline:?}"
    );

    let (stop, stopped) = watch::channel(false);
    let line_out = tokio::spawn(nexus_host::relay::run_with(
        running.server.clone(),
        sources(&running.root, port),
        stopped,
        fast(),
    ));
    until("the control connection", || {
        state.lock().is_ok_and(|s| s.control.is_some())
    })
    .await;

    // The app: the hello's token is empty, and `host_provision` runs before it says connected.
    let mut link = link;
    link.setup = vec![(
        nexus_workbench::provision::CALL.to_string(),
        json!({
            "github": null,
            "agentEnv": "CLAUDE_CODE_OAUTH_TOKEN=x\n",
            "mcp": null,
            "instructions": "Be kind.",
        }),
    )];
    let statuses = Arc::new(Statuses(Mutex::new(Vec::new())));
    let backoff = Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy: Duration::from_secs(60),
    };
    let remote = Remote::start(Arc::new(link), statuses.clone(), backoff);
    until("connected", || matches!(remote.status(), Status::Connected { .. })).await;
    let env = running.root.join("agent.env");
    assert_eq!(
        std::fs::read_to_string(&env).unwrap_or_default(),
        "CLAUDE_CODE_OAUTH_TOKEN=x\n"
    );
    let claude_md = std::fs::read_to_string(running.root.join("home/.claude/CLAUDE.md")).unwrap_or_default();
    assert!(claude_md.contains("Be kind."));

    // Real calls on the real host.
    let status = remote.call("host_status", json!(null)).await;
    assert!(status.is_ok(), "{status:?}");
    let overview = remote.call("wb_overview", json!({})).await;
    assert!(overview.is_ok(), "{overview:?}");

    // A dropped connection comes back through Gateway by itself.
    remote.drop_connection();
    until("reconnected", || {
        statuses.0.lock().is_ok_and(|s| {
            s.iter()
                .filter(|status| matches!(status, Status::Connected { .. }))
                .count()
                >= 2
        })
    })
    .await;
    assert!(remote.call("host_status", json!(null)).await.is_ok());

    // The machine dialled with its token, the app with the person's.
    let seen = state.lock().map(|s| s.seen.clone()).unwrap_or_default();
    let host = format!("/v1/cloud/machines/{MACHINE}/host");
    assert!(
        seen.iter()
            .any(|(path, auth)| *path == host && auth == &format!("Bearer {MACHINE_TOKEN}"))
    );
    assert!(
        seen.iter()
            .any(|(path, auth)| path.starts_with(&format!("{host}/streams/st_"))
                && auth == &format!("Bearer {MACHINE_TOKEN}"))
    );
    assert!(
        seen.iter()
            .any(|(path, auth)| path.ends_with("/connect") && auth == &format!("Bearer {PERSON_TOKEN}"))
    );

    // A refused sign-in does not heal: the tries stop until "Try again".
    let wrong = RelayLink::new(&gateway, MACHINE, "nx_wrong").unwrap_or_else(|e| panic!("{e}"));
    let refused = Remote::start(Arc::new(wrong), Arc::new(Statuses(Mutex::new(Vec::new()))), backoff);
    until("refused", || {
        matches!(refused.status(), Status::Unreachable { retry_in_secs: 0, .. })
    })
    .await;
    refused.close();

    remote.close();
    let _ = stop.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), line_out).await;
    let _ = running.stop.send(true);
    let _ = std::fs::remove_dir_all(&running.root);
}

/// A refused token (403) and a Gateway without the routes yet (404) both take the slow wait: one dial in the time
/// the quick waits would have made many.
#[tokio::test]
async fn a_refused_token_or_a_missing_route_waits_the_long_time() {
    for (machine, token) in [(MACHINE, "nx_wrong_machine_token"), ("m_unknown", MACHINE_TOKEN)] {
        let running = start("relay-refused");
        let state = Arc::new(Mutex::new(State::default()));
        let port = fake_gateway(state.clone()).await;
        let (stop, stopped) = watch::channel(false);
        let line_out = tokio::spawn(nexus_host::relay::run_with(
            running.server.clone(),
            sources_with(&running.root, port, machine, token),
            stopped,
            fast(),
        ));
        until("the first dial", || state.lock().is_ok_and(|s| !s.seen.is_empty())).await;
        // `fast()` retries after 50 to 200 ms; `refused` is 60 s.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let dials = state.lock().map(|s| s.seen.len()).unwrap_or(0);
        assert_eq!(dials, 1, "{machine}: dialled {dials} times");
        assert!(state.lock().is_ok_and(|s| s.control.is_none()));
        let _ = stop.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), line_out).await;
        let _ = running.stop.send(true);
        let _ = std::fs::remove_dir_all(&running.root);
    }
}

/// A person's own server (`login --pair`) is not a cloud machine: it takes the app's machine setup and
/// applies none of it, so no cloud instructions, agent credentials or git settings land on someone's own computer.
/// The answer is a success, because the app drops a connection whose setup fails.
#[tokio::test]
async fn a_paired_server_applies_no_cloud_machine_setup() {
    let running = start("paired");
    std::fs::write(running.root.join("paired.json"), r#"{"machineId":"cm_1"}"#).unwrap_or_else(|e| panic!("{e}"));
    let reply = running
        .server
        .answer(
            nexus_workbench::api::Request {
                id: 1,
                call: nexus_workbench::provision::CALL.to_string(),
                args: json!({
                    "github": null,
                    "agentEnv": "CLAUDE_CODE_OAUTH_TOKEN=x\n",
                    "mcp": null,
                    "instructions": "Be kind.",
                }),
            },
            nexus_host::server::Peer::Relayed,
        )
        .await;
    assert!(matches!(reply.answer, nexus_workbench::api::Answer::Ok(_)), "{reply:?}");
    assert!(!running.root.join("agent.env").exists());
    assert!(!running.root.join("home/.claude/CLAUDE.md").exists());
    assert!(!running.root.join("home/.codex/AGENTS.md").exists());
}
