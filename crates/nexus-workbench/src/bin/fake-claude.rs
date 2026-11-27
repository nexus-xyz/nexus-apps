//! A stand-in for `claude -p --input-format stream-json --output-format stream-json …`, for the workbench's tests:
//! the same lines in and out, with no model. Each message is answered with streamed text, one call's usage and a
//! result; words in the message add to that:
//!
//! - `tool`: asks to run `echo hi` with Bash (a `can_use_tool` request) and reports what the answer was. Changes
//!   sent while it waits are answered `FAKE_CLAUDE_ACK_DELAY_MS` later (a mode refused, with
//!   `FAKE_CLAUDE_REJECT_MODES`).
//! - `queue-probe`: completes tools until interrupted and reads follow-ups at the next tool boundary.
//! - `slow`: works until interrupted (or 30 seconds, `FAKE_CLAUDE_SLOW_MS` when set), or until a message arrives,
//!   which it answers within the turn; with `dies`, exits with code 3 half a second in; with `nostep`, the turn has no
//!   step left to take that message in, so it ends first and the message is answered as a turn of its own; with
//!   `nostep hold`, the turn works on until it is stopped, and the message is answered after the stop, as Claude Code
//!   answers a message it got during a tool call that the stop cut short.
//! - `crash`: exits with code 3 at once.
//! - `orphan`: starts a process that outlives it (`sleep 30`, its pid in `orphan.pid` in its folder), then exits with
//!   code 3; with `holdout`, that process keeps the agent's output open, as a shell it started can; with `detach`
//!   too, it also leaves the agent's process group (as a process that called `setsid` does), out of the stop's reach.
//! - `keepchild`: starts a process that lives as long as it does, as an MCP server or a background shell would
//!   (`sleep 600`, in its process group), and writes its own pid and that process's to `keepchild.pids` in its folder.
//! - `env`: says where its model calls go (`ANTHROPIC_BASE_URL`).
//! - `tokens`: says whether `CLAUDE_CODE_OAUTH_TOKEN` and `GH_TOKEN` are set in its environment, never their values.
//! - `bgtask`: reports background work as Claude Code 2.1.287 does: a shell that ends by itself and a sub-agent that is
//!   stopped (`background_tasks_changed`, `task_started`, `task_updated` and `task_notification` system messages).
//! - `subagents`: reports a background sub-agent as Claude Code 2.1.292 does: its start, a frame of its own (with its
//!   `agent_id` and model), three `task_progress` reports, and a foreground command's start and end. After the
//!   result, the sub-agent ends twice (`task_updated`, then `task_notification` with its usage), one late progress
//!   report follows, and last an end of a task `last1`.
//! - `lateagent`: as Claude Code 2.1.292 does with sub-agents that outlive the turn. In the turn, a background sub-agent
//!   `agent2` asks for two approvals (`sub-ask-1`, `sub-ask-2`) and a foreground one, `agent3`, for one (`sub-ask-3`).
//!   After the result, `agent2` writes text and runs a tool. The answer to `sub-ask-1` runs its command; then `agent2`
//!   ends, the main agent answers it in a turn of its own, and a result with no turn open follows, then an end of a
//!   task `last2`. With `thenexit`, it exits with code 3 right after `agent2`'s late frames.
//! - `rerunask`: a background sub-agent `agent5` asks for an approval (`sub-ask-5`), then starts its second run before
//!   the end of its first is written: the end of run `run-5a` comes after the start of `run-5b`.
//! - `team`: as Claude Code 2.1.292 does with a team of sub-agents. A background sub-agent `boss` has a foreground
//!   sub-agent `worker`, a background one `helper` and a background shell `bshell`, all running, and a workflow `flow`
//!   (a task kind Nexus does not control). `stop_task` on one answers success and stops it, as `stop_task` does: `boss`
//!   with its foreground sub-agent and its shell, never its background sub-agent (`stop-spares-background`). A
//!   `stop_task` on a task that is not running is answered with an error of the fake's own, so a test can tell the core
//!   never asked; `FAKE_CLAUDE_REFUSE_STOPS` refuses every one, and `FAKE_CLAUDE_IGNORE_STOPS` answers none. With
//!   `slow`, an interrupt ends only the turn when `initialize` declared `perTaskStopAffordance`, and the team runs on;
//!   without it, the interrupt stops the team too, as a bare `initialize` made it stop its turn's background sub-agent
//!   (`claude-2.1.292-interrupt`).
//! - `fgwait`: runs a long foreground command (`toolu_fg`, task `b_fg`) and waits up to ten seconds for it, as Claude
//!   Code 2.1.292 does (`move-to-background`). A `background_tasks` on its call moves it: the task is marked
//!   backgrounded, the answer is `{backgrounded: true}`, and the call ends with a receipt; it then runs on as a shell.
//! - `get_task_output` reads a running shell (`team`'s `bshell`, a moved `b_fg`): its ticks, or 20000 bytes with
//!   `FAKE_CLAUDE_BIG_OUTPUT`. For any other task it answers Claude Code's refusal of a sub-agent.
//! - `latecompact`: after the result, a `compact_boundary` with no parent, as a background sub-agent's compaction
//!   would arrive.
//! - `latereport`: after the result, reports a background task that ended (a `task_notification`), as a sub-agent
//!   finishing after its turn does.
//! - `nearlimit`: reports the plan's five-hour limit 95% used (a `rate_limit_event`), as a call near the limit does.
//! - `hitlimit`: the plan refuses the call at its five-hour limit, as Claude Code 2.1 reports it: a rejected
//!   `rate_limit_event` that resets at `FAKE_CLAUDE_RESETS_AT` (epoch seconds), and a failed result.
//! - `write`: writes the message to `msg-<n>.txt` in its folder as soon as it reads it.
//! - `autoturn`: starts a streamed continuation when `automatic-<session>` appears in the fake's memory folder.
//!   It finishes after receiving an interrupt or `finish-automatic-<session>`.
//! - `pause`: takes 300 ms longer to end the turn.
//! - `background`: leaves a background shell running past the turn (`task_started`), which ends
//!   `FAKE_CLAUDE_BACKGROUND_MS` later (default 1000; `task_notification`).
//! - `recall`: repeats the whole message it was given, after `Heard:`.
//! - `wakeup`: sets a wake-up ten minutes on (`FAKE_CLAUDE_WAKE_SECONDS` when set; `ScheduleWakeup`, reason "watching
//!   CI run") and reports it scheduled; with `wakeup-refused`, the call fails, as a denied tool does. It never wakes
//!   by itself: `autoturn` begins the turn a wake-up would.
//! - `manycalls`: reports 500 more model calls' usage before the answer's own, as a long turn of many steps does.
//! - `relay`: the turn runs until told to end, as a lead waiting on the app's tools does. Each time a file `relay`
//!   appears in `$FAKE_CLAUDE_DIR`, it is taken, and its text comes back as a `wait_sessions` call's result (an error
//!   result when it starts with `ERROR `); `END` ends the turn. An interrupt ends it too.
//! - `bgagent`: leaves a background sub-agent running past the turn. Once a file `bgagent-end` appears in
//!   `$FAKE_CLAUDE_DIR` the sub-agent ends (a `stop_task` on it ends it as stopped); once `bgagent-turn` does, the agent
//!   answers it in a turn of its own, as Claude Code 2.1.292 does (`nested-agents`). With `FAKE_CLAUDE_OWN_TURN_MS`,
//!   that turn works that long before it ends, and an interrupt ends it then as interrupted.
//! - `deaf`: with `slow`, the turn ignores an interrupt, as an agent that does not answer one in time.
//!   With `hold-after-eof`, it also keeps working after stdin closes.
//! - `orchestrate`: as a lead given the app's `nexus_app` server (its `--mcp-config` and `NEXUS_APP_MCP_TOKEN`), calls
//!   it over HTTP within the turn: `initialize`, `tools/list`, `start_session` (a `claude` child; `orchestrate codex 2`
//!   starts two `codex` children), then `wait_sessions` on them. Each call is reported as Claude Code reports an MCP
//!   tool's call and result (`mcp__nexus_app__<tool>`). Nothing when it was not given the server.
//! - `handoff` or `hand off`: as `orchestrate`, but calls `handoff_session` to the other agent the message names
//!   (`codex` by default).
//!
//! `FAKE_LAUNCH_RECORD`: a file it writes its arguments and its environment to as it starts, as JSON.
//!
//! `CLAUDE_CONFIG_DIR`: it also keeps each conversation as Claude Code does, a line per message in
//! `$CLAUDE_CONFIG_DIR/projects/<working folder as a name>/<session id>.jsonl`, and `--resume` needs that file.
//!
//! `FAKE_CLAUDE_FAIL_START`: exits with code 1 before it reads anything, as a Claude Code that can't start does.
//!
//! `/rename <name>` renames the session with no model turn, as Claude Code 2.1.312 does: the line comes back, then a
//! `session_title_changed`, a `<synthetic>` assistant message ("Session renamed to: <name>") and a result with
//! `num_turns` 0. Each such line is appended to `$FAKE_CLAUDE_DIR/renames`.
//!
//! `--replay-user-messages`: each message comes back, with `isReplay`, when it is read: at the turn's start, or mid-turn
//! at the step that takes it in.
//!
//! `--resume <id> --fork-session` starts a new conversation, `<id>-forked`, that goes on from where `<id>` was.
//!
//! Each answer's message carries the `uuid` `<session>-a<n>`; `--resume-session-at` one of them goes on from there.
//!
//! It remembers how many messages each session had in `$FAKE_CLAUDE_DIR/<session id>`, so `--resume` can be checked,
//! and the prompts it read, one per line and in the order it read them, in `$FAKE_CLAUDE_DIR/<session id>.prompts`.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "fake/mcp.rs"]
mod mcp;

/// A user message read back, as `--replay-user-messages` asks.
fn replay(text: &str) {
    out(json!({"type": "user", "isReplay": true, "session_id": "fake", "message": {"role": "user", "content": text}}));
}

fn out(value: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

/// `CLAUDE_CONFIG_DIR`: where Claude Code keeps conversation `session` for the working folder, a line per message.
fn transcript(session: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")?;
    let cwd = std::env::current_dir().ok()?;
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    Some(
        std::path::Path::new(&dir)
            .join("projects")
            .join(name)
            .join(format!("{session}.jsonl")),
    )
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The environment for `FAKE_LAUNCH_RECORD`. A variable whose name looks like a secret has its value redacted, unless
/// `FAKE_RECORD_SECRETS` is set: the tests that hand the agent a made-up credential and read it back set it.
fn recorded_env() -> serde_json::Map<String, Value> {
    let show = std::env::var_os("FAKE_RECORD_SECRETS").is_some();
    let show_app_grant = std::env::var_os("FAKE_RECORD_MCP_APP_TOKEN").is_some();
    std::env::vars()
        .map(|(name, value)| {
            let upper = name.to_ascii_uppercase();
            let secret = ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
                .iter()
                .any(|word| upper.contains(word));
            let fixture_grant = show_app_grant && name == nexus_workbench::mcp_servers::APP_TOKEN_VAR;
            let value = if secret && !show && !fixture_grant {
                "<redacted>".to_string()
            } else {
                value
            };
            (name, json!(value))
        })
        .collect()
}

/// `FAKE_LAUNCH_RECORD`: what this process was started with, for the tests that check what the app gave it.
fn record_launch(args: &[String]) {
    if let Ok(path) = std::env::var("FAKE_LAUNCH_RECORD") {
        let env: serde_json::Map<String, Value> = recorded_env();
        let staged = format!("{path}.tmp");
        if std::fs::write(&staged, json!({"args": args, "env": env}).to_string()).is_ok() {
            let _ = std::fs::rename(staged, path);
        }
    }
}

fn queue_probe(rx: &mpsc::Receiver<Value>, session: &str, replays: bool) -> bool {
    let mut waiting = Vec::new();
    let mut step = 0;
    let mut taken = 0;
    let mut boundary = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if Instant::now() >= boundary {
            if step > 0 {
                out(json!({"type":"user","parent_tool_use_id":null,"session_id":session,
                    "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("probe-{step}"),"content":"step"}]}}));
                for content in waiting.drain(..) {
                    taken += 1;
                    if replays {
                        out(
                            json!({"type":"user","isReplay":true,"session_id":session,"message":{"role":"user","content":content}}),
                        );
                    }
                    out(
                        json!({"type":"assistant","parent_tool_use_id":null,"session_id":session,
                        "message":{"id":format!("received-{taken}"),"content":[{"type":"text","text":format!("Steered input: {content}")}]}}),
                    );
                }
            }
            step += 1;
            out(
                json!({"type":"assistant","parent_tool_use_id":null,"session_id":session,
                "message":{"id":format!("step-{step}"),"content":[{"type":"tool_use","id":format!("probe-{step}"),"name":"Bash","input":{"command":"echo step"}}]}}),
            );
            boundary = Instant::now() + Duration::from_millis(75);
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(message) if message.get("type").and_then(Value::as_str) == Some("user") => {
                waiting.push(message["message"]["content"].clone());
            }
            Ok(message) if message.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt") => {
                out(
                    json!({"type":"control_response","response":{"subtype":"success","request_id":message["request_id"],"response":{}}}),
                );
                return true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            _ => {}
        }
    }
    false
}

/// Tests opt in to initializing one configured stdio server before the first turn.
fn initialize_settings_server(args: &[String], name: &str) {
    use std::process::Stdio;

    let server = args
        .iter()
        .find_map(|arg| {
            let config: Value = serde_json::from_str(arg.strip_prefix("--mcp-config=")?).ok()?;
            config["mcpServers"].get(name).cloned()
        })
        .expect("the configured test MCP server");
    let program = server["command"].as_str().expect("launcher program");
    // A Rust test executable has no app command dispatcher. The host binary runs the same launcher.
    let launcher = std::env::var_os("FAKE_CLAUDE_MCP_LAUNCHER").unwrap_or_else(|| program.into());
    let mut command = nexus_workbench::process::command(launcher);
    for arg in server["args"].as_array().expect("launcher arguments") {
        command.arg(arg.as_str().expect("launcher argument"));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the MCP launcher");
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}});
    writeln!(child.stdin.take().expect("launcher stdin"), "{request}").expect("send initialize");
    let output = child.wait_with_output().expect("MCP launcher reply");
    assert!(output.status.success(), "the MCP server failed: {}", output.status);
    let reply: Value = serde_json::from_slice(&output.stdout).expect("MCP initialize response");
    assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
}

fn main() {
    if let Some(capture) = std::env::var_os("FAKE_PROMPT_CAPTURE") {
        let args: Vec<_> = std::env::args().collect();
        let text = args.windows(2).find_map(|pair| match pair[0].as_str() {
            "--append-system-prompt-file" => Some(std::fs::read_to_string(&pair[1]).expect("read prompt")),
            "--append-system-prompt" => Some(pair[1].clone()),
            _ => None,
        });
        std::fs::write(capture, text.expect("system prompt")).expect("capture prompt");
    }
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("auth") && args.get(2).map(String::as_str) == Some("status") {
        out(json!({"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max"}));
        return;
    }
    record_launch(&args);
    if let Ok(server) = std::env::var("FAKE_CLAUDE_INIT_MCP") {
        initialize_settings_server(&args, &server);
    }
    let resume = arg(&args, "--resume");
    let mut session = resume
        .clone()
        .or_else(|| arg(&args, "--session-id"))
        .unwrap_or_else(|| "fake-session".into());
    // `FAKE_CLAUDE_FORK`: a new conversation runs under another id than the one asked for.
    if resume.is_none() && std::env::var_os("FAKE_CLAUDE_FORK").is_some() {
        session.push_str("-fork");
    }
    // `FAKE_CLAUDE_REJECT_MODES`: a permission mode change is refused.
    let reject_modes = std::env::var_os("FAKE_CLAUDE_REJECT_MODES").is_some();
    // `FAKE_CLAUDE_IGNORE_CHANGES`: a mode, model or effort change is never answered.
    let ignore_changes = std::env::var_os("FAKE_CLAUDE_IGNORE_CHANGES").is_some();
    // `FAKE_CLAUDE_NO_UUID`: messages carry no `uuid`, as older logs never recorded one.
    let no_uuid = std::env::var_os("FAKE_CLAUDE_NO_UUID").is_some();
    let model = arg(&args, "--model").unwrap_or_else(|| "claude-fake-1".into());
    let replays = args.iter().any(|a| a == "--replay-user-messages");
    let remembered = |session: &str| {
        std::env::var("FAKE_CLAUDE_DIR")
            .ok()
            .map(|dir| std::path::Path::new(&dir).join(session))
    };
    let mut count: u64 = remembered(&session)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    if std::env::var_os("FAKE_CLAUDE_FAIL_START").is_some() {
        eprintln!("Claude Code could not start.");
        std::process::exit(1);
    }
    if resume.is_some() && count == 0 {
        eprintln!("No conversation found with session ID: {session}");
        std::process::exit(1);
    }
    // `CLAUDE_CONFIG_DIR`: as Claude Code does, the conversation is resumed only from its transcript there.
    let source = resume.as_deref().and_then(transcript);
    if let Some(path) = &source {
        match std::fs::read_to_string(path) {
            Ok(text) if !text.is_empty() => count = text.lines().count() as u64,
            _ => {
                eprintln!("No conversation found with session ID: {session}");
                std::process::exit(1);
            }
        }
    }
    // `--resume-session-at <session>-a<n>`: the conversation goes on from its n-th answer, the later ones forgotten.
    if let Some(at) = arg(&args, "--resume-session-at") {
        match at
            .rsplit_once("-a")
            .and_then(|(_, n)| n.parse::<u64>().ok())
            .filter(|n| *n <= count)
        {
            Some(n) => count = n,
            None => {
                eprintln!("No message found with message.uuid of: {at}");
                std::process::exit(1);
            }
        }
    }
    if resume.is_some() && args.iter().any(|a| a == "--fork-session") {
        session.push_str("-forked");
    }
    let memory = remembered(&session);
    let kept = transcript(&session);
    if let (Some(from), Some(to)) = (&source, &kept)
        && from != to
    {
        let _ = std::fs::create_dir_all(to.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::copy(from, to);
    }

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && tx.send(value).is_err()
            {
                break;
            }
        }
    });

    let mut total_cost = 0.0;
    let mut requests = 0;
    let mut initialized = false;
    // `lateagent`: the sub-agent approvals still open after the turn.
    let mut late_asks: Vec<String> = Vec::new();
    // `team`: its tasks still running: (task id, run, call, the task a stop of it also stops).
    let mut team: Vec<(&str, &str, &str, Option<&str>)> = Vec::new();
    let refuse_stops = std::env::var_os("FAKE_CLAUDE_REFUSE_STOPS").is_some();
    // The shells running in the background, whose output `get_task_output` reads.
    let mut shells: Vec<String> = Vec::new();
    let ignore_stops = std::env::var_os("FAKE_CLAUDE_IGNORE_STOPS").is_some();
    // `bgagent`: its sub-agents still running, and those a `stop_task` stopped.
    let bg_agents: Arc<Mutex<(HashSet<String>, HashSet<String>)>> = Arc::default();
    // An interrupt that came between turns: what a turn the agent started on its own (`bgagent`) ends on.
    let own_interrupt = Arc::new(AtomicBool::new(false));
    // `initialize` declared `perTaskStopAffordance`.
    let mut per_task_stop = false;
    // `FAKE_CLAUDE_VERSION`: the version `init` reports (default 2.1.292, the one whose controls are recorded).
    let version = std::env::var("FAKE_CLAUDE_VERSION").unwrap_or_else(|_| "2.1.292".into());
    while let Ok(message) = rx.recv() {
        match message.get("type").and_then(Value::as_str) {
            // `lateagent`: the person answered a sub-agent's approval after the turn.
            Some("control_response") => {
                let id = message
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !late_asks.contains(&id) {
                    continue;
                }
                late_asks.retain(|ask| *ask != id);
                if id != "sub-ask-1" {
                    continue;
                }
                let allowed = message.pointer("/response/response/behavior").and_then(Value::as_str) == Some("allow");
                let system = |value: Value| {
                    let mut value = value;
                    value["type"] = json!("system");
                    out(value);
                };
                out(
                    json!({"type": "user", "parent_tool_use_id": "toolu_la1", "agent_id": "agent2", "session_id": session,
                    "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_la_cmd",
                    "content": if allowed { "allowed" } else { "denied" }, "is_error": !allowed}]}}),
                );
                system(
                    json!({"subtype": "task_updated", "task_id": "agent2", "run_id": "run-a2", "patch": {"status": "completed", "end_time": 1}}),
                );
                system(
                    json!({"subtype": "task_notification", "task_id": "agent2", "run_id": "run-a2", "tool_use_id": "toolu_la1",
                    "status": "completed", "summary": "Done late", "output_file": "", "usage": {"total_tokens": 900, "tool_uses": 2}}),
                );
                count += 1;
                out(
                    json!({"type": "assistant", "parent_tool_use_id": null, "session_id": session, "message": {"id": format!("msg_{count}"),
                    "content": [{"type": "text", "text": "agent2 finished"}]}}),
                );
                total_cost += 0.01;
                out(
                    json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "total_cost_usd": total_cost,
                    "result": "agent2 finished", "session_id": session}),
                );
                out(
                    json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "total_cost_usd": total_cost,
                    "result": "", "session_id": session}),
                );
                system(
                    json!({"subtype": "task_notification", "task_id": "last2", "status": "completed", "summary": "", "output_file": ""}),
                );
            }
            Some("control_request")
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("get_task_output") =>
            {
                let id = message.get("request_id").cloned().unwrap_or(Value::Null);
                let task = message
                    .pointer("/request/task_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let running =
                    shells.iter().any(|shell| shell == task) || team.iter().any(|(t, ..)| *t == task && *t == "bshell");
                if !running {
                    out(
                        json!({"type": "control_response", "response": {"subtype": "error", "request_id": id,
                        "error": "get_task_output: no shell or Monitor task with that task_id in this session"}}),
                    );
                    continue;
                }
                let output = if std::env::var_os("FAKE_CLAUDE_BIG_OUTPUT").is_some() {
                    format!("{}end\n", "x".repeat(19996))
                } else {
                    "tick 1\ntick 2\ntick 3\n".to_string()
                };
                out(
                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                    "response": {"output": output, "total_bytes": output.len(), "truncated": false}}}),
                );
            }
            Some("control_request")
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("stop_task") =>
            {
                let id = message.get("request_id").cloned().unwrap_or(Value::Null);
                let target = message
                    .pointer("/request/task_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if ignore_stops {
                    continue;
                }
                {
                    let mut bg = bg_agents.lock().unwrap_or_else(|p| p.into_inner());
                    if bg.0.contains(&target) && !refuse_stops {
                        bg.1.insert(target);
                        out(
                            json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                        );
                        continue;
                    }
                }
                let error = if refuse_stops {
                    Some("fake: refused")
                } else if !team.iter().any(|(task, ..)| *task == target) {
                    Some("fake: asked to stop a task that is not running")
                } else {
                    None
                };
                if let Some(error) = error {
                    out(
                        json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": error}}),
                    );
                    continue;
                }
                out(
                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                );
                let (stopped, left): (Vec<_>, Vec<_>) = team
                    .iter()
                    .partition(|(task, _, _, with)| *task == target || *with == Some(target.as_str()));
                for (task, run, call, _) in stopped {
                    out(
                        json!({"type": "system", "subtype": "task_notification", "task_id": task, "run_id": run, "tool_use_id": call,
                        "status": "stopped", "summary": "", "output_file": ""}),
                    );
                }
                team = left;
            }
            Some("control_request") => {
                let id = message.get("request_id").cloned().unwrap_or(Value::Null);
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt") {
                    own_interrupt.store(true, Ordering::SeqCst);
                }
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("mcp_status") {
                    let reply = json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"mcpServers":[{"name":"tracker","status":"connected","config":{"env":{"TOKEN":"SECRET_MCP_TOKEN"}}},{"name":"offline","status":"failed","error":"SECRET_MCP_ERROR"},{"name":"auth","status":"needs-auth"}]}}});
                    let delay = std::env::var("FAKE_CLAUDE_MCP_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0);
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(delay));
                        out(reply);
                    });
                    continue;
                }
                if ignore_changes
                    && matches!(
                        message.pointer("/request/subtype").and_then(Value::as_str),
                        Some("set_permission_mode" | "set_model" | "apply_flag_settings")
                    )
                {
                    continue;
                }
                if reject_modes
                    && message.pointer("/request/subtype").and_then(Value::as_str) == Some("set_permission_mode")
                {
                    out(
                        json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "not allowed"}}),
                    );
                    continue;
                }
                out(
                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                );
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt")
                    && let Ok(dir) = std::env::var("FAKE_CLAUDE_DIR")
                {
                    std::fs::write(std::path::Path::new(&dir).join(format!("interrupt-{session}")), "")
                        .expect("write interrupt marker");
                }
                if message.pointer("/request/subtype").and_then(Value::as_str) == Some("initialize") {
                    per_task_stop = message.pointer("/request/perTaskStopAffordance") == Some(&json!(true));
                }
                if !initialized {
                    initialized = true;
                    out(
                        json!({"type": "system", "subtype": "init", "session_id": session, "model": model, "cwd": std::env::current_dir().ok(), "slash_commands": ["compact", "review"], "skills":["fixture-check"], "plugins":[{"name":"fixture-plugin","path":"/private/fixture"}], "permissionMode":"default", "claude_code_version": version}),
                    );
                }
            }
            Some("user") => {
                let text = message
                    .pointer("/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if text == "/compact" || text.starts_with("/compact ") {
                    if let Ok(dir) = std::env::var("FAKE_CLAUDE_DIR") {
                        let path = std::path::Path::new(&dir).join("compactions");
                        let old = std::fs::read_to_string(&path).unwrap_or_default();
                        let _ = std::fs::write(path, format!("{old}{session}: {text}\n"));
                    }
                    if text.contains("insufficient") {
                        out(
                            json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"result":"Not enough messages to compact.","session_id":session}),
                        );
                        continue;
                    }
                    out(json!({"type":"system","subtype":"compact_boundary","session_id":session}));
                    out(
                        json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"total_cost_usd":total_cost,"result":"Compacted","session_id":session}),
                    );
                    continue;
                }
                if let Some(name) = text.strip_prefix("/rename ") {
                    if let Ok(dir) = std::env::var("FAKE_CLAUDE_DIR") {
                        let path = std::path::Path::new(&dir).join("renames");
                        let old = std::fs::read_to_string(&path).unwrap_or_default();
                        let _ = std::fs::write(path, format!("{old}{text}\n"));
                    }
                    if replays {
                        replay(&text);
                    }
                    out(json!({"type":"system","subtype":"session_title_changed","title":name,"session_id":session}));
                    out(
                        json!({"type":"assistant","parent_tool_use_id":null,"session_id":session,
                        "message":{"id":format!("msg_rename_{name}"),"type":"message","role":"assistant","model":"<synthetic>",
                        "stop_reason":"end_turn","content":[{"type":"text","text":format!("Session renamed to: {name}")}],
                        "usage":{"input_tokens":0,"output_tokens":0}}}),
                    );
                    out(
                        json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"num_turns":0,"total_cost_usd":0,"result":"","session_id":session}),
                    );
                    continue;
                }
                if text == "removecommands" {
                    out(
                        json!({"type":"system","subtype":"init","session_id":session,"model":model,"slash_commands":[],"claude_code_version":version}),
                    );
                }
                if text == "reinitialize" {
                    out(
                        json!({"type":"system","subtype":"init","session_id":session,"model":model,"slash_commands":["compact","review"],"claude_code_version":version}),
                    );
                }
                if let Ok(path) = std::env::var("FAKE_AGENT_INPUT_RECORD") {
                    let _ = std::fs::write(path, &text);
                }
                let text = text
                    .rsplit("The person's new message follows.\n\n")
                    .next()
                    .unwrap_or(&text)
                    .to_string();
                if text.contains("crash") {
                    std::process::exit(3);
                }
                if replays {
                    replay(&text);
                }
                if text.contains("orphan") {
                    // A test stand-in, never shipped: it needs no console rule (clippy.toml).
                    #[allow(clippy::disallowed_methods)]
                    let mut command = std::process::Command::new("sleep");
                    command
                        .arg("30")
                        .stdin(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    if !text.contains("holdout") {
                        command.stdout(std::process::Stdio::null());
                    }
                    #[cfg(unix)]
                    if text.contains("detach") {
                        use std::os::unix::process::CommandExt;
                        command.process_group(0);
                    }
                    if let Ok(child) = command.spawn() {
                        let _ = std::fs::write("orphan.pid", child.id().to_string());
                    }
                    std::process::exit(3);
                }
                if text.contains("keepchild") {
                    // A test stand-in, never shipped: it needs no console rule (clippy.toml).
                    #[allow(clippy::disallowed_methods)]
                    let spawned = std::process::Command::new("sleep")
                        .arg("600")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    if let Ok(child) = spawned {
                        let pids = format!("{} {}", std::process::id(), child.id());
                        let _ = std::fs::write("keepchild.pids", pids);
                        // Held, never waited for: it lives until the agent's group is stopped.
                        std::mem::forget(child);
                    }
                }
                count += 1;
                if let Some(path) = &kept {
                    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(file, "{}", json!({ "message": count }));
                    }
                }
                if let Some(path) = &memory {
                    let _ = std::fs::write(path, count.to_string());
                    let mut prompts = path.clone().into_os_string();
                    prompts.push(".prompts");
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(prompts) {
                        let _ = writeln!(file, "{}", text.replace('\n', " "));
                    }
                }
                if text.contains("write") {
                    let _ = std::fs::write(format!("msg-{count}.txt"), format!("{text}\n"));
                }
                if text.contains("hitlimit") {
                    let resets_at: i64 = std::env::var("FAKE_CLAUDE_RESETS_AT")
                        .ok()
                        .and_then(|at| at.parse().ok())
                        .unwrap_or(1_792_605_600);
                    out(
                        json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": resets_at,
                        "rateLimitType": "five_hour", "unifiedWindows": {"five_hour": {"utilization": 1.0, "resetsAt": resets_at}}}}),
                    );
                    out(
                        json!({"type": "result", "subtype": "success", "is_error": true, "duration_ms": 1, "total_cost_usd": total_cost,
                        "result": format!("Claude AI usage limit reached|{resets_at}"), "session_id": session}),
                    );
                    continue;
                }
                let started = Instant::now();
                let message_id = format!("msg_{count}");
                out(
                    json!({"type": "stream_event", "parent_tool_use_id": null, "event": {"type": "message_start", "message": {"id": message_id, "model": model}}}),
                );
                let mut reply = format!("Message {count} of this session.");
                if text.contains("env") {
                    reply.push_str(&format!(
                        " Calls go to {}.",
                        std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default()
                    ));
                }
                if text.contains("tokens") {
                    let set = |name: &str| {
                        if std::env::var_os(name).is_some() {
                            "set"
                        } else {
                            "unset"
                        }
                    };
                    reply.push_str(&format!(
                        " Claude token {}. GitHub token {}.",
                        set("CLAUDE_CODE_OAUTH_TOKEN"),
                        set("GH_TOKEN")
                    ));
                }
                if text.contains("openpr") {
                    // The agent opens a pull request from its shell: `gh pr create` prints its URL (created_pulls.rs).
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": null, "session_id": session,
                        "message": {"id": format!("pr-{count}"), "content": [{"type": "tool_use", "id": "toolu_pr",
                        "name": "Bash", "input": {"command": "git push -u origin HEAD && gh pr create --fill"}}]}}),
                    );
                    out(
                        json!({"type": "user", "parent_tool_use_id": null, "session_id": session,
                        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_pr",
                        "content": "Creating pull request for feat into main in example/repo\n\nhttps://github.com/example/repo/pull/77\n"}]}}),
                    );
                }
                if text.contains("bgtask") {
                    let system = |value: Value| {
                        let mut value = value;
                        value["type"] = json!("system");
                        out(value);
                    };
                    let running = |tasks: Value| system(json!({"subtype": "background_tasks_changed", "tasks": tasks}));
                    let bash = json!({"task_id": "bash1", "task_type": "local_bash", "description": "sleep 1"});
                    let agent =
                        json!({"task_id": "agent1", "task_type": "local_agent", "description": "Explore the repo"});
                    running(json!([bash]));
                    system(
                        json!({"subtype": "task_started", "task_id": "bash1", "tool_use_id": "toolu_bg1", "description": "sleep 1",
                        "is_backgrounded": true, "task_type": "local_bash"}),
                    );
                    running(json!([bash, agent]));
                    system(
                        json!({"subtype": "task_started", "task_id": "agent1", "tool_use_id": "toolu_bg2", "description": "Explore the repo",
                        "subagent_type": "Explore", "is_backgrounded": true, "spawn_depth": 1, "task_type": "local_agent", "prompt": "Look around"}),
                    );
                    running(json!([agent]));
                    system(
                        json!({"subtype": "task_updated", "task_id": "bash1", "patch": {"status": "completed", "end_time": 1}}),
                    );
                    system(
                        json!({"subtype": "task_notification", "task_id": "bash1", "tool_use_id": "toolu_bg1", "status": "completed",
                        "summary": "Background command \"sleep 1\" completed (exit code 0)", "output_file": ""}),
                    );
                    running(json!([]));
                    system(
                        json!({"subtype": "task_updated", "task_id": "agent1", "patch": {"status": "killed", "end_time": 2}}),
                    );
                    system(
                        json!({"subtype": "task_notification", "task_id": "agent1", "tool_use_id": "toolu_bg2", "status": "stopped",
                        "summary": "Explore the repo", "output_file": "", "usage": {"total_tokens": 1200, "tool_uses": 4, "duration_ms": 3000}}),
                    );
                }
                if text.contains("subagents") {
                    let system = |value: Value| {
                        let mut value = value;
                        value["type"] = json!("system");
                        out(value);
                    };
                    system(
                        json!({"subtype": "task_started", "task_id": "agent1", "run_id": "run-a1", "tool_use_id": "toolu_sa1",
                        "description": "Survey", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1,
                        "task_type": "local_agent", "prompt": "Look around"}),
                    );
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": "toolu_sa1", "agent_id": "agent1", "session_id": session,
                        "message": {"id": "msg_sa1", "model": "claude-sub-1", "content": [{"type": "tool_use", "id": "toolu_sa_cmd",
                        "name": "Bash", "input": {"command": "ls"}}]}}),
                    );
                    for step in 1..=3u64 {
                        system(
                            json!({"subtype": "task_progress", "task_id": "agent1", "run_id": "run-a1", "tool_use_id": "toolu_sa1",
                            "description": format!("Running step {step}"), "usage": {"total_tokens": 1000 * step, "tool_uses": step,
                            "duration_ms": 100 * step}, "last_tool_name": "Bash"}),
                        );
                    }
                    system(
                        json!({"subtype": "task_started", "task_id": "cmd1", "run_id": "run-c1", "tool_use_id": "toolu_fg1",
                        "description": "make", "is_backgrounded": false, "task_type": "local_bash"}),
                    );
                    system(
                        json!({"subtype": "task_notification", "task_id": "cmd1", "run_id": "run-c1", "tool_use_id": "toolu_fg1",
                        "status": "completed", "summary": "Command \"make\" completed", "output_file": ""}),
                    );
                }
                if text.contains("team") {
                    let started = |task: &str,
                                   run: &str,
                                   call: &str,
                                   title: &str,
                                   kind: &str,
                                   bg: bool,
                                   parent: Option<&str>| {
                        let mut row = json!({"type": "system", "subtype": "task_started", "task_id": task, "run_id": run,
                            "tool_use_id": call, "description": title, "is_backgrounded": bg, "task_type": kind});
                        if let Some(parent) = parent {
                            row["parent_task_id"] = json!(parent);
                        }
                        out(row);
                    };
                    started("boss", "run-boss", "toolu_boss", "Boss", "local_agent", true, None);
                    started(
                        "flow",
                        "run-flow",
                        "toolu_flow",
                        "Release",
                        "local_workflow",
                        true,
                        None,
                    );
                    started(
                        "worker",
                        "run-worker",
                        "toolu_worker",
                        "Worker",
                        "local_agent",
                        false,
                        Some("boss"),
                    );
                    started(
                        "helper",
                        "run-helper",
                        "toolu_helper",
                        "Helper",
                        "local_agent",
                        true,
                        Some("boss"),
                    );
                    started(
                        "bshell",
                        "run-bshell",
                        "toolu_bshell",
                        "sleep 60",
                        "local_bash",
                        true,
                        Some("boss"),
                    );
                    team = vec![
                        ("boss", "run-boss", "toolu_boss", None),
                        ("worker", "run-worker", "toolu_worker", Some("boss")),
                        ("helper", "run-helper", "toolu_helper", None),
                        ("bshell", "run-bshell", "toolu_bshell", Some("boss")),
                        ("flow", "run-flow", "toolu_flow", None),
                    ];
                }
                if text.contains("rerunask") {
                    let system = |value: Value| {
                        let mut value = value;
                        value["type"] = json!("system");
                        out(value);
                    };
                    let start = |run: &str| {
                        json!({"subtype": "task_started", "task_id": "agent5", "run_id": run, "tool_use_id": "toolu_ra5",
                        "description": "Retry", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1,
                        "task_type": "local_agent"})
                    };
                    system(start("run-5a"));
                    out(
                        json!({"type": "control_request", "request_id": "sub-ask-5", "request": {"subtype": "can_use_tool",
                        "tool_name": "Bash", "input": {"command": "make"}, "tool_use_id": "toolu_ra_cmd", "agent_id": "agent5"}}),
                    );
                    system(start("run-5b"));
                    system(
                        json!({"subtype": "task_notification", "task_id": "agent5", "run_id": "run-5a", "tool_use_id": "toolu_ra5",
                        "status": "stopped", "summary": "", "output_file": ""}),
                    );
                }
                if text.contains("lateagent") {
                    let system = |value: Value| {
                        let mut value = value;
                        value["type"] = json!("system");
                        out(value);
                    };
                    system(
                        json!({"subtype": "task_started", "task_id": "agent2", "run_id": "run-a2", "tool_use_id": "toolu_la1",
                        "description": "Work late", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1,
                        "task_type": "local_agent"}),
                    );
                    system(
                        json!({"subtype": "task_started", "task_id": "agent3", "run_id": "run-a3", "tool_use_id": "toolu_la3",
                        "description": "Work now", "subagent_type": "general-purpose", "is_backgrounded": false, "spawn_depth": 1,
                        "task_type": "local_agent"}),
                    );
                    for (ask, agent, tool) in [
                        ("sub-ask-1", "agent2", "toolu_la_cmd"),
                        ("sub-ask-2", "agent2", "toolu_la_cmd2"),
                        ("sub-ask-3", "agent3", "toolu_la_cmd3"),
                    ] {
                        out(
                            json!({"type": "control_request", "request_id": ask, "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                            "input": {"command": "make"}, "tool_use_id": tool, "agent_id": agent}}),
                        );
                    }
                    late_asks = vec!["sub-ask-1".into(), "sub-ask-2".into()];
                }
                if text.contains("recall") {
                    reply.push_str(&format!(" Heard: {text}"));
                }
                if text.contains("nearlimit") {
                    out(
                        json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed_warning", "unifiedWindows": {"five_hour": {"utilization": 0.95}, "seven_day": {"utilization": 0.4}}}}),
                    );
                }
                if text.contains("background") {
                    let task_id = format!("bg-{count}");
                    out(
                        json!({"type": "system", "subtype": "task_started", "task_id": task_id, "tool_use_id": format!("toolu_bg_{count}"),
                        "description": "sleep", "task_type": "local_bash", "is_backgrounded": true, "session_id": session}),
                    );
                    let after = std::env::var("FAKE_CLAUDE_BACKGROUND_MS")
                        .ok()
                        .and_then(|ms| ms.parse().ok())
                        .unwrap_or(1000);
                    let session = session.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(after));
                        out(
                            json!({"type": "system", "subtype": "task_notification", "task_id": task_id, "status": "completed",
                            "summary": "sleep finished", "session_id": session}),
                        );
                    });
                }
                if text.contains("wakeup") {
                    let call = format!("toolu_wake_{count}");
                    let refused = text.contains("wakeup-refused");
                    let delay: f64 = std::env::var("FAKE_CLAUDE_WAKE_SECONDS")
                        .ok()
                        .and_then(|seconds| seconds.parse().ok())
                        .unwrap_or(600.);
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("wake-{count}"), "content": [{"type": "tool_use",
                        "id": call, "name": "ScheduleWakeup", "input": {"delaySeconds": delay, "reason": "watching CI run", "prompt": "check CI"}}]}}),
                    );
                    let result = if refused { "denied" } else { "Wake-up scheduled." };
                    out(
                        json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [{"type": "tool_result",
                        "tool_use_id": call, "content": result, "is_error": refused}]}}),
                    );
                }
                // Title tests provide the model's metadata response, without making a model call. A gate lets a
                // manual rename finish before that response, so the race is deterministic.
                if let Ok(title) = std::env::var("FAKE_CLAUDE_TITLE")
                    && let Some(prefix) = text.lines().find(|line| line.starts_with("[nexus-title:"))
                    && let Some((prefix, _)) = prefix.split_once("] ")
                {
                    if let Ok(gate) = std::env::var("FAKE_CLAUDE_TITLE_GATE") {
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !std::path::Path::new(&gate).exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    reply = format!("{prefix}] {title}\n\n{reply}");
                }
                let (first, rest) = reply.split_at(reply.len() / 2);
                for chunk in [first, rest] {
                    out(
                        json!({"type": "stream_event", "parent_tool_use_id": null, "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": chunk}}}),
                    );
                }
                out({
                    let mut answer = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": message_id, "content": [{"type": "text", "text": reply}]}});
                    if !no_uuid {
                        answer["uuid"] = json!(format!("{session}-a{count}"));
                    }
                    answer
                });
                if text.contains("manycalls") {
                    for n in 0..500 {
                        out(
                            json!({"type": "stream_event", "parent_tool_use_id": null, "event": {"type": "message_delta", "usage": {"input_tokens": n, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}}),
                        );
                    }
                }
                out(
                    json!({"type": "stream_event", "parent_tool_use_id": null, "event": {"type": "message_delta", "usage": {"input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0}}}),
                );
                let mut interrupted = false;
                if text.contains("queue-probe") {
                    interrupted = queue_probe(&rx, &session, replays);
                }
                // `slow nostep`: a message that arrived mid-turn, answered as the next turn.
                let mut deferred: Option<String> = None;
                if text.contains("tool") {
                    requests += 1;
                    let tool_id = format!("toolu_{count}");
                    let request_id = format!("perm-{requests}");
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": message_id, "content": [{"type": "tool_use", "id": tool_id, "name": "Bash", "input": {"command": "echo hi"}}]}}),
                    );
                    // `FAKE_CLAUDE_MODE_SUGGESTION`: "allow for this session" switches to acceptEdits.
                    let suggestion = if std::env::var_os("FAKE_CLAUDE_MODE_SUGGESTION").is_some() {
                        json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})
                    } else {
                        json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "echo:*"}], "behavior": "allow", "destination": "session"})
                    };
                    out(
                        json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                        "input": {"command": "echo hi"}, "tool_use_id": tool_id, "description": "Print hi",
                        "permission_suggestions": [suggestion]}}),
                    );
                    loop {
                        let Ok(answer) = rx.recv() else { std::process::exit(0) };
                        // A mode, model or effort change while the question is open: answered, after
                        // `FAKE_CLAUDE_ACK_DELAY_MS` (default 0), unless changes are ignored. A mode is refused
                        // with `FAKE_CLAUDE_REJECT_MODES`.
                        let subtype = answer.pointer("/request/subtype").and_then(Value::as_str);
                        if answer.get("type").and_then(Value::as_str) == Some("control_request")
                            && subtype != Some("interrupt")
                        {
                            if !ignore_changes {
                                let id = answer.get("request_id").cloned().unwrap_or(Value::Null);
                                let delay = std::env::var("FAKE_CLAUDE_ACK_DELAY_MS")
                                    .ok()
                                    .and_then(|ms| ms.parse().ok())
                                    .unwrap_or(0);
                                let response = if reject_modes && subtype == Some("set_permission_mode") {
                                    json!({"subtype": "error", "request_id": id, "error": "not allowed"})
                                } else {
                                    json!({"subtype": "success", "request_id": id, "response": {}})
                                };
                                std::thread::spawn(move || {
                                    std::thread::sleep(Duration::from_millis(delay));
                                    out(json!({"type": "control_response", "response": response}));
                                });
                            }
                            continue;
                        }
                        if answer.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt") {
                            let id = answer.get("request_id").cloned().unwrap_or(Value::Null);
                            out(json!({"type": "control_cancel_request", "request_id": request_id}));
                            out(
                                json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                            );
                            interrupted = true;
                            break;
                        }
                        if answer.pointer("/response/request_id").and_then(Value::as_str) != Some(request_id.as_str()) {
                            continue;
                        }
                        let response = answer.pointer("/response/response").cloned().unwrap_or(Value::Null);
                        let allowed = response.get("behavior").and_then(Value::as_str) == Some("allow");
                        let remembered = response.get("updatedPermissions").is_some();
                        let output = if allowed {
                            format!("hi{}", if remembered { " (remembered)" } else { "" })
                        } else {
                            format!(
                                "denied: {}",
                                response.get("message").and_then(Value::as_str).unwrap_or_default()
                            )
                        };
                        out(
                            json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_id, "content": output, "is_error": !allowed}]}}),
                        );
                        break;
                    }
                }
                if text.contains("fgwait") {
                    let system = |value: Value| {
                        let mut value = value;
                        value["type"] = json!("system");
                        out(value);
                    };
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": message_id, "content": [{"type": "tool_use",
                        "id": "toolu_fg", "name": "Bash", "input": {"command": "make long"}}]}}),
                    );
                    system(
                        json!({"subtype": "task_started", "task_id": "b_fg", "run_id": "run-fg", "tool_use_id": "toolu_fg",
                        "description": "make long", "is_backgrounded": false, "task_type": "local_bash"}),
                    );
                    let result = |content: &str| {
                        json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [{"type": "tool_result",
                        "tool_use_id": "toolu_fg", "content": content, "is_error": false}]}})
                    };
                    let deadline = Instant::now() + Duration::from_secs(10);
                    let mut moved = false;
                    while Instant::now() < deadline && !moved {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(ask)
                                if ask.pointer("/request/subtype").and_then(Value::as_str)
                                    == Some("background_tasks")
                                    && ask.pointer("/request/tool_use_id") == Some(&json!("toolu_fg")) =>
                            {
                                let id = ask.get("request_id").cloned().unwrap_or(Value::Null);
                                if refuse_stops {
                                    out(
                                        json!({"type": "control_response", "response": {"subtype": "error", "request_id": id,
                                        "error": "fake: refused"}}),
                                    );
                                    continue;
                                }
                                system(
                                    json!({"subtype": "task_updated", "task_id": "b_fg", "run_id": "run-fg", "patch": {"is_backgrounded": true}}),
                                );
                                out(
                                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                                    "response": {"backgrounded": true}}}),
                                );
                                out(result("Command was manually backgrounded by user with ID: b_fg."));
                                shells.push("b_fg".into());
                                moved = true;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            _ => {}
                        }
                    }
                    if !moved {
                        out(result("made"));
                        system(
                            json!({"subtype": "task_notification", "task_id": "b_fg", "run_id": "run-fg", "tool_use_id": "toolu_fg",
                            "status": "completed", "summary": "Command \"make long\" completed", "output_file": ""}),
                        );
                    }
                }
                if text.contains("slow") && !interrupted {
                    let slow_ms = std::env::var("FAKE_CLAUDE_SLOW_MS")
                        .ok()
                        .and_then(|ms| ms.parse().ok())
                        .unwrap_or(30_000);
                    let deadline = Instant::now() + Duration::from_millis(slow_ms);
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(answer)
                                if answer.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt")
                                    && !text.contains("deaf") =>
                            {
                                let id = answer.get("request_id").cloned().unwrap_or(Value::Null);
                                out(
                                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                                );
                                if !per_task_stop {
                                    for (task, run, call, _) in team.drain(..) {
                                        out(
                                            json!({"type": "system", "subtype": "task_notification", "task_id": task, "run_id": run,
                                            "tool_use_id": call, "status": "stopped", "summary": "", "output_file": ""}),
                                        );
                                    }
                                }
                                interrupted = true;
                                break;
                            }
                            // A message that arrives mid-turn joins it, as Claude Code does at its next step: the
                            // turn answers it and ends.
                            Ok(steer) if steer.get("type").and_then(Value::as_str) == Some("user") => {
                                let heard = steer
                                    .pointer("/message/content")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                if text.contains("nostep") {
                                    deferred = Some(heard.to_string());
                                    // `hold`: the turn works on until it is stopped, still owing the message.
                                    if text.contains("hold") {
                                        continue;
                                    }
                                    break;
                                }
                                if replays {
                                    replay(heard);
                                }
                                out(
                                    json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("{message_id}-steer"), "content": [{"type": "text", "text": format!("Heard mid-turn: {heard}")}]}}),
                                );
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) if text.contains("hold-after-eof") => {
                                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            // `dies`: the agent exits with code 3 half a second into the turn.
                            _ if text.contains("dies") && started.elapsed() >= Duration::from_millis(500) => {
                                std::process::exit(3)
                            }
                            _ => {}
                        }
                    }
                }
                if text.contains("relay") && !interrupted {
                    let relay = std::env::var("FAKE_CLAUDE_DIR")
                        .ok()
                        .map(|dir| std::path::Path::new(&dir).join("relay"));
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let mut calls = 0;
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Ok(answer)
                                if answer.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt") =>
                            {
                                let id = answer.get("request_id").cloned().unwrap_or(Value::Null);
                                out(
                                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}),
                                );
                                interrupted = true;
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            _ => {}
                        }
                        let Some(relay) = &relay else { break };
                        let Ok(body) = std::fs::read_to_string(relay) else {
                            continue;
                        };
                        let _ = std::fs::remove_file(relay);
                        if body == "END" {
                            break;
                        }
                        if body == "USAGE" {
                            out(json!({"type": "stream_event", "parent_tool_use_id": null,
                                "event": {"type": "message_start", "message": {"id": format!("budget-{count}-{calls}"),
                                "model": "claude-fake-1", "usage": {"input_tokens": 0, "output_tokens": 0}}}}));
                            out(json!({"type": "stream_event", "parent_tool_use_id": null,
                                "event": {"type": "message_delta", "usage": {"output_tokens": 50}}}));
                            calls += 1;
                            continue;
                        }
                        calls += 1;
                        let call = format!("toolu_relay_{count}_{calls}");
                        let (body, is_error) = match body.strip_prefix("ERROR ") {
                            Some(rest) => (rest.to_string(), true),
                            None => (body, false),
                        };
                        out(
                            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("{message_id}-{calls}"),
                            "content": [{"type": "tool_use", "id": call, "name": "mcp__nexus_app__wait_sessions", "input": {}}]}}),
                        );
                        out(
                            json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [{"type": "tool_result",
                            "tool_use_id": call, "content": [{"type": "text", "text": body}], "is_error": is_error}]}}),
                        );
                    }
                }
                if (text.contains("orchestrate") || mcp::hands_off(&text)) && !interrupted {
                    let mut calls = 0;
                    mcp::act(mcp::Server::claude(&args), "claude", &text, |called| {
                        calls += 1;
                        let call = format!("toolu_orch_{count}_{calls}");
                        out(
                            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("{message_id}-orch-{calls}"),
                            "content": [{"type": "tool_use", "id": call, "name": format!("mcp__nexus_app__{}", called.tool), "input": called.args}]}}),
                        );
                        out(
                            json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [{"type": "tool_result",
                            "tool_use_id": call, "content": [{"type": "text", "text": called.text}], "is_error": called.is_error}]}}),
                        );
                    });
                }
                if text.contains("bgagent") {
                    let task_id = format!("bga-{count}");
                    let call = format!("toolu_bga_{count}");
                    out(
                        json!({"type": "system", "subtype": "task_started", "task_id": task_id, "tool_use_id": call,
                        "description": "survey", "task_type": "local_agent", "session_id": session}),
                    );
                    let dir = std::env::var("FAKE_CLAUDE_DIR").ok().map(std::path::PathBuf::from);
                    let session = session.clone();
                    let (bg, own_interrupt) = (bg_agents.clone(), own_interrupt.clone());
                    bg.lock().unwrap_or_else(|p| p.into_inner()).0.insert(task_id.clone());
                    let works = std::env::var("FAKE_CLAUDE_OWN_TURN_MS")
                        .ok()
                        .and_then(|ms| ms.parse().ok())
                        .map(Duration::from_millis);
                    std::thread::spawn(move || {
                        let Some(dir) = dir else { return };
                        let stopped = || bg.lock().unwrap_or_else(|p| p.into_inner()).1.contains(&task_id);
                        let until = |name: &str, or: &dyn Fn() -> bool| {
                            let path = dir.join(name);
                            let deadline = Instant::now() + Duration::from_secs(30);
                            while !path.exists() && !or() && Instant::now() < deadline {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            let _ = std::fs::remove_file(path);
                        };
                        until("bgagent-end", &stopped);
                        let status = if stopped() { "stopped" } else { "completed" };
                        bg.lock().unwrap_or_else(|p| p.into_inner()).0.remove(&task_id);
                        out(
                            json!({"type": "system", "subtype": "task_updated", "task_id": task_id, "patch": {"status": status, "end_time": 1},
                            "session_id": session}),
                        );
                        out(
                            json!({"type": "system", "subtype": "task_notification", "task_id": task_id, "tool_use_id": call,
                            "status": status, "summary": "Surveyed", "output_file": "", "session_id": session}),
                        );
                        until("bgagent-turn", &|| false);
                        own_interrupt.store(false, Ordering::SeqCst);
                        out(
                            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("{task_id}-answer"),
                            "content": [{"type": "text", "text": "The survey is in."}]}}),
                        );
                        let interrupted = works.is_some_and(|works| {
                            let deadline = Instant::now() + works;
                            while !own_interrupt.load(Ordering::SeqCst) && Instant::now() < deadline {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            own_interrupt.load(Ordering::SeqCst)
                        });
                        if interrupted {
                            out(
                                json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "duration_ms": 1,
                                "total_cost_usd": 1.0, "session_id": session}),
                            );
                        } else {
                            out(
                                json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "total_cost_usd": 1.0,
                                "result": "The survey is in.", "session_id": session}),
                            );
                        }
                    });
                }
                if text.contains("autoturn") {
                    let dir = std::env::var("FAKE_CLAUDE_DIR").ok().map(std::path::PathBuf::from);
                    let session = session.clone();
                    std::thread::spawn(move || {
                        let Some(dir) = dir else { return };
                        let path = dir.join(format!("automatic-{session}"));
                        let deadline = Instant::now() + Duration::from_secs(30);
                        while !path.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if !path.exists() {
                            return;
                        }
                        out(json!({"type": "stream_event", "parent_tool_use_id": null,
                            "event": {"type": "message_start", "message": {"id": "automatic", "model": "claude-fake"}}}));
                        out(json!({"type": "stream_event", "parent_tool_use_id": null,
                            "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Automatic"}}}));
                        let interrupted = dir.join(format!("interrupt-{session}"));
                        let finish = dir.join(format!("finish-automatic-{}", session));
                        while !interrupted.exists() && !finish.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if !interrupted.exists() && !finish.exists() {
                            return;
                        }
                        out(
                            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": "automatic",
                            "content": [{"type": "text", "text": "An automatic turn."}]}}),
                        );
                        out(
                            json!({"type": "result", "subtype": if interrupted.exists() { "error_during_execution" } else { "success" },
                            "is_error": interrupted.exists(), "duration_ms": 1, "total_cost_usd": 0.01, "session_id": session}),
                        );
                    });
                }
                if text.contains("pause") {
                    std::thread::sleep(Duration::from_millis(300));
                }
                total_cost += 0.01;
                let duration = started.elapsed().as_millis() as u64;
                if interrupted {
                    out(
                        json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "duration_ms": duration, "total_cost_usd": total_cost, "session_id": session}),
                    );
                } else {
                    out(
                        json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": duration, "total_cost_usd": total_cost,
                        "result": reply, "session_id": session, "modelUsage": {model.clone(): {"contextWindow": 200000}}}),
                    );
                }
                if let Some(heard) = deferred {
                    if replays {
                        replay(&heard);
                    }
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("{message_id}-next"), "content": [{"type": "text", "text": format!("Heard next turn: {heard}")}]}}),
                    );
                    total_cost += 0.01;
                    out(
                        json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "total_cost_usd": total_cost,
                        "result": format!("Heard next turn: {heard}"), "session_id": session, "modelUsage": {model.clone(): {"contextWindow": 200000}}}),
                    );
                }
                if text.contains("latecompact") {
                    out(
                        json!({"type": "system", "subtype": "compact_boundary", "session_id": session,
                        "compact_metadata": {"trigger": "auto", "pre_tokens": 160000}}),
                    );
                }
                if text.contains("latereport") {
                    out(
                        json!({"type": "system", "subtype": "task_notification", "task_id": "late1", "status": "completed",
                        "summary": "Finished after the turn", "output_file": ""}),
                    );
                }
                if text.contains("lateagent") {
                    out(
                        json!({"type": "assistant", "parent_tool_use_id": "toolu_la1", "agent_id": "agent2", "session_id": session,
                        "message": {"id": "msg_la1", "model": "claude-sub-1", "content": [{"type": "text", "text": "Working late"},
                        {"type": "tool_use", "id": "toolu_la_t1", "name": "Read", "input": {"file_path": "notes.txt"}}]}}),
                    );
                    out(
                        json!({"type": "user", "parent_tool_use_id": "toolu_la1", "agent_id": "agent2", "session_id": session,
                        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_la_t1", "content": "notes"}]}}),
                    );
                    if text.contains("thenexit") {
                        std::process::exit(3);
                    }
                }
                if text.contains("subagents") {
                    for line in [
                        json!({"type": "system", "subtype": "task_updated", "task_id": "agent1", "run_id": "run-a1",
                            "patch": {"status": "completed", "end_time": 1}}),
                        json!({"type": "system", "subtype": "task_notification", "task_id": "agent1", "run_id": "run-a1",
                            "tool_use_id": "toolu_sa1", "status": "completed", "summary": "Surveyed", "output_file": "",
                            "usage": {"total_tokens": 3500, "tool_uses": 3, "duration_ms": 400}}),
                        json!({"type": "system", "subtype": "task_progress", "task_id": "agent1", "run_id": "run-a1",
                            "tool_use_id": "toolu_sa1", "description": "Late", "usage": {"total_tokens": 9, "tool_uses": 9}}),
                        // The last line: a test that sees it has seen everything before it.
                        json!({"type": "system", "subtype": "task_notification", "task_id": "last1", "status": "completed",
                            "summary": "", "output_file": ""}),
                    ] {
                        out(line);
                    }
                }
            }
            _ => {}
        }
    }
}
