//! Nexus's own agent: `nexus __agent --session <id>`, the nexus CLI's agent with no screen. One JSON object a line each way. Its model calls go through Nexus
//! Gateway on the person's sign-in, so any model the organization allows works, and Gateway's own figures give the
//! cost.
//!
//! Its tools are reported as Claude Code names them, as Codex's are ([`crate::codex`]), so the screens need nothing of
//! their own: a read is `Read`, a folder listing `Glob`, a search `Grep`, a changed file `Edit` (its diff's lines out
//! and in), a new one `Write`, a command `Bash`, and its plan `TodoWrite`. A tool line reports a tool that already ran,
//! so its call and its result come together. The agent asks before an edit or a command; the answer goes back as
//! `decide` (`always` when the person allows the rest of the session).
//!
//! The conversation is the CLI's, by this session's id. A fork goes on from a copy of the source's (`--fork`), under the
//! id `ready` names. Each turn's end reports how many messages the conversation holds: that count is the turn's cursor,
//! and a revert or a message edit starts the agent again with `--rewind <count>`, as Claude Code resumes up to a
//! message.
//!
//! The app's own MCP server reaches it as it reaches Claude Code (`orchestra::inject`): a call to one of its tools shows
//! by the tool's own name (`mcp__nexus_app__start_session`), and asks first unless it is one Claude Code would run
//! without asking.
//!
//! A subagent (its `agent` tool) shows as Claude Code's do: an `Agent` call whose tools carry it as their parent, and a
//! task that runs until the subagent's answer, which is the call's result.
//!
//! Not supported yet, and said so plainly: compaction, task controls and metadata reads.

use std::time::Instant;

use serde_json::{Value, json};

use crate::driver::{Out, Protocol, Say, Start, cap};
use crate::event::{Image, PermissionMode, PermissionSuggestion, TaskDetail, TaskStatus, WbEvent};

/// The arguments for one run: the conversation lives in the CLI's file for this session's id. A fork's `native_id` is
/// its source's, which the copy leaves as it is.
pub fn args(start: &Start) -> Vec<String> {
    let mut args = vec![
        "__agent".to_string(),
        "--session".to_string(),
        start.native_id.to_string(),
    ];
    if start.resume && start.fork {
        args.push("--fork".into());
    }
    if let Some(at) = start.rewind.filter(|_| start.resume) {
        args.extend(["--rewind".to_string(), at.to_string()]);
    }
    args
}

/// How to install the `nexus` CLI from this Gateway, on a server. Which `nexus` runs the agent, and the desktop's own
/// install, are [`crate::nexus_runtime`]'s.
pub fn install_hint(gateway: &str) -> String {
    format!("curl -fsSL {}/install.sh | bash", gateway.trim_end_matches('/'))
}

/// The variable that tells `nexus __agent` which program drives it, and its value for this app. The agent's prompt then
/// says that it runs in a Code session, and what the app shows.
pub const HOST_VARIABLE: &str = "NEXUS_AGENT_HOST";
pub const HOST: &str = "desktop";

/// The launch of Nexus's agent, as both hosts make it: `program` with the workbench's arguments, signed in with the
/// host's own Gateway sign-in (`NEXUS_API_KEY`, `NEXUS_URL`), so its calls are the person's, under their policies and
/// budget. No plan account, no subscription route and no MCP servers.
pub fn launch(
    request: &crate::LaunchRequest,
    program: std::path::PathBuf,
    mut env: std::collections::HashMap<String, String>,
    gateway: &str,
    token: &str,
) -> Result<crate::Launch, String> {
    if request.via == crate::event::Via::Subscription {
        return Err("Nexus's agent runs through Nexus Gateway only: start the session through Nexus.".into());
    }
    env.insert("NEXUS_API_KEY".into(), token.to_string());
    env.insert("NEXUS_URL".into(), gateway.to_string());
    env.insert(HOST_VARIABLE.into(), HOST.into());
    Ok(crate::Launch {
        session_mcp: Default::default(),
        attribution: Some(crate::event::SessionAttribution {
            gateway: crate::connection::gateway_origin(gateway),
            organization: request
                .scope
                .as_str()
                .split_once('|')
                .map(|(_, org)| org.to_string())
                .filter(|org| !org.is_empty()),
            team: None,
            access: None,
            configured_mcp: Some(Vec::new()),
        }),
        spawn: crate::process::Spawn {
            program,
            args: request.args.clone(),
            env,
            cwd: request.cwd.clone(),
        },
        nexus_session: None,
        local_run: None,
        local_provider: None,
        model: request.model.clone(),
    })
}

pub struct Driver {
    native_id: String,
    model: Option<String>,
    mode: PermissionMode,
    /// The text block streaming now, and its id.
    text: Option<(String, String)>,
    /// The approval the agent waits on.
    asking: Option<String>,
    /// Ids for text blocks, tool calls and approvals.
    next: u64,
    /// A turn is under way; messages sent meanwhile wait for its end.
    running: bool,
    waiting: Vec<(String, Vec<Image>)>,
    started: Option<Instant>,
    /// What every turn of this process cost, as Gateway counts it.
    total_cost: f64,
    /// The subagents running: their `Agent` call's id, when they started, and the tools they ran.
    tasks: std::collections::HashMap<String, (Instant, u64)>,
}

impl Driver {
    pub fn new(start: &Start) -> Driver {
        Driver {
            native_id: start.native_id.to_string(),
            model: start.model.map(String::from),
            mode: start.mode,
            text: None,
            asking: None,
            next: 0,
            running: false,
            waiting: Vec::new(),
            started: None,
            total_cost: 0.0,
            tasks: std::collections::HashMap::new(),
        }
    }

    fn id(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("nexus-{kind}-{}", self.next)
    }

    /// A line to the agent.
    fn line(value: Value) -> Out {
        Out::Reply(value.to_string())
    }

    /// The `approvals` line for a permission mode ([`crate::harness::nexus::approval_level`]).
    fn approvals(mode: PermissionMode) -> Out {
        let level = crate::harness::nexus::approval_level(mode);
        Driver::line(json!({ "type": "approvals", "level": level }))
    }

    fn prompt(&mut self, text: String, images: Vec<Image>) -> Vec<Out> {
        self.running = true;
        self.started = Some(Instant::now());
        let images: Vec<Value> = images
            .into_iter()
            .map(|image| {
                let extension = image.media_type.rsplit('/').next().unwrap_or("png").to_string();
                json!({ "data": image.data, "name": image.name.unwrap_or_else(|| format!("image.{extension}")) })
            })
            .collect();
        let mut prompt = json!({ "type": "prompt", "text": text });
        if let Some(model) = &self.model {
            prompt["model"] = json!(model);
        }
        if !images.is_empty() {
            prompt["images"] = json!(images);
        }
        vec![Driver::line(prompt)]
    }

    /// The text block streaming now goes to the log as it ends.
    fn end_text(&mut self) -> Vec<Out> {
        match self.text.take() {
            Some((id, text)) if !text.trim().is_empty() => vec![Out::Event(WbEvent::Text { id, text, parent: None })],
            _ => Vec::new(),
        }
    }

    /// The turn ended: its result, then the next message that waited for it.
    fn end_turn(&mut self, error: Option<String>, interrupted: bool) -> Vec<Out> {
        let mut out = self.end_text();
        self.running = false;
        self.asking = None;
        out.push(Out::Result {
            total_cost_usd: Some(self.total_cost),
            duration_ms: self
                .started
                .take()
                .map_or(0, |started| started.elapsed().as_millis() as u64),
            error,
            interrupted,
            context_window: None,
        });
        if !self.waiting.is_empty() {
            let (text, images) = self.waiting.remove(0);
            out.extend(self.prompt(text, images));
        }
        out
    }

    /// A subagent's `Agent` call: unique in the session, as the agent numbers them per turn.
    fn task_call(turn: &Value, id: &str) -> String {
        format!("nexus-{}-{id}", turn.as_u64().unwrap_or(0))
    }

    /// A subagent started or ended.
    fn task(&mut self, turn: &Value, task: &Value) -> Vec<Out> {
        let text = |key: &str| task.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let call = Driver::task_call(turn, &text("id"));
        let mut out = self.end_text();
        let event = |status: TaskStatus, description: String, tools: Option<u64>, duration_ms: Option<u64>| {
            Out::Event(WbEvent::Task {
                task_id: call.clone(),
                tool_use_id: Some(call.clone()),
                task_type: Some("local_agent".into()),
                agent_type: None,
                status,
                description,
                tool_count: tools,
                total_tokens: None,
                duration_ms,
                detail: TaskDetail {
                    depth: Some(1),
                    ..TaskDetail::default()
                }
                .some(),
            })
        };
        if text("state") == "started" {
            self.tasks.insert(call.clone(), (Instant::now(), 0));
            out.push(Out::Event(WbEvent::Tool {
                id: call.clone(),
                name: "Agent".into(),
                input: json!({ "description": text("description"), "prompt": text("prompt") }),
                parent: None,
            }));
            out.push(event(TaskStatus::Running, text("description"), None, None));
            return out;
        }
        let failed = text("state") == "failed";
        let (started, tools) = self.tasks.remove(&call).unwrap_or((Instant::now(), 0));
        let (output, truncated) = cap(text("result"));
        out.push(Out::Event(WbEvent::ToolResult {
            id: call.clone(),
            output: output.clone(),
            images: Vec::new(),
            is_error: failed,
            truncated,
            parent: None,
        }));
        let status = if failed {
            TaskStatus::Failed
        } else {
            TaskStatus::Completed
        };
        out.push(event(
            status,
            output,
            Some(tools),
            Some(started.elapsed().as_millis() as u64),
        ));
        out
    }

    /// A tool that ran: its call and its result, under the subagent that ran it (`parent`).
    fn tool(&mut self, tool: &Value, parent: Option<String>) -> Vec<Out> {
        if let Some((_, tools)) = parent.as_ref().and_then(|parent| self.tasks.get_mut(parent)) {
            *tools += 1;
        }
        let mut out = self.end_text();
        let text = |key: &str| tool.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let count = |key: &str| tool.get(key).and_then(Value::as_u64).unwrap_or(0);
        let lines = |key: &str| -> Vec<String> {
            tool.get(key)
                .and_then(Value::as_array)
                .map(|lines| lines.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default()
        };
        let called = text("name");
        let (name, input, output, is_error) = match text("kind").as_str() {
            "read" => (
                "Read",
                json!({ "file_path": text("path") }),
                format!("{} lines", count("lines")),
                false,
            ),
            "list" => (
                "Glob",
                json!({ "pattern": format!("{}/*", text("path").trim_end_matches('/')) }),
                format!("{} entries", count("entries")),
                false,
            ),
            "search" => (
                "Grep",
                json!({ "pattern": text("pattern") }),
                format!("{} matches", count("matches")),
                false,
            ),
            "edit" => {
                let (before, after) = sides(&lines("diff"));
                (
                    "Edit",
                    json!({ "file_path": text("path"), "old_string": before, "new_string": after }),
                    format!("+{} -{}", count("added"), count("removed")),
                    false,
                )
            }
            "create" => {
                let (_, after) = sides(&lines("diff"));
                (
                    "Write",
                    json!({ "file_path": text("path"), "content": after }),
                    format!("+{}", count("added")),
                    false,
                )
            }
            "run" => {
                let exit = tool.get("exit").and_then(Value::as_i64);
                (
                    "Bash",
                    json!({ "command": text("command") }),
                    text("output"),
                    exit != Some(0),
                )
            }
            // An MCP server's tool, by its own name.
            "call" => (
                called.as_str(),
                tool.get("input").cloned().unwrap_or(Value::Null),
                text("output"),
                tool.get("error").and_then(Value::as_bool).unwrap_or(false),
            ),
            "plan" => {
                let todos: Vec<Value> = tool
                    .get("steps")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|step| {
                        let content = step.get("step").and_then(Value::as_str).unwrap_or("");
                        json!({ "content": content, "status": step.get("status"), "activeForm": content })
                    })
                    .collect();
                ("TodoWrite", json!({ "todos": todos }), "Plan updated.".into(), false)
            }
            _ => {
                // A tool that could not run, or that the person declined: as the tool it was meant to be.
                let what = text("what");
                let (name, input) = match what.split_once(' ') {
                    Some(("run", command)) => ("Bash", json!({ "command": command })),
                    Some(("edit", path)) => ("Edit", json!({ "file_path": path })),
                    Some(("read", path)) => ("Read", json!({ "file_path": path })),
                    _ => ("Nexus", json!({ "what": what })),
                };
                (name, input, text("error"), true)
            }
        };
        let id = self.id("tool");
        let (output, truncated) = cap(output);
        out.push(Out::Event(WbEvent::Tool {
            id: id.clone(),
            name: name.into(),
            input,
            parent: parent.clone(),
        }));
        out.push(Out::Event(WbEvent::ToolResult {
            id,
            output,
            images: Vec::new(),
            is_error,
            truncated,
            parent,
        }));
        out
    }

    /// The agent asks before an edit or a command.
    fn approve(&mut self, ask: &Value) -> Vec<Out> {
        let mut out = self.end_text();
        let kind = ask.get("kind").and_then(Value::as_str);
        let command = kind == Some("run");
        let named = ask.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let (tool, input) = if kind == Some("tool") {
            (named.as_str(), ask.get("input").cloned().unwrap_or(Value::Null))
        } else if command {
            (
                "Bash",
                json!({ "command": ask.get("command").and_then(Value::as_str).unwrap_or("") }),
            )
        } else {
            let diff: Vec<String> = ask
                .get("diff")
                .and_then(Value::as_array)
                .map(|lines| lines.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default();
            let (before, after) = sides(&diff);
            let path = ask.get("path").and_then(Value::as_str).unwrap_or("");
            (
                "Edit",
                json!({ "file_path": path, "old_string": before, "new_string": after }),
            )
        };
        let request_id = self.id("approval");
        self.asking = Some(request_id.clone());
        let label = match kind {
            Some("tool") => "Always allow this tool for this session",
            Some("run") => "Always allow commands for this session",
            _ => "Allow all edits for this session",
        };
        out.push(Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id: None,
            tool: tool.into(),
            input,
            description: None,
            suggestions: vec![PermissionSuggestion {
                kind: "acceptForSession".into(),
                label: label.into(),
                raw: json!("acceptForSession"),
            }],
            parent: None,
        }));
        out
    }
}

/// A unified diff's lines as the text it takes out and the text it puts in, as Codex's `file_change` reads them.
fn sides(diff: &[String]) -> (String, String) {
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for line in diff {
        if line.starts_with("@@") || line.starts_with('\\') {
            continue;
        }
        match line.split_at(line.len().min(1)) {
            ("-", rest) => before.push(rest),
            ("+", rest) => after.push(rest),
            (_, rest) => {
                before.push(rest);
                after.push(rest);
            }
        }
    }
    (before.join("\n"), after.join("\n"))
}

impl Protocol for Driver {
    fn open(&mut self) -> Vec<Out> {
        vec![Driver::approvals(self.mode)]
    }

    fn say(&mut self, say: Say) -> Vec<Out> {
        let refuse = |request: String, what: &str| {
            vec![Out::Answered {
                request_id: request,
                result: Err(format!("The Nexus agent does not support {what} yet.")),
            }]
        };
        match say {
            Say::Steer { text, images, person } if self.running => {
                self.waiting.push((text, images));
                vec![Out::TurnQueued { person }]
            }
            Say::User { text, images } if self.running => {
                self.waiting.push((text, images));
                vec![Out::TurnQueued { person: true }]
            }
            Say::User { text, images } | Say::Steer { text, images, .. } => self.prompt(text, images),
            Say::Interrupt { request } => {
                self.asking = None;
                vec![
                    Driver::line(json!({ "type": "stop" })),
                    Out::Ack {
                        request_id: request,
                        error: None,
                    },
                ]
            }
            Say::Mode { request, mode } => {
                self.mode = mode;
                vec![
                    Driver::approvals(mode),
                    Out::Ack {
                        request_id: request,
                        error: None,
                    },
                ]
            }
            // The next prompt names the model; one under way keeps its own.
            Say::Model { request, model } => {
                self.model = model;
                vec![Out::Ack {
                    request_id: request,
                    error: None,
                }]
            }
            Say::Effort { request, .. } | Say::Fast { request, .. } => vec![Out::Ack {
                request_id: request,
                error: None,
            }],
            Say::Allow {
                request_id, remember, ..
            } if self.asking.as_deref() == Some(request_id.as_str()) => {
                self.asking = None;
                let decision = if remember.is_some() { "always" } else { "allow" };
                vec![Driver::line(json!({ "type": "decide", "decision": decision }))]
            }
            Say::Deny { request_id, .. } if self.asking.as_deref() == Some(request_id.as_str()) => {
                self.asking = None;
                vec![Driver::line(json!({ "type": "decide", "decision": "deny" }))]
            }
            Say::Allow { .. } | Say::Deny { .. } | Say::ForgetInspection { .. } | Say::Rename { .. } => Vec::new(),
            Say::Compact { request, .. } => refuse(request, "compaction"),
            Say::Inspect { request, .. } => refuse(request, "metadata reads"),
            Say::TaskStop { request, .. } | Say::TaskBackground { request, .. } | Say::TaskOutput { request, .. } => {
                refuse(request, "task controls")
            }
        }
    }

    fn read(&mut self, line: &str) -> Vec<Out> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let text = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        match text("type").as_str() {
            // A fork's conversation has its own id from here on. A server that did not start is said.
            "ready" => {
                let mut out = vec![Out::Init {
                    native_id: Some(text("session"))
                        .filter(|session| !session.is_empty())
                        .unwrap_or_else(|| self.native_id.clone()),
                    model: self.model.clone(),
                    version: Some(text("version")).filter(|version| !version.is_empty()),
                }];
                for note in event.get("notes").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(text) = note.as_str() {
                        out.push(Out::Event(WbEvent::Notice { text: text.to_string() }));
                    }
                }
                out
            }
            "delta" => {
                if self.text.is_none() {
                    let id = self.id("text");
                    self.text = Some((id, String::new()));
                }
                let Some((id, so_far)) = &mut self.text else {
                    return Vec::new();
                };
                so_far.push_str(&text("text"));
                vec![Out::Delta {
                    id: id.clone(),
                    text: so_far.clone(),
                    parent: None,
                }]
            }
            "tool" => {
                let parent = event
                    .get("parent")
                    .and_then(Value::as_str)
                    .map(|id| Driver::task_call(&event["turn"], id));
                event
                    .get("tool")
                    .map(|tool| self.tool(tool, parent))
                    .unwrap_or_default()
            }
            "task" => event
                .get("task")
                .map(|task| self.task(&event["turn"], task))
                .unwrap_or_default(),
            "approve" => event.get("ask").map(|ask| self.approve(ask)).unwrap_or_default(),
            "done" => {
                self.total_cost += event.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
                let stopped = event.get("stopped").and_then(Value::as_bool).unwrap_or(false);
                // Where the conversation stands: its turn end keeps it, for a later rewind.
                let mut out: Vec<Out> = event
                    .get("messages")
                    .and_then(Value::as_u64)
                    .map(|count| Out::Cursor(count.to_string()))
                    .into_iter()
                    .collect();
                out.extend(self.end_turn(None, stopped));
                out
            }
            // A turn's error ends it; another error (a bad line, a prompt while one runs) is only said.
            "error" if event.get("turn").is_some() => {
                let hint = Some(text("hint")).filter(|hint| !hint.is_empty());
                let message = match hint {
                    Some(hint) => format!("{} {hint}", text("message")),
                    None => text("message"),
                };
                self.end_turn(Some(message), false)
            }
            "error" => vec![Out::Event(WbEvent::Error {
                text: text("message"),
                detail: None,
            })],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(fork: bool) -> Start<'static> {
        Start {
            mode: PermissionMode::Default,
            model: Some("anthropic/claude-haiku-5.5"),
            effort: None,
            fast: false,
            native_id: "thread-1",
            resume: false,
            fork,
            cwd: "/tmp",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        }
    }

    fn reply(out: &[Out]) -> Vec<Value> {
        out.iter()
            .filter_map(|out| match out {
                Out::Reply(line) => serde_json::from_str(line).ok(),
                _ => None,
            })
            .collect()
    }

    /// The agent starts on the app's sign-in, and is told that the app drives it.
    #[test]
    fn the_launch_signs_in_as_the_app_and_says_it_is_the_app() {
        let request = crate::LaunchRequest {
            thread_id: "thread-1".into(),
            agent: crate::event::SessionAgent::Nexus,
            cwd: "/tmp".into(),
            model: None,
            via: crate::event::Via::Nexus,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: vec!["__agent".into()],
            scope: "https://gateway.example|acme".into(),
            prompt_counts: None,
        };
        let launched = launch(
            &request,
            "/bin/nexus".into(),
            Default::default(),
            "https://gateway.example",
            "nx_token",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let env = &launched.spawn.env;
        assert_eq!(env.get("NEXUS_API_KEY").map(String::as_str), Some("nx_token"));
        assert_eq!(
            env.get("NEXUS_URL").map(String::as_str),
            Some("https://gateway.example")
        );
        assert_eq!(env.get(HOST_VARIABLE).map(String::as_str), Some("desktop"));
        let on_a_plan = crate::LaunchRequest {
            via: crate::event::Via::Subscription,
            ..request
        };
        assert!(
            launch(
                &on_a_plan,
                "/bin/nexus".into(),
                Default::default(),
                "https://gateway.example",
                "nx_token"
            )
            .is_err()
        );
    }

    #[test]
    fn a_turn_streams_runs_tools_asks_and_ends_with_its_cost() {
        let mut nexus = Driver::new(&start(false));
        assert_eq!(reply(&nexus.open()), [json!({ "type": "approvals", "level": "ask" })]);
        assert!(matches!(
            nexus.read(r#"{"type":"ready","version":"0.19.0"}"#)[..],
            [Out::Init { .. }]
        ));
        let sent = reply(&nexus.say(Say::User {
            text: "fix it".into(),
            images: Vec::new(),
        }));
        assert_eq!(
            sent,
            [json!({ "type": "prompt", "text": "fix it", "model": "anthropic/claude-haiku-5.5" })]
        );

        nexus.read(r#"{"type":"delta","turn":1,"text":"Let me "}"#);
        let delta = nexus.read(r#"{"type":"delta","turn":1,"text":"look."}"#);
        assert!(matches!(&delta[..], [Out::Delta { text, .. }] if text == "Let me look."));

        let ask = nexus.read(
            r#"{"type":"approve","turn":1,"ask":{"kind":"edit","path":"a.py","diff":["@@ -1 +1 @@","-hi","+hello"]}}"#,
        );
        let Some(Out::Event(WbEvent::Approval {
            request_id,
            tool,
            input,
            ..
        })) = ask.last()
        else {
            panic!("an approval: {ask:?}");
        };
        assert!(
            matches!(&ask[0], Out::Event(WbEvent::Text { text, .. }) if text == "Let me look."),
            "the text ends first"
        );
        assert_eq!(
            (
                tool.as_str(),
                input["old_string"].as_str(),
                input["new_string"].as_str()
            ),
            ("Edit", Some("hi"), Some("hello"))
        );
        let allow = Say::Allow {
            request_id: request_id.clone(),
            input: json!({}),
            answers: None,
            remember: Some(json!("acceptForSession")),
        };
        assert_eq!(
            reply(&nexus.say(allow)),
            [json!({ "type": "decide", "decision": "always" })]
        );

        let ran = nexus
            .read(r#"{"type":"tool","turn":1,"tool":{"kind":"run","command":"cat a.py","exit":1,"output":"oops"}}"#);
        assert!(
            matches!(&ran[0], Out::Event(WbEvent::Tool { name, input, .. }) if name == "Bash" && input["command"] == "cat a.py")
        );
        assert!(matches!(&ran[1], Out::Event(WbEvent::ToolResult { output, is_error: true, .. }) if output == "oops"));

        let end = nexus.read(r#"{"type":"done","turn":1,"model":"m","cost_usd":0.002,"steps":3,"stopped":false}"#);
        assert!(
            matches!(&end[..], [Out::Result { total_cost_usd: Some(cost), interrupted: false, error: None, .. }] if (*cost - 0.002).abs() < 1e-9)
        );
    }

    #[test]
    fn a_message_during_a_turn_waits_for_its_end() {
        let mut nexus = Driver::new(&start(false));
        nexus.say(Say::User {
            text: "one".into(),
            images: Vec::new(),
        });
        assert_eq!(
            nexus.say(Say::User {
                text: "two".into(),
                images: Vec::new()
            }),
            [Out::TurnQueued { person: true }]
        );
        let end = nexus.read(r#"{"type":"done","turn":1,"cost_usd":0.001,"stopped":false}"#);
        assert_eq!(reply(&end)[0]["text"], "two");
    }

    /// The turn a steer during a turn reserves is its sender's: Nexus's notice gets one that is not the person's.
    #[test]
    fn a_steer_during_a_turn_reserves_a_turn_for_its_sender() {
        let mut nexus = Driver::new(&start(false));
        nexus.say(Say::User {
            text: "one".into(),
            images: Vec::new(),
        });
        let steer = |text: &str, person: bool| Say::Steer {
            text: text.into(),
            images: Vec::new(),
            person,
        };
        assert_eq!(nexus.say(steer("a notice", false)), [Out::TurnQueued { person: false }]);
        assert_eq!(
            nexus.say(steer("the person's", true)),
            [Out::TurnQueued { person: true }]
        );
    }

    #[test]
    fn mcp_tools_show_by_their_own_names_and_ask_for_this_tool() {
        let mut nexus = Driver::new(&start(false));
        let ready = nexus.read(r#"{"type":"ready","notes":["The MCP server gone did not start: no such file"]}"#);
        assert!(matches!(&ready[1], Out::Event(WbEvent::Notice { text }) if text.contains("gone")));
        let ask = nexus.read(
            r#"{"type":"approve","turn":1,"ask":{"kind":"tool","name":"mcp__nexus_app__start_session","input":{"agent":"nexus"}}}"#,
        );
        assert!(matches!(
            &ask[..],
            [Out::Event(WbEvent::Approval { tool, input, suggestions, .. })]
                if tool == "mcp__nexus_app__start_session" && input["agent"] == "nexus"
                    && suggestions[0].label == "Always allow this tool for this session"
        ));
        let called = nexus.read(
            r#"{"type":"tool","turn":1,"tool":{"kind":"call","name":"mcp__nexus_app__wait_sessions","input":{},"output":"done","error":false}}"#,
        );
        assert!(
            matches!(&called[0], Out::Event(WbEvent::Tool { name, .. }) if name == "mcp__nexus_app__wait_sessions")
        );
        assert!(
            matches!(&called[1], Out::Event(WbEvent::ToolResult { output, is_error: false, .. }) if output == "done")
        );
    }

    #[test]
    fn a_subagent_is_an_agent_call_with_its_tools_under_it() {
        let mut nexus = Driver::new(&start(false));
        let started = nexus.read(
            r#"{"type":"task","turn":2,"task":{"id":"agent-1","state":"started","description":"Read hello","prompt":"Read it."}}"#,
        );
        assert!(
            matches!(&started[..], [Out::Event(WbEvent::Tool { id, name, parent: None, .. }), Out::Event(WbEvent::Task { status: TaskStatus::Running, .. })]
                if id == "nexus-2-agent-1" && name == "Agent")
        );
        let read = nexus
            .read(r#"{"type":"tool","turn":2,"parent":"agent-1","tool":{"kind":"read","path":"hello.py","lines":1}}"#);
        assert!(
            matches!(&read[0], Out::Event(WbEvent::Tool { name, parent: Some(parent), .. }) if name == "Read" && parent == "nexus-2-agent-1")
        );
        let ended = nexus
            .read(r#"{"type":"task","turn":2,"task":{"id":"agent-1","state":"completed","result":"It prints hi."}}"#);
        assert!(matches!(
            &ended[..],
            [Out::Event(WbEvent::ToolResult { id, output, is_error: false, .. }),
             Out::Event(WbEvent::Task { status: TaskStatus::Completed, tool_count: Some(1), .. })]
                if id == "nexus-2-agent-1" && output == "It prints hi."
        ));
    }

    #[test]
    fn modes_plans_forks_and_rewinds() {
        let mut nexus = Driver::new(&start(true));
        assert_eq!(args(&start(false)), ["__agent", "--session", "thread-1"]);
        let fork = Start {
            resume: true,
            ..start(true)
        };
        assert_eq!(args(&fork), ["__agent", "--session", "thread-1", "--fork"]);
        let rewound = Start {
            resume: true,
            rewind: Some("4"),
            ..start(false)
        };
        assert_eq!(args(&rewound), ["__agent", "--session", "thread-1", "--rewind", "4"]);
        assert!(matches!(
            &nexus.read(r#"{"type":"ready","session":"fork-9"}"#)[..],
            [Out::Init { native_id, .. }] if native_id == "fork-9"
        ));
        let end = nexus.read(r#"{"type":"done","turn":1,"cost_usd":0.001,"messages":6}"#);
        assert!(
            matches!(&end[..], [Out::Cursor(at), Out::Result { .. }] if at == "6"),
            "{end:?}"
        );
        let mode = nexus.say(Say::Mode {
            request: "r".into(),
            mode: PermissionMode::AcceptEdits,
        });
        assert_eq!(reply(&mode), [json!({ "type": "approvals", "level": "auto-edit" })]);
        let plan = nexus
            .read(r#"{"type":"tool","turn":1,"tool":{"kind":"plan","steps":[{"step":"Read","status":"completed"}]}}"#);
        assert!(
            matches!(&plan[0], Out::Event(WbEvent::Tool { name, input, .. }) if name == "TodoWrite" && input["todos"][0]["status"] == "completed")
        );
        assert!(matches!(
            &nexus.say(Say::Compact {
                request: "c".into(),
                instructions: None
            })[..],
            [Out::Answered { result: Err(_), .. }]
        ));
    }
}
