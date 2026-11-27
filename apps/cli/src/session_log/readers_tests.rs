use super::*;

fn read(reader: &mut dyn Reader, lines: &[Value]) -> Logged {
    let mut out = Vec::new();
    for line in lines {
        reader.line(line, 0, &mut out);
    }
    reader.settle(&mut out);
    out
}

fn kinds(logged: &Logged) -> Vec<String> {
    logged
        .iter()
        .map(|(_, event)| {
            serde_json::to_value(event).expect("event")["kind"]
                .as_str()
                .expect("kind")
                .to_string()
        })
        .collect()
}

fn prices() -> Prices {
    Prices::from([
        ("claude-opus-5-5".to_string(), (5.0, 25.0)),
        ("gpt-6".to_string(), (2.0, 8.0)),
    ])
}

fn assistant(uuid: &str, id: &str, block: Value, stop: &str) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "timestamp": "2026-11-05T10:00:01.000Z",
        "message": {
            "id": id, "model": "claude-opus-5-5", "content": [block], "stop_reason": stop,
            "usage": { "input_tokens": 10, "output_tokens": 20, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0 },
        },
    })
}

fn user(uuid: &str, content: Value) -> Value {
    json!({ "type": "user", "uuid": uuid, "timestamp": "2026-11-05T10:00:00.000Z", "message": { "role": "user", "content": content } })
}

#[test]
fn a_claude_transcript_reads_as_turns_with_usage_counted_once_per_message() {
    let lines = [
        json!({ "type": "summary", "summary": "kept out" }),
        user("u1", json!("Fix the flaky test")),
        assistant(
            "a1",
            "msg_1",
            json!({ "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "cargo test" } }),
            "tool_use",
        ),
        user(
            "u2",
            json!([{ "type": "tool_result", "tool_use_id": "toolu_1", "content": [{ "type": "text", "text": "ok" }] }]),
        ),
        assistant(
            "a2",
            "msg_2",
            json!({ "type": "thinking", "thinking": "It passes." }),
            "end_turn",
        ),
        assistant("a3", "msg_2", json!({ "type": "text", "text": "Done." }), "end_turn"),
        user("u3", json!("<command-name>/clear</command-name>")),
        json!({ "type": "user", "isMeta": true, "message": { "content": "a reminder" } }),
        json!({ "type": "assistant", "isSidechain": true, "message": { "id": "msg_9", "content": [{ "type": "text", "text": "a sub-agent" }] } }),
        user("u4", json!("again")),
        user(
            "u5",
            json!([{ "type": "text", "text": "[Request interrupted by user]" }]),
        ),
    ];
    let logged = read(&mut Claude::new(prices()), &lines);
    assert_eq!(
        kinds(&logged),
        [
            "user",
            "turnStart",
            "tool",
            "usage",
            "toolResult",
            "thinking",
            "usage",
            "text",
            "turnEnd",
            "user",
            "turnStart",
            "turnEnd"
        ]
    );
    let events: Vec<Value> = logged
        .iter()
        .map(|(_, event)| serde_json::to_value(event).expect("event"))
        .collect();
    assert_eq!(events[0]["text"], "Fix the flaky test");
    assert_eq!(events[2]["input"]["command"], "cargo test");
    assert_eq!(events[3]["cacheReadTokens"], 1000);
    // 10 + 1000 × 0.1 input tokens at $5, 20 output at $25, per million.
    let call = (110.0 * 5.0 + 20.0 * 25.0) / 1e6;
    assert!((events[3]["costUsd"].as_f64().expect("cost") - call).abs() < 1e-12);
    assert_eq!(events[4]["output"], "ok");
    assert_eq!(events[8]["status"], "completed");
    assert_eq!(
        events[8]["cursor"], "a3",
        "a revert resumes after the answer's last line"
    );
    assert!((events[8]["costUsd"].as_f64().expect("cost") - 2.0 * call).abs() < 1e-12);
    assert_eq!(events[9]["text"], "again");
    assert_eq!(events[11]["status"], "interrupted");
    assert_eq!(logged[0].0, 1_793_872_800_000, "each event keeps its line's time");
}

#[test]
fn an_open_turn_ends_when_the_agent_exits() {
    let mut reader = Claude::new(Prices::new());
    let mut out = Vec::new();
    reader.line(&user("u1", json!("Plan the release")), 0, &mut out);
    reader.finish(5, TurnStatus::Error, &mut out);
    assert_eq!(kinds(&out), ["user", "turnStart", "turnEnd"]);
    assert_eq!(serde_json::to_value(&out[2].1).expect("event")["status"], "error");
    let mut idle = Vec::new();
    reader.finish(6, TurnStatus::Interrupted, &mut idle);
    assert!(idle.is_empty(), "no turn, nothing to end");
}

#[test]
fn long_text_and_large_inputs_are_cut_to_sync() {
    let huge = "x".repeat(200 * 1024);
    let lines = [
        user("u1", json!(huge)),
        assistant(
            "a1",
            "msg_1",
            json!({ "type": "tool_use", "id": "toolu_1", "name": "Write", "input": { "content": huge } }),
            "tool_use",
        ),
        user(
            "u2",
            json!([{ "type": "tool_result", "tool_use_id": "toolu_1", "content": huge }]),
        ),
    ];
    let logged = read(&mut Claude::new(Prices::new()), &lines);
    for (_, event) in &logged {
        assert!(serde_json::to_string(event).expect("json").len() < 70 * 1024);
    }
    let result = serde_json::to_value(&logged[4].1).expect("event");
    assert_eq!(result["truncated"], true);
}

#[test]
fn a_codex_rollout_reads_as_turns() {
    let at = "2026-11-05T10:00:00.000Z";
    let lines = [
        json!({ "timestamp": at, "type": "session_meta", "payload": { "id": "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d", "cwd": "/w" } }),
        json!({ "timestamp": at, "type": "turn_context", "payload": { "model": "gpt-6.1", "cwd": "/w" } }),
        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "<environment_context>" }] } }),
        json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "user_message", "message": "List the files" } }),
        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "Use ls." }] } }),
        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "function_call", "name": "shell", "arguments": "{\"command\":[\"ls\"]}", "call_id": "call_1" } }),
        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "function_call_output", "call_id": "call_1", "output": "{\"output\":\"a.rs\"}" } }),
        json!({ "timestamp": at, "type": "response_item", "payload": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "One file: a.rs" }] } }),
        json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "agent_message", "message": "One file: a.rs" } }),
        json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "token_count", "info": { "last_token_usage": { "input_tokens": 1200, "cached_input_tokens": 1000, "output_tokens": 50 } } } }),
        json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "task_complete", "turn_id": "turn-7" } }),
    ];
    let logged = read(&mut Codex::new(prices(), None), &lines);
    assert_eq!(
        kinds(&logged),
        [
            "user",
            "turnStart",
            "thinking",
            "tool",
            "toolResult",
            "text",
            "usage",
            "turnEnd"
        ]
    );
    let events: Vec<Value> = logged
        .iter()
        .map(|(_, event)| serde_json::to_value(event).expect("event"))
        .collect();
    assert_eq!(events[3]["input"]["command"][0], "ls");
    assert_eq!(events[6]["model"], "gpt-6.1");
    assert_eq!(events[6]["inputTokens"], 200, "cached input counts apart");
    assert_eq!(events[6]["cacheReadTokens"], 1000);
    assert_eq!(events[7]["cursor"], "turn-7");
    let call = ((200.0 + 1000.0 * 0.1) * 2.0 + 50.0 * 8.0) / 1e6;
    assert!(
        (events[7]["costUsd"].as_f64().expect("cost") - call).abs() < 1e-12,
        "{}",
        events[7]
    );
}

#[test]
fn prices_match_the_catalogs_ids() {
    let prices = prices();
    assert!(price(&prices, "anthropic/claude-opus-5.5", [1_000_000, 0, 0, 0]) - 5.0 < 1e-9);
    assert!(price(&prices, "claude-opus-5-5-20261101", [0, 1_000_000, 0, 0]) - 25.0 < 1e-9);
    assert_eq!(price(&prices, "unknown", [1, 1, 1, 1]), 0.0);
}
