//! A stand-in for `codex app-server`, for the workbench's tests: the same JSON-RPC lines in and out, with no model.
//! Each turn streams a message, reports one call's tokens and completes; words in the message add to that:
//!
//! - `tool`: runs `echo hi` after asking (`item/commandExecution/requestApproval`) and says what the answer was; a
//!   reason sent with a denial (`turn/steer`) shows in the reply.
//! - `edit`: writes `notes.txt` after asking (`item/fileChange/requestApproval`).
//! - `autoturn`: starts a streamed continuation when `automatic-<session>` appears in the fake's memory folder.
//!   It finishes after receiving an interrupt or `finish-automatic-<session>`.
//! - `slow`: works until interrupted (or 30 seconds).
//! - `queue-probe`: completes tools until interrupted and reads steering input at the next tool boundary.
//! - `crash`: exits with code 3 at once.
//! - `model`: says the model it runs (`-c model=…`).
//! - `subagent`: spawns a sub-agent as Codex 0.164.0 reports one (`subAgentActivity`); its own thread answers. After
//!   the turn's last call it reports a call's tokens of its own and ends, before the turn does.
//!
//! - `latechild`: spawns a sub-agent that outlives the turn, as in Codex 0.164.0's `late-child` recording. After the
//!   turn ends, the child asks to run `make test` (request 901) during the turn and leaves it unanswered. After the turn
//!   it updates its plan, writes, starts `make` and asks to run it (request 900, on its own thread and turn). The answer
//!   runs or declines it; then the child reports a call's tokens (as `gpt-child`), its turn ends and its spawn completes.
//!
//! - `sleeper`: spawns a sub-agent that works on after the turn with no approval: it writes once the turn ended, and
//!   its turn completes `FAKE_CODEX_CHILD_MS` later (default 600). A `turn/interrupt` on its thread before then ends its
//!   turn `interrupted`. With `FAKE_CODEX_CHILD_NO_TURN`, the child's turn is never named (no `turn/started`).
//!
//! - `relay`: the turn runs until told to end, as a lead waiting on the app's tools does. Each time a file `relay`
//!   appears in `$FAKE_CODEX_DIR`, it is taken, and its text comes back as a `nexus_app` `wait_sessions` call's
//!   result (a failed call when it starts with `ERROR `); `END` ends the turn. An interrupt ends it too.
//!
//! - `orchestrate`: as a lead given the app's `nexus_app` server (its `-c mcp_servers.nexus_app` override and
//!   `NEXUS_APP_MCP_TOKEN`), calls it over HTTP within the turn: `initialize`, `tools/list`, `start_session` (a `codex`
//!   child; `orchestrate claude 2` starts two `claude` children), then `wait_sessions` on them. Each call is reported
//!   as Codex reports an MCP tool call (`mcpToolCall`). Nothing when it was not given the server.
//! - `handoff` or `hand off`: as `orchestrate`, but calls `handoff_session` to the other agent the message names
//!   (`claude` by default).
//!
//! `FAKE_CODEX_REJECT_STEER`: a `turn/steer` ends the `queue-probe` turn and is refused ("no active turn to steer").
//! With `crash`, it then exits with code 3; with `late`, the refusal waits for `reject-steer` in `$FAKE_CODEX_DIR`.
//!
//! `FAKE_LAUNCH_RECORD`: a file it writes its arguments and its environment to as it starts, as JSON.
//!
//! `writer-held` in `$FAKE_CODEX_DIR`: `thread/resume` is refused as when another Codex process has the thread open (the
//! ChatGPT app shares `~/.codex`), with Codex 0.169.0's words on stdout and stderr. `thread/fork` still works, as it
//! does there.
//!
//! `thread/fork` starts `<thread id>-forked`, which goes on from where that thread was.
//! `thread/revert` drops a turn and every later one, so the next turn is numbered after the ones kept.
//!
//! `FAKE_CODEX_LATE_TURN`: `turn/start` answers before the turn has an id, and `turn/started` names it a moment later,
//! as older Codex releases do. It remembers how many turns each thread had in `$FAKE_CODEX_DIR/<thread id>`, so
//! `thread/resume` can be checked.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "fake/mcp.rs"]
mod mcp;

fn out(value: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

fn method(message: &Value) -> &str {
    message.get("method").and_then(Value::as_str).unwrap_or_default()
}

fn record_steer(message: &Value, thread: &str, turn: &str) {
    let id = &message["id"];
    out(json!({"id":id,"result":{"turnId":turn}}));
    out(
        json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,
        "item":{"type":"userMessage","id":format!("steer-{id}"),"content":message["params"]["input"]}}}),
    );
}

fn queue_probe(rx: &mpsc::Receiver<Value>, thread: &str, turn: &str) -> bool {
    let mut waiting = Vec::new();
    let mut step = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut boundary = Instant::now();
    while Instant::now() < deadline {
        if Instant::now() >= boundary {
            if step > 0 {
                out(
                    json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,
                    "item":{"type":"commandExecution","id":format!("probe-{step}"),"command":"echo step",
                    "status":"completed","aggregatedOutput":"step","exitCode":0}}}),
                );
                for (id, input) in waiting.drain(..) {
                    out(
                        json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,
                        "item":{"type":"userMessage","id":format!("steer-{id}"),"content":input}}}),
                    );
                    out(
                        json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,
                        "item":{"type":"agentMessage","id":format!("received-{id}"),"text":format!("Steered input: {input}")}}}),
                    );
                }
            }
            step += 1;
            out(
                json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,
                "item":{"type":"commandExecution","id":format!("probe-{step}"),"command":"echo step","status":"inProgress"}}}),
            );
            boundary = Instant::now() + Duration::from_millis(75);
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(message) if method(&message) == "turn/steer" => {
                let id = message["id"].clone();
                // `late`: the turn ends first, and the steer is refused once `reject-steer` appears in the fake's
                // memory folder, as a steer that crossed the turn's end is.
                if std::env::var("FAKE_CODEX_REJECT_STEER").as_deref() == Ok("late") {
                    let signal = std::env::var("FAKE_CODEX_DIR")
                        .map(|dir| std::path::Path::new(&dir).join("reject-steer"))
                        .unwrap_or_default();
                    std::thread::spawn(move || {
                        let deadline = Instant::now() + Duration::from_secs(30);
                        while !signal.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        out(json!({"id":id,"error":{"code":-32600,"message":"no active turn to steer"}}));
                    });
                    return false;
                }
                if std::env::var_os("FAKE_CODEX_REJECT_STEER").is_some() {
                    out(json!({"id":id,"error":{"code":-32600,"message":"no active turn to steer"}}));
                    if std::env::var("FAKE_CODEX_REJECT_STEER").as_deref() == Ok("crash") {
                        std::process::exit(3);
                    }
                    return false;
                }
                assert_eq!(message["params"]["expectedTurnId"], turn);
                out(json!({"id":id,"result":{"turnId":turn}}));
                waiting.push((id, message["params"]["input"].clone()));
            }
            Ok(message) if method(&message) == "turn/interrupt" => {
                out(json!({"id":message["id"],"result":{}}));
                return true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            _ => {}
        }
    }
    false
}

/// The environment for `FAKE_LAUNCH_RECORD`. A variable whose name looks like a secret has its value redacted, unless
/// `FAKE_RECORD_SECRETS` is set: the tests that hand the agent a made-up credential and read it back set it.
fn recorded_env() -> serde_json::Map<String, Value> {
    let show = std::env::var_os("FAKE_RECORD_SECRETS").is_some();
    std::env::vars()
        .map(|(name, value)| {
            let upper = name.to_ascii_uppercase();
            let secret = ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
                .iter()
                .any(|word| upper.contains(word));
            let value = if secret && !show {
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    record_launch(&args);
    let mut model = args
        .iter()
        .find_map(|arg| arg.strip_prefix("model="))
        .map(|model| model.trim_matches('"').to_string())
        .unwrap_or_else(|| "gpt-fake-1".into());
    let late = std::env::var_os("FAKE_CODEX_LATE_TURN").is_some();
    let memory = std::env::var("FAKE_CODEX_DIR").ok().map(std::path::PathBuf::from);

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

    let mut thread = String::new();
    let mut count: u64 = 0;
    let mut account_reads = 0_u64;
    let mut mcp_reads = 0_u64;
    let mut skills_reads = 0_u64;
    let mut requests = 0;
    // `latechild`: the child still at work after the turn: its thread, its turn and its spawn's id.
    let mut late_child: Option<(String, String, String)> = None;
    while let Ok(message) = rx.recv() {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        // `latechild`: the answer to the child's request, after the turn.
        if id == json!(900)
            && message.get("result").is_some()
            && let Some((child, child_turn, _)) = late_child.take()
        {
            let accepted = message.pointer("/result/decision") == Some(&json!("accept"));
            let mut done = json!({"type": "commandExecution", "id": "exec-late", "command": "make", "cwd": "/r",
                "status": if accepted { "completed" } else { "declined" }});
            if accepted {
                done["aggregatedOutput"] = json!("made");
                done["exitCode"] = json!(0);
            }
            out(json!({"method": "item/completed", "params": {"threadId": child, "turnId": child_turn, "item": done}}));
            out(
                json!({"method": "thread/tokenUsage/updated", "params": {"threadId": child, "turnId": child_turn, "tokenUsage": {
                "last": {"inputTokens": 500, "cachedInputTokens": 400, "cacheWriteInputTokens": 0, "outputTokens": 20},
                "modelContextWindow": 258400}}}),
            );
            out(
                json!({"method": "turn/completed", "params": {"threadId": child, "turn": {"id": child_turn, "status": "completed",
                "error": null, "durationMs": 7}}}),
            );
            let end = json!({"type": "subAgentActivity", "id": format!("subagent-completed-{child_turn}"), "kind": "completed",
                "model": null, "agentThreadId": child, "agentPath": "/root/late"});
            for event in ["item/started", "item/completed"] {
                out(json!({"method": event, "params": {"item": end, "threadId": thread, "turnId": "none"}}));
            }
            continue;
        }
        match method(&message) {
            "initialize" => out(json!({"id": id, "result": {"userAgent": "fake-codex/0.164.0 (fake)"}})),
            "account/read" => {
                account_reads += 1;
                let mode = std::env::var("FAKE_CODEX_CONNECTION").unwrap_or_default();
                if account_reads > 1 && ["expires", "timeout"].contains(&mode.as_str()) {
                    if mode == "expires" {
                        out(json!({"id":id,"error":{"code":-32601,"message":"unavailable"}}));
                    }
                    continue;
                }
                let race = mode == "race";
                if race && account_reads == 1 {
                    out(json!({"method":"account/updated","params":{"authMode":"chatgpt","planType":"pro"}}));
                    let old = id.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(120));
                        out(
                            json!({"id":old,"result":{"account":{"type":"chatgpt","email":"stale@example.test","planType":"free"}}}),
                        );
                    });
                } else {
                    out(
                        json!({"id":id,"result":{"account":{"type":"chatgpt","email":"current@example.test","planType":"pro","accessToken":"SECRET_ACCOUNT_TOKEN"},"requiresOpenaiAuth":true}}),
                    );
                }
            }
            "mcpServerStatus/list" => {
                mcp_reads += 1;
                let mode = std::env::var("FAKE_CODEX_CONNECTION").unwrap_or_default();
                if mcp_reads > 2 && mode == "timeout" {
                    continue;
                }
                if mode == "unavailable" || (mcp_reads > 2 && mode == "expires") {
                    out(json!({"id":id,"error":{"code":-32601,"message":"SECRET_RAW_ERROR"}}));
                    continue;
                }
                let target = message
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if target != thread {
                    out(json!({"id":id,"error":{"code":-32602,"message":"Wrong thread"}}));
                    continue;
                }
                if mode == "race" && mcp_reads == 1 {
                    out(
                        json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":thread,"name":"tracker","status":"failed","error":"SECRET_STARTUP_ERROR"}}),
                    );
                    let delayed = id.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(100));
                        out(
                            json!({"id":delayed,"result":{"data":[{"name":"tracker","runtimeStatus":"connected","authStatus":"oAuth"}],"nextCursor":null}}),
                        );
                    });
                } else if message.pointer("/params/cursor").and_then(Value::as_str) == Some("next") {
                    out(
                        json!({"id":id,"result":{"data":[{"name":"configured-only","runtimeStatus":null,"authStatus":"oAuth","tools":{}},{"name":"offline","runtimeStatus":"failed","authStatus":"unsupported","toolsError":"SECRET_MCP_ERROR","config":{"headers":{"Authorization":"SECRET_MCP_TOKEN"}}}],"nextCursor":null}}),
                    );
                } else {
                    out(
                        json!({"id":id,"result":{"data":[{"name":"tracker","runtimeStatus":"connected","authStatus":"notLoggedIn","tools":{}}],"nextCursor":"next"}}),
                    );
                    out(
                        json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":null,"name":"tracker","status":"failed","error":"app-scope"}}),
                    );
                }
            }
            "skills/list" => {
                skills_reads += 1;
                let mode = std::env::var("FAKE_CODEX_CONNECTION").unwrap_or_default();
                if skills_reads > 1 && ["expires", "timeout"].contains(&mode.as_str()) {
                    if mode == "expires" {
                        out(json!({"id":id,"error":{"code":-32601,"message":"unavailable"}}));
                    }
                    continue;
                }
                let cwd = message.pointer("/params/cwds/0").cloned().unwrap_or(Value::Null);
                let result = json!({"id":id,"result":{"data":[{"cwd":"/wrong-folder","skills":[{"name":"wrong-skill"}]},{"cwd":cwd,"skills":[{"name":"fixture-check","path":"/private/fixture/SKILL.md","enabled":true}],"errors":[]}]}});
                if std::env::var("FAKE_CODEX_CONNECTION").as_deref() == Ok("race") {
                    out(json!({"method":"skills/changed","params":{}}));
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(110));
                        out(result);
                    });
                } else {
                    out(result);
                }
            }
            "thread/compact/start" => {
                let target = message
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(dir) = &memory {
                    let path = dir.join("compactions");
                    let old = std::fs::read_to_string(&path).unwrap_or_default();
                    let _ = std::fs::write(path, format!("{old}{target}\n"));
                }
                if std::env::var("FAKE_CODEX_COMPACT").as_deref() == Ok("refuse") || target != thread {
                    out(json!({"id": id, "error": {"code": -32601, "message": "Compaction unavailable"}}));
                    continue;
                }
                if std::env::var("FAKE_CODEX_COMPACT").as_deref() == Ok("silent") {
                    continue;
                }
                out(json!({"id": id, "result": {}}));
                let run = "compact-1";
                out(json!({"method":"turn/started", "params":{"threadId":thread,"turn":{"id":run}}}));
                std::thread::sleep(Duration::from_millis(80));
                out(
                    json!({"method":"item/completed", "params":{"threadId":thread,"turnId":run,"item":{"id":"compacted","type":"contextCompaction"}}}),
                );
                out(
                    json!({"method":"turn/completed", "params":{"threadId":thread,"turn":{"id":run,"status":"completed","durationMs":80}}}),
                );
            }
            "thread/start" | "thread/resume" | "thread/fork" => {
                if let Some(capture) = std::env::var_os("FAKE_PROMPT_CAPTURE") {
                    let text = message["params"]["developerInstructions"]
                        .as_str()
                        .expect("developer instructions");
                    std::fs::write(capture, text).expect("capture prompt");
                }
                if let Some(ms) = std::env::var("FAKE_CODEX_THREAD_MS")
                    .ok()
                    .and_then(|ms| ms.parse().ok())
                {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                thread = match message.pointer("/params/threadId").and_then(Value::as_str) {
                    Some(old) => old.to_string(),
                    None => format!("thr_{}", std::process::id()),
                };
                let path = memory.as_ref().map(|dir| dir.join(&thread));
                count = path
                    .as_ref()
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .and_then(|text| text.trim().parse().ok())
                    .unwrap_or(0);
                if method(&message) == "thread/fork" {
                    thread.push_str("-forked");
                }
                if method(&message) == "thread/resume"
                    && memory.as_ref().is_some_and(|dir| dir.join("writer-held").exists())
                {
                    eprintln!(
                        "ERROR codex_core::session: Failed to create session: thread-store conflict: thread {thread} already has an active writer"
                    );
                    out(
                        json!({"id": id, "error": {"code": -32600, "message": format!("thread {thread} already has an active writer")}}),
                    );
                    continue;
                }
                if method(&message) != "thread/start" && count == 0 {
                    out(
                        json!({"id": id, "error": {"code": -32600, "message": format!("no rollout found for thread id {thread}")}}),
                    );
                    continue;
                }
                if let Some(dir) = &memory {
                    let policy = message
                        .pointer("/params/approvalPolicy")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let _ = std::fs::write(dir.join(format!("{thread}.policy")), policy);
                }
                out(
                    json!({"id": id, "result": {"thread": {"id": thread, "model": model}, "model": model, "modelProvider": "openai"}}),
                );
            }
            // Drops `beforeTurnId` (`turn_<n>`) and every later turn.
            "thread/revert" => {
                let before = message
                    .pointer("/params/beforeTurnId")
                    .and_then(Value::as_str)
                    .and_then(|turn| turn.strip_prefix("turn_"))
                    .and_then(|n| n.parse::<u64>().ok())
                    .filter(|n| *n >= 1 && *n <= count);
                match before {
                    Some(n) => {
                        count = n - 1;
                        if let Some(dir) = &memory {
                            let _ = std::fs::write(dir.join(&thread), count.to_string());
                        }
                        out(json!({"id": id, "result": {"thread": {"id": thread}}}));
                    }
                    None => out(json!({"id": id, "error": {"code": -32600, "message": "unknown turn"}})),
                }
            }
            "turn/interrupt" => {
                if let Some(dir) = &memory {
                    std::fs::write(dir.join(format!("interrupt-{thread}")), "").expect("write interrupt marker");
                }

                out(json!({"error": {"code": -32600, "message": "no active turn to interrupt"}, "id": id}))
            }
            "turn/start" => {
                // A model change rides on the turn, as Codex takes it.
                if let Some(next) = message
                    .pointer("/params/model")
                    .or_else(|| message.pointer("/params/collaborationMode/settings/model"))
                    .and_then(Value::as_str)
                {
                    model = next.to_string();
                }
                let text = message
                    .pointer("/params/input")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|input| input.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
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
                count += 1;
                if let Some(dir) = &memory {
                    let _ = std::fs::write(dir.join(&thread), count.to_string());
                }
                let turn = format!("turn_{count}");
                let started = Instant::now();
                if late {
                    out(json!({"id": id, "result": {"turn": {"id": null, "status": "inProgress"}}}));
                    std::thread::sleep(Duration::from_millis(300));
                } else {
                    out(json!({"id": id, "result": {"turn": {"id": turn, "status": "inProgress"}}}));
                }
                out(
                    json!({"method": "turn/started", "params": {"threadId": thread, "turn": {"id": turn, "status": "inProgress"}}}),
                );
                // The message, recorded in the conversation, as Codex 0.164.0 does.
                out(
                    json!({"method": "item/completed", "params": {"threadId": thread, "turnId": turn, "item": {"type": "userMessage", "id": format!("user-{count}"), "content": message.pointer("/params/input")}}}),
                );

                let mut reply = format!("Message {count} of this thread.");
                if text.contains("model") {
                    reply.push_str(&format!(" Running {model}."));
                }
                let mut interrupted = false;
                if text.contains("queue-probe") {
                    interrupted = queue_probe(&rx, &thread, &turn);
                }
                if text.contains("question") {
                    requests += 1;
                    let request = json!(requests);
                    out(json!({"method":"item/tool/requestUserInput","id":request,"params":{
                    "threadId":thread,"turnId":turn,"itemId":format!("question-{count}"),
                    "isBlocking":!text.contains("nonblocking"), "questions":[{
                        "id":"usage","header":"Usage","question":"Which usage counts?",
                        "options":[{"label":"Every turn","description":"Include person-started turns"}]
                    }]}}));
                    if text.contains("nonblocking") {
                        out(
                            json!({"method":"item/completed","params":{"threadId":thread,"turnId":turn,
                            "item":{"type":"agentMessage","id":"independent","text":"Checking another file while you answer."}}}),
                        );
                    }
                    loop {
                        let Ok(answer) = rx.recv() else { std::process::exit(0) };
                        if method(&answer) == "turn/steer" {
                            record_steer(&answer, &thread, &turn);
                            continue;
                        }
                        if method(&answer) == "turn/interrupt" {
                            out(json!({"id":answer.get("id"),"result":{}}));
                            interrupted = true;
                        } else if answer.get("id") == Some(&request) && answer.get("method").is_none() {
                            reply.push_str(&format!(" Answer: {}", answer.get("result").unwrap_or(&Value::Null)));
                        } else {
                            continue;
                        }
                        out(
                            json!({"method":"serverRequest/resolved","params":{"threadId":thread,"requestId":request}}),
                        );
                        break;
                    }
                }
                for (word, kind) in [("tool", "commandExecution"), ("edit", "fileChange")] {
                    if !text.contains(word) {
                        continue;
                    }
                    requests += 1;
                    let item_id = format!("exec-{count}-{word}");
                    let item = if kind == "commandExecution" {
                        json!({"type": kind, "id": item_id, "command": "/bin/zsh -lc 'echo hi'", "status": "inProgress",
                            "commandActions": [{"type": "unknown", "command": "echo hi"}], "aggregatedOutput": null, "exitCode": null})
                    } else {
                        json!({"type": kind, "id": item_id, "status": "inProgress",
                            "changes": [{"path": "notes.txt", "kind": {"type": "add"}, "diff": "one\n"}]})
                    };
                    out(
                        json!({"method": "item/started", "params": {"item": item, "threadId": thread, "turnId": turn}}),
                    );
                    let request = json!(requests);
                    let mut params = json!({"threadId": thread, "turnId": turn, "itemId": item_id, "reason": format!("Wants to {word}")});
                    if kind == "commandExecution" {
                        params["availableDecisions"] = json!(["accept", {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["echo", "hi"]}}, "cancel"]);
                    }
                    out(json!({"method": format!("item/{kind}/requestApproval"), "id": request, "params": params}));
                    let decision = loop {
                        let Ok(answer) = rx.recv() else { std::process::exit(0) };
                        if method(&answer) == "turn/steer" {
                            record_steer(&answer, &thread, &turn);
                            continue;
                        }
                        if method(&answer) == "turn/interrupt" {
                            out(
                                json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": request}}),
                            );
                            out(json!({"id": answer.get("id"), "result": {}}));
                            interrupted = true;
                            break Value::Null;
                        }
                        if answer.get("id") == Some(&request) && answer.get("method").is_none() {
                            break answer.pointer("/result/decision").cloned().unwrap_or(Value::Null);
                        }
                    };
                    if interrupted {
                        break;
                    }
                    let declined = decision == json!("decline");
                    if declined {
                        // A reason comes as a steer right after the decline.
                        if let Ok(steer) = rx.recv_timeout(Duration::from_millis(500))
                            && method(&steer) == "turn/steer"
                        {
                            out(json!({"id": steer.get("id"), "result": {"turnId": turn}}));
                            let said = steer
                                .pointer("/params/input/0/text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            reply.push_str(&format!(" Told: {said}."));
                        }
                    }
                    let mut done = item.clone();
                    done["status"] = json!(if declined { "declined" } else { "completed" });
                    if kind == "commandExecution" && !declined {
                        let remembered = decision.is_object();
                        done["aggregatedOutput"] =
                            json!(format!("hi{}", if remembered { " (remembered)" } else { "" }));
                        done["exitCode"] = json!(0);
                    }
                    out(
                        json!({"method": "item/completed", "params": {"item": done, "threadId": thread, "turnId": turn}}),
                    );
                }
                // A sub-agent's turn: it answers now, and its model call comes after the session's last one, as in the
                // 0.164.0 recordings.
                let mut child_end = None;
                if text.contains("subagent") && !interrupted {
                    let child = format!("{thread}-child-{count}");
                    let child_turn = format!("{turn}-child");
                    let spawn = json!({"type": "subAgentActivity", "id": format!("call-{count}-child"), "kind": "started",
                        "model": "gpt-child", "reasoningEffort": "low", "agentThreadId": child, "agentPath": "/root/helper"});
                    for event in ["item/started", "item/completed"] {
                        out(json!({"method": event, "params": {"item": spawn, "threadId": thread, "turnId": turn}}));
                    }
                    out(
                        json!({"method": "turn/started", "params": {"threadId": child, "turn": {"id": child_turn, "rootTurnId": turn, "status": "inProgress"}}}),
                    );
                    out(
                        json!({"method": "item/completed", "params": {"threadId": child, "turnId": child_turn, "item": {"type": "agentMessage", "id": format!("msg_{count}_child"), "text": "Helped.", "phase": "final_answer"}}}),
                    );
                    child_end = Some((child, child_turn));
                }
                if text.contains("slow") && !interrupted {
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(next) if method(&next) == "turn/steer" => record_steer(&next, &thread, &turn),
                            Ok(next) if method(&next) == "turn/interrupt" => {
                                out(json!({"id": next.get("id"), "result": {}}));
                                interrupted = true;
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            _ => {}
                        }
                    }
                }
                if text.contains("relay") && !interrupted {
                    let relay = memory.as_ref().map(|dir| dir.join("relay"));
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let mut calls = 0;
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Ok(next) if method(&next) == "turn/interrupt" => {
                                out(json!({"id": next.get("id"), "result": {}}));
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
                            out(json!({"method": "thread/tokenUsage/updated", "params": {
                                "threadId": thread, "turnId": turn, "tokenUsage": {"last": {
                                "inputTokens": 0, "cachedInputTokens": 0, "cacheWriteInputTokens": 0, "outputTokens": 50},
                                "modelContextWindow": 258400}}}));
                            continue;
                        }
                        calls += 1;
                        let mut item = json!({"type": "mcpToolCall", "id": format!("mcp-{count}-{calls}"), "server": "nexus_app",
                            "tool": "wait_sessions", "arguments": {}, "status": "inProgress"});
                        out(
                            json!({"method": "item/started", "params": {"item": item, "threadId": thread, "turnId": turn}}),
                        );
                        match body.strip_prefix("ERROR ") {
                            Some(error) => {
                                item["status"] = json!("failed");
                                item["error"] = json!({"message": error});
                            }
                            None => {
                                item["status"] = json!("completed");
                                item["result"] = json!({"content": [{"type": "text", "text": body}]});
                            }
                        }
                        out(
                            json!({"method": "item/completed", "params": {"item": item, "threadId": thread, "turnId": turn}}),
                        );
                    }
                }
                if (text.contains("orchestrate") || mcp::hands_off(&text)) && !interrupted {
                    let mut calls = 0;
                    mcp::act(mcp::Server::codex(&args), "codex", &text, |called| {
                        calls += 1;
                        let mut item = json!({"type": "mcpToolCall", "id": format!("mcp-orch-{count}-{calls}"), "server": "nexus_app",
                            "tool": called.tool, "arguments": called.args, "status": "inProgress"});
                        out(
                            json!({"method": "item/started", "params": {"item": item, "threadId": thread, "turnId": turn}}),
                        );
                        item["status"] = json!("completed");
                        item["result"] =
                            json!({"content": [{"type": "text", "text": called.text}], "isError": called.is_error});
                        out(
                            json!({"method": "item/completed", "params": {"item": item, "threadId": thread, "turnId": turn}}),
                        );
                    });
                }
                let sleeper = text.contains("sleeper") && !interrupted;
                let sleeper_child = format!("{thread}-sleeper-{count}");
                if sleeper {
                    let spawn = json!({"type": "subAgentActivity", "id": format!("call-{count}-sleeper"), "kind": "started",
                        "model": "gpt-child", "reasoningEffort": "low", "agentThreadId": sleeper_child, "agentPath": "/root/sleeper"});
                    for event in ["item/started", "item/completed"] {
                        out(json!({"method": event, "params": {"item": spawn, "threadId": thread, "turnId": turn}}));
                    }
                    if std::env::var_os("FAKE_CODEX_CHILD_NO_TURN").is_none() {
                        out(
                            json!({"method": "turn/started", "params": {"threadId": sleeper_child, "turn": {"id": "sleeper-turn", "rootTurnId": turn, "status": "inProgress"}}}),
                        );
                    }
                }
                if text.contains("latechild") && !interrupted {
                    let child = format!("{thread}-late-{count}");
                    let child_turn = format!("{turn}-late");
                    let call = format!("call-{count}-late");
                    let spawn = json!({"type": "subAgentActivity", "id": call, "kind": "started", "model": "gpt-child",
                        "reasoningEffort": "low", "agentThreadId": child, "agentPath": "/root/late"});
                    for event in ["item/started", "item/completed"] {
                        out(json!({"method": event, "params": {"item": spawn, "threadId": thread, "turnId": turn}}));
                    }
                    out(
                        json!({"method": "turn/started", "params": {"threadId": child, "turn": {"id": child_turn, "rootTurnId": turn, "status": "inProgress"}}}),
                    );
                    out(
                        json!({"method": "item/started", "params": {"threadId": child, "turnId": child_turn, "item": {
                        "type": "commandExecution", "id": "exec-early", "command": "make test", "cwd": "/r", "status": "inProgress"}}}),
                    );
                    out(
                        json!({"method": "item/commandExecution/requestApproval", "id": 901, "params": {"threadId": child,
                        "turnId": child_turn, "itemId": "exec-early", "command": "make test", "cwd": "/r"}}),
                    );
                    late_child = Some((child, child_turn, call));
                }
                let status = if interrupted { "interrupted" } else { "completed" };
                if !interrupted {
                    if let Ok(title) = std::env::var("FAKE_CODEX_TITLE")
                        && let Some(prefix) = text.lines().find(|line| line.starts_with("[nexus-title:"))
                        && let Some((prefix, _)) = prefix.split_once("] ")
                    {
                        reply = format!("{prefix}] {title}\n\n{reply}");
                    }
                    let message_id = format!("msg_{count}");
                    out(
                        json!({"method": "item/started", "params": {"item": {"type": "agentMessage", "id": message_id, "text": ""}, "threadId": thread, "turnId": turn}}),
                    );
                    let (first, rest) = reply.split_at(reply.len() / 2);
                    for chunk in [first, rest] {
                        out(
                            json!({"method": "item/agentMessage/delta", "params": {"threadId": thread, "turnId": turn, "itemId": message_id, "delta": chunk}}),
                        );
                    }
                    out(
                        json!({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": message_id, "text": reply}, "threadId": thread, "turnId": turn}}),
                    );
                    out(
                        json!({"method": "thread/tokenUsage/updated", "params": {"threadId": thread, "turnId": turn, "tokenUsage": {
                        "last": {"inputTokens": 1100, "cachedInputTokens": 1000, "cacheWriteInputTokens": 0, "outputTokens": 50},
                        "modelContextWindow": 258400}}}),
                    );
                }
                if let Some((child, child_turn)) = child_end {
                    out(
                        json!({"method": "thread/tokenUsage/updated", "params": {"threadId": child, "turnId": child_turn, "tokenUsage": {
                        "last": {"inputTokens": 500, "cachedInputTokens": 400, "cacheWriteInputTokens": 0, "outputTokens": 20},
                        "modelContextWindow": 258400}}}),
                    );
                    out(
                        json!({"method": "turn/completed", "params": {"threadId": child, "turn": {"id": child_turn, "status": "completed", "error": null, "durationMs": 5}}}),
                    );
                }
                out(
                    json!({"method": "turn/completed", "params": {"threadId": thread, "turn": {"id": turn, "status": status, "error": null,
                    "durationMs": started.elapsed().as_millis() as u64}}}),
                );
                if text.contains("autoturn") {
                    let dir = memory.clone();
                    let thread = thread.clone();
                    std::thread::spawn(move || {
                        let Some(dir) = dir else { return };
                        let path = dir.join(format!("automatic-{thread}"));
                        let deadline = Instant::now() + Duration::from_secs(30);
                        while !path.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if !path.exists() {
                            return;
                        }
                        out(
                            json!({"method": "turn/started", "params": {"threadId": thread, "turn": {"id": "automatic"}}}),
                        );
                        out(
                            json!({"method": "item/agentMessage/delta", "params": {"threadId": thread, "turnId": "automatic",
                            "itemId": "automatic", "delta": "Automatic"}}),
                        );
                        let interrupted = dir.join(format!("interrupt-{thread}"));
                        let finish = dir.join(format!("finish-automatic-{}", thread));
                        while !interrupted.exists() && !finish.exists() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if !interrupted.exists() && !finish.exists() {
                            return;
                        }
                        out(
                            json!({"method": "item/completed", "params": {"threadId": thread, "turnId": "automatic",
                            "item": {"type": "agentMessage", "id": "automatic", "text": "An automatic turn."}}}),
                        );
                        out(json!({"method": "turn/completed", "params": {"threadId": thread,
                            "turn": {"id": "automatic", "status": if interrupted.exists() { "interrupted" } else { "completed" }, "error": null}}}));
                    });
                }
                // A deterministic unsolicited turn, after a background report, for linked halt tests.
                if text.contains("bgagent") {
                    let root_thread = thread.clone();
                    let dir = memory.clone();
                    std::thread::spawn(move || {
                        let Some(dir) = dir else { return };
                        for name in ["bgagent-end", "bgagent-turn"] {
                            let path = dir.join(name);
                            let deadline = Instant::now() + Duration::from_secs(30);
                            while !path.exists() && Instant::now() < deadline {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            let _ = std::fs::remove_file(path);
                        }
                        out(json!({"method": "turn/started", "params": {"threadId": root_thread,
                            "turn": {"id": "unsolicited", "status": "inProgress"}}}));
                        out(
                            json!({"method": "item/completed", "params": {"threadId": root_thread, "turnId": "unsolicited",
                            "item": {"type": "agentMessage", "id": "unsolicited-message", "text": "The survey is in."}}}),
                        );
                        out(json!({"method": "turn/completed", "params": {"threadId": root_thread,
                            "turn": {"id": "unsolicited", "status": "completed", "error": null}}}));
                    });
                }
                // `sleeper`: the child writes after the turn, then ends on its own.
                if sleeper {
                    out(
                        json!({"method": "item/completed", "params": {"threadId": sleeper_child, "turnId": "sleeper-turn", "item": {
                        "type": "agentMessage", "id": format!("msg_{count}_sleeper"), "text": "Sleeping", "phase": "commentary"}}}),
                    );
                    let ms = std::env::var("FAKE_CODEX_CHILD_MS")
                        .ok()
                        .and_then(|ms| ms.parse().ok())
                        .unwrap_or(600);
                    // It waits on its input meanwhile: once the app closes it (a stop), it exits, child and all.
                    let deadline = Instant::now() + Duration::from_millis(ms);
                    let mut status = "completed";
                    while Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Err(mpsc::RecvTimeoutError::Disconnected) => std::process::exit(0),
                            Ok(stop)
                                if method(&stop) == "turn/interrupt"
                                    && stop.pointer("/params/threadId") == Some(&json!(sleeper_child)) =>
                            {
                                out(json!({"id": stop.get("id"), "result": {}}));
                                status = "interrupted";
                                break;
                            }
                            _ => {}
                        }
                    }
                    out(
                        json!({"method": "turn/completed", "params": {"threadId": sleeper_child, "turn": {"id": "sleeper-turn", "status": status,
                        "error": null, "durationMs": ms}}}),
                    );
                }
                // `latechild`: the child goes on after the turn and asks to run a command.
                if let Some((child, child_turn, _)) = &late_child {
                    out(
                        json!({"method": "turn/plan/updated", "params": {"threadId": child, "turnId": child_turn, "explanation": null,
                        "plan": [{"step": "Build", "status": "inProgress"}]}}),
                    );
                    out(
                        json!({"method": "item/completed", "params": {"threadId": child, "turnId": child_turn, "item": {
                        "type": "agentMessage", "id": format!("msg_{count}_late"), "text": "Child at work", "phase": "commentary"}}}),
                    );
                    out(
                        json!({"method": "item/started", "params": {"threadId": child, "turnId": child_turn, "item": {
                        "type": "commandExecution", "id": "exec-late", "command": "make", "cwd": "/r", "status": "inProgress"}}}),
                    );
                    out(
                        json!({"method": "item/commandExecution/requestApproval", "id": 900, "params": {"threadId": child,
                        "turnId": child_turn, "itemId": "exec-late", "command": "make", "cwd": "/r"}}),
                    );
                }
            }
            _ => {}
        }
    }
}
