//! Agent mode (⇧tab): the model works in the workspace with tools, as Codex does. Each step is one call to Gateway's
//! `/v1/chat/completions` with the conversation and the tools (`tools.rs`), so every step is reserved, metered and
//! checked by policy like any other call. The model answers with text, tool calls or both; the tools run here, on
//! this machine, after the person approves the ones that change something, and their results go back in the next
//! step. A turn ends when the model answers without a tool call, when the person stops it (esc), or after
//! `MAX_STEPS`.
//!
//! The conversation is kept here, not in Nexus's chat sessions: `/new` starts it again. Until Gateway caches chat
//! turns, each step on an Anthropic model pays for the whole conversation again.
//!
//! What the agent knows of Nexus is in two parts. `prompt.rs` is what every step tells it: who it is, a
//! map of Nexus, and what this session has. `knowledge.rs` is what it reads when it needs it, with `nexus_docs`.

pub mod cells;
pub mod knowledge;
pub mod mcp;
pub mod prompt;
pub mod sessions;
pub mod stdio;
pub mod tools;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::cli::{cli_client, left_text};
use crate::error::{CliError, Error, to_cli_error};

use super::turn::{Gateway, TurnEvent};
use prompt::Host;
use tools::{Ask, Decision};

/// The most model calls one turn makes before it asks the person to go on.
const MAX_STEPS: usize = 40;
/// Tool arguments can contain whole files; the model's short chat default can cut their JSON in half.
const MAX_OUTPUT_TOKENS: i64 = 16_384;

/// How the session steers a running turn: the person's answers to what the agent asks, and esc.
pub struct Control {
    pub decisions: Receiver<Decision>,
    pub stop: Arc<AtomicBool>,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub model: Option<String>,
    pub cost_usd: f64,
    pub steps: usize,
    pub stopped: bool,
}

/// A tool call as it streams in: its id, its name, and its arguments so far.
#[derive(Debug, Default, Clone)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

/// What every turn of the agent works with: where Gateway is, the model, the folder, and the conversation so far.
pub struct Work {
    pub gateway: Gateway,
    pub model: String,
    pub workspace: PathBuf,
    /// Where the agent runs: the terminal session, or the program that drives `nexus __agent`.
    pub host: Host,
    pub memory: Option<nexus_agent_memory::Store>,
    pub history: Arc<Mutex<Vec<Value>>>,
    /// The MCP servers this run was given (`nexus __agent --mcp-config`), whose tools join the agent's own.
    pub mcp: Option<Arc<mcp::Mcp>>,
}

/// The agent's tools, its knowledge base's and its MCP servers', as `/v1/chat/completions` takes them.
pub(crate) fn specs(mcp: Option<&mcp::Mcp>) -> Value {
    let mut specs = tools::specs();
    if let Some(list) = specs.as_array_mut() {
        if shared_terminal(mcp) {
            list.retain(|spec| spec["function"]["name"] != "run_command");
        }
        list.push(knowledge::spec());
        if let Some(mcp) = mcp {
            list.extend(mcp.specs());
        }
    }
    specs
}

/// A desktop with shared command execution owns shell work, including sub-agent commands.
fn shared_terminal(mcp: Option<&mcp::Mcp>) -> bool {
    mcp.is_some_and(|mcp| mcp.has("mcp__nexus_app__terminal_run"))
}

fn run_builtin(
    name: &str,
    args: &str,
    workspace: &Path,
    ask: &mut dyn FnMut(Ask) -> Decision,
    shared: bool,
) -> (String, tools::Action) {
    if name == "run_command" && shared {
        let error = "Native command tools are disabled here. Use mcp__nexus_app__terminal_run so the command enters desktop history.".to_string();
        return (
            error.clone(),
            tools::Action::Failed {
                what: "run_command".into(),
                error,
            },
        );
    }
    tools::run(name, args, workspace, ask)
}

/// What the agent is told before the conversation (`prompt.rs`), from where it runs and the servers it was given.
pub fn system_prompt(
    workspace: &Path,
    host: Host,
    mcp: Option<&mcp::Mcp>,
    memory: Option<&nexus_agent_memory::Store>,
) -> String {
    let servers = mcp.map(mcp::Mcp::servers).unwrap_or_default();
    let mut prompt = prompt::system_prompt(&prompt::Setting {
        workspace,
        host,
        servers: &servers,
    });
    if let Some(store) = memory {
        match store.instructions(workspace, None) {
            Ok(memory) if !memory.is_empty() => {
                prompt.push_str("\n\n");
                prompt.push_str(&memory);
            }
            Err(error) => eprintln!("Could not load shared memory: {error}"),
            _ => {}
        }
    }
    prompt
}

/// A call to an MCP server's tool, once the person allows it.
fn call_mcp(
    mcp: &mcp::Mcp,
    name: &str,
    arguments: &str,
    ask: &mut dyn FnMut(Ask) -> Decision,
) -> (String, tools::Action) {
    let input: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let shown = input.to_string();
    if ask(Ask::Tool {
        name: name.to_string(),
        input: shown.clone(),
    }) == Decision::Deny
    {
        return tools::declined(&format!("call {name}"));
    }
    let (output, error) = match mcp.call(name, &input) {
        Ok(output) => (output, false),
        Err(error) => (error, true),
    };
    let result = if error {
        format!("Error: {output}")
    } else {
        output.clone()
    };
    (
        result,
        tools::Action::Called {
            name: name.to_string(),
            input: shown,
            output,
            error,
        },
    )
}

/// Most of a subagent's steps, before it must answer with what it has.
const MAX_SUBAGENT_STEPS: usize = 30;

/// A subagent's work, as the session shows it: it started, ran a tool, or finished with its answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// `call`: the `agent` tool call that started it, which its transcript is kept under.
    Started {
        description: String,
        prompt: String,
        call: String,
    },
    Tool(tools::Action),
    /// `messages`: its own conversation, for its transcript.
    Finished {
        result: String,
        error: bool,
        messages: Vec<Value>,
    },
}

/// What every agent of a turn shares: the session's own and its subagents.
struct Run<'a> {
    client: &'a crate::client::Client,
    url: &'a str,
    model: &'a str,
    workspace: &'a Path,
    host: Host,
    memory: Option<&'a nexus_agent_memory::Store>,
    mcp: Option<&'a mcp::Mcp>,
    control: &'a Control,
    send: &'a dyn Fn(TurnEvent),
    /// Subagents started so far, for their ids.
    subagents: std::cell::Cell<u64>,
    /// The requested output allowance, lowered when Gateway reports a smaller limit before inference.
    output_tokens: std::cell::Cell<i64>,
}

impl Run<'_> {
    fn stopped(&self) -> bool {
        self.control.stop.load(Ordering::Relaxed)
    }

    fn completion(&self, mut body: Value, on_chunk: &mut dyn FnMut(&Value) -> bool) -> Result<(), CliError> {
        body["max_tokens"] = json!(self.output_tokens.get());
        let result = self.client.stream_completion(body.clone(), on_chunk);
        // This 400 happens before inference. Retry it once with the reported deployment or model limit.
        if let Err(Error::Core(error)) = &result
            && error.status == 400
            && error.code.as_deref() == Some("output_over_limit")
            && let Some(limit) = error.details.as_ref().and_then(|details| details["limit"].as_i64())
            && limit > 0
            && limit < self.output_tokens.get()
        {
            self.output_tokens.set(limit);
            body["max_tokens"] = json!(limit);
            return self
                .client
                .stream_completion(body, on_chunk)
                .map_err(|error| to_cli_error(&error, self.url));
        }
        result.map_err(|error| to_cli_error(&error, self.url))
    }

    /// One agent's steps on `history` until it answers without a tool call: the session's own (`task` is `None`), or
    /// subagent `task`'s, whose text and tools go to the session as that task's. Answers its last text.
    fn steps(
        &self,
        history: &Mutex<Vec<Value>>,
        task: Option<&str>,
        summary: &mut Summary,
    ) -> Result<String, CliError> {
        let send = self.send;
        let push = |message: Value| {
            if let Ok(mut history) = history.lock() {
                history.push(message);
            }
        };
        let told = system_prompt(self.workspace, self.host, self.mcp, self.memory);
        let system = json!({ "role": "system", "content": match task {
            None => told,
            Some(_) => format!("{told}\n\n{SUBAGENT_BRIEF}"),
        } });
        let mut tools = specs(self.mcp);
        if task.is_none()
            && let Some(list) = tools.as_array_mut()
        {
            list.push(subagent_spec());
        }
        let most = if task.is_none() { MAX_STEPS } else { MAX_SUBAGENT_STEPS };
        for _ in 0..most {
            if task.is_none() {
                summary.steps += 1;
            }
            let mut messages = vec![system.clone()];
            messages.extend(
                history
                    .lock()
                    .map(|history| model_history(&history))
                    .unwrap_or_default(),
            );
            let body = json!({ "model": self.model, "messages": messages, "tools": tools });
            let mut text = String::new();
            let mut calls: Vec<Call> = Vec::new();
            let mut finish_reason = None;
            let spent_before = summary.cost_usd;
            self.completion(body, &mut |chunk| {
                if self.stopped() {
                    return false;
                }
                if let Some(model) = chunk.get("model").and_then(Value::as_str).filter(|m| !m.is_empty()) {
                    summary.model = Some(model.to_string());
                }
                if let Some(cost) = cost_of(chunk) {
                    summary.cost_usd += cost;
                }
                if let Some(reason) = chunk.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                    finish_reason = Some(reason.to_string());
                }
                let Some(delta) = chunk.pointer("/choices/0/delta") else {
                    return true;
                };
                if let Some(content) = delta.get("content").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                    text.push_str(content);
                    // A subagent's text is its own: only its answer reaches the session.
                    if task.is_none() {
                        send(TurnEvent::Delta(content.to_string()));
                    }
                }
                for part in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let index = part.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    if calls.len() <= index {
                        calls.resize(index + 1, Call::default());
                    }
                    let call = &mut calls[index];
                    if let Some(id) = part.get("id").and_then(Value::as_str) {
                        call.id = id.to_string();
                    }
                    if let Some(name) = part.pointer("/function/name").and_then(Value::as_str) {
                        call.name.push_str(name);
                    }
                    if let Some(arguments) = part.pointer("/function/arguments").and_then(Value::as_str) {
                        call.arguments.push_str(arguments);
                    }
                }
                true
            })?;
            calls.retain(|call| !call.name.is_empty());
            send(TurnEvent::Spent(summary.cost_usd - spent_before));
            let mut assistant =
                json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
            if self.stopped() {
                // A stopped step keeps what was said, without the tool calls it did not finish.
                if !text.is_empty() {
                    push(assistant);
                }
                summary.stopped = true;
                return Ok(text);
            }
            let incomplete = if finish_reason.is_none() {
                Some(CliError::hint(
                    "The model's stream ended before finishing. No tools from this step ran.",
                    "Send a prompt to try again.",
                ))
            } else if finish_reason.as_deref() == Some("length") {
                Some(CliError::hint(
                    "The model reached its output limit before finishing. No tools from this step ran.",
                    "Ask it to split the work into smaller edits.",
                ))
            } else {
                // Some providers finish a zero-argument call with empty arguments instead of an empty object.
                for call in &mut calls {
                    if call.arguments.trim().is_empty() {
                        call.arguments = "{}".into();
                    }
                }
                calls
                    .iter()
                    .find(|call| serde_json::from_str::<serde_json::Map<String, Value>>(&call.arguments).is_err())
                    .map(|call| {
                        CliError::hint(
                            format!(
                                "The model returned invalid arguments for {}. No tools from this step ran.",
                                call.name
                            ),
                            "Try again, or ask for a smaller edit.",
                        )
                    })
            };
            if let Some(error) = incomplete {
                if !text.trim().is_empty() {
                    push(assistant);
                }
                return Err(error);
            }
            if text.trim().is_empty() && calls.is_empty() {
                return Err(CliError::hint(
                    "The model ended without an answer.",
                    "Send a prompt to try again.",
                ));
            }
            if !calls.is_empty() {
                assistant["tool_calls"] = json!(
                    calls
                        .iter()
                        .map(|call| json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        }))
                        .collect::<Vec<_>>()
                );
            }
            push(assistant);
            if calls.is_empty() {
                return Ok(text);
            }
            // Every tool call gets an answer, so the next step is a valid conversation even after a stop.
            for call in &calls {
                let result = if self.stopped() {
                    "Not run: the person stopped the agent.".to_string()
                } else if call.name == SUBAGENT_TOOL && task.is_none() {
                    self.subagent(&call.id, &call.arguments, summary)
                } else {
                    let mut ask = |ask: Ask| {
                        send(TurnEvent::Approve(ask));
                        wait_for(self.control)
                    };
                    let (result, action) = match self.mcp.filter(|mcp| mcp.has(&call.name)) {
                        Some(mcp) => call_mcp(mcp, &call.name, &call.arguments, &mut ask),
                        // The knowledge base reads only, so it never asks; `status` is Gateway's answer, now.
                        None if call.name == knowledge::TOOL => {
                            knowledge::run(&call.arguments, &|| knowledge::status(self.client, self.url, self.host))
                        }
                        None => run_builtin(
                            &call.name,
                            &call.arguments,
                            self.workspace,
                            &mut ask,
                            shared_terminal(self.mcp),
                        ),
                    };
                    send(match task {
                        None => TurnEvent::Tool(action),
                        Some(id) => TurnEvent::Task {
                            id: id.to_string(),
                            task: Task::Tool(action),
                        },
                    });
                    result
                };
                push(json!({ "role": "tool", "tool_call_id": call.id, "content": result }));
            }
            if self.stopped() {
                summary.stopped = true;
                return Ok(text);
            }
        }
        Err(match task {
            None => CliError::hint(
                format!("The agent stopped after {MAX_STEPS} steps."),
                "Send a prompt to let it go on.",
            ),
            Some(_) => CliError::new(format!("The subagent stopped after {MAX_SUBAGENT_STEPS} steps.")),
        })
    }

    /// The `agent` tool: a subagent does the task in a conversation of its own, and its answer is the tool's result.
    fn subagent(&self, call: &str, arguments: &str, summary: &mut Summary) -> String {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let (description, prompt) = (text("description"), text("prompt"));
        if prompt.is_empty() {
            return "Error: the agent tool needs a prompt: the whole task.".into();
        }
        self.subagents.set(self.subagents.get() + 1);
        let id = format!("agent-{}", self.subagents.get());
        let event = |task: Task| (self.send)(TurnEvent::Task { id: id.clone(), task });
        event(Task::Started {
            description: if description.is_empty() {
                first_line(&prompt)
            } else {
                description
            },
            prompt: prompt.clone(),
            call: call.to_string(),
        });
        let history = Mutex::new(vec![json!({ "role": "user", "content": prompt })]);
        let (result, error) = match self.steps(&history, Some(&id), summary) {
            Ok(answer) if answer.trim().is_empty() => ("The subagent ended without an answer.".to_string(), true),
            Ok(answer) => (answer, false),
            Err(error) => (error.message, true),
        };
        event(Task::Finished {
            result: result.clone(),
            error,
            messages: history.into_inner().unwrap_or_default(),
        });
        if error { format!("Error: {result}") } else { result }
    }
}

/// Skip empty assistant replies from older sessions when building a model request.
/// Keep saved messages unchanged so desktop rewind offsets still refer to the same boundaries.
pub(super) fn model_history(history: &[Value]) -> Vec<Value> {
    history
        .iter()
        .filter(|message| {
            message["role"] != "assistant"
                || message["content"].as_str().is_some_and(|text| !text.trim().is_empty())
                || message["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty())
        })
        .cloned()
        .collect()
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").chars().take(60).collect()
}

/// The tool that starts a subagent.
pub const SUBAGENT_TOOL: &str = "agent";

/// What a subagent is told on top of the agent's own instructions.
const SUBAGENT_BRIEF: &str = "You are a subagent. Another agent gave you the task in the first message and sees only \
your final answer. Do the task with your tools, then answer with a short report: what you found or changed, with \
paths, and anything left undone.";

fn subagent_spec() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": SUBAGENT_TOOL,
            "description": "Hand a self-contained part of the task to a subagent. It works in this workspace with your \
                            tools, in a conversation of its own, and answers with a short report, which is all you see. \
                            Use it for searches and investigations that would fill your context, or for separate \
                            pieces of work. Its prompt must say everything it needs: it can't see this conversation.",
            "parameters": {
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "A few words on what it does." },
                    "prompt": { "type": "string", "description": "The whole task." },
                },
                "required": ["description", "prompt"],
            },
        }
    })
}

/// Run turn `turn` on its own thread: `prompt` joins the conversation, and the agent works until it is done.
pub fn spawn(
    work: Work,
    prompt: String,
    images: Vec<Value>,
    turn: u64,
    events: Sender<(u64, TurnEvent)>,
    control: Control,
) {
    std::thread::spawn(move || {
        let Work {
            gateway,
            model,
            workspace,
            host,
            memory,
            history,
            mcp,
        } = work;
        let send = |event: TurnEvent| {
            let _ = events.send((turn, event));
        };
        let client = cli_client(&gateway.url, Some(&gateway.api_key));
        if let Ok(mut history) = history.lock() {
            history.push(json!({ "role": "user", "content": user_content(&prompt, &images) }));
        }
        let mut summary = Summary {
            model: None,
            cost_usd: 0.0,
            steps: 0,
            stopped: false,
        };
        let run = Run {
            client: &client,
            url: &gateway.url,
            model: &model,
            workspace: &workspace,
            host,
            memory: memory.as_ref(),
            mcp: mcp.as_deref(),
            control: &control,
            send: &send,
            subagents: std::cell::Cell::new(0),
            output_tokens: std::cell::Cell::new(MAX_OUTPUT_TOKENS),
        };
        let result = run.steps(&history, None, &mut summary);
        send(TurnEvent::Finished(result.map(|_| summary)));
        if let Ok(spend) = client.get("/v1/spend") {
            send(TurnEvent::Budget(left_text(&spend)));
        }
    });
}

/// `/compact`: the model summarizes the conversation, and the summary takes its place, so each later step sends far
/// less. Answers the conversation's size before and after (characters) and what the summary cost.
pub fn compact(work: &Work) -> Result<(usize, usize, f64), CliError> {
    let history = work.history.lock().map(|history| history.clone()).unwrap_or_default();
    if history.is_empty() {
        return Err(CliError::new(
            "The agent's conversation is empty: there is nothing to compact.",
        ));
    }
    let size = |messages: &[Value]| serde_json::to_string(messages).map_or(0, |text| text.len());
    let before = size(&history);
    let told = system_prompt(&work.workspace, work.host, work.mcp.as_deref(), work.memory.as_ref());
    let mut messages = vec![json!({ "role": "system", "content": told })];
    messages.extend(model_history(&history));
    messages.push(json!({
        "role": "user",
        "content": "Summarize our conversation so far for yourself, to go on from it: the task, what you found, what \
                    you changed (files and how), what is left, and anything the person asked for. Be complete but \
                    short. Reply with the summary only.",
    }));
    let body = json!({ "model": work.model, "messages": messages, "tools": specs(work.mcp.as_deref()), "tool_choice": "none" });
    let client = cli_client(&work.gateway.url, Some(&work.gateway.api_key));
    let answer = client
        .post("/v1/chat/completions", body)
        .map_err(|error| to_cli_error(&error, &work.gateway.url))?;
    let summary = answer
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .ok_or_else(|| CliError::new("The model gave no summary; the conversation is unchanged."))?;
    let compacted = vec![
        json!({ "role": "user", "content": format!("A summary of our conversation so far:\n\n{summary}") }),
        json!({ "role": "assistant", "content": "Understood. I'll go on from this summary." }),
    ];
    let after = size(&compacted);
    if let Ok(mut history) = work.history.lock() {
        *history = compacted;
    }
    let cost = cost_of(&answer).unwrap_or(0.0);
    Ok((before, after, cost))
}

/// What a call cost, from its `nexus` object: `/v1/chat/completions` names it `cost_usd` (`ChatNexus`), unlike the chat
/// sessions' `costUsd`.
pub(crate) fn cost_of(answer: &Value) -> Option<f64> {
    let nexus = answer.get("nexus")?;
    nexus
        .get("cost_usd")
        .or_else(|| nexus.get("costUsd"))
        .and_then(Value::as_f64)
}

/// The prompt, and its pasted images as OpenAI's `image_url` parts (data URLs), which Gateway passes to every provider.
fn user_content(prompt: &str, images: &[Value]) -> Value {
    if images.is_empty() {
        return json!(prompt);
    }
    let mut parts = vec![json!({ "type": "text", "text": prompt })];
    for image in images {
        let name = image
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("image.png")
            .to_ascii_lowercase();
        let media = match name.rsplit('.').next() {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "image/png",
        };
        let data = image.get("data").and_then(Value::as_str).unwrap_or("");
        parts.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{media};base64,{data}") } }));
    }
    json!(parts)
}

/// The person's answer, or no when they stop the turn.
fn wait_for(control: &Control) -> Decision {
    loop {
        match control.decisions.recv_timeout(Duration::from_millis(100)) {
            Ok(decision) => return decision,
            Err(RecvTimeoutError::Timeout) if !control.stop.load(Ordering::Relaxed) => {}
            Err(_) => return Decision::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cost_gateway_sends() {
        assert_eq!(cost_of(&json!({ "nexus": { "cost_usd": 0.0127 } })), Some(0.0127));
        assert_eq!(cost_of(&json!({ "nexus": { "costUsd": 0.5 } })), Some(0.5));
        assert_eq!(cost_of(&json!({ "choices": [] })), None);
    }

    /// The model is offered the knowledge base beside the agent's own tools, whatever servers the run has.
    #[test]
    fn the_tools_offered_include_the_knowledge_base() {
        let offered = specs(None);
        let names: Vec<&str> = offered
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert!(
            names.contains(&"read_file") && names.contains(&knowledge::TOOL),
            "{names:?}"
        );
        assert!(
            !names.contains(&SUBAGENT_TOOL),
            "a subagent gets these, and cannot start another"
        );
    }
}

#[cfg(test)]
mod shared_terminal_tests {
    use super::*;
    #[test]
    fn even_an_unadvertised_native_call_cannot_execute_when_the_desktop_owns_commands() {
        let dir = std::env::temp_dir().join(format!("nexus-terminal-route-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("workspace");
        let (result, _) = run_builtin(
            "run_command",
            r#"{"command":"echo escaped > marker"}"#,
            &dir,
            &mut |_| Decision::Allow,
            true,
        );
        assert!(result.contains("terminal_run"));
        assert!(!dir.join("marker").exists());
        let (result, _) = run_builtin(
            "run_command",
            r#"{"command":"echo allowed > marker"}"#,
            &dir,
            &mut |_| Decision::Allow,
            false,
        );
        assert!(result.contains("Exit code 0"));
        assert!(dir.join("marker").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
