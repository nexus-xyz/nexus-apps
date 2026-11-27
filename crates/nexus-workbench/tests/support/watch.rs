//! Helpers the `terminal_watch_*` tests share: the session's MCP endpoint, agent terminal calls, and readers for the
//! notices a watched command leaves in the transcript.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use nexus_workbench::event::{PermissionMode, SessionAgent, StoredEvent, ThreadSummary, WbEvent};
use nexus_workbench::orchestra::TOKEN_VAR;
use nexus_workbench::{Config, Workbench};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{Setup, bounded, new_root, root, setup_with, wait_for};

pub struct Launched {
    pub args: Vec<String>,
    pub env: serde_json::Map<String, Value>,
}

#[derive(Clone)]
pub struct Endpoint {
    pub port: u16,
    pub path: String,
    pub token: String,
}

fn record_path(s: &Setup) -> PathBuf {
    s.root.join("launch.json")
}

fn record_launches(s: &Setup) {
    let mut extra = s.host.extra.lock().unwrap_or_else(|p| p.into_inner());
    extra.insert(
        "FAKE_LAUNCH_RECORD".into(),
        record_path(s).to_string_lossy().into_owned(),
    );
    extra.insert("FAKE_RECORD_SECRETS".into(), "1".into());
}

pub async fn launched(s: &Setup, ready: impl Fn(&Launched) -> bool) -> Launched {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = std::fs::read_to_string(record_path(s))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Launched {
                args: value["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect(),
                env: value["env"].as_object().cloned().unwrap_or_default(),
            });
        if let Some(found) = found.filter(&ready) {
            return found;
        }
        assert!(Instant::now() < deadline, "no launch recorded");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn endpoint(launch: &Launched) -> Option<Endpoint> {
    let url = launch.args.iter().find_map(|arg| {
        if let Some(config) = arg.strip_prefix("--mcp-config=") {
            let config: Value = serde_json::from_str(config).ok()?;
            return config.pointer("/mcpServers/nexus_app/url")?.as_str().map(String::from);
        }
        let table = arg.strip_prefix("mcp_servers.nexus_app={url=\"")?;
        table.split_once('"').map(|(url, _)| url.to_string())
    })?;
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let (port, path) = rest.split_once('/')?;
    Some(Endpoint {
        port: port.parse().ok()?,
        path: format!("/{path}"),
        token: launch.env.get(TOKEN_VAR).and_then(Value::as_str)?.to_string(),
    })
}

/// The MCP endpoint of the session's first launch.
pub async fn first_endpoint(s: &Setup) -> Endpoint {
    endpoint(&launched(s, |l| endpoint(l).is_some()).await).expect("MCP endpoint")
}

pub async fn post_rpc(at: &Endpoint, body: &Value) -> Value {
    let bytes = body.to_string();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        at.path,
        at.port,
        at.token,
        bytes.len(),
        bytes
    );
    let mut stream = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .expect("connect to orchestra MCP server");
    stream.write_all(request.as_bytes()).await.expect("write HTTP request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read HTTP response");
    let text = String::from_utf8_lossy(&raw);
    let (_, body) = text.split_once("\r\n\r\n").expect("HTTP response body");
    serde_json::from_str(body).expect("JSON-RPC response")
}

pub async fn mcp_call(at: &Endpoint, id: u64, name: &str, args: Value) -> (Value, bool) {
    let response = post_rpc(
        at,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        }),
    )
    .await;
    let is_error = response["result"]["isError"].as_bool().unwrap_or(true);
    let content = response["result"]["content"][0]["text"]
        .as_str()
        .expect("tool content text");
    (serde_json::from_str(content).expect("tool JSON content"), is_error)
}

pub fn setup_terminal(name: &str) -> Setup {
    let s = setup_with(name, bounded(), false, |config| Config {
        terminal: true,
        shell: nexus_workbench::process::Shell::sh(),
        ..config
    });
    record_launches(&s);
    s
}

/// A Claude Code session in Full access whose first turn has ended.
pub async fn idle_session(s: &Setup) -> ThreadSummary {
    session_of(s, SessionAgent::Claude).await
}

pub async fn session_of(s: &Setup, agent: SessionAgent) -> ThreadSummary {
    let mut new = new_root(&s.project, "hello");
    new.agent = agent;
    new.orchestration = false;
    new.permission_mode = PermissionMode::BypassPermissions;
    root(s, new).await
}

/// The notices the agent was sent.
pub fn watch_notices(events: &[StoredEvent]) -> Vec<String> {
    notices_where(events, |text| {
        text.starts_with("The watched command ") && text.ends_with(" Read its output with terminal_read.")
    })
}

/// The rows of notices the agent could not take.
pub fn undelivered(events: &[StoredEvent]) -> Vec<String> {
    notices_where(events, |text| {
        text.starts_with("The watched ") && text.contains(" The agent was not told: ")
    })
}

/// Whether `notice` is the one the agent gets when command `id`, by any name, exits with `code`.
pub fn is_notice_for(notice: &str, id: &str, code: u32) -> bool {
    notice.starts_with("The watched command \"")
        && notice.ends_with(&format!(
            "\" ({id}) exited with code {code}. Read its output with terminal_read."
        ))
}

fn notices_where(events: &[StoredEvent], matches: impl Fn(&str) -> bool) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            WbEvent::Notice { text } if matches(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

pub fn texts(events: &[StoredEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            WbEvent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Starts `command` as the session's agent would, with `watch` on, and returns its id.
pub async fn run_watched(wb: &Workbench, thread_id: &str, request_id: &str, command: &str) -> String {
    let started = wb
        .terminal(
            thread_id,
            "run",
            json!({"command": command, "request_id": request_id, "watch": true}),
            false,
        )
        .await
        .expect("agent terminal_run");
    started["id"].as_str().expect("command id").to_string()
}

/// A shell command that waits until `gate` exists, then runs `then`. The test decides when it exits.
pub fn gated(gate: &Path, then: &str) -> String {
    format!("while [ ! -e '{}' ]; do sleep 0.02; done; {then}", gate.display())
}

pub fn open(gate: &Path) {
    std::fs::write(gate, b"").expect("open the gate");
}

pub async fn wait_command_ended(wb: &Workbench, thread_id: &str, cmd_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = wb
            .terminal(thread_id, "read", json!({"id": cmd_id, "wait_ms": 0}), true)
            .await
            .expect("human read");
        if !matches!(
            read["terminal"]["status"].as_str(),
            Some("starting" | "running" | "awaitingUser")
        ) {
            return;
        }
        assert!(Instant::now() < deadline, "command {cmd_id} did not end");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Reads as the person until the output holds `text`, and returns the cursor after it.
pub async fn output_seen(wb: &Workbench, thread_id: &str, cmd_id: &str, text: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let read = wb
            .terminal(thread_id, "read", json!({"id": cmd_id, "wait_ms": 0}), true)
            .await
            .expect("human read");
        if read["output"].as_str().unwrap_or_default().contains(text) {
            return read["next"].as_u64().expect("next cursor");
        }
        assert!(Instant::now() < deadline, "{cmd_id} never printed {text:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub type Read<'a> = Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>;

/// An agent `terminal_read` from `after`, polled once. The read path awaits nothing before it registers as a waiting
/// reader, so when this returns the read waits on the command: an exit now finds it waiting.
pub async fn waiting_agent_read<'a>(wb: &'a Workbench, thread_id: &'a str, cmd_id: &str, after: u64) -> Read<'a> {
    let mut read: Read<'a> = Box::pin(wb.terminal(
        thread_id,
        "read",
        json!({"id": cmd_id, "after": after, "wait_ms": 10_000}),
        false,
    ));
    assert!(futures::poll!(read.as_mut()).is_pending(), "the agent read must wait");
    read
}

/// The rows that say further notices were dropped because the budget is spent.
pub fn budget_rows(events: &[StoredEvent]) -> usize {
    notices_where(events, |text| text == BUDGET_SPENT).len()
}

pub const BUDGET_SPENT: &str =
    "More watched commands ended. The agent is not told about them until you send a message.";

/// Runs a watched command that exits at once, and returns the notices in the transcript once its notice arrived.
/// Nexus delivers due notices one at a time, in the order the commands' ends were published (`ExitQueue`). So when
/// this notice arrives, the delivery attempt of every command that ended before it started has finished, and it is
/// the latest notice.
pub async fn marker(wb: &Workbench, thread_id: &str, name: &str) -> Vec<String> {
    let id = run_watched(wb, thread_id, name, &format!("printf '{name}'")).await;
    let events = wait_for(wb, thread_id, |events| {
        watch_notices(events).iter().any(|notice| is_notice_for(notice, &id, 0))
    })
    .await;
    let notices = watch_notices(&events);
    assert!(
        notices.last().is_some_and(|notice| is_notice_for(notice, &id, 0)),
        "{notices:?}"
    );
    notices
}
