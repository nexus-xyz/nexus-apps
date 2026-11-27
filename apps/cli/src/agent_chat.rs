//! `nexus agent ask` and `nexus agent chat`, as `agent-chat.ts` does them (Agent Interfaces S2.2): the
//! person's own agent, the same one as @nexus in Slack, through the agent API (S2.1) at `NEXUS_AGENTS_API_URL`.
//!
//! The CLI's own sign-in is the credential: the API asks Gateway who it is. The CLI never runs or confirms a write, as
//! the MCP `ask` tool never does: the API turns a write into a `pending_action` with a Nexus UI page, and the
//! CLI prints that link. Every failure has a machine-readable `code`.

use std::io::BufRead;
use std::time::Duration;

use serde_json::{Value, json};

use crate::VERSION;
use crate::catalog::Rows;
use crate::cli::Cmd;
use crate::error::{CliError, Result, error_text, usage};
use crate::json::{compact, pretty, s};
use crate::safe_text::safe_text;
use crate::style::{Style, usd};

pub const AGENT_CHAT_COMMANDS: Rows = &[
    (
        "ask \"<question>\" [--conversation <id>] [--model <id>]",
        "Ask Nexus, the same one as @nexus in Slack, with its memory and saved model; the answer streams, then its sources",
    ),
    (
        "chat [--conversation <id>] [--model <id>]",
        "Talk with Nexus in one conversation; type exit or press Ctrl-D to end",
    ),
];
pub const AGENT_CHAT_VERBS: [&str; 2] = ["ask", "chat"];
pub const AGENTS_API_URL_VAR: &str = "NEXUS_AGENTS_API_URL";
/// `on` asks through Gateway's `/v1/agent/ask`, at the CLI's own Gateway with its own sign-in, and not at
/// `NEXUS_AGENTS_API_URL`. Unset or anything else: the agent API directly, as before. Gateway's proxy is off in
/// production (`NEXUS_AGENT_PROXY`), so this stays off there too.
pub const VIA_GATEWAY_VAR: &str = "NEXUS_AGENT_VIA_GATEWAY";
pub const ASK_PATH: &str = "/agent/ask";
/// The ask through Gateway.
pub const GATEWAY_ASK_PATH: &str = "/v1/agent/ask";
/// Past the agent API's own 110 s deadline for an ask, so its 504 arrives first.
const ASK_DEADLINE: Duration = Duration::from_secs(115);
const RETRY: &str = "Try again in a moment.";

/// A failure of the agent API, or of reaching it.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: String,
    pub status: u16,
    pub message: String,
    pub hint: Option<String>,
}

fn failure(code: &str, status: u16, message: impl Into<String>, hint: Option<String>) -> Failure {
    Failure {
        code: code.to_string(),
        status,
        message: message.into(),
        hint,
    }
}

/// The agent API's base URL without a trailing slash; `None` when unset or not http(s).
pub fn agents_api_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return None;
    }
    reqwest::Url::parse(value).ok()?;
    Some(value.trim_end_matches('/').to_string())
}

/// Whether `value` (of [`VIA_GATEWAY_VAR`]) turns the Gateway path on: `on` only.
pub fn via_gateway(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("on"))
}

fn unreachable(base: &str, status: Option<u16>) -> Failure {
    let why = status.map_or_else(
        || "did not answer".to_string(),
        |status| format!("answered HTTP {status}, not as the agent API"),
    );
    failure(
        "agents_api_unreachable",
        502,
        format!("The agent API at {base} {why}."),
        Some(format!(
            "It may be off (NEXUS_AGENTS_API=on starts it) or at another address: check {AGENTS_API_URL_VAR}."
        )),
    )
}

/// The API's `{ error, detail }`, or Gateway's own `{ error: { code, message } }` (403 `not_a_person`, 502
/// `agents_api_unreachable`, 504 `upstream_timeout`), as a failure with the fix for its code.
fn refusal(status: u16, body: &Value, retry_after: Option<&str>) -> Failure {
    let gateway = body.get("error").filter(|error| error.is_object());
    let code = gateway
        .and_then(|error| s(error, "code"))
        .or_else(|| s(body, "error"))
        .unwrap_or("agents_api_error");
    let hint = match code {
        "credential_refused" => Some("Sign in again with `nexus auth login`.".to_string()),
        "not_a_person" => Some("Nexus answers a person. Sign in as yourself with `nexus auth login`.".to_string()),
        "memory_unavailable" | "busy" | "timeout" | "upstream_timeout" | "agents_api_unreachable" => {
            Some(RETRY.to_string())
        }
        _ if status == 429 => Some(format!(
            "Wait {} seconds and ask again.",
            retry_after.unwrap_or("a few")
        )),
        _ => None,
    };
    let detail = gateway
        .and_then(|error| s(error, "message"))
        .or_else(|| s(body, "detail"))
        .map_or_else(|| format!("The agent API answered HTTP {status}."), String::from);
    failure(code, status, detail, hint)
}

/// Server-sent events, a line at a time: a block ends at a blank line, and a comment line is skipped.
#[derive(Debug, Default)]
pub struct Events {
    event: String,
    data: Vec<String>,
}

impl Events {
    /// One line; answers the event its block makes when the line is blank. An unreadable event is skipped.
    pub fn line(&mut self, line: &str) -> Option<(String, Value)> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(name) = line.strip_prefix("event:") {
            self.event = name.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data.push(data.trim_start().to_string());
        }
        if !line.is_empty() {
            return None;
        }
        let event = std::mem::take(&mut self.event);
        let data = std::mem::take(&mut self.data);
        if data.is_empty() {
            return None;
        }
        let value = serde_json::from_str(&data.join("\n")).ok()?;
        Some((if event.is_empty() { "message".into() } else { event }, value))
    }
}

/// One ask at `base` and `path` (the agent API's [`ASK_PATH`], or Gateway's [`GATEWAY_ASK_PATH`]): streamed (each
/// `text` delta to `on_text`), or one JSON answer. Answers the API's payload.
pub fn ask(
    base: &str,
    path: &str,
    api_key: &str,
    body: Value,
    on_text: Option<&mut dyn FnMut(&str)>,
) -> std::result::Result<Value, Failure> {
    nexus_client::install_tls();
    let http = reqwest::blocking::Client::builder()
        .user_agent(crate::client::USER_AGENT)
        .timeout(ASK_DEADLINE)
        .build()
        .map_err(|_| unreachable(base, None))?;
    let mut body = body;
    body["stream"] = json!(on_text.is_some());
    let accept = ["application/json", "text/event-stream"][usize::from(on_text.is_some())];
    let response = http
        .post(format!("{base}{path}"))
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .header("accept", accept)
        .header("x-nexus-client", format!("cli/{VERSION}"))
        .header("idempotency-key", uuid::Uuid::new_v4().to_string())
        .body(body.to_string())
        .send()
        .map_err(|_| unreachable(base, None))?;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(String::from)
    };
    let kind = header("content-type").unwrap_or_default();
    let retry_after = header("retry-after");
    if kind.contains("text/event-stream") {
        return read_stream(response, on_text.unwrap_or(&mut |_| {}));
    }
    let text = response.text().unwrap_or_default();
    let parsed = if kind.contains("application/json") {
        serde_json::from_str::<Value>(&text).ok().filter(Value::is_object)
    } else {
        None
    };
    let Some(mut payload) = parsed else {
        return Err(unreachable(base, Some(status)));
    };
    if status >= 400 && !payload["messages"].is_array() {
        return Err(refusal(status, &payload, retry_after.as_deref()));
    }
    payload["status"] = json!(status);
    Ok(payload)
}

fn read_stream(
    response: reqwest::blocking::Response,
    on_text: &mut dyn FnMut(&str),
) -> std::result::Result<Value, Failure> {
    let mut events = Events::default();
    for line in std::io::BufReader::new(response).lines() {
        let Ok(line) = line else { break };
        let Some((event, data)) = events.line(&line) else {
            continue;
        };
        match event.as_str() {
            "text" => {
                if let Some(delta) = s(&data, "delta") {
                    on_text(delta);
                }
            }
            "error" => {
                let status = data["status"]
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .unwrap_or(502);
                return Err(refusal(status, &data, None));
            }
            "done" => return Ok(data),
            _ => {}
        }
    }
    Err(failure(
        "stream_cut",
        502,
        "The answer's stream ended before its last event.",
        Some(RETRY.into()),
    ))
}

/// The reply as terminal lines; `streamed`: the answer's text was already written. `true`: a refusal or an error.
pub fn reply_lines(style: &Style, payload: &Value, streamed: bool) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut failed = false;
    let say = |value: &Value, key: &str| safe_text(s(value, key).unwrap_or(""));
    let empty = Vec::new();
    for message in payload["messages"].as_array().unwrap_or(&empty) {
        match s(message, "kind").unwrap_or("") {
            "answer" => {
                if !streamed {
                    lines.push(say(message, "text"));
                }
                if message["aiNotice"] == json!(true) {
                    lines.push(style.dim("AI can make mistakes. Check important answers."));
                }
                let sources = message["sources"].as_array().unwrap_or(&empty);
                if !sources.is_empty() {
                    lines.push(String::new());
                    lines.push(style.bold("Sources"));
                    for (n, source) in sources.iter().enumerate() {
                        let at = format!("{} ({})", say(source, "url"), say(source, "kind"));
                        lines.push(format!("  [{}] {}  {}", n + 1, say(source, "title"), style.dim(&at)));
                    }
                }
                let cost = message["costUsd"].as_f64().map(usd).unwrap_or_default();
                let mut footer = vec![
                    say(message, "model"),
                    cost,
                    format!("conversation {}", say(payload, "conversation")),
                ];
                footer.retain(|part| !part.is_empty());
                lines.push(String::new());
                lines.push(style.dim(&footer.join(" · ")));
            }
            "pending_action" => {
                lines.push(format!("{} {}", style.warn, say(message, "detail")));
                lines.push(format!(
                    "  {} Approve it in Nexus UI: {}",
                    style.arrow,
                    say(message, "approveUrl")
                ));
                lines.push(style.dim("The CLI never runs or confirms a write."));
            }
            "refused" => {
                failed = true;
                lines.push(format!("{} {}", style.fail, say(message, "detail")));
                if s(message, "billingUrl").is_some() {
                    lines.push(format!("  {} {}", style.arrow, say(message, "billingUrl")));
                }
            }
            "error" => {
                failed = true;
                let request =
                    s(message, "requestId").map_or_else(String::new, |id| format!(", request {}", safe_text(id)));
                lines.push(format!(
                    "{} The agent failed (status {}){request}. {RETRY}",
                    style.fail,
                    message["status"].as_u64().unwrap_or(0)
                ));
            }
            // A command's reply with no model call (notice, insight, memories, forgotten): its text, link and notes.
            _ => {
                let notes = message["notes"].as_array().unwrap_or(&empty).iter();
                lines.extend(s(message, "text").map(safe_text));
                lines.extend(s(message, "url").map(|url| format!("  {} {}", style.arrow, safe_text(url))));
                lines.extend(notes.map(|note| format!("  {} {}", style.bullet, say(note, "text"))));
                lines.extend(message["count"].as_u64().map(|n| format!("Forgot {n} note(s).")));
            }
        }
    }
    (lines, failed)
}

pub fn agent_chat_command(rest: &[String], cmd: &Cmd) -> Result<i32> {
    let verb = rest.first().map(String::as_str).unwrap_or("");
    let question = rest[1..].join(" ").trim().to_string();
    if verb == "ask" && question.is_empty() {
        return usage(
            "Usage: nexus agent ask \"<question>\" [--conversation <id>] [--model <id>]",
            Some("Ask a question in quotes."),
        );
    }
    let (style, json, io) = (cmd.style, cmd.json, &cmd.ctx.io);
    let fail = |failure: Failure| {
        if json {
            let body = json!({ "error": { "code": failure.code, "status": failure.status, "message": failure.message, "hint": failure.hint } });
            io.err(&compact(&body));
        } else {
            let error = CliError {
                message: failure.message,
                hint: failure.hint,
                exit_code: 1,
                code: None,
            };
            io.err(&error_text(style, &error));
        }
        1
    };
    let through_gateway = via_gateway(cmd.ctx.var(VIA_GATEWAY_VAR).as_deref());
    let target = if through_gateway {
        Some((cmd.config.url.trim_end_matches('/').to_string(), GATEWAY_ASK_PATH))
    } else {
        agents_api_url(cmd.ctx.var(AGENTS_API_URL_VAR).as_deref()).map(|base| (base, ASK_PATH))
    };
    let Some((base, path)) = target else {
        return Ok(fail(failure(
            "agents_api_unconfigured",
            503,
            "Nexus cannot answer from this CLI: NEXUS_AGENTS_API_URL is not set.",
            Some(format!(
                "Set {AGENTS_API_URL_VAR} to the agent API's address, or ask @nexus in Slack."
            )),
        )));
    };
    let api_key = cmd.config.api_key.clone().unwrap_or_default();
    let model = cmd.args.flag_set("model");
    // One question; `chat` keeps the conversation the first answer names.
    let turn = |message: &str, conversation: &mut Option<String>| -> i32 {
        let mut body = json!({ "message": message });
        if let Some(id) = conversation.as_deref() {
            body["conversation"] = json!(id);
        }
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        let mut streamed = false;
        let mut write = |delta: &str| streamed = io.write(&safe_text(delta)) || streamed;
        let on_text: Option<&mut dyn FnMut(&str)> = if json { None } else { Some(&mut write) };
        let answered = ask(&base, path, &api_key, body, on_text);
        if streamed {
            io.write("\n");
        }
        let payload = match answered {
            Ok(payload) => payload,
            Err(error) => return fail(error),
        };
        if let Some(id) = s(&payload, "conversation").filter(|id| !id.is_empty()) {
            *conversation = Some(id.to_string());
        }
        if json {
            // `chat --json`: one JSON line per answer.
            io.out(&if verb == "chat" {
                compact(&payload)
            } else {
                pretty(&payload)
            });
            return i32::from(payload["status"].as_u64().unwrap_or(200) >= 400);
        }
        let (lines, failed) = reply_lines(style, &payload, streamed);
        cmd.lines(lines);
        i32::from(failed)
    };
    let mut conversation = cmd.args.flag_set("conversation").map(String::from);
    if verb == "ask" {
        return Ok(turn(&question, &mut conversation));
    }

    if !json {
        cmd.out(style.dim("Nexus, the same one as @nexus in Slack. Type exit or press Ctrl-D to end."));
    }
    loop {
        if !json {
            io.write(&format!("{} {} ", style.cyan("you"), style.arrow));
        }
        let Some(line) = io.read_line() else {
            if !json {
                io.write("\n");
            }
            return Ok(0);
        };
        let message = line.trim();
        if message == "exit" || message == "quit" {
            return Ok(0);
        }
        if !message.is_empty() {
            turn(message, &mut conversation);
        }
    }
}
