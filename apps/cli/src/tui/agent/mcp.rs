//! MCP servers for the agent (`nexus __agent --mcp-config=<json>`): the desktop gives a session the app's
//! own server this way, so the agent can start other sessions and read their results, as Claude Code and Codex do.
//!
//! The configuration is Claude Code's (`{"mcpServers": {name: {...}}}`), so the desktop builds one for both: a server
//! is `{"type": "stdio", "command", "args", "env"}` or `{"type": "http", "url", "headers"}`, and `${VAR}` in a value
//! is that variable of the agent's environment. Each server's tools reach the model as `mcp__<server>__<tool>`, the
//! names Claude Code gives them, so the desktop's lists of tools that run without asking hold for both.
//!
//! A server is MCP's JSON-RPC over the server's stdin and stdout (one message a line), or over HTTP: one POST a
//! message, answered with one JSON body or with an event stream that ends in the answer.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

/// The MCP version asked for; a server that speaks another answers with its own.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// How long a server may take to start and list its tools.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// The most text a tool's answer sends back to the model.
const MAX_RESULT_CHARS: usize = 20_000;

/// The servers this run was given, and their tools.
pub struct Mcp {
    servers: Vec<Server>,
    tools: Vec<Tool>,
    /// How long one tool call may take.
    timeout: Duration,
}

struct Tool {
    /// `mcp__<server>__<tool>`.
    full: String,
    server: usize,
    name: String,
    description: String,
    schema: Value,
}

struct Server {
    name: String,
    transport: Mutex<Transport>,
}

enum Transport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        lines: Receiver<String>,
        next: u64,
    },
    Http {
        client: reqwest::blocking::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Option<String>,
        next: u64,
    },
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(transport) = self.transport.get_mut()
            && let Transport::Stdio { child, .. } = transport
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// `${VAR}` in `text` as `var` gives it; an unset variable is empty.
fn expand(text: &str, var: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let Some(end) = rest[start..].find('}') else { break };
        out.push_str(&rest[..start]);
        out.push_str(&var(&rest[start + 2..start + end]).unwrap_or_default());
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// A tool or server name as it may stand in a function name.
fn clean(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Mcp {
    /// Starts every server in `config` and lists its tools. A server that fails is left out, and why is in the notes.
    /// `timeout`: how long one tool call may take.
    pub fn start(config: &Value, var: &dyn Fn(&str) -> Option<String>, timeout: Duration) -> (Mcp, Vec<String>) {
        let mut mcp = Mcp {
            servers: Vec::new(),
            tools: Vec::new(),
            timeout,
        };
        let mut notes = Vec::new();
        let configured = config
            .get("mcpServers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (name, server) in configured {
            match Server::connect(&name, &server, var).and_then(|server| server.tools().map(|tools| (server, tools))) {
                Ok((server, tools)) => {
                    let index = mcp.servers.len();
                    for tool in tools {
                        let Some(tool_name) = tool.get("name").and_then(Value::as_str) else {
                            continue;
                        };
                        mcp.tools.push(Tool {
                            full: format!("mcp__{}__{}", clean(&name), clean(tool_name)),
                            server: index,
                            name: tool_name.to_string(),
                            description: tool
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            schema: tool
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({ "type": "object" })),
                        });
                    }
                    mcp.servers.push(server);
                }
                Err(error) => notes.push(format!("The MCP server {name} did not start: {error}")),
            }
        }
        (mcp, notes)
    }

    /// The tools, as `/v1/chat/completions` takes them.
    pub fn specs(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": { "name": tool.full, "description": tool.description, "parameters": tool.schema },
                })
            })
            .collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.full == name)
    }

    /// Each server that started, with its tools by their own names: what the agent's prompt says it has (`prompt.rs`).
    pub fn servers(&self) -> Vec<(String, Vec<String>)> {
        self.servers
            .iter()
            .enumerate()
            .map(|(index, server)| {
                let tools = self
                    .tools
                    .iter()
                    .filter(|tool| tool.server == index)
                    .map(|tool| tool.name.clone())
                    .collect();
                (server.name.clone(), tools)
            })
            .collect()
    }

    /// Calls tool `name` (`mcp__…`) with `input`: its answer's text, or why it failed.
    pub fn call(&self, name: &str, input: &Value) -> Result<String, String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.full == name)
            .ok_or_else(|| format!("no tool {name}"))?;
        let server = &self.servers[tool.server];
        let result = server.request(
            "tools/call",
            json!({ "name": tool.name, "arguments": input }),
            self.timeout,
        )?;
        let mut text: String = result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                Some(kind) => format!("[{kind} content]"),
                None => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty()
            && let Some(structured) = result.get("structuredContent")
        {
            text = structured.to_string();
        }
        if text.chars().count() > MAX_RESULT_CHARS {
            text = text.chars().take(MAX_RESULT_CHARS).collect::<String>() + "\n[cut: the answer was longer]";
        }
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(text);
        }
        Ok(text)
    }
}

impl Server {
    fn connect(name: &str, config: &Value, var: &dyn Fn(&str) -> Option<String>) -> Result<Server, String> {
        let text = |key: &str| config.get(key).and_then(Value::as_str).map(|value| expand(value, var));
        let pairs = |key: &str| -> Vec<(String, String)> {
            config
                .get(key)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(name, value)| (name.clone(), expand(value.as_str().unwrap_or(""), var)))
                .collect()
        };
        let kind = text("type").unwrap_or_else(|| if config.get("url").is_some() { "http" } else { "stdio" }.into());
        let transport = match kind.as_str() {
            "stdio" => {
                let command = text("command").ok_or("it names no command")?;
                let args: Vec<String> = config
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|arg| expand(arg, var))
                    .collect();
                let mut child = Command::new(&command)
                    .args(&args)
                    .envs(pairs("env"))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| format!("{command}: {error}"))?;
                let stdin = child.stdin.take().ok_or("no stdin")?;
                let stdout = child.stdout.take().ok_or("no stdout")?;
                let (sender, lines) = mpsc::channel();
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                });
                Transport::Stdio {
                    child,
                    stdin,
                    lines,
                    next: 0,
                }
            }
            "http" | "streamable-http" => Transport::Http {
                client: crate::client::http_client(),
                url: text("url").ok_or("it names no url")?,
                headers: pairs("headers"),
                session: None,
                next: 0,
            },
            other => return Err(format!("its type {other:?} is not stdio or http")),
        };
        let server = Server {
            name: name.to_string(),
            transport: Mutex::new(transport),
        };
        server.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "nexus", "version": crate::VERSION },
            }),
            START_TIMEOUT,
        )?;
        server.notify("notifications/initialized")?;
        Ok(server)
    }

    fn tools(&self) -> Result<Vec<Value>, String> {
        let listed = self.request("tools/list", json!({}), START_TIMEOUT)?;
        Ok(listed
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Sends a request and waits up to `timeout` for its answer: the result, or the error's message.
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let mut transport = self.transport.lock().map_err(|_| "the server's connection broke")?;
        let answer = match &mut *transport {
            Transport::Stdio { stdin, lines, next, .. } => {
                *next += 1;
                let id = *next;
                let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
                writeln!(stdin, "{message}")
                    .and_then(|()| stdin.flush())
                    .map_err(|error| error.to_string())?;
                let deadline = std::time::Instant::now() + timeout;
                loop {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    let line = match lines.recv_timeout(left) {
                        Ok(line) => line,
                        Err(mpsc::RecvTimeoutError::Timeout) => return Err(format!("no answer to {method} in time")),
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the server ended".into()),
                    };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                        break message;
                    }
                    // The server's own requests (roots, sampling) are not offered: say so, so it does not wait.
                    if let (Some(their), Some(_)) = (message.get("id"), message.get("method")) {
                        let refusal = json!({ "jsonrpc": "2.0", "id": their,
                            "error": { "code": -32601, "message": "not supported" } });
                        let _ = writeln!(stdin, "{refusal}").and_then(|()| stdin.flush());
                    }
                }
            }
            Transport::Http {
                client,
                url,
                headers,
                session,
                next,
            } => {
                *next += 1;
                let id = *next;
                let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
                let response = post(client, url, headers, session.as_deref(), &message, timeout)?;
                if let Some(id) = response
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|value| value.to_str().ok())
                {
                    *session = Some(id.to_string());
                }
                answer_of(response, id)?
            }
        };
        if let Some(error) = answer.get("error") {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("an error");
            return Err(format!("{} said: {message}", self.name));
        }
        Ok(answer.get("result").cloned().unwrap_or(Value::Null))
    }

    fn notify(&self, method: &str) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        let mut transport = self.transport.lock().map_err(|_| "the server's connection broke")?;
        match &mut *transport {
            Transport::Stdio { stdin, .. } => writeln!(stdin, "{message}")
                .and_then(|()| stdin.flush())
                .map_err(|error| error.to_string()),
            Transport::Http {
                client,
                url,
                headers,
                session,
                ..
            } => post(client, url, headers, session.as_deref(), &message, START_TIMEOUT).map(|_| ()),
        }
    }
}

fn post(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &[(String, String)],
    session: Option<&str>,
    message: &Value,
    timeout: Duration,
) -> Result<reqwest::blocking::Response, String> {
    let mut request = client
        .post(url)
        .timeout(timeout)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", PROTOCOL_VERSION)
        .body(message.to_string());
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("HTTP {status}: {}", body.chars().take(300).collect::<String>()));
    }
    Ok(response)
}

/// The answer to request `id` in an HTTP response: its one JSON body, or the event of its stream that carries it.
fn answer_of(response: reqwest::blocking::Response, id: u64) -> Result<Value, String> {
    let stream = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|kind| kind.starts_with("text/event-stream"));
    let is_answer = |message: &Value| message.get("id") == Some(&json!(id)) && message.get("method").is_none();
    if !stream {
        let body: Value = response.json().map_err(|error| error.to_string())?;
        let messages = match body {
            Value::Array(messages) => messages,
            message => vec![message],
        };
        return messages
            .into_iter()
            .find(is_answer)
            .ok_or_else(|| "the server gave no answer".into());
    }
    let mut data = String::new();
    for line in BufReader::new(response).lines() {
        let line = line.map_err(|error| error.to_string())?;
        if let Some(part) = line.strip_prefix("data:") {
            data.push_str(part.strip_prefix(' ').unwrap_or(part));
            data.push('\n');
        } else if line.is_empty() && !data.is_empty() {
            if let Ok(message) = serde_json::from_str::<Value>(&data)
                && is_answer(&message)
            {
                return Ok(message);
            }
            data.clear();
        }
    }
    serde_json::from_str::<Value>(&data)
        .ok()
        .filter(is_answer)
        .ok_or_else(|| "the stream ended without an answer".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn expands_variables_and_names_tools_as_claude_code_does() {
        let var = |name: &str| (name == "TOKEN").then(|| "t0k".to_string());
        assert_eq!(expand("Bearer ${TOKEN}", &var), "Bearer t0k");
        assert_eq!(expand("${MISSING}x${TOKEN}", &var), "xt0k");
        assert_eq!(clean("nexus_app"), "nexus_app");
        assert_eq!(clean("my server.v2"), "my_server_v2");
    }

    /// An MCP server over HTTP: answers `initialize` and `tools/list` with JSON, and `tools/call` as an event stream
    /// with a progress note before the answer. Checks the bearer header each time. It closes each connection after its
    /// answer and says so, or the client could send its next request on a connection already closed.
    fn http_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let body = loop {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break String::new();
                    };
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| line.to_lowercase().strip_prefix("content-length: ").map(String::from))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if body.len() >= length {
                            if !head.to_lowercase().contains("authorization: bearer t0k") {
                                break "unauthorized".into();
                            }
                            break body.to_string();
                        }
                    }
                    if read == 0 {
                        break String::new();
                    }
                };
                let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let id = message["id"].clone();
                let (kind, payload) = match message["method"].as_str() {
                    _ if body == "unauthorized" => ("401", String::new()),
                    Some("initialize") => (
                        "json",
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": PROTOCOL_VERSION } })
                            .to_string(),
                    ),
                    Some("tools/list") => (
                        "json",
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [
                            { "name": "start_session", "description": "Starts a session.",
                              "inputSchema": { "type": "object", "properties": { "prompt": { "type": "string" } } } },
                        ] } })
                        .to_string(),
                    ),
                    Some("tools/call") => (
                        "stream",
                        format!(
                            "data: {}\n\ndata: {}\n\n",
                            json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [
                                { "type": "text", "text": format!("started: {}", message["params"]["arguments"]["prompt"].as_str().unwrap_or("")) }
                            ] } })
                        ),
                    ),
                    _ => ("202", String::new()),
                };
                let response = match kind {
                    "401" => "HTTP/1.1 401 Unauthorized\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_string(),
                    "202" => "HTTP/1.1 202 Accepted\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_string(),
                    "json" => format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{payload}",
                        payload.len()
                    ),
                    _ => format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{payload}",
                        payload.len()
                    ),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}/mcp/agent-1")
    }

    #[test]
    fn calls_a_tool_over_http_through_an_event_stream() {
        let config = json!({ "mcpServers": { "nexus_app": {
            "type": "http", "url": http_server(), "headers": { "Authorization": "Bearer ${TOKEN}" } } } });
        let var = |name: &str| (name == "TOKEN").then(|| "t0k".to_string());
        let (mcp, notes) = Mcp::start(&config, &var, Duration::from_secs(10));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(mcp.specs()[0]["function"]["name"], "mcp__nexus_app__start_session");
        assert!(mcp.has("mcp__nexus_app__start_session"));
        assert_eq!(mcp.servers()[0].0, "nexus_app");
        assert!(mcp.servers()[0].1.contains(&"start_session".to_string()));
        assert_eq!(
            mcp.call("mcp__nexus_app__start_session", &json!({ "prompt": "fix the tests" })),
            Ok("started: fix the tests".into())
        );

        // A wrong token: the server is left out, and the note says why.
        let (mcp, notes) = Mcp::start(&config, &|_: &str| None, Duration::from_secs(10));
        assert!(mcp.specs().is_empty());
        assert!(notes[0].contains("401"), "{notes:?}");
    }

    #[test]
    fn calls_a_tool_over_stdio() {
        // A server in a few lines of shell: it answers each request by its id.
        let script = r#"while read -r line; do
            id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
            case "$line" in
              *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
              *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo"}]}}\n' "$id" ;;
              *'"tools/call"'*) printf '{"jsonrpc":"2.0","method":"notifications/message"}\n{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}],"isError":true}}\n' "$id" ;;
            esac
          done"#;
        let config = json!({ "mcpServers": { "shell": { "command": "sh", "args": ["-c", script] } } });
        let (mcp, notes) = Mcp::start(&config, &|_: &str| None, Duration::from_secs(10));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            mcp.call("mcp__shell__echo", &json!({})),
            Err("pong".into()),
            "isError is an error"
        );

        let (_, notes) = Mcp::start(
            &json!({ "mcpServers": { "gone": { "command": "/no/such/server" } } }),
            &|_: &str| None,
            Duration::from_secs(1),
        );
        assert!(notes[0].starts_with("The MCP server gone did not start"), "{notes:?}");
    }
}
