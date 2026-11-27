//! The agents' own transcripts as the workbench's events: Claude Code's `~/.claude/projects/<folder>/<id>.jsonl` and
//! Codex's `~/.codex/sessions/…/rollout-…-<id>.jsonl`, one JSON object a line. Each line's events depend only on the
//! lines before it, so a resumed conversation reads on from where the log stopped. What a viewer draws (prompts,
//! answers, thinking, tool calls and results, usage, turns) is kept; the agents' bookkeeping is not. Long text is cut,
//! so one event stays well under session sync's 256 KiB.

use std::collections::{HashMap, HashSet};

use nexus_workbench::event::{TurnStatus, WbEvent};
use serde_json::{Value, json};

/// The catalog's list prices per million tokens, input and output, by [`model_key`].
pub type Prices = HashMap<String, (f64, f64)>;
pub type Logged = Vec<(i64, WbEvent)>;

const MAX_TEXT: usize = 64 * 1024;
const MAX_OUTPUT: usize = 32 * 1024;

/// A model id as the catalog's prices are keyed: no `anthropic/`, dots as dashes, lower case (the desktop's).
pub fn model_key(id: &str) -> String {
    id.strip_prefix("anthropic/")
        .unwrap_or(id)
        .replace('.', "-")
        .to_lowercase()
}

/// One call at its model's list price: cache reads at a tenth of input, writes at 1.25 times (the desktop's estimate).
pub fn price(prices: &Prices, model: &str, tokens: [u64; 4]) -> f64 {
    let key = model_key(model);
    let found = prices.get(&key).copied().or_else(|| {
        prices
            .iter()
            .filter(|(id, _)| key.starts_with(id.as_str()))
            .max_by_key(|(id, _)| id.len())
            .map(|(_, price)| *price)
    });
    let Some((input, output)) = found else {
        return 0.0;
    };
    let [fresh, out, read, write] = tokens.map(|count| count as f64);
    ((fresh + read * 0.1 + write * 1.25) * input + out * output) / 1_000_000.0
}

/// `text` cut to at most `max` bytes at a character boundary, and whether it was cut.
fn cap(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &text[..end]), true)
}

/// A tool's input, unless it is too large to sync.
fn input(value: Value) -> Value {
    if value.to_string().len() <= MAX_TEXT {
        value
    } else {
        json!({ "note": "The input is too large to sync." })
    }
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn millis(timestamp: Option<&str>) -> Option<i64> {
    let at = time::OffsetDateTime::parse(timestamp?, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

struct Turn {
    id: String,
    started: i64,
    cost: f64,
}

/// The open turn, if any.
#[derive(Default)]
struct Turns {
    open: Option<Turn>,
}

impl Turns {
    fn start(&mut self, at: i64, id: String, out: &mut Logged) {
        out.push((at, WbEvent::TurnStart { turn_id: id.clone() }));
        self.open = Some(Turn {
            id,
            started: at,
            cost: 0.0,
        });
    }

    fn end(&mut self, at: i64, status: TurnStatus, cursor: Option<String>, out: &mut Logged) {
        let Some(turn) = self.open.take() else {
            return;
        };
        out.push((
            at,
            WbEvent::TurnEnd {
                turn_id: turn.id,
                status,
                cost_usd: Some(turn.cost),
                duration_ms: u64::try_from(at - turn.started).unwrap_or(0),
                error: None,
                cursor,
            },
        ));
    }

    fn spent(&mut self, usd: f64) {
        if let Some(turn) = &mut self.open {
            turn.cost += usd;
        }
    }
}

/// One agent's transcript, line by line.
pub trait Reader: Send {
    fn line(&mut self, line: &Value, now: i64, out: &mut Logged);
    /// The lines read so far are all there is for now: settle what waited for a next line.
    fn settle(&mut self, _out: &mut Logged) {}
    /// The agent exited: a turn still open ends with `status`.
    fn finish(&mut self, at: i64, status: TurnStatus, out: &mut Logged);
}

/// Claude Code's transcript. Each content block of a message is its own line, and every line of a message carries the
/// message's usage and stop reason: usage counts once per message, and a turn ends after the last line of the message
/// that ended it.
pub struct Claude {
    turns: Turns,
    counted: HashSet<String>,
    /// The message that ended the turn: its id, its time and its line's `uuid` (where a revert resumes).
    ending: Option<(String, i64, Option<String>)>,
    prices: Prices,
}

impl Claude {
    pub fn new(prices: Prices) -> Claude {
        Claude {
            turns: Turns::default(),
            counted: HashSet::new(),
            ending: None,
            prices,
        }
    }
}

/// Text Claude Code writes as the person's for its own commands and notes, not a prompt.
fn claude_local(text: &str) -> bool {
    [
        "<command-name>",
        "<command-message>",
        "<command-args>",
        "<local-command-",
        "<bash-input>",
        "<bash-stdout>",
        "<bash-stderr>",
        "Caveat: The messages below",
    ]
    .iter()
    .any(|start| text.trim_start().starts_with(start))
}

fn tool_output(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| match str_of(block, "type") {
                Some("text") => str_of(block, "text").map(String::from),
                Some("image") => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl Reader for Claude {
    fn line(&mut self, line: &Value, now: i64, out: &mut Logged) {
        if line.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let at = millis(str_of(line, "timestamp")).unwrap_or(now);
        let uuid = str_of(line, "uuid").map(String::from);
        let message = line.get("message").unwrap_or(&Value::Null);
        match str_of(line, "type") {
            Some("user") => {
                if line.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    return;
                }
                let mut prompt = Vec::new();
                match message.get("content") {
                    Some(Value::String(text)) => prompt.push(text.clone()),
                    Some(Value::Array(blocks)) => {
                        for block in blocks {
                            match str_of(block, "type") {
                                Some("tool_result") => {
                                    let (output, truncated) = cap(&tool_output(block.get("content")), MAX_OUTPUT);
                                    out.push((
                                        at,
                                        WbEvent::ToolResult {
                                            id: str_of(block, "tool_use_id").unwrap_or_default().to_string(),
                                            output,
                                            images: vec![],
                                            is_error: block.get("is_error").and_then(Value::as_bool) == Some(true),
                                            truncated,
                                            parent: None,
                                        },
                                    ));
                                }
                                Some("text") => prompt.extend(str_of(block, "text").map(String::from)),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                let text = prompt.join("\n");
                if text.trim().is_empty() || claude_local(&text) {
                    return;
                }
                self.settle(out);
                if text.trim_start().starts_with("[Request interrupted by user") {
                    self.turns.end(at, TurnStatus::Interrupted, uuid, out);
                    return;
                }
                // A prompt while a turn is open: that turn stopped without its answer.
                self.turns.end(at, TurnStatus::Interrupted, None, out);
                let (text, _) = cap(&text, MAX_TEXT);
                out.push((
                    at,
                    WbEvent::User {
                        text,
                        images: vec![],
                        from: None,
                    },
                ));
                let id = uuid.unwrap_or_else(|| format!("turn-{at}"));
                self.turns.start(at, id, out);
            }
            Some("assistant") => {
                let id = str_of(message, "id").unwrap_or_default().to_string();
                if self.ending.as_ref().is_some_and(|(ending, _, _)| *ending != id) {
                    self.settle(out);
                }
                let line_id = uuid.clone().unwrap_or_else(|| format!("{id}-{at}"));
                let blocks = message.get("content").and_then(Value::as_array);
                for (index, block) in blocks.into_iter().flatten().enumerate() {
                    let block_id = format!("{line_id}:{index}");
                    match str_of(block, "type") {
                        Some("text") => {
                            let (text, _) = cap(str_of(block, "text").unwrap_or_default(), MAX_TEXT);
                            out.push((
                                at,
                                WbEvent::Text {
                                    id: block_id,
                                    text,
                                    parent: None,
                                },
                            ));
                        }
                        Some("thinking") => {
                            let thinking = str_of(block, "thinking").unwrap_or_default();
                            if !thinking.trim().is_empty() {
                                let (text, _) = cap(thinking, MAX_TEXT);
                                out.push((
                                    at,
                                    WbEvent::Thinking {
                                        id: block_id,
                                        text,
                                        parent: None,
                                    },
                                ));
                            }
                        }
                        Some("tool_use") => out.push((
                            at,
                            WbEvent::Tool {
                                id: str_of(block, "id").unwrap_or_default().to_string(),
                                name: str_of(block, "name").unwrap_or_default().to_string(),
                                input: input(block.get("input").cloned().unwrap_or(Value::Null)),
                                parent: None,
                            },
                        )),
                        _ => {}
                    }
                }
                let model = str_of(message, "model").unwrap_or_default();
                if let Some(usage) = message.get("usage").filter(|usage| usage.is_object())
                    && !model.is_empty()
                    && model != "<synthetic>"
                    && self.counted.insert(id.clone())
                {
                    let tokens = [
                        count(usage, "input_tokens"),
                        count(usage, "output_tokens"),
                        count(usage, "cache_read_input_tokens"),
                        count(usage, "cache_creation_input_tokens"),
                    ];
                    let cost = price(&self.prices, model, [tokens[0], tokens[1], tokens[2], tokens[3]]);
                    self.turns.spent(cost);
                    out.push((
                        at,
                        WbEvent::Usage {
                            model: model.to_string(),
                            input_tokens: tokens[0],
                            output_tokens: tokens[1],
                            cache_read_tokens: tokens[2],
                            cache_write_tokens: tokens[3],
                            cost_usd: cost,
                            parent: None,
                        },
                    ));
                }
                let stopped = matches!(
                    str_of(message, "stop_reason"),
                    Some("end_turn" | "stop_sequence" | "max_tokens" | "refusal")
                );
                if stopped && self.turns.open.is_some() {
                    self.ending = Some((id, at, uuid));
                }
            }
            Some("system") if str_of(line, "subtype") == Some("compact_boundary") => out.push((
                at,
                WbEvent::Notice {
                    text: "Claude Code compacted the conversation to fit its context window.".into(),
                },
            )),
            _ => {}
        }
    }

    fn settle(&mut self, out: &mut Logged) {
        if let Some((_, at, cursor)) = self.ending.take() {
            self.turns.end(at, TurnStatus::Completed, cursor, out);
        }
    }

    fn finish(&mut self, at: i64, status: TurnStatus, out: &mut Logged) {
        self.settle(out);
        self.turns.end(at, status, None, out);
    }
}

/// Codex's rollout. Its `event_msg` lines carry the person's prompts, the turns and the token counts; its
/// `response_item` lines carry the model's messages, reasoning summaries and tool calls. (A `response_item` user message
/// is also the context Codex injects, so prompts come only from `user_message`.)
pub struct Codex {
    turns: Turns,
    model: String,
    items: u64,
    prices: Prices,
}

impl Codex {
    pub fn new(prices: Prices, model: Option<&str>) -> Codex {
        Codex {
            turns: Turns::default(),
            model: model.unwrap_or_default().to_string(),
            items: 0,
            prices,
        }
    }

    fn next_id(&mut self, kind: &str) -> String {
        self.items += 1;
        format!("codex-{kind}-{}", self.items)
    }
}

fn texts(content: Option<&Value>, kinds: &[&str]) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| str_of(block, "type").is_some_and(|kind| kinds.contains(&kind)))
        .filter_map(|block| str_of(block, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Reader for Codex {
    fn line(&mut self, line: &Value, now: i64, out: &mut Logged) {
        let at = millis(str_of(line, "timestamp")).unwrap_or(now);
        let payload = line.get("payload").unwrap_or(&Value::Null);
        let kind = str_of(payload, "type");
        match (str_of(line, "type"), kind) {
            (Some("turn_context"), _) => {
                if let Some(model) = str_of(payload, "model").filter(|model| !model.is_empty()) {
                    self.model = model.to_string();
                }
            }
            (Some("event_msg"), Some("user_message")) => {
                let text = str_of(payload, "message").unwrap_or_default();
                if text.trim().is_empty() {
                    return;
                }
                self.turns.end(at, TurnStatus::Interrupted, None, out);
                let (text, _) = cap(text, MAX_TEXT);
                out.push((
                    at,
                    WbEvent::User {
                        text,
                        images: vec![],
                        from: None,
                    },
                ));
                let id = self.next_id("turn");
                self.turns.start(at, id, out);
            }
            (Some("event_msg"), Some("task_complete")) => {
                let cursor = str_of(payload, "turn_id").map(String::from);
                self.turns.end(at, TurnStatus::Completed, cursor, out);
            }
            (Some("event_msg"), Some("turn_aborted")) => self.turns.end(at, TurnStatus::Interrupted, None, out),
            (Some("event_msg"), Some("error")) => out.push((
                at,
                WbEvent::Error {
                    text: cap(
                        str_of(payload, "message").unwrap_or("Codex reported an error."),
                        MAX_TEXT,
                    )
                    .0,
                    detail: None,
                },
            )),
            (Some("event_msg"), Some("token_count")) => {
                let Some(last) = payload
                    .pointer("/info/last_token_usage")
                    .filter(|last| last.is_object())
                else {
                    return;
                };
                if self.model.is_empty() {
                    return;
                }
                // Codex counts cached input inside its input; the log counts it apart, as Claude Code does.
                let cached = count(last, "cached_input_tokens");
                let tokens = [
                    count(last, "input_tokens").saturating_sub(cached),
                    count(last, "output_tokens"),
                    cached,
                    0,
                ];
                let cost = price(&self.prices, &self.model, tokens);
                self.turns.spent(cost);
                out.push((
                    at,
                    WbEvent::Usage {
                        model: self.model.clone(),
                        input_tokens: tokens[0],
                        output_tokens: tokens[1],
                        cache_read_tokens: tokens[2],
                        cache_write_tokens: 0,
                        cost_usd: cost,
                        parent: None,
                    },
                ));
            }
            (Some("response_item"), Some("message")) if str_of(payload, "role") == Some("assistant") => {
                let text = texts(payload.get("content"), &["output_text"]);
                if !text.trim().is_empty() {
                    let id = self.next_id("text");
                    out.push((
                        at,
                        WbEvent::Text {
                            id,
                            text: cap(&text, MAX_TEXT).0,
                            parent: None,
                        },
                    ));
                }
            }
            (Some("response_item"), Some("reasoning")) => {
                let text = texts(payload.get("summary"), &["summary_text"]);
                if !text.trim().is_empty() {
                    let id = self.next_id("thinking");
                    out.push((
                        at,
                        WbEvent::Thinking {
                            id,
                            text: cap(&text, MAX_TEXT).0,
                            parent: None,
                        },
                    ));
                }
            }
            (Some("response_item"), Some(call @ ("function_call" | "custom_tool_call" | "local_shell_call"))) => {
                let arguments = match call {
                    "function_call" => {
                        let raw = str_of(payload, "arguments").unwrap_or_default();
                        serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
                    }
                    "custom_tool_call" => json!({ "input": payload.get("input") }),
                    _ => payload.get("action").cloned().unwrap_or(Value::Null),
                };
                let name = str_of(payload, "name").unwrap_or("shell").to_string();
                out.push((
                    at,
                    WbEvent::Tool {
                        id: str_of(payload, "call_id").unwrap_or_default().to_string(),
                        name,
                        input: input(arguments),
                        parent: None,
                    },
                ));
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                let output = match payload.get("output") {
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => str_of(other, "content")
                        .map(String::from)
                        .unwrap_or_else(|| other.to_string()),
                    None => String::new(),
                };
                let failed = payload.pointer("/output/success").and_then(Value::as_bool) == Some(false);
                let (output, truncated) = cap(&output, MAX_OUTPUT);
                out.push((
                    at,
                    WbEvent::ToolResult {
                        id: str_of(payload, "call_id").unwrap_or_default().to_string(),
                        output,
                        images: vec![],
                        is_error: failed,
                        truncated,
                        parent: None,
                    },
                ));
            }
            _ => {}
        }
    }

    fn finish(&mut self, at: i64, status: TurnStatus, out: &mut Logged) {
        self.turns.end(at, status, None, out);
    }
}

#[cfg(test)]
#[path = "readers_tests.rs"]
mod tests;
