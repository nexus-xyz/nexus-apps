//! Codex's app server (`codex app-server`), the JSON-RPC protocol Codex ships for IDE integrations: one JSON object per
//! line each way, with no `"jsonrpc"` field. We send `initialize` and `initialized`, then `thread/start` (or
//! `thread/resume` with Codex's thread id) and one `turn/start` per message; `turn/interrupt` stops the turn under way.
//! Codex reports its work as notifications (`turn/started`, `item/started`, `item/agentMessage/delta`,
//! `item/completed`, `thread/tokenUsage/updated`, `turn/completed`) and asks for approvals with requests of its own
//! (`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`), answered with a decision. [`Driver`]
//! reads them as the same [`Out`]s as Claude Code's, its tools named as Claude Code names them (a command is `Bash`, a
//! new file `Write`, a changed one `Edit`), so the screens need nothing of their own. Checked against `codex-cli`
//! 0.163.0.
//!
//! Follow-ups use `turn/steer` so Codex reads them at its next step. A refused steer waits for the next turn.
//! An interrupt before Codex names the turn waits until it does; "no active turn" counts as done.
//!
//! A Codex sub-agent is a thread of its own, reported on the same connection under its own `threadId`. Its spawn
//! (`subAgentActivity` on newer models, `collabAgentToolCall` with `spawnAgent` on older ones) becomes an `Agent` call,
//! its thread's work goes under that call as Claude Code's sub-agent work does, and `task` reports say when it starts
//! and how it ends. Shapes from `codex-cli` 0.156.1 runs recorded by T3 Code, checked against 0.160.0's schema.
//!
//! Codex's plan (its `update_plan` tool, reported as `turn/plan/updated`) becomes a `TodoWrite` call, so it shows as
//! Claude Code's plan does. Shape checked against 0.164.0's schema.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Value, json};

use crate::driver::{Background, Out, Protocol, Say, Start, cap};
use crate::event::{Effort, Image, PermissionMode, PermissionSuggestion, TaskDetail, TaskStatus, Usage, WbEvent};
use crate::harness::Harness;
use crate::harness::codex::{collaboration_mode, policy, reviewer, sandbox_policy};

/// The arguments for one run: the model and effort as config overrides (so the host can replace the model, as it does
/// Claude Code's `--model`), then the app server.
pub fn args(model: Option<&str>, effort: Option<Effort>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        args.extend(["-c".into(), format!("model={}", Value::from(model))]);
    }
    if let Some(effort) = effort {
        args.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", Value::from(effort.as_str())),
        ]);
    }
    args.extend(["-c".into(), "features.default_mode_request_user_input=true".into()]);
    args.push("app-server".into());
    args
}

/// Codex's refusal to open a thread that another Codex process has open, as `codex-cli` 0.169.0 words it: "thread <id>
/// already has an active writer". The ChatGPT app and a `codex` in a terminal share `~/.codex` with Nexus.
const WRITER_HELD: &str = "already has an active writer";

/// What the person reads when Codex will not open the thread (`what`: "resume this conversation"). A thread another
/// Codex holds says what to do, with Codex's words as the detail; any other refusal is Codex's words.
fn thread_refused(what: &str, error: String) -> WbEvent {
    if error.contains(WRITER_HELD) {
        WbEvent::Error {
            text: "This conversation is open in another Codex app, such as the ChatGPT app. Close it there, then \
                   send your message again. Or fork this session to keep working in Nexus."
                .into(),
            detail: Some(error),
        }
    } else {
        WbEvent::Error {
            text: format!("Codex could not {what}: {error}"),
            detail: None,
        }
    }
}

/// The turn Codex is on.
enum Turn {
    Idle,
    /// `turn/start` was sent, and Codex has not named the turn yet. `interrupt`: one was asked for meanwhile.
    Starting {
        interrupt: bool,
    },
    Running {
        id: String,
    },
}

/// What one of our requests was, until its answer.
enum Asked {
    Initialize,
    Thread,
    /// `thread/revert`, after a revert in the app: the first turn waits for it.
    Revert,
    Turn,
    /// A message for the turn under way (`turn/steer`), and whether it is the person's (`Say::Steer`): if Codex
    /// refuses it, it runs as the next turn.
    Steer(Value, bool),
    /// A request about one task (`Say::TaskStop`), by our request id: its answer is an `Out::Answered`.
    Task(String),
    Compact(String),
    AccountUpdated(u64),
    /// An interrupt, or a denial's reason: nothing waits for its answer.
    Other,
}

/// Settings changed while the agent runs: the next `turn/start` carries them.
#[derive(Default)]
struct Changes {
    mode: Option<PermissionMode>,
    model: Option<String>,
    effort: Option<Effort>,
}

/// One run of `codex app-server`, for one session.
pub struct Driver {
    next_id: u64,
    asked: HashMap<u64, Asked>,
    /// Codex's thread, once it started or resumed it.
    thread: Option<String>,
    native_id: String,
    resume: bool,
    /// Resuming as a fork (`thread/fork`): a new thread that starts with everything `native_id` had.
    fork: bool,
    /// The turn to drop, with every later one, once the thread is live (`Start::rewind`).
    rewind: Option<String>,
    rewind_required: bool,
    cwd: String,
    prompt: String,
    mode: PermissionMode,
    /// Codex's auto review answers the approvals: it was last told Auto, or a resumed thread came back with it.
    /// Leaving Auto has to say "user" once.
    reviewing: bool,
    changes: Changes,
    /// Messages waiting for the turn under way to end, as `turn/start`'s input.
    queue: VecDeque<Value>,
    turn: Turn,
    /// Messages for the turn under way, waiting for Codex to name it, each with whether it is the person's.
    steers: Vec<(Value, bool)>,
    /// The `turn/completed`s that came while the turn was starting: one ends the turn only if Codex then names that
    /// turn. All are held, so a late end of an earlier turn cannot push out the new turn's own.
    early_ends: Vec<Value>,
    /// Open approvals: Codex's request id, by ours.
    approvals: HashMap<String, Value>,
    /// Question requests use an answers payload instead of an approval decision.
    questions: HashSet<String>,
    /// The open approvals a sub-agent asked for, by our request id: a decline of one says no reason to the session.
    asked_by_children: HashSet<String>,
    /// Streamed text so far, by item.
    texts: HashMap<String, String>,
    /// The first tool of each item seen starting, for the approval that follows it.
    tools: HashMap<String, (String, Value)>,
    started: HashSet<String>,
    model: String,
    effort: Option<Effort>,
    context_window: Option<u64>,
    /// Sub-agents Codex spawned, by their own thread.
    children: HashMap<String, Child>,
    /// The plans Codex reported in this run, which number their `TodoWrite` calls.
    plans: u64,
    /// Codex's version, from `initialize`'s `userAgent`.
    version: Option<String>,
    account_revision: u64,
    /// When the plan's used-up limit resets (epoch milliseconds), as Codex last reported its limits
    /// (`account/rateLimits/updated`).
    limit_resets: Option<i64>,
    /// The error Codex last said it retries after, in the turn under way: each one is said once (`error` with
    /// `willRetry`).
    retrying: Option<String>,
}

/// A turn's `codexErrorInfo` says the plan's usage limit stopped it: `usageLimitExceeded`, as a name or as the one key
/// of an object.
fn usage_limit(info: Option<&Value>) -> bool {
    match info {
        Some(Value::String(name)) => name == "usageLimitExceeded",
        Some(Value::Object(fields)) => fields.contains_key("usageLimitExceeded"),
        _ => false,
    }
}

/// A Codex sub-agent: a thread of its own whose notifications come on the same connection, tagged with its thread id.
struct Child {
    /// The `Agent` call it shows under, which its own work is placed under.
    call: String,
    /// The `Agent` call of the sub-agent that spawned it. None: the session's own agent did.
    parent_call: Option<String>,
    /// 1 for a sub-agent the session's own agent spawned, N + 1 for one a depth-N sub-agent spawned.
    depth: u32,
    /// The model it runs on, when Codex says: its calls are priced with it.
    model: Option<String>,
    /// The tool calls it made.
    tools: u64,
    /// Its commands started and not completed: an interrupted child's command runs on, and can still write files.
    open: HashSet<String>,
    /// Its latest message: on its end, how it went.
    last: String,
    ended: bool,
    /// Its turn under way, which a stop interrupts. None between its turns.
    turn: Option<String>,
}

/// A message as `turn/start` and `turn/steer` take it: images first, then the text.
fn input(text: &str, images: &[Image]) -> Value {
    let mut input: Vec<Value> = images
        .iter()
        .map(|image| json!({"type": "image", "url": format!("data:{};base64,{}", image.media_type, image.data)}))
        .collect();
    if !text.is_empty() {
        input.push(json!({"type": "text", "text": text, "text_elements": []}));
    }
    Value::Array(input)
}

fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_string()
}

impl Driver {
    pub fn new(start: &Start) -> Driver {
        Driver {
            next_id: 0,
            asked: HashMap::new(),
            thread: None,
            native_id: start.native_id.to_string(),
            resume: start.resume,
            fork: start.resume && start.fork,
            rewind: start.rewind.filter(|_| start.resume).map(String::from),
            rewind_required: start.rewind_required,
            cwd: start.cwd.to_string(),
            prompt: start
                .prompt
                .map(str::to_owned)
                .unwrap_or_else(|| nexus_agents::Codex.instructions().text()),
            mode: start.mode,
            reviewing: start.mode == PermissionMode::Auto,
            changes: Changes::default(),
            queue: VecDeque::new(),
            turn: Turn::Idle,
            steers: Vec::new(),
            early_ends: Vec::new(),
            approvals: HashMap::new(),
            questions: HashSet::new(),
            asked_by_children: HashSet::new(),
            texts: HashMap::new(),
            tools: HashMap::new(),
            started: HashSet::new(),
            model: start
                .model
                .filter(|m| !nexus_client::agents::is_auto_name(Some(m)))
                .unwrap_or_default()
                .to_string(),
            effort: start.effort,
            context_window: None,
            children: HashMap::new(),
            plans: 0,
            version: None,
            account_revision: 0,
            limit_resets: None,
            retrying: None,
        }
    }

    /// A sub-agent's thread starts under its `Agent` call `call`, spawned by the sub-agent whose call is `parent` (none:
    /// the session's own agent): a task report. Nothing when the thread is known already (a spawn seen both as a tool
    /// call and as activity, or its start and its completion).
    fn spawned(
        &mut self,
        thread: &str,
        call: &str,
        title: &str,
        parent: Option<String>,
        model: Option<String>,
    ) -> Vec<Out> {
        if thread.is_empty() || self.children.contains_key(thread) {
            return vec![];
        }
        // A spawn on a sub-agent's thread comes under that sub-agent's call: it is the parent.
        let spawner = parent
            .as_ref()
            .and_then(|parent| self.children.iter().find(|(_, child)| &child.call == parent));
        let parent_task = spawner.map(|(thread, _)| thread.clone());
        let depth = spawner.map_or(1, |(_, child)| child.depth + 1);
        self.children.insert(
            thread.into(),
            Child {
                call: call.into(),
                parent_call: parent,
                depth,
                model: model.clone(),
                tools: 0,
                open: HashSet::new(),
                last: String::new(),
                ended: false,
                turn: None,
            },
        );
        let detail = TaskDetail {
            parent_task_id: parent_task,
            depth: Some(depth),
            model,
            ..TaskDetail::default()
        };
        // Until it ends and its commands finish, the agent is not idle, and a revert waits (lib.rs `background`).
        vec![
            Out::Event(WbEvent::Task {
                task_id: thread.into(),
                tool_use_id: Some(call.into()),
                task_type: Some("local_agent".into()),
                agent_type: None,
                status: TaskStatus::Running,
                description: title.into(),
                tool_count: None,
                total_tokens: None,
                duration_ms: None,
                detail: detail.some(),
            }),
            Out::Background(Background::Started(thread.into())),
        ]
    }

    /// A sub-agent ended (or ran again, with `TaskStatus::Running` and its new turn `run`): a task report, and on an
    /// end its `Agent` call's result, its last message.
    fn child_status(
        &mut self,
        thread: &str,
        status: TaskStatus,
        duration_ms: Option<u64>,
        run: Option<String>,
    ) -> Vec<Out> {
        let Some(child) = self.children.get_mut(thread) else {
            return vec![];
        };
        let running = status == TaskStatus::Running;
        if child.ended != running {
            return vec![];
        }
        child.ended = !running;
        let report = |description: String| {
            Out::Event(WbEvent::Task {
                task_id: thread.into(),
                tool_use_id: Some(child.call.clone()),
                task_type: Some("local_agent".into()),
                agent_type: None,
                status,
                description,
                tool_count: (!running).then_some(child.tools),
                total_tokens: None,
                duration_ms: if running { None } else { duration_ms },
                detail: TaskDetail {
                    run_id: run.clone().filter(|_| running),
                    ..TaskDetail::default()
                }
                .some(),
            })
        };
        if running {
            return vec![
                report(String::new()),
                Out::Background(Background::Started(thread.into())),
            ];
        }
        // A command still waiting for its approval never ran, and it will not: it does not hold the ended child.
        let waiting: Vec<String> = child
            .open
            .iter()
            .filter(|item| self.approvals.keys().any(|ours| ours.starts_with(&format!("{item}#"))))
            .cloned()
            .collect();
        for item in waiting {
            child.open.remove(&item);
        }
        let drained = child.open.is_empty();
        let (output, truncated) = cap(child.last.clone());
        let mut out = vec![
            report(child.last.clone()),
            Out::Event(WbEvent::ToolResult {
                images: vec![],
                id: child.call.clone(),
                output,
                is_error: status == TaskStatus::Failed,
                truncated,
                parent: child.parent_call.clone(),
            }),
        ];
        if drained {
            out.push(Out::Background(Background::Ended(thread.into())));
        }
        out
    }

    /// A sub-agent's own thread: its work goes under its `Agent` call, and its turn is its own, not the session's.
    fn child_notification(&mut self, thread: &str, method: &str, params: &Value) -> Vec<Out> {
        let call = self.children.get(thread).map(|child| child.call.clone());
        match method {
            "turn/started" => {
                let run = params.pointer("/turn/id").and_then(Value::as_str).map(String::from);
                if let Some(child) = self.children.get_mut(thread) {
                    if child.turn == run {
                        return vec![];
                    }
                    child.turn = run.clone();
                }
                let ready = run.is_some();
                let mut out = self.child_status(thread, TaskStatus::Running, None, run);
                if ready {
                    out.push(Out::TaskReady(thread.into()));
                }
                out
            }
            "turn/completed" => {
                let turn = params.get("turn").unwrap_or(&Value::Null);
                if let Some(child) = self.children.get_mut(thread)
                    && child.turn.as_deref() == turn.get("id").and_then(Value::as_str)
                {
                    child.turn = None;
                }
                let status = match text(turn.get("status")).as_str() {
                    "failed" => TaskStatus::Failed,
                    "interrupted" => TaskStatus::Stopped,
                    _ => TaskStatus::Completed,
                };
                self.child_status(thread, status, turn.get("durationMs").and_then(Value::as_u64), None)
            }
            "item/agentMessage/delta" => {
                let id = text(params.get("itemId"));
                let so_far = self.texts.entry(id.clone()).or_default();
                so_far.push_str(params.get("delta").and_then(Value::as_str).unwrap_or_default());
                vec![Out::Delta {
                    id,
                    text: so_far.clone(),
                    parent: call,
                }]
            }
            "item/started" | "item/completed" => {
                let item = params.get("item").unwrap_or(&Value::Null);
                let mut out = self.item(item, method == "item/completed", call);
                if let Some(child) = self.children.get_mut(thread) {
                    if text(item.get("type")) == "commandExecution" {
                        let id = text(item.get("id"));
                        if method == "item/started" {
                            child.open.insert(id);
                        } else if child.open.remove(&id) && child.ended && child.open.is_empty() {
                            // The last command of a child that already ended has finished: it no longer runs.
                            out.push(Out::Background(Background::Ended(thread.to_string())));
                        }
                    }
                    for out in &out {
                        match out {
                            Out::Event(WbEvent::Tool { .. }) => child.tools += 1,
                            Out::Event(WbEvent::Text { text, .. }) => child.last = text.clone(),
                            _ => {}
                        }
                    }
                }
                out
            }
            "turn/plan/updated" => self.plan(params, call),
            "thread/tokenUsage/updated" => {
                let last = params.pointer("/tokenUsage/last").unwrap_or(&Value::Null);
                let model = self.children.get(thread).and_then(|child| child.model.clone());
                let mut out = self.call(last, model);
                if let Some(Out::Call { parent, .. }) = out.first_mut() {
                    *parent = call;
                }
                out
            }
            _ => vec![],
        }
    }

    /// Codex's whole plan as it stands, as a `TodoWrite` call and its result. Codex gives a step no `activeForm`.
    fn plan(&mut self, params: &Value, parent: Option<String>) -> Vec<Out> {
        let todos: Vec<Value> = params
            .get("plan")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|step| {
                let content = step.get("step").and_then(Value::as_str)?;
                let status = match text(step.get("status")).as_str() {
                    "completed" => "completed",
                    "inProgress" => "in_progress",
                    _ => "pending",
                };
                Some(json!({"content": content, "status": status}))
            })
            .collect();
        self.plans += 1;
        // Codex's turn ids are unique across runs; the count keeps two plans of one turn apart.
        let id = format!("plan-{}-{}", text(params.get("turnId")), self.plans);
        vec![
            Out::Event(WbEvent::Tool {
                id: id.clone(),
                name: "TodoWrite".into(),
                input: json!({"todos": todos}),
                parent: parent.clone(),
            }),
            Out::Event(WbEvent::ToolResult {
                images: vec![],
                id,
                output: String::new(),
                is_error: false,
                truncated: false,
                parent,
            }),
        ]
    }

    /// One model call's usage, from Codex's `last` counts, priced with `model` (a sub-agent's), else the session's.
    fn call(&self, last: &Value, model: Option<String>) -> Vec<Out> {
        // Codex counts cached input inside its input, Claude Code apart from it.
        let tokens = |key: &str| last.get(key).and_then(Value::as_u64).unwrap_or(0);
        let cached = tokens("cachedInputTokens");
        vec![Out::Call {
            model: model.unwrap_or_else(|| self.model.clone()),
            usage: Usage {
                input_tokens: tokens("inputTokens").saturating_sub(cached),
                output_tokens: tokens("outputTokens"),
                cache_read_tokens: cached,
                cache_write_tokens: tokens("cacheWriteInputTokens"),
                // Codex reports no cache lifetime: its sessions show no warm-cache timer.
                cache_writes: None,
            },
            parent: None,
        }]
    }

    fn request(&mut self, asked: Asked, method: &str, params: Value) -> Out {
        self.next_id += 1;
        self.asked.insert(self.next_id, asked);
        Out::Reply(json!({"id": self.next_id, "method": method, "params": params}).to_string())
    }

    /// The next waiting message as a turn, when the thread is live and no turn is under way.
    fn next_turn(&mut self) -> Vec<Out> {
        let (Turn::Idle, Some(thread)) = (&self.turn, self.thread.clone()) else {
            return vec![];
        };
        let Some(input) = self.queue.pop_front() else {
            return vec![];
        };
        let mut params = json!({"threadId": thread, "input": input});
        let changes = std::mem::take(&mut self.changes);
        let mode_changed = changes.mode.is_some();
        if let Some(mode) = changes.mode {
            params["approvalPolicy"] = json!(policy(mode).0);
            params["sandboxPolicy"] = sandbox_policy(mode);
            // Only into or out of Auto: the other modes never send the field.
            if mode == PermissionMode::Auto || self.reviewing {
                params["approvalsReviewer"] = json!(reviewer(mode));
                self.reviewing = mode == PermissionMode::Auto;
            }
        }
        let model_changed = changes.model.is_some();
        if let Some(model) = changes.model {
            self.model = model;
        }
        let effort_changed = changes.effort.is_some();
        if let Some(effort) = changes.effort {
            self.effort = Some(effort);
        }
        if !self.model.is_empty() && (self.mode == PermissionMode::Plan || mode_changed) {
            params["collaborationMode"] = json!({
                "mode": collaboration_mode(self.mode),
                "settings": {"model": self.model, "reasoning_effort": self.effort.map(|effort| effort.as_str()), "developer_instructions": null}
            });
        } else {
            if model_changed {
                params["model"] = json!(self.model);
            }
            if effort_changed {
                params["effort"] = json!(self.effort.map(|effort| effort.as_str()));
            }
        }
        let out = self.request(Asked::Turn, "turn/start", params);
        self.turn = Turn::Starting { interrupt: false };
        self.early_ends.clear();
        vec![out]
    }

    fn interrupt(&mut self, turn: String) -> Out {
        let thread = self.thread.clone();
        self.request(
            Asked::Other,
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
        )
    }

    fn steer(&mut self, turn: &str, input: Value, person: bool) -> Out {
        let params = json!({"threadId": self.thread, "expectedTurnId": turn, "input": input});
        self.request(Asked::Steer(input, person), "turn/steer", params)
    }

    /// [`Driver::named`], then the turn's end if that came first ([`Driver::completed`]).
    fn name(&mut self, id: &str) -> Vec<Out> {
        let mut out = self.named(id);
        if let Some(at) = self.early_ends.iter().position(|end| text(end.get("id")) == id) {
            let end = self.early_ends.swap_remove(at);
            out.extend(self.completed(&end));
        }
        out
    }

    /// Codex named the turn it started (in `turn/start`'s answer or `turn/started`, whichever comes first).
    fn named(&mut self, id: &str) -> Vec<Out> {
        let Turn::Starting { interrupt } = self.turn else {
            return vec![];
        };
        self.turn = Turn::Running { id: id.into() };
        self.retrying = None;
        let mut out = vec![Out::Cursor(id.into())];
        if interrupt {
            // Interrupted before it was named: what was meant for it runs as the next turn.
            self.queue.extend(self.steers.drain(..).map(|(input, _)| input));
            out.push(self.interrupt(id.into()));
        } else {
            let steers = std::mem::take(&mut self.steers);
            out.extend(steers.into_iter().map(|(input, person)| self.steer(id, input, person)));
        }
        out
    }

    fn answer(&mut self, request_id: &str, decision: Value) -> Vec<Out> {
        self.asked_by_children.remove(request_id);
        match self.approvals.remove(request_id) {
            Some(id) => vec![Out::Reply(
                json!({"id": id, "result": {"decision": decision}}).to_string(),
            )],
            None => vec![],
        }
    }

    fn answer_question(&mut self, request_id: &str, answers: Option<serde_json::Map<String, Value>>) -> Vec<Out> {
        self.questions.remove(request_id);
        self.asked_by_children.remove(request_id);
        let Some(id) = self.approvals.remove(request_id) else {
            return vec![];
        };
        let answers: serde_json::Map<String, Value> = answers
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| {
                let values = match value {
                    Value::Array(values) => values.into_iter().filter(Value::is_string).collect(),
                    Value::String(value) => vec![Value::String(value)],
                    _ => vec![],
                };
                (key, json!({"answers": values}))
            })
            .collect();
        vec![Out::Reply(
            json!({"id": id, "result": {"answers": answers}}).to_string(),
        )]
    }

    fn response(&mut self, id: &Value, message: &Value) -> Vec<Out> {
        let Some(asked) = id.as_u64().and_then(|id| self.asked.remove(&id)) else {
            return vec![];
        };
        let error = message
            .get("error")
            .map(|error| match error.get("message").and_then(Value::as_str) {
                Some(text) => text.to_string(),
                None => error.to_string(),
            });
        let result = message.get("result").unwrap_or(&Value::Null);
        match (asked, error) {
            (Asked::Initialize | Asked::Thread, Some(error)) => {
                let what = if self.fork {
                    "fork this conversation"
                } else if self.resume {
                    "resume this conversation"
                } else {
                    "start the conversation"
                };
                vec![Out::Event(thread_refused(what, error)), Out::Close]
            }
            (Asked::Thread, None) => {
                let thread = result
                    .pointer("/thread/id")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.native_id)
                    .to_string();
                let model = result.get("model").and_then(Value::as_str).map(String::from);
                if let Some(model) = &model {
                    self.model = model.clone();
                }
                self.thread = Some(thread.clone());
                // Codex says who answers the approvals now. A resumed or forked thread can keep the reviewer it last
                // ran with (and a config.toml can name one), so one that is not this mode's is set again with the
                // first turn. A Codex too old to know the field says nothing and is never sent it.
                if let Some(now) = result.get("approvalsReviewer").and_then(Value::as_str)
                    && now != reviewer(self.mode)
                {
                    self.reviewing = now != "user";
                    self.changes.mode.get_or_insert(self.mode);
                }
                let mut out = vec![Out::Init {
                    native_id: thread.clone(),
                    model,
                    version: self.version.clone(),
                }];
                let observation = json!({"effort":result.get("reasoningEffort"),"permissions":result.get("approvalPolicy"),"provider":result.get("modelProvider")});
                if observation
                    .as_object()
                    .is_some_and(|fields| fields.values().any(|value| !value.is_null()))
                {
                    out.push(Out::Observe(observation));
                }
                // A revert in the app: Codex forgets the turns after it before it reads the next message.
                match self.rewind.take() {
                    Some(before) => out.push(self.request(
                        Asked::Revert,
                        "thread/revert",
                        json!({"threadId": thread, "beforeTurnId": before}),
                    )),
                    None => out.extend(self.next_turn()),
                }
                out
            }
            // "nexus-desktop/0.164.0 (Mac OS 26.6.0; arm64) …": Codex's version follows the client's name.
            (Asked::Initialize, None) => {
                self.version = result
                    .get("userAgent")
                    .and_then(Value::as_str)
                    .and_then(|agent| agent.split_once('/'))
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    .filter(|version| !version.is_empty())
                    .map(String::from);
                vec![]
            }
            (Asked::AccountUpdated(revision), error) => {
                if revision != self.account_revision {
                    return vec![];
                }
                vec![Out::Observe(
                    json!({ "account": if error.is_none() { result.get("account") } else { None } }),
                )]
            }
            (Asked::Compact(request_id), error) => {
                let answer = Out::Answered {
                    request_id,
                    result: error.clone().map_or(Ok(Value::Null), Err),
                };
                if let Some(error) = error {
                    vec![
                        answer,
                        Out::Result {
                            total_cost_usd: None,
                            duration_ms: 0,
                            error: Some(error),
                            interrupted: false,
                            context_window: self.context_window,
                        },
                    ]
                } else {
                    vec![answer]
                }
            }
            (Asked::Task(request_id), error) => vec![Out::Answered {
                request_id,
                result: match error {
                    Some(error) => Err(error),
                    None => Ok(result.clone()),
                },
            }],
            (Asked::Revert, error) => {
                if self.rewind_required
                    && let Some(error) = error
                {
                    self.queue.clear();
                    return vec![
                        Out::Result {
                            total_cost_usd: None,
                            duration_ms: 0,
                            error: Some(format!(
                                "Codex could not rewind this conversation ({error}). Send again to continue from the saved transcript."
                            )),
                            interrupted: false,
                            context_window: self.context_window,
                        },
                        Out::Close,
                    ];
                }
                // The files are back either way; the conversation going on with more than the files is said, not fatal.
                let mut out: Vec<Out> = error
                    .map(|error| {
                        Out::Event(WbEvent::Notice {
                            text: format!(
                                "Codex could not forget the reverted turns ({error}), so it may still refer to them."
                            ),
                        })
                    })
                    .into_iter()
                    .collect();
                out.extend(self.next_turn());
                out
            }
            (Asked::Turn, Some(error)) => {
                self.turn = Turn::Idle;
                self.queue.extend(self.steers.drain(..).map(|(input, _)| input));
                let mut out = vec![Out::Result {
                    total_cost_usd: None,
                    duration_ms: 0,
                    error: Some(error),
                    interrupted: false,
                    context_window: self.context_window,
                }];
                out.extend(self.next_turn());
                out
            }
            (Asked::Turn, None) => match result.pointer("/turn/id").and_then(Value::as_str) {
                Some(turn) => self.name(turn),
                None => vec![],
            },
            // The turn ended first, or can't be steered: the message is not lost, it runs next.
            (Asked::Steer(input, person), Some(_)) => {
                self.queue.push_back(input);
                let mut out = vec![Out::TurnQueued { person }];
                out.extend(self.next_turn());
                out
            }
            // An interrupt that finds no turn ("no active turn to interrupt") has nothing left to do.
            _ => vec![],
        }
    }

    fn completed(&mut self, turn: &Value) -> Vec<Out> {
        let id = text(turn.get("id"));
        match &self.turn {
            Turn::Running { id: current } if *current == id => {}
            // Not named yet, so whose end this is can't be told: it waits for the name (`name`). Another turn's end
            // (a late or repeated one) must not end this one.
            Turn::Starting { .. } => {
                self.early_ends.push(turn.clone());
                return vec![];
            }
            _ => return vec![],
        }
        self.turn = Turn::Idle;
        // Ended before Codex named it: what was meant for it runs next.
        self.queue.extend(self.steers.drain(..).map(|(input, _)| input));
        let status = text(turn.get("status"));
        let error = (status == "failed").then(|| {
            turn.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Codex could not finish the turn.")
                .to_string()
        });
        let mut out = vec![];
        if error.is_some() && usage_limit(turn.pointer("/error/codexErrorInfo")) {
            out.push(Out::RateLimit {
                session: None,
                weekly: None,
                session_resets: None,
                weekly_resets: None,
                refused: true,
                resets_at: self.limit_resets,
            });
        }
        out.push(Out::Result {
            total_cost_usd: None,
            duration_ms: turn.get("durationMs").and_then(Value::as_u64).unwrap_or(0),
            error,
            interrupted: status == "interrupted",
            context_window: self.context_window,
        });
        out.extend(self.next_turn());
        out
    }

    /// Codex's plan limits (`account/rateLimits/updated`, as `account/rateLimits/read` answers them in [`crate::plans`]):
    /// how much each window has used, and when the used-up one resets.
    fn rate_limits(&mut self, limits: &Value) -> Vec<Out> {
        let windows: Vec<(Option<i64>, f64, Option<i64>)> = ["primary", "secondary"]
            .iter()
            .filter_map(|key| limits.get(*key))
            .filter_map(|window| {
                let used = window.get("usedPercent").and_then(Value::as_f64)?;
                let minutes = window.get("windowDurationMins").and_then(Value::as_i64);
                let resets = window
                    .get("resetsAt")
                    .and_then(Value::as_i64)
                    .map(|seconds| seconds * 1000);
                Some((minutes, used.clamp(0.0, 100.0), resets))
            })
            .collect();
        if windows.is_empty() {
            return vec![];
        }
        let window = |length: i64| windows.iter().find(|(minutes, ..)| *minutes == Some(length));
        let used = |length: i64| window(length).map(|w| w.1);
        let resets = |length: i64| window(length).and_then(|w| w.2);
        self.limit_resets = windows
            .iter()
            .filter(|(_, used, _)| *used >= 100.0)
            .filter_map(|(.., resets)| *resets)
            .max();
        vec![Out::RateLimit {
            session: used(300),
            weekly: used(10_080),
            session_resets: resets(300),
            weekly_resets: resets(10_080),
            refused: false,
            resets_at: None,
        }]
    }

    /// One item, made under `parent` (a sub-agent's `Agent` call) when a sub-agent's thread made it.
    fn item(&mut self, item: &Value, done: bool, parent: Option<String>) -> Vec<Out> {
        let id = text(item.get("id"));
        let kind = text(item.get("type"));
        match kind.as_str() {
            "agentMessage" if done => {
                self.texts.remove(&id);
                let text = text(item.get("text"));
                if text.trim().is_empty() {
                    return vec![];
                }
                vec![Out::Event(WbEvent::Text { id, text, parent })]
            }
            "reasoning" if done => {
                let parts = |key: &str| -> Vec<String> {
                    item.get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|part| part.as_str().map(String::from))
                        .collect()
                };
                let summary = parts("summary");
                let text = if summary.is_empty() { parts("content") } else { summary }.join("\n\n");
                if text.trim().is_empty() {
                    return vec![];
                }
                vec![Out::Event(WbEvent::Thinking { id, text, parent })]
            }
            // A sub-agent compacting its own conversation is not the session's.
            "contextCompaction" if done && parent.is_none() => vec![Out::Event(WbEvent::Notice {
                text: "Codex compacted the conversation to fit its context window.".into(),
            })],
            "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall" | "webSearch" => {
                let calls = calls(item);
                let mut out = Vec::new();
                if self.started.insert(id.clone()) {
                    if let Some((_, name, input)) = calls.first() {
                        self.tools.insert(id.clone(), (name.clone(), input.clone()));
                    }
                    out.extend(calls.iter().map(|(id, name, input)| {
                        Out::Event(WbEvent::Tool {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            parent: parent.clone(),
                        })
                    }));
                }
                if done {
                    self.started.remove(&id);
                    self.tools.remove(&id);
                    let (output, is_error) = outcome(item);
                    let (output, truncated) = cap(output);
                    let content = match text(item.get("type")).as_str() {
                        "mcpToolCall" => item.pointer("/result/content"),
                        "dynamicToolCall" => item.get("contentItems"),
                        _ => None,
                    };
                    let images = crate::tool_images::extract(content);
                    out.extend(calls.into_iter().map(|(id, ..)| {
                        Out::Event(WbEvent::ToolResult {
                            images: images.clone(),
                            id,
                            output: output.clone(),
                            is_error,
                            truncated,
                            parent: parent.clone(),
                        })
                    }));
                }
                out
            }
            // A newer model's sub-agent: it started (on the item's start and again on its completion), or ended.
            "subAgentActivity" => {
                let thread = text(item.get("agentThreadId"));
                match text(item.get("kind")).as_str() {
                    "started" if !self.children.contains_key(&thread) => {
                        let path = text(item.get("agentPath"));
                        let name = path.rsplit('/').next().unwrap_or_default().replace('_', " ");
                        let model = Some(text(item.get("model"))).filter(|model| !model.is_empty());
                        let mut out = vec![agent_call(&id, &name, "", parent.clone())];
                        out.extend(self.spawned(&thread, &id, &name, parent, model));
                        out
                    }
                    "completed" => self.child_status(&thread, TaskStatus::Completed, None, None),
                    "interrupted" => self.child_status(&thread, TaskStatus::Stopped, None, None),
                    _ => vec![],
                }
            }
            // An older model's sub-agent: the spawn is its `Agent` call, which stays open until the sub-agent ends; the
            // spawn's completion names the sub-agent's thread.
            "collabAgentToolCall" if text(item.get("tool")) == "spawnAgent" => {
                let prompt = text(item.get("prompt"));
                let mut out = Vec::new();
                if self.started.insert(id.clone()) {
                    out.push(agent_call(&id, &title(&prompt), &prompt, parent.clone()));
                }
                if done {
                    self.started.remove(&id);
                    let threads: Vec<String> = item
                        .get("receiverThreadIds")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|thread| thread.as_str().map(String::from))
                        .collect();
                    if threads.is_empty() || text(item.get("status")) == "failed" {
                        out.push(Out::Event(WbEvent::ToolResult {
                            images: vec![],
                            id,
                            output: "Codex could not start the sub-agent.".into(),
                            is_error: true,
                            truncated: false,
                            parent,
                        }));
                    } else {
                        let model = Some(text(item.get("model"))).filter(|model| !model.is_empty());
                        for thread in threads {
                            out.extend(self.spawned(&thread, &id, &title(&prompt), parent.clone(), model.clone()));
                        }
                    }
                }
                out
            }
            // A wait, a close and the like: how the sub-agents it names stand.
            "collabAgentToolCall" if done => {
                let mut out = Vec::new();
                for (thread, state) in item
                    .get("agentsStates")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    let status = match text(state.get("status")).as_str() {
                        "completed" => TaskStatus::Completed,
                        "errored" | "notFound" => TaskStatus::Failed,
                        "interrupted" | "shutdown" => TaskStatus::Stopped,
                        _ => continue,
                    };
                    if let Some(child) = self.children.get_mut(thread) {
                        let message = text(state.get("message"));
                        if child.last.is_empty() && !message.trim().is_empty() {
                            child.last = message;
                        }
                    }
                    out.extend(self.child_status(thread, status, None, None));
                }
                out
            }
            _ => vec![],
        }
    }

    /// Codex no longer waits for an approval we had not answered (its turn ended). The request id is unique across
    /// threads, so a sub-agent's approval resolves here too.
    fn resolved(&mut self, params: &Value) -> Vec<Out> {
        let id = params.get("requestId");
        let found = self
            .approvals
            .iter()
            .find(|(_, raw)| Some(*raw) == id)
            .map(|(ours, _)| ours.clone());
        match found {
            Some(request_id) => {
                self.approvals.remove(&request_id);
                self.questions.remove(&request_id);
                self.asked_by_children.remove(&request_id);
                // Its command will not run now: an ended child it held is done.
                let item = request_id.split('#').next().unwrap_or_default().to_string();
                let mut out = vec![Out::Cancelled { request_id }];
                for (thread, child) in self.children.iter_mut() {
                    if child.open.remove(&item) && child.ended && child.open.is_empty() {
                        out.push(Out::Background(Background::Ended(thread.clone())));
                    }
                }
                out
            }
            None => vec![],
        }
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<Out> {
        if method == "account/updated" {
            self.account_revision += 1;
            self.asked.retain(|_, asked| !matches!(asked, Asked::AccountUpdated(_)));
            return vec![
                Out::Observe(json!({"account":Value::Null})),
                self.request(
                    Asked::AccountUpdated(self.account_revision),
                    "account/read",
                    json!({"refreshToken":false}),
                ),
            ];
        }
        if method == "mcpServer/startupStatus/updated"
            && params
                .get("threadId")
                .and_then(Value::as_str)
                .is_some_and(|id| Some(id) == self.thread.as_deref())
        {
            return vec![Out::Observe(json!({"mcpUpdate": params}))];
        }
        if method == "skills/changed" {
            return vec![Out::Observe(json!({"skills":Value::Null}))];
        }
        if method == "account/rateLimits/updated" {
            return self.rate_limits(params.get("rateLimits").unwrap_or(&Value::Null));
        }
        if method == "serverRequest/resolved" {
            return self.resolved(params);
        }
        let thread = params.get("threadId").and_then(Value::as_str);
        match (thread, &self.thread) {
            (Some(thread), Some(ours)) if thread != ours => {
                if !self.children.contains_key(thread) {
                    // Another thread we did not see spawned: not this session's.
                    return vec![];
                }
                // Its work names it (`parent`), after the session's turn too: the core opens no turn for it.
                let thread = thread.to_string();
                self.child_notification(&thread, method, params)
            }
            _ => self.main_notification(method, params),
        }
    }

    /// A notification about the session's own thread.
    fn main_notification(&mut self, method: &str, params: &Value) -> Vec<Out> {
        match method {
            "thread/name/updated"
                if self
                    .thread
                    .as_deref()
                    .is_some_and(|thread| params.get("threadId").and_then(Value::as_str) == Some(thread)) =>
            {
                params
                    .get("threadName")
                    .and_then(Value::as_str)
                    .and_then(crate::title::clean)
                    .map(Out::Title)
                    .into_iter()
                    .collect()
            }
            "turn/started" => match params.pointer("/turn/id").and_then(Value::as_str) {
                Some(turn) => {
                    // A spontaneous continuation needs its turn id before an interrupt can reach it.
                    let own = matches!(self.turn, Turn::Idle);
                    if own {
                        self.turn = Turn::Starting { interrupt: false };
                    }
                    let mut out = self.name(turn);
                    // Codex goes on by itself (a background command ended, a goal is not met yet): the turn says so,
                    // or it reads as the turn before still working. After `name`, whose cursor opens the turn.
                    if own {
                        out.push(Out::Event(WbEvent::Notice {
                            text: "Codex started this turn on its own, without a new message.".into(),
                        }));
                    }
                    out
                }
                None => vec![],
            },
            // Codex retries a failed request by itself (the model at capacity, the stream cut) and says so only here:
            // without it the turn shows nothing while it waits. Only the turn under way's: a notice outside a turn
            // would open one that Codex never ends. One that will not retry ends the turn, which says it.
            "error"
                if params.get("willRetry").and_then(Value::as_bool) == Some(true)
                    && matches!(&self.turn, Turn::Running { id }
                        if params.get("turnId").and_then(Value::as_str).is_none_or(|turn| turn == id)) =>
            {
                let error = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("an error")
                    .to_string();
                if self.retrying.as_ref() == Some(&error) {
                    return vec![];
                }
                self.retrying = Some(error.clone());
                vec![Out::Event(WbEvent::Notice {
                    text: format!("Codex is retrying after an error: {error}"),
                })]
            }
            "turn/completed" => self.completed(params.get("turn").unwrap_or(&Value::Null)),
            "item/agentMessage/delta" => {
                let id = text(params.get("itemId"));
                let so_far = self.texts.entry(id.clone()).or_default();
                so_far.push_str(params.get("delta").and_then(Value::as_str).unwrap_or_default());
                vec![Out::Delta {
                    id,
                    text: so_far.clone(),
                    parent: None,
                }]
            }
            "item/started" => self.item(params.get("item").unwrap_or(&Value::Null), false, None),
            // One of the person's messages, recorded in the conversation: a steer only once Codex takes it in.
            "item/completed" if params.pointer("/item/type").and_then(Value::as_str) == Some("userMessage") => {
                let texts = params
                    .pointer("/item/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .map(|part| text(part.get("text")))
                    .collect();
                vec![Out::Taken { texts }]
            }
            "item/completed" => self.item(params.get("item").unwrap_or(&Value::Null), true, None),
            // Only the turn under way's: a plan outside it would open a turn of the session's that Codex never ends.
            "turn/plan/updated" if matches!(self.turn, Turn::Running { .. }) => self.plan(params, None),
            "thread/tokenUsage/updated" => {
                let usage = params.get("tokenUsage").unwrap_or(&Value::Null);
                if let Some(window) = usage.get("modelContextWindow").and_then(Value::as_u64) {
                    self.context_window = Some(window);
                }
                // A resumed thread reports its last turn's usage again: only the turn under way's calls count.
                let turn = params.get("turnId").and_then(Value::as_str);
                if !matches!(&self.turn, Turn::Running { id } if turn.is_none_or(|turn| turn == id)) {
                    return vec![];
                }
                self.call(usage.get("last").unwrap_or(&Value::Null), None)
            }
            _ => vec![],
        }
    }

    /// One of Codex's own requests: an approval, or something Nexus does not do (answered, so nothing waits).
    fn asks(&mut self, method: &str, id: Value, params: &Value) -> Vec<Out> {
        if method == "item/tool/requestUserInput" {
            return self.ask_question(id, params);
        }
        let command = match method {
            "item/commandExecution/requestApproval" => true,
            "item/fileChange/requestApproval" => false,
            _ => {
                return vec![Out::Reply(
                    json!({"id": id, "error": {"code": -32601, "message": "Nexus does not handle this request"}})
                        .to_string(),
                )];
            }
        };
        let item = text(params.get("itemId"));
        let (tool, input) = self.tools.get(&item).cloned().unwrap_or_else(|| {
            if command {
                ("Bash".into(), json!({"command": text(params.get("command"))}))
            } else {
                ("Edit".into(), json!({}))
            }
        });
        // An edit whose files Codex never named (no `item/started` seen for it, as for a sub-agent whose start was
        // missed): the card can show no path or diff, so it is allowed once at most, never for the whole session.
        let blind = !command && input.get("file_path").and_then(Value::as_str).is_none_or(str::is_empty);
        let (request_id, parent) = self.register_approval(id, params, &item);
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .map(String::from);
        vec![Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id: Some(item),
            tool,
            input,
            description: if blind {
                Some(reason.unwrap_or_else(|| "Codex did not say which files it will change.".into()))
            } else {
                reason
            },
            suggestions: if blind {
                Vec::new()
            } else {
                suggestions(command, params)
            },
            parent,
        })]
    }

    /// Registers an agent request with one ID and sub-agent attribution scheme for every approval path.
    fn register_approval(&mut self, id: Value, params: &Value, item: &str) -> (String, Option<String>) {
        // Codex numbers its requests from 0 in each run; the item id keeps ours apart across runs.
        let request_id = format!(
            "{item}#{}",
            id.as_u64().map_or_else(|| text(Some(&id)), |n| n.to_string())
        );
        self.approvals.insert(request_id.clone(), id);
        // A sub-agent's request names its thread: the approval names the sub-agent's call.
        let parent = params
            .get("threadId")
            .and_then(Value::as_str)
            .and_then(|thread| self.children.get(thread))
            .map(|child| child.call.clone());
        if parent.is_some() {
            self.asked_by_children.insert(request_id.clone());
        }
        (request_id, parent)
    }

    fn ask_question(&mut self, id: Value, params: &Value) -> Vec<Out> {
        let questions = params.get("questions").and_then(Value::as_array);
        let valid = questions.is_some_and(|questions| {
            !questions.is_empty()
                && questions.iter().all(|question| {
                    question
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                        && question.get("question").and_then(Value::as_str).is_some()
                })
        });
        let item = text(params.get("itemId"));
        if !valid || item.is_empty() {
            return vec![Out::Reply(
                json!({"id": id, "error": {"code": -32602, "message": "Invalid question request"}}).to_string(),
            )];
        }
        let (request_id, parent) = self.register_approval(id, params, &item);
        self.questions.insert(request_id.clone());
        vec![Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id: Some(item),
            tool: "AskUserQuestion".into(),
            input: json!({"questions": questions, "isBlocking": params.get("isBlocking").and_then(Value::as_bool).unwrap_or(true)}),
            description: None,
            suggestions: vec![],
            parent,
        })]
    }
}

impl Protocol for Driver {
    fn launch_model(&mut self, model: &str) {
        if !model.is_empty() && !nexus_client::agents::is_auto_name(Some(model)) {
            self.model = model.to_string();
        }
    }

    fn open(&mut self) -> Vec<Out> {
        let (approval, sandbox) = policy(self.mode);
        let mut params = json!({"cwd": self.cwd, "approvalPolicy": approval, "sandbox": sandbox,
            "developerInstructions": self.prompt});
        if self.mode == PermissionMode::Auto {
            params["approvalsReviewer"] = json!(reviewer(self.mode));
        }
        let method = if self.resume {
            params["threadId"] = json!(self.native_id);
            if self.fork { "thread/fork" } else { "thread/resume" }
        } else {
            "thread/start"
        };
        vec![
            self.request(
                Asked::Initialize,
                "initialize",
                json!({"clientInfo": {"name": "nexus-desktop", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"experimentalApi": true}}),
            ),
            Out::Reply(json!({"method": "initialized"}).to_string()),
            self.request(Asked::Thread, method, params),
        ]
    }

    fn say(&mut self, say: Say) -> Vec<Out> {
        let ack = |request_id: String| {
            vec![Out::Ack {
                request_id,
                error: None,
            }]
        };
        match say {
            // Claude Code's command; the workbench sends it to no other agent.
            Say::Rename { .. } => vec![],
            Say::ForgetInspection { request } => {
                self.asked
                    .retain(|_, asked| !matches!(asked, Asked::Task(waiting) if waiting == &request));
                vec![]
            }
            Say::Inspect { request, field, cursor } => {
                if field == "account" {
                    self.account_revision += 1;
                }
                let (method, params) = match field {
                    "account" => ("account/read", json!({"refreshToken":false})),
                    "mcp" => (
                        "mcpServerStatus/list",
                        json!({"threadId":self.thread,"cursor":cursor,"limit":100,"detail":"toolsAndAuthOnly"}),
                    ),
                    "skills" => ("skills/list", json!({"cwds":[self.cwd],"forceReload":false})),
                    _ => {
                        return vec![Out::Answered {
                            request_id: request,
                            result: Err("Inventory unavailable.".into()),
                        }];
                    }
                };
                vec![self.request(Asked::Task(request), method, params)]
            }
            Say::Compact {
                request,
                instructions: _,
            } => match self.thread.clone() {
                Some(thread) if matches!(self.turn, Turn::Idle) => vec![self.request(
                    Asked::Compact(request),
                    "thread/compact/start",
                    json!({ "threadId": thread }),
                )],
                _ => vec![Out::Answered {
                    request_id: request,
                    result: Err("Wait for an initialized, idle conversation.".into()),
                }],
            },
            Say::User { text, images } => {
                self.queue.push_back(input(&text, &images));
                self.next_turn()
            }
            // Codex 0.164.0 has no background commands: a command runs inside its turn, and its output is its result.
            Say::TaskBackground { request, .. } => vec![Out::Answered {
                request_id: request,
                result: Err("Codex can't move a command to the background.".into()),
            }],
            Say::TaskOutput { request, .. } => vec![Out::Answered {
                request_id: request,
                result: Err("Codex has no background command to read.".into()),
            }],
            // A child stops by its own turn; its command under way runs on to its end (0.164.0, `late-child`).
            Say::TaskStop { request, task_id } => {
                match self.children.get(&task_id).and_then(|child| child.turn.clone()) {
                    Some(turn) => vec![self.request(
                        Asked::Task(request),
                        "turn/interrupt",
                        json!({"threadId": task_id, "turnId": turn}),
                    )],
                    None => vec![Out::Answered {
                        request_id: request,
                        result: Err("Codex has no turn of that sub-agent under way.".into()),
                    }],
                }
            }
            Say::Steer { text, images, person } => {
                let steer = input(&text, &images);
                match &self.turn {
                    Turn::Running { id } => {
                        let id = id.clone();
                        vec![self.steer(&id, steer, person)]
                    }
                    Turn::Starting { .. } => {
                        self.steers.push((steer, person));
                        vec![]
                    }
                    // No turn: one waiting for the thread takes it in; otherwise it starts one.
                    Turn::Idle => match self.queue.front_mut() {
                        Some(Value::Array(first)) => {
                            first.extend(steer.as_array().cloned().unwrap_or_default());
                            vec![]
                        }
                        _ => {
                            self.queue.push_back(steer);
                            self.next_turn()
                        }
                    },
                }
            }
            Say::Interrupt { .. } => match &mut self.turn {
                Turn::Running { id } => {
                    let id = id.clone();
                    vec![self.interrupt(id)]
                }
                Turn::Starting { interrupt } => {
                    *interrupt = true;
                    vec![]
                }
                // The thread is not live yet: the message waiting for it is the turn, and it ends before it starts.
                Turn::Idle => match self.queue.pop_front() {
                    Some(_) => vec![Out::Result {
                        total_cost_usd: None,
                        duration_ms: 0,
                        error: None,
                        interrupted: true,
                        context_window: self.context_window,
                    }],
                    None => vec![],
                },
            },
            // Codex takes these with the next turn; a turn under way keeps what it started with.
            Say::Mode { request, mode } => {
                self.mode = mode;
                self.changes.mode = Some(mode);
                ack(request)
            }
            Say::Model { request, model } => {
                // Later: back to the default (None) waits for the next run, which starts without `-c model`; send
                // the organization's default here if a session must switch back mid-run.
                self.changes.model = model;
                ack(request)
            }
            Say::Effort { request, effort } => {
                self.changes.effort = effort;
                ack(request)
            }
            // Later: Codex's service tier is not supported yet; only "off" reaches here.
            Say::Fast { request, .. } => ack(request),
            Say::Allow {
                request_id,
                answers,
                remember,
                ..
            } => {
                if self.questions.contains(&request_id) {
                    self.answer_question(&request_id, answers)
                } else {
                    self.answer(&request_id, remember.unwrap_or_else(|| json!("accept")))
                }
            }
            Say::Deny { request_id, message } => {
                if self.questions.contains(&request_id) {
                    return self.answer_question(&request_id, None);
                }
                // A sub-agent refuses messages of its own, and its reason is not for the session's agent: a child's
                // decline goes without one (the log keeps it, in `approvalResolved`).
                let child = self.asked_by_children.contains(&request_id);
                let mut out = self.answer(&request_id, json!("decline"));
                // A decline carries no reason: the reason reaches the turn under way as the person's own words.
                if let (false, Some(reason), Turn::Running { id }) =
                    (child, message.filter(|m| !m.trim().is_empty()), &self.turn)
                {
                    let params = json!({"threadId": self.thread, "expectedTurnId": id,
                        "input": [{"type": "text", "text": reason, "text_elements": []}]});
                    out.push(self.request(Asked::Other, "turn/steer", params));
                }
                out
            }
        }
    }

    fn read(&mut self, line: &str) -> Vec<Out> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        let params = message.get("params").unwrap_or(&Value::Null);
        match (message.get("method").and_then(Value::as_str), message.get("id")) {
            (Some(method), Some(id)) => self.asks(method, id.clone(), params),
            (Some(method), None) => self.notification(method, params),
            (None, Some(id)) => self.response(id, &message),
            (None, None) => vec![],
        }
    }
}

/// A sub-agent as Claude Code's `Agent` tool call, so the screens group its work and track it as they do Claude's.
fn agent_call(id: &str, description: &str, prompt: &str, parent: Option<String>) -> Out {
    Out::Event(WbEvent::Tool {
        id: id.into(),
        name: "Agent".into(),
        input: json!({"description": description, "prompt": prompt}),
        parent,
    })
}

/// A short title for a sub-agent from what it was asked: the first line, cut at 80 characters.
fn title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(80) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

/// An item's tool calls as Claude Code's tools: (id, name, input). A file change is one call per file, the first under
/// the item's id.
fn calls(item: &Value) -> Vec<(String, String, Value)> {
    let id = text(item.get("id"));
    match text(item.get("type")).as_str() {
        "commandExecution" => vec![(id, "Bash".into(), json!({"command": command(item)}))],
        "fileChange" => item
            .get("changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(n, change)| {
                let id = if n == 0 { id.clone() } else { format!("{id}:{n}") };
                let (name, input) = file_change(change);
                (id, name, input)
            })
            .collect(),
        "mcpToolCall" => vec![(
            id,
            format!("mcp__{}__{}", text(item.get("server")), text(item.get("tool"))),
            item.get("arguments").cloned().unwrap_or(Value::Null),
        )],
        "dynamicToolCall" => vec![(
            id,
            text(item.get("tool")),
            item.get("arguments").cloned().unwrap_or(Value::Null),
        )],
        "webSearch" => vec![(id, "WebSearch".into(), json!({"query": text(item.get("query"))}))],
        _ => vec![],
    }
}

/// The command as typed: Codex's own reading of it when it is one command, else the line it runs.
fn command(item: &Value) -> String {
    match item.get("commandActions").and_then(Value::as_array).map(Vec::as_slice) {
        Some([action]) if action.get("command").and_then(Value::as_str).is_some() => text(action.get("command")),
        _ => text(item.get("command")),
    }
}

/// One file of a change: a new file is `Write` with its content; a changed or deleted one is `Edit`, with the lines
/// its diff takes out and puts in.
fn file_change(change: &Value) -> (String, Value) {
    let path = text(change.get("path"));
    let diff = text(change.get("diff"));
    match change.pointer("/kind/type").and_then(Value::as_str) {
        Some("add") => ("Write".into(), json!({"file_path": path, "content": diff})),
        Some("delete") => (
            "Edit".into(),
            json!({"file_path": path, "old_string": diff, "new_string": ""}),
        ),
        _ => {
            let path = change
                .pointer("/kind/move_path")
                .and_then(Value::as_str)
                .map_or(path, String::from);
            let (mut before, mut after) = (Vec::new(), Vec::new());
            // Later: hunks are joined, so lines between two hunks don't show; parse @@ ranges if that misleads.
            for line in diff.lines() {
                if line.starts_with("@@") || line.starts_with('\\') {
                    continue;
                }
                // By prefix, never by byte: a line may start with a multi-byte character, and slicing it would panic.
                if let Some(rest) = line.strip_prefix('-') {
                    before.push(rest);
                } else if let Some(rest) = line.strip_prefix('+') {
                    after.push(rest);
                } else {
                    let rest = line.strip_prefix(' ').unwrap_or(line);
                    before.push(rest);
                    after.push(rest);
                }
            }
            (
                "Edit".into(),
                json!({"file_path": path, "old_string": before.join("\n"), "new_string": after.join("\n")}),
            )
        }
    }
}

/// What a finished item gave back, and whether it failed.
fn outcome(item: &Value) -> (String, bool) {
    let status = text(item.get("status"));
    let declined = status == "declined";
    let failed = declined || status == "failed";
    let output = match text(item.get("type")).as_str() {
        "commandExecution" => {
            let exit = item.get("exitCode").and_then(Value::as_i64);
            let output = text(item.get("aggregatedOutput"));
            return (
                if declined { "Declined.".into() } else { output },
                failed || exit.is_some_and(|code| code != 0),
            );
        }
        "mcpToolCall" => match item.pointer("/error/message").and_then(Value::as_str) {
            Some(error) => return (error.to_string(), true),
            None => blocks(item.pointer("/result/content")),
        },
        "dynamicToolCall" => {
            let output = blocks(item.get("contentItems"));
            return (output, failed || item.get("success") == Some(&Value::Bool(false)));
        }
        _ => String::new(),
    };
    if declined {
        ("Declined.".into(), true)
    } else if failed {
        ("The change was not applied.".into(), true)
    } else {
        (output, false)
    }
}

/// The text of a list of content blocks.
fn blocks(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `program` (a path or a name) is a POSIX shell, whose `-c` line is the command itself.
fn is_shell(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    matches!(name, "sh" | "bash" | "zsh" | "dash")
}

/// One argument as a person reads it in a shell: as it is when it is plain, else single-quoted.
fn shown_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if plain {
        arg.to_string()
    } else {
        crate::open::shell_quote(arg)
    }
}

/// The "always" answers Codex offers for an approval, as buttons: its `availableDecisions` for a command (else
/// `acceptForSession`), `acceptForSession` for edits. The raw decision is what Allow sends back.
fn suggestions(command: bool, params: &Value) -> Vec<PermissionSuggestion> {
    let offered = match params.get("availableDecisions").and_then(Value::as_array) {
        Some(decisions) if command => decisions.clone(),
        _ => vec![json!("acceptForSession")],
    };
    offered
        .into_iter()
        .filter_map(|raw| {
            let (kind, label) = match &raw {
                Value::String(kind) if kind == "acceptForSession" => (
                    kind.clone(),
                    if command {
                        "Always allow this command for this session"
                    } else {
                        "Allow all edits for this session"
                    }
                    .to_string(),
                ),
                Value::Object(decision) => {
                    let (kind, body) = decision.iter().next()?;
                    let label = match kind.as_str() {
                        "acceptWithExecpolicyAmendment" => {
                            let argv: Vec<&str> = body
                                .get("execpolicy_amendment")?
                                .as_array()?
                                .iter()
                                .filter_map(Value::as_str)
                                .collect();
                            // `/bin/zsh -lc 'cmd'` reads as `cmd`. Any other program shows by name, each argument
                            // quoted as needed: `python3 -c '...'` must not read as the Python code alone.
                            let shown = match argv.as_slice() {
                                [shell, flag, line] if is_shell(shell) && (*flag == "-lc" || *flag == "-c") => {
                                    line.to_string()
                                }
                                _ => argv.iter().map(|arg| shown_arg(arg)).collect::<Vec<_>>().join(" "),
                            };
                            format!("Always allow {shown} from now on")
                        }
                        "applyNetworkPolicyAmendment" => {
                            let rule = body.get("network_policy_amendment")?;
                            let verb = if text(rule.get("action")) == "deny" {
                                "block"
                            } else {
                                "allow"
                            };
                            format!("Always {verb} {} from now on", text(rule.get("host")))
                        }
                        _ => return None,
                    };
                    (kind.clone(), label)
                }
                _ => return None,
            };
            Some(PermissionSuggestion { kind, label, raw })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_mcp_and_dynamic_tool_images() {
        for item in [
            json!({"id":"shot", "type":"mcpToolCall", "server":"browser", "tool":"screenshot",
                "status":"completed", "result":{"content":[{"type":"text", "text":"Captured"},
                    {"type":"image", "mimeType":"image/png", "data":"aGk="}]}}),
            json!({"id":"shot", "type":"dynamicToolCall", "tool":"browser_screenshot", "success":true,
                "contentItems":[{"type":"inputText", "text":"Captured"},
                    {"type":"inputImage", "imageUrl":"data:image/png;base64,aGk="}]}),
        ] {
            let out = lines(
                &mut codex(false),
                &[json!({"method":"item/completed", "params":{"threadId":"thr_old", "item":item}})],
            );
            assert!(events(&out).iter().any(|event| matches!(event,
                WbEvent::ToolResult { id, output, images, is_error: false, truncated: false, .. }
                if id == "shot" && output == "Captured" && images.len() == 1 && images[0].data == "aGk=")));
        }
    }

    fn codex(resume: bool) -> Driver {
        Driver::new(&Start {
            mode: PermissionMode::Default,
            model: None,
            effort: None,
            fast: false,
            native_id: "thr_old",
            resume,
            fork: false,
            cwd: "/r",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        })
    }

    fn lines(codex: &mut Driver, lines: &[Value]) -> Vec<Out> {
        lines.iter().flat_map(|line| codex.read(&line.to_string())).collect()
    }

    /// The lines an `Out::Reply` list writes, parsed.
    fn written(out: &[Out]) -> Vec<Value> {
        out.iter()
            .filter_map(|out| match out {
                Out::Reply(line) => serde_json::from_str(line).ok(),
                _ => None,
            })
            .collect()
    }

    fn user(text: &str) -> Say {
        Say::User {
            text: text.into(),
            images: vec![],
        }
    }

    #[test]
    fn a_native_title_belongs_only_to_its_own_thread() {
        let mut codex = codex(false);
        codex.thread = Some("ours".into());
        assert_eq!(
            codex.notification(
                "thread/name/updated",
                &json!({"threadId": "ours", "threadName": "Fix login redirect"})
            ),
            vec![Out::Title("Fix login redirect".into())]
        );
        assert!(
            codex
                .notification(
                    "thread/name/updated",
                    &json!({"threadId": "elsewhere", "threadName": "Another title"})
                )
                .is_empty()
        );
        assert!(
            codex
                .notification("thread/name/updated", &json!({"threadId": "ours", "threadName": ""}))
                .is_empty()
        );
        assert!(
            codex
                .notification("thread/name/updated", &json!({"threadName": "Missing thread"}))
                .is_empty()
        );
    }

    #[test]
    fn builds_the_arguments() {
        assert_eq!(
            args(None, None),
            ["-c", "features.default_mode_request_user_input=true", "app-server"]
        );
        assert_eq!(
            args(Some("gpt-6-sol"), Some(Effort::Max)),
            [
                "-c",
                "model=\"gpt-6-sol\"",
                "-c",
                "model_reasoning_effort=\"max\"",
                "-c",
                "features.default_mode_request_user_input=true",
                "app-server"
            ]
        );
    }

    #[test]
    fn preserves_each_high_effort_in_launch_overrides_and_turn_requests() {
        for effort in [Effort::Xhigh, Effort::Max, Effort::Ultra] {
            assert!(args(None, Some(effort)).contains(&format!("model_reasoning_effort=\"{}\"", effort.as_str())));
            let mut c = codex(false);
            c.open();
            lines(&mut c, &[json!({"id":2,"result":{"thread":{"id":"thr_1"}}})]);
            c.say(Say::Effort {
                request: "effort".into(),
                effort: Some(effort),
            });
            let requests = written(&c.say(user("next")));
            assert_eq!(requests[0]["method"], "turn/start");
            assert_eq!(requests[0]["params"]["effort"], effort.as_str());
        }
    }

    #[test]
    fn opens_a_new_thread_or_resumes_one() {
        let first = written(&codex(false).open());
        assert_eq!(first[0]["method"], "initialize");
        assert_eq!(first[0]["params"]["capabilities"]["experimentalApi"], true);
        assert_eq!(first[1], json!({"method": "initialized"}));
        assert_eq!(
            first[2],
            json!({"id": 2, "method": "thread/start", "params": {"cwd": "/r", "approvalPolicy": "untrusted", "sandbox": "workspace-write", "developerInstructions": crate::driver::instructions()}})
        );
        let mut resumed = codex(true);
        let again = written(&resumed.open());
        assert_eq!(again[2]["method"], "thread/resume");
        assert_eq!(again[2]["params"]["threadId"], "thr_old");
        // The resumed thread's last usage, reported again, is no call of this run.
        let out = lines(
            &mut resumed,
            &[
                json!({"id":2,"result":{"thread":{"id":"thr_old"},"model":"gpt-6-luna"}}),
                json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thr_old","turnId":"turn_9","tokenUsage":{"last":{"inputTokens":284,"outputTokens":5},"modelContextWindow":258400}}}),
            ],
        );
        assert!(matches!(out.as_slice(), [Out::Init { .. }]), "{out:?}");
    }

    #[test]
    fn auto_has_codex_review_its_own_approvals() {
        let mut c = Driver::new(&Start {
            mode: PermissionMode::Auto,
            model: None,
            effort: None,
            fast: false,
            native_id: "thr_old",
            resume: false,
            fork: false,
            cwd: "/r",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        });
        let start = &written(&c.open())[2];
        assert_eq!(start["params"]["approvalPolicy"], "on-request");
        assert_eq!(start["params"]["sandbox"], "workspace-write");
        assert_eq!(start["params"]["approvalsReviewer"], "auto_review");
        // The other modes leave the field out when a thread opens.
        assert!(
            written(&codex(false).open())[2]["params"]
                .get("approvalsReviewer")
                .is_none()
        );
    }

    #[test]
    fn drops_the_reverted_turns_before_the_first_message() {
        let mut c = Driver::new(&Start {
            mode: PermissionMode::Default,
            model: None,
            effort: None,
            fast: false,
            native_id: "thr_old",
            resume: true,
            fork: false,
            cwd: "/r",
            rewind: Some("turn_2"),
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        });
        c.open();
        assert!(c.say(user("next")).is_empty());
        let out = lines(&mut c, &[json!({"id":2,"result":{"thread":{"id":"thr_old"}}})]);
        let sent = written(&out);
        assert_eq!(
            sent,
            [json!({"id": 3, "method": "thread/revert", "params": {"threadId": "thr_old", "beforeTurnId": "turn_2"}})]
        );
        // The message goes once Codex answered, and a failure is said but stops nothing.
        let out = lines(&mut c, &[json!({"id":3,"error":{"message":"unknown turn"}})]);
        assert!(matches!(&out[0], Out::Event(WbEvent::Notice { text }) if text.contains("unknown turn")));
        assert_eq!(written(&out)[0]["method"], "turn/start");
        // Each turn Codex names is where a later revert would cut.
        let out = lines(&mut c, &[json!({"id":4,"result":{"turn":{"id":"turn_2b"}}})]);
        assert_eq!(out, [Out::Cursor("turn_2b".into())]);
    }

    /// Lines as `codex-cli` 0.163.0 wrote them (trimmed), from a recorded session: a command that asks first, then the
    /// answer streamed.
    #[test]
    fn translates_a_recorded_turn() {
        let mut c = codex(false);
        c.open();
        // A message before the thread exists waits for it.
        assert!(c.say(user("Run echo hi > hi.txt")).is_empty());
        let out = lines(
            &mut c,
            &[
                json!({"id":1,"result":{"userAgent":"nexus-desktop/0.163.0","codexHome":"/h/.codex"}}),
                json!({"method":"remoteControl/status/changed","params":{"status":"disabled"}}),
                json!({"id":2,"result":{"thread":{"id":"thr_1","model":"gpt-6-luna","status":{"type":"idle"}},"model":"gpt-6-luna","approvalPolicy":"untrusted"}}),
            ],
        );
        assert_eq!(
            out[0],
            Out::Init {
                native_id: "thr_1".into(),
                model: Some("gpt-6-luna".into()),
                version: Some("0.163.0".into())
            }
        );
        let start = &written(&out)[0];
        assert_eq!(start["method"], "turn/start");
        assert_eq!(
            start["params"],
            json!({"threadId": "thr_1", "input": [{"type": "text", "text": "Run echo hi > hi.txt", "text_elements": []}]})
        );

        let out = lines(
            &mut c,
            &[
                json!({"id":3,"result":{"turn":{"id":"turn_1","items":[],"status":"inProgress"}}}),
                json!({"method":"turn/started","params":{"threadId":"thr_1","turn":{"id":"turn_1","status":"inProgress"}}}),
                json!({"method":"item/started","params":{"item":{"type":"userMessage","id":"u1","content":[]},"threadId":"thr_1","turnId":"turn_1"}}),
                json!({"method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-1","command":"/bin/zsh -lc 'echo hi > hi.txt'","cwd":"/r","status":"inProgress","commandActions":[{"type":"unknown","command":"echo hi > hi.txt"}],"aggregatedOutput":null,"exitCode":null},"threadId":"thr_1","turnId":"turn_1"}}),
                json!({"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"thr_1","turnId":"turn_1","itemId":"exec-1","command":"/bin/zsh -lc 'echo hi > hi.txt'","cwd":"/r","proposedExecpolicyAmendment":["/bin/zsh","-lc","echo hi > hi.txt"],"availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["/bin/zsh","-lc","echo hi > hi.txt"]}},"cancel"]}}),
            ],
        );
        assert_eq!(out[0], Out::Cursor("turn_1".into()));
        let out = &out[1..];
        assert_eq!(
            out[0],
            Out::Event(WbEvent::Tool {
                id: "exec-1".into(),
                name: "Bash".into(),
                input: json!({"command": "echo hi > hi.txt"}),
                parent: None
            })
        );
        let Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id,
            tool,
            input,
            suggestions,
            parent,
            ..
        }) = &out[1]
        else {
            panic!("not an approval: {out:?}");
        };
        assert_eq!((request_id.as_str(), tool.as_str()), ("exec-1#0", "Bash"));
        // The session's own thread asks: no sub-agent.
        assert_eq!(parent, &None);
        assert_eq!(tool_use_id.as_deref(), Some("exec-1"));
        assert_eq!(input, &json!({"command": "echo hi > hi.txt"}));
        let labels: Vec<&str> = suggestions.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Always allow echo hi > hi.txt from now on"]);

        // "Always allow" sends that decision back, under Codex's own request id.
        let remember = suggestions[0].raw.clone();
        let answer = written(&c.say(Say::Allow {
            request_id: request_id.clone(),
            input: Value::Null,
            answers: None,
            remember: Some(remember.clone()),
        }));
        assert_eq!(answer, [json!({"id": 0, "result": {"decision": remember}})]);

        let out = lines(
            &mut c,
            &[
                json!({"method":"serverRequest/resolved","params":{"threadId":"thr_1","requestId":0}}),
                json!({"method":"item/completed","params":{"item":{"type":"commandExecution","id":"exec-1","command":"/bin/zsh -lc 'echo hi > hi.txt'","status":"completed","commandActions":[{"type":"unknown","command":"echo hi > hi.txt"}],"aggregatedOutput":null,"exitCode":0},"threadId":"thr_1","turnId":"turn_1"}}),
                json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thr_1","turnId":"turn_1","tokenUsage":{"total":{"inputTokens":14225},"last":{"totalTokens":14255,"inputTokens":14225,"cachedInputTokens":11520,"cacheWriteInputTokens":0,"outputTokens":30,"reasoningOutputTokens":0},"modelContextWindow":258400}}}),
                json!({"method":"item/started","params":{"item":{"type":"agentMessage","id":"msg_1","text":"","phase":"final_answer"},"threadId":"thr_1","turnId":"turn_1"}}),
                json!({"method":"item/agentMessage/delta","params":{"threadId":"thr_1","turnId":"turn_1","itemId":"msg_1","delta":"do"}}),
                json!({"method":"item/agentMessage/delta","params":{"threadId":"thr_1","turnId":"turn_1","itemId":"msg_1","delta":"ne"}}),
                json!({"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"done","phase":"final_answer"},"threadId":"thr_1","turnId":"turn_1"}}),
                json!({"method":"turn/completed","params":{"threadId":"thr_1","turn":{"id":"turn_1","items":[],"status":"completed","error":null,"durationMs":4495}}}),
            ],
        );
        assert_eq!(
            out,
            vec![
                Out::Event(WbEvent::ToolResult {
                    images: vec![],
                    id: "exec-1".into(),
                    output: String::new(),
                    is_error: false,
                    truncated: false,
                    parent: None,
                }),
                Out::Call {
                    model: "gpt-6-luna".into(),
                    usage: Usage {
                        input_tokens: 2705,
                        output_tokens: 30,
                        cache_read_tokens: 11520,
                        cache_write_tokens: 0,
                        cache_writes: None,
                    },
                    parent: None
                },
                Out::Delta {
                    id: "msg_1".into(),
                    text: "do".into(),
                    parent: None
                },
                Out::Delta {
                    id: "msg_1".into(),
                    text: "done".into(),
                    parent: None
                },
                Out::Event(WbEvent::Text {
                    id: "msg_1".into(),
                    text: "done".into(),
                    parent: None
                }),
                Out::Result {
                    total_cost_usd: None,
                    duration_ms: 4495,
                    error: None,
                    interrupted: false,
                    context_window: Some(258400)
                },
            ]
        );
    }

    fn running(c: &mut Driver) {
        c.open();
        lines(
            c,
            &[json!({"id":2,"result":{"thread":{"id":"thr_1"},"model":"gpt-6-luna"}})],
        );
    }

    #[test]
    fn reads_a_message_back_when_codex_records_it() {
        let mut c = codex(false);
        running(&mut c);
        let item = |thread: &str| {
            json!({"method":"item/completed","params":{"threadId":thread,"turnId":"turn_1","item":{"type":"userMessage","id":"u2",
                "content":[{"type":"image","url":"data:image/png;base64,x"},{"type":"text","text":"also this","text_elements":[]}]}}})
        };
        assert_eq!(
            lines(&mut c, &[item("thr_1")]),
            [Out::Taken {
                texts: vec!["also this".into()]
            }]
        );
        // Another thread's (a sub-agent's prompt, or one not this session's) is not the person's message.
        assert!(lines(&mut c, &[item("thr_other")]).is_empty());
    }

    #[test]
    fn a_message_during_a_turn_waits_for_it_to_end() {
        let mut c = codex(false);
        running(&mut c);
        assert_eq!(written(&c.say(user("one"))).len(), 1);
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        // Settings changed meanwhile go with the next turn.
        assert!(matches!(
            c.say(Say::Mode {
                request: "r1".into(),
                mode: PermissionMode::BypassPermissions
            })
            .as_slice(),
            [Out::Ack { request_id, error: None }] if request_id == "r1"
        ));
        assert!(c.say(user("two")).is_empty());
        let out = lines(
            &mut c,
            &[json!({"method":"turn/completed","params":{"turn":{"id":"turn_1","status":"completed"}}})],
        );
        assert!(matches!(out[0], Out::Result { .. }));
        let next = &written(&out)[0];
        assert_eq!(next["params"]["input"][0]["text"], "two");
        assert_eq!(next["params"]["approvalPolicy"], "never");
        assert_eq!(next["params"]["sandboxPolicy"], json!({"type": "dangerFullAccess"}));
        // Neither mode is Auto: the field stays out, for a Codex too old to know it.
        assert!(next["params"].get("approvalsReviewer").is_none());
    }

    #[test]
    fn only_auto_and_leaving_it_name_the_reviewer() {
        // One turn in `mode`, run to its end; its `turn/start`.
        fn turn(c: &mut Driver, id: u64, mode: PermissionMode) -> Value {
            c.say(Say::Mode {
                request: "r".into(),
                mode,
            });
            let start = written(&c.say(user("go")))[0].clone();
            let name = format!("turn_{id}");
            lines(
                c,
                &[
                    json!({"id":id,"result":{"turn":{"id":name}}}),
                    json!({"method":"turn/completed","params":{"turn":{"id":name,"status":"completed"}}}),
                ],
            );
            start
        }
        let reviewer = |start: &Value| start["params"].get("approvalsReviewer").cloned();
        let mut c = codex(false);
        running(&mut c);
        assert_eq!(reviewer(&turn(&mut c, 3, PermissionMode::AcceptEdits)), None);
        assert_eq!(
            reviewer(&turn(&mut c, 4, PermissionMode::Auto)),
            Some(json!("auto_review"))
        );
        // Leaving Auto gives the approvals back to the person, once.
        assert_eq!(reviewer(&turn(&mut c, 5, PermissionMode::Default)), Some(json!("user")));
        assert_eq!(reviewer(&turn(&mut c, 6, PermissionMode::BypassPermissions)), None);
    }

    #[test]
    fn a_thread_that_comes_back_with_another_reviewer_gets_this_modes() {
        // `(mode, reviewer Codex reports for the thread)` -> the first turn's `approvalsReviewer`.
        let first_turn = |mode: PermissionMode, fork: bool, reported: Option<&str>| {
            let mut c = Driver::new(&Start {
                mode,
                model: None,
                effort: None,
                fast: false,
                native_id: "thr_old",
                resume: true,
                fork,
                cwd: "/r",
                rewind: None,
                rewind_required: false,
                instructions: None,
                prompt: None,
                name: None,
            });
            c.open();
            let mut result = json!({"thread":{"id":"thr_old"},"model":"gpt-6-luna"});
            if let Some(reported) = reported {
                result["approvalsReviewer"] = json!(reported);
            }
            lines(&mut c, &[json!({"id":2,"result":result})]);
            let start = written(&c.say(user("go")))[0].clone();
            assert_eq!(start["method"], "turn/start");
            start["params"].get("approvalsReviewer").cloned()
        };
        // Last run in Auto, opened now in Ask first (resumed or forked): the person answers again.
        for fork in [false, true] {
            assert_eq!(
                first_turn(PermissionMode::Default, fork, Some("auto_review")),
                Some(json!("user"))
            );
        }
        assert_eq!(
            first_turn(PermissionMode::AcceptEdits, false, Some("guardian_subagent")),
            Some(json!("user"))
        );
        // Opened in Auto but Codex reports the person: Auto again.
        assert_eq!(
            first_turn(PermissionMode::Auto, false, Some("user")),
            Some(json!("auto_review"))
        );
        // Already this mode's, or a Codex too old to report it: nothing is sent.
        assert_eq!(first_turn(PermissionMode::Default, false, Some("user")), None);
        assert_eq!(first_turn(PermissionMode::Auto, false, Some("auto_review")), None);
        assert_eq!(first_turn(PermissionMode::Default, false, None), None);
    }

    #[test]
    fn while_a_turn_starts_only_its_own_end_ends_it() {
        let completed = |id: &str| json!({"method":"turn/completed","params":{"turn":{"id":id,"status":"completed"}}});
        let results = |out: &[Out]| out.iter().filter(|out| matches!(out, Out::Result { .. })).count();
        let mut c = codex(false);
        running(&mut c);
        c.say(user("one"));
        // Not named yet: the end of some other turn (a late or repeated one) is not this turn's.
        assert_eq!(results(&lines(&mut c, &[completed("turn_0")])), 0);
        assert_eq!(
            results(&lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})])),
            0
        );
        assert_eq!(results(&lines(&mut c, &[completed("turn_1")])), 1);

        // Its own end, come before Codex named it, counts once it is named.
        c.say(user("two"));
        assert_eq!(results(&lines(&mut c, &[completed("turn_2")])), 0);
        assert_eq!(
            results(&lines(&mut c, &[json!({"id":4,"result":{"turn":{"id":"turn_2"}}})])),
            1
        );
        // Idle again: the next message starts a turn at once.
        assert_eq!(written(&c.say(user("three")))[0]["method"], "turn/start");
    }

    #[test]
    fn a_late_end_of_an_earlier_turn_does_not_push_out_the_new_turns_own_end() {
        let completed = |id: &str| json!({"method":"turn/completed","params":{"turn":{"id":id,"status":"completed"}}});
        let results = |out: &[Out]| out.iter().filter(|out| matches!(out, Out::Result { .. })).count();
        let mut c = codex(false);
        running(&mut c);
        c.say(user("one"));
        // Its own end comes before Codex named it, then a late or repeated end of an earlier turn.
        assert_eq!(results(&lines(&mut c, &[completed("turn_1")])), 0);
        assert_eq!(results(&lines(&mut c, &[completed("turn_0")])), 0);
        // Named: the held end of this turn counts, so it does not hang waiting for another.
        assert_eq!(
            results(&lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})])),
            1
        );
        assert_eq!(written(&c.say(user("two")))[0]["method"], "turn/start");
    }

    fn steer(text: &str) -> Say {
        Say::Steer {
            text: text.into(),
            images: vec![],
            person: true,
        }
    }

    /// A refused steer reserves the turn it runs as for whoever sent it: Nexus's notice gets a turn that is not the
    /// person's, also when it waited for Codex to name the turn.
    #[test]
    fn a_refused_steer_says_whose_turn_it_reserves() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("one"));
        let notice = |text: &str| Say::Steer {
            text: text.into(),
            images: vec![],
            person: false,
        };
        assert!(c.say(notice("before the name")).is_empty());
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        c.say(steer("the person's"));
        c.say(notice("after the name"));
        let refused = |c: &mut Driver, id: u64| {
            lines(
                c,
                &[json!({"error":{"code":-32600,"message":"no active turn to steer"},"id":id})],
            )
            .into_iter()
            .filter(|out| matches!(out, Out::TurnQueued { .. }))
            .collect::<Vec<_>>()
        };
        assert_eq!(refused(&mut c, 4), [Out::TurnQueued { person: false }]);
        assert_eq!(refused(&mut c, 5), [Out::TurnQueued { person: true }]);
        assert_eq!(refused(&mut c, 6), [Out::TurnQueued { person: false }]);
    }

    #[test]
    fn a_steer_joins_the_turn_under_way() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("one"));
        // Codex has not named the turn yet: the steer waits for its id.
        assert!(c.say(steer("also this")).is_empty());
        let out = lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        assert_eq!(
            written(&out),
            [
                json!({"id": 4, "method": "turn/steer", "params": {"threadId": "thr_1", "expectedTurnId": "turn_1",
                "input": [{"type": "text", "text": "also this", "text_elements": []}]}})
            ]
        );
        // Named: it goes at once.
        assert_eq!(written(&c.say(steer("and this")))[0]["method"], "turn/steer");
        // Codex refused it (the turn had just ended, or can't be steered): it runs as the next turn instead.
        let out = lines(
            &mut c,
            &[
                json!({"method":"turn/completed","params":{"turn":{"id":"turn_1","status":"completed"}}}),
                json!({"error":{"code":-32600,"message":"no active turn to steer"},"id":5}),
            ],
        );
        let next = written(&out);
        assert_eq!(next[0]["method"], "turn/start");
        assert_eq!(next[0]["params"]["input"][0]["text"], "and this");
    }

    #[test]
    fn a_steer_before_the_thread_is_live_joins_the_first_message() {
        let mut c = codex(false);
        c.open();
        c.say(user("one"));
        assert!(c.say(steer("two")).is_empty());
        let out = lines(&mut c, &[json!({"id":2,"result":{"thread":{"id":"thr_1"}}})]);
        let start = written(&out);
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["method"], "turn/start");
        assert_eq!(start[0]["params"]["input"][1]["text"], "two");
    }

    #[test]
    fn a_turn_codex_starts_by_itself_says_so() {
        let mut c = codex(false);
        running(&mut c);
        let out = lines(
            &mut c,
            &[json!({"method":"turn/started","params":{"threadId":"thr_1","turn":{"id":"turn_9"}}})],
        );
        assert!(
            matches!(out.as_slice(), [Out::Cursor(id), Out::Event(WbEvent::Notice { text })]
                if id == "turn_9" && text.contains("on its own")),
            "{out:?}"
        );
        // The person's own turn, named by `turn/started` before the answer: no notice.
        lines(
            &mut c,
            &[json!({"method":"turn/completed","params":{"turn":{"id":"turn_9","status":"completed"}}})],
        );
        c.say(user("work"));
        let out = lines(
            &mut c,
            &[json!({"method":"turn/started","params":{"threadId":"thr_1","turn":{"id":"turn_10"}}})],
        );
        assert_eq!(out, [Out::Cursor("turn_10".into())]);
    }

    #[test]
    fn a_retry_says_its_error_once_per_turn_and_only_inside_one() {
        let mut c = codex(false);
        running(&mut c);
        let retry = |message: &str, will: bool| {
            json!({"method":"error","params":{"threadId":"thr_1","turnId":"turn_1","willRetry":will,
                "error":{"message":message,"codexErrorInfo":"serverOverloaded"}}})
        };
        let capacity = "Selected model is at capacity. Please try a different model.";
        // Outside a turn: nothing, as a notice would open a turn Codex never ends.
        assert_eq!(lines(&mut c, &[retry(capacity, true)]), []);
        c.say(user("work"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let out = lines(
            &mut c,
            &[retry(capacity, true), retry(capacity, true), retry(capacity, false)],
        );
        assert!(
            matches!(out.as_slice(), [Out::Event(WbEvent::Notice { text })] if text.ends_with(capacity)),
            "{out:?}"
        );
        // Another error in the same turn is said too; another turn's retry is not this one's.
        assert_eq!(lines(&mut c, &[retry("stream disconnected", true)]).len(), 1);
        let other = json!({"method":"error","params":{"threadId":"thr_1","turnId":"turn_0","willRetry":true,
            "error":{"message":"late"}}});
        assert_eq!(lines(&mut c, &[other]), []);
    }

    #[test]
    fn a_turn_the_plan_refused_says_when_its_limit_resets() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("work"));
        let limits = |used: f64| {
            json!({"method":"account/rateLimits/updated","params":{"rateLimits":{
                "primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":1_792_605_600},
                "secondary":{"usedPercent":40,"windowDurationMins":10_080,"resetsAt":1_793_000_000}}}})
        };
        assert_eq!(
            lines(&mut c, &[limits(100.0)]),
            [Out::RateLimit {
                session: Some(100.0),
                weekly: Some(40.0),
                session_resets: Some(1_792_605_600_000),
                weekly_resets: Some(1_793_000_000_000),
                refused: false,
                resets_at: None
            }]
        );
        let out = lines(
            &mut c,
            &[
                json!({"id":3,"result":{"turn":{"id":"turn_1"}}}),
                json!({"method":"turn/completed","params":{"turn":{"id":"turn_1","status":"failed",
                    "error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}}),
            ],
        );
        assert!(
            matches!(
                out.as_slice(),
                [
                    Out::Cursor(_),
                    Out::RateLimit {
                        refused: true,
                        resets_at: Some(1_792_605_600_000),
                        ..
                    },
                    Out::Result { error: Some(_), .. }
                ]
            ),
            "{out:?}"
        );
        // Another failure is no limit stop; nor is a limit that is not used up a reset time.
        c.say(user("again"));
        lines(&mut c, &[limits(60.0)]);
        let out = lines(
            &mut c,
            &[
                json!({"id":4,"result":{"turn":{"id":"turn_2"}}}),
                json!({"method":"turn/completed","params":{"turn":{"id":"turn_2","status":"failed",
                    "error":{"message":"stream disconnected","codexErrorInfo":{"responseStreamDisconnected":{}}}}}}),
            ],
        );
        assert!(!out.iter().any(|out| matches!(out, Out::RateLimit { .. })), "{out:?}");
        assert!(usage_limit(Some(&json!({"usageLimitExceeded": {}}))));
        assert!(!usage_limit(None));
    }

    #[test]
    fn an_interrupt_waits_for_codex_to_name_the_turn() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("count slowly"));
        assert!(c.say(Say::Interrupt { request: "i1".into() }).is_empty());
        let out = lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        assert_eq!(
            written(&out),
            [json!({"id": 4, "method": "turn/interrupt", "params": {"threadId": "thr_1", "turnId": "turn_1"}})]
        );
        // `turn/started` after the answer names the same turn: no second interrupt.
        assert!(
            lines(
                &mut c,
                &[json!({"method":"turn/started","params":{"turn":{"id":"turn_1"}}})]
            )
            .is_empty()
        );
        let out = lines(
            &mut c,
            &[
                json!({"id":4,"result":{}}),
                json!({"method":"turn/completed","params":{"turn":{"id":"turn_1","status":"interrupted","durationMs":15}}}),
            ],
        );
        assert!(matches!(
            out.as_slice(),
            [Out::Result {
                interrupted: true,
                error: None,
                ..
            }]
        ));
        // Idle, an interrupt sends nothing, and Codex's "no active turn" answer is no error.
        assert!(c.say(Say::Interrupt { request: "i2".into() }).is_empty());
        // Before the thread is live, it ends the message waiting for it.
        let mut early = codex(false);
        early.open();
        early.say(user("hi"));
        assert!(matches!(
            early.say(Say::Interrupt { request: "i3".into() }).as_slice(),
            [Out::Result { interrupted: true, .. }]
        ));
        assert!(
            written(&lines(
                &mut early,
                &[json!({"id":2,"result":{"thread":{"id":"thr_2"}}})]
            ))
            .is_empty()
        );
        assert!(
            lines(
                &mut c,
                &[json!({"error":{"code":-32600,"message":"no active turn to interrupt"},"id":4})]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_turn_codex_refuses_ends_with_its_error() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("hi"));
        let out = lines(
            &mut c,
            &[json!({"id":3,"error":{"code":-32600,"message":"model not allowed"}})],
        );
        assert!(matches!(out.as_slice(), [Out::Result { error: Some(e), .. }] if e == "model not allowed"));
        // A thread that cannot resume ends the run.
        let mut c = codex(true);
        c.open();
        let out = lines(
            &mut c,
            &[json!({"id":2,"error":{"code":-32600,"message":"no rollout found"}})],
        );
        assert!(
            matches!(out.as_slice(), [Out::Event(WbEvent::Error { text, .. }), Out::Close] if text.contains("resume") && text.contains("no rollout found"))
        );
        // A thread another Codex holds says what to do, with Codex's words as the detail.
        let mut c = codex(true);
        c.open();
        let out = lines(
            &mut c,
            &[json!({"id":2,"error":{"code":-32600,"message":"thread thr_1 already has an active writer"}})],
        );
        assert!(
            matches!(out.as_slice(), [Out::Event(WbEvent::Error { text, detail: Some(detail) }), Out::Close]
                if text.contains("open in another Codex app") && text.contains("fork") && detail.contains("active writer")),
            "{out:?}"
        );
    }

    #[test]
    fn a_denial_declines_and_passes_the_reason_on() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("touch it"));
        lines(
            &mut c,
            &[
                json!({"id":3,"result":{"turn":{"id":"turn_1"}}}),
                json!({"method":"item/fileChange/requestApproval","id":5,"params":{"itemId":"exec-2","reason":"Write notes"}}),
            ],
        );
        let out = c.say(Say::Deny {
            request_id: "exec-2#5".into(),
            message: Some("use docs/ instead".into()),
        });
        let sent = written(&out);
        assert_eq!(sent[0], json!({"id": 5, "result": {"decision": "decline"}}));
        assert_eq!(sent[1]["method"], "turn/steer");
        assert_eq!(sent[1]["params"]["expectedTurnId"], "turn_1");
        assert_eq!(sent[1]["params"]["input"][0]["text"], "use docs/ instead");
        // An approval the turn left behind is withdrawn.
        lines(
            &mut c,
            &[json!({"method":"item/fileChange/requestApproval","id":6,"params":{"itemId":"exec-3"}})],
        );
        let out = lines(
            &mut c,
            &[json!({"method":"serverRequest/resolved","params":{"requestId":6}})],
        );
        assert_eq!(
            out,
            [Out::Cancelled {
                request_id: "exec-3#6".into()
            }]
        );
        // Anything else Codex asks is answered at once.
        let out = written(&lines(
            &mut c,
            &[json!({"method":"item/tool/requestUserInput","id":7,"params":{}})],
        ));
        assert_eq!(out[0]["id"], 7);
        assert!(out[0].get("error").is_some());
    }

    #[test]
    fn a_file_approval_with_no_files_shown_offers_no_session_wide_allow() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("touch it"));
        let out = lines(
            &mut c,
            &[
                json!({"id":3,"result":{"turn":{"id":"turn_1"}}}),
                json!({"method":"item/fileChange/requestApproval","id":5,"params":{"itemId":"unseen"}}),
            ],
        );
        let approval = events(&out).into_iter().find_map(|event| match event {
            WbEvent::Approval {
                suggestions,
                description,
                ..
            } => Some((suggestions.clone(), description.clone())),
            _ => None,
        });
        let (suggestions, description) = approval.unwrap_or_else(|| panic!("no approval: {out:?}"));
        assert!(suggestions.is_empty(), "{suggestions:?}");
        assert!(description.unwrap_or_default().contains("did not say which files"));
    }

    #[test]
    fn a_permanent_command_rule_names_the_program_it_runs() {
        let label = |argv: Value| {
            let params =
                json!({"availableDecisions": [{"acceptWithExecpolicyAmendment": {"execpolicy_amendment": argv}}]});
            suggestions(true, &params)
                .first()
                .map(|s| s.label.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            label(json!(["/bin/zsh", "-lc", "echo hi"])),
            "Always allow echo hi from now on"
        );
        assert_eq!(
            label(json!(["python3", "-c", "import os; os.remove('x')"])),
            "Always allow python3 -c 'import os; os.remove('\\''x'\\'')' from now on"
        );
        assert_eq!(label(json!(["cargo", "test"])), "Always allow cargo test from now on");
    }

    #[test]
    fn a_diff_line_starting_with_a_multi_byte_character_does_not_panic() {
        let change =
            json!({"path": "a.txt", "kind": {"type": "update"}, "diff": "@@ -1 +1 @@\n-old\n+new\né context\n ü kept"});
        let (tool, input) = file_change(&change);
        assert_eq!(tool, "Edit");
        assert_eq!(input["old_string"], "old\né context\nü kept");
        assert_eq!(input["new_string"], "new\né context\nü kept");
    }

    fn question_request(blocking: bool) -> Value {
        json!({"method":"item/tool/requestUserInput","id":7,"params":{
            "threadId":"thr_1","turnId":"turn_1","itemId":"question-1","isBlocking":blocking,
            "questions":[{"id":"usage","header":"Usage","question":"Which turns count?",
                "options":[{"label":"Every turn","description":"Includes person-started turns"}]}]}})
    }

    #[test]
    fn waits_for_question_answers_and_returns_ids_and_arrays() {
        let mut c = codex(false);
        running(&mut c);
        let out = lines(&mut c, &[question_request(true)]);
        assert!(
            matches!(out.as_slice(), [Out::Event(WbEvent::Approval { tool, input, .. })]
            if tool == "AskUserQuestion" && input["questions"][0]["id"] == "usage" && input["isBlocking"] == true)
        );
        let sent = written(&c.say(Say::Allow {
            request_id: "question-1#7".into(),
            input: json!({}),
            answers: Some(serde_json::Map::from_iter([("usage".into(), json!(["Every turn"]))])),
            remember: None,
        }));
        assert_eq!(
            sent,
            [json!({"id":7,"result":{"answers":{"usage":{"answers":["Every turn"]}}}})]
        );
        assert!(c.questions.is_empty());
        assert!(c.approvals.is_empty());
    }

    #[test]
    fn nonblocking_questions_allow_work_and_can_be_skipped_or_withdrawn() {
        let mut c = codex(false);
        running(&mut c);
        let out = lines(&mut c, &[question_request(false)]);
        assert!(matches!(&out[0], Out::Event(WbEvent::Approval { input, .. }) if input["isBlocking"] == false));
        let out = lines(
            &mut c,
            &[json!({"method":"item/agentMessage/delta","params":{
            "threadId":"thr_1","itemId":"independent","delta":"Still checking"}})],
        );
        assert!(matches!(&out[0], Out::Delta { text, .. } if text == "Still checking"));
        let sent = written(&c.say(Say::Deny {
            request_id: "question-1#7".into(),
            message: Some("Skipped".into()),
        }));
        assert_eq!(sent, [json!({"id":7,"result":{"answers":{}}})]);
        lines(&mut c, &[question_request(true)]);
        let out = lines(
            &mut c,
            &[json!({"method":"serverRequest/resolved","params":{"threadId":"thr_1","requestId":7}})],
        );
        assert_eq!(
            out,
            [Out::Cancelled {
                request_id: "question-1#7".into()
            }]
        );
        assert!(c.questions.is_empty());
    }

    #[test]
    fn plan_mode_sets_the_collaboration_mode_and_preserves_model_and_effort() {
        let mut c = codex(false);
        running(&mut c);
        c.say(Say::Mode {
            request: "mode".into(),
            mode: PermissionMode::Plan,
        });
        c.say(Say::Effort {
            request: "effort".into(),
            effort: Some(Effort::Max),
        });
        c.say(Say::Model {
            request: "model".into(),
            model: Some("gpt-next-test".into()),
        });
        let sent = written(&c.say(user("Plan it")));
        let start = &sent[0]["params"];
        assert_eq!(start["collaborationMode"]["mode"], "plan");
        assert_eq!(start["collaborationMode"]["settings"]["model"], c.model);
        assert_eq!(start["collaborationMode"]["settings"]["reasoning_effort"], "max");
        assert_eq!(start["sandboxPolicy"]["type"], "readOnly");
        assert!(start.get("model").is_none());
        assert!(start.get("effort").is_none());
        assert_eq!(c.model, "gpt-next-test");
    }

    #[test]
    fn normal_turn_model_changes_use_top_level_settings() {
        let mut c = codex(false);
        running(&mut c);
        c.say(Say::Model {
            request: "model".into(),
            model: Some("gpt-next-test".into()),
        });
        c.say(Say::Effort {
            request: "effort".into(),
            effort: Some(Effort::Max),
        });
        let sent = written(&c.say(user("Continue")));
        let start = &sent[0]["params"];
        assert_eq!(start["model"], "gpt-next-test");
        assert_eq!(start["effort"], "max");
        assert!(start.get("collaborationMode").is_none());
    }

    /// File changes as `codex-cli` 0.163.0 reported them: a new file, then an edit.
    #[test]
    fn shows_file_changes_as_writes_and_edits() {
        let added = json!({"type":"fileChange","id":"exec-a","changes":[{"path":"/r/notes.txt","kind":{"type":"add"},"diff":"one\ntwo\n"}],"status":"completed"});
        assert_eq!(
            calls(&added),
            [(
                "exec-a".into(),
                "Write".into(),
                json!({"file_path": "/r/notes.txt", "content": "one\ntwo\n"})
            )]
        );
        let edited = json!({"type":"fileChange","id":"exec-b","changes":[
            {"path":"/r/notes.txt","kind":{"type":"update","move_path":null},"diff":"@@ -1,2 +1,2 @@\n one\n-two\n+three\n"},
            {"path":"/r/old.txt","kind":{"type":"delete"},"diff":"gone\n"}],"status":"declined"});
        assert_eq!(
            calls(&edited),
            [
                (
                    "exec-b".into(),
                    "Edit".into(),
                    json!({"file_path": "/r/notes.txt", "old_string": "one\ntwo", "new_string": "one\nthree"})
                ),
                (
                    "exec-b:1".into(),
                    "Edit".into(),
                    json!({"file_path": "/r/old.txt", "old_string": "gone\n", "new_string": ""})
                ),
            ]
        );
        assert_eq!(outcome(&edited), ("Declined.".into(), true));
        let failed = json!({"type":"commandExecution","status":"failed","aggregatedOutput":"boom","exitCode":2});
        assert_eq!(outcome(&failed), ("boom".into(), true));
    }

    /// A Codex on `thread`, in the turn `turn`, then every line of a recorded app-server run.
    fn replay(thread: &str, turn: &str, recorded: &str) -> Vec<Out> {
        let mut c = codex(false);
        c.open();
        lines(&mut c, &[json!({"id": 2, "result": {"thread": {"id": thread}}})]);
        c.say(user("go"));
        lines(&mut c, &[json!({"id": 3, "result": {"turn": {"id": turn}}})]);
        recorded
            .lines()
            .filter(|line| !line.trim().is_empty())
            .flat_map(|line| c.read(line))
            .collect()
    }

    fn events(out: &[Out]) -> Vec<&WbEvent> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn task(task_id: &str, tool: &str, status: TaskStatus, description: &str, ended: Option<(u64, u64)>) -> WbEvent {
        WbEvent::Task {
            task_id: task_id.into(),
            tool_use_id: Some(tool.into()),
            task_type: Some("local_agent".into()),
            agent_type: None,
            status,
            description: description.into(),
            tool_count: ended.map(|(tools, _)| tools),
            total_tokens: None,
            duration_ms: ended.map(|(_, ms)| ms),
            detail: None,
        }
    }

    /// Codex's plan (`turn/plan/updated`, shaped as 0.164.0's schema has it) is a `TodoWrite` call with its steps'
    /// states in Claude Code's words, once per update. A plan outside a turn is dropped.
    #[test]
    fn a_plan_update_is_a_todo_write() {
        let update = |turn: &str, second: &str| {
            json!({"method": "turn/plan/updated", "params": {"threadId": "thr_1", "turnId": turn, "explanation": null, "plan": [
                {"step": "Read the code", "status": "completed"},
                {"step": "Fix the bug", "status": second},
                {"step": "Run the tests", "status": "pending"},
            ]}})
            .to_string()
        };
        let todos = |second: &str| {
            json!({"todos": [
                {"content": "Read the code", "status": "completed"},
                {"content": "Fix the bug", "status": second},
                {"content": "Run the tests", "status": "pending"},
            ]})
        };
        let call = |id: &str, second: &str| WbEvent::Tool {
            id: id.into(),
            name: "TodoWrite".into(),
            input: todos(second),
            parent: None,
        };
        let result = |id: &str| WbEvent::ToolResult {
            images: vec![],
            id: id.into(),
            output: String::new(),
            is_error: false,
            truncated: false,
            parent: None,
        };
        let out = replay(
            "thr_1",
            "turn_1",
            &format!("{}\n{}", update("turn_1", "inProgress"), update("turn_1", "completed")),
        );
        assert_eq!(
            events(&out),
            [
                &call("plan-turn_1-1", "in_progress"),
                &result("plan-turn_1-1"),
                &call("plan-turn_1-2", "completed"),
                &result("plan-turn_1-2"),
            ]
        );

        let mut idle = codex(false);
        idle.open();
        lines(&mut idle, &[json!({"id": 2, "result": {"thread": {"id": "thr_1"}}})]);
        assert!(events(&idle.read(&update("turn_0", "pending"))).is_empty());
    }

    /// A task report that says where the task sits.
    fn placed(mut report: WbEvent, place: TaskDetail) -> WbEvent {
        if let WbEvent::Task { detail, .. } = &mut report {
            *detail = place.some();
        }
        report
    }

    /// The first detail a start of the task carries.
    fn start_of<'a>(events: &[&'a WbEvent], task: &str) -> Option<&'a TaskDetail> {
        events.iter().find_map(|event| match event {
            WbEvent::Task {
                task_id,
                status: TaskStatus::Running,
                detail,
                ..
            } if task_id == task => detail.as_deref(),
            _ => None,
        })
    }

    /// The parent a tool result names.
    fn answered_under<'a>(events: &[&'a WbEvent], call: &str) -> Option<Option<&'a str>> {
        events.iter().find_map(|event| match event {
            WbEvent::ToolResult { id, parent, .. } if id == call => Some(parent.as_deref()),
            _ => None,
        })
    }

    /// Each model call's model and parent, in order.
    fn calls_by(out: &[Out]) -> Vec<(&str, Option<&str>)> {
        out.iter()
            .filter_map(|out| match out {
                Out::Call { model, parent, .. } => Some((model.as_str(), parent.as_deref())),
                _ => None,
            })
            .collect()
    }

    /// A newer model's sub-agent (`subAgentActivity`), as `codex-cli` 0.156.1 reported it (recorded by T3 Code): the
    /// sub-agent is an `Agent` call, its own thread's work sits under that call, and task reports say when it ran.
    #[test]
    fn replays_a_recorded_sub_agent_activity() {
        let (main, child) = (
            "01a0d5ff-49e8-7741-9ca7-b12aa375019a",
            "01a0d5ff-5f36-77f1-afa9-cc3dd93018dc",
        );
        let call = "call_opQacJbIfXWqOZb5gT9hd9Mv";
        let out = replay(
            main,
            "01a0d5ff-4b00-7641-bba1-06faa0ef23f1",
            include_str!("../tests/fixtures/codex-0.156.1-subagent-activity.jsonl"),
        );
        assert_eq!(
            events(&out),
            [
                &WbEvent::Tool {
                    id: call.into(),
                    name: "Agent".into(),
                    input: json!({"description": "say hello", "prompt": ""}),
                    parent: None,
                },
                &placed(
                    task(child, call, TaskStatus::Running, "say hello", None),
                    TaskDetail {
                        depth: Some(1),
                        ..TaskDetail::default()
                    }
                ),
                &WbEvent::Text {
                    id: "msg_004d0750968f87b0016ab5c26956e887d098ae49f514e634b8".into(),
                    text: "Hello.".into(),
                    parent: Some(call.into()),
                },
                &task(child, call, TaskStatus::Completed, "Hello.", Some((0, 3408))),
                &WbEvent::ToolResult {
                    images: vec![],
                    id: call.into(),
                    output: "Hello.".into(),
                    is_error: false,
                    truncated: false,
                    parent: None,
                },
                &WbEvent::Text {
                    id: "msg_034802ac4db0e789016ab5c26b3d8c87d085ee92cd13fc7288".into(),
                    text: "Hello.".into(),
                    parent: None,
                },
            ]
        );
        // The sub-agent's streamed text and model calls are its own, not the main answer's.
        let parents: Vec<_> = out
            .iter()
            .filter_map(|out| match out {
                Out::Delta { parent, .. } | Out::Call { parent, .. } => Some(parent.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            parents,
            [None, Some(call), Some(call), Some(call), None, None, None, None]
        );
        // The sub-agent's turn ending is not the session's.
        assert_eq!(out.iter().filter(|out| matches!(out, Out::Result { .. })).count(), 1);
    }

    /// An older model's sub-agents (`collabAgentToolCall` with `spawnAgent`), as `codex-cli` 0.156.1 reported two of
    /// them (recorded by T3 Code, long text cut).
    #[test]
    fn replays_recorded_collab_spawns() {
        let out = replay(
            "01a0d617-2920-75e0-8957-ee5a48afefc4",
            "01a0d617-29e0-73d3-a4fd-856238566501",
            include_str!("../tests/fixtures/codex-0.156.1-collab-spawn.jsonl"),
        );
        let events = events(&out);
        let spawns = [
            (
                "exec-27515368-be3b-4e14-bf8d-65276ab48c93",
                "01a0d617-4ebf-7b11-aa51-01f6319c42aa",
            ),
            (
                "exec-9f999071-8180-4493-8d6d-bbdf44be5b91",
                "01a0d617-4f5c-7f91-ae63-640360c8f198",
            ),
        ];
        for (call, child) in spawns {
            let row = events.iter().find_map(|event| match event {
                WbEvent::Tool {
                    id,
                    name,
                    input,
                    parent,
                } if id == call => Some((name, input, parent)),
                _ => None,
            });
            let Some((name, input, parent)) = row else {
                panic!("no Agent call {call}");
            };
            assert_eq!((name.as_str(), parent), ("Agent", &None));
            assert!(text(input.get("description")).starts_with("Read "));
            let reports: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    WbEvent::Task {
                        task_id,
                        tool_use_id,
                        status,
                        tool_count,
                        ..
                    } if task_id == child => Some((tool_use_id.as_deref(), *status, *tool_count)),
                    _ => None,
                })
                .collect();
            assert_eq!(reports.len(), 2, "{reports:?}");
            assert_eq!(reports[0], (Some(call), TaskStatus::Running, None));
            assert_eq!((reports[1].0, reports[1].1), (Some(call), TaskStatus::Completed));
            assert!(reports[1].2 >= Some(1));
            // Its commands and its answer sit under its call.
            let under = events.iter().filter(|event| match event {
                WbEvent::Tool { parent, .. } | WbEvent::Text { parent, .. } | WbEvent::Thinking { parent, .. } => {
                    parent.as_deref() == Some(call)
                }
                _ => false,
            });
            assert!(under.count() >= 3);
            // Its result is its final answer, not the note it wrote first.
            assert!(events.iter().any(|event| matches!(event,
                WbEvent::ToolResult { id, output, is_error: false, .. } if id == call && output.starts_with('`'))));
            // The spawn's completion names its model; its commands' results sit under its call too.
            assert_eq!(
                start_of(&events, child),
                Some(&TaskDetail {
                    depth: Some(1),
                    model: Some("gpt-5.6-luna".into()),
                    ..TaskDetail::default()
                })
            );
            assert!(events.iter().any(|event| matches!(event,
                WbEvent::ToolResult { parent: Some(parent), .. } if parent == call)));
        }
        // Only the main agent's own words are main text.
        let main_text = events
            .iter()
            .filter(|event| matches!(event, WbEvent::Text { parent: None, .. }))
            .count();
        assert_eq!(main_text, 2);
    }

    /// Work a sub-agent does after the session's turn ended is kept: its text, its end and its Agent call's result. Each
    /// names the sub-agent, so the core opens no turn for it (lib.rs).
    #[test]
    fn a_sub_agent_after_the_turn_keeps_its_work() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        lines(
            &mut c,
            &[json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}})],
        );
        lines(
            &mut c,
            &[
                json!({"method":"turn/completed","params":{"threadId":"thr_1","turn":{"id":"turn_1","status":"completed"}}}),
            ],
        );
        let out = lines(
            &mut c,
            &[
                json!({"method":"item/completed","params":{"threadId":"thr_c","item":{"type":"agentMessage","id":"m1","text":"found it"}}}),
                json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c","status":"failed","durationMs":5}}}),
            ],
        );
        assert_eq!(
            events(&out),
            [
                &WbEvent::Text {
                    id: "m1".into(),
                    text: "found it".into(),
                    parent: Some("call_1".into()),
                },
                &task("thr_c", "call_1", TaskStatus::Failed, "found it", Some((0, 5))),
                &WbEvent::ToolResult {
                    images: vec![],
                    id: "call_1".into(),
                    output: "found it".into(),
                    is_error: true,
                    truncated: false,
                    parent: None,
                },
            ]
        );
    }

    /// A sub-agent's approval that Codex resolves (the sub-agent was interrupted) is withdrawn like the session's own.
    #[test]
    fn a_sub_agent_approval_resolved_is_withdrawn() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                json!({"method":"item/commandExecution/requestApproval","id":8,"params":{"threadId":"thr_c","itemId":"exec-9"}}),
            ],
        );
        let out = lines(
            &mut c,
            &[json!({"method":"serverRequest/resolved","params":{"threadId":"thr_c","requestId":8}})],
        );
        assert_eq!(
            out,
            [Out::Cancelled {
                request_id: "exec-9#8".into()
            }]
        );
    }

    /// A thread we never saw spawned is not this session's. The turn is running, so only the thread guard keeps its
    /// text out (an idle session would drop it anyway).
    #[test]
    fn an_unknown_thread_is_ignored_mid_turn() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let out = lines(
            &mut c,
            &[
                json!({"method":"item/completed","params":{"threadId":"thr_other","item":{"type":"agentMessage","id":"m2","text":"stray"}}}),
            ],
        );
        assert!(events(&out).is_empty());
    }

    /// A sub-agent that spawns its own, whose command needs the person's approval, recorded from `codex-cli` 0.164.0
    /// (`multi_agent`, `subAgentActivity` items). The grandchild's spawn arrives on the child's thread, its approval
    /// names the grandchild's thread, and Codex sends no `thread/started` for either.
    #[test]
    fn replays_recorded_nested_spawns() {
        let out = replay(
            "01a17400-6e1d-7160-a0c3-d8478fa78e59",
            "01a17400-94ad-71d1-9aab-77e91858523d",
            include_str!("../tests/fixtures/codex-0.164.0-nested-spawn.jsonl"),
        );
        let events = events(&out);
        let (outer, inner) = (
            (
                "01a17400-a2e6-7672-b2e8-360e1c821fa5",
                "call_69eb8a6380244fb89aa7327f35b9a267",
            ),
            (
                "01a17400-b039-7012-95fa-55c1f58c905a",
                "call_ee18fad6d1f04814b39c7a99c5860f16",
            ),
        );
        let command = "exec-e49cbf6e-ae50-432f-a3a5-53290af9f1ad";
        let parent = |call: &str| {
            events.iter().find_map(|event| match event {
                WbEvent::Tool { id, parent, .. } if id == call => Some(parent.as_deref()),
                _ => None,
            })
        };
        assert_eq!(parent(outer.1), Some(None));
        assert_eq!(parent(inner.1), Some(Some(outer.1)));
        assert_eq!(parent(command), Some(Some(inner.1)));
        assert!(events.iter().any(|event| matches!(event,
            WbEvent::Approval { tool_use_id: Some(id), .. } if id == command)));
        for (thread, call) in [outer, inner] {
            let statuses: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    WbEvent::Task {
                        task_id,
                        tool_use_id,
                        status,
                        ..
                    } if task_id == thread => {
                        assert_eq!(tool_use_id.as_deref(), Some(call));
                        Some(*status)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(statuses, [TaskStatus::Running, TaskStatus::Completed], "{thread}");
        }
        // Each sub-agent's model calls are its own, priced with its model, not the session's.
        for call in [outer.1, inner.1] {
            assert!(
                calls_by(&out).contains(&("gpt-6-astra", Some(call))),
                "{:?}",
                calls_by(&out)
            );
        }
        assert!(calls_by(&out).contains(&("", None)));
        assert_eq!(out.iter().filter(|out| matches!(out, Out::Result { .. })).count(), 1);
        // The grandchild sits under the child that spawned it.
        assert_eq!(
            start_of(&events, outer.0),
            Some(&TaskDetail {
                depth: Some(1),
                model: Some("gpt-6-astra".into()),
                ..TaskDetail::default()
            })
        );
        assert_eq!(
            start_of(&events, inner.0),
            Some(&TaskDetail {
                parent_task_id: Some(outer.0.into()),
                depth: Some(2),
                model: Some("gpt-6-astra".into()),
                ..TaskDetail::default()
            })
        );
        // Each Agent call's result names the sub-agent that made the call; the command's, the grandchild.
        assert_eq!(answered_under(&events, outer.1), Some(None));
        assert_eq!(answered_under(&events, inner.1), Some(Some(outer.1)));
        assert_eq!(answered_under(&events, command), Some(Some(inner.1)));
        assert!(events.iter().any(|event| matches!(event,
            WbEvent::Approval { tool_use_id: Some(id), parent: Some(parent), .. } if id == command && parent == inner.1)));
    }

    /// A child's long command writes to its terminal (`item/commandExecution/terminalInteraction`, Codex 0.164.0
    /// `late-child` l.35): nothing of it reaches the session.
    #[test]
    fn a_terminal_interaction_adds_nothing() {
        let recorded = include_str!("../tests/fixtures/codex-0.164.0-late-child.jsonl");
        let mut c = codex(false);
        c.open();
        lines(
            &mut c,
            &[json!({"id": 2, "result": {"thread": {"id": "01a17400-6e1d-7160-a0c3-d8478fa78e59"}}})],
        );
        c.say(user("go"));
        lines(
            &mut c,
            &[json!({"id": 3, "result": {"turn": {"id": "01a17401-4436-7c90-9c3e-b3e64e78e40a"}}})],
        );
        let mut seen = false;
        for line in recorded.lines().filter(|line| !line.trim().is_empty()) {
            let out = c.read(line);
            if line.contains("\"item/commandExecution/terminalInteraction\"") {
                seen = true;
                assert_eq!(out, [], "{line}");
            }
        }
        assert!(seen, "the recording has no terminal interaction");
    }

    /// A sub-agent still at work when the session's turn ends, then stopped with `turn/interrupt` on its own thread
    /// and turn (`codex-cli` 0.164.0). Its command's approval arrives after the session's turn ended. Its turn ends
    /// interrupted, with no `subAgentActivity` completion. Before that, Codex refused `turn/steer` and `turn/start` on
    /// its thread (ids 4 and 6 in the recording; a replay never asked them, so it ignores them).
    #[test]
    fn replays_a_recorded_sub_agent_after_the_turn() {
        let recorded = include_str!("../tests/fixtures/codex-0.164.0-late-child.jsonl");
        let out = replay(
            "01a17400-6e1d-7160-a0c3-d8478fa78e59",
            "01a17401-4436-7c90-9c3e-b3e64e78e40a",
            recorded,
        );
        let session_end = out.iter().position(|out| matches!(out, Out::Result { .. }));
        let asked = out
            .iter()
            .position(|out| matches!(out, Out::Event(WbEvent::Approval { .. })));
        let child = "01a17401-4f62-7562-943c-5dcec2d5cd19";
        let stopped = out.iter().position(|out| {
            matches!(out,
            Out::Event(WbEvent::Task { task_id, status: TaskStatus::Stopped, .. }) if task_id == child)
        });
        assert!(
            matches!((session_end, asked, stopped), (Some(end), Some(ask), Some(stop)) if end < ask && ask < stop),
            "{out:?}"
        );
        let refused = "direct app-server input is not allowed for multi-agent v2 sub-agents";
        assert_eq!(recorded.matches(refused).count(), 2);
        // Its approval and its calls name its Agent call; its calls are priced with its model.
        let call = "call_f753533e2f0f42bcb6c3e7e5d2f7af13";
        assert!(out.iter().any(|out| matches!(out,
            Out::Event(WbEvent::Approval { parent: Some(parent), .. }) if parent == call)));
        let calls = calls_by(&out);
        assert!(
            calls
                .iter()
                .filter(|(_, parent)| *parent == Some(call))
                .all(|(model, _)| *model == "gpt-6-astra")
        );
        assert_eq!(calls.iter().filter(|(_, parent)| *parent == Some(call)).count(), 3);
    }

    /// A sub-agent's spawn that fails sits under that sub-agent's call, as its spawn would have.
    #[test]
    fn a_sub_agents_failed_spawn_sits_under_it() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        let failed = json!({"type":"collabAgentToolCall","id":"call_2","tool":"spawnAgent","status":"failed","prompt":"Read x",
            "receiverThreadIds":[]});
        let out = lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                json!({"method":"item/completed","params":{"threadId":"thr_c","item":failed}}),
            ],
        );
        assert!(events(&out).iter().any(|event| matches!(event,
            WbEvent::ToolResult { id, is_error: true, parent: Some(parent), .. } if id == "call_2" && parent == "call_1")));
    }

    /// A sub-agent that runs again (Codex starts a new turn on its thread after it ended) says its new run: the turn.
    #[test]
    fn a_sub_agent_run_again_names_its_run() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        let out = lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c1"}}}),
                json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c1","status":"completed","durationMs":5}}}),
                json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c2"}}}),
            ],
        );
        let runs: Vec<_> = events(&out)
            .into_iter()
            .filter_map(|event| match event {
                WbEvent::Task { status, detail, .. } => {
                    Some((*status, detail.as_ref().and_then(|d| d.run_id.as_deref())))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            runs,
            [
                (TaskStatus::Running, None),
                (TaskStatus::Completed, None),
                (TaskStatus::Running, Some("t_c2"))
            ]
        );
    }

    /// A child's decline goes without its reason: the reason is not for the session's agent, and a child refuses
    /// messages of its own. The session's own decline still tells its turn why (`a_denial_declines_and_passes_the_reason_on`).
    #[test]
    fn a_childs_decline_never_steers_the_lead() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                json!({"method":"item/commandExecution/requestApproval","id":8,"params":{"threadId":"thr_c","itemId":"exec-9","command":"rm x"}}),
            ],
        );
        let sent = written(&c.say(Say::Deny {
            request_id: "exec-9#8".into(),
            message: Some("not that file".into()),
        }));
        assert_eq!(sent, [json!({"id": 8, "result": {"decision": "decline"}})]);
    }

    /// A child compacting its own conversation is not the session's: no notice in the session's timeline.
    #[test]
    fn a_childs_compaction_is_not_the_sessions() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        let compacted = |thread: &str| json!({"method":"item/completed","params":{"threadId":thread,"item":{"type":"contextCompaction","id":"cc1"}}});
        let child = lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                compacted("thr_c"),
            ],
        );
        assert!(!events(&child).iter().any(|e| matches!(e, WbEvent::Notice { .. })));
        let own = lines(&mut c, &[compacted("thr_1")]);
        assert!(events(&own).iter().any(|e| matches!(e, WbEvent::Notice { .. })));
    }

    /// Every event the driver emits while the session's own turn is not running belongs to a sub-agent: a report, an
    /// event naming its sub-agent, or an Agent call's own result. So the core opens no session turn for any of it
    /// Checked on every recording and on a child that works on after the turn.
    #[test]
    fn every_event_outside_the_sessions_turn_is_owned() {
        let recordings = [
            (
                "01a0d5ff-49e8-7741-9ca7-b12aa375019a",
                "01a0d5ff-4b00-7641-bba1-06faa0ef23f1",
                include_str!("../tests/fixtures/codex-0.156.1-subagent-activity.jsonl"),
            ),
            (
                "01a0d617-2920-75e0-8957-ee5a48afefc4",
                "01a0d617-29e0-73d3-a4fd-856238566501",
                include_str!("../tests/fixtures/codex-0.156.1-collab-spawn.jsonl"),
            ),
            (
                "01a17400-6e1d-7160-a0c3-d8478fa78e59",
                "01a17400-94ad-71d1-9aab-77e91858523d",
                include_str!("../tests/fixtures/codex-0.164.0-nested-spawn.jsonl"),
            ),
            (
                "01a17400-6e1d-7160-a0c3-d8478fa78e59",
                "01a17401-4436-7c90-9c3e-b3e64e78e40a",
                include_str!("../tests/fixtures/codex-0.164.0-late-child.jsonl"),
            ),
        ];
        let synthetic = [
            json!({"method":"item/started","params":{"threadId":"thr_1","item":{"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"}}}),
            json!({"method":"turn/completed","params":{"threadId":"thr_1","turn":{"id":"turn_1","status":"completed"}}}),
            json!({"method":"item/completed","params":{"threadId":"thr_c","item":{"type":"agentMessage","id":"m1","text":"late"}}}),
            json!({"method":"item/started","params":{"threadId":"thr_c","item":{"type":"commandExecution","id":"exec-1","command":"ls","status":"inProgress"}}}),
            json!({"method":"item/commandExecution/requestApproval","id":4,"params":{"threadId":"thr_c","itemId":"exec-1"}}),
            json!({"method":"item/completed","params":{"threadId":"thr_c","item":{"type":"commandExecution","id":"exec-1","command":"ls","status":"completed","aggregatedOutput":"a","exitCode":0}}}),
            json!({"method":"item/completed","params":{"threadId":"thr_c","item":{"type":"contextCompaction","id":"cc1"}}}),
            json!({"method":"turn/plan/updated","params":{"threadId":"thr_c","turnId":"t_c","explanation":null,"plan":[{"step":"Check","status":"inProgress"}]}}),
            json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c","status":"completed","durationMs":5}}}),
        ]
        .map(|line| line.to_string())
        .join("\n");
        let mut checked = 0;
        for (main, turn, recorded) in recordings.into_iter().chain([("thr_1", "turn_1", synthetic.as_str())]) {
            let mut c = codex(false);
            c.open();
            lines(&mut c, &[json!({"id": 2, "result": {"thread": {"id": main}}})]);
            c.say(user("go"));
            lines(&mut c, &[json!({"id": 3, "result": {"turn": {"id": turn}}})]);
            let mut agents = std::collections::HashSet::new();
            for line in recorded.lines().filter(|line| !line.trim().is_empty()) {
                let outside = matches!(c.turn, Turn::Idle);
                for out in c.read(line) {
                    let Out::Event(event) = out else { continue };
                    if let WbEvent::Tool { id, name, .. } = &event
                        && name == "Agent"
                    {
                        agents.insert(id.clone());
                    }
                    if !outside {
                        continue;
                    }
                    checked += 1;
                    let agent_result = matches!(&event, WbEvent::ToolResult { id, .. } if agents.contains(id));
                    assert!(
                        event.is_report() || event.parent().is_some() || agent_result,
                        "not owned after the turn: {event:?}"
                    );
                }
            }
        }
        // The late child's approval, text, command and end, and the synthetic child's.
        assert!(checked >= 6, "{checked}");
    }

    /// A child is background work from its spawn until its turn ends and its commands finish: an interrupted child's
    /// command runs on (Codex 0.164.0) and can still write files, so the agent is not idle and a revert waits.
    #[test]
    fn a_child_counts_as_background_until_its_commands_finish() {
        let background = |out: &[Out]| -> Vec<String> {
            out.iter()
                .filter_map(|out| match out {
                    Out::Background(Background::Started(id)) => Some(format!("+{id}")),
                    Out::Background(Background::Ended(id)) => Some(format!("-{id}")),
                    _ => None,
                })
                .collect()
        };
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        let spawned = lines(
            &mut c,
            &[json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}})],
        );
        assert_eq!(background(&spawned), ["+thr_c"]);
        let command = |status: &str| json!({"type":"commandExecution","id":"exec-1","command":"make","status":status});
        let interrupted = lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_c","item":command("inProgress")}}),
                json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c","status":"interrupted"}}}),
            ],
        );
        assert!(background(&interrupted).is_empty(), "{:?}", background(&interrupted));
        let drained = lines(
            &mut c,
            &[json!({"method":"item/completed","params":{"threadId":"thr_c","item":command("completed")}})],
        );
        assert_eq!(background(&drained), ["-thr_c"]);
        // Run again, it counts again.
        let again = lines(
            &mut c,
            &[json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c2"}}})],
        );
        assert_eq!(background(&again), ["+thr_c"]);
    }

    /// A command still waiting for its approval never ran: a child that ends with one asked, or has one withdrawn after
    /// its end, is no longer background work.
    #[test]
    fn an_unanswered_command_does_not_hold_an_ended_child() {
        let background = |out: &[Out]| -> Vec<String> {
            out.iter()
                .filter_map(|out| match out {
                    Out::Background(Background::Ended(id)) => Some(id.clone()),
                    _ => None,
                })
                .collect()
        };
        let setup = || {
            let mut c = codex(false);
            running(&mut c);
            c.say(user("go"));
            lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
            let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
            lines(
                &mut c,
                &[
                    json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                    json!({"method":"item/started","params":{"threadId":"thr_c","item":{"type":"commandExecution","id":"exec-1","command":"make","status":"inProgress"}}}),
                    json!({"method":"item/commandExecution/requestApproval","id":7,"params":{"threadId":"thr_c","itemId":"exec-1"}}),
                ],
            );
            c
        };
        let ended =
            json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c","status":"completed"}}});
        let mut c = setup();
        assert_eq!(background(&lines(&mut c, std::slice::from_ref(&ended))), ["thr_c"]);
        // Withdrawn after the child ended (the core expired it): the same.
        let mut c = setup();
        lines(
            &mut c,
            &[json!({"method":"serverRequest/resolved","params":{"threadId":"thr_c","requestId":7}})],
        );
        assert_eq!(background(&lines(&mut c, &[ended])), ["thr_c"]);
    }

    #[test]
    fn a_childs_first_named_turn_reports_its_stop_target() {
        let mut c = codex(false);
        running(&mut c);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        lines(
            &mut c,
            &[json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}})],
        );
        let named = json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c1"}}});
        let out = lines(&mut c, std::slice::from_ref(&named));
        assert_eq!(out, [Out::TaskReady("thr_c".into())]);
        assert!(lines(&mut c, &[named]).is_empty());
        let stop = written(&c.say(Say::TaskStop {
            request: "stop".into(),
            task_id: "thr_c".into(),
        }));
        assert_eq!(stop[0]["params"], json!({"threadId":"thr_c","turnId":"t_c1"}));
    }

    /// A stop interrupts the child's own turn; Codex's answer, or its refusal, comes back as `Out::Answered`. A child
    /// between its turns, or a task Codex never spawned, is refused without asking Codex.
    #[test]
    fn stops_a_child_by_its_turn() {
        let mut c = codex(false);
        running(&mut c);
        c.say(user("go"));
        lines(&mut c, &[json!({"id":3,"result":{"turn":{"id":"turn_1"}}})]);
        let spawn = json!({"type":"subAgentActivity","id":"call_1","kind":"started","agentThreadId":"thr_c","agentPath":"/root/scan"});
        lines(
            &mut c,
            &[
                json!({"method":"item/started","params":{"threadId":"thr_1","item":spawn}}),
                json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c1"}}}),
            ],
        );
        let stop = |c: &mut Driver, request: &str, task: &str| {
            c.say(Say::TaskStop {
                request: request.into(),
                task_id: task.into(),
            })
        };
        let asked = written(&stop(&mut c, "nexus-1", "thr_c"));
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0]["method"], "turn/interrupt");
        assert_eq!(asked[0]["params"], json!({"threadId": "thr_c", "turnId": "t_c1"}));
        let answered = lines(&mut c, &[json!({"id": asked[0]["id"], "result": {}})]);
        assert_eq!(
            answered,
            [Out::Answered {
                request_id: "nexus-1".into(),
                result: Ok(json!({}))
            }]
        );
        let asked = written(&stop(&mut c, "nexus-2", "thr_c"));
        let refused = lines(
            &mut c,
            &[json!({"id": asked[0]["id"], "error": {"code": -32600, "message": "no active turn to interrupt"}})],
        );
        assert_eq!(
            refused,
            [Out::Answered {
                request_id: "nexus-2".into(),
                result: Err("no active turn to interrupt".into())
            }]
        );
        lines(
            &mut c,
            &[
                json!({"method":"turn/completed","params":{"threadId":"thr_c","turn":{"id":"t_c1","status":"interrupted"}}}),
            ],
        );
        for (request, task) in [("nexus-3", "thr_c"), ("nexus-4", "thr_unknown")] {
            let out = stop(&mut c, request, task);
            assert!(written(&out).is_empty(), "{out:?}");
            assert!(
                matches!(out.as_slice(), [Out::Answered { request_id, result: Err(_) }] if request_id == request),
                "{out:?}"
            );
        }
        // Run again, it can be stopped again, by its new turn.
        lines(
            &mut c,
            &[json!({"method":"turn/started","params":{"threadId":"thr_c","turn":{"id":"t_c2"}}})],
        );
        let asked = written(&stop(&mut c, "nexus-5", "thr_c"));
        assert_eq!(asked[0]["params"]["turnId"], "t_c2");
    }

    /// Codex 0.164.0's `userAgent`, as it answered `initialize` live: the version follows the client's name.
    #[test]
    fn reads_its_version_from_the_user_agent() {
        let version = |agent: &str| {
            let mut c = codex(false);
            c.open();
            let out = lines(
                &mut c,
                &[
                    json!({"id":1,"result":{"userAgent": agent,"codexHome":"/h/.codex"}}),
                    json!({"id":2,"result":{"thread":{"id":"thr_1"}}}),
                ],
            );
            match out.first() {
                Some(Out::Init { version, .. }) => version.clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            version("nexus-desktop/0.164.0 (Mac OS 26.6.0; arm64) unknown (nexus-desktop; 0.1.0)").as_deref(),
            Some("0.164.0")
        );
        assert_eq!(version("codex"), None);
    }

    #[test]
    fn auto_model_is_never_sent_in_plan_mode_before_codex_reports_the_routed_model() {
        let mut c = Driver::new(&Start {
            mode: PermissionMode::Plan,
            model: Some("auto"),
            effort: None,
            fast: false,
            native_id: "t",
            resume: false,
            fork: false,
            cwd: "/p",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        });
        c.open();
        let _ = lines(
            &mut c,
            &[
                json!({"id":1,"result":{"userAgent":"codex/0.164.0","codexHome":"/h/.codex"}}),
                json!({"id":2,"result":{"thread":{"id":"thr_1"}}}),
            ],
        );
        let out = c.say(Say::User {
            text: "hi".into(),
            images: Vec::new(),
        });
        let sent = written(&out);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["method"], "turn/start");
        assert!(sent[0]["params"].get("collaborationMode").is_none(), "{sent:?}");
        assert!(sent[0]["params"].get("model").is_none(), "{sent:?}");

        // Once the host resolves Auto, Plan mode preserves collaborationMode with that routed model.
        let mut routed = Driver::new(&Start {
            mode: PermissionMode::Plan,
            model: Some("auto"),
            effort: None,
            fast: false,
            native_id: "t2",
            resume: false,
            fork: false,
            cwd: "/p",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        });
        routed.launch_model("gpt-6.1-sol");
        routed.open();
        let _ = lines(
            &mut routed,
            &[
                json!({"id":1,"result":{"userAgent":"codex/0.164.0","codexHome":"/h/.codex"}}),
                json!({"id":2,"result":{"thread":{"id":"thr_2"}}}),
            ],
        );
        let out = routed.say(Say::User {
            text: "plan this".into(),
            images: Vec::new(),
        });
        let sent = written(&out);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["method"], "turn/start");
        assert_eq!(sent[0]["params"]["collaborationMode"]["mode"], "plan");
        assert_eq!(
            sent[0]["params"]["collaborationMode"]["settings"]["model"],
            "gpt-6.1-sol"
        );
    }
}
