use super::*;
use crate::harness::cursor::ACP_PROFILE;

/// Cursor's protocol, which the recorded `agent acp` sessions drive.
fn cursor(start: &Start) -> Driver {
    Driver::new(start, ACP_PROFILE)
}

/// Replays a recorded Cursor session: each prompt goes as the one before it ends, every approval is allowed, and
/// with `stop_last` the last prompt is stopped as soon as it is sent, as the recording did.
fn replay(acp: &mut Driver, fixture: &str, prompts: &[&str], stop_last: bool) -> Vec<Out> {
    let mut next = 0;
    let mut out = acp.open();
    for line in fixture.lines() {
        let read = acp.read(line);
        let mut said = Vec::new();
        for event in &read {
            match event {
                Out::Init { .. } | Out::Result { .. } if next < prompts.len() => {
                    said.push(Say::User {
                        text: prompts[next].into(),
                        images: Vec::new(),
                    });
                    next += 1;
                    if stop_last && next == prompts.len() {
                        said.push(Say::Interrupt { request: "stop".into() });
                    }
                }
                Out::Event(WbEvent::Approval { request_id, .. }) => said.push(Say::Allow {
                    request_id: request_id.clone(),
                    input: json!({}),
                    answers: None,
                    remember: None,
                }),
                _ => {}
            }
        }
        out.extend(read);
        for say in said {
            out.extend(acp.say(say));
        }
    }
    out
}

fn tools(out: &[Out]) -> Vec<(String, Value)> {
    out.iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Tool { name, input, .. }) => Some((name.clone(), input.clone())),
            _ => None,
        })
        .collect()
}

fn tool_outputs(out: &[Out]) -> Vec<String> {
    out.iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::ToolResult { output, .. }) => Some(output.clone()),
            _ => None,
        })
        .collect()
}

fn texts(out: &[Out]) -> Vec<String> {
    out.iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Text { text, .. }) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Cursor's recorded turns: a reply, a new file it writes without asking, a command it asks about, and a count
/// stopped as it starts. It echoes no message, so each is marked read as its turn's first update arrives; its
/// answers carry no tokens, so no call is counted.
#[test]
fn cursor_recorded_turns_write_ask_run_and_stop() {
    let prompts = [
        "Reply with the single word: ready.",
        "Create a file named hello.txt in the current folder containing the text hi. Do nothing else.",
        "Run the shell command: git status --short. Then tell me its output in one line.",
        "Count slowly from 1 to 200, one number per line, explaining each number in a sentence.",
    ];
    let mut acp = cursor(&Start {
        mode: PermissionMode::AcceptEdits,
        ..start()
    });
    let out = replay(
        &mut acp,
        include_str!("../../../tests/fixtures/cursor-2026.11.17-acp-turns.jsonl"),
        &prompts,
        true,
    );

    // Accept edits is Cursor's `agent`, the mode it opened in: no mode is set.
    assert_eq!(
        methods(&out),
        [
            "initialize",
            "session/new",
            "session/prompt",
            "session/prompt",
            "session/prompt",
            "answer 0",
            "session/prompt",
            "session/cancel",
        ]
    );
    let initialize = &sent(&out)[0];
    assert_eq!(
        initialize["params"]["clientCapabilities"],
        json!({ "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false })
    );
    let taken: Vec<Vec<String>> = out
        .iter()
        .filter_map(|out| match out {
            Out::Taken { texts } => Some(texts.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(taken, prompts.map(|prompt| vec![prompt.to_string()]));
    assert_eq!(
        tools(&out),
        [
            (
                "Write".to_string(),
                json!({ "file_path": "/work/repo/hello.txt", "content": "hi" })
            ),
            ("Bash".to_string(), json!({ "command": "git status --short" })),
        ]
    );
    assert_eq!(tool_outputs(&out)[1], "?? README.md\n?? hello.txt\n");
    let approvals: Vec<(String, Vec<String>)> = out
        .iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Approval { tool, suggestions, .. }) => {
                Some((tool.clone(), suggestions.iter().map(|s| s.label.clone()).collect()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(approvals, [("Bash".to_string(), vec!["Allow always".to_string()])]);
    let allowed = sent(&out)
        .into_iter()
        .find(|message| message["id"] == 0)
        .unwrap_or_default();
    assert_eq!(allowed["result"]["outcome"]["optionId"], "allow-once");
    assert_eq!(texts(&out)[0], "ready");
    assert!(!out.iter().any(|out| matches!(out, Out::Call { .. })), "{out:?}");
    let results: Vec<bool> = out
        .iter()
        .filter_map(|out| match out {
            Out::Result { interrupted, error, .. } => Some((*interrupted, error.clone())),
            _ => None,
        })
        .map(|(interrupted, error)| {
            assert_eq!(error, None);
            interrupted
        })
        .collect();
    assert_eq!(results, [false, false, false, true]);
}

/// An edit of a file Cursor read first: the read shows the file, the edit its real change.
#[test]
fn cursor_recorded_edit_shows_the_read_and_the_change() {
    let mut acp = cursor(&Start {
        mode: PermissionMode::AcceptEdits,
        ..start()
    });
    let out = replay(
        &mut acp,
        include_str!("../../../tests/fixtures/cursor-2026.11.17-acp-edit.jsonl"),
        &["In lines.txt, change the line two to TWO. Change nothing else."],
        false,
    );
    assert_eq!(
        tools(&out),
        [
            ("Read".to_string(), json!({ "file_path": "/work/repo/lines.txt" })),
            (
                "Edit".to_string(),
                json!({ "file_path": "/work/repo/lines.txt", "old_string": "one\ntwo\nthree\n", "new_string": "one\nTWO\nthree\n" })
            ),
        ]
    );
    assert_eq!(tool_outputs(&out)[0], "one\ntwo\nthree\n");
}

/// A load replays the conversation, which the session's log already holds, then answers without the session's id.
#[test]
fn cursor_recorded_load_drops_the_replay_and_keeps_the_id() {
    let id = "d0ce249e-1085-4373-a06f-d01d607bcb69";
    let mut acp = cursor(&Start {
        mode: PermissionMode::AcceptEdits,
        native_id: id,
        resume: true,
        ..start()
    });
    let out = replay(
        &mut acp,
        include_str!("../../../tests/fixtures/cursor-2026.11.17-acp-load.jsonl"),
        &["What file did you create earlier? Answer in one short sentence."],
        false,
    );
    assert_eq!(methods(&out)[..3], ["initialize", "session/load", "session/prompt"]);
    assert_eq!(sent(&out)[1]["params"]["sessionId"], id);
    assert!(
        out.iter()
            .any(|out| matches!(out, Out::Init { native_id, .. } if native_id == id)),
        "{out:?}"
    );
    assert_eq!(texts(&out), ["I created `hello.txt`."]);
}

fn open_cursor(resume: bool, observed: &str) -> (Driver, Vec<Out>) {
    let mut acp = cursor(&Start {
        mode: PermissionMode::Plan,
        resume,
        ..start()
    });
    acp.open();
    let open = acp.read(&reply(&json!(1), None));
    let message = sent(&open).pop().expect("open request");
    assert_eq!(message["method"], if resume { "session/load" } else { "session/new" });
    assert!(
        acp.say(Say::User {
            text: "Make a plan".into(),
            images: Vec::new()
        })
        .is_empty()
    );
    let result = json!({
        "jsonrpc": "2.0", "id": message["id"],
        "result": { "sessionId": "session", "modes": { "currentModeId": observed } },
    });
    let after = acp.read(&result.to_string());
    (acp, after)
}

#[test]
fn cursor_confirmed_plan_releases_the_first_prompt() {
    for resume in [false, true] {
        let (mut acp, out) = open_cursor(resume, "agent");
        assert_eq!(methods(&out), ["session/set_mode"]);
        let mode = sent(&out).pop().expect("set mode request");
        assert_eq!(mode["params"]["modeId"], "plan");
        let prompt = acp.read(&reply(&mode["id"], None));
        assert_eq!(methods(&prompt), ["session/prompt"]);
    }
}

#[test]
fn cursor_refused_plan_discards_the_first_prompt_on_open_and_resume() {
    for resume in [false, true] {
        let (mut acp, out) = open_cursor(resume, "agent");
        assert_eq!(methods(&out), ["session/set_mode"]);
        let set = sent(&out).pop().expect("set mode request");
        let refusal = acp.read(&reply(&set["id"], Some("Plan unavailable")));
        assert!(matches!(
            refusal.as_slice(),
            [Out::Event(WbEvent::Error { text, .. }), Out::Close] if text.contains("Plan unavailable")
        ));
        assert!(acp.read(&reply(&set["id"], None)).is_empty());
        assert!(
            acp.say(Say::User {
                text: "Still do not run".into(),
                images: Vec::new()
            })
            .is_empty()
        );
    }
}

#[test]
fn cursor_refuses_an_unrecognized_opening_mode() {
    for resume in [false, true] {
        let (_, out) = open_cursor(resume, "unknown");
        assert!(matches!(
            out.as_slice(),
            [Out::Event(WbEvent::Error { .. }), Out::Close]
        ));
    }
}

#[test]
fn cursor_rejects_unsupported_mode_changes() {
    let (mut acp, _) = open_cursor(false, "plan");
    let out = acp.say(Say::Mode {
        request: "bad-mode".into(),
        mode: PermissionMode::Auto,
    });
    assert!(matches!(out.as_slice(), [Out::Ack { error: Some(_), .. }]));
}

#[test]
fn cursor_requires_a_successful_mode_response_on_open_and_resume() {
    for resume in [false, true] {
        for result in [None, Some(Value::Null)] {
            let (mut acp, out) = open_cursor(resume, "agent");
            let set = sent(&out).pop().expect("set mode request");
            let mut response = json!({ "jsonrpc": "2.0", "id": set["id"] });
            if let Some(result) = result {
                response["result"] = result;
            }
            assert!(matches!(
                acp.read(&response.to_string()).as_slice(),
                [Out::Event(WbEvent::Error { .. }), Out::Close]
            ));
        }
    }
}

#[test]
fn cursor_does_not_release_a_plan_after_a_contradictory_mode_update() {
    let (mut acp, out) = open_cursor(false, "agent");
    let set = sent(&out).pop().expect("set mode request");
    let mode = json!({
        "jsonrpc": "2.0", "method": "session/update",
        "params": { "update": { "sessionUpdate": "current_mode_update", "currentModeId": "agent" } },
    });
    assert!(acp.read(&mode.to_string()).is_empty());
    let refused = acp.read(&reply(&set["id"], None));
    assert!(matches!(
        refused.as_slice(),
        [Out::Event(WbEvent::Error { .. }), Out::Close]
    ));
}
