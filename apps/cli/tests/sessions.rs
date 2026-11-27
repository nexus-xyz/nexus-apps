//! `nexus sessions` against a stand-in for Nexus for desktop's socket (`sessions.test.ts`): it fails clearly when the
//! app is not listening, asks for access once and keeps the token private, asks again after a revoke, reports a denial
//! and a read-only refusal, sends a start's picks with the current folder's project, names a session by its name or its
//! title (a name first), sets and clears a session's name, and relays MCP messages.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nexus_cli::sessions::{Desktop, bridge};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

#[derive(Default)]
struct Seen {
    asked: Vec<Value>,
    calls: Vec<Value>,
    valid: HashSet<String>,
    issued: usize,
    /// What `list_sessions` answers.
    sessions: Vec<Value>,
    /// When set, `list_sessions` fails with this text.
    list_error: Option<&'static str>,
    /// Sessions the app has but does not list: past the newest 200, or side chats.
    hidden: Vec<Value>,
}

/// What `show_session` answers for `word`, as the app's `find` does: the session with that id, else the one with that
/// name in any case, among all its sessions; else "No session {word}.".
fn shown(seen: &Seen, word: &str) -> Result<Value, String> {
    let all = || seen.sessions.iter().chain(&seen.hidden);
    all()
        .find(|session| session["id"] == word)
        .or_else(|| {
            all().find(|session| {
                session["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(word))
            })
        })
        .cloned()
        .ok_or_else(|| format!("No session {word}."))
}

/// A stand-in for the app: one JSON line in, one out. It grants what was asked for (or `scope`), unless `deny`.
fn fake_app(dir: &Path, deny: bool, scope: Option<&'static str>) -> (PathBuf, Arc<Mutex<Seen>>) {
    let path = dir.join("sessions.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let seen = Arc::new(Mutex::new(Seen {
        sessions: vec![session()],
        ..Seen::default()
    }));
    let state = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let request: Value = serde_json::from_str(&line).expect("json");
            let mut seen = state.lock().expect("seen");
            let reply = if let Some(asked) = request.get("connect") {
                seen.asked.push(request.clone());
                if deny {
                    json!({"error": {"code": "denied", "message": "The person denied access."}})
                } else {
                    seen.issued += 1;
                    let token = format!("tok{}", seen.issued);
                    seen.valid.insert(token.clone());
                    json!({"token": token, "scope": scope.map(Value::from).unwrap_or(asked.clone())})
                }
            } else if !seen.valid.contains(request["token"].as_str().unwrap_or_default()) {
                json!({"jsonrpc": "2.0", "id": request["message"]["id"], "error": {"code": -32001, "message": "no access"}})
            } else {
                let message = &request["message"];
                let name = message["params"]["name"].as_str().unwrap_or_default().to_string();
                seen.calls.push(json!({"token": request["token"], "method": message["method"], "name": name, "args": message["params"]["arguments"]}));
                let acts = ["start_session", "send_message", "stop_session", "name_session"].contains(&name.as_str());
                if let (Some(text), "list_sessions") = (seen.list_error, name.as_str()) {
                    json!({"jsonrpc": "2.0", "id": message["id"], "result": {"isError": true, "content": [{"type": "text", "text": text}]}})
                } else if scope == Some("read") && acts {
                    json!({"jsonrpc": "2.0", "id": message["id"], "result": {"isError": true, "content": [{"type": "text", "text": "nexus CLI may only read sessions."}]}})
                } else {
                    let answer = match name.as_str() {
                        "list_sessions" => Ok(json!({"sessions": seen.sessions})),
                        "start_session" => Ok(session()),
                        "show_session" => shown(
                            &seen,
                            message["params"]["arguments"]["session_id"]
                                .as_str()
                                .unwrap_or_default(),
                        ),
                        _ => Ok(json!({"ok": true})),
                    };
                    match answer {
                        Ok(answer) => {
                            json!({"jsonrpc": "2.0", "id": message["id"], "result": {"isError": false, "structuredContent": answer}})
                        }
                        Err(text) => {
                            json!({"jsonrpc": "2.0", "id": message["id"], "result": {"isError": true, "content": [{"type": "text", "text": text}]}})
                        }
                    }
                }
            };
            drop(seen);
            let _ = writeln!(stream, "{reply}");
        }
    });
    (path, seen)
}

fn session() -> Value {
    json!({"id": "t_1", "title": "Fix the tests", "status": "idle", "agent": "claude", "model": null,
        "permissionMode": "default", "project": {"id": "p", "name": "nexus", "path": "/src/nexus"}})
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-cli-sessions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

fn run(dir: &Path, socket: &Path, argv: &[&str], stdin: &str) -> Run {
    let capture = Capture(std::rc::Rc::new(Captured {
        stdin: stdin.to_string(),
        ..Captured::default()
    }));
    let ctx = Context {
        config_path: dir.join("config.json"),
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::from([("NEXUS_DESKTOP_SOCKET".to_string(), socket.display().to_string())]),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.to_path_buf(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
    }
}

#[test]
fn fails_clearly_when_the_app_is_not_listening() {
    let dir = temp("missing");
    let result = run(&dir, &dir.join("missing.sock"), &["sessions", "list"], "");
    assert_eq!(result.code, 1);
    assert!(
        result
            .err
            .contains("Nexus for desktop is not running, or its outside access is off."),
        "{}",
        result.err
    );
    assert!(
        result.err.contains("Settings › MCP servers › Outside access"),
        "{}",
        result.err
    );
}

#[test]
fn asks_for_access_once_keeps_the_token_private_and_lists() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("list");
    let (socket, seen) = fake_app(&dir, false, None);
    let first = run(&dir, &socket, &["sessions", "list"], "");
    assert_eq!(first.code, 0, "{}", first.err);
    assert_eq!(first.out, "t_1  idle        Fix the tests  nexus");
    assert!(
        first
            .err
            .contains("Approve “nexus CLI” in Nexus for desktop: it asks you now.")
    );
    let tokens = dir.join("desktop-access.json");
    let mode = std::fs::metadata(&tokens).expect("tokens").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&tokens).expect("read")).expect("json");
    assert_eq!(saved, json!({"tokens": {"nexus CLI": "tok1"}}));

    let second = run(&dir, &socket, &["sessions", "list", "--archived", "--limit", "5"], "");
    assert_eq!(second.code, 0, "{}", second.err);
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.asked, vec![json!({"client": "nexus CLI", "connect": "act"})]);
    assert_eq!(seen.calls[1]["token"], "tok1");
    assert_eq!(seen.calls[1]["args"], json!({"archived": true, "limit": 5}));
}

#[test]
fn asks_again_once_after_a_revoke() {
    let dir = temp("revoke");
    let (socket, seen) = fake_app(&dir, false, None);
    assert_eq!(run(&dir, &socket, &["sessions", "stop", "t_1"], "").code, 0);
    seen.lock().expect("seen").valid.clear();
    let again = run(&dir, &socket, &["sessions", "stop", "t_1"], "");
    assert_eq!(again.code, 0, "{}", again.err);
    assert_eq!(again.out, "Stopped t_1.");
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.asked.len(), 2);
    assert_eq!(seen.calls.last().expect("call")["token"], "tok2");
}

#[test]
fn reports_a_denial_and_a_read_only_refusal() {
    let dir = temp("denied");
    let (socket, seen) = fake_app(&dir, true, None);
    let denied = run(&dir, &socket, &["sessions", "list"], "");
    assert_eq!(denied.code, 1);
    assert!(
        denied
            .err
            .contains("Nexus for desktop did not grant access: The person denied access."),
        "{}",
        denied.err
    );
    assert!(seen.lock().expect("seen").calls.is_empty());

    let dir = temp("read");
    let (socket, _) = fake_app(&dir, false, Some("read"));
    let connect = run(&dir, &socket, &["sessions", "connect"], "");
    assert_eq!(connect.out, "Nexus for desktop granted nexus CLI read only access.");
    let refused = run(&dir, &socket, &["sessions", "send", "t_1", "hi"], "");
    assert_eq!(refused.code, 1);
    assert!(
        refused.err.contains("nexus CLI may only read sessions."),
        "{}",
        refused.err
    );
}

#[test]
fn starts_in_the_current_folders_project_with_the_picks_given() {
    let dir = temp("start");
    let (socket, seen) = fake_app(&dir, false, None);
    let started = run(
        &dir,
        &socket,
        &[
            "sessions",
            "start",
            "--agent",
            "codex",
            "--mode",
            "plan",
            "--folder",
            "--name",
            "orchestrator",
            "-",
        ],
        "Find why the tests are slow",
    );
    assert_eq!(started.code, 0, "{}", started.err);
    assert_eq!(started.out, "Started t_1: Fix the tests\nnexus sessions show t_1");
    let project = dir.canonicalize().expect("canonical").display().to_string();
    assert_eq!(
        seen.lock().expect("seen").calls.last().expect("call")["args"],
        json!({"project": project, "prompt": "Find why the tests are slow", "agent": "codex", "permission_mode": "plan", "name": "orchestrator", "worktree": false})
    );
    let started = run(
        &dir,
        &socket,
        &["sessions", "start", "Review this project", "--agent", "grok"],
        "",
    );
    assert_eq!(started.code, 0, "{}", started.err);
    assert_eq!(
        seen.lock().expect("seen").calls.last().expect("Grok start call")["args"],
        json!({"project": project, "prompt": "Review this project", "agent": "grok"})
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "start", "--worktree", "--folder", "x"], "").code,
        2
    );
    assert_eq!(run(&dir, &socket, &["sessions", "start"], "").code, 2);
}

/// Sets the sessions the app lists, each `(id, title)`.
fn titled(seen: &Mutex<Seen>, sessions: &[(&str, &str)]) {
    seen.lock().expect("seen").sessions = sessions
        .iter()
        .map(|(id, title)| json!({"id": id, "title": title, "status": "idle"}))
        .collect();
}

/// Each call but the lists: its tool and the `session_id` it named.
fn acted_on(seen: &Mutex<Seen>) -> Vec<(String, Value)> {
    let seen = seen.lock().expect("seen");
    seen.calls
        .iter()
        .filter(|call| call["name"] != "list_sessions")
        .map(|call| {
            (
                call["name"].as_str().unwrap_or_default().to_string(),
                call["args"]["session_id"].clone(),
            )
        })
        .collect()
}

#[test]
fn show_send_and_stop_name_a_session_by_its_title() {
    let dir = temp("title");
    let (socket, seen) = fake_app(&dir, false, None);
    titled(&seen, &[("t_1", "Fix the tests"), ("t_2", "Ship it")]);
    // An archived session's title names it too.
    seen.lock()
        .expect("seen")
        .sessions
        .push(json!({"id": "t_3", "title": "Old work", "status": "idle", "archived": true}));
    let sent = run(&dir, &socket, &["sessions", "send", "Ship it", "hi"], "");
    assert_eq!(sent.code, 0, "{}", sent.err);
    assert_eq!(sent.out, "Sent to t_2.");
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "Ship it"], "").out,
        "Stopped t_2."
    );
    assert_eq!(run(&dir, &socket, &["sessions", "show", "Fix the tests"], "").code, 0);
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "Old work"], "").out,
        "Stopped t_3."
    );
    // Exact and case-sensitive: another case or spacing is no title, so the app is asked for that id as before.
    assert_eq!(run(&dir, &socket, &["sessions", "stop", "ship it"], "").code, 0);
    // A matched title first asks the app whether some unlisted session holds the word as its name.
    let probe = |word: &str| ("show_session".to_string(), json!(word));
    assert_eq!(
        acted_on(&seen),
        [
            probe("Ship it"),
            ("send_message".to_string(), json!("t_2")),
            probe("Ship it"),
            ("stop_session".to_string(), json!("t_2")),
            probe("Fix the tests"),
            ("show_session".to_string(), json!("t_1")),
            probe("Old work"),
            ("stop_session".to_string(), json!("t_3")),
            ("stop_session".to_string(), json!("ship it")),
        ]
    );
    // The list asks for archived sessions too, as many as the app gives.
    let seen = seen.lock().expect("seen");
    let lists: Vec<&Value> = seen
        .calls
        .iter()
        .filter(|call| call["name"] == "list_sessions")
        .map(|call| &call["args"])
        .collect();
    assert_eq!(lists, [&json!({"archived": true, "limit": 200}); 5]);
}

#[test]
fn refuses_a_title_several_sessions_share_and_names_their_ids() {
    let dir = temp("ambiguous");
    let (socket, seen) = fake_app(&dir, false, None);
    titled(
        &seen,
        &[("t_1", "Ship it"), ("t_2", "Fix the tests"), ("t_3", "Ship it")],
    );
    let refused = run(&dir, &socket, &["sessions", "stop", "Ship it"], "");
    assert_eq!(refused.code, 1);
    assert!(
        refused
            .err
            .contains("2 sessions are titled “Ship it”: t_1, t_3. Name one by its id."),
        "{}",
        refused.err
    );
    // Only the read that looks for a name: nothing is stopped.
    assert_eq!(acted_on(&seen), [("show_session".to_string(), json!("Ship it"))]);
}

#[test]
fn a_failed_title_lookup_says_so_and_asks_for_the_id() {
    let dir = temp("lookup");
    let (socket, seen) = fake_app(&dir, false, None);
    seen.lock().expect("seen").list_error = Some("The sessions did not load.");
    let failed = run(&dir, &socket, &["sessions", "stop", "Ship it"], "");
    assert_eq!(failed.code, 1);
    assert!(
        failed
            .err
            .contains("Could not look up the title “Ship it”: The sessions did not load. Name the session by its id."),
        "{}",
        failed.err
    );
    assert!(acted_on(&seen).is_empty(), "{:?}", acted_on(&seen));
}

#[test]
fn a_word_starting_with_t_is_an_id_and_needs_no_list() {
    let dir = temp("id");
    let (socket, seen) = fake_app(&dir, false, None);
    // t_1's title is t_2's id: `t_2` is still t_2. t_9 is in no list (a side chat, or past the newest 200).
    titled(&seen, &[("t_1", "t_2"), ("t_2", "Ship it")]);
    assert_eq!(run(&dir, &socket, &["sessions", "stop", "t_2"], "").out, "Stopped t_2.");
    assert_eq!(
        run(&dir, &socket, &["sessions", "send", "t_1", "hi"], "").out,
        "Sent to t_1."
    );
    let unknown = run(&dir, &socket, &["sessions", "show", "t_9"], "");
    assert_eq!(unknown.code, 1);
    assert!(unknown.err.contains("No session t_9."), "{}", unknown.err);
    assert_eq!(
        acted_on(&seen),
        [
            ("stop_session".to_string(), json!("t_2")),
            ("send_message".to_string(), json!("t_1")),
            ("show_session".to_string(), json!("t_9")),
        ]
    );
    assert_eq!(seen.lock().expect("seen").calls.len(), 3, "an id makes no list call");
}

/// Sets the sessions the app lists, each `(id, name, title)`.
fn named(seen: &Mutex<Seen>, sessions: &[(&str, Option<&str>, &str)]) {
    seen.lock().expect("seen").sessions = sessions
        .iter()
        .map(|(id, name, title)| json!({"id": id, "name": name, "title": title, "status": "idle"}))
        .collect();
}

#[test]
fn a_name_names_a_session_before_any_title() {
    let dir = temp("named");
    let (socket, seen) = fake_app(&dir, false, None);
    // "orchestrator" is t_1's title and t_2's name: the name wins. "Ship it" is only a title, as before.
    named(
        &seen,
        &[
            ("t_1", None, "orchestrator"),
            ("t_2", Some("orchestrator"), "Plan the work"),
            ("t_3", Some("worker"), "Ship it"),
        ],
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "orchestrator"], "").out,
        "Stopped t_2."
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "send", "worker", "hi"], "").out,
        "Sent to t_3."
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "Ship it"], "").out,
        "Stopped t_3."
    );
    // A name past the newest 200 goes to the app as it is: the app looks it up too.
    seen.lock()
        .expect("seen")
        .hidden
        .push(json!({"id": "t_8", "name": "elsewhere", "title": "Old", "status": "idle"}));
    assert_eq!(run(&dir, &socket, &["sessions", "show", "elsewhere"], "").code, 0);
    assert_eq!(
        acted_on(&seen),
        [
            ("stop_session".to_string(), json!("t_2")),
            ("send_message".to_string(), json!("t_3")),
            ("show_session".to_string(), json!("Ship it")),
            ("stop_session".to_string(), json!("t_3")),
            ("show_session".to_string(), json!("elsewhere")),
        ]
    );
}

#[test]
fn an_unlisted_name_wins_over_a_listed_title() {
    let dir = temp("unlisted");
    let (socket, seen) = fake_app(&dir, false, None);
    // t_1 is named "orchestrator" but is past the newest 200; t_2, listed, is titled "orchestrator".
    named(&seen, &[("t_2", None, "orchestrator")]);
    seen.lock()
        .expect("seen")
        .hidden
        .push(json!({"id": "t_1", "name": "orchestrator", "title": "Plan the work", "status": "idle"}));
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "orchestrator"], "").out,
        "Stopped t_1."
    );
    // With no name anywhere, the title names the session as before.
    seen.lock().expect("seen").hidden.clear();
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "orchestrator"], "").out,
        "Stopped t_2."
    );
}

#[test]
fn a_name_is_matched_in_any_case() {
    let dir = temp("case");
    let (socket, seen) = fake_app(&dir, false, None);
    // Names are unique ignoring case, so "Orchestrator" is t_2's name, over t_1's title "Orchestrator".
    named(
        &seen,
        &[
            ("t_1", None, "Orchestrator"),
            ("t_2", Some("orchestrator"), "Plan the work"),
        ],
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "stop", "Orchestrator"], "").out,
        "Stopped t_2."
    );
    assert_eq!(
        run(&dir, &socket, &["sessions", "send", "ORCHESTRATOR", "hi"], "").out,
        "Sent to t_2."
    );
    // A title stays exact: "plan the work" is no title and no name.
    assert_eq!(run(&dir, &socket, &["sessions", "stop", "plan the work"], "").code, 0);
    assert_eq!(
        acted_on(&seen),
        [
            ("stop_session".to_string(), json!("t_2")),
            ("send_message".to_string(), json!("t_2")),
            ("stop_session".to_string(), json!("plan the work")),
        ]
    );
}

#[test]
fn name_sets_or_clears_a_sessions_name() {
    let dir = temp("rename");
    let (socket, seen) = fake_app(&dir, false, None);
    named(
        &seen,
        &[("t_1", None, "Fix the tests"), ("t_2", Some("old"), "Ship it")],
    );
    let set = run(
        &dir,
        &socket,
        &["sessions", "name", "Fix the tests", "orchestrator"],
        "",
    );
    assert_eq!(set.code, 0, "{}", set.err);
    assert_eq!(set.out, "t_1 is named orchestrator.");
    let cleared = run(&dir, &socket, &["sessions", "name", "old", "--clear"], "");
    assert_eq!(cleared.code, 0, "{}", cleared.err);
    assert_eq!(cleared.out, "t_2 has no name now.");
    assert_eq!(run(&dir, &socket, &["sessions", "name", "t_1"], "").code, 2);
    assert_eq!(
        run(&dir, &socket, &["sessions", "name", "t_1", "x", "--clear"], "").code,
        2
    );
    let named: Vec<Value> = seen
        .lock()
        .expect("seen")
        .calls
        .iter()
        .filter(|call| call["name"] == "name_session")
        .map(|call| call["args"].clone())
        .collect();
    assert_eq!(
        named,
        [
            json!({"session_id": "t_1", "name": "orchestrator"}),
            json!({"session_id": "t_2", "name": ""}),
        ]
    );
}

#[test]
fn list_shows_a_name_column_only_when_a_session_has_a_name() {
    let dir = temp("columns");
    let (socket, seen) = fake_app(&dir, false, None);
    named(
        &seen,
        &[("t_1", Some("orchestrator"), "Plan"), ("t_2", None, "Ship it")],
    );
    let listed = run(&dir, &socket, &["sessions", "list"], "");
    assert_eq!(listed.code, 0, "{}", listed.err);
    assert_eq!(
        listed.out,
        "t_1  idle        orchestrator  Plan\nt_2  idle                      Ship it"
    );
}

#[test]
fn relays_mcp_messages_and_answers_initialize_itself() {
    let dir = temp("mcp");
    let (socket, seen) = fake_app(&dir, false, None);
    let capture = Capture::default();
    let ctx = Context {
        config_path: dir.join("config.json"),
        io: Box::new(capture),
        color: false,
        env: HashMap::from([("NEXUS_DESKTOP_SOCKET".to_string(), socket.display().to_string())]),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let desktop = Desktop::new(&ctx, "Claude Code").expect("desktop");
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let mut lines = Vec::new();
    bridge(&desktop, input.as_bytes(), |line| lines.push(line));
    let replies: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(
        replies.iter().map(|reply| reply["id"].clone()).collect::<Vec<_>>(),
        [json!(1), json!(2)]
    );
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "nexus_sessions");
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.asked, vec![json!({"client": "Claude Code", "connect": "act"})]);
    assert_eq!(seen.calls.len(), 1);
    assert_eq!(seen.calls[0]["method"], "tools/list");
}
