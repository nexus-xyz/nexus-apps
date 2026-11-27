//! A stand-in for `agy acp`, for the workbench's tests: the same ACP lines in and out (acp.rs), with no model. Each
//! prompt is answered with `You said: <prompt>`. A prompt with the word `run` first asks to run `ls`
//! (`session/request_permission`), in every mode, as agy asks before a command, and runs it when allowed.
//!
//! Its `session/new` answer offers the modes `default`, `accept-edits`, `auto` and `plan`. With `FAKE_AGY_NO_AUTO`
//! set, it offers no `auto`, and refuses it as agy 1.5.10 refuses a Gemini Enterprise account. With
//! `FAKE_AGY_REFUSE_AUTO` set, it lists `auto` and still refuses it, as agy 1.5.10 also did there. Each mode it takes is
//! a line of `FAKE_AGY_DIR/modes`, each start's arguments a JSON line of `FAKE_AGY_DIR/args`, and each conversation
//! it loads a line of `FAKE_AGY_DIR/loads`, and each start's `OPENCODE_DISABLE_PROJECT_CONFIG` a line of
//! `FAKE_AGY_DIR/project_config_off`. With `FAKE_AGY_SIGNED_OUT` set, it answers `session/new`, `session/load`
//! and `authenticate` with ACP's auth-required error, as agy does with no Google sign-in.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The JSON-RPC id of the one approval it asks for at a time.
const ASK: &str = "agy-ask";

fn main() {
    let cursor_dir = std::env::var("FAKE_CURSOR_DIR").ok();
    let cursor = cursor_dir.is_some();
    let folder = std::path::PathBuf::from(
        cursor_dir
            .or_else(|| std::env::var("FAKE_AGY_DIR").ok())
            .unwrap_or_else(|| ".".into()),
    );
    let append = |name: &str, line: &str| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join(name))
            .unwrap_or_else(|e| panic!("{e}"));
        let _ = writeln!(file, "{line}");
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    append("args", &json!(args).to_string());
    // OpenCode's limit on the project's own config (`claude_trust.rs`): its value, or an empty line when unset.
    append(
        "project_config_off",
        &std::env::var("OPENCODE_DISABLE_PROJECT_CONFIG").unwrap_or_default(),
    );
    let auto = std::env::var_os("FAKE_AGY_NO_AUTO").is_none();
    let takes_auto = auto && std::env::var_os("FAKE_AGY_REFUSE_AUTO").is_none();
    let signed_out = std::env::var_os("FAKE_AGY_SIGNED_OUT").is_some();
    let mut modes = vec!["default", "accept-edits", "plan"];
    if auto {
        modes.insert(2, "auto");
    }
    if cursor {
        modes = vec!["agent", "plan", "ask"];
    }
    let opens_in = modes[0];
    let mut out = std::io::stdout();
    let mut say = |value: Value| {
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    };
    let update =
        |update: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "update": update } });
    // The prompt waiting for the person's answer: its id and its text.
    let mut waiting: Option<(Value, String)> = None;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let params = &message["params"];
        let answer = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let end = |id: &Value, text: &str, stop: &str| {
            if cursor {
                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": stop } })
            } else {
                json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": stop, "usage": { "inputTokens": 10, "outputTokens": text.len() } } })
            }
        };
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => say(answer(
                json!({ "protocolVersion": 1, "agentInfo": { "name": "fake agy", "version": "0.0.0-fake" } }),
            )),
            Some("session/new" | "session/load" | "authenticate") if signed_out => say(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": "Authentication required" } }),
            ),
            Some(method @ ("session/new" | "session/load")) => {
                let session = params["sessionId"].as_str().unwrap_or("fake-agy-session");
                if method == "session/load" {
                    append("loads", session);
                }
                let available: Vec<Value> = modes.iter().map(|mode| json!({ "id": mode, "name": mode })).collect();
                let options: Vec<Value> = modes
                    .iter()
                    .map(|mode| json!({ "value": mode, "name": mode }))
                    .collect();
                say(answer(json!({
                    "sessionId": session,
                    "modes": { "currentModeId": opens_in, "availableModes": available },
                    "configOptions": [
                        { "id": "model", "type": "select", "currentValue": "fake-gemini", "options": [{ "value": "fake-gemini" }] },
                        { "id": "mode", "type": "select", "currentValue": opens_in, "options": options },
                    ],
                })));
            }
            Some("session/set_mode") => {
                let mode = params["modeId"].as_str().unwrap_or_default();
                if cursor && mode == "plan" && std::env::var_os("FAKE_CURSOR_REFUSE_PLAN").is_some() {
                    say(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "Plan refused by stand-in" } }),
                    );
                } else if modes.contains(&mode) && (mode != "auto" || takes_auto) {
                    append("modes", mode);
                    say(answer(json!({})));
                } else {
                    say(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602,
                        "message": "Auto mode is not available: it needs the OS terminal sandbox and may not be rolled out to you yet" } }));
                }
            }
            Some("session/set_config_option") => say(answer(json!({}))),
            Some("session/prompt") => {
                if cursor {
                    append("prompts", &params["prompt"].to_string());
                }
                let text = params["prompt"]
                    .as_array()
                    .and_then(|prompt| prompt.iter().rev().find_map(|block| block["text"].as_str()))
                    .and_then(|text| text.lines().next())
                    .unwrap_or_default()
                    .to_string();
                if text.split_whitespace().any(|word| word == "run") {
                    let call = json!({ "toolCallId": "call-ls", "kind": "execute", "title": "ls", "status": "pending", "rawInput": { "command": "ls" } });
                    say(
                        json!({ "jsonrpc": "2.0", "id": ASK, "method": "session/request_permission", "params": {
                            "toolCall": call,
                            "options": [{ "optionId": "allow", "kind": "allow_once" }, { "optionId": "reject", "kind": "reject_once" }],
                        }}),
                    );
                    waiting = Some((id, text));
                } else {
                    say(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": format!("You said: {text}") } }),
                    ));
                    say(end(&id, &text, "end_turn"));
                }
            }
            Some("session/cancel") => {
                if let Some((prompt, text)) = waiting.take() {
                    say(end(&prompt, &text, "cancelled"));
                }
            }
            Some(_) if !id.is_null() => {
                say(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "not offered" } }))
            }
            Some(_) => {}
            // The person's answer to the approval.
            None if id == ASK => {
                let Some((prompt, text)) = waiting.take() else {
                    continue;
                };
                let allowed = message.pointer("/result/outcome/optionId") == Some(&json!("allow"));
                let status = if allowed { "completed" } else { "failed" };
                say(update(
                    json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-ls", "status": status, "rawOutput": "notes.txt" }),
                ));
                say(update(
                    json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": format!("You said: {text}") } }),
                ));
                say(end(&prompt, &text, "end_turn"));
            }
            None => {}
        }
    }
}
