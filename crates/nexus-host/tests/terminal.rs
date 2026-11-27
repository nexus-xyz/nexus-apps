//! `host_terminal` on a server: a host whose sessions have the shared terminal takes the person's actions
//! from a client that passed the hello's token, and refuses them from a client Gateway relays.

mod common;

use std::sync::Arc;

use common::{start, start_with};
use futures::{SinkExt, StreamExt};
use nexus_host::server::{PROTOCOL, Peer, RELAYED_TERMINAL, serve_socket};
use nexus_workbench::api::{Answer, Request};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

const UNAVAILABLE: &str = "takes no terminal actions from an app";

fn list(id: u64) -> Request {
    Request {
        id,
        call: nexus_workbench::terminal::SERVICE_CALL.to_string(),
        args: json!({ "threadId": "no-such-session", "op": "list", "args": {} }),
    }
}

fn error(answer: &Answer) -> Option<&str> {
    match answer {
        Answer::Error(error) => Some(error.as_str()),
        _ => None,
    }
}

#[tokio::test]
async fn a_host_without_the_shared_terminal_refuses_terminal_actions() {
    let running = start("term-off");
    let reply = running.server.answer(list(1), Peer::Local).await;
    assert!(
        error(&reply.answer).is_some_and(|e| e.contains(UNAVAILABLE)),
        "{reply:?}"
    );
}

#[tokio::test]
async fn a_server_with_the_shared_terminal_takes_terminal_actions() {
    let running = start_with("term-on", true);
    assert!(
        running
            .server
            .info
            .features
            .iter()
            .any(|feature| feature == nexus_workbench::terminal::FEATURE),
        "the hello names the shared terminal"
    );
    let reply = running.server.answer(list(1), Peer::Local).await;
    // The session does not exist, so the workbench may refuse it, but the host passed the action on.
    assert!(
        !error(&reply.answer).is_some_and(|e| e.contains(UNAVAILABLE)),
        "{reply:?}"
    );
}

#[tokio::test]
async fn a_relayed_client_takes_no_terminal_actions() {
    let running = start_with("term-relay", true);
    let (app, host) = tokio::io::duplex(64 * 1024);
    let (_stop, stopped) = watch::channel(false);
    let server = Arc::clone(&running.server);
    tokio::spawn(async move {
        let ws = tokio_tungstenite::accept_async(host)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        serve_socket(ws, server, stopped, Peer::Relayed).await;
    });
    let (mut ws, _) = tokio_tungstenite::client_async("ws://relayed/", app)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let hello = json!({ "hello": { "token": "", "protocol": PROTOCOL } });
    ws.send(Message::Text(hello.to_string().into()))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let request = json!({ "id": 7, "call": nexus_workbench::terminal::SERVICE_CALL, "args": list(7).args });
    let mut sent = false;
    while let Some(Ok(message)) = ws.next().await {
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(text.as_str()).unwrap_or_else(|e| panic!("{e}"));
        if value.get("hello").is_some() && !sent {
            ws.send(Message::Text(request.to_string().into()))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            sent = true;
            continue;
        }
        if value.get("id") == Some(&json!(7)) {
            assert_eq!(
                value.get("error").and_then(Value::as_str),
                Some(RELAYED_TERMINAL),
                "{value}"
            );
            return;
        }
    }
    panic!("no answer to the terminal action");
}
