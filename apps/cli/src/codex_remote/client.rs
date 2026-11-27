//! The CLI's own client of the run's Codex app-server. The person's TUI is the other client of the same server. This
//! one attaches to the TUI's thread (`thread/resume`) and applies a phone's commands to it: a message starts a turn
//! (`turn/start`, or `turn/steer` while one runs), Stop interrupts it (`turn/interrupt`), and an approval answers
//! Codex's request. Codex sends each approval request to both clients. The first answer wins, and Codex tells the other
//! client with `serverRequest/resolved`. The client never answers a request the phone did not answer: the TUI does.
//!
//! What it reads becomes the session log's events ([`WbEvent::Approval`], [`WbEvent::ApprovalResolved`] and the
//! phone's notices), on `events`. The transcript reader logs everything else.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use nexus_workbench::event::{Decision, PermissionSuggestion, WbEvent};
use nexus_workbench::session_sync::control::{Action, notices};
use nexus_workbench::session_sync::control_loop::Performer;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::session_log::Performed;

/// How long a request to the app-server may take.
const REQUEST: Duration = Duration::from_secs(10);
const COMMAND: &str = "item/commandExecution/requestApproval";
const EDIT: &str = "item/fileChange/requestApproval";

/// One of Codex's approval requests, open until it is resolved.
struct Asked {
    /// Codex's own request id, for the answer.
    raw: Value,
    item: String,
    tool: String,
    /// Whether Codex offers "for this session".
    session: bool,
    /// The phone's answer, once sent.
    answered: Option<Decision>,
}

#[derive(Default)]
struct State {
    /// The TUI's thread, once attached.
    thread: Option<String>,
    turn: Option<String>,
    asked: HashMap<String, Asked>,
    /// Requests the TUI answered while a turn ran: the item's end says how (item id → request id).
    elsewhere: HashMap<String, String>,
    waiting: HashMap<u64, oneshot::Sender<Result<Value, String>>>,
    closed: bool,
}

pub struct Client {
    state: Mutex<State>,
    out: mpsc::UnboundedSender<Message>,
    events: mpsc::UnboundedSender<WbEvent>,
    next: AtomicU64,
}

impl Client {
    /// Connects to the app-server at `socket` (a WebSocket on a Unix socket, as `codex --remote unix://` connects) and
    /// initializes. The client's events go to `events`.
    pub async fn connect(socket: &Path, events: mpsc::UnboundedSender<WbEvent>) -> Result<Arc<Client>, String> {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|_| "Codex's app-server is not reachable".to_string())?;
        let (ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .map_err(|_| "Codex's app-server refused the connection".to_string())?;
        Self::start(ws, events).await
    }

    async fn start<S>(
        ws: tokio_tungstenite::WebSocketStream<S>,
        events: mpsc::UnboundedSender<WbEvent>,
    ) -> Result<Arc<Client>, String>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        let client = Arc::new(Client {
            state: Mutex::default(),
            out,
            events,
            next: AtomicU64::new(1),
        });
        let reader = Arc::downgrade(&client);
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let Some(client) = reader.upgrade() else {
                    return;
                };
                if let Message::Text(text) = message
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    client.read(&value);
                }
            }
            if let Some(client) = reader.upgrade() {
                client.close();
            }
        });
        let info = json!({"clientInfo": {"name": "nexus-cli", "title": "Nexus CLI", "version": crate::VERSION}});
        client.request("initialize", info).await?;
        client.notify("initialized");
        Ok(client)
    }

    /// Subscribes to the TUI's thread, `thread`: Codex's conversation id. Codex refuses it until the conversation's
    /// first turn wrote its rollout, so the caller tries again.
    pub async fn attach(&self, thread: &str) -> Result<(), String> {
        self.request("thread/resume", json!({"threadId": thread})).await?;
        self.lock().thread = Some(thread.to_string());
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn send(&self, value: Value) {
        let _ = self.out.send(Message::Text(value.to_string().into()));
    }

    fn notify(&self, method: &str) {
        self.send(json!({"method": method}));
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.lock();
            if state.closed {
                return Err("Codex's app-server closed".into());
            }
            state.waiting.insert(id, tx);
        }
        self.send(json!({"id": id, "method": method, "params": params}));
        match tokio::time::timeout(REQUEST, rx).await {
            Ok(Ok(answer)) => answer,
            _ => {
                self.lock().waiting.remove(&id);
                Err(format!("Codex did not answer {method}"))
            }
        }
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        for (_, waiting) in state.waiting.drain() {
            let _ = waiting.send(Err("Codex's app-server closed".into()));
        }
    }

    fn emit(&self, event: WbEvent) {
        let _ = self.events.send(event);
    }

    fn read(&self, message: &Value) {
        let params = message.get("params").unwrap_or(&Value::Null);
        match (message.get("method").and_then(Value::as_str), message.get("id")) {
            (Some(method), Some(id)) => self.asks(method, id.clone(), params),
            (Some(method), None) => self.notification(method, params),
            (None, Some(id)) => {
                let Some(waiting) = id.as_u64().and_then(|id| self.lock().waiting.remove(&id)) else {
                    return;
                };
                let answer = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex refused the request")
                        .to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            (None, None) => {}
        }
    }

    fn ours(state: &State, params: &Value) -> bool {
        state.thread.is_some() && params.get("threadId").and_then(Value::as_str) == state.thread.as_deref()
    }

    /// An approval of the TUI's thread becomes the log's approval, with an id the phone can answer. Every other request
    /// is the TUI's to answer.
    fn asks(&self, method: &str, raw: Value, params: &Value) {
        let command = match method {
            COMMAND => true,
            EDIT => false,
            _ => return,
        };
        let mut state = self.lock();
        if !Self::ours(&state, params) {
            return;
        }
        let item = text(params.get("itemId"));
        // Codex numbers its requests from 0 in each run; the item id keeps them apart, as the desktop's ids do.
        let request_id = format!(
            "{item}#{}",
            raw.as_u64().map_or_else(|| text(Some(&raw)), |n| n.to_string())
        );
        let (tool, input) = if command {
            ("Bash", json!({"command": text(params.get("command"))}))
        } else {
            ("Edit", json!({}))
        };
        let session = !command
            || params
                .get("availableDecisions")
                .and_then(Value::as_array)
                .is_none_or(|offered| offered.iter().any(|decision| decision == "acceptForSession"));
        let suggestions = if session {
            vec![PermissionSuggestion {
                kind: "acceptForSession".into(),
                label: if command {
                    "Always allow this command for this session"
                } else {
                    "Allow all edits for this session"
                }
                .into(),
                raw: json!("acceptForSession"),
            }]
        } else {
            vec![]
        };
        state.asked.insert(
            request_id.clone(),
            Asked {
                raw,
                item: item.clone(),
                tool: tool.into(),
                session,
                answered: None,
            },
        );
        drop(state);
        self.emit(WbEvent::Approval {
            request_id,
            tool_use_id: Some(item),
            tool: tool.into(),
            input,
            description: params
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .map(String::from),
            suggestions,
            parent: None,
        });
    }

    fn notification(&self, method: &str, params: &Value) {
        let mut state = self.lock();
        if method == "serverRequest/resolved" {
            let raw = params.get("requestId").cloned().unwrap_or(Value::Null);
            let Some(request_id) = state
                .asked
                .iter()
                .find(|(_, asked)| asked.raw == raw)
                .map(|(id, _)| id.clone())
            else {
                return;
            };
            let Some(asked) = state.asked.remove(&request_id) else {
                return;
            };
            let decision = match (asked.answered, &state.turn) {
                (Some(decision), _) => decision,
                // The TUI answered while the turn runs: the item's end says how.
                (None, Some(_)) => {
                    state.elsewhere.insert(asked.item, request_id);
                    return;
                }
                // The turn ended first (Stop, or Codex gave up): nobody answered.
                (None, None) => Decision::Expired,
            };
            drop(state);
            return self.resolved(request_id, decision);
        }
        if !Self::ours(&state, params) {
            return;
        }
        match method {
            "turn/started" => state.turn = params.pointer("/turn/id").and_then(Value::as_str).map(String::from),
            "turn/completed" => {
                state.turn = None;
                let open: Vec<String> = state.elsewhere.drain().map(|(_, request_id)| request_id).collect();
                drop(state);
                for request_id in open {
                    self.resolved(request_id, Decision::Expired);
                }
            }
            "item/completed" => {
                let item = text(params.pointer("/item/id"));
                if let Some(request_id) = state.elsewhere.remove(&item) {
                    let declined = params.pointer("/item/status").and_then(Value::as_str) == Some("declined");
                    drop(state);
                    self.resolved(request_id, if declined { Decision::Deny } else { Decision::Allow });
                }
            }
            _ => {}
        }
    }

    fn resolved(&self, request_id: String, decision: Decision) {
        self.emit(WbEvent::ApprovalResolved {
            request_id,
            decision,
            message: None,
        });
    }

    async fn run(&self, action: Action) -> Result<(), String> {
        let (thread, turn) = {
            let state = self.lock();
            (state.thread.clone().ok_or("Codex is not attached")?, state.turn.clone())
        };
        match action {
            Action::Send { text } => {
                self.emit(WbEvent::Notice {
                    text: notices::SENT.into(),
                });
                let input = json!([{"type": "text", "text": text, "text_elements": []}]);
                match turn {
                    Some(turn) => {
                        let params = json!({"threadId": thread, "input": input, "expectedTurnId": turn});
                        self.request("turn/steer", params).await?
                    }
                    None => {
                        self.request("turn/start", json!({"threadId": thread, "input": input}))
                            .await?
                    }
                };
                Ok(())
            }
            // As the desktop's Stop: the turn ends, and Codex keeps the conversation. Nothing to stop is no error.
            Action::Stop => {
                self.emit(WbEvent::Notice {
                    text: notices::STOPPED.into(),
                });
                if let Some(turn) = turn {
                    self.request("turn/interrupt", json!({"threadId": thread, "turnId": turn}))
                        .await?;
                }
                Ok(())
            }
            Action::Approve {
                request_id, decision, ..
            } => {
                let (raw, tool, decision, answer) = {
                    let mut state = self.lock();
                    let asked = state
                        .asked
                        .get_mut(&request_id)
                        .filter(|asked| asked.answered.is_none())
                        .ok_or("That request was already answered, or Codex withdrew it.")?;
                    // With no session answer to keep, the call is allowed once, and the session says so.
                    let (decision, answer) = match decision {
                        Decision::AllowSession if asked.session => (Decision::AllowSession, "acceptForSession"),
                        Decision::Allow | Decision::AllowSession => (Decision::Allow, "accept"),
                        Decision::Deny => (Decision::Deny, "decline"),
                        Decision::Expired => return Err("An expired answer is never sent.".into()),
                    };
                    asked.answered = Some(decision);
                    (asked.raw.clone(), asked.tool.clone(), decision, answer)
                };
                self.emit(WbEvent::Notice {
                    text: notices::approval_notice(&tool, decision, decision == Decision::AllowSession),
                });
                self.send(json!({"id": raw, "result": {"decision": answer}}));
                Ok(())
            }
        }
    }
}

impl Performer for Client {
    fn ready(&self) -> bool {
        let state = self.lock();
        !state.closed && state.thread.is_some()
    }
    fn perform<'a>(&'a self, _: &'a str, _: Option<i64>, action: Action) -> Performed<'a> {
        Box::pin(self.run(action))
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
