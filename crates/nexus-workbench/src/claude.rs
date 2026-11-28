//! Claude Code's stream-json control protocol, the one Anthropic's Agent SDKs speak to the `claude` binary:
//!
//! ```text
//! claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages
//!        --replay-user-messages --permission-prompt-tool stdio --permission-mode <mode>
//!        (--session-id <uuid> | --resume <uuid>) [--name=<name>]
//!        [--resume-session-at <message uuid>]
//! ```
//!
//! One JSON object per line each way. On stdin: `initialize` first, then user messages, answers to approvals and
//! control requests (interrupt, permission mode, model). On stdout: `system` (`init` with the session id), `assistant`
//! and `user` messages (one content block each; `parent_tool_use_id` set inside a sub-agent), `stream_event` (the API's
//! raw stream, for live text and each call's usage), each of our user messages again (`isReplay`) when Claude Code reads
//! it, `control_request` (`can_use_tool`: an approval), `control_response`
//! (answers to ours) and `result` (a turn ended, with its cost). [`Translator`] turns stdout into [`Out`]s; the
//! functions at the bottom build what goes on stdin. Checked against `claude` 2.1.287.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::driver::{Background, Out, Protocol, Say, cap};
use crate::event::{
    BackgroundTask, CacheWrites, Effort, Image, PermissionMode, PermissionSuggestion, TaskDetail, TaskStatus, Usage,
    WbEvent,
};
use crate::harness::Harness;
use crate::harness::claude::permission_mode;

/// A tool result longer than this is cut (the agent itself keeps the whole).
pub use crate::driver::MAX_TOOL_OUTPUT;

/// The arguments for one run of `claude` in a session. `name` is the session's name (session_name.rs), given as
/// `--name=<name>` on a fresh start and on a resume alike, so Claude Code's ListAgents and SendMessage reach the session by
/// it. Claude Code reads it only when it starts: a rename reaches the next run, and a running agent keeps its old name.
#[allow(clippy::too_many_arguments)]
pub fn args(
    mode: PermissionMode,
    model: Option<&str>,
    effort: Option<Effort>,
    native_id: &str,
    resume: bool,
    rewind: Option<&str>,
    instructions: Option<&std::path::Path>,
    name: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        // Each message again, with `isReplay`, when Claude Code reads it: a message sent mid-turn waits for its next
        // step, so this is when the person's message was read, not when it was written (`Out::Taken`).
        "--replay-user-messages",
        "--permission-prompt-tool",
        "stdio",
        // Lets the person switch to Full access later without restarting the agent; it grants nothing by itself.
        "--allow-dangerously-skip-permissions",
        "--permission-mode",
        permission_mode(mode),
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    // From a file when there is one: the text spans several lines, which `claude.cmd` (an npm install on Windows) can't
    // be started with ([`crate::driver::instructions_file`]).
    match instructions {
        Some(file) => args.extend([
            "--append-system-prompt-file".into(),
            file.to_string_lossy().into_owned(),
        ]),
        None => args.extend([
            "--append-system-prompt".into(),
            nexus_agents::Claude.instructions().text(),
        ]),
    }
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(effort) = effort {
        args.extend(["--effort".into(), effort.as_str().into()]);
    }
    args.extend([
        if resume { "--resume" } else { "--session-id" }.to_string(),
        native_id.to_string(),
    ]);
    // One argv entry: the value can never be read as a flag of its own.
    if let Some(name) = name {
        args.push(format!("--name={name}"));
    }
    // After a revert: the conversation goes on from this message, as if the later turns never happened. Hidden from
    // `claude --help` but what the Agent SDK's `resumeSessionAt` passes (checked in Claude Code 2.1.287).
    if let Some(at) = rewind {
        args.extend(["--resume-session-at".into(), at.into()]);
    }
    args
}

#[derive(Default)]
struct Stream {
    message_id: String,
    model: String,
    texts: HashMap<u64, String>,
    /// What its `message_start` said it wrote to the prompt cache, by lifetime.
    cache_writes: Option<CacheWrites>,
}

/// Claude Code's stdout, line by line, into [`Out`]s. Keeps what it needs across lines: the message each streamed
/// block belongs to (per sub-agent, since they interleave) and how many blocks of each message it has seen.
#[derive(Default)]
pub struct Translator {
    streams: HashMap<Option<String>, Stream>,
    blocks: HashMap<String, usize>,
    /// The slash commands last reported: an `init` that repeats them logs nothing.
    commands: Option<Vec<String>>,
    /// The tool call that started each task (a sub-agent's Agent call, a shell's Bash call), by task id: an approval
    /// names the sub-agent that asks by its task id (`agent_id`).
    task_calls: HashMap<String, String>,
    /// Each task's depth in the agent tree, by task id: a shell sits one below the sub-agent that started it.
    depths: HashMap<String, u32>,
    /// The sub-agents whose model was sent, by task id.
    models: HashSet<String>,
    /// Our requests about one task (`Say::TaskStop` and the others) not answered yet: their answer is an `Out::Answered`.
    task_asks: HashSet<String>,
    compact: Option<String>,
    compact_boundary: bool,
    /// `/rename` lines sent (`Say::Rename`) whose turnless result (`num_turns` 0) has not come: it ends no turn.
    renames: usize,
    /// Whether outside browser tools wait for a recorded bypass ([`crate::browser_routing`]).
    browser: crate::browser_routing::Gate,
    /// The run has the shared terminal tools (its `init` lists `terminal_run`), so its sub-agents need telling.
    terminal_tools: bool,
}

/// Declares `perTaskStopAffordance` in `initialize`: the session's Interrupt then ends only the lead's turn, and
/// sub-agents and background commands run on (recorded from 2.1.292 in `claude-2.1.292-interrupt-per-task-stop`). The
/// app says so on the Stop button, says what still runs after an Interrupt and offers to stop it all, and each
/// sub-agent has its own Stop (`wb_task_stop`).
pub const PER_TASK_STOP: bool = true;

/// The `SubagentStart` hook's callback id: it gives a sub-agent the terminal guidance.
const SUBAGENT_HOOK: &str = "nexus_subagent_terminal";

/// What a sub-agent is told when the session removed Bash for the shared terminal tools.
const SUBAGENT_TERMINAL: &str = "This session has no Bash tool. To run a command, call mcp__nexus_app__terminal_run \
with a `command` and a unique `request_id`. Then call mcp__nexus_app__terminal_read with the returned ID to read its \
output and exit status. Never try another executor.";

fn text_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_string()
}

/// What a task message means for the idle timeout: a background task started or ended.
/// A foreground task ends inside its turn, and a task without an id could never be matched to its end.
fn background(message: &Value) -> Option<Out> {
    let subtype = message.get("subtype").and_then(Value::as_str);
    let event = match subtype {
        Some("task_started") if message.get("is_backgrounded").and_then(Value::as_bool) == Some(true) => {
            Background::Started(task_id(message)?)
        }
        Some("task_notification") => Background::Ended(task_id(message)?),
        _ => return None,
    };
    Some(Out::Background(event))
}

/// A task message's `task_id`, unless missing or empty.
fn task_id(message: &Value) -> Option<String> {
    message
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(String::from)
}

/// A message of the main conversation (not a sub-agent's) is where a revert to this turn would resume: its `uuid`.
fn with_cursor(message: &Value, mut out: Vec<Out>) -> Vec<Out> {
    if parent_of(message).is_none()
        && let Some(uuid) = message
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|uuid| !uuid.is_empty())
    {
        out.push(Out::Cursor(uuid.into()));
    }
    out
}

fn parent_of(message: &Value) -> Option<String> {
    message
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .map(String::from)
}

fn tokens(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// What a call wrote to the prompt cache, by lifetime: its `usage.cache_creation`, else the sum of its `iterations`'
/// (Claude Code 2.1.292's `message_delta` keeps the split only there). None: the call does not say.
pub fn cache_writes(usage: &Value) -> Option<CacheWrites> {
    let split = |creation: &Value| CacheWrites {
        five_minute_tokens: tokens(creation, "ephemeral_5m_input_tokens"),
        one_hour_tokens: tokens(creation, "ephemeral_1h_input_tokens"),
    };
    if let Some(creation) = usage.get("cache_creation").filter(|creation| creation.is_object()) {
        return Some(split(creation));
    }
    let mut parts = usage
        .get("iterations")?
        .as_array()?
        .iter()
        .filter_map(|iteration| iteration.get("cache_creation").filter(|creation| creation.is_object()))
        .peekable();
    parts.peek()?;
    Some(parts.map(split).fold(CacheWrites::default(), |sum, part| CacheWrites {
        five_minute_tokens: sum.five_minute_tokens + part.five_minute_tokens,
        one_hour_tokens: sum.one_hour_tokens + part.one_hour_tokens,
    }))
}

impl Translator {
    pub fn line(&mut self, line: &str) -> Vec<Out> {
        let out = self.translate(line);
        for out in &out {
            if let Out::Event(WbEvent::Tool { name, input, .. }) = out {
                self.browser.call(name, input);
            }
        }
        out
    }

    fn translate(&mut self, line: &str) -> Vec<Out> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        match message.get("type").and_then(Value::as_str).unwrap_or_default() {
            "system" => self.system(&message),
            "stream_event" => self.stream(&message),
            // A call the plan refused also says so on its assistant message, each time: Claude Code sends a
            // `rate_limit_event` only when the limits change.
            // Claude Code 2.1.312 answers `/rename` with a `<synthetic>` "Session renamed to: …": no model turn, so no
            // transcript text and no turn of its own (it would open one that its turnless result never ends).
            "assistant"
                if self.renames > 0
                    && message.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>") =>
            {
                vec![]
            }
            "assistant" => {
                let mut out: Vec<Out> = message
                    .pointer("/api_error_params/rate_limit_info")
                    .filter(|info| info.get("status").and_then(Value::as_str) == Some("rejected"))
                    .map(rate_limit)
                    .into_iter()
                    .collect();
                out.extend(with_cursor(&message, self.assistant(&message)));
                out
            }
            // Not a cursor: the conversation stands where it did, the message only joined it.
            "user" if message.get("isReplay").and_then(Value::as_bool) == Some(true) => vec![taken(&message)],
            "user" => with_cursor(&message, self.user(&message)),
            "result"
                if self.renames > 0
                    && self.compact.is_none()
                    && message.get("num_turns").and_then(Value::as_u64) == Some(0) =>
            {
                self.renames -= 1;
                vec![]
            }
            "result" => {
                let outcome = result(&message);
                let mut out = vec![];
                if let Some(request_id) = self.compact.take() {
                    let answer = match &outcome {
                        Out::Result { error: Some(error), .. } => Err(error.clone()),
                        _ if !self.compact_boundary => Err(message
                            .get("result")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                            .unwrap_or("The agent did not report a compaction boundary.")
                            .to_string()),
                        _ => Ok(Value::Null),
                    };
                    out.push(Out::Answered {
                        request_id,
                        result: answer,
                    });
                }
                out.push(outcome);
                out
            }
            "control_request" => self.control_request(&message),
            "control_response" => {
                let response = message.get("response").unwrap_or(&Value::Null);
                let error = (response.get("subtype").and_then(Value::as_str) == Some("error"))
                    .then(|| text_of(response.get("error")));
                let request_id = text_of(response.get("request_id"));
                if self.task_asks.remove(&request_id) {
                    let result = match error {
                        Some(error) => Err(error),
                        None => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
                    };
                    return vec![Out::Answered { request_id, result }];
                }
                if request_id.starts_with("status-") {
                    return vec![];
                }
                // `initialize`'s answer names the plan the login is on: the host keeps its tier (`plan_tier`).
                let plan = (request_id == "init")
                    .then(|| {
                        response
                            .pointer("/response/account")
                            .and_then(nexus_local_agents::claude_plan)
                    })
                    .flatten()
                    .filter(|plan| !plan.trim().is_empty());
                let mut out = Vec::new();
                if let Some(plan) = plan {
                    out.push(Out::Observe(json!({ "plan": plan })));
                }
                out.push(Out::Ack { request_id, error });
                out
            }
            "control_cancel_request" => vec![Out::Cancelled {
                request_id: text_of(message.get("request_id")),
            }],
            "rate_limit_event" => vec![rate_limit(message.get("rate_limit_info").unwrap_or(&Value::Null))],
            _ => vec![],
        }
    }

    fn system(&mut self, message: &Value) -> Vec<Out> {
        match message.get("subtype").and_then(Value::as_str).unwrap_or_default() {
            "init" => {
                let tools: Vec<String> = message
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.as_str().map(String::from))
                    .collect();
                self.browser.init(&tools);
                self.terminal_tools = tools
                    .iter()
                    .any(|tool| *tool == format!("mcp__{}__terminal_run", crate::orchestra::SERVER));
                let mut out = vec![Out::Init {
                    native_id: text_of(message.get("session_id")),
                    model: message.get("model").and_then(Value::as_str).map(String::from),
                    version: message
                        .get("claude_code_version")
                        .and_then(Value::as_str)
                        .map(String::from),
                }];
                // `slash_commands` is a flat list of bare names ("compact", "vercel:deploy"). Left out: the ones only a
                // terminal can run (`terminal_slash_commands`) and Claude Code's hidden ones ("__remote-workflow").
                let names = |key: &str| -> Vec<String> {
                    message
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|name| name.as_str())
                        .map(|name| name.trim_start_matches('/').to_string())
                        .filter(|name| !name.is_empty())
                        .collect()
                };
                let observation = json!({ "mcp": message.get("mcp_servers"), "skills": message.get("skills"), "plugins": message.get("plugins"), "effort": message.get("effortLevel"), "permissions": message.get("permissionMode") });
                let mut observation = observation;
                if let Some(off) = fast_off(message) {
                    observation["fastOff"] = off;
                }
                if observation
                    .as_object()
                    .is_some_and(|fields| fields.values().any(|value| !value.is_null()))
                {
                    out.push(Out::Observe(observation));
                }

                let terminal_only = names("terminal_slash_commands");
                let commands: Vec<String> = names("slash_commands")
                    .into_iter()
                    .filter(|name| !name.starts_with("__") && !terminal_only.contains(name))
                    .collect();
                if self.commands.as_ref() != Some(&commands) {
                    self.commands = Some(commands.clone());
                    out.push(Out::Event(WbEvent::Commands { commands }));
                }
                out
            }
            // Sub-agents and shells, as Claude Code 2.1.292 reports them.
            "task_started" | "task_updated" | "task_notification" => {
                let mut out: Vec<Out> = self.task(message).into_iter().collect();
                out.extend(background(message));
                out
            }
            // A running sub-agent's progress: its latest action and its counts so far.
            "task_progress" => progress(message).into_iter().collect(),
            "background_tasks_changed" => {
                let mut out = Vec::new();
                // Only a list replaces the tasks known: a message of another shape changes nothing.
                if let Some(tasks) = message.get("tasks").and_then(Value::as_array) {
                    out.push(Out::Background(Background::Now(
                        tasks.iter().filter_map(task_id).collect(),
                    )));
                }
                let tasks = message
                    .get("tasks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|task| BackgroundTask {
                        task_id: text_of(task.get("task_id")),
                        task_type: Some(text_of(task.get("task_type"))).filter(|t| !t.is_empty()),
                        description: text_of(task.get("description")),
                    })
                    .filter(|task| !task.task_id.is_empty())
                    .collect();
                out.push(Out::Event(WbEvent::BackgroundTasks { tasks }));
                out
            }
            "compact_boundary" => {
                self.compact_boundary = true;
                vec![Out::Event(WbEvent::Notice {
                    text: "Claude Code compacted the conversation to fit its context window.".into(),
                })]
            }
            _ => vec![],
        }
    }

    fn stream(&mut self, message: &Value) -> Vec<Out> {
        let parent = parent_of(message);
        let Some(event) = message.get("event") else {
            return vec![];
        };
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_start" => {
                let start = event.get("message").unwrap_or(&Value::Null);
                self.streams.insert(
                    parent,
                    Stream {
                        message_id: text_of(start.get("id")),
                        model: text_of(start.get("model")),
                        texts: HashMap::new(),
                        cache_writes: start.get("usage").and_then(cache_writes),
                    },
                );
                vec![]
            }
            "content_block_delta" => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                // A sub-agent's streamed text carries `parent` (the sub-agent's tool call), so the timeline
                // shows it inside that sub-agent's group, and lib.rs gives it a refresh slot of its own. Claude Code 2.1.292
                // streams only the main conversation (every `stream_event` has a null parent): a sub-agent's text arrives
                // whole, in its assistant frames.
                if delta.get("type").and_then(Value::as_str) != Some("text_delta") {
                    return vec![];
                }
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(stream) = self.streams.get_mut(&parent) else {
                    return vec![];
                };
                let text = stream.texts.entry(index).or_default();
                text.push_str(delta.get("text").and_then(Value::as_str).unwrap_or_default());
                vec![Out::Delta {
                    id: format!("{}:{index}", stream.message_id),
                    text: text.clone(),
                    parent,
                }]
            }
            "message_delta" => {
                let Some(usage) = event.get("usage") else {
                    return vec![];
                };
                let stream = self.streams.get(&parent);
                let model = stream.map(|s| s.model.clone()).unwrap_or_default();
                let started = stream.and_then(|s| s.cache_writes);
                vec![Out::Call {
                    model,
                    usage: Usage {
                        input_tokens: tokens(usage, "input_tokens"),
                        output_tokens: tokens(usage, "output_tokens"),
                        cache_read_tokens: tokens(usage, "cache_read_input_tokens"),
                        cache_write_tokens: tokens(usage, "cache_creation_input_tokens"),
                        cache_writes: cache_writes(usage).or(started),
                    },
                    parent,
                }]
            }
            _ => vec![],
        }
    }

    fn assistant(&mut self, message: &Value) -> Vec<Out> {
        let parent = parent_of(message);
        let body = message.get("message").unwrap_or(&Value::Null);
        let message_id = text_of(body.get("id"));
        let mut out = Vec::new();
        // A sub-agent's first frame names the model it runs on: an update of its task, carried on its next row.
        if parent.is_some()
            && let Some(agent) = message
                .get("agent_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            && let Some(model) = body
                .get("model")
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            && self.models.insert(agent.to_string())
        {
            out.push(Out::Progress(update(
                agent,
                TaskDetail {
                    model: Some(model.into()),
                    ..TaskDetail::default()
                },
            )));
        }
        for block in body.get("content").and_then(Value::as_array).into_iter().flatten() {
            let count = self.blocks.entry(message_id.clone()).or_insert(0);
            let id = format!("{message_id}:{count}");
            *count += 1;
            match block.get("type").and_then(Value::as_str).unwrap_or_default() {
                "text" => {
                    let text = text_of(block.get("text"));
                    if !text.trim().is_empty() {
                        out.push(Out::Event(WbEvent::Text {
                            id,
                            text,
                            parent: parent.clone(),
                        }));
                    }
                }
                "thinking" => {
                    let text = text_of(block.get("thinking"));
                    if !text.trim().is_empty() {
                        out.push(Out::Event(WbEvent::Thinking {
                            id,
                            text,
                            parent: parent.clone(),
                        }));
                    }
                }
                "tool_use" | "server_tool_use" => out.push(Out::Event(WbEvent::Tool {
                    id: text_of(block.get("id")),
                    name: text_of(block.get("name")),
                    input: block.get("input").cloned().unwrap_or(Value::Null),
                    parent: parent.clone(),
                })),
                _ => {}
            }
        }
        out
    }

    fn user(&mut self, message: &Value) -> Vec<Out> {
        let content = message.pointer("/message/content");
        let mut out = Vec::new();
        for block in content.and_then(Value::as_array).into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let (output, truncated) = cap(flatten(block.get("content")));
            out.push(Out::Event(WbEvent::ToolResult {
                images: crate::tool_images::extract(block.get("content")),
                id: text_of(block.get("tool_use_id")),
                output,
                is_error: block.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                truncated,
                parent: parent_of(message),
            }));
        }
        out
    }

    fn task(&mut self, message: &Value) -> Option<Out> {
        let task_id = text_of(message.get("task_id"));
        if task_id.is_empty() {
            return None;
        }
        let ended = |status: &str| match status {
            "failed" => Some(TaskStatus::Failed),
            "stopped" | "killed" => Some(TaskStatus::Stopped),
            "completed" => Some(TaskStatus::Completed),
            _ => None,
        };
        let started = message.get("subtype").and_then(Value::as_str) == Some("task_started");
        let optional = |key: &str| Some(text_of(message.get(key))).filter(|t| !t.is_empty());
        let patch = message.get("patch");
        let status = match text_of(message.get("subtype")).as_str() {
            "task_started" => TaskStatus::Running,
            // `patch` holds only what changed: its end (`status`, `end_time`), or a move to the background.
            "task_updated" => match ended(&text_of(patch.and_then(|p| p.get("status")))) {
                Some(status) => status,
                None => {
                    let backgrounded = patch.and_then(|p| p.get("is_backgrounded")).and_then(Value::as_bool)?;
                    return Some(Out::Progress(update(
                        &task_id,
                        TaskDetail {
                            backgrounded: Some(backgrounded),
                            run_id: optional("run_id"),
                            ..TaskDetail::default()
                        },
                    )));
                }
            },
            // `task_notification` always ends the task. A status this does not know is not read as success: the task
            // ended, so it shows as stopped, which claims neither success nor failure.
            _ => ended(&text_of(message.get("status"))).unwrap_or(TaskStatus::Stopped),
        };
        let usage = message.get("usage");
        let number = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
        // The end of a task has a `summary` where the start has a `description`.
        let description = Some(text_of(message.get("description")))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| text_of(message.get("summary")));
        let tool_use_id = optional("tool_use_id");
        let task_type = optional("task_type");
        // Its place in the tree is on its start only. A re-run's start has no `parent_task_id`: the translator passes on
        // what Claude Code says, and the core's task book (agents.rs) fills it in from the first start. Every row names
        // its run, so a report of an older run is never taken for the current one's.
        let mut detail = TaskDetail {
            run_id: optional("run_id"),
            ..TaskDetail::default()
        };
        if started {
            if let Some(call) = &tool_use_id {
                self.task_calls.insert(task_id.clone(), call.clone());
            }
            detail.parent_task_id = optional("parent_task_id");
            // A sub-agent says its depth. A shell is one below the sub-agent that started it, or at the top when the
            // session's own agent did; a shell owned by a sub-agent this run never saw has no depth.
            let owned = message.get("owned_by_subagent").and_then(Value::as_bool) == Some(true);
            detail.depth = match message.get("spawn_depth").and_then(Value::as_u64) {
                Some(depth) => u32::try_from(depth).ok(),
                None if task_type.as_deref() != Some("local_bash") => None,
                None => match &detail.parent_task_id {
                    Some(parent) => self.depths.get(parent).map(|depth| depth + 1),
                    None => (!owned).then_some(1),
                },
            };
            if let Some(depth) = detail.depth {
                self.depths.insert(task_id.clone(), depth);
            }
            detail.backgrounded = message.get("is_backgrounded").and_then(Value::as_bool);
        } else if status != TaskStatus::Running {
            detail.reason = optional("reason");
        }
        Some(Out::Event(WbEvent::Task {
            task_id,
            tool_use_id,
            task_type,
            agent_type: optional("subagent_type"),
            status,
            description,
            tool_count: number("tool_uses"),
            total_tokens: number("total_tokens"),
            duration_ms: number("duration_ms"),
            detail: detail.some(),
        }))
    }

    fn control_request(&self, message: &Value) -> Vec<Out> {
        let request_id = text_of(message.get("request_id"));
        let request = message.get("request").unwrap_or(&Value::Null);
        if request.get("subtype").and_then(Value::as_str) == Some("hook_callback")
            && request.get("callback_id").and_then(Value::as_str) == Some(crate::browser_routing::HOOK)
        {
            let tool = request
                .pointer("/input/tool_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return vec![Out::Reply(
                json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id,
                    "response": self.browser.decide(tool)}})
                .to_string(),
            )];
        }
        if request.get("subtype").and_then(Value::as_str) == Some("hook_callback")
            && request.get("callback_id").and_then(Value::as_str) == Some(SUBAGENT_HOOK)
        {
            // A sub-agent's prompt lacks the session's `--append-system-prompt`, and the session removed its Bash.
            let answer = if self.terminal_tools {
                json!({"hookSpecificOutput": {"hookEventName": "SubagentStart", "additionalContext": SUBAGENT_TERMINAL}})
            } else {
                json!({})
            };
            return vec![Out::Reply(
                json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id,
                    "response": answer}})
                .to_string(),
            )];
        }
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            // Only the browser hook and no SDK MCP server is registered, so nothing else is expected; answer so nothing
            // waits.
            return vec![Out::Reply(
                json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id,
                    "error": "Nexus does not handle this request"}})
                .to_string(),
            )];
        }
        let suggestions = request
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|raw| PermissionSuggestion {
                kind: text_of(raw.get("type")),
                label: suggestion_label(raw),
                raw: raw.clone(),
            })
            .collect();
        let description = request
            .get("description")
            .or_else(|| request.get("decision_reason"))
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
            .map(String::from);
        // A sub-agent's request names it by task id: its Agent call, when its start was read.
        let parent = request
            .get("agent_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(|id| self.task_calls.get(id).cloned().unwrap_or_else(|| id.to_string()));
        vec![Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id: request.get("tool_use_id").and_then(Value::as_str).map(String::from),
            tool: text_of(request.get("tool_name")),
            input: request.get("input").cloned().unwrap_or(Value::Null),
            description,
            suggestions,
            parent,
        })]
    }
}

/// An update of a running task: no type, no call, no description, so it never renames or restarts it.
fn update(task_id: &str, detail: TaskDetail) -> WbEvent {
    WbEvent::Task {
        task_id: task_id.into(),
        tool_use_id: None,
        task_type: None,
        agent_type: None,
        status: TaskStatus::Running,
        description: String::new(),
        tool_count: None,
        total_tokens: None,
        duration_ms: None,
        detail: TaskDetail {
            progress: Some(true),
            ..detail
        }
        .some(),
    }
}

/// `task_progress`: a running sub-agent's latest action (`description`) and its tool calls and context size so far.
fn progress(message: &Value) -> Option<Out> {
    let task_id = task_id(message)?;
    let usage = message.get("usage");
    let number = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
    let text = |key: &str| Some(text_of(message.get(key))).filter(|t| !t.is_empty());
    let mut event = update(
        &task_id,
        TaskDetail {
            activity: text("description"),
            run_id: text("run_id"),
            ..TaskDetail::default()
        },
    );
    if let WbEvent::Task {
        tool_count,
        total_tokens,
        ..
    } = &mut event
    {
        *tool_count = number("tool_uses");
        *total_tokens = number("total_tokens");
    }
    Some(Out::Progress(event))
}

/// A user message of ours that Claude Code read: its text, a plain string or text blocks.
fn taken(message: &Value) -> Out {
    let texts = match message.pointer("/message/content") {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .map(|block| text_of(block.get("text")))
            .collect(),
        _ => vec![],
    };
    Out::Taken { texts }
}

/// A tool result's content (a string, or text and other blocks) as text.
fn flatten(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("text") => text_of(block.get("text")),
                Some("image") => "[image]".into(),
                Some("tool_reference") => format!("[{}]", text_of(block.get("tool_name"))),
                _ => String::new(),
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn result(message: &Value) -> Out {
    let subtype = text_of(message.get("subtype"));
    // Claude Code ends an interrupted turn with this subtype, but a real failure during execution can carry it too:
    // the turn shows as interrupted either way, and any text the result carries is kept as its error.
    let interrupted = subtype == "error_during_execution";
    let failed = message.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype != "success";
    let text = text_of(message.get("result"));
    let error = if interrupted {
        Some(text).filter(|t| !t.trim().is_empty())
    } else {
        failed.then(|| {
            if text.is_empty() {
                subtype.replace('_', " ")
            } else {
                text
            }
        })
    };
    let context_window = message
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|models| models.values().filter_map(|m| m.get("contextWindow")?.as_u64()).max());
    Out::Result {
        total_cost_usd: message.get("total_cost_usd").and_then(Value::as_f64),
        duration_ms: message.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
        error,
        interrupted,
        context_window,
    }
}

/// Short words for a button from one of Claude Code's permission suggestions.
pub fn suggestion_label(raw: &Value) -> String {
    let place = match raw.get("destination").and_then(Value::as_str) {
        Some("session") => "for this session",
        Some("userSettings") => "everywhere",
        Some("projectSettings") | Some("localSettings") => "in this project",
        _ => "from now on",
    };
    match raw.get("type").and_then(Value::as_str) {
        Some("addRules") | Some("replaceRules") => {
            let rules: Vec<String> = raw
                .get("rules")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|rule| {
                    let tool = text_of(rule.get("toolName"));
                    match rule.get("ruleContent").and_then(Value::as_str) {
                        Some(content) if !content.is_empty() => format!("{tool}({content})"),
                        _ => tool,
                    }
                })
                .collect();
            let verb = if raw.get("behavior").and_then(Value::as_str) == Some("deny") {
                "Always deny"
            } else {
                "Always allow"
            };
            format!("{verb} {} {place}", rules.join(", "))
        }
        Some("setMode") => match raw.get("mode").and_then(Value::as_str) {
            Some("acceptEdits") => format!("Allow all edits {place}"),
            Some("bypassPermissions") => format!("Allow everything {place}"),
            Some(mode) => format!("Switch to {mode} {place}"),
            None => "Change mode".into(),
        },
        Some("addDirectories") => {
            let dirs: Vec<String> = raw
                .get("directories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|d| d.as_str().map(String::from))
                .collect();
            format!("Allow access to {} {place}", dirs.join(", "))
        }
        _ => "Allow and remember".into(),
    }
}

impl Protocol for Translator {
    fn open(&mut self) -> Vec<Out> {
        vec![Out::Reply(initialize())]
    }

    fn say(&mut self, say: Say) -> Vec<Out> {
        if let Say::ForgetInspection { request } = &say {
            self.task_asks.remove(request);
            return vec![];
        }
        if let Say::Inspect { request, field, .. } = &say {
            if *field != "mcp" {
                return vec![Out::Answered {
                    request_id: request.clone(),
                    result: Err("This runtime does not report that inventory.".into()),
                }];
            }
            self.task_asks.insert(request.clone());
            return vec![Out::Reply(
                json!({"type":"control_request","request_id":request,"request":{"subtype":"mcp_status"}}).to_string(),
            )];
        }
        if let Say::Compact { request, instructions } = say {
            self.compact = Some(request);
            self.compact_boundary = false;
            let text = instructions.map_or_else(
                || "/compact".to_string(),
                |instructions| format!("/compact {instructions}"),
            );
            return vec![Out::Reply(user_message(&text, &[]))];
        }
        if let Say::Rename { name } = say {
            self.renames += 1;
            return vec![Out::Reply(user_message(&format!("/rename {name}"), &[]))];
        }
        vec![Out::Reply(match say {
            Say::Compact { .. } | Say::Inspect { .. } | Say::ForgetInspection { .. } | Say::Rename { .. } => {
                unreachable!("handled above")
            }
            Say::TaskStop { request, task_id } => {
                self.task_asks.insert(request.clone());
                stop_task(&request, &task_id)
            }
            Say::TaskBackground { request, tool_use_id } => {
                self.task_asks.insert(request.clone());
                background_task(&request, &tool_use_id)
            }
            Say::TaskOutput { request, task_id } => {
                self.task_asks.insert(request.clone());
                task_output(&request, &task_id)
            }
            // Claude Code takes a message that arrives mid-turn into the turn at its next step (after the tool
            // running), or, when the turn has no step left, as the next turn. Checked against claude 2.1.287: a
            // `priority: "now"` message instead always ends the turn and starts another.
            Say::User { text, images } | Say::Steer { text, images, .. } => user_message(&text, &images),
            Say::Interrupt { request } => interrupt(&request),
            Say::Mode { request, mode } => set_permission_mode(&request, mode),
            Say::Model { request, model } => set_model(&request, model.as_deref()),
            Say::Effort { request, effort } => set_effort(&request, effort),
            Say::Fast { request, on } => set_fast(&request, on),
            Say::Allow {
                request_id,
                input,
                answers,
                remember,
            } => allow(&request_id, &input, answers.as_ref(), remember.as_ref()),
            Say::Deny { request_id, message } => {
                deny(&request_id, message.as_deref().unwrap_or("The person denied this."))
            }
        })]
    }

    fn read(&mut self, line: &str) -> Vec<Out> {
        self.line(line)
    }
}

/// The first line on stdin.
pub fn initialize() -> String {
    initialize_with(PER_TASK_STOP)
}

fn initialize_with(per_task_stop: bool) -> String {
    // Every MCP call reaches the browser hook, which refuses outside browser tools until a bypass
    // ([`crate::browser_routing`]). Hooks run in every permission mode, Full access included.
    let mut request = json!({"subtype": "initialize", "hooks": {"PreToolUse": [
        {"matcher": "^mcp__", "hookCallbackIds": [crate::browser_routing::HOOK]}
    ], "SubagentStart": [{"hookCallbackIds": [SUBAGENT_HOOK]}]}});
    if per_task_stop {
        request["perTaskStopAffordance"] = json!(true);
    }
    json!({"type": "control_request", "request_id": "init", "request": request}).to_string()
}

/// Moves the foreground command of `tool_use_id` to the background: `{backgrounded}`, recorded for a command only
/// (2.1.292, `claude-2.1.292-move-to-background`). Claude Code then marks the task backgrounded and ends the tool call
/// with a receipt.
pub fn background_task(request_id: &str, tool_use_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "background_tasks", "tool_use_id": tool_use_id}})
    .to_string()
}

/// Reads a shell's output: `{output, total_bytes, truncated}`. Claude Code refuses a sub-agent (2.1.292,
/// `claude-2.1.292-shell-controls`, `claude-2.1.292-stop-spares-background`).
pub fn task_output(request_id: &str, task_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "get_task_output", "task_id": task_id}})
    .to_string()
}

/// Stops one task. Claude Code also stops its foreground sub-agents and running commands, and lets its background
/// sub-agents run on (2.1.292, `claude-2.1.292-stop-spares-background`). It answers success for an unknown or ended
/// task id, so the core asks only about a task it knows runs.
pub fn stop_task(request_id: &str, task_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "stop_task", "task_id": task_id}})
    .to_string()
}

/// A message: plain text, or its images first and then its text (the order the Messages API recommends).
pub fn user_message(text: &str, images: &[Image]) -> String {
    if images.is_empty() {
        return json!({"type": "user", "message": {"role": "user", "content": text}}).to_string();
    }
    let mut content: Vec<Value> = images
        .iter()
        .map(|image| json!({"type": "image", "source": {"type": "base64", "media_type": image.media_type, "data": image.data}}))
        .collect();
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    json!({"type": "user", "message": {"role": "user", "content": content}}).to_string()
}

pub fn interrupt(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}}).to_string()
}

pub fn set_permission_mode(request_id: &str, mode: PermissionMode) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "set_permission_mode", "mode": permission_mode(mode)}})
    .to_string()
}

/// Changes the effort of a running session (None: back to the model's default). The `/effort` command sends the same
/// request; Claude Code 2.1.303 reports the result in `get_settings` (`applied.effort`).
pub fn set_effort(request_id: &str, effort: Option<Effort>) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "apply_flag_settings", "settings": {"effortLevel": effort.map(Effort::as_str)}}})
    .to_string()
}

/// Claude Code's `--settings` for a session that starts in fast mode. The Agent SDK's opt-in: without it Claude Code
/// reports `fast_mode_disabled_reason: "sdk_opt_in_required"` (2.1.292 fixtures) and runs at standard speed.
pub const FAST_SETTINGS: &str = r#"{"fastMode":true}"#;

/// Turns fast mode on or off in a running session: the same `apply_flag_settings` as [`set_effort`], read at the next
/// model call. Off is `false`, not `null`: a null would fall back to the person's own settings, which may turn it on,
/// and a session started without [`FAST_SETTINGS`] is off. Checked against claude 2.1.328 (`claude-2.1.328-fast-mode`).
pub fn set_fast(request_id: &str, on: bool) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "apply_flag_settings", "settings": {"fastMode": on}}})
    .to_string()
}

/// Whether `model` has fast mode: the models that have a fast speed. Others
/// do not take `speed: "fast"`. Either spelling: `anthropic/claude-opus-5.5` (Gateway) or `claude-opus-5-5`
/// (Claude Code).
pub fn supports_fast(model: &str) -> bool {
    // Later: a copy of the list of fast models; read it from Gateway's model list once that reports fast mode.
    let key = model.rsplit('/').next().unwrap_or(model).replace('.', "-");
    ["claude-opus-5-5", "claude-fable-5-2", "claude-opus-5"].contains(&key.as_str())
}

/// Why fast mode is not serving, from an `init`: Claude Code's `fast_mode_disabled_reason` ("preference" when the
/// Claude organization turned it off), else its state ("cooldown" after a rate limit). `null` when it is on; None
/// when this Claude Code reports nothing about it.
fn fast_off(init: &Value) -> Option<Value> {
    let state = init.get("fast_mode_state")?.as_str()?;
    Some(
        match (state, init.get("fast_mode_disabled_reason").and_then(Value::as_str)) {
            ("on", _) => Value::Null,
            (_, Some(reason)) => json!(reason),
            (state, None) => json!(state),
        },
    )
}

pub fn set_model(request_id: &str, model: Option<&str>) -> String {
    json!({"type": "control_request", "request_id": request_id,
        "request": {"subtype": "set_model", "model": model}})
    .to_string()
}

/// Allows the tool call, with the input Claude Code asked about (plus `answers` for AskUserQuestion) and the
/// permission rules to remember, if the person chose one of its suggestions.
pub fn allow(
    request_id: &str,
    input: &Value,
    answers: Option<&Map<String, Value>>,
    remember: Option<&Value>,
) -> String {
    let mut updated = input.clone();
    if let (Some(answers), Some(object)) = (answers, updated.as_object_mut()) {
        object.insert("answers".into(), Value::Object(answers.clone()));
    }
    let mut response = json!({"behavior": "allow", "updatedInput": updated});
    if let Some(rule) = remember {
        response["updatedPermissions"] = json!([rule]);
    }
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}})
        .to_string()
}

pub fn deny(request_id: &str, message: &str) -> String {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id,
        "response": {"behavior": "deny", "message": message}}})
    .to_string()
}

/// A `rate_limit_event`'s `rate_limit_info`: both windows under `unifiedWindows` (as fractions), or, from an older
/// Claude Code, the one window it names (`rateLimitType`, `utilization`). A refused call's reset is `resetsAt` (epoch
/// seconds), else the named window's, else the latest of the used-up windows'.
fn rate_limit(info: &Value) -> Out {
    let named = info.get("rateLimitType").and_then(Value::as_str);
    let used = |window: &str| {
        info.pointer(&format!("/unifiedWindows/{window}/utilization"))
            .and_then(Value::as_f64)
            .or_else(|| {
                (named == Some(window))
                    .then(|| info.get("utilization").and_then(Value::as_f64))
                    .flatten()
            })
            .map(|fraction| (fraction * 100.0).clamp(0.0, 100.0))
    };
    let refused = info.get("status").and_then(Value::as_str) == Some("rejected");
    let resets = |window: &Value| window.get("resetsAt").and_then(Value::as_i64);
    // Each window's reset, in seconds: under `unifiedWindows`, or the named window's own `resetsAt`.
    let window_resets = |window: &str| {
        info.pointer(&format!("/unifiedWindows/{window}"))
            .and_then(resets)
            .or_else(|| (named == Some(window)).then(|| resets(info)).flatten())
            .filter(|seconds| *seconds > 0)
            .map(|seconds| seconds * 1000)
    };
    let resets_at = refused
        .then(|| {
            resets(info)
                .or_else(|| {
                    named
                        .and_then(|window| info.pointer(&format!("/unifiedWindows/{window}")))
                        .and_then(resets)
                })
                .or_else(|| {
                    info.get("unifiedWindows")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|windows| windows.values())
                        .filter(|window| {
                            window
                                .get("utilization")
                                .and_then(Value::as_f64)
                                .is_some_and(|used| used >= 1.0)
                        })
                        .filter_map(resets)
                        .max()
                })
        })
        .flatten()
        .filter(|seconds| *seconds > 0)
        .map(|seconds| seconds * 1000);
    Out::RateLimit {
        session: used("five_hour"),
        weekly: used("seven_day"),
        session_resets: window_resets("five_hour"),
        weekly_resets: window_resets("seven_day"),
        refused,
        resets_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(translator: &mut Translator, lines: &[Value]) -> Vec<Out> {
        lines
            .iter()
            .flat_map(|line| translator.line(&line.to_string()))
            .collect()
    }

    #[test]
    fn preserves_tool_images_beyond_the_text_cap() {
        let mut translator = Translator::default();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(vec![0; MAX_TOOL_OUTPUT + 1]);
        let out = lines(
            &mut translator,
            &[json!({"type":"user", "parent_tool_use_id":"browser-agent",
            "message":{"content":[{"type":"tool_result", "tool_use_id":"shot", "is_error":true,
                "content":[{"type":"text", "text":"x".repeat(MAX_TOOL_OUTPUT + 1)},
                    {"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":data}}]}]}})],
        );
        assert!(
            matches!(&out[..], [Out::Event(WbEvent::ToolResult { id, output, images, is_error: true, truncated: true, parent: Some(parent) })]
            if id == "shot" && parent == "browser-agent" && output.len() <= MAX_TOOL_OUTPUT
                && images.len() == 1 && images[0].data == data)
        );
    }

    #[test]
    fn reads_the_background_work_left_running() {
        let mut translator = Translator::default();
        let outs = lines(
            &mut translator,
            &[
                json!({"type":"system","subtype":"task_started","task_id":"fg","task_type":"local_agent"}),
                json!({"type":"system","subtype":"task_started","task_id":"bg","task_type":"local_bash","is_backgrounded":true}),
                json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bg","task_type":"local_bash"}]}),
                json!({"type":"system","subtype":"task_notification","task_id":"bg","status":"completed"}),
                // Malformed: no id, or no list. Nothing changes.
                json!({"type":"system","subtype":"task_started","is_backgrounded":true}),
                json!({"type":"system","subtype":"background_tasks_changed"}),
            ],
        );
        let outs: Vec<Out> = outs
            .into_iter()
            .filter(|out| matches!(out, Out::Background(_)))
            .collect();
        assert_eq!(
            outs,
            vec![
                Out::Background(Background::Started("bg".into())),
                Out::Background(Background::Now(vec!["bg".into()])),
                Out::Background(Background::Ended("bg".into())),
            ]
        );
    }

    #[test]
    fn reads_how_much_of_the_plan_is_used() {
        let mut translator = Translator::default();
        let now = json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "rateLimitType": "five_hour",
            "unifiedWindows": {"five_hour": {"utilization": 0.25, "resetsAt": 1}, "seven_day": {"utilization": 0.53}}}});
        let older = json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "rateLimitType": "seven_day", "utilization": 1.2}});
        assert_eq!(
            lines(&mut translator, &[now, older]),
            vec![
                Out::RateLimit {
                    session: Some(25.0),
                    weekly: Some(53.0),
                    session_resets: Some(1000),
                    weekly_resets: None,
                    refused: false,
                    resets_at: None,
                },
                Out::RateLimit {
                    session: None,
                    weekly: Some(100.0),
                    session_resets: None,
                    weekly_resets: None,
                    refused: true,
                    resets_at: None,
                },
            ]
        );
    }

    #[test]
    fn reads_when_a_refused_call_s_limit_resets() {
        let refused = |info: Value| {
            let mut translator = Translator::default();
            match lines(
                &mut translator,
                &[json!({"type": "rate_limit_event", "rate_limit_info": info})],
            )
            .as_slice()
            {
                [Out::RateLimit { refused, resets_at, .. }] => (*refused, *resets_at),
                other => panic!("{other:?}"),
            }
        };
        // Its own `resetsAt`, in seconds.
        assert_eq!(
            refused(json!({"status": "rejected", "resetsAt": 1_792_605_600, "rateLimitType": "five_hour"})),
            (true, Some(1_792_605_600_000))
        );
        // The named window's, then the latest used-up window's.
        let windows = json!({"five_hour": {"utilization": 1.0, "resetsAt": 1_792_605_600},
            "seven_day": {"utilization": 1.0, "resetsAt": 1_793_000_000}});
        assert_eq!(
            refused(json!({"status": "rejected", "rateLimitType": "five_hour", "unifiedWindows": windows})),
            (true, Some(1_792_605_600_000))
        );
        assert_eq!(
            refused(json!({"status": "rejected", "unifiedWindows": windows})),
            (true, Some(1_793_000_000_000))
        );
        // No time said, or an allowed call: none.
        assert_eq!(refused(json!({"status": "rejected"})), (true, None));
        assert_eq!(
            refused(json!({"status": "allowed", "resetsAt": 1_792_605_600})),
            (false, None)
        );
    }

    #[test]
    fn a_refused_assistant_message_says_the_limit_again() {
        let mut translator = Translator::default();
        let line = json!({"type": "assistant", "parent_tool_use_id": null, "uuid": "u1",
            "api_error_params": {"rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour", "resetsAt": 1_792_605_600}},
            "message": {"id": "m1", "content": [{"type": "text", "text": "You've hit your limit"}]}});
        let out = lines(&mut translator, &[line]);
        assert!(
            matches!(
                out.first(),
                Some(Out::RateLimit {
                    refused: true,
                    resets_at: Some(1_792_605_600_000),
                    ..
                })
            ),
            "{out:?}"
        );
    }

    #[test]
    fn sets_the_effort_of_a_running_session() {
        let line: Value = serde_json::from_str(&set_effort("r1", Some(Effort::Max))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            line,
            json!({"type": "control_request", "request_id": "r1",
                "request": {"subtype": "apply_flag_settings", "settings": {"effortLevel": "max"}}})
        );
        let line: Value = serde_json::from_str(&set_effort("r2", None)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(line["request"]["settings"]["effortLevel"], Value::Null);
    }

    #[test]
    fn turns_fast_mode_on_and_off_in_a_running_session() {
        let on: Value = serde_json::from_str(&set_fast("r1", true)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            on,
            json!({"type": "control_request", "request_id": "r1",
                "request": {"subtype": "apply_flag_settings", "settings": {"fastMode": true}}})
        );
        // Off is false, never null: null would fall back to the person's own settings.
        let off: Value = serde_json::from_str(&set_fast("r2", false)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(off["request"]["settings"]["fastMode"], json!(false));
        assert_eq!(
            serde_json::from_str::<Value>(FAST_SETTINGS).ok(),
            Some(json!({"fastMode": true}))
        );
    }

    #[test]
    fn fast_mode_is_on_the_models_gateway_prices_it_for() {
        for model in [
            "anthropic/claude-opus-5.5",
            "claude-opus-5-5",
            "anthropic/claude-fable-5.2",
            "claude-opus-5",
        ] {
            assert!(supports_fast(model), "{model}");
        }
        for model in [
            "anthropic/claude-sonnet-5.5",
            "claude-haiku-5-5",
            "claude-opus-4-8",
            "claude-opus-5-55",
            "",
        ] {
            assert!(!supports_fast(model), "{model}");
        }
    }

    /// Recorded from claude 2.1.328 through Gateway: off before the opt-in
    /// (`sdk_opt_in_required`), on after `apply_flag_settings`.
    #[test]
    fn reports_whether_fast_mode_serves_from_each_init() {
        let reported: Vec<Value> = replay(include_str!("../tests/fixtures/claude-2.1.328-fast-mode.jsonl"))
            .into_iter()
            .filter_map(|out| match out {
                Out::Observe(observation) => observation.get("fastOff").cloned(),
                _ => None,
            })
            .collect();
        assert_eq!(reported, [json!("sdk_opt_in_required"), Value::Null]);
        // A Claude Code that reports nothing about it leaves the report as it is.
        assert_eq!(fast_off(&json!({"type": "system", "subtype": "init"})), None);
        assert_eq!(
            fast_off(&json!({"fast_mode_state": "cooldown"})),
            Some(json!("cooldown"))
        );
    }

    #[test]
    fn builds_the_arguments() {
        let first = args(
            PermissionMode::AcceptEdits,
            Some("claude-opus-5-5"),
            Some(Effort::High),
            "abc",
            false,
            None,
            None,
            None,
        );
        assert!(first.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(first.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]));
        assert!(first.windows(2).any(|w| w == ["--effort", "high"]));
        assert!(first.ends_with(&["--session-id".to_string(), "abc".to_string()]));
        let again = args(PermissionMode::Default, None, None, "abc", true, None, None, None);
        assert!(!again.contains(&"--model".to_string()));
        assert!(!again.contains(&"--effort".to_string()));
        assert!(again.ends_with(&["--resume".to_string(), "abc".to_string()]));
        let auto = args(PermissionMode::Auto, None, None, "abc", false, None, None, None);
        assert!(auto.windows(2).any(|w| w == ["--permission-mode", "auto"]));
        let reverted = args(
            PermissionMode::Default,
            None,
            None,
            "abc",
            true,
            Some("u-7"),
            None,
            None,
        );
        assert!(reverted.ends_with(&[
            "--resume".to_string(),
            "abc".to_string(),
            "--resume-session-at".to_string(),
            "u-7".to_string()
        ]));
    }

    /// A session's name reaches Claude Code as `--name` on a fresh start and on a resume, and only when it has one.
    #[test]
    fn passes_the_sessions_name_only_when_it_has_one() {
        let name_of = |args: &[String]| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--name=").map(String::from))
        };
        for resume in [false, true] {
            let named = args(
                PermissionMode::Default,
                None,
                None,
                "abc",
                resume,
                None,
                None,
                Some("lead"),
            );
            assert_eq!(name_of(&named), Some("lead".to_string()), "resume: {resume}");
            assert_eq!(named.iter().filter(|arg| arg.starts_with("--name")).count(), 1);
            let unnamed = args(PermissionMode::Default, None, None, "abc", resume, None, None, None);
            assert!(!unnamed.iter().any(|arg| arg.starts_with("--name")), "resume: {resume}");
        }
    }

    #[test]
    fn reports_where_the_main_conversation_stands() {
        let mut translator = Translator::default();
        let cursors = |out: Vec<Out>| -> Vec<String> {
            out.into_iter()
                .filter_map(|out| match out {
                    Out::Cursor(at) => Some(at),
                    _ => None,
                })
                .collect()
        };
        let main = json!({"type":"assistant","uuid":"u-1","parent_tool_use_id":null,"message":{"id":"msg_1","content":[{"type":"text","text":"hi"}]}});
        assert_eq!(cursors(translator.line(&main.to_string())), ["u-1"]);
        let result = json!({"type":"user","uuid":"u-2","parent_tool_use_id":null,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}});
        assert_eq!(cursors(translator.line(&result.to_string())), ["u-2"]);
        // A sub-agent's messages are not the main conversation's.
        let sub = json!({"type":"assistant","uuid":"u-3","parent_tool_use_id":"t1","message":{"id":"msg_2","content":[{"type":"text","text":"x"}]}});
        assert!(cursors(translator.line(&sub.to_string())).is_empty());
    }

    #[test]
    fn reads_a_message_back_when_claude_code_takes_it_in() {
        let mut translator = Translator::default();
        let plain = json!({"type":"user","isReplay":true,"uuid":"u-9","message":{"role":"user","content":"also this"}});
        assert_eq!(
            translator.line(&plain.to_string()),
            [Out::Taken {
                texts: vec!["also this".into()]
            }]
        );
        let blocks = json!({"type":"user","isReplay":true,"message":{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"x"}},{"type":"text","text":"see this"}]}});
        assert_eq!(
            translator.line(&blocks.to_string()),
            [Out::Taken {
                texts: vec!["see this".into()]
            }]
        );
        // A tool's result is not a message read back.
        let result = json!({"type":"user","uuid":"u-2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}});
        assert!(
            !translator
                .line(&result.to_string())
                .iter()
                .any(|out| matches!(out, Out::Taken { .. }))
        );
        assert!(
            args(PermissionMode::Default, None, None, "abc", false, None, None, None)
                .contains(&"--replay-user-messages".to_string())
        );
    }

    /// A task report, for comparing.
    #[allow(clippy::too_many_arguments)]
    fn task_report(
        id: &str,
        tool: Option<&str>,
        kind: Option<&str>,
        agent: Option<&str>,
        status: TaskStatus,
        description: &str,
        usage: (Option<u64>, Option<u64>, Option<u64>),
    ) -> Out {
        Out::Event(WbEvent::Task {
            task_id: id.into(),
            tool_use_id: tool.map(String::from),
            task_type: kind.map(String::from),
            agent_type: agent.map(String::from),
            status,
            description: description.into(),
            tool_count: usage.0,
            total_tokens: usage.1,
            duration_ms: usage.2,
            detail: None,
        })
    }

    /// A task report that says where the task sits.
    fn placed(mut report: Out, place: TaskDetail) -> Out {
        if let Out::Event(WbEvent::Task { detail, .. }) = &mut report {
            *detail = place.some();
        }
        report
    }

    fn running(tasks: &[(&str, &str, &str)]) -> Out {
        Out::Event(WbEvent::BackgroundTasks {
            tasks: tasks
                .iter()
                .map(|(id, kind, description)| BackgroundTask {
                    task_id: (*id).into(),
                    task_type: Some((*kind).into()),
                    description: (*description).into(),
                })
                .collect(),
        })
    }

    #[test]
    fn keeps_the_reported_slash_commands_a_session_can_run() {
        let mut t = Translator::default();
        let init = json!({"type":"system","subtype":"init","session_id":"s","model":"m",
            "slash_commands":["compact","/review","","vercel:deploy","__remote-workflow","doctor","mcp__claude_ai_Notion__make-this-a-notion-page"],
            "terminal_slash_commands":["doctor","color"]});
        assert_eq!(
            lines(&mut t, &[init]),
            vec![
                Out::Init {
                    native_id: "s".into(),
                    model: Some("m".into()),
                    version: None
                },
                Out::Event(WbEvent::Commands {
                    commands: vec![
                        "compact".into(),
                        "review".into(),
                        "vercel:deploy".into(),
                        "mcp__claude_ai_Notion__make-this-a-notion-page".into()
                    ]
                }),
            ]
        );
        // The same list again logs nothing; a changed one is logged.
        let again = json!({"type":"system","subtype":"init","session_id":"s","model":"m",
            "slash_commands":["compact","/review","","vercel:deploy","__remote-workflow","doctor","mcp__claude_ai_Notion__make-this-a-notion-page"],
            "terminal_slash_commands":["doctor","color"]});
        assert!(
            !lines(&mut t, &[again])
                .iter()
                .any(|out| matches!(out, Out::Event(WbEvent::Commands { .. })))
        );
        let changed =
            json!({"type":"system","subtype":"init","session_id":"s","model":"m","slash_commands":["compact"]});
        assert!(lines(&mut t, &[changed]).contains(&Out::Event(WbEvent::Commands {
            commands: vec!["compact".into()]
        })));
    }

    #[test]
    fn reads_task_reports_and_skips_what_it_cannot_place() {
        let mut t = Translator::default();
        let out = lines(
            &mut t,
            &[
                // A patch without an end or a move, a report without an id and a progress without one say nothing
                // kept.
                json!({"type":"system","subtype":"task_updated","task_id":"b1","patch":{"description":"x"}}),
                json!({"type":"system","subtype":"task_started","description":"no id"}),
                json!({"type":"system","subtype":"task_progress","description":"Reading"}),
                json!({"type":"system","subtype":"task_updated","task_id":"b1","patch":{"status":"failed","end_time":1}}),
                json!({"type":"system","subtype":"task_updated","task_id":"b2","patch":{"status":"killed"}}),
                json!({"type":"system","subtype":"task_notification","task_id":"b3","status":"something new","summary":"ok"}),
                json!({"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"","task_type":"local_bash"},{"task_id":"b4"}]}),
            ],
        );
        let out: Vec<Out> = out
            .into_iter()
            .filter(|out| !matches!(out, Out::Background(_)))
            .collect();
        let none = (None, None, None);
        assert_eq!(
            out,
            vec![
                task_report("b1", None, None, None, TaskStatus::Failed, "", none),
                task_report("b2", None, None, None, TaskStatus::Stopped, "", none),
                task_report("b3", None, None, None, TaskStatus::Stopped, "ok", none),
                Out::Event(WbEvent::BackgroundTasks {
                    tasks: vec![BackgroundTask {
                        task_id: "b4".into(),
                        task_type: None,
                        description: String::new()
                    }]
                }),
            ]
        );
    }

    /// Every line of a recorded Claude Code 2.1.287 session that ran a background sub-agent and a background shell.
    #[test]
    fn replays_recorded_background_task_events() {
        let mut t = Translator::default();
        let out: Vec<Out> = include_str!("../tests/fixtures/claude-2.1.287-task-events.jsonl")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .flat_map(|line| t.line(line))
            .filter(|out| !matches!(out, Out::Background(_)))
            .collect();
        let agent = ("acbd762b0ad64aa2c", "toolu_01PQGgvEfKjMJT7GPDLcTvRd");
        let shell = ("b546bebxr", "toolu_01JQRbfKWsms52H3cF1N3aA3");
        let none = (None, None, None);
        let top = TaskDetail {
            depth: Some(1),
            backgrounded: Some(true),
            ..TaskDetail::default()
        };
        assert_eq!(
            out,
            vec![
                running(&[(agent.0, "local_agent", "Reply with pong")]),
                placed(
                    task_report(
                        agent.0,
                        Some(agent.1),
                        Some("local_agent"),
                        Some("general-purpose"),
                        TaskStatus::Running,
                        "Reply with pong",
                        none
                    ),
                    top.clone()
                ),
                running(&[]),
                task_report(agent.0, None, None, None, TaskStatus::Completed, "", none),
                task_report(
                    agent.0,
                    Some(agent.1),
                    None,
                    None,
                    TaskStatus::Completed,
                    "pong",
                    (Some(0), Some(19449), Some(1050))
                ),
                running(&[(shell.0, "local_bash", "Sleep for 2 seconds in the background")]),
                placed(
                    task_report(
                        shell.0,
                        Some(shell.1),
                        Some("local_bash"),
                        None,
                        TaskStatus::Running,
                        "Sleep for 2 seconds in the background",
                        none
                    ),
                    top
                ),
                running(&[]),
                task_report(shell.0, None, None, None, TaskStatus::Completed, "", none),
                task_report(
                    shell.0,
                    Some(shell.1),
                    None,
                    None,
                    TaskStatus::Completed,
                    "Background command \"Sleep for 2 seconds in the background\" completed (exit code 0)",
                    none
                ),
            ]
        );
    }

    /// Synthetic stream-json frames cover a normal turn and authoritative empty command discovery.
    #[test]
    fn translates_a_synthetic_turn_with_empty_discovery() {
        let mut translator = Translator::default();
        let frames = [
            json!({"type":"control_response","response":{"subtype":"success","request_id":"init","response":{}}}),
            json!({"type":"system","subtype":"init","session_id":"synthetic-session","model":"synthetic-model"}),
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"synthetic-message","model":"synthetic-model"}},"parent_tool_use_id":null}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Synthetic answer"}},"parent_tool_use_id":null}),
            json!({"type":"assistant","message":{"id":"synthetic-message","content":[{"type":"text","text":"Synthetic answer"}]},"parent_tool_use_id":null}),
            json!({"type":"assistant","message":{"id":"synthetic-tools","content":[{"type":"tool_use","id":"synthetic-tool","name":"Read","input":{"file_path":"/synthetic/example.txt"}}]},"parent_tool_use_id":null}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"synthetic-tool","content":"Synthetic file"}]},"parent_tool_use_id":null}),
            json!({"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":13,"output_tokens":7}},"parent_tool_use_id":null}),
            json!({"type":"result","subtype":"success","is_error":false,"duration_ms":20,"total_cost_usd":0.01}),
        ];
        let out = lines(&mut translator, &frames);
        assert!(matches!(&out[0], Out::Ack { request_id, error: None } if request_id == "init"));
        assert!(
            matches!(&out[1], Out::Init { native_id, model: Some(model), .. } if native_id == "synthetic-session" && model == "synthetic-model")
        );
        assert!(matches!(&out[2], Out::Event(WbEvent::Commands { commands }) if commands.is_empty()));
        assert!(out.iter().any(|event| matches!(event, Out::Delta { id, text, parent: None } if id == "synthetic-message:0" && text == "Synthetic answer")));
        assert!(
            out.iter()
                .any(|event| matches!(event, Out::Event(WbEvent::Text { text, .. }) if text == "Synthetic answer"))
        );
        assert!(out.iter().any(|event| matches!(event, Out::Event(WbEvent::Tool { name, input, .. }) if name == "Read" && input["file_path"] == "/synthetic/example.txt")));
        assert!(
            out.iter()
                .any(|event| matches!(event, Out::Event(WbEvent::ToolResult { id, .. }) if id == "synthetic-tool"))
        );
        assert!(out.iter().any(|event| matches!(event, Out::Call { model, usage, parent: None } if model == "synthetic-model" && usage.input_tokens == 13 && usage.output_tokens == 7)));
        assert!(matches!(
            out.last(),
            Some(Out::Result {
                duration_ms: 20,
                error: None,
                interrupted: false,
                ..
            })
        ));
    }

    /// A sub-agent streams beside the main answer: each stream keeps its own text, and the sub-agent's says whose it is.
    #[test]
    fn streams_a_sub_agents_text_under_its_parent() {
        let mut t = Translator::default();
        let out = lines(
            &mut t,
            &[
                json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"m","id":"msg_main"}},"parent_tool_use_id":null}),
                json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"m","id":"msg_sub"}},"parent_tool_use_id":"toolu_task"}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Looking"}},"parent_tool_use_id":"toolu_task"}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Main"}},"parent_tool_use_id":null}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" now"}},"parent_tool_use_id":"toolu_task"}),
            ],
        );
        assert_eq!(
            out,
            vec![
                Out::Delta {
                    id: "msg_sub:0".into(),
                    text: "Looking".into(),
                    parent: Some("toolu_task".into())
                },
                Out::Delta {
                    id: "msg_main:0".into(),
                    text: "Main".into(),
                    parent: None
                },
                Out::Delta {
                    id: "msg_sub:0".into(),
                    text: "Looking now".into(),
                    parent: Some("toolu_task".into())
                },
            ]
        );
    }

    #[test]
    fn turns_a_permission_request_into_an_approval() {
        let mut t = Translator::default();
        let out = t.line(
            &json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash",
                "input":{"command":"touch b.txt"},"description":"Create b.txt","tool_use_id":"toolu_9",
                "permission_suggestions":[
                    {"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch:*"}],"behavior":"allow","destination":"localSettings"},
                    {"type":"setMode","mode":"acceptEdits","destination":"session"}]}})
            .to_string(),
        );
        let [
            Out::Event(WbEvent::Approval {
                request_id,
                tool,
                description,
                suggestions,
                tool_use_id,
                ..
            }),
        ] = out.as_slice()
        else {
            panic!("not an approval: {out:?}");
        };
        assert_eq!((request_id.as_str(), tool.as_str()), ("r1", "Bash"));
        assert_eq!(tool_use_id.as_deref(), Some("toolu_9"));
        assert_eq!(description.as_deref(), Some("Create b.txt"));
        let labels: Vec<&str> = suggestions.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Always allow Bash(touch:*) in this project",
                "Allow all edits for this session"
            ]
        );
    }

    #[test]
    fn questions_without_codex_ids_reach_the_claude_approval_path() {
        let input = json!({"questions":[{"question":"Pick a synthetic option", "options":[{"label":"A"}]}]});
        let mut translator = Translator::default();
        let out = translator.line(
            &json!({
                "type":"control_request", "request_id":"question-test",
                "request":{"subtype":"can_use_tool", "tool_name":"AskUserQuestion", "input":input}
            })
            .to_string(),
        );
        assert!(
            matches!(out.as_slice(), [Out::Event(WbEvent::Approval { tool, input: actual, .. })]
            if tool == "AskUserQuestion" && actual == &input)
        );
    }

    #[test]
    fn reports_an_interrupted_and_a_failed_turn() {
        let mut t = Translator::default();
        let interrupted = t.line(
            &json!({"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":5}).to_string(),
        );
        assert!(matches!(
            interrupted.as_slice(),
            [Out::Result {
                interrupted: true,
                error: None,
                ..
            }]
        ));
        let failed = t.line(&json!({"type":"result","subtype":"success","is_error":true,"result":"API Error: 402 budget exceeded","duration_ms":5}).to_string());
        assert!(matches!(failed.as_slice(), [Out::Result { error: Some(e), .. }] if e.contains("402")));
    }

    #[test]
    fn answers_and_caps() {
        let allow = allow(
            "r1",
            &json!({"questions": []}),
            Some(&Map::from_iter([("Q?".into(), json!("Yes"))])),
            Some(&json!({"type": "setMode"})),
        );
        let allow: Value = serde_json::from_str(&allow).unwrap_or_default();
        assert_eq!(
            allow.pointer("/response/response/updatedInput/answers/Q?"),
            Some(&json!("Yes"))
        );
        assert_eq!(
            allow.pointer("/response/response/updatedPermissions/0/type"),
            Some(&json!("setMode"))
        );
        let (text, truncated) = cap("é".repeat(MAX_TOOL_OUTPUT));
        assert!(truncated && text.len() <= MAX_TOOL_OUTPUT);
        // Three bytes a character: the limit falls inside one, so the cut walks back to the boundary before it.
        let (text, truncated) = cap("€".repeat(MAX_TOOL_OUTPUT));
        assert!(truncated && text.len() == MAX_TOOL_OUTPUT - MAX_TOOL_OUTPUT % 3);
    }

    #[test]
    fn sends_images_before_the_text() {
        let line =
            |text, images: &[Image]| serde_json::from_str::<Value>(&user_message(text, images)).unwrap_or_default();
        assert_eq!(line("hi", &[]).pointer("/message/content"), Some(&json!("hi")));
        let shot = Image {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
            name: Some("shot.png".into()),
        };
        assert_eq!(
            line("what's wrong here?", std::slice::from_ref(&shot)).pointer("/message/content"),
            Some(&json!([
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
                {"type": "text", "text": "what's wrong here?"},
            ]))
        );
        assert_eq!(line("", &[shot]).pointer("/message/content/1"), None);
        // A message stored before images existed still reads.
        let old: WbEvent =
            serde_json::from_value(json!({"kind": "user", "text": "hi"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            old,
            WbEvent::User {
                text: "hi".into(),
                images: vec![],
                from: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&old).ok(),
            Some(json!({"kind": "user", "text": "hi"}))
        );
    }

    /// Every line of a Claude Code 2.1.292 recording (tests/fixtures), through one translator.
    fn replay(recorded: &str) -> Vec<Out> {
        let mut t = Translator::default();
        recorded
            .lines()
            .filter(|line| !line.trim().is_empty())
            .flat_map(|line| t.line(line))
            .collect()
    }

    /// Each task report's id and status, in order.
    fn reports(out: &[Out]) -> Vec<(&str, TaskStatus)> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(WbEvent::Task { task_id, status, .. }) => Some((task_id.as_str(), *status)),
                _ => None,
            })
            .collect()
    }

    /// Where in `out` a task is first reported with `status`.
    fn reported(out: &[Out], task: &str, status: TaskStatus) -> Option<usize> {
        out.iter().position(|out| {
            matches!(out,
            Out::Event(WbEvent::Task { task_id, status: s, .. }) if task_id == task && *s == status)
        })
    }

    fn parent_of_call<'a>(out: &'a [Out], call: &str) -> Option<Option<&'a str>> {
        out.iter().find_map(|out| match out {
            Out::Event(WbEvent::Tool { id, parent, .. }) if id == call => Some(parent.as_deref()),
            _ => None,
        })
    }

    /// The answer to our control request `request`: where it is, and its error.
    fn ack<'a>(out: &'a [Out], request: &str) -> Option<(usize, &'a Option<String>)> {
        out.iter().enumerate().find_map(|(at, out)| match out {
            Out::Ack { request_id, error } if request_id == request => Some((at, error)),
            _ => None,
        })
    }

    /// The first detail a report of the task with `status` carries.
    fn detail<'a>(out: &'a [Out], task: &str, status: TaskStatus) -> Option<&'a TaskDetail> {
        out.iter().find_map(|out| match out {
            Out::Event(WbEvent::Task {
                task_id,
                status: s,
                detail,
                ..
            }) if task_id == task && *s == status => Some(detail.as_deref()?),
            _ => None,
        })
    }

    /// The depth each task's start says, by task id.
    fn depths(out: &[Out]) -> Vec<(&str, Option<u32>)> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(WbEvent::Task {
                    task_id,
                    status: TaskStatus::Running,
                    detail,
                    ..
                }) => Some((task_id.as_str(), detail.as_ref().and_then(|d| d.depth))),
                _ => None,
            })
            .collect()
    }

    /// The parent an approval names.
    fn asker<'a>(out: &'a [Out], request: &str) -> Option<Option<&'a str>> {
        out.iter().find_map(|out| match out {
            Out::Event(WbEvent::Approval { request_id, parent, .. }) if request_id == request => {
                Some(parent.as_deref())
            }
            _ => None,
        })
    }

    /// The parent a tool result names.
    fn answered_under<'a>(out: &'a [Out], call: &str) -> Option<Option<&'a str>> {
        out.iter().find_map(|out| match out {
            Out::Event(WbEvent::ToolResult { id, parent, .. }) if id == call => Some(parent.as_deref()),
            _ => None,
        })
    }

    /// A sub-agent that starts its own, recorded from Claude Code 2.1.292 with today's bare `initialize`. Neither
    /// Agent call sets `run_in_background`, so both run in the background: the turn ends before either sub-agent does,
    /// and Claude Code runs a turn of its own as each one ends.
    #[test]
    fn replays_recorded_nested_sub_agents() {
        let out = replay(include_str!("../tests/fixtures/claude-2.1.292-nested-agents.jsonl"));
        let (outer, outer_call) = ("aae842cb2af332d8a", "toolu_013SvT4b4ovV5umd4BrwfD2G");
        let (inner, inner_call) = ("aa70530138911738d", "toolu_01PxuRAJUBGMegYvCkuHGkvB");
        // The tree is in the frames: the inner Agent call sits under the outer one, its command under it.
        assert_eq!(parent_of_call(&out, outer_call), Some(None));
        assert_eq!(parent_of_call(&out, inner_call), Some(Some(outer_call)));
        assert_eq!(
            parent_of_call(&out, "toolu_01FF71vVZW4cXu9WZ3wmJEer"),
            Some(Some(inner_call))
        );
        // Both sub-agents' text arrives, with its parent, without forwardSubagentText.
        for call in [outer_call, inner_call] {
            assert!(out.iter().any(|out| matches!(out,
                Out::Event(WbEvent::Text { parent: Some(parent), .. }) if parent == call)));
        }
        let first_end = out.iter().position(|out| matches!(out, Out::Result { .. }));
        for task in [outer, inner] {
            let ended = reported(&out, task, TaskStatus::Completed);
            assert!(
                matches!((first_end, ended), (Some(end), Some(done)) if end < done),
                "{task}"
            );
        }
        // Only the main conversation streams: its four model calls, and none of a sub-agent's.
        let calls: Vec<_> = out
            .iter()
            .filter_map(|out| match out {
                Out::Call { parent, .. } => Some(parent.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, [None; 4]);
        assert_eq!(out.iter().filter(|out| matches!(out, Out::Result { .. })).count(), 3);
        // Each start says where the sub-agent sits, and each result whose sub-agent's call it answers.
        assert_eq!(
            detail(&out, outer, TaskStatus::Running),
            Some(&TaskDetail {
                depth: Some(1),
                backgrounded: Some(true),
                run_id: Some("0mvo2cpp2-76b46edf".into()),
                ..TaskDetail::default()
            })
        );
        assert_eq!(
            detail(&out, inner, TaskStatus::Running),
            Some(&TaskDetail {
                parent_task_id: Some(outer.into()),
                depth: Some(2),
                backgrounded: Some(true),
                run_id: Some("0mvo2crlm-93605f90".into()),
                ..TaskDetail::default()
            })
        );
        // Every row names its run; only the start says where the task sits.
        assert_eq!(
            detail(&out, outer, TaskStatus::Completed),
            Some(&TaskDetail {
                run_id: Some("0mvo2cpp2-76b46edf".into()),
                ..TaskDetail::default()
            })
        );
        assert_eq!(answered_under(&out, outer_call), Some(None));
        assert_eq!(answered_under(&out, inner_call), Some(Some(outer_call)));
        assert_eq!(
            answered_under(&out, "toolu_01FF71vVZW4cXu9WZ3wmJEer"),
            Some(Some(inner_call))
        );
    }

    /// A sub-agent's command that needs the person's approval (Claude Code 2.1.292). The request names the
    /// sub-agent in `agent_id`; the approval names its Agent call, the parent of the tool call it asks about.
    #[test]
    fn replays_a_recorded_sub_agent_approval() {
        let recorded = include_str!("../tests/fixtures/claude-2.1.292-subagent-approval.jsonl");
        let out = replay(recorded);
        let asked = out.iter().find_map(|out| match out {
            Out::Event(WbEvent::Approval { tool, tool_use_id, .. }) => Some((tool.as_str(), tool_use_id.clone())),
            _ => None,
        });
        let command = "toolu_0145jHhrfH6yfd8t29GGnpVx";
        assert_eq!(asked, Some(("Bash", Some(command.into()))));
        assert_eq!(
            parent_of_call(&out, command),
            Some(Some("toolu_018cADHaYS3W5soeLdQ6GGFk"))
        );
        let request = recorded
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| line.pointer("/request/subtype") == Some(&json!("can_use_tool")));
        assert_eq!(
            request.and_then(|r| r.pointer("/request/agent_id").cloned()),
            Some(json!("ae032da541e2890ec"))
        );
        assert_eq!(
            asker(&out, "a5d80804-db4c-4486-a5a1-790d43976a63"),
            Some(Some("toolu_018cADHaYS3W5soeLdQ6GGFk"))
        );
    }

    /// A background shell read and stopped through control requests (Claude Code 2.1.292): `nexus-out-1` is
    /// `get_task_output` on the shell, `nexus-stop-1` and `nexus-stop-2` stop it (the second after it ended), and
    /// `nexus-stop-3` stops a task id that does not exist. Every one succeeds.
    #[test]
    fn replays_recorded_shell_controls() {
        let out = replay(include_str!("../tests/fixtures/claude-2.1.292-shell-controls.jsonl"));
        for request in ["nexus-out-1", "nexus-stop-1", "nexus-stop-2", "nexus-stop-3"] {
            assert_eq!(ack(&out, request).map(|(_, error)| error), Some(&None), "{request}");
        }
        let shell: Vec<_> = reports(&out).into_iter().filter(|(id, _)| *id == "bsw0d7nv9").collect();
        assert_eq!(
            shell,
            [
                ("bsw0d7nv9", TaskStatus::Running),
                ("bsw0d7nv9", TaskStatus::Stopped),
                ("bsw0d7nv9", TaskStatus::Stopped)
            ]
        );
    }

    /// Claude Code 2.1.292: `send_task_message` refused (`nexus-msg-1`); a stop of a sub-agent that already ended
    /// (`nexus-stop-4`, on `parent`); `child` run again under its task id; and a stop that cascades (`nexus-stop-5`, on
    /// `boss`) to its foreground sub-agent `worker` and that one's command.
    #[test]
    fn replays_a_recorded_stop_cascade() {
        let out = replay(include_str!("../tests/fixtures/claude-2.1.292-stop-cascade.jsonl"));
        assert_eq!(
            ack(&out, "nexus-msg-1").map(|(_, error)| error),
            Some(&Some("Unsupported control request subtype: send_task_message".into()))
        );
        assert_eq!(ack(&out, "nexus-stop-4").map(|(_, error)| error), Some(&None));
        let stopped = |task: &str| reported(&out, task, TaskStatus::Stopped).is_some();
        assert!(!stopped("a943332f1fdb58c0d"), "{:?}", reports(&out));
        let runs = reports(&out)
            .into_iter()
            .filter(|report| *report == ("aee2692b1951225af", TaskStatus::Running))
            .count();
        assert_eq!(runs, 2);
        for task in ["a1eb7ac1a9dbdf5cd", "a98124af05351e400", "bab3q5kpk"] {
            assert!(stopped(task), "{task}: {:?}", reports(&out));
        }
        assert_eq!(ack(&out, "nexus-stop-5").map(|(_, error)| error), Some(&None));
        // `worker` is a foreground sub-agent whose frames Claude Code does not forward: its approval still names it.
        assert_eq!(
            asker(&out, "455389c6-67bb-4921-84a3-de9324a48b7b"),
            Some(Some("toolu_016sQ7sYG5gBLN1a8YZmtquo"))
        );
        // A sub-agent's shells sit one below it. `child`'s second run has no parent task: it keeps its first one's.
        assert_eq!(
            depths(&out),
            [
                ("a943332f1fdb58c0d", Some(1)),
                ("aee2692b1951225af", Some(2)),
                ("blqjiskbv", Some(3)),
                ("a1eb7ac1a9dbdf5cd", Some(1)),
                ("a98124af05351e400", Some(2)),
                ("bab3q5kpk", Some(3)),
                ("aee2692b1951225af", Some(2)),
            ]
        );
        let rerun = out.iter().rev().find_map(|out| match out {
            Out::Event(WbEvent::Task {
                task_id,
                status: TaskStatus::Running,
                detail,
                ..
            }) if task_id == "aee2692b1951225af" => detail.as_deref(),
            _ => None,
        });
        assert_eq!(
            rerun.map(|d| (d.parent_task_id.as_deref(), d.run_id.as_deref())),
            Some((None, Some("0mvo2iqo1-a72b64a8")))
        );
    }

    /// A stop is asked as `stop_task`, and its answer comes back as `Out::Answered`, success or refusal, never as an
    /// `Out::Ack`. Another request's answer stays an `Out::Ack`. The version comes with `init`.
    #[test]
    fn a_stop_is_asked_and_answered() {
        let mut t = Translator::default();
        let said = t.say(Say::TaskStop {
            request: "nexus-7".into(),
            task_id: "a1".into(),
        });
        let [Out::Reply(line)] = said.as_slice() else {
            panic!("{said:?}");
        };
        let line: Value = serde_json::from_str(line).unwrap_or_default();
        assert_eq!(line["request"], json!({"subtype": "stop_task", "task_id": "a1"}));
        assert_eq!(line["request_id"], "nexus-7");
        t.say(Say::TaskStop {
            request: "nexus-8".into(),
            task_id: "a2".into(),
        });
        let out = lines(
            &mut t,
            &[
                json!({"type":"control_response","response":{"subtype":"success","request_id":"nexus-7","response":{}}}),
                json!({"type":"control_response","response":{"subtype":"error","request_id":"nexus-8","error":"busy"}}),
                json!({"type":"control_response","response":{"subtype":"success","request_id":"nexus-9","response":{}}}),
                json!({"type":"control_response","response":{"subtype":"success","request_id":"nexus-7","response":{}}}),
                json!({"type":"system","subtype":"init","session_id":"s","claude_code_version":"2.1.292"}),
            ],
        );
        assert_eq!(
            out[..4],
            [
                Out::Answered {
                    request_id: "nexus-7".into(),
                    result: Ok(json!({}))
                },
                Out::Answered {
                    request_id: "nexus-8".into(),
                    result: Err("busy".into())
                },
                Out::Ack {
                    request_id: "nexus-9".into(),
                    error: None
                },
                // An answer comes once: a repeat is an answer to nothing we wait for.
                Out::Ack {
                    request_id: "nexus-7".into(),
                    error: None
                },
            ]
        );
        assert!(
            matches!(&out[4], Out::Init { version: Some(v), .. } if v == "2.1.292"),
            "{:?}",
            out[4]
        );
    }

    /// A move to the background and an output read are asked as Claude Code 2.1.292 was recorded taking them, and
    /// their answers come back whole as `Out::Answered`.
    #[test]
    fn a_move_and_a_read_are_asked_and_answered() {
        let mut t = Translator::default();
        let request = |out: Vec<Out>| match out.as_slice() {
            [Out::Reply(line)] => serde_json::from_str::<Value>(line).unwrap_or_default()["request"].clone(),
            other => panic!("{other:?}"),
        };
        let moved = t.say(Say::TaskBackground {
            request: "nexus-1".into(),
            tool_use_id: "toolu_1".into(),
        });
        assert_eq!(
            request(moved),
            json!({"subtype": "background_tasks", "tool_use_id": "toolu_1"})
        );
        let read = t.say(Say::TaskOutput {
            request: "nexus-2".into(),
            task_id: "b1".into(),
        });
        assert_eq!(request(read), json!({"subtype": "get_task_output", "task_id": "b1"}));
        let out = lines(
            &mut t,
            &[
                json!({"type":"control_response","response":{"subtype":"success","request_id":"nexus-1","response":{"backgrounded":true}}}),
                json!({"type":"control_response","response":{"subtype":"success","request_id":"nexus-2","response":{"output":"tick 1\n","total_bytes":7,"truncated":false}}}),
            ],
        );
        assert_eq!(
            out,
            [
                Out::Answered {
                    request_id: "nexus-1".into(),
                    result: Ok(json!({"backgrounded": true}))
                },
                Out::Answered {
                    request_id: "nexus-2".into(),
                    result: Ok(json!({"output": "tick 1\n", "total_bytes": 7, "truncated": false}))
                },
            ]
        );
    }

    /// `initialize` declares `perTaskStopAffordance` only when it is on (`PER_TASK_STOP`), as the recordings sent it.
    #[test]
    fn initialize_declares_per_task_stop_only_when_on() {
        let request = |line: String| serde_json::from_str::<Value>(&line).unwrap_or_default()["request"].clone();
        let hooks = json!({"PreToolUse": [{"matcher": "^mcp__", "hookCallbackIds": [crate::browser_routing::HOOK]}],
            "SubagentStart": [{"hookCallbackIds": [SUBAGENT_HOOK]}]});
        assert_eq!(
            request(initialize_with(false)),
            json!({"subtype": "initialize", "hooks": hooks})
        );
        assert_eq!(
            request(initialize_with(true)),
            json!({"subtype": "initialize", "hooks": hooks, "perTaskStopAffordance": true})
        );
        assert_eq!(initialize(), initialize_with(PER_TASK_STOP));
    }

    /// A sub-agent learns of `terminal_run` only when its run has the shared terminal tools.
    #[test]
    fn a_subagent_is_told_of_the_terminal_only_when_the_run_has_it() {
        let start = |t: &mut Translator| {
            let out = t.line(
                &json!({"type": "control_request", "request_id": "s1", "request": {"subtype": "hook_callback",
                    "callback_id": SUBAGENT_HOOK, "input": {"hook_event_name": "SubagentStart"}}})
                .to_string(),
            );
            let [Out::Reply(reply)] = out.as_slice() else {
                panic!("one reply: {out:?}")
            };
            serde_json::from_str::<Value>(reply).unwrap_or_default()["response"]["response"].clone()
        };
        let init = |tools: Value| {
            json!({"type": "system", "subtype": "init", "session_id": "s", "model": "m", "tools": tools}).to_string()
        };
        let mut t = Translator::default();
        t.line(&init(json!(["Bash"])));
        assert_eq!(start(&mut t), json!({}));
        t.line(&init(json!(["mcp__nexus_app__terminal_run"])));
        let said = start(&mut t);
        assert_eq!(said["hookSpecificOutput"]["hookEventName"], "SubagentStart");
        assert!(
            said["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .is_some_and(|c| c.contains("terminal_run"))
        );
    }

    /// The browser hook (recorded shape from Claude Code 2.1.323 in Full access) refuses an outside browser tool while
    /// the run has the Nexus browser tools, until the agent records a bypass.
    #[test]
    fn the_browser_hook_refuses_outside_browser_tools_until_a_bypass() {
        let mut t = Translator::default();
        let hook = |t: &mut Translator, tool: &str| {
            let out = t.line(
                &json!({"type": "control_request", "request_id": "h1", "request": {"subtype": "hook_callback",
                    "callback_id": crate::browser_routing::HOOK, "input": {"hook_event_name": "PreToolUse",
                    "permission_mode": "bypassPermissions", "tool_name": tool, "tool_input": {}}, "tool_use_id": "toolu_1"}})
                .to_string(),
            );
            let [Out::Reply(reply)] = out.as_slice() else {
                panic!("one reply: {out:?}")
            };
            let reply: Value = serde_json::from_str(reply).unwrap_or_default();
            assert_eq!(reply["response"]["request_id"], "h1");
            reply["response"]["response"].clone()
        };
        let outside = "mcp__playwright__browser_navigate";
        assert_eq!(hook(&mut t, outside), json!({}));
        t.line(
            &json!({"type": "system", "subtype": "init", "session_id": "s", "model": "m",
                "tools": ["Bash", "mcp__nexus_browser__browser_open", "mcp__playwright__browser_navigate"]})
            .to_string(),
        );
        assert_eq!(
            hook(&mut t, outside)["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
        assert_eq!(hook(&mut t, "mcp__nexus_browser__browser_open"), json!({}));
        t.line(
            &json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": "m1", "content": [
                {"type": "tool_use", "id": "toolu_b", "name": "mcp__nexus_browser__browser_bypass",
                    "input": {"capability": "network log", "reason": "The step must read the failed request's headers."}}
            ]}})
            .to_string(),
        );
        assert_eq!(hook(&mut t, outside), json!({}));
    }

    /// Claude Code 2.1.292 with today's bare `initialize`: `get_task_output` refuses a sub-agent (`nexus-out-agent`).
    /// Stopping `boss` (`nexus-stop-boss`) stops its own command. Stopping `chief` (`nexus-stop-chief`) stops its
    /// own command, and its background sub-agent `helper` runs on to its end.
    #[test]
    fn replays_a_recorded_stop_that_spares_a_background_sub_agent() {
        let out = replay(include_str!(
            "../tests/fixtures/claude-2.1.292-stop-spares-background.jsonl"
        ));
        assert_eq!(
            ack(&out, "nexus-out-agent").map(|(_, error)| error),
            Some(&Some(
                "get_task_output: no shell or Monitor task with that task_id in this session".into()
            ))
        );
        assert_eq!(ack(&out, "nexus-stop-boss").map(|(_, error)| error), Some(&None));
        for task in ["a1a435ed6908484cc", "b7u69exql", "a5da8d287240e9083", "bdio6dsk8"] {
            assert!(
                reported(&out, task, TaskStatus::Stopped).is_some(),
                "{task}: {:?}",
                reports(&out)
            );
        }
        let stop = ack(&out, "nexus-stop-chief").map(|(at, _)| at);
        let helper_done = reported(&out, "a2eb21571acffa9c3", TaskStatus::Completed);
        assert!(
            matches!((stop, helper_done), (Some(stop), Some(done)) if stop < done),
            "{:?}",
            reports(&out)
        );
        assert_eq!(reported(&out, "a2eb21571acffa9c3", TaskStatus::Stopped), None);
    }

    /// A foreground command moved to the background (`background_tasks`, `nexus-bg-1`), then an interrupt with
    /// `perTaskStopAffordance` on: the turn's command stops, the background one runs to its end (2.1.292).
    #[test]
    fn replays_a_recorded_move_to_the_background() {
        let out = replay(include_str!(
            "../tests/fixtures/claude-2.1.292-move-to-background.jsonl"
        ));
        let (moved, error) = ack(&out, "nexus-bg-1").unwrap_or_else(|| panic!("no answer to nexus-bg-1"));
        assert_eq!(error, &None);
        // What the idle timer counts as running, up to the answer: the moved command.
        let mut running = std::collections::HashSet::new();
        for out in &out[..moved] {
            match out {
                Out::Background(Background::Started(id)) => {
                    running.insert(id.clone());
                }
                Out::Background(Background::Ended(id)) => {
                    running.remove(id);
                }
                Out::Background(Background::Now(ids)) => running = ids.iter().cloned().collect(),
                _ => {}
            }
        }
        assert!(running.contains("b1v9an1oo"), "{running:?}");
        assert!(out.iter().any(|out| matches!(out,
            Out::Event(WbEvent::ToolResult { id, output, .. })
                if id == "toolu_01F1HV6HfZ9Fa7Yzg3mZZnBD" && output.starts_with("Command was manually backgrounded"))));
        assert_eq!(ack(&out, "nexus-int-1").map(|(_, error)| error), Some(&None));
        assert!(reported(&out, "bfljjwofh", TaskStatus::Stopped).is_some());
        assert!(reported(&out, "b1v9an1oo", TaskStatus::Completed).is_some());
    }

    /// Interrupts recorded from Claude Code 2.1.292. With today's bare `initialize`, one interrupt leaves an earlier
    /// turn's background shell running, and another stops the background sub-agent its own turn started. With
    /// `perTaskStopAffordance`, an interrupt stops the turn's command and the background sub-agent runs on to its end.
    #[test]
    fn replays_recorded_interrupts() {
        let bare = replay(include_str!("../tests/fixtures/claude-2.1.292-interrupt.jsonl"));
        assert!(reported(&bare, "b7mftbexs", TaskStatus::Completed).is_some());
        assert!(reported(&bare, "a88836eee8f314c2d", TaskStatus::Stopped).is_some());
        // The session's own shells are at the top; the sub-agent's is one below it. Its approval names it, and the
        // session's own approvals name no one.
        assert_eq!(
            depths(&bare),
            [
                ("b7mftbexs", Some(1)),
                ("bqj2a2luz", Some(1)),
                ("a88836eee8f314c2d", Some(1)),
                ("bzhmr2695", Some(1)),
                ("b664xd1k2", Some(2)),
            ]
        );
        assert_eq!(
            detail(&bare, "bqj2a2luz", TaskStatus::Running).and_then(|d| d.backgrounded),
            Some(false)
        );
        assert_eq!(
            asker(&bare, "4cbd9ccd-6212-4493-bd5f-61c704305c56"),
            Some(Some("toolu_018RQJz9PJu41pYLsg1tNCMF"))
        );
        assert_eq!(asker(&bare, "1ed653fe-a31f-406e-824a-8d88de27251a"), Some(None));
        let spared = replay(include_str!(
            "../tests/fixtures/claude-2.1.292-interrupt-per-task-stop.jsonl"
        ));
        let interrupt = ack(&spared, "nexus-int-1").map(|(at, _)| at);
        assert!(reported(&spared, "b3tdorlzc", TaskStatus::Stopped).is_some());
        let looper = reported(&spared, "a0819a8b0bc963eaa", TaskStatus::Completed);
        assert!(
            matches!((interrupt, looper), (Some(i), Some(done)) if i < done),
            "{:?}",
            reports(&spared)
        );
        assert_eq!(reported(&spared, "a0819a8b0bc963eaa", TaskStatus::Stopped), None);
    }

    /// What a report says that no recording shows: an approval from a sub-agent whose start was never read names its
    /// task id; a shell of a sub-agent never seen has no depth; an end says why it ended (`worker_restart`), and a
    /// start or an update never does.
    #[test]
    fn places_what_it_cannot_link() {
        let mut t = Translator::default();
        let out = lines(
            &mut t,
            &[
                json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool",
                    "tool_name": "Bash", "input": {}, "tool_use_id": "toolu_x", "agent_id": "a-unseen"}}),
                json!({"type": "system", "subtype": "task_started", "task_id": "b1", "tool_use_id": "toolu_x",
                    "task_type": "local_bash", "description": "ls", "is_backgrounded": false,
                    "owned_by_subagent": true, "parent_task_id": "a-unseen", "run_id": "r-b1"}),
                json!({"type": "system", "subtype": "task_started", "task_id": "b2", "tool_use_id": "toolu_y",
                    "task_type": "local_bash", "description": "ls", "owned_by_subagent": true}),
                json!({"type": "system", "subtype": "task_started", "task_id": "a1", "tool_use_id": "toolu_a",
                    "task_type": "local_agent", "description": "d", "reason": "worker_restart"}),
                json!({"type": "system", "subtype": "task_notification", "task_id": "a1", "tool_use_id": "toolu_a",
                    "status": "stopped", "summary": "s", "reason": "worker_restart"}),
            ],
        );
        assert_eq!(asker(&out, "r1"), Some(Some("a-unseen")));
        assert_eq!(depths(&out), [("b1", None), ("b2", None), ("a1", None)]);
        assert_eq!(
            detail(&out, "b1", TaskStatus::Running).map(|d| d.parent_task_id.as_deref()),
            Some(Some("a-unseen"))
        );
        assert_eq!(
            detail(&out, "a1", TaskStatus::Running).and_then(|d| d.reason.as_deref()),
            None
        );
        assert_eq!(
            detail(&out, "a1", TaskStatus::Stopped),
            Some(&TaskDetail {
                reason: Some("worker_restart".into()),
                ..TaskDetail::default()
            })
        );
    }

    /// The task rows a recording leaves in the log: each line one second after the one before, through the core's
    /// book as lib.rs routes them (tool rows tell it whose work shows; a turn's end writes what it holds).
    fn logged(recorded: &str) -> Vec<WbEvent> {
        use crate::agents::{Take, TaskBook};
        let (mut t, mut book, start) = (Translator::default(), TaskBook::default(), std::time::Instant::now());
        let mut rows = Vec::new();
        for (n, line) in recorded.lines().filter(|line| !line.trim().is_empty()).enumerate() {
            let now = start + std::time::Duration::from_secs(n as u64);
            for out in t.line(line) {
                let report = match out {
                    Out::Event(WbEvent::Task { .. }) | Out::Progress(_) => {
                        let (Out::Event(event) | Out::Progress(event)) = out else {
                            continue;
                        };
                        rows.extend(book.due(Some(now)));
                        Some(event)
                    }
                    Out::Event(WbEvent::Tool { parent: Some(call), .. }) => {
                        book.saw_work_of(&call);
                        None
                    }
                    Out::Event(WbEvent::ToolResult { id, .. }) => {
                        book.result_of(&id);
                        None
                    }
                    Out::Result { .. } => {
                        rows.extend(book.due(None));
                        None
                    }
                    _ => None,
                };
                if let Some(event) = report
                    && let Take::Write(row) = book.take(event, now)
                {
                    rows.push(*row);
                }
            }
        }
        rows
    }

    /// A row's task, status, run and whether it is an update.
    fn row(event: &WbEvent) -> Option<(&str, TaskStatus, Option<&TaskDetail>)> {
        match event {
            WbEvent::Task {
                task_id,
                status,
                detail,
                ..
            } => Some((task_id.as_str(), *status, detail.as_deref())),
            _ => None,
        }
    }

    fn rows_of<'a>(rows: &'a [WbEvent], task: &str) -> Vec<(TaskStatus, Option<&'a TaskDetail>)> {
        rows.iter()
            .filter_map(row)
            .filter(|(id, ..)| *id == task)
            .map(|(_, status, detail)| (status, detail))
            .collect()
    }

    fn is_update(detail: Option<&TaskDetail>) -> bool {
        detail.and_then(|d| d.progress) == Some(true)
    }

    const NESTED: &str = include_str!("../tests/fixtures/claude-2.1.292-nested-agents.jsonl");
    const INTERRUPT: &str = include_str!("../tests/fixtures/claude-2.1.292-interrupt.jsonl");
    const MOVED: &str = include_str!("../tests/fixtures/claude-2.1.292-move-to-background.jsonl");
    const CASCADE: &str = include_str!("../tests/fixtures/claude-2.1.292-stop-cascade.jsonl");

    /// A command in the foreground leaves no task row: its tool row shows it. One moved to the background gets its
    /// start row then.
    #[test]
    fn the_log_keeps_no_row_for_a_recorded_foreground_command() {
        let (interrupt, cascade, moved) = (logged(INTERRUPT), logged(CASCADE), logged(MOVED));
        for (rows, task) in [
            (&interrupt, "bqj2a2luz"),
            (&interrupt, "bzhmr2695"),
            (&interrupt, "b664xd1k2"),
            (&cascade, "bab3q5kpk"),
            (&moved, "bfljjwofh"),
        ] {
            assert_eq!(rows_of(rows, task), [], "{task}");
        }
        let statuses: Vec<_> = rows_of(&interrupt, "b7mftbexs")
            .into_iter()
            .map(|(status, _)| status)
            .collect();
        assert_eq!(
            statuses,
            [TaskStatus::Running, TaskStatus::Completed, TaskStatus::Completed]
        );
        let first = moved
            .iter()
            .find(|event| row(event).is_some_and(|(id, ..)| id == "b1v9an1oo"));
        assert!(
            matches!(first, Some(WbEvent::Task { tool_use_id: Some(call), status: TaskStatus::Running, detail: Some(d), .. })
                if call == "toolu_01F1HV6HfZ9Fa7Yzg3mZZnBD" && d.backgrounded == Some(true)),
            "{first:?}"
        );
    }

    /// `child` runs twice under one task id: two starts, each with its own run, both under `parent`.
    #[test]
    fn the_log_keeps_a_recorded_re_run_apart() {
        let rows = logged(CASCADE);
        let starts: Vec<_> = rows_of(&rows, "aee2692b1951225af")
            .into_iter()
            .filter(|(status, detail)| *status == TaskStatus::Running && !is_update(*detail))
            .filter_map(|(_, detail)| detail.map(|d| (d.run_id.as_deref(), d.parent_task_id.as_deref())))
            .collect();
        let parent = Some("a943332f1fdb58c0d");
        assert_eq!(
            starts,
            [
                (Some("0mvo2gq2m-0cdb5277"), parent),
                (Some("0mvo2iqo1-a72b64a8"), parent)
            ]
        );
    }

    /// A sub-agent's progress reaches the log at most once per interval (a hidden one's every 5 seconds, here each
    /// line a second apart), the model its frames named rides one row, and nothing of a run follows its end.
    #[test]
    fn the_log_keeps_recorded_progress_lean() {
        let nested = logged(NESTED);
        let outer = rows_of(&nested, "aae842cb2af332d8a");
        assert_eq!(outer.len(), 4, "{outer:?}");
        assert!(is_update(outer[1].1));
        let detail = outer[1].1.cloned().unwrap_or_default();
        assert_eq!(
            (detail.activity.as_deref(), detail.model.as_deref()),
            (Some("inner"), Some("claude-haiku-4-5-20251001"))
        );
        for task in ["aae842cb2af332d8a", "aa70530138911738d"] {
            let models = rows_of(&nested, task)
                .into_iter()
                .filter(|(_, d)| d.and_then(|d| d.model.as_ref()).is_some())
                .count();
            assert_eq!(models, 1, "{task}");
        }
        // `worker`'s frames never reach the log: its progress does, while `boss`'s waits for the turn's end.
        let cascade = logged(CASCADE);
        let worker = rows_of(&cascade, "a98124af05351e400");
        assert!(worker.iter().any(|(_, d)| {
            is_update(*d)
                && d.and_then(|d| d.activity.as_deref())
                    .is_some_and(|a| a.starts_with("Running for i"))
        }));
        for rows in [&nested, &logged(INTERRUPT), &logged(MOVED), &cascade] {
            let mut ended = std::collections::HashSet::new();
            for (task, status, detail) in rows.iter().filter_map(row) {
                match status {
                    TaskStatus::Running if is_update(detail) => {
                        assert!(!ended.contains(task), "{task}: progress after its end")
                    }
                    TaskStatus::Running => {
                        ended.remove(task);
                    }
                    _ => {
                        ended.insert(task);
                    }
                }
            }
        }
    }

    /// Claude Code 2.1.292's progress, its move to the background and a sub-agent's model are updates of a running
    /// task: no type, no call, no description.
    #[test]
    fn reads_recorded_updates() {
        let updates = |recorded: &str| -> Vec<WbEvent> {
            replay(recorded)
                .into_iter()
                .filter_map(|out| match out {
                    Out::Progress(event) => Some(event),
                    _ => None,
                })
                .collect()
        };
        let nested = updates(NESTED);
        assert!(nested.iter().all(|event| matches!(event,
            WbEvent::Task { tool_use_id: None, task_type: None, description, status: TaskStatus::Running, .. } if description.is_empty())));
        let inner: Vec<_> = nested
            .iter()
            .filter_map(row)
            .filter(|(id, ..)| *id == "aa70530138911738d")
            .collect();
        assert_eq!(inner.len(), 2);
        assert_eq!(
            inner[0].2.and_then(|d| d.model.as_deref()),
            Some("claude-haiku-4-5-20251001")
        );
        let progress = inner[1].2.cloned().unwrap_or_default();
        assert_eq!(
            (progress.activity.as_deref(), progress.run_id.as_deref()),
            (Some("Running Echo the word inner"), Some("0mvo2crlm-93605f90"))
        );
        assert!(matches!(
            &nested[3],
            WbEvent::Task {
                tool_count: Some(1),
                total_tokens: Some(23061),
                ..
            }
        ));
        let moved = updates(MOVED);
        assert_eq!(moved.len(), 1);
        assert_eq!(
            row(&moved[0]).and_then(|(_, _, d)| d.and_then(|d| d.backgrounded)),
            Some(true)
        );
    }

    #[test]
    fn a_call_keeps_its_cache_writes_by_lifetime() {
        // Claude Code 2.1.292's `message_delta` keeps the split only in `iterations`; `message_start` has it on top.
        let start = |id: &str, five: u64, hour: u64| {
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start","message":{
                "id":id,"model":"claude-haiku-4-5","usage":{"input_tokens":10,"cache_creation_input_tokens":five + hour,
                "cache_read_input_tokens":27864,"cache_creation":{"ephemeral_5m_input_tokens":five,
                "ephemeral_1h_input_tokens":hour}}}}})
        };
        let frames = [
            start("m1", 0, 1),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_delta","usage":{
                "input_tokens":10,"cache_creation_input_tokens":4364,"cache_read_input_tokens":27864,"output_tokens":241,
                "iterations":[{"cache_creation_input_tokens":4364,"cache_creation":{"ephemeral_5m_input_tokens":0,
                "ephemeral_1h_input_tokens":4364}}]}}}),
            // No split in the delta: the start's stands.
            start("m2", 70, 0),
            json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_delta","usage":{
                "input_tokens":8,"cache_creation_input_tokens":70,"cache_read_input_tokens":32228,"output_tokens":42}}}),
        ];
        let mut translator = Translator::default();
        let writes: Vec<_> = lines(&mut translator, &frames)
            .into_iter()
            .filter_map(|out| match out {
                Out::Call { usage, .. } => Some(usage.cache_writes),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            [
                Some(CacheWrites {
                    five_minute_tokens: 0,
                    one_hour_tokens: 4364
                }),
                Some(CacheWrites {
                    five_minute_tokens: 70,
                    one_hour_tokens: 0
                }),
            ]
        );
        assert_eq!(cache_writes(&json!({"cache_creation_input_tokens": 5})), None);
    }
}
