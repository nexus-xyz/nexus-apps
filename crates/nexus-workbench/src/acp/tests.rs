use super::*;
use crate::harness::{
    Harness,
    antigravity::{ACCOUNT_POLICY_NOTICE, ACP_PROFILE, OWN_TURN_QUIET, args},
};
use nexus_agents::Antigravity;

fn antigravity(start: &Start) -> Driver {
    Driver::new(start, ACP_PROFILE)
}

const SESSION: &str = "32916f82-7bdf-4284-b729-407fca3e67bb";

fn start() -> Start<'static> {
    Start {
        mode: PermissionMode::Default,
        model: None,
        effort: None,
        fast: false,
        native_id: "wb-thread-1",
        resume: false,
        fork: false,
        cwd: "/work/repo",
        rewind: None,
        rewind_required: false,
        instructions: None,
        prompt: None,
        name: None,
    }
}

fn sent(out: &[Out]) -> Vec<Value> {
    out.iter()
        .filter_map(|out| match out {
            Out::Reply(line) => serde_json::from_str(line).ok(),
            _ => None,
        })
        .collect()
}

fn methods(out: &[Out]) -> Vec<String> {
    sent(out)
        .iter()
        .map(|message| match message.get("method").and_then(Value::as_str) {
            Some(method) => method.to_string(),
            None => format!("answer {}", message["id"]),
        })
        .collect()
}

/// Opens a conversation as agy 1.5.8 answers: its `initialize` and `session/new` answers.
fn opened(acp: &mut Driver) -> Vec<Out> {
    let mut out = acp.open();
    for line in include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
        .lines()
        .take(2)
    {
        out.extend(acp.read(line));
    }
    out
}

/// The servers the host launched with go in `session/new`, and again in `session/load`: agy starts a conversation's
/// servers each time it opens it. A new conversation says once which remote servers agy cannot take.
#[test]
fn launched_servers_go_in_every_open_and_skipped_ones_are_named_once() {
    let mcp = || SessionMcp {
        servers: vec![
            json!({"name": "nexus", "command": "/app", "args": ["--mcp-launch=nexus"], "env": [
                {"name": "NEXUS_MCP_APP_ADDR", "value": "127.0.0.1:4123"},
                {"name": "NEXUS_MCP_APP_TOKEN", "value": "launch_tok"},
            ]}),
        ],
        skipped: vec!["docs".into(), "jira".into()],
    };
    let notices = |out: &[Out]| -> Vec<String> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(WbEvent::Notice { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    };

    let mut acp = antigravity(&start());
    acp.session_mcp(mcp());
    let out = opened(&mut acp);
    let new = &sent(&out)[1];
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"]["mcpServers"], json!(mcp().servers));
    assert_eq!(
        notices(&out),
        ["Antigravity takes only local MCP servers, so it can't use these servers from Settings: docs, jira."]
    );

    let mut resumed = antigravity(&Start {
        resume: true,
        native_id: SESSION,
        ..start()
    });
    resumed.session_mcp(mcp());
    let out = resumed.open();
    let load = resumed.read(
        include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    );
    let load = sent(&[out, load].concat());
    let load = load
        .iter()
        .find(|message| message["method"] == "session/load")
        .expect("a load");
    assert_eq!(load["params"]["mcpServers"], json!(mcp().servers));
}

/// agy names a session server's tool `mcp_<server>_<tool>`; the app reads `mcp__<server>__<tool>`, so page links and
/// the tool rows see agy's calls as they see Claude Code's. A server name with an underscore keeps it.
#[test]
fn session_server_tools_take_the_apps_mcp_names() {
    let servers = ["nexus".to_string(), "nexus_browser".to_string()];
    assert_eq!(
        mcp_tool_name("mcp_nexus_createPage", &servers).as_deref(),
        Some("mcp__nexus__createPage")
    );
    assert_eq!(
        mcp_tool_name("mcp_nexus_browser_browser_open", &servers).as_deref(),
        Some("mcp__nexus_browser__browser_open")
    );
    // Not a launched server's, or not an MCP tool: agy's own name stays.
    assert_eq!(mcp_tool_name("mcp_linear_list_issues", &servers), None);
    assert_eq!(mcp_tool_name("run_command", &servers), None);
    assert_eq!(mcp_tool_name("mcp_nexus_", &servers), None);

    // As agy 1.5.15 sends it (recorded 2026-11-23): `name`, kind `other`, and its own `toolSummary` in the input.
    let mut acp = antigravity(&start());
    acp.session_mcp(SessionMcp {
        servers: vec![json!({"name": "nexus"}), json!({"name": "nexus_browser"})],
        skipped: Vec::new(),
    });
    opened(&mut acp);
    let call = json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": {
        "sessionUpdate": "tool_call", "toolCallId": "call_1", "name": "mcp_nexus_createPage",
        "args": {"title": "t", "toolSummary": "Publish"}, "status": "in_progress", "kind": "other",
        "title": "Run mcp_nexus_createPage", "rawInput": {"title": "t", "toolSummary": "Publish"}}}});
    let out = acp.read(&call.to_string());
    let names: Vec<&str> = out
        .iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Tool { name, .. }) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["mcp__nexus__createPage"]);
}

/// Antigravity starts on the person's Google AI plan only, and never while agy is set to bill an API key.
#[test]
fn launch_needs_the_plan_and_no_api_key() {
    use crate::event::Via;
    let home = std::env::temp_dir().join(format!("wb-agy-{}", uuid::Uuid::new_v4().simple()));
    let home_text = home.to_string_lossy().into_owned();
    assert_eq!(Antigravity.launch_refusal(Via::Subscription, Some(&home_text)), Ok(()));
    let gateway = Antigravity
        .launch_refusal(Via::Nexus, Some(&home_text))
        .err()
        .unwrap_or_default();
    assert!(gateway.contains("Google AI plan only"), "{gateway}");
    let settings = nexus_client::local_agents::antigravity_settings(&home);
    std::fs::create_dir_all(settings.parent().unwrap_or(&home)).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&settings, r#"{"modelProvider":"gemini-api-key"}"#).unwrap_or_else(|e| panic!("{e}"));
    let key = Antigravity
        .launch_refusal(Via::Subscription, Some(&home_text))
        .err()
        .unwrap_or_default();
    assert!(key.contains("Remove \"modelProvider\""), "{key}");
    let _ = std::fs::remove_dir_all(home);
}

/// The recorded session, replayed: the client answers as the probe did, and every line agy wrote means what the
/// person saw.
#[test]
fn recorded_turns_stream_ask_and_end_with_their_tokens() {
    let mut acp = antigravity(&start());
    let prompts = [
        "Reply with the single word: ready.",
        "Create a file named hello.txt in the current folder containing the text hi. Use your file tool.",
        "Count slowly from 1 to 200, one number per line, explaining each number in a sentence.",
    ];
    let mut next = 0;
    let mut out = acp.open();
    for line in include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl").lines() {
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
                    // The third turn is stopped as soon as it is sent.
                    if next == prompts.len() {
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

    assert_eq!(
        methods(&out),
        [
            "initialize",
            "session/new",
            "session/prompt",
            "session/prompt",
            "answer \"1\"",
            "session/prompt",
            "session/cancel"
        ]
    );
    let messages = sent(&out);
    let new = &messages[1]["params"];
    assert_eq!(new["cwd"], "/work/repo");
    // The host handed no servers: none go.
    assert_eq!(new["mcpServers"], json!([]));
    // Only the tools agy gets: the built-in `nexus` server and `nexus_browser`, not the shared terminal or a
    // structured question tool.
    let append = new["_meta"]["antigravity"]["systemPrompt"]["append"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(append, crate::harness::antigravity::instructions());
    // agy forwards no tool answer to the app: it prints the page's link, which the app shows as a card.
    assert!(append.contains("`View it:`") && !append.contains("there is no viewer link to print"));
    assert_ne!(append, crate::driver::session_tool_instructions());
    for missing in ["terminal_run", "structured user-input tool"] {
        assert!(!append.contains(missing), "agy is told about {missing}");
    }
    for given in [
        "createPage",
        "nexus_browser",
        "nexus sessions send",
        ".nexus/artifacts/",
    ] {
        assert!(append.contains(given), "agy is not told about {given}");
    }
    assert_eq!(messages[2]["params"]["sessionId"], SESSION);
    assert_eq!(messages[2]["params"]["prompt"][0]["text"], prompts[0]);
    assert_eq!(
        messages[4]["result"],
        json!({ "outcome": { "outcome": "selected", "optionId": "allow" } })
    );

    assert!(out.iter().any(|out| matches!(out,
        Out::Init { native_id, model, version }
            if native_id == SESSION && model.as_deref() == Some("gemini-3.8-flash") && version.as_deref() == Some("1.5.8"))));
    // agy's title named the appended instructions, not the request.
    assert!(!out.iter().any(|out| matches!(out, Out::Title(_))));
    let taken: Vec<&Vec<String>> = out
        .iter()
        .filter_map(|out| match out {
            Out::Taken { texts } => Some(texts),
            _ => None,
        })
        .collect();
    assert_eq!(
        taken,
        prompts
            .map(|prompt| vec![prompt.to_string()])
            .iter()
            .collect::<Vec<_>>()
    );

    let texts: Vec<&str> = out
        .iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Text { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        [
            "ready",
            "Created [hello.txt](file:///work/repo/hello.txt) with the text `hi`."
        ]
    );
    assert!(out.iter().any(|out| matches!(out,
        Out::Event(WbEvent::Thinking { text, .. }) if text.contains("write_file"))));

    // The write shows once, with the diff the person approved, then its result.
    let tools: Vec<(&str, &Value)> = out
        .iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Tool { name, input, .. }) => Some((name.as_str(), input)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tools,
        [(
            "Write",
            &json!({ "file_path": "/work/repo/hello.txt", "content": "hi\n" })
        )]
    );
    assert!(out.iter().any(|out| matches!(out,
        Out::Event(WbEvent::Approval { tool, tool_use_id: Some(call), description: Some(title), suggestions, .. })
            if tool == "Write" && call == "call_16232601368592308396" && title == "File hello.txt" && suggestions.is_empty())));
    assert!(out.iter().any(|out| matches!(out,
        Out::Event(WbEvent::ToolResult { id, is_error: false, .. }) if id == "call_16232601368592308396")));

    let calls: Vec<&Usage> = out
        .iter()
        .filter_map(|out| match out {
            Out::Call {
                model,
                usage,
                parent: None,
            } if model == "gemini-3.8-flash" => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            &Usage {
                input_tokens: 8646,
                output_tokens: 111,
                cache_read_tokens: 0,
                cache_write_tokens: 0
            },
            &Usage {
                input_tokens: 4677,
                output_tokens: 452,
                cache_read_tokens: 13419,
                cache_write_tokens: 0
            },
        ]
    );
    let results: Vec<(bool, bool, Option<u64>)> = out
        .iter()
        .filter_map(|out| match out {
            Out::Result {
                error,
                interrupted,
                context_window,
                total_cost_usd: None,
                ..
            } => Some((error.is_none(), *interrupted, *context_window)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            (true, false, Some(1_048_576)),
            (true, false, Some(1_048_576)),
            (true, true, Some(1_048_576))
        ]
    );
}

/// A resumed session loads agy's conversation by its id and drops the replay its log already holds.
#[test]
fn recorded_load_reopens_the_conversation_without_its_replay() {
    let mut acp = antigravity(&Start {
        native_id: SESSION,
        resume: true,
        ..start()
    });
    let mut lines = include_str!("../../tests/fixtures/agy-1.5.8-acp-load.jsonl").lines();
    let mut out = acp.open();
    out.extend(acp.read(lines.next().unwrap_or_default()));
    let mut after = Vec::new();
    for line in lines {
        after.extend(acp.read(line));
    }
    assert_eq!(methods(&out), ["initialize", "session/load"]);
    let load = &sent(&out)[1]["params"];
    assert_eq!(
        (load["sessionId"].as_str(), load["cwd"].as_str()),
        (Some(SESSION), Some("/work/repo"))
    );
    assert!(
        after
            .iter()
            .all(|out| !matches!(out, Out::Event(_) | Out::Delta { .. } | Out::Taken { .. })),
        "{after:?}"
    );
    assert!(matches!(&after[0], Out::Init { native_id, .. } if native_id == SESSION));
}

#[test]
fn deny_picks_the_reject_option_and_interrupt_cancels_an_open_approval() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    acp.say(Say::User {
        text: "edit".into(),
        images: Vec::new(),
    });
    let ask = |acp: &mut Driver, id: &str| -> String {
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": "session/request_permission", "params": {
            "sessionId": SESSION,
            "toolCall": { "toolCallId": format!("call-{id}"), "kind": "execute", "title": "ls", "status": "pending", "rawInput": { "command": "ls -la" } },
            "options": [{ "optionId": "allow", "kind": "allow_once" }, { "optionId": "deny", "kind": "reject_once" }],
        }});
        let out = acp.read(&line.to_string());
        assert!(out.iter().any(|out| matches!(out,
            Out::Event(WbEvent::Tool { name, input, .. }) if name == "Bash" && input["command"] == "ls -la")));
        out.iter()
            .find_map(|out| match out {
                Out::Event(WbEvent::Approval { request_id, .. }) => Some(request_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no approval: {out:?}"))
    };
    let first = ask(&mut acp, "7");
    let denied = sent(&acp.say(Say::Deny {
        request_id: first,
        message: None,
    }));
    assert_eq!(
        denied,
        [
            json!({ "jsonrpc": "2.0", "id": "7", "result": { "outcome": { "outcome": "selected", "optionId": "deny" } } })
        ]
    );
    let second = ask(&mut acp, "8");
    let stopped = acp.say(Say::Interrupt { request: "stop".into() });
    assert_eq!(methods(&stopped), ["session/cancel", "answer \"8\""], "{stopped:?}");
    assert_eq!(sent(&stopped)[1]["result"]["outcome"]["outcome"], "cancelled");
    assert!(stopped.contains(&Out::Cancelled { request_id: second }));
    assert!(stopped.contains(&Out::Ack {
        request_id: "stop".into(),
        error: None
    }));
}

/// ACP's answer to an approval carries no text, and the adapter keeps no reason of its own: the workbench sends it as
/// the person's queued message (`Workbench::queue_denial_reason`). A denial with a reason sends only the answer, and
/// the turn's end sends no prompt and counts no turn.
#[test]
fn deny_reason_is_not_the_adapters_to_send() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    let turn = acp.say(Say::User {
        text: "run it".into(),
        images: Vec::new(),
    });
    let line = json!({ "jsonrpc": "2.0", "id": "8", "method": "session/request_permission", "params": {
        "sessionId": SESSION,
        "toolCall": { "toolCallId": "call-8", "kind": "execute", "title": "ls" },
        "options": [{ "optionId": "allow", "kind": "allow_once" }, { "optionId": "deny", "kind": "reject_once" }],
    }});
    let reasoned = acp
        .read(&line.to_string())
        .iter()
        .find_map(|out| match out {
            Out::Event(WbEvent::Approval { request_id, .. }) => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no approval"));
    let out = acp.say(Say::Deny {
        request_id: reasoned,
        message: Some("use docs/ instead".into()),
    });
    assert_eq!(
        sent(&out),
        [
            json!({ "jsonrpc": "2.0", "id": "8", "result": { "outcome": { "outcome": "selected", "optionId": "deny" } } })
        ]
    );
    let prompt = sent(&turn)
        .into_iter()
        .rfind(|message| message["method"] == "session/prompt")
        .unwrap_or_else(|| panic!("no prompt: {turn:?}"));
    let ended =
        acp.read(&json!({ "jsonrpc": "2.0", "id": prompt["id"], "result": { "stopReason": "end_turn" } }).to_string());
    assert!(methods(&ended).is_empty(), "{ended:?}");
    assert!(
        !ended.iter().any(|out| matches!(out, Out::TurnQueued { .. })),
        "{ended:?}"
    );
}

#[test]
fn messages_wait_for_the_conversation_and_for_the_turn_under_way() {
    let mut acp = antigravity(&start());
    let early = acp.say(Say::User {
        text: "one".into(),
        images: vec![Image {
            media_type: "image/png".into(),
            data: "aGk=".into(),
            name: None,
        }],
    });
    assert!(early.is_empty());
    assert_eq!(
        acp.say(Say::User {
            text: "two".into(),
            images: Vec::new()
        }),
        [Out::TurnQueued { person: true }]
    );
    let out = opened(&mut acp);
    let prompt = sent(&out).pop().unwrap_or_default();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(
        prompt["params"]["prompt"],
        json!([{ "type": "image", "mimeType": "image/png", "data": "aGk=" }, { "type": "text", "text": "one" }])
    );
    let id = prompt["id"].clone();
    let ended = acp.read(&json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }).to_string());
    let next = sent(&ended).pop().unwrap_or_default();
    assert_eq!(next["params"]["prompt"][0]["text"], "two");
}

#[test]
fn requests_agy_makes_of_the_client_are_refused() {
    let mut acp = antigravity(&start());
    let out = acp.read(r#"{"jsonrpc":"2.0","id":3,"method":"fs/read_text_file","params":{"path":"/etc/hosts"}}"#);
    assert_eq!(sent(&out)[0]["error"]["code"], NO_SUCH_METHOD);
    // Lines that are not JSON-RPC mean nothing.
    assert!(acp.read("agy: starting").is_empty());
}

/// The client signs in only when agy asks, once, with a personal Google account, and then opens again.
#[test]
fn sign_in_happens_only_when_agy_asks_and_only_once() {
    let initialized = || {
        let mut acp = antigravity(&start());
        acp.open();
        acp.read(
            include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
                .lines()
                .next()
                .unwrap_or_default(),
        );
        acp
    };
    let required = |id: u64| {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": AUTH_REQUIRED, "message": "Authentication required" } })
            .to_string()
    };
    let mut acp = initialized();
    let asked = sent(&acp.read(&required(2)));
    assert_eq!(asked[0]["method"], "authenticate");
    assert_eq!(asked[0]["params"]["methodId"], "oauth-personal");
    let reopened = acp.read(&json!({ "jsonrpc": "2.0", "id": 3, "result": {} }).to_string());
    assert_eq!(methods(&reopened), ["session/new"]);
    // Still signed out after signing in: say what to run, and end the run.
    let out = acp.read(&required(4));
    assert!(matches!(&out[..], [Out::Event(WbEvent::Error { text, .. }), Out::Close]
        if text.contains("Run `agy` in a terminal") && text.contains("Authentication required")));

    let mut refused = initialized();
    refused.read(&required(2));
    let out = refused.read(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"Login cancelled"}}"#);
    assert!(
        matches!(&out[..], [Out::Event(WbEvent::Error { text, .. }), Out::Close] if text.contains("Login cancelled"))
    );
}

/// A steer while a turn runs waits for it, and the turn it reserves is its sender's: Nexus's notice gets one that is
/// not the person's.
#[test]
fn a_steer_during_a_turn_reserves_a_turn_for_its_sender() {
    let mut acp = antigravity(&start());
    let steer = |text: &str, person: bool| Say::Steer {
        text: text.into(),
        images: Vec::new(),
        person,
    };
    assert!(
        acp.say(Say::User {
            text: "one".into(),
            images: Vec::new()
        })
        .is_empty()
    );
    assert_eq!(acp.say(steer("a notice", false)), [Out::TurnQueued { person: false }]);
    assert_eq!(acp.say(steer("the person's", true)), [Out::TurnQueued { person: true }]);
}

/// Stop before agy opened the conversation ends that turn; the message queued behind it is the next turn.
#[test]
fn interrupt_before_the_conversation_opens_keeps_the_queued_message() {
    let mut acp = antigravity(&start());
    let say = |text: &str| Say::User {
        text: text.into(),
        images: Vec::new(),
    };
    assert!(acp.say(say("one")).is_empty());
    assert_eq!(acp.say(say("two")), [Out::TurnQueued { person: true }]);
    let stopped = acp.say(Say::Interrupt { request: "stop".into() });
    assert!(
        matches!(&stopped[..], [Out::Result { interrupted: true, .. }, Out::Ack { .. }]),
        "{stopped:?}"
    );
    let out = opened(&mut acp);
    let prompt = sent(&out).pop().unwrap_or_default();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["prompt"][0]["text"], "two");
}

/// A long message's echo can come in pieces; it is read once, whole.
#[test]
fn split_echo_is_read_once_whole() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    acp.say(Say::User {
        text: "hello there".into(),
        images: Vec::new(),
    });
    let chunk = |kind: &str, text: &str| {
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": SESSION,
            "update": { "sessionUpdate": kind, "content": { "type": "text", "text": text } } } })
        .to_string()
    };
    assert!(acp.read(&chunk("user_message_chunk", "hello ")).is_empty());
    assert!(acp.read(&chunk("user_message_chunk", "there")).is_empty());
    let reply = acp.read(&chunk("agent_message_chunk", "hi"));
    assert_eq!(
        reply[0],
        Out::Taken {
            texts: vec!["hello there".into()]
        }
    );
}

fn update(kind: &str, text: &str) -> String {
    json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": SESSION,
        "update": { "sessionUpdate": kind, "content": { "type": "text", "text": text } } } })
    .to_string()
}

fn tool_call(id: &str, status: &str) -> String {
    json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": SESSION,
        "update": { "sessionUpdate": "tool_call_update", "toolCallId": id, "kind": "execute",
            "title": "make", "status": status } } })
    .to_string()
}

/// A conversation agy has open, in a turn it started itself: its text, with no prompt of the client's.
fn in_own_turn() -> Driver {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    let out = acp.read(&update("agent_message_chunk", "Done"));
    assert!(
        matches!(&out[..], [Out::Delta { text, .. }] if text == "Done"),
        "{out:?}"
    );
    acp
}

/// agy says nothing when a turn it started ends: a silence with nothing under way ends it, with its last text.
#[test]
fn own_turn_ends_after_a_quiet_spell_with_its_last_text() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    assert_eq!(acp.quiet_after(), None);
    let mut acp = in_own_turn();
    assert_eq!(acp.quiet_after(), Some(OWN_TURN_QUIET));
    let ended = acp.quiet();
    assert!(
        matches!(&ended[..], [Out::Event(WbEvent::Text { text, .. }), Out::Result { interrupted: false, error: None, .. }]
            if text == "Done"),
        "{ended:?}"
    );
    assert_eq!(acp.quiet_after(), None);
    assert!(acp.quiet().is_empty());
}

/// A tool call still running, or an approval still open, holds agy's own turn open however long it is quiet.
#[test]
fn own_turn_waits_for_its_tool_calls_and_approvals() {
    let mut acp = in_own_turn();
    acp.read(&tool_call("build", "in_progress"));
    assert_eq!(acp.quiet_after(), None);
    assert!(acp.quiet().is_empty());
    acp.read(&tool_call("build", "completed"));
    assert_eq!(acp.quiet_after(), Some(OWN_TURN_QUIET));

    let asked = acp.read(
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "session/request_permission", "params": {
            "sessionId": SESSION, "toolCall": { "toolCallId": "rm", "kind": "execute", "title": "rm" },
            "options": [{ "optionId": "allow", "kind": "allow_once", "name": "Allow" }] } })
        .to_string(),
    );
    let request_id = asked
        .iter()
        .find_map(|out| match out {
            Out::Event(WbEvent::Approval { request_id, .. }) => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(acp.quiet_after(), None);
    acp.say(Say::Allow {
        request_id,
        input: Value::Null,
        answers: None,
        remember: None,
    });
    acp.read(&tool_call("rm", "completed"));
    assert_eq!(acp.quiet_after(), Some(OWN_TURN_QUIET));
}

/// A message sent while agy runs its own turn goes to agy at once, as agy 1.5.12 takes it at its next step. The
/// prompt's answer ends the one turn the session shows, and no silence ends it before.
#[test]
fn message_sent_into_own_turn_goes_at_once_and_its_answer_ends_the_turn() {
    let mut acp = in_own_turn();
    let out = acp.say(Say::Steer {
        text: "Are you finished?".into(),
        images: Vec::new(),
        person: true,
    });
    let prompt = sent(&out).pop().unwrap_or_default();
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(acp.quiet_after(), None);
    let reply = acp.read(&update("agent_message_chunk", " Yes."));
    assert_eq!(
        reply[0],
        Out::Taken {
            texts: vec!["Are you finished?".into()]
        }
    );
    let ended =
        acp.read(&json!({ "jsonrpc": "2.0", "id": prompt["id"], "result": { "stopReason": "end_turn" } }).to_string());
    let results = ended.iter().filter(|out| matches!(out, Out::Result { .. })).count();
    assert_eq!(results, 1, "{ended:?}");
    assert!(!ended.iter().any(|out| matches!(out, Out::Taken { .. })), "{ended:?}");
    assert_eq!(acp.quiet_after(), None);
}

/// A denial in agy's own turn: the denied call holds the turn open no longer, and its end sends no prompt (the
/// workbench queued the reason as the person's message).
#[test]
fn own_turn_denial_frees_its_call() {
    let mut acp = in_own_turn();
    let asked = acp.read(
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "session/request_permission", "params": {
            "sessionId": SESSION, "toolCall": { "toolCallId": "rm", "kind": "execute", "title": "rm -rf build",
                "status": "pending" },
            "options": [{ "optionId": "reject", "kind": "reject_once", "name": "Reject" }] } })
        .to_string(),
    );
    let request_id = asked
        .iter()
        .find_map(|out| match out {
            Out::Event(WbEvent::Approval { request_id, .. }) => Some(request_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    acp.say(Say::Deny {
        request_id,
        message: Some("use git clean".into()),
    });
    assert_eq!(acp.quiet_after(), Some(OWN_TURN_QUIET));
    let ended = acp.quiet();
    assert!(ended.iter().any(|out| matches!(out, Out::Result { .. })), "{ended:?}");
    assert!(
        !ended.iter().any(|out| matches!(out, Out::TurnQueued { .. })),
        "{ended:?}"
    );
    assert!(methods(&ended).is_empty(), "{ended:?}");
    assert_eq!(acp.quiet_after(), None);
}

/// Output the session does not show opens no turn of agy's own: there is no turn for a silence to end.
#[test]
fn output_the_session_does_not_show_opens_no_own_turn() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    let usage = json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": SESSION,
        "update": { "sessionUpdate": "usage_update", "used": 1, "size": 10 } } });
    assert!(acp.read(&usage.to_string()).is_empty());
    assert!(acp.read(&update("agent_thought_chunk", "  ")).is_empty());
    assert_eq!(acp.quiet_after(), None);
    assert!(acp.quiet().is_empty());
}

/// Stop ends agy's own turn at once: no answer would.
#[test]
fn stop_ends_own_turn() {
    let mut acp = in_own_turn();
    acp.read(&tool_call("build", "in_progress"));
    let stopped = acp.say(Say::Interrupt { request: "stop".into() });
    assert_eq!(methods(&stopped), ["session/cancel"]);
    assert!(
        stopped
            .iter()
            .any(|out| matches!(out, Out::Result { interrupted: true, .. })),
        "{stopped:?}"
    );
    assert_eq!(acp.quiet_after(), None);
}

/// agy 1.5.12 echoes no message: the message is read once agy acts after it, and not before. A prompt refused or
/// cancelled before agy acted was not read.
#[test]
fn message_without_an_echo_is_read_once_agy_acts() {
    let say = |acp: &mut Driver, text: &str| {
        let out = acp.say(Say::User {
            text: text.into(),
            images: Vec::new(),
        });
        sent(&out).pop().unwrap_or_default()["id"].clone()
    };
    let answer = |id: &Value, result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
    let taken = |out: &[Out]| {
        out.iter()
            .filter_map(|out| match out {
                Out::Taken { texts } => Some(texts.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut acp = antigravity(&start());
    opened(&mut acp);
    let id = say(&mut acp, "one");
    let usage = json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": SESSION,
        "update": { "sessionUpdate": "usage_update", "used": 1, "size": 10 } } });
    assert!(taken(&acp.read(&usage.to_string())).is_empty());
    assert_eq!(
        taken(&acp.read(&update("agent_thought_chunk", "Thinking"))),
        [vec!["one".to_string()]]
    );
    assert!(taken(&acp.read(&update("agent_message_chunk", "Hi"))).is_empty());
    assert!(taken(&acp.read(&answer(&id, json!({ "stopReason": "end_turn" })))).is_empty());

    // Answered with no output: the answer says agy ran it.
    let id = say(&mut acp, "two");
    assert_eq!(
        taken(&acp.read(&answer(&id, json!({ "stopReason": "end_turn" })))),
        [vec!["two".to_string()]]
    );
    let id = say(&mut acp, "three");
    assert!(taken(&acp.read(&answer(&id, json!({ "stopReason": "cancelled" })))).is_empty());
    let id = say(&mut acp, "four");
    let refused = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": "quota" } });
    assert!(taken(&acp.read(&refused.to_string())).is_empty());
}

/// A plain Allow never grants more than this once: an "always" option only when the person picked it.
#[test]
fn allow_picks_always_only_when_the_person_did() {
    let mut acp = antigravity(&start());
    opened(&mut acp);
    let ask = |acp: &mut Driver, id: &str| -> String {
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": "session/request_permission", "params": {
            "sessionId": SESSION,
            "toolCall": { "toolCallId": format!("call-{id}"), "kind": "execute", "title": "ls" },
            "options": [{ "optionId": "always", "name": "Always", "kind": "allow_always" }, { "optionId": "no", "kind": "reject_once" }],
        }});
        acp.read(&line.to_string())
            .iter()
            .find_map(|out| match out {
                Out::Event(WbEvent::Approval {
                    request_id,
                    suggestions,
                    ..
                }) => {
                    assert_eq!(suggestions[0].raw, json!("always"));
                    Some(request_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no approval"))
    };
    let allow = |request_id: String, remember: Option<Value>| Say::Allow {
        request_id,
        input: json!({}),
        answers: None,
        remember,
    };
    let once = ask(&mut acp, "1");
    let answer = sent(&acp.say(allow(once, None)));
    assert_eq!(answer[0]["result"]["outcome"]["outcome"], "cancelled");
    let always = ask(&mut acp, "2");
    let answer = sent(&acp.say(allow(always, Some(json!("always")))));
    assert_eq!(answer[0]["result"]["outcome"]["optionId"], "always");
}

/// The picker's row for `agy models`' `gemini-4-hydrogen-xhigh` is `gemini-4-hydrogen` at Extra high: the model and
/// the level as agy's session names them, set as it opens, before the first turn. Nothing goes on agy's command
/// line. The level goes once agy has taken the model: only Hydrogen offers xhigh, and agy answers out of order.
#[test]
fn picked_model_and_level_are_set_as_the_conversation_opens() {
    let mut acp = antigravity(&Start {
        model: Some("gemini-4-hydrogen"),
        effort: Some(Effort::Xhigh),
        ..start()
    });
    acp.say(Say::User {
        text: "hi".into(),
        images: Vec::new(),
    });
    let out = opened(&mut acp);
    let messages = sent(&out);
    assert_eq!(
        methods(&out),
        ["initialize", "session/new", "session/set_config_option"]
    );
    let set = |message: &Value| {
        (
            message["params"]["configId"].clone(),
            message["params"]["value"].clone(),
        )
    };
    assert_eq!(set(&messages[2]), (json!("model"), json!("gemini-4-hydrogen")));
    let level = acp.read(&json!({ "jsonrpc": "2.0", "id": messages[2]["id"], "result": {} }).to_string());
    assert_eq!(acp.model.as_deref(), Some("gemini-4-hydrogen"));
    assert_eq!(
        methods(&level),
        ["session/set_config_option"],
        "the turn waits for the level"
    );
    let level = sent(&level);
    assert_eq!(set(&level[0]), (json!("thought_level"), json!("xhigh")));
    let taken = acp.read(&json!({ "jsonrpc": "2.0", "id": level[0]["id"], "result": {} }).to_string());
    assert_eq!(methods(&taken), ["session/prompt"]);
    // Back to Default: agy's first model, and then the level a new conversation opened at.
    let back = sent(&acp.say(Say::Model {
        request: "default".into(),
        model: None,
    }));
    assert_eq!(set(&back[0]), (json!("model"), json!("gemini-3.8-flash")));
    assert!(
        acp.say(Say::Effort {
            request: "default-level".into(),
            effort: None,
        })
        .is_empty()
    );
    let after = acp.read(&json!({ "jsonrpc": "2.0", "id": back[0]["id"], "result": {} }).to_string());
    assert!(matches!(&after[0], Out::Ack { request_id, error: None } if request_id == "default"));
    assert_eq!(set(&sent(&after)[0]), (json!("thought_level"), json!("high")));
    // The model agy already runs is not asked for again.
    let mut same = antigravity(&Start {
        model: Some("gemini-3.8-flash"),
        ..start()
    });
    assert_eq!(methods(&opened(&mut same)), ["initialize", "session/new"]);
}

#[test]
fn settings_apply_as_the_conversation_opens_and_after() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::BypassPermissions,
        effort: Some(Effort::Medium),
        ..start()
    });
    acp.say(Say::User {
        text: "plan it".into(),
        images: Vec::new(),
    });
    let out = opened(&mut acp);
    let messages = sent(&out);
    assert_eq!(
        methods(&out),
        [
            "initialize",
            "session/new",
            "session/set_mode",
            "session/set_config_option"
        ],
        "the first turn waits for the settings"
    );
    // Full access runs `accept-edits` with `always-proceed`, never agy's `auto`.
    assert_eq!(messages[2]["params"]["modeId"], "accept-edits");
    assert_eq!(
        (&messages[3]["params"]["configId"], &messages[3]["params"]["value"]),
        (&json!("thought_level"), &json!("medium"))
    );
    // A refused setting is said, not fatal; the turn goes once both are answered.
    let taken = acp.read(&json!({ "jsonrpc": "2.0", "id": messages[2]["id"], "result": {} }).to_string());
    assert_eq!(taken, []);
    let refused = acp.read(
        &json!({ "jsonrpc": "2.0", "id": messages[3]["id"], "error": { "code": -32602, "message": "no such level" } })
            .to_string(),
    );
    assert!(
        matches!(&refused[..], [Out::Event(WbEvent::Notice { text }), Out::Reply(_)] if text.contains("thinking level"))
    );
    assert_eq!(methods(&refused), ["session/prompt"]);

    // A model counts once agy takes it.
    let asked = sent(&acp.say(Say::Model {
        request: "model".into(),
        model: Some("claude-sonnet-4-6".into()),
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(asked["params"]["configId"], "model");
    acp.read(&json!({ "jsonrpc": "2.0", "id": asked["id"], "error": { "code": -32602, "message": "no" } }).to_string());
    assert_eq!(acp.model.as_deref(), Some("gemini-3.8-flash"));

    let mode = acp.say(Say::Mode {
        request: "m".into(),
        mode: PermissionMode::AcceptEdits,
    });
    let asked = sent(&mode).pop().unwrap_or_default();
    assert_eq!(asked["params"]["modeId"], "accept-edits");
    let acked = acp.read(&json!({ "jsonrpc": "2.0", "id": asked["id"], "result": {} }).to_string());
    assert_eq!(
        acked,
        [Out::Ack {
            request_id: "m".into(),
            error: None
        }]
    );
    // Extra high is agy's xhigh, which agy offers for the models it lists at it.
    let asked = sent(&acp.say(Say::Effort {
        request: "x".into(),
        effort: Some(Effort::Xhigh),
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(
        (&asked["params"]["configId"], &asked["params"]["value"]),
        (&json!("thought_level"), &json!("xhigh"))
    );
    assert!(matches!(
        &acp.say(Say::Effort {
            request: "e".into(),
            effort: Some(Effort::Max)
        })[..],
        [Out::Ack { error: Some(_), .. }]
    ));
    assert!(matches!(
        &acp.say(Say::Compact {
            request: "c".into(),
            instructions: None
        })[..],
        [Out::Answered { result: Err(_), .. }]
    ));
}

#[test]
fn forks_and_message_edits_are_refused() {
    let mut fork = antigravity(&Start {
        resume: true,
        fork: true,
        ..start()
    });
    assert!(matches!(
        &fork.open()[..],
        [Out::Event(WbEvent::Error { .. }), Out::Close]
    ));

    let mut edit = antigravity(&Start {
        resume: true,
        rewind: Some("4"),
        rewind_required: true,
        ..start()
    });
    edit.say(Say::User {
        text: "again".into(),
        images: Vec::new(),
    });
    let out = opened(&mut edit);
    assert!(!methods(&out).contains(&"session/prompt".to_string()));
    assert!(out.iter().any(|out| matches!(out, Out::Result { error: Some(_), .. })));
    assert_eq!(out.last(), Some(&Out::Close));

    let mut reverted = antigravity(&Start {
        resume: true,
        rewind: Some("4"),
        ..start()
    });
    let out = opened(&mut reverted);
    assert!(
        out.iter()
            .any(|out| matches!(out, Out::Event(WbEvent::Notice { text }) if text.contains("reverted")))
    );
}

/// agy 1.5.8's `session/new` answer, without its `auto` mode unless `auto`: agy 1.5.10 offers none to a Gemini
/// Enterprise account.
fn session_new(auto: bool) -> String {
    let line = include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
        .lines()
        .nth(1)
        .unwrap_or_default();
    let mut answer: Value = serde_json::from_str(line).unwrap_or_default();
    if !auto {
        if let Some(modes) = answer
            .pointer_mut("/result/modes/availableModes")
            .and_then(Value::as_array_mut)
        {
            modes.retain(|mode| mode["id"] != "auto");
        }
        let options = answer
            .pointer_mut("/result/configOptions")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter(|option| option["id"] == "mode")
            .filter_map(|option| option["options"].as_array_mut());
        for modes in options {
            modes.retain(|mode| mode["value"] != "auto");
        }
    }
    answer.to_string()
}

/// Opens a conversation whose answer offers agy's `auto` mode, or not.
fn opened_offering(acp: &mut Driver, auto: bool) -> Vec<Out> {
    let mut out = acp.open();
    let initialize = include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
        .lines()
        .next()
        .unwrap_or_default();
    out.extend(acp.read(initialize));
    out.extend(acp.read(&session_new(auto)));
    out
}

/// agy's answer to the client's request `id`: its result, or a refusal saying `error`.
fn reply(id: &Value, error: Option<&str>) -> String {
    match error {
        Some(error) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": error } }),
        None => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
    }
    .to_string()
}

const NO_AUTO: &str =
    "Auto mode is not available: it needs the OS terminal sandbox and may not be rolled out to you yet";

fn has_notice(out: &[Out]) -> bool {
    out.iter().any(|out| matches!(out, Out::Event(WbEvent::Notice { .. })))
}

fn notices(out: &[Out]) -> Vec<String> {
    out.iter()
        .filter_map(|out| match out {
            Out::Event(WbEvent::Notice { text }) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn notice(text: &str) -> Out {
    Out::Event(WbEvent::Notice { text: text.into() })
}

/// Where agy offers `auto`, Auto runs it. Full access runs `accept-edits` with `always-proceed` instead, because agy's
/// `auto` asks before irreversible actions: it never asks for `auto`, and shows no notice.
#[test]
fn auto_runs_agy_auto_when_it_is_offered_and_full_access_runs_accept_edits() {
    for (mode, id) in [
        (PermissionMode::Auto, "auto"),
        (PermissionMode::BypassPermissions, "accept-edits"),
    ] {
        let mut acp = antigravity(&Start { mode, ..start() });
        acp.say(Say::User {
            text: "hi".into(),
            images: Vec::new(),
        });
        let out = opened_offering(&mut acp, true);
        let set = sent(&out).pop().unwrap_or_default();
        assert_eq!(set["params"]["modeId"], id, "{mode:?}");
        let taken = acp.read(&reply(&set["id"], None));
        assert_eq!(methods(&taken), ["session/prompt"], "{mode:?}");
        assert!(!has_notice(&taken), "{mode:?}: {taken:?}");
        assert_eq!(acp.in_force.1, mode, "{mode:?}");
        // A switch back to it later is taken the same way.
        let again = sent(&acp.say(Say::Mode {
            request: "again".into(),
            mode,
        }))
        .pop()
        .unwrap_or_default();
        assert_eq!(again["params"]["modeId"], id, "{mode:?}");
        let mut expected = Vec::new();
        if id == "auto" {
            expected.push(Out::AutoOffered(true));
        }
        expected.push(Out::Ack {
            request_id: "again".into(),
            error: None,
        });
        assert_eq!(acp.read(&reply(&again["id"], None)), expected, "{mode:?}");
    }
}

/// Without agy's `auto`, Auto and Full access set `accept-edits` over ACP while `toolPermission` governs tool
/// approval: the session keeps its mode, shows the account-policy notice once on a new session, and waits for it.
/// Full access shows it too: the account's policy can still make agy ask despite `always-proceed`.
#[test]
fn full_access_and_auto_without_agy_auto_use_accept_edits_acp_mode_and_keep_session_mode() {
    for mode in [PermissionMode::BypassPermissions, PermissionMode::Auto] {
        let expected = [ACCOUNT_POLICY_NOTICE];
        let mut acp = antigravity(&Start { mode, ..start() });
        acp.say(Say::User {
            text: "hi".into(),
            images: Vec::new(),
        });
        let out = opened_offering(&mut acp, false);
        assert_eq!(methods(&out), ["initialize", "session/new", "session/set_mode"]);
        let set = sent(&out).pop().unwrap_or_default();
        assert_eq!(set["params"]["modeId"], "accept-edits");
        let taken = acp.read(&reply(&set["id"], None));
        assert_eq!(methods(&taken), ["session/prompt"]);
        assert!(!has_notice(&out), "{out:?}");
        assert_eq!(notices(&taken), expected, "{mode:?}");
        assert_eq!(acp.in_force.1, mode);
        // A turn says nothing more.
        let prompt = sent(&taken).pop().unwrap_or_default();
        let ended = acp
            .read(&json!({ "jsonrpc": "2.0", "id": prompt["id"], "result": { "stopReason": "end_turn" } }).to_string());
        assert!(!has_notice(&ended), "{ended:?}");
    }
}

/// A switch to Auto or Full access without agy's `auto` sets `accept-edits` over ACP, acks the switch with the
/// account-policy notice, and keeps the chosen mode. A refusal of it is the change's own.
#[test]
fn switch_to_full_access_without_agy_auto_acks_with_accept_edits_acp_mode() {
    let mut acp = antigravity(&start());
    let out = opened_offering(&mut acp, false);
    assert_eq!(methods(&out), ["initialize", "session/new"]);
    for (request, mode) in [
        ("full", PermissionMode::BypassPermissions),
        ("auto", PermissionMode::Auto),
    ] {
        let set = sent(&acp.say(Say::Mode {
            request: request.into(),
            mode,
        }))
        .pop()
        .unwrap_or_default();
        assert_eq!(set["params"]["modeId"], "accept-edits");
        assert_eq!(
            acp.read(&reply(&set["id"], None)),
            [
                Out::Ack {
                    request_id: request.into(),
                    error: None
                },
                notice(ACCOUNT_POLICY_NOTICE),
            ],
            "{mode:?}"
        );
        assert_eq!(acp.in_force.1, mode);
    }
    let set = sent(&acp.say(Say::Mode {
        request: "again".into(),
        mode: PermissionMode::BypassPermissions,
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(
        acp.read(&reply(&set["id"], Some("busy"))),
        [Out::Ack {
            request_id: "again".into(),
            error: Some("busy".into())
        }]
    );
}

/// agy listed `auto` and refused it, as agy 1.5.10 refuses a Gemini Enterprise account: retry `accept-edits` over
/// ACP (before the first turn, and on a switch), keep the session mode, emit the policy notice once, and never ask
/// for `auto` again.
#[test]
fn refused_auto_retries_accept_edits_over_acp_and_keeps_session_mode() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::Auto,
        ..start()
    });
    acp.say(Say::User {
        text: "hi".into(),
        images: Vec::new(),
    });
    let auto = sent(&opened_offering(&mut acp, true)).pop().unwrap_or_default();
    assert_eq!(auto["params"]["modeId"], "auto");
    let refused = acp.read(&reply(&auto["id"], Some(NO_AUTO)));
    assert_eq!(methods(&refused), ["session/set_mode"], "the first turn still waits");
    assert!(!has_notice(&refused), "{refused:?}");
    let edits = sent(&refused).pop().unwrap_or_default();
    assert_eq!(edits["params"]["modeId"], "accept-edits");
    let taken = acp.read(&reply(&edits["id"], None));
    assert_eq!(methods(&taken), ["session/prompt"]);
    assert_eq!(notices(&taken), [ACCOUNT_POLICY_NOTICE]);
    assert_eq!(acp.in_force.1, PermissionMode::Auto);
    // agy is not asked for `auto` again.
    let set = sent(&acp.say(Say::Mode {
        request: "m".into(),
        mode: PermissionMode::Auto,
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(set["params"]["modeId"], "accept-edits");

    let mut switched = antigravity(&start());
    opened_offering(&mut switched, true);
    let auto = sent(&switched.say(Say::Mode {
        request: "auto".into(),
        mode: PermissionMode::Auto,
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(auto["params"]["modeId"], "auto");
    let refused = switched.read(&reply(&auto["id"], Some(NO_AUTO)));
    assert!(!has_notice(&refused), "{refused:?}");
    let edits = sent(&refused).pop().unwrap_or_default();
    assert_eq!(edits["params"]["modeId"], "accept-edits");
    assert_eq!(
        switched.read(&reply(&edits["id"], None)),
        [
            Out::Ack {
                request_id: "auto".into(),
                error: None
            },
            notice(ACCOUNT_POLICY_NOTICE),
        ]
    );
    // Once the refusal said agy offers no `auto`, a switch to Full access shows the notice too.
    let full = sent(&switched.say(Say::Mode {
        request: "full".into(),
        mode: PermissionMode::BypassPermissions,
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(full["params"]["modeId"], "accept-edits");
    assert_eq!(
        switched.read(&reply(&full["id"], None)),
        [
            Out::Ack {
                request_id: "full".into(),
                error: None
            },
            notice(ACCOUNT_POLICY_NOTICE),
        ]
    );
}

/// A resumed session already in Accept edits says nothing.
#[test]
fn resumed_session_in_accept_edits_says_nothing() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::AcceptEdits,
        native_id: SESSION,
        resume: true,
        ..start()
    });
    acp.say(Say::User {
        text: "again".into(),
        images: Vec::new(),
    });
    acp.open();
    acp.read(
        include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    );
    let out = acp.read(&session_new(false));
    let set = sent(&out).pop().unwrap_or_default();
    assert_eq!(set["params"]["modeId"], "accept-edits");
    let taken = acp.read(&reply(&set["id"], None));
    assert_eq!(methods(&taken), ["session/prompt"]);
    assert!(!has_notice(&out) && !has_notice(&taken), "{out:?} {taken:?}");
}

/// Without agy's `auto`, the opening `accept-edits` answered after a newer switch to Plan leaves Plan in force.
#[test]
fn opening_accept_edits_answered_after_a_newer_switch_keeps_the_switch() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::BypassPermissions,
        ..start()
    });
    let opening = sent(&opened_offering(&mut acp, false)).pop().unwrap_or_default();
    assert_eq!(opening["params"]["modeId"], "accept-edits");
    let plan = sent(&acp.say(Say::Mode {
        request: "plan".into(),
        mode: PermissionMode::Plan,
    }))
    .pop()
    .unwrap_or_default();
    assert_eq!(
        acp.read(&reply(&plan["id"], None)),
        [Out::Ack {
            request_id: "plan".into(),
            error: None
        }]
    );
    let late = acp.read(&reply(&opening["id"], None));
    assert!(!has_notice(&late), "{late:?}");
    assert_eq!(acp.in_force.1, PermissionMode::Plan);
}

/// A pick of Full access answered after a newer pick of Plan settles cleanly and leaves Plan in force.
#[test]
fn full_access_pick_answered_after_a_newer_pick_keeps_plan() {
    let mut acp = antigravity(&start());
    opened_offering(&mut acp, false);
    let pick = |acp: &mut Driver, request: &str, mode| {
        sent(&acp.say(Say::Mode {
            request: request.into(),
            mode,
        }))
        .pop()
        .unwrap_or_default()
    };
    let full = pick(&mut acp, "full", PermissionMode::BypassPermissions);
    let plan = pick(&mut acp, "plan", PermissionMode::Plan);
    assert_eq!(
        acp.read(&reply(&plan["id"], None)),
        [Out::Ack {
            request_id: "plan".into(),
            error: None
        }]
    );
    assert_eq!(
        acp.read(&reply(&full["id"], None)),
        [Out::Ack {
            request_id: "full".into(),
            error: None
        }]
    );
    assert_eq!(acp.in_force.1, PermissionMode::Plan);
}

/// Launch arguments: each mode's `toolPermission`, and `--sandbox` with the Sandbox switch. Auto is agy's own
/// `auto` on a personal plan, with no sandbox or `toolPermission` of the app's; without agy's `auto`, or before agy
/// said, it runs `proceed-in-sandbox` in the sandbox.
#[test]
fn launch_arguments_follow_the_mode_the_switch_and_agys_auto() {
    let settings = |permission: &str| {
        vec![
            "--settings".to_string(),
            format!(r#"{{"toolPermission":"{permission}"}}"#),
        ]
    };
    let line = |sandbox: bool, settings: Vec<String>| {
        let mut line: Vec<String> = Vec::new();
        if sandbox {
            line.push("--sandbox".into());
        }
        line.extend(settings);
        line.push("acp".into());
        line
    };
    for switch in [false, true] {
        for auto in [None, Some(false), Some(true)] {
            let case = format!("switch {switch}, auto {auto:?}");
            for mode in [PermissionMode::Default, PermissionMode::AcceptEdits] {
                assert_eq!(
                    args(mode, switch, auto),
                    line(switch, settings("request-review")),
                    "{case}"
                );
            }
            assert_eq!(
                args(PermissionMode::Plan, switch, auto),
                line(switch, Vec::new()),
                "{case}"
            );
            assert_eq!(
                args(PermissionMode::BypassPermissions, switch, auto),
                line(switch, settings("always-proceed")),
                "{case}"
            );
            let auto_line = match auto {
                Some(true) => line(switch, Vec::new()),
                _ => line(true, settings("proceed-in-sandbox")),
            };
            assert_eq!(args(PermissionMode::Auto, switch, auto), auto_line, "{case}");
        }
    }
}

/// Whether agy offers `auto`, as the workbench keeps it for the next start: no `auto` in the session's answer, or a
/// refusal of it, says no; only agy taking `auto` says yes. A list alone says nothing, as agy 1.5.10 listed `auto`
/// on a Gemini Enterprise account and then refused it.
#[test]
fn agys_auto_is_reported_when_agy_takes_or_lacks_it_never_from_its_list() {
    let offered = |out: &[Out]| {
        out.iter()
            .filter_map(|out| match out {
                Out::AutoOffered(offered) => Some(*offered),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Ask first: no `auto` asked for, so a list with it says nothing; a list without it says no.
    let mut acp = antigravity(&start());
    assert!(offered(&opened_offering(&mut acp, true)).is_empty());
    let mut acp = antigravity(&start());
    assert_eq!(offered(&opened_offering(&mut acp, false)), [false]);

    // Auto: agy takes `auto`.
    let mut acp = antigravity(&Start {
        mode: PermissionMode::Auto,
        ..start()
    });
    let out = opened_offering(&mut acp, true);
    assert!(offered(&out).is_empty());
    let set = sent(&out).pop().unwrap_or_default();
    assert_eq!(set["params"]["modeId"], "auto");
    assert_eq!(offered(&acp.read(&reply(&set["id"], None))), [true]);

    // Auto: agy lists `auto` and refuses it.
    let mut acp = antigravity(&Start {
        mode: PermissionMode::Auto,
        ..start()
    });
    let out = opened_offering(&mut acp, true);
    let set = sent(&out).pop().unwrap_or_default();
    let refused = acp.read(&reply(&set["id"], Some(NO_AUTO)));
    assert_eq!(offered(&refused), [false]);
    let edits = sent(&refused).pop().unwrap_or_default();
    assert_eq!(edits["params"]["modeId"], "accept-edits");
    assert!(offered(&acp.read(&reply(&edits["id"], None))).is_empty());
}

#[test]
fn an_owned_profile_can_keep_auto_evidence_local() {
    let profile = || Profile {
        name: Cow::Owned(String::from("Local ACP")),
        reports_auto_offered: false,
        ..ACP_PROFILE
    };
    let reports = |out: &[Out]| out.iter().any(|out| matches!(out, Out::AutoOffered(_)));
    let mut driver = Driver::new(&start(), profile());
    assert!(!reports(&opened_offering(&mut driver, false)));
    assert_eq!(driver.auto_offered, Some(false));

    let automatic = Start {
        mode: PermissionMode::Auto,
        ..start()
    };
    let mut driver = Driver::new(&automatic, profile());
    let auto = sent(&opened_offering(&mut driver, true)).pop().expect("mode request");
    assert_eq!(auto["params"]["modeId"], "auto");
    assert!(!reports(&driver.read(&reply(&auto["id"], None))));

    let mut driver = Driver::new(&automatic, profile());
    let auto = sent(&opened_offering(&mut driver, true)).pop().expect("mode request");
    let refused = driver.read(&reply(&auto["id"], Some(NO_AUTO)));
    assert!(!reports(&refused));
    assert_eq!(driver.auto_offered, Some(false));
    assert_eq!(sent(&refused)[0]["params"]["modeId"], "accept-edits");
}

#[test]
fn a_refused_auto_mode_without_a_distinct_fallback_is_not_retried() {
    let profile = Profile {
        modes: Some(ModeMapping {
            requested: |_, _| Some("auto"),
            ..ACP_PROFILE.modes.expect("mode mapping")
        }),
        ..ACP_PROFILE
    };
    let mut driver = Driver::new(
        &Start {
            mode: PermissionMode::Auto,
            ..start()
        },
        profile,
    );
    let auto = sent(&opened_offering(&mut driver, true)).pop().expect("mode request");
    let refused = driver.read(&reply(&auto["id"], Some(NO_AUTO)));
    assert!(sent(&refused).is_empty());
    assert!(has_notice(&refused));
}

/// agy answers out of order: an answer to the opening mode read after a newer switch's leaves the switch in force.
#[test]
fn opening_mode_answered_after_a_newer_switch_keeps_the_switch() {
    let open_then_plan = || {
        let mut acp = antigravity(&Start {
            mode: PermissionMode::Auto,
            ..start()
        });
        let opening = sent(&opened_offering(&mut acp, true)).pop().unwrap_or_default();
        assert_eq!(opening["params"]["modeId"], "auto");
        let plan = sent(&acp.say(Say::Mode {
            request: "plan".into(),
            mode: PermissionMode::Plan,
        }))
        .pop()
        .unwrap_or_default();
        acp.read(&reply(&plan["id"], None));
        (acp, opening)
    };
    let (mut taken, opening) = open_then_plan();
    let late = taken.read(&reply(&opening["id"], None));
    assert!(!has_notice(&late), "{late:?}");
    assert_eq!(taken.in_force.1, PermissionMode::Plan);
    let (mut refused, opening) = open_then_plan();
    let late = refused.read(&reply(&opening["id"], Some(NO_AUTO)));
    assert!(!has_notice(&late), "{late:?}");
    assert_eq!(refused.in_force.1, PermissionMode::Plan);
}

/// An error other than agy's refusal of a mode it lacks is said, and a later switch asks for `auto` again.
#[test]
fn auto_failing_for_another_reason_is_said_and_asked_again() {
    let mut acp = antigravity(&start());
    opened_offering(&mut acp, true);
    let switch = |acp: &mut Driver| {
        sent(&acp.say(Say::Mode {
            request: "auto".into(),
            mode: PermissionMode::Auto,
        }))
        .pop()
        .unwrap_or_default()
    };
    let auto = switch(&mut acp);
    assert_eq!(auto["params"]["modeId"], "auto");
    let failed = json!({ "jsonrpc": "2.0", "id": auto["id"], "error": { "code": -32603, "message": "internal" } });
    assert_eq!(
        acp.read(&failed.to_string()),
        [Out::Ack {
            request_id: "auto".into(),
            error: Some("internal".into())
        }]
    );
    assert_eq!(switch(&mut acp)["params"]["modeId"], "auto");
}

/// A loaded conversation that runs another mode than the session's is set to the session's.
#[test]
fn loaded_conversation_in_another_mode_is_set_to_the_sessions() {
    let mut acp = antigravity(&Start {
        native_id: SESSION,
        resume: true,
        ..start()
    });
    acp.open();
    acp.read(
        include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    );
    let mut answer: Value = serde_json::from_str(&session_new(true)).unwrap_or_default();
    answer["result"]["modes"]["currentModeId"] = json!("accept-edits");
    let set = sent(&acp.read(&answer.to_string())).pop().unwrap_or_default();
    assert_eq!(set["method"], "session/set_mode");
    assert_eq!(set["params"]["modeId"], "default");
}

/// A Full access conversation that already runs `accept-edits` is not asked again and emits no notice, whether agy
/// offers `auto` or not.
#[test]
fn full_access_on_a_conversation_in_accept_edits_skips_set_mode() {
    for offered in [true, false] {
        let mut acp = antigravity(&Start {
            mode: PermissionMode::BypassPermissions,
            native_id: SESSION,
            resume: true,
            ..start()
        });
        acp.open();
        acp.read(
            include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
                .lines()
                .next()
                .unwrap_or_default(),
        );
        let mut answer: Value = serde_json::from_str(&session_new(offered)).unwrap_or_default();
        answer["result"]["modes"]["currentModeId"] = json!("accept-edits");
        let out = acp.read(&answer.to_string());
        assert!(
            !methods(&out).contains(&"session/set_mode".to_string()),
            "offered {offered}: {out:?}"
        );
        assert!(!has_notice(&out), "offered {offered}: {out:?}");
    }
}

/// A resumed Full access conversation reopens silently without agy's `auto`: the notice was shown when the session
/// started, and a restart between turns does not show it again.
#[test]
fn full_access_without_agy_auto_reopens_a_resumed_conversation_silently() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::BypassPermissions,
        native_id: SESSION,
        resume: true,
        ..start()
    });
    acp.say(Say::User {
        text: "again".into(),
        images: Vec::new(),
    });
    acp.open();
    acp.read(
        include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    );
    let out = acp.read(&session_new(false));
    let set = sent(&out).pop().unwrap_or_default();
    assert_eq!(set["params"]["modeId"], "accept-edits");
    let taken = acp.read(&reply(&set["id"], None));
    assert_eq!(methods(&taken), ["session/prompt"]);
    assert!(!has_notice(&out) && !has_notice(&taken), "{out:?} {taken:?}");
    assert_eq!(acp.in_force.1, PermissionMode::BypassPermissions);
}

/// A Full access conversation that an older build left in agy's `auto` is set to `accept-edits`, with no notice.
#[test]
fn full_access_on_a_conversation_left_in_agys_auto_is_set_to_accept_edits() {
    let mut acp = antigravity(&Start {
        mode: PermissionMode::BypassPermissions,
        native_id: SESSION,
        resume: true,
        ..start()
    });
    acp.open();
    acp.read(
        include_str!("../../tests/fixtures/agy-1.5.8-acp-turns.jsonl")
            .lines()
            .next()
            .unwrap_or_default(),
    );
    let mut answer: Value = serde_json::from_str(&session_new(true)).unwrap_or_default();
    answer["result"]["modes"]["currentModeId"] = json!("auto");
    let out = acp.read(&answer.to_string());
    let set = sent(&out).pop().unwrap_or_default();
    assert_eq!(set["method"], "session/set_mode");
    assert_eq!(set["params"]["modeId"], "accept-edits");
    let taken = acp.read(&reply(&set["id"], None));
    assert!(!has_notice(&out) && !has_notice(&taken), "{out:?} {taken:?}");
    assert_eq!(acp.in_force.1, PermissionMode::BypassPermissions);
}

mod cursor;
