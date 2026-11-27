//! A stand-in for `nexus __agent`, for the workbench's tests: the same JSON lines in and out, with no model. Each prompt
//! is answered with `You said: <prompt>`, and costs $0.001; words in it add to that:
//!
//! - `edit`: asks to edit `notes.txt` (an `approve` line), then writes it when allowed.
//! - `tool`: asks to run `echo hi`, then reports it ran.
//! - `delegate`: a subagent (`agent-1`) starts, reads `notes.txt`, and reports back.
//! - `orchestrate`: starts a child through the app's `nexus_app` server (its `--mcp-config`, as Claude Code's) and
//!   waits for it, as `fake-claude` does; each call is a `call` tool line.
//!
//! With `approvals` at `auto`, nothing asks; at `auto-edit`, only `tool` does. With `FAKE_NEXUS_ASK` set, each one asks
//! at every level. `FAKE_LAUNCH_RECORD`: a file it writes its arguments and its environment to as it starts, as JSON.
//!
//! It remembers as the real agent does: `--session <id>` keeps the conversation (two messages a turn) in
//! `FAKE_NEXUS_DIR/<id>.json`, `--fork` goes on from a copy under a new id, and `--rewind <n>` keeps the first `n`
//! messages. An answer names the earlier prompts it remembers (`(remembers: a | b)`).

use std::io::{BufRead, Write};

use serde_json::{Value, json};

#[path = "fake/mcp.rs"]
mod mcp;

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(path) = std::env::var("FAKE_LAUNCH_RECORD") {
        let env: serde_json::Map<String, Value> = recorded_env();
        let _ = std::fs::write(path, json!({ "args": args, "env": env }).to_string());
    }
    let value = |flag: &str| args.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].clone());
    let folder = std::path::PathBuf::from(std::env::var("FAKE_NEXUS_DIR").unwrap_or_else(|_| ".".into()));
    let file = |id: &str| folder.join(format!("{id}.json"));
    let mut session = value("--session");
    let mut history: Vec<String> = session
        .as_deref()
        .and_then(|id| std::fs::read_to_string(file(id)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if args.iter().any(|arg| arg == "--fork") {
        session = Some(format!("fork-{}", std::process::id()));
    }
    if let Some(keep) = value("--rewind").and_then(|n| n.parse().ok()) {
        history.truncate(keep);
    }
    let mut out = std::io::stdout();
    let mut say = |value: Value| {
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    };
    say(
        json!({ "type": "ready", "version": "0.0.0-fake", "workspace": ".", "session": session, "messages": history.len() }),
    );
    let (mut auto_edits, mut auto_commands) = (false, false);
    let always_ask = std::env::var_os("FAKE_NEXUS_ASK").is_some();
    let mut turn = 0;
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let Ok(command) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match command["type"].as_str() {
            Some("approvals") => {
                (auto_edits, auto_commands) = match command["level"].as_str() {
                    Some("auto") => (true, true),
                    Some("auto-edit") => (true, false),
                    _ => (false, false),
                };
            }
            Some("prompt") => {
                turn += 1;
                // The first line: the app adds its own instructions below a session's first message.
                // The first line: the app adds its own instructions below a session's first message.
                let text = command["text"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string();
                say(json!({ "type": "started", "turn": turn }));
                // Asks when it must, and reads the answer: allowed, unless denied.
                let mut allowed = |ask: Value, auto: bool, say: &mut dyn FnMut(Value)| -> bool {
                    if auto && !always_ask {
                        return true;
                    }
                    say(json!({ "type": "approve", "turn": turn, "ask": ask }));
                    while let Some(Ok(line)) = lines.next() {
                        let answer: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        if answer["type"] == "decide" {
                            return answer["decision"] != "deny";
                        }
                    }
                    false
                };
                if text.contains("edit") {
                    let diff = ["@@ -0,0 +1 @@", "+hello"];
                    let ask = json!({ "kind": "edit", "path": "notes.txt", "diff": diff });
                    if allowed(ask, auto_edits, &mut say) {
                        let _ = std::fs::write("notes.txt", "hello\n");
                        say(json!({ "type": "tool", "turn": turn, "tool": {
                            "kind": "create", "path": "notes.txt", "added": 1, "removed": 0, "diff": diff } }));
                    } else {
                        say(json!({ "type": "tool", "turn": turn, "tool": {
                            "kind": "failed", "what": "edit notes.txt", "error": "declined" } }));
                    }
                }
                if text.contains("tool") {
                    let ask = json!({ "kind": "run", "command": "echo hi" });
                    if allowed(ask, auto_commands, &mut say) {
                        say(json!({ "type": "tool", "turn": turn, "tool": {
                            "kind": "run", "command": "echo hi", "exit": 0, "output": "hi\n" } }));
                    } else {
                        say(json!({ "type": "tool", "turn": turn, "tool": {
                            "kind": "failed", "what": "run echo hi", "error": "declined" } }));
                    }
                }
                if text.contains("delegate") {
                    say(
                        json!({ "type": "task", "turn": turn, "task": { "id": "agent-1", "state": "started",
                        "description": "Read the notes", "prompt": "Read notes.txt and report." } }),
                    );
                    say(json!({ "type": "tool", "turn": turn, "parent": "agent-1",
                        "tool": { "kind": "read", "path": "notes.txt", "lines": 1 } }));
                    say(
                        json!({ "type": "task", "turn": turn, "task": { "id": "agent-1", "state": "completed",
                        "result": "The notes say hello." } }),
                    );
                }
                if text.contains("orchestrate") {
                    mcp::act(mcp::Server::claude(&args), "nexus", &text, |called| {
                        say(json!({ "type": "tool", "turn": turn, "tool": { "kind": "call",
                            "name": format!("mcp__nexus_app__{}", called.tool), "input": called.args,
                            "output": called.text, "error": called.is_error } }));
                    });
                }
                let earlier: Vec<&str> = history.iter().step_by(2).map(String::as_str).collect();
                let answer = if earlier.is_empty() {
                    format!("You said: {text}")
                } else {
                    format!("You said: {text} (remembers: {})", earlier.join(" | "))
                };
                say(json!({ "type": "delta", "turn": turn, "text": answer }));
                history.extend([text, answer]);
                if let Some(id) = &session {
                    let _ = std::fs::create_dir_all(&folder);
                    let _ = std::fs::write(file(id), json!(history).to_string());
                }
                say(json!({ "type": "spent", "turn": turn, "cost_usd": 0.001 }));
                say(
                    json!({ "type": "done", "turn": turn, "model": command["model"], "cost_usd": 0.001, "steps": 1,
                    "stopped": false, "messages": history.len() }),
                );
            }
            _ => {}
        }
    }
}
