//! The stand-ins' MCP client for the app's `nexus_app` server (src/orchestra/), with no dependency beyond the standard
//! library and serde_json: one blocking HTTP/1.1 POST per message over a `TcpStream`, a JSON or an event-stream
//! answer. `fake-claude` reads the server from its `--mcp-config`, `fake-codex` from its `-c mcp_servers.nexus_app`
//! override, and both read the token from `NEXUS_APP_MCP_TOKEN`, as the real agents do.

// Each stand-in reads its own agent's way of naming the server only.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

/// The app's server as an agent was told of it.
pub struct Server {
    port: u16,
    path: String,
    token: String,
    next: u64,
}

/// One tool call and how it went: its name, its arguments, the result's text, and whether it is an error.
pub struct Called {
    pub tool: String,
    pub args: Value,
    pub text: String,
    pub is_error: bool,
}

fn token(variable: &str) -> Option<String> {
    std::env::var(variable).ok().filter(|token| !token.is_empty())
}

/// `http://127.0.0.1:<port>/<path>`: the port and the path.
fn split_url(url: &str) -> Option<(u16, String)> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let (port, path) = rest.split_once('/')?;
    Some((port.parse().ok()?, format!("/{path}")))
}

impl Server {
    /// From Claude Code's arguments: the `--mcp-config=` that names `nexus_app`, its URL, and the variable its
    /// `Authorization` header expands.
    pub fn claude(args: &[String]) -> Option<Server> {
        args.iter().find_map(|arg| {
            let config: Value = serde_json::from_str(arg.strip_prefix("--mcp-config=")?).ok()?;
            let server = config.pointer("/mcpServers/nexus_app")?;
            let (port, path) = split_url(server.get("url")?.as_str()?)?;
            let header = server.pointer("/headers/Authorization")?.as_str()?;
            let variable = header.strip_prefix("Bearer ${")?.strip_suffix('}')?;
            Some(Server {
                port,
                path,
                token: token(variable)?,
                next: 0,
            })
        })
    }

    /// From Codex's arguments: the `-c mcp_servers.nexus_app={url="…",bearer_token_env_var="…",…}` override.
    pub fn codex(args: &[String]) -> Option<Server> {
        args.iter().find_map(|arg| {
            let table = arg.strip_prefix("mcp_servers.nexus_app=")?;
            let field = |name: &str| {
                let start = table.find(&format!("{name}=\""))? + name.len() + 2;
                let end = table[start..].find('"')? + start;
                Some(table[start..end].to_string())
            };
            let (port, path) = split_url(&field("url")?)?;
            Some(Server {
                port,
                path,
                token: token(&field("bearer_token_env_var")?)?,
                next: 0,
            })
        })
    }

    /// One POST of `body`: the status, the content type and the body.
    fn post(&self, body: &Value) -> Result<(u16, String, String), String> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).map_err(|e| e.to_string())?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
        let body = body.to_string();
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.path,
            self.port,
            self.token,
            body.len()
        );
        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no head")?;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let body = &raw[split + 4..];
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse().ok())
            .ok_or("no status")?;
        let mut kind = String::new();
        let mut chunked = false;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            match name.trim().to_ascii_lowercase().as_str() {
                "content-type" => kind = value.trim().to_string(),
                "transfer-encoding" => chunked = value.trim().eq_ignore_ascii_case("chunked"),
                _ => {}
            }
        }
        let body = if chunked { dechunk(body) } else { body.to_vec() };
        Ok((status, kind, String::from_utf8_lossy(&body).into_owned()))
    }

    /// A JSON-RPC request: its result, or why it failed.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let (status, kind, body) =
            self.post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        if status != 200 {
            return Err(format!("HTTP {status}: {}", body.trim()));
        }
        let answer = if kind.starts_with("text/event-stream") {
            body.lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                .find(|message| message.get("id") == Some(&json!(id)))
                .ok_or("the stream ended with no answer")?
        } else {
            serde_json::from_str(&body).map_err(|e| e.to_string())?
        };
        match answer.get("error") {
            Some(error) => Err(error.to_string()),
            None => Ok(answer.get("result").cloned().unwrap_or(Value::Null)),
        }
    }

    /// A JSON-RPC notification.
    pub fn notify(&self, method: &str) -> Result<(), String> {
        match self.post(&json!({"jsonrpc": "2.0", "method": method}))? {
            (202, ..) => Ok(()),
            (status, _, body) => Err(format!("HTTP {status}: {}", body.trim())),
        }
    }

    /// Calls `tool` with `args`.
    pub fn call(&mut self, tool: &str, args: Value) -> Called {
        let answer = self.request("tools/call", json!({"name": tool, "arguments": args}));
        let (text, is_error) = match answer {
            Ok(result) => (
                result
                    .pointer("/content/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                result.get("isError") == Some(&json!(true)),
            ),
            Err(error) => (error, true),
        };
        Called {
            tool: tool.to_string(),
            args,
            text,
            is_error,
        }
    }
}

fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(end) = body.windows(2).position(|w| w == b"\r\n") {
        let size = std::str::from_utf8(&body[..end])
            .ok()
            .and_then(|size| usize::from_str_radix(size.trim(), 16).ok());
        let rest = &body[end + 2..];
        let Some(size) = size.filter(|size| *size > 0 && rest.len() >= *size) else {
            break;
        };
        out.extend_from_slice(&rest[..size]);
        body = rest[size..].strip_prefix(b"\r\n").unwrap_or(&rest[size..]);
    }
    out
}

/// Whether a message asks this agent to hand its work off (`handoff`, `hand off`).
pub fn hands_off(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("handoff") || text.contains("hand off")
}

/// What a lead given the app's server does with a message that says `orchestrate` or asks for a handoff: [`hand_off`]
/// or [`orchestrate`]. `own` is this fake's agent. `report` hears each tool call once it is answered. Nothing when the
/// agent was not given the server (a session too deep for the tools, say).
pub fn act(server: Option<Server>, own: &str, text: &str, report: impl FnMut(Called)) {
    let Some(mut server) = server else { return };
    let Some(names) = handshake(&mut server) else { return };
    if hands_off(text) && names.iter().any(|name| name == "handoff_session") {
        hand_off(&mut server, own, text, report);
    } else if names.iter().any(|name| name == "start_session") {
        orchestrate(&mut server, own, text, report);
    }
}

/// Initializes and lists the tools: their names, or None when the server answers neither.
fn handshake(server: &mut Server) -> Option<Vec<String>> {
    let init = server.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake", "version": "1"}}),
    );
    if init.is_err() || server.notify("notifications/initialized").is_err() {
        return None;
    }
    let listed = server.request("tools/list", json!({})).ok()?;
    Some(
        listed
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(String::from))
            .collect(),
    )
}

/// `orchestrate [claude|codex] [count]`: starts `count` children (default 1, at most 4) of the named agent (default
/// `own`), then waits for them all.
fn orchestrate(server: &mut Server, own: &str, text: &str, mut report: impl FnMut(Called)) {
    let lower = text.to_lowercase();
    let mut words = lower
        .split("orchestrate")
        .nth(1)
        .unwrap_or_default()
        .split_whitespace()
        .peekable();
    let agent = match words.peek() {
        Some(&word @ ("claude" | "codex")) => {
            words.next();
            word.to_string()
        }
        _ => own.to_string(),
    };
    let count = words
        .next()
        .and_then(|word| word.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 4);
    let mut children = Vec::new();
    for n in 1..=count {
        let title = if count == 1 {
            "Builder".to_string()
        } else {
            format!("Builder {n}")
        };
        let started = server.call(
            "start_session",
            json!({"agent": agent, "prompt": "Report back. End with a Tests: line.", "title": title}),
        );
        let child = (!started.is_error)
            .then(|| serde_json::from_str::<Value>(&started.text).ok())
            .flatten()
            .and_then(|answer| answer.get("session_id").and_then(Value::as_str).map(String::from));
        report(started);
        children.extend(child);
    }
    if children.is_empty() {
        return;
    }
    report(server.call("wait_sessions", json!({"session_ids": children, "timeout_s": 60})));
}

/// A handoff to the other agent the message names (`codex` or `claude`), else to the other one of `own`. Its prompt
/// never repeats the person's words, so the new session does not hand off again.
fn hand_off(server: &mut Server, own: &str, text: &str, mut report: impl FnMut(Called)) {
    let other = if own == "claude" { "codex" } else { "claude" };
    let lower = text.to_lowercase();
    let agent = if lower.contains(other) || !lower.contains(own) {
        other
    } else {
        own
    };
    report(server.call(
        "handoff_session",
        json!({"agent": agent, "prompt": "Continue the work the last session left: the goal, what is done and what \
                is left are in this message. End with a Tests: line."}),
    ));
}
