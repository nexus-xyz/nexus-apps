//! `nexus __agent`: the session's agent with no screen, for programs (the desktop's Code workbench). One
//! JSON object a line each way: the same loop, tools and approvals as agent mode, driven by the lines that come in.
//!
//! In (stdin):
//! - `{"type":"prompt","text":"…","model":"…","images":[{data,name}]}`: start a turn (`model` and `images` optional).
//! - `{"type":"decide","decision":"allow"|"always"|"deny"}`: answer the approval the agent waits on.
//! - `{"type":"approvals","level":"read-only"|"ask"|"auto-edit"|"auto"}`: what runs without asking. `read-only`
//!   (Plan only) refuses every edit, command and MCP call without asking; a build before it reads it as `ask`.
//! - `{"type":"stop"}`: stop the running turn. `{"type":"reset"}`: forget the conversation.
//!
//! Out (stdout): `ready` first; then for each turn `delta` (text), `tool` (a tool that ran), `approve` (a question
//! that waits for `decide`), `spent` (a step's cost) and `done` or `error`; `budget` after a turn. With `--session <id>`
//! the conversation is kept in a file under the CLI's folder, so a later run with the same id goes on with it.
//!
//! - `--fork`: go on from a copy of that conversation, under a new id; the one at `--session` stays as it is.
//! - `--rewind <n>`: keep only its first `n` messages. `done` says how many there are (`messages`), so a program can
//!   come back to the end of any turn.
//!
//! `ready` names the session (`session`), a fork's new id included, and the protocol these lines follow (`protocol`).
//!
//! - `--handshake`: print `{"type":"handshake","protocol":<n>,"version":"…"}` and exit, before any sign-in or request.
//!   The desktop runs it to check that a `nexus` can be its agent ([`PROTOCOL`]): the JavaScript CLI and older builds
//!   print no such line.
//!
//! The model may hand part of a turn to a subagent (the `agent` tool): `task` lines say it `started` (with its
//! `description` and `prompt`) and how it ended (`completed` or `failed`, with its `result`), and its tools' `tool`
//! lines name it as their `parent`.
//!
//! - `--mcp-config=<json>`: MCP servers whose tools the agent may call (`mcp.rs`). A call asks first, as a command
//!   does, unless its name is in `--allowedTools=<a,b>` or approvals are `auto`. `ready` lists a server that did not
//!   start in `notes`.
//!
//! The program says what it is in `NEXUS_AGENT_HOST` (`desktop` for Nexus for desktop), so the agent is told where it
//! runs and what that place shows (`prompt.rs`).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::mcp::Mcp;
use super::prompt::Host;
use super::tools::{Action, Ask, Decision};
use super::{Control, Task, Work};
use crate::tui::turn::{Gateway, TurnEvent};

/// The version of the lines above. 1: everything this file documents. A change a program would misread takes the next
/// number; a new line or field does not. The desktop's `nexus-workbench` (`nexus_runtime::PROTOCOL`) names the one it
/// drives, and refuses any other.
pub const PROTOCOL: u32 = 1;

/// The answer to `--handshake`: this build's protocol and version.
pub fn handshake() -> Value {
    json!({ "type": "handshake", "protocol": PROTOCOL, "version": crate::VERSION })
}

/// What comes in: a line from the program, or an event of a turn.
enum Message {
    Line(String),
    Closed,
    Event(u64, TurnEvent),
}

/// A conversation of `--session`: its id, its file, and what was said.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub file: PathBuf,
    pub history: Vec<Value>,
}

/// The MCP servers of a run, and the tools of theirs that run without asking.
pub struct Servers {
    pub mcp: Arc<Mcp>,
    pub allowed: Vec<String>,
    /// Why a server was left out.
    pub notes: Vec<String>,
}

/// Opens session `id`: as it is, as a copy under a new id (`fork`), and cut back to its first `rewind` messages. A
/// fork or a cut is saved at once, so the next run finds it.
pub fn open(config_path: &Path, id: &str, fork: bool, rewind: Option<usize>) -> Result<Session, String> {
    let file = session_file(config_path, id)?;
    let mut history: Vec<Value> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut session = if fork {
        let id = format!("fork-{}", uuid::Uuid::new_v4());
        let file = session_file(config_path, &id)?;
        Session {
            id,
            file,
            history: Vec::new(),
        }
    } else {
        Session {
            id: id.to_string(),
            file,
            history: Vec::new(),
        }
    };
    let changed = fork || rewind.is_some_and(|keep| keep < history.len());
    if let Some(keep) = rewind {
        history.truncate(keep);
    }
    session.history = history;
    if changed {
        save(Some(&session.file), &Arc::new(Mutex::new(session.history.clone())));
    }
    Ok(session)
}

/// Where the conversations of `--session` are kept: one JSON file each, beside the CLI's config.
pub fn session_file(config_path: &Path, id: &str) -> Result<PathBuf, String> {
    let valid =
        !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err("a session id is letters, digits, - and _ (at most 128)".into());
    }
    let folder = config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok(folder.join("agent-sessions").join(format!("{id}.json")))
}

/// Serve the agent on `input` and `output` until the input ends. `gateway` signs in; `default_model` is the model a
/// prompt that names none uses; `host` is the program that drives it.
// Eight things that have nothing in common: a struct for them would only name the list again.
#[allow(clippy::too_many_arguments)]
pub fn serve(
    gateway: Gateway,
    default_model: String,
    workspace: PathBuf,
    host: Host,
    memory: Option<nexus_agent_memory::Store>,
    session: Option<Session>,
    servers: Option<Servers>,
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
) -> std::io::Result<()> {
    let (mcp, allowed, notes) = match servers {
        Some(Servers { mcp, allowed, notes }) => (Some(mcp), allowed, notes),
        None => (None, Vec::new(), Vec::new()),
    };
    let (id, file, saved) = match session {
        Some(Session { id, file, history }) => (Some(id), Some(file), history),
        None => (None, None, Vec::new()),
    };
    let history = Arc::new(Mutex::new(saved));
    let (messages, received) = mpsc::channel::<Message>();
    let lines = messages.clone();
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if lines.send(Message::Line(line)).is_err() {
                return;
            }
        }
        let _ = lines.send(Message::Closed);
    });
    // The turns' events come in on the same channel as the lines.
    let (events, turn_events) = mpsc::channel::<(u64, TurnEvent)>();
    std::thread::spawn(move || {
        for (turn, event) in turn_events {
            if messages.send(Message::Event(turn, event)).is_err() {
                return;
            }
        }
    });

    let mut say = |value: Value| -> std::io::Result<()> {
        writeln!(output, "{value}")?;
        output.flush()
    };
    let count = history.lock().map_or(0, |history| history.len());
    say(json!({
        "type": "ready",
        "version": crate::VERSION,
        "protocol": PROTOCOL,
        "workspace": workspace.display().to_string(),
        "session": id,
        "messages": count,
        "notes": notes,
    }))?;

    let mut running: Option<(u64, Sender<Decision>, Arc<AtomicBool>)> = None;
    let mut asking: Option<Ask> = None;
    let (mut always_edits, mut always_commands) = (false, false);
    // `auto` runs every tool; `always` on a tool's question runs that tool from then on. `read_only` runs none.
    let mut auto = false;
    let mut read_only = false;
    let mut always_tools: Vec<String> = Vec::new();
    let mut turns = 0u64;
    let mut closed = false;
    loop {
        let Ok(message) = received.recv_timeout(Duration::from_millis(200)) else {
            if closed && running.is_none() {
                return Ok(());
            }
            continue;
        };
        match message {
            Message::Closed => {
                // The program went away: stop what runs, and end once it has.
                closed = true;
                if let Some((_, decide, stop)) = &running {
                    stop.store(true, Ordering::Relaxed);
                    let _ = decide.send(Decision::Deny);
                } else {
                    return Ok(());
                }
            }
            Message::Line(line) => {
                let Ok(command) = serde_json::from_str::<Value>(&line) else {
                    say(json!({ "type": "error", "message": "a line is one JSON object" }))?;
                    continue;
                };
                match command.get("type").and_then(Value::as_str).unwrap_or("") {
                    "prompt" if running.is_some() => {
                        say(json!({ "type": "error", "message": "a turn is running: stop it or wait for done" }))?;
                    }
                    "prompt" => {
                        let text = command.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                        if text.trim().is_empty() {
                            say(json!({ "type": "error", "message": "the prompt is empty" }))?;
                            continue;
                        }
                        turns += 1;
                        let (decide, decisions) = mpsc::channel();
                        let stop = Arc::new(AtomicBool::new(false));
                        running = Some((turns, decide, stop.clone()));
                        let model = command.get("model").and_then(Value::as_str).filter(|m| !m.is_empty());
                        let images = command
                            .get("images")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let work = Work {
                            gateway: gateway.clone(),
                            model: model.map_or_else(|| default_model.clone(), String::from),
                            workspace: workspace.clone(),
                            host,
                            memory: memory.clone(),
                            history: history.clone(),
                            mcp: mcp.clone(),
                        };
                        super::spawn(work, text, images, turns, events.clone(), Control { decisions, stop });
                        say(json!({ "type": "started", "turn": turns }))?;
                    }
                    "decide" => {
                        let decision = match command.get("decision").and_then(Value::as_str) {
                            Some("allow") => Decision::Allow,
                            Some("always") => Decision::Always,
                            _ => Decision::Deny,
                        };
                        if decision == Decision::Always {
                            match asking.take() {
                                Some(Ask::Edit { .. }) => always_edits = true,
                                Some(Ask::Run { .. }) => always_commands = true,
                                Some(Ask::Tool { name, .. }) => always_tools.push(name),
                                None => {}
                            }
                        }
                        asking = None;
                        if let Some((_, decide, _)) = &running {
                            let _ = decide.send(decision);
                        }
                    }
                    "approvals" => {
                        let level = command.get("level").and_then(Value::as_str);
                        auto = level == Some("auto");
                        read_only = level == Some("read-only");
                        (always_edits, always_commands) = match level {
                            Some("auto") => (true, true),
                            Some("auto-edit") => (true, false),
                            _ => (false, false),
                        };
                    }
                    "stop" => {
                        if let Some((_, decide, stop)) = &running {
                            stop.store(true, Ordering::Relaxed);
                            let _ = decide.send(Decision::Deny);
                        }
                    }
                    "reset" if running.is_some() => {
                        say(json!({ "type": "error", "message": "a turn is running: stop it before a reset" }))?;
                    }
                    "reset" => {
                        if let Ok(mut history) = history.lock() {
                            history.clear();
                        }
                        if let Some(file) = &file {
                            let _ = std::fs::remove_file(file);
                        }
                    }
                    other => say(json!({ "type": "error", "message": format!("unknown type {other:?}") }))?,
                }
            }
            Message::Event(turn, event) => {
                let current = running.as_ref().is_some_and(|(id, _, _)| *id == turn);
                match event {
                    TurnEvent::Delta(text) if current => say(json!({ "type": "delta", "turn": turn, "text": text }))?,
                    TurnEvent::Tool(action) if current => {
                        say(json!({ "type": "tool", "turn": turn, "tool": action_json(&action) }))?;
                    }
                    TurnEvent::Task { id, task } if current => say(match task {
                        Task::Started {
                            description, prompt, ..
                        } => json!({ "type": "task", "turn": turn,
                            "task": { "id": id, "state": "started", "description": description, "prompt": prompt } }),
                        Task::Tool(action) => {
                            json!({ "type": "tool", "turn": turn, "parent": id, "tool": action_json(&action) })
                        }
                        Task::Finished { result, error, .. } => json!({ "type": "task", "turn": turn,
                            "task": { "id": id, "state": if error { "failed" } else { "completed" }, "result": result } }),
                    })?,
                    // A stopped turn's question needs no answer: the stop already says no.
                    TurnEvent::Approve(ask) if current => {
                        let allowed = match &ask {
                            Ask::Edit { .. } => always_edits,
                            Ask::Run { .. } => always_commands,
                            Ask::Tool { name, .. } => auto || allowed.contains(name) || always_tools.contains(name),
                        };
                        match &running {
                            Some((_, decide, _)) if read_only => {
                                let _ = decide.send(Decision::Deny);
                            }
                            Some((_, decide, _)) if allowed => {
                                let _ = decide.send(Decision::Allow);
                            }
                            _ => {
                                say(json!({ "type": "approve", "turn": turn, "ask": ask_json(&ask) }))?;
                                asking = Some(ask);
                            }
                        }
                    }
                    TurnEvent::Spent(cost) if current => {
                        say(json!({ "type": "spent", "turn": turn, "cost_usd": cost }))?
                    }
                    TurnEvent::Finished(result) if current => {
                        running = None;
                        asking = None;
                        save(file.as_deref(), &history);
                        let messages = history.lock().map_or(0, |history| history.len());
                        match result {
                            Ok(summary) => say(json!({
                                "type": "done",
                                "turn": turn,
                                "model": summary.model,
                                "cost_usd": summary.cost_usd,
                                "steps": summary.steps,
                                "stopped": summary.stopped,
                                "messages": messages,
                            }))?,
                            Err(error) => say(json!({
                                "type": "error",
                                "turn": turn,
                                "message": error.message,
                                "hint": error.hint,
                            }))?,
                        }
                        if closed {
                            return Ok(());
                        }
                    }
                    TurnEvent::Budget(left) => say(json!({ "type": "budget", "left": left }))?,
                    _ => {}
                }
            }
        }
    }
}

/// Keep the conversation for the next run with the same `--session`.
fn save(file: Option<&Path>, history: &Arc<Mutex<Vec<Value>>>) {
    let (Some(file), Ok(history)) = (file, history.lock()) else {
        return;
    };
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(text) = serde_json::to_string(&*history) {
        let _ = std::fs::write(file, text);
    }
}

/// What a tool did, as the program reads it.
pub fn action_json(action: &Action) -> Value {
    match action {
        Action::Read { path, lines } => json!({ "kind": "read", "path": path, "lines": lines }),
        Action::Listed { path, entries } => json!({ "kind": "list", "path": path, "entries": entries }),
        Action::Searched { pattern, matches } => json!({ "kind": "search", "pattern": pattern, "matches": matches }),
        Action::Edited {
            path,
            added,
            removed,
            diff,
            created,
        } => json!({
            "kind": if *created { "create" } else { "edit" },
            "path": path,
            "added": added,
            "removed": removed,
            "diff": diff,
        }),
        Action::Ran { command, exit, output } => {
            json!({ "kind": "run", "command": command, "exit": exit, "output": output })
        }
        Action::Planned { steps } => json!({
            "kind": "plan",
            "steps": steps.iter().map(|(step, status)| json!({ "step": step, "status": status })).collect::<Vec<_>>(),
        }),
        Action::Called {
            name,
            input,
            output,
            error,
        } => {
            json!({ "kind": "call", "name": name, "input": serde_json::from_str::<Value>(input).unwrap_or(Value::Null),
            "output": output, "error": error })
        }
        // A read of the knowledge base goes out as a `call`, a kind every program of protocol 1 shows by the tool's
        // name. A kind of its own would be misread by an app from before it, as a tool that failed.
        Action::Docs { topic, text } => json!({ "kind": "call", "name": super::knowledge::TOOL,
            "input": { "topic": topic }, "output": text, "error": false }),
        Action::Failed { what, error } => json!({ "kind": "failed", "what": what, "error": error }),
    }
}

/// What the agent asks before it runs, as the program reads it.
pub fn ask_json(ask: &Ask) -> Value {
    match ask {
        Ask::Edit { path, diff } => json!({ "kind": "edit", "path": path, "diff": diff }),
        Ask::Run { command } => json!({ "kind": "run", "command": command }),
        Ask::Tool { name, input } => json!({ "kind": "tool", "name": name,
            "input": serde_json::from_str::<Value>(input).unwrap_or(Value::Null) }),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write as _};
    use std::net::TcpListener;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};

    use super::*;

    /// Lines the test writes, as the agent's stdin.
    struct Input {
        lines: Receiver<String>,
        pending: Vec<u8>,
    }

    impl Read for Input {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.pending.is_empty() {
                match self.lines.recv() {
                    Ok(line) => self.pending = format!("{line}\n").into_bytes(),
                    Err(_) => return Ok(0),
                }
            }
            let n = buffer.len().min(self.pending.len());
            buffer[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            Ok(n)
        }
    }

    /// The agent's stdout, line by line, for the test to read.
    struct Output(Sender<String>, Vec<u8>);

    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.1.extend_from_slice(bytes);
            while let Some(at) = self.1.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.1.drain(..=at).collect();
                let _ = self.0.send(String::from_utf8_lossy(&line).trim().to_string());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A fake Gateway: `/v1/chat/completions` streams a scripted agent (an edit, a command, then an answer), each
    /// step costing $0.001; `/v1/spend` answers a budget.
    fn gateway() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || answer(stream));
            }
        });
        format!("http://{address}")
    }

    fn answer(mut stream: std::net::TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0u8; 8192];
        let (head, body) = loop {
            let Ok(n) = stream.read(&mut buffer) else { return };
            if n == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0usize);
                if request.len() >= end + 4 + length {
                    break (text[..end].to_string(), text[end + 4..].to_string());
                }
            }
        };
        if head.starts_with("GET /v1/spend") {
            let body = r#"{"remainingUsd":4,"budgetUsd":5}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            return;
        }
        let parsed: Value = serde_json::from_str(&body).expect("request JSON");
        let messages = parsed["messages"].as_array().expect("messages");
        if messages.iter().any(|message| {
            message["role"] == "assistant"
                && !message["content"].is_string()
                && message["tool_calls"].as_array().is_none_or(|calls| calls.is_empty())
        }) {
            let body = json!({ "error": {
                "code": "invalid_request",
                "message": "an assistant message needs string content, tool_calls, or both",
            } });
            let _ = write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}"
            );
            return;
        }
        let step = body.matches(r#""role":"tool""#).count();
        if parsed["model"] == "test/limited-output" && parsed["max_tokens"].as_i64().unwrap_or(0) > 4096 {
            let body = json!({ "error": {
                "code": "output_over_limit", "message": "max_tokens may be at most 4096",
                "nexus": { "field": "max_tokens", "limit": 4096 },
            } });
            let _ = write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}"
            );
            return;
        }
        let chunk = |delta: Value, finish: Option<&str>| {
            let mut chunk =
                json!({ "model": "test/model", "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
            if finish.is_some() {
                chunk["nexus"] = json!({ "cost_usd": 0.001 });
            }
            format!("data: {chunk}\n\n")
        };
        let call = |name: &str, arguments: Value| {
            json!({ "tool_calls": [{ "index": 0, "id": format!("call_{step}"), "type": "function",
                "function": { "name": name, "arguments": arguments.to_string() } }] })
        };
        let subagent = body.contains("You are a subagent");
        let mut events = match step {
            0 if matches!(
                parsed["model"].as_str(),
                Some("test/empty-arguments" | "test/whitespace-arguments")
            ) =>
            {
                let mut delta = call("list_dir", json!({}));
                delta["tool_calls"][0]["function"]["arguments"] = json!(if parsed["model"] == "test/empty-arguments" {
                    ""
                } else {
                    " \n\t"
                });
                chunk(delta, None) + &chunk(json!({}), Some("tool_calls"))
            }
            _ if matches!(
                parsed["model"].as_str(),
                Some("test/empty-arguments" | "test/whitespace-arguments")
            ) =>
            {
                chunk(json!({ "content": "Listed." }), Some("stop"))
            }
            _ if parsed["model"] == "test/missing-finish" => chunk(
                call("write_file", json!({ "path": "first.txt", "content": "Do not write." })),
                None,
            ),
            0 if parsed["model"] == "test/limited-output" => {
                let mut delta = call("write_file", json!({}));
                delta["tool_calls"][0]["function"]["arguments"] = json!("");
                let mut events = chunk(delta, None);
                let arguments = json!({ "path": "large.txt", "content": "A complete line.\n".repeat(400) }).to_string();
                let enough = parsed["max_tokens"].as_i64().unwrap_or(1024) >= 4096;
                let arguments = if enough { &arguments } else { &arguments[..20] };
                for fragment in arguments.as_bytes().chunks(37) {
                    events += &chunk(
                        json!({ "tool_calls": [{ "index": 0, "function": {
                        "arguments": String::from_utf8_lossy(fragment),
                    } }] }),
                        None,
                    );
                }
                events + &chunk(json!({}), Some(if enough { "tool_calls" } else { "length" }))
            }
            _ if parsed["model"] == "test/limited-output" => chunk(json!({ "content": "Written." }), Some("stop")),
            _ if matches!(
                parsed["model"].as_str(),
                Some("test/truncated" | "test/invalid-arguments" | "test/not-an-object")
            ) =>
            {
                // Even the complete first call must not run when the second call is incomplete.
                let mut delta = call("write_file", json!({ "path": "first.txt", "content": "Do not write." }));
                let arguments = if parsed["model"] == "test/not-an-object" {
                    "[]"
                } else {
                    r#"{"path":"second.txt""#
                };
                delta["tool_calls"].as_array_mut().expect("calls").push(json!({
                    "index": 1, "id": "second", "type": "function",
                    "function": { "name": "write_file", "arguments": arguments },
                }));
                chunk(delta, None)
                    + &chunk(
                        json!({}),
                        Some(if parsed["model"] == "test/truncated" {
                            "length"
                        } else {
                            "tool_calls"
                        }),
                    )
            }
            _ if parsed["model"] == "test/truncated-text" => {
                chunk(json!({ "content": "Partial answer." }), Some("length"))
            }
            _ if parsed["model"] == "test/empty" => chunk(json!({}), Some("stop")),
            _ if parsed["model"] == "test/whitespace" => {
                chunk(json!({ "content": " \n\t" }), None) + &chunk(json!({}), Some("stop"))
            }
            0 if parsed["model"] == "test/empty-after-tools" => {
                chunk(call("read_file", json!({ "path": "hello.py" })), None) + &chunk(json!({}), Some("tool_calls"))
            }
            _ if parsed["model"] == "test/empty-after-tools" => chunk(json!({}), Some("stop")),
            _ if parsed["model"] == "test/recovered" => {
                chunk(json!({ "content": "Recovered." }), None) + &chunk(json!({}), Some("stop"))
            }
            // "test/docs": the agent reads a topic of its knowledge base, then answers.
            0 if parsed["model"] == "test/docs" => {
                chunk(call("nexus_docs", json!({ "topic": "pages" })), None) + &chunk(json!({}), Some("tool_calls"))
            }
            _ if parsed["model"] == "test/docs" => {
                chunk(json!({ "content": "A page is a file." }), None) + &chunk(json!({}), Some("stop"))
            }
            // "delegate": the lead hands the reading to a subagent, which reads and reports.
            0 if body.contains("delegate") && !subagent => {
                chunk(
                    call(
                        "agent",
                        json!({ "description": "Read hello", "prompt": "Read hello.py and say what it prints." }),
                    ),
                    None,
                ) + &chunk(json!({}), Some("tool_calls"))
            }
            0 if subagent => {
                chunk(call("read_file", json!({ "path": "hello.py" })), None) + &chunk(json!({}), Some("tool_calls"))
            }
            _ if subagent => chunk(json!({ "content": "hello.py prints hi." }), None) + &chunk(json!({}), Some("stop")),
            _ if body.contains("delegate") => {
                chunk(json!({ "content": "The subagent says hi." }), None) + &chunk(json!({}), Some("stop"))
            }
            0 => {
                chunk(
                    call(
                        "edit_file",
                        json!({ "path": "hello.py", "old_string": "hi", "new_string": "hello" }),
                    ),
                    None,
                ) + &chunk(json!({}), Some("tool_calls"))
            }
            1 => {
                chunk(call("run_command", json!({ "command": "cat hello.py" })), None)
                    + &chunk(json!({}), Some("tool_calls"))
            }
            _ => chunk(json!({ "content": "Done." }), None) + &chunk(json!({}), Some("stop")),
        };
        if parsed["model"] != "test/missing-finish" {
            events += "data: [DONE]\n\n";
        }
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{events}"
        );
    }

    struct Run {
        lines: Sender<String>,
        out: Receiver<String>,
    }

    impl Run {
        fn start(url: &str, workspace: &Path, session: Option<Session>) -> Run {
            let (lines, input) = mpsc::channel();
            let (sender, out) = mpsc::channel();
            let gateway = Gateway {
                url: url.into(),
                api_key: "k".into(),
                model: None,
                team: None,
            };
            let workspace = workspace.to_path_buf();
            std::thread::spawn(move || {
                let input = std::io::BufReader::new(Input {
                    lines: input,
                    pending: Vec::new(),
                });
                let _ = serve(
                    gateway,
                    "nexus/auto".into(),
                    workspace,
                    Host::Desktop,
                    None,
                    session,
                    None,
                    input,
                    Output(sender, Vec::new()),
                );
            });
            Run { lines, out }
        }

        fn send(&self, line: Value) {
            let _ = self.lines.send(line.to_string());
        }

        /// The next line of the given type (others are kept in `seen`).
        fn next(&self, kind: &str, seen: &mut Vec<Value>) -> Value {
            loop {
                match self.out.recv_timeout(Duration::from_secs(10)) {
                    Ok(line) => {
                        let value: Value = serde_json::from_str(&line).expect("a JSON line");
                        seen.push(value.clone());
                        if value["type"] == kind {
                            return value;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => panic!("no {kind} line; saw {seen:#?}"),
                    Err(RecvTimeoutError::Disconnected) => panic!("the agent ended before {kind}; saw {seen:#?}"),
                }
            }
        }
    }

    #[test]
    fn a_program_drives_a_turn_and_its_approvals() {
        let workspace = std::env::temp_dir().join(format!("nexus-stdio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
        let workspace = std::fs::canonicalize(&workspace).expect("real path");
        let config = workspace.join("config.json");
        let url = gateway();
        let run = Run::start(&url, &workspace, open(&config, "s1", false, None).ok());
        let mut seen = Vec::new();
        let ready = run.next("ready", &mut seen);
        assert_eq!(
            (
                ready["messages"].as_u64(),
                ready["session"].as_str(),
                &ready["protocol"]
            ),
            (Some(0), Some("s1"), &json!(PROTOCOL))
        );

        run.send(json!({ "type": "prompt", "text": "say hello" }));
        let ask = run.next("approve", &mut seen);
        assert_eq!(ask["ask"]["kind"], "edit");
        run.send(json!({ "type": "decide", "decision": "allow" }));
        let edit = run.next("tool", &mut seen);
        assert_eq!(
            (edit["tool"]["kind"].as_str(), edit["tool"]["path"].as_str()),
            (Some("edit"), Some("hello.py"))
        );
        let ask = run.next("approve", &mut seen);
        assert_eq!(ask["ask"]["command"], "cat hello.py");
        run.send(json!({ "type": "decide", "decision": "allow" }));
        let ran = run.next("tool", &mut seen);
        assert_eq!(ran["tool"]["output"], "print(\"hello\")\n");
        assert_eq!(run.next("delta", &mut seen)["text"], "Done.");
        let done = run.next("done", &mut seen);
        assert_eq!(
            (done["steps"].as_u64(), done["stopped"].as_bool()),
            (Some(3), Some(false))
        );
        assert!(
            (done["cost_usd"].as_f64().unwrap_or(0.0) - 0.003).abs() < 1e-9,
            "{done}"
        );
        assert_eq!(done["messages"], 6, "prompt, 3 answers, 2 tool results");
        assert_eq!(seen.iter().filter(|line| line["type"] == "spent").count(), 3);
        drop(run);

        // The same session goes on where it was.
        let again = Run::start(&url, &workspace, open(&config, "s1", false, None).ok());
        let mut seen = Vec::new();
        assert_eq!(again.next("ready", &mut seen)["messages"], 6);
        drop(again);

        // A fork copies it under a new id and leaves it as it is; a rewind cuts the fork back.
        let fork = open(&config, "s1", true, None).expect("a fork");
        assert!(fork.id.starts_with("fork-") && fork.history.len() == 6, "{fork:?}");
        let cut = open(&config, &fork.id, false, Some(1)).expect("a rewind");
        assert_eq!(cut.history.len(), 1);
        assert_eq!(
            open(&config, &fork.id, false, None).map(|s| s.history.len()),
            Ok(1),
            "the cut is saved"
        );
        assert_eq!(
            open(&config, "s1", false, None).map(|s| s.history.len()),
            Ok(6),
            "the source is unchanged"
        );
        assert_eq!(open(&config, "s1", false, Some(99)).map(|s| s.history.len()), Ok(6));
        let _ = std::fs::remove_dir_all(workspace);
    }

    /// The handshake is the line the desktop's tests read (`tests/fixtures/agent-handshake.json`), with this version.
    #[test]
    fn the_handshake_is_the_fixtures_line() {
        let mut fixture: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/agent-handshake.json")).expect("JSON");
        fixture["version"] = json!(crate::VERSION);
        assert_eq!(handshake(), fixture);
    }

    #[test]
    fn auto_runs_without_asking_and_bad_lines_say_so() {
        let workspace = std::env::temp_dir().join(format!("nexus-stdio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
        let workspace = std::fs::canonicalize(&workspace).expect("real path");
        let run = Run::start(&gateway(), &workspace, None);
        let mut seen = Vec::new();
        run.next("ready", &mut seen);
        let _ = run.lines.send("not json".into());
        assert_eq!(run.next("error", &mut seen)["message"], "a line is one JSON object");
        run.send(json!({ "type": "approvals", "level": "auto" }));
        run.send(json!({ "type": "prompt", "text": "go" }));
        run.next("done", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "approve"), "{seen:#?}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("hello.py")).unwrap_or_default(),
            "print(\"hello\")\n"
        );
        assert!(session_file(Path::new("/x/config.json"), "../escape").is_err());
        let _ = std::fs::remove_dir_all(workspace);
    }

    /// Plan only: the edit and the command are refused without a question, and the file stays as it was.
    #[test]
    fn read_only_refuses_edits_and_commands_without_asking() {
        let workspace = std::env::temp_dir().join(format!("nexus-stdio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
        let workspace = std::fs::canonicalize(&workspace).expect("real path");
        let run = Run::start(&gateway(), &workspace, None);
        let mut seen = Vec::new();
        run.next("ready", &mut seen);
        run.send(json!({ "type": "approvals", "level": "read-only" }));
        run.send(json!({ "type": "prompt", "text": "go" }));
        run.next("done", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "approve"), "{seen:#?}");
        let refused: Vec<_> = seen
            .iter()
            .filter(|line| line["type"] == "tool")
            .map(|line| (line["tool"]["what"].as_str(), line["tool"]["error"].as_str()))
            .collect();
        assert_eq!(
            refused,
            [
                (Some("edit hello.py"), Some("declined")),
                (Some("run cat hello.py"), Some("declined"))
            ]
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("hello.py")).unwrap_or_default(),
            "print(\"hi\")\n"
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn a_large_fragmented_write_uses_the_gateways_output_allowance() {
        let workspace = std::env::temp_dir().join(format!("nexus-large-write-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config = workspace.join("config.json");
        let run = Run::start(&gateway(), &workspace, open(&config, "large", false, None).ok());
        let mut seen = Vec::new();
        run.next("ready", &mut seen);
        run.send(json!({ "type": "prompt", "text": "Write a large file.", "model": "test/limited-output" }));
        let ask = run.next("approve", &mut seen);
        assert_eq!(ask["ask"]["path"], "large.txt");
        run.send(json!({ "type": "decide", "decision": "allow" }));
        run.next("budget", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "error"), "{seen:#?}");
        assert_eq!(seen.iter().filter(|line| line["type"] == "tool").count(), 1);
        let done = seen.iter().find(|line| line["type"] == "done").expect("done");
        assert_eq!(done["steps"], 2, "a refused allowance uses no inference step");
        assert!((done["cost_usd"].as_f64().unwrap_or_default() - 0.002).abs() < 1e-9);
        assert_eq!(
            std::fs::read_to_string(workspace.join("large.txt")).expect("file"),
            "A complete line.\n".repeat(400)
        );
        let saved = open(&config, "large", false, None).expect("saved session");
        assert_eq!(saved.history.len(), 4, "prompt, call, result, answer");
        drop(run);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn a_completed_zero_argument_call_accepts_empty_arguments_and_saves_an_object() {
        for model in ["test/empty-arguments", "test/whitespace-arguments"] {
            let workspace = std::env::temp_dir().join(format!("nexus-empty-arguments-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
            let config = workspace.join("config.json");
            let run = Run::start(
                &gateway(),
                &workspace,
                open(&config, "empty-arguments", false, None).ok(),
            );
            let mut seen = Vec::new();
            run.next("ready", &mut seen);
            run.send(json!({ "type": "prompt", "text": "List the workspace.", "model": model }));
            run.next("budget", &mut seen);
            assert!(!seen.iter().any(|line| line["type"] == "error"), "{model}: {seen:#?}");
            let tool = seen.iter().find(|line| line["type"] == "tool").expect("list tool ran");
            assert_eq!(tool["tool"]["kind"], "list");
            assert!(seen.iter().any(|line| line["type"] == "done"), "{model}: {seen:#?}");
            let saved = open(&config, "empty-arguments", false, None).expect("saved session");
            assert_eq!(saved.history[1]["tool_calls"][0]["function"]["arguments"], "{}");
            assert!(
                saved.history[2]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hello.py")
            );
            drop(run);
            let _ = std::fs::remove_dir_all(workspace);
        }
    }

    #[test]
    fn incomplete_steps_run_no_tools_and_the_next_prompt_still_works() {
        for model in [
            "test/truncated",
            "test/invalid-arguments",
            "test/not-an-object",
            "test/truncated-text",
            "test/missing-finish",
        ] {
            let workspace = std::env::temp_dir().join(format!("nexus-incomplete-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&workspace).expect("workspace");
            let config = workspace.join("config.json");
            let run = Run::start(&gateway(), &workspace, open(&config, "incomplete", false, None).ok());
            let mut seen = Vec::new();
            run.next("ready", &mut seen);
            // Automatic approval exposes any accidental execution of the first complete call.
            run.send(json!({ "type": "approvals", "level": "auto" }));
            run.send(json!({ "type": "prompt", "text": "Write two files.", "model": model }));
            run.next("budget", &mut seen);
            let error = seen.iter().find(|line| line["type"] == "error").expect("error");
            let reason = if model.starts_with("test/truncated") {
                "output limit"
            } else if model == "test/missing-finish" {
                "stream ended"
            } else {
                "invalid arguments"
            };
            assert!(
                error["message"].as_str().unwrap_or_default().contains(reason),
                "{model}: {seen:#?}"
            );
            assert!(
                !seen
                    .iter()
                    .any(|line| matches!(line["type"].as_str(), Some("tool" | "approve" | "done"))),
                "{seen:#?}"
            );
            assert_eq!(seen.iter().filter(|line| line["type"] == "spent").count(), 1);
            assert!(!workspace.join("first.txt").exists());
            assert!(!workspace.join("second.txt").exists());
            let saved = open(&config, "incomplete", false, None).expect("saved session");
            assert!(saved.history.iter().all(|message| message.get("tool_calls").is_none()));
            assert_eq!(saved.history.len(), if model == "test/truncated-text" { 2 } else { 1 });

            seen.clear();
            run.send(json!({ "type": "prompt", "text": "Continue.", "model": "test/recovered" }));
            run.next("budget", &mut seen);
            assert!(seen.iter().any(|line| line["type"] == "done"), "{seen:#?}");
            assert!(!seen.iter().any(|line| line["type"] == "error"), "{seen:#?}");
            drop(run);
            let _ = std::fs::remove_dir_all(workspace);
        }
    }

    #[test]
    fn an_empty_answer_reports_an_error_and_the_next_prompt_still_works() {
        for (model, messages, steps) in [
            ("test/empty", 1, 1),
            ("test/whitespace", 1, 1),
            ("test/empty-after-tools", 3, 2),
        ] {
            let workspace = std::env::temp_dir().join(format!("nexus-empty-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
            let config = workspace.join("config.json");
            let run = Run::start(&gateway(), &workspace, open(&config, "empty", false, None).ok());
            let mut seen = Vec::new();
            run.next("ready", &mut seen);
            run.send(json!({ "type": "prompt", "text": "Review the code.", "model": model }));
            run.next("budget", &mut seen);
            let error = seen.iter().find(|line| line["type"] == "error");
            assert_eq!(
                error.and_then(|line| line["message"].as_str()),
                Some("The model ended without an answer."),
                "{model}: {seen:#?}"
            );
            assert!(!seen.iter().any(|line| line["type"] == "done"), "{seen:#?}");
            assert_eq!(seen.iter().filter(|line| line["type"] == "spent").count(), steps);
            let saved = open(&config, "empty", false, None).expect("saved session");
            assert_eq!(saved.history.len(), messages, "{model}: {:?}", saved.history);

            seen.clear();
            run.send(json!({ "type": "prompt", "text": "Continue.", "model": "test/recovered" }));
            run.next("budget", &mut seen);
            assert!(!seen.iter().any(|line| line["type"] == "error"), "{seen:#?}");
            let done = seen.iter().find(|line| line["type"] == "done").expect("done");
            assert_eq!(done["messages"], messages + 2);
            assert_eq!(
                seen.iter().find(|line| line["type"] == "delta").expect("text")["text"],
                "Recovered."
            );
            drop(run);
            let _ = std::fs::remove_dir_all(workspace);
        }
    }

    #[test]
    fn a_saved_empty_answer_can_resume_without_changing_rewind_positions() {
        let workspace = std::env::temp_dir().join(format!("nexus-recover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config = workspace.join("config.json");
        let file = session_file(&config, "legacy").expect("session path");
        let original = vec![
            json!({ "role": "user", "content": "Review the code." }),
            json!({ "role": "assistant", "content": null, "tool_calls": [{
                "id": "read-1", "type": "function", "function": { "name": "read_file", "arguments": "{}" },
            }] }),
            json!({ "role": "tool", "tool_call_id": "read-1", "content": "The code." }),
            json!({ "role": "assistant", "content": null }),
        ];
        save(Some(&file), &Arc::new(Mutex::new(original.clone())));
        let run = Run::start(&gateway(), &workspace, open(&config, "legacy", false, None).ok());
        let mut seen = Vec::new();
        assert_eq!(run.next("ready", &mut seen)["messages"], original.len());
        run.send(json!({ "type": "prompt", "text": "Continue.", "model": "test/recovered" }));
        run.next("budget", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "error"), "{seen:#?}");
        let done = seen.iter().find(|line| line["type"] == "done").expect("done");
        assert_eq!(done["messages"], original.len() + 2);
        let saved = open(&config, "legacy", false, None).expect("saved session");
        assert_eq!(&saved.history[..original.len()], original);
        drop(run);

        let fork = open(&config, "legacy", true, Some(original.len())).expect("fork at old turn boundary");
        assert_eq!(fork.history, original);
        let again = Run::start(&gateway(), &workspace, Some(fork));
        let mut seen = Vec::new();
        assert_eq!(again.next("ready", &mut seen)["messages"], original.len());
        again.send(json!({ "type": "prompt", "text": "Continue.", "model": "test/recovered" }));
        again.next("budget", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "error"), "{seen:#?}");
        assert!(seen.iter().any(|line| line["type"] == "done"), "{seen:#?}");
        drop(again);
        let _ = std::fs::remove_dir_all(workspace);
    }

    /// The knowledge base reads only, so it runs at `ask` with no question. A program sees it as a tool by its name,
    /// and the model gets the topic's text.
    #[test]
    fn the_agent_reads_its_knowledge_base_without_asking() {
        let workspace = std::env::temp_dir().join(format!("nexus-docs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config = workspace.join("config.json");
        let run = Run::start(&gateway(), &workspace, open(&config, "docs", false, None).ok());
        let mut seen = Vec::new();
        run.next("ready", &mut seen);
        run.send(json!({ "type": "prompt", "text": "What is a page?", "model": "test/docs" }));
        let tool = run.next("tool", &mut seen);
        assert_eq!(
            (
                tool["tool"]["kind"].as_str(),
                tool["tool"]["name"].as_str(),
                tool["tool"]["input"]["topic"].as_str(),
                tool["tool"]["error"].as_bool()
            ),
            (Some("call"), Some("nexus_docs"), Some("pages"), Some(false))
        );
        assert!(
            tool["tool"]["output"].as_str().unwrap_or("").starts_with("# Pages"),
            "{tool}"
        );
        run.next("done", &mut seen);
        assert!(!seen.iter().any(|line| line["type"] == "approve"), "{seen:#?}");
        let saved = open(&config, "docs", false, None).expect("the session");
        assert!(
            saved.history[2]["content"]
                .as_str()
                .unwrap_or("")
                .starts_with("# Pages"),
            "the model got the topic: {:?}",
            saved.history[2]
        );
        drop(run);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn a_subagent_works_in_its_own_conversation_and_reports_back() {
        let workspace = std::env::temp_dir().join(format!("nexus-stdio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("hello.py"), "print(\"hi\")\n").expect("file");
        let workspace = std::fs::canonicalize(&workspace).expect("real path");
        let config = workspace.join("config.json");
        let run = Run::start(&gateway(), &workspace, open(&config, "lead", false, None).ok());
        let mut seen = Vec::new();
        run.next("ready", &mut seen);
        run.send(json!({ "type": "prompt", "text": "delegate the reading" }));
        let started = run.next("task", &mut seen);
        assert_eq!(
            (
                started["task"]["state"].as_str(),
                started["task"]["description"].as_str()
            ),
            (Some("started"), Some("Read hello"))
        );
        let read = run.next("tool", &mut seen);
        assert_eq!(
            (read["parent"].as_str(), read["tool"]["kind"].as_str()),
            (Some("agent-1"), Some("read"))
        );
        let finished = run.next("task", &mut seen);
        assert_eq!(
            (finished["task"]["state"].as_str(), finished["task"]["result"].as_str()),
            (Some("completed"), Some("hello.py prints hi."))
        );
        let done = run.next("done", &mut seen);
        let deltas: Vec<&str> = seen
            .iter()
            .filter(|line| line["type"] == "delta")
            .filter_map(|line| line["text"].as_str())
            .collect();
        assert_eq!(deltas, ["The subagent says hi."], "only the lead's text streams");
        assert_eq!(done["steps"], 2, "the lead's own steps");
        assert!(
            (done["cost_usd"].as_f64().unwrap_or(0.0) - 0.004).abs() < 1e-9,
            "both agents' calls count: {done}"
        );
        // The lead's conversation holds the subagent's report, not its work.
        let saved = open(&config, "lead", false, None).expect("the session");
        let text = serde_json::to_string(&saved.history).unwrap_or_default();
        assert!(
            text.contains("hello.py prints hi.") && !text.contains("read_file"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(workspace);
    }
}
