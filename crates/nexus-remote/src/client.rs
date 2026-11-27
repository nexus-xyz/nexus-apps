//! One WebSocket to a host (nexus-host's src/server.rs has the protocol): the hello, then calls that may be in flight
//! together (each answered by its id, in the order they finish) and the events the host pushes between them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use nexus_workbench::api::{Answer, HostInfo, PROTOCOL, Reply, Request};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::{Duplex, Result, relay};

/// How long the upgrade and the hello may take.
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);

/// An event the host pushed: `{ "event": "wb:event", "payload": {…} }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pushed {
    pub event: String,
    pub payload: Value,
}

/// Why a hello failed.
#[derive(Debug, Clone, PartialEq)]
pub enum HelloError {
    /// The host speaks another protocol: it needs an update (or the app does). Its hello says which host it is
    /// (boxed: it is the largest variant by far).
    Protocol { error: String, host: Box<HostInfo> },
    /// Anything else: a wrong token, a closed stream, a timeout.
    Failed(String),
}

impl std::fmt::Display for HelloError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelloError::Protocol { error, .. } | HelloError::Failed(error) => f.write_str(error),
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// A connection after its hello. Dropping it closes the connection.
pub struct Connection {
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: AtomicU64,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// What reading a message gave.
enum Read {
    Text(String),
    Closed,
    Other,
}

fn read(message: Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>) -> Read {
    match message {
        Some(Ok(Message::Text(text))) => Read::Text(text.as_str().to_string()),
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Read::Closed,
        Some(Ok(_)) => Read::Other,
    }
}

/// What a call waiting for its answer hears when the connection goes. The host may still be doing the call.
pub const LOST: &str = "Lost the connection to the machine.";

/// Upgrades `stream`, says hello with `token` (and, to the app's own service, the app's Gateway sign-in), and starts
/// reading. The receiver gets every pushed event in order, and closes when the connection does.
pub async fn open(
    stream: Box<dyn Duplex>,
    token: &str,
    sign_in: Option<&Value>,
) -> std::result::Result<(Connection, HostInfo, mpsc::UnboundedReceiver<Pushed>), HelloError> {
    let ws = tokio::time::timeout(
        HELLO_TIMEOUT,
        tokio_tungstenite::client_async("ws://localhost/", stream),
    )
    .await
    .map_err(|_| HelloError::Failed("The Nexus host did not answer in time.".into()))?
    .map_err(|e| HelloError::Failed(format!("Could not reach the Nexus host: {e}")))?
    .0;
    open_socket(ws, token, sign_in, false).await
}

/// [`open`] on a WebSocket already upgraded (through Gateway's relay, src/relay.rs). With `keepalive`, it pings every
/// [`crate::relay::PING`] and takes [`crate::relay::SILENCE`] with no frame as a lost connection.
pub async fn open_socket<S>(
    mut ws: WebSocketStream<S>,
    token: &str,
    sign_in: Option<&Value>,
    keepalive: bool,
) -> std::result::Result<(Connection, HostInfo, mpsc::UnboundedReceiver<Pushed>), HelloError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handshake = async {
        let mut hello = json!({ "hello": { "token": token, "protocol": PROTOCOL } });
        if let Some(sign_in) = sign_in {
            hello["hello"]["signIn"] = sign_in.clone();
        }
        ws.send(Message::text(hello.to_string()))
            .await
            .map_err(|e| HelloError::Failed(format!("Could not reach the Nexus host: {e}")))?;
        loop {
            match read(ws.next().await) {
                Read::Text(text) => {
                    let answer: Value = serde_json::from_str(&text)
                        .map_err(|_| HelloError::Failed("The Nexus host answered something else.".into()))?;
                    let host = serde_json::from_value::<HostInfo>(answer["hello"].clone()).ok();
                    if let Some(error) = answer.get("error").and_then(Value::as_str) {
                        return Err(match host {
                            Some(host) if host.protocol != PROTOCOL => HelloError::Protocol {
                                error: error.to_string(),
                                host: Box::new(host),
                            },
                            _ => HelloError::Failed(error.to_string()),
                        });
                    }
                    let host =
                        host.ok_or_else(|| HelloError::Failed("The Nexus host answered something else.".into()))?;
                    return Ok(host);
                }
                Read::Closed => return Err(HelloError::Failed("The Nexus host closed the connection.".into())),
                Read::Other => {}
            }
        }
    };
    let host = tokio::time::timeout(HELLO_TIMEOUT, handshake)
        .await
        .map_err(|_| HelloError::Failed("The Nexus host did not answer in time.".into()))??;

    let (mut sink, mut source) = ws.split();
    let pending: Pending = Arc::default();
    let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
    let (pushes, pushed) = mpsc::unbounded_channel::<Pushed>();
    let writer = tokio::spawn(async move {
        let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + relay::PING, relay::PING);
        loop {
            let message = tokio::select! {
                message = outgoing.recv() => match message {
                    Some(message) => message,
                    None => break,
                },
                _ = pings.tick(), if keepalive => Message::Ping(Vec::new().into()),
            };
            if sink.send(message).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });
    let reader = tokio::spawn({
        let pending = pending.clone();
        async move {
            loop {
                let next = if keepalive {
                    match tokio::time::timeout(relay::SILENCE, source.next()).await {
                        Ok(next) => next,
                        Err(_) => break,
                    }
                } else {
                    source.next().await
                };
                let text = match read(next) {
                    Read::Text(text) => text,
                    Read::Closed => break,
                    Read::Other => continue,
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(event) = message.get("event").and_then(Value::as_str) {
                    let push = Pushed {
                        event: event.to_string(),
                        payload: message.get("payload").cloned().unwrap_or(Value::Null),
                    };
                    if pushes.send(push).is_err() {
                        break;
                    }
                    continue;
                }
                let Ok(reply) = serde_json::from_value::<Reply>(message) else {
                    continue;
                };
                let waiting = pending.lock().ok().and_then(|mut p| p.remove(&reply.id));
                if let Some(waiting) = waiting {
                    let _ = waiting.send(match reply.answer {
                        Answer::Ok(value) => Ok(value),
                        Answer::Error(error) => Err(error),
                    });
                }
            }
            // Whoever still waits hears the connection is gone; the push receiver closes with `pushes`.
            if let Ok(mut pending) = pending.lock() {
                for (_, waiting) in pending.drain() {
                    let _ = waiting.send(Err(LOST.into()));
                }
            }
        }
    });
    let connection = Connection {
        out,
        pending,
        next_id: AtomicU64::new(1),
        tasks: vec![writer, reader],
    };
    Ok((connection, host, pushed))
}

impl Connection {
    /// Sends `call` and waits for its answer (or for the connection to go).
    pub async fn call(&self, call: &str, args: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = Request {
            id,
            call: call.to_string(),
            args,
        };
        let text = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let (answer, answered) = oneshot::channel();
        self.pending.lock().map_err(|_| LOST.to_string())?.insert(id, answer);
        if self.out.send(Message::text(text)).is_err() {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(LOST.into());
        }
        answered.await.unwrap_or_else(|_| Err(LOST.into()))
    }

    /// Whether the connection is still up.
    pub fn is_open(&self) -> bool {
        !self.out.is_closed() && self.tasks.iter().all(|task| !task.is_finished())
    }

    /// Closes the connection politely (the host sees the client leave).
    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    /// Drops the connection at once, as a lost network would: the push receiver closes and every call waiting fails.
    pub fn abort(&self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Ok(mut pending) = self.pending.lock() {
            for (_, waiting) in pending.drain() {
                let _ = waiting.send(Err(LOST.into()));
            }
        }
    }
}
