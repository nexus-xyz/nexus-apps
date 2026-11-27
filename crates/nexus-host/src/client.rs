//! The client side of the protocol ([`crate::server`]), for `nexus-host status` and the tests: connect, say hello,
//! call. The desktop app's own client speaks the same messages.

use futures::{SinkExt, StreamExt};
use nexus_workbench::api::{Answer, Request};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::server::{HostInfo, PROTOCOL};

/// One connection after its hello.
pub struct Connection<S> {
    pub ws: WebSocketStream<S>,
    pub info: HostInfo,
    next_id: u64,
}

/// What came in while waiting for an answer: pushed events, in order.
pub type Pushed = Vec<Value>;

/// Upgrades `stream` and says hello with `token`.
pub async fn connect<S>(stream: S, token: &str) -> Result<Connection<S>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .map_err(|e| format!("Could not reach the Nexus host: {e}"))?;
    let hello = json!({ "hello": { "token": token, "protocol": PROTOCOL } });
    ws.send(Message::text(hello.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    let answer = next_json(&mut ws)
        .await
        .ok_or_else(|| "The Nexus host closed the connection.".to_string())?;
    if let Some(error) = answer.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    let info: HostInfo = serde_json::from_value(answer["hello"].clone()).map_err(|e| e.to_string())?;
    Ok(Connection { ws, info, next_id: 1 })
}

/// The next text message as JSON; None once closed.
pub async fn next_json<S>(ws: &mut WebSocketStream<S>) -> Option<Value>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match ws.next().await? {
            Ok(Message::Text(text)) => return serde_json::from_str(text.as_str()).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

impl<S> Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Sends a request and waits for its answer; what was pushed meanwhile is returned with it.
    pub async fn call(&mut self, call: &str, args: Value) -> (Result<Value, String>, Pushed) {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            call: call.to_string(),
            args,
        };
        let mut pushed = Vec::new();
        let Ok(text) = serde_json::to_string(&request) else {
            return (Err("Could not write the request.".into()), pushed);
        };
        if let Err(error) = self.ws.send(Message::text(text)).await {
            return (Err(error.to_string()), pushed);
        }
        loop {
            let Some(message) = next_json(&mut self.ws).await else {
                return (Err("The Nexus host closed the connection.".into()), pushed);
            };
            if message.get("event").is_some() {
                pushed.push(message);
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            let answer = match serde_json::from_value::<nexus_workbench::api::Reply>(message) {
                Ok(reply) => match reply.answer {
                    Answer::Ok(value) => Ok(value),
                    Answer::Error(error) => Err(error),
                },
                Err(error) => Err(error.to_string()),
            };
            return (answer, pushed);
        }
    }

    /// The next pushed event (`{event, payload}`); None once closed.
    pub async fn next_push(&mut self) -> Option<Value> {
        loop {
            let message = next_json(&mut self.ws).await?;
            if message.get("event").is_some() {
                return Some(message);
            }
        }
    }
}
