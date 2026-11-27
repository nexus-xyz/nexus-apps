//! The agents' MCP server
//! over real loopback HTTP: one test per check on a request, the grant of each run, the wait's event stream, what
//! each agent is started with, and a lead of each agent that starts a child through the server and reads its result.
//! The stand-ins record what they were started with (`FAKE_LAUNCH_RECORD`); the requests here are written by hand, so
//! each check meets exactly the bytes it is about.

mod support;

#[cfg(unix)]
#[tokio::test]
async fn shared_terminal_tools_work_for_both_runtimes_without_enabling_linked_sessions() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let name = format!("shared-terminal-{}", agent.as_str());
        let s = setup_with(&name, bounded(), false, |config| Config {
            terminal: true,
            ..config
        });
        record_launches(&s);
        let mut new = new_root(&s.project, "hello");
        new.agent = agent;
        new.orchestration = false;
        new.permission_mode = nexus_workbench::event::PermissionMode::BypassPermissions;
        let thread = root(&s, new).await;
        let at = endpoint(&launched(&s, |l| endpoint(l).is_some()).await).expect("terminal MCP server");
        let list = rpc(&post(&at, &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await);
        let names: Vec<_> = list["result"]["tools"]
            .as_array()
            .expect("MCP tools list")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"terminal_run"));
        assert!(names.contains(&"terminal_open"));
        assert!(!names.contains(&"start_session"));
        let start=rpc(&post(&at,&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"terminal_run","arguments":{"command":"printf 'mcp-terminal-ok'","request_id":"one"}}})).await);
        assert_eq!(start["result"]["isError"], false, "{start}");
        let entry: Value = serde_json::from_str(
            start["result"]["content"][0]["text"]
                .as_str()
                .expect("terminal start content"),
        )
        .expect("terminal start JSON");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let answer=rpc(&post(&at,&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"terminal_read","arguments":{"id":entry["id"],"wait_ms":1000}}})).await);
            assert_eq!(answer["result"]["isError"], false, "{answer}");
            let read: Value = serde_json::from_str(
                answer["result"]["content"][0]["text"]
                    .as_str()
                    .expect("terminal read content"),
            )
            .expect("terminal read JSON");
            assert!(read.get("bytes").is_none());
            if read["terminal"]["exitCode"].is_u64() {
                assert_eq!(read["terminal"]["exitCode"], 0);
                assert!(
                    read["output"]
                        .as_str()
                        .expect("terminal output")
                        .contains("mcp-terminal-ok")
                );
                break;
            }
            assert!(Instant::now() < deadline);
        }
        s.workbench.shutdown().await;
        let refused = post(&at, &json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})).await;
        assert_eq!(refused.status, 401);
        assert!(
            s.workbench
                .threads()
                .expect("stored threads")
                .iter()
                .any(|t| t.id == thread.id)
        );
        let _ = std::fs::remove_dir_all(s.root);
    }
}

#[tokio::test]
async fn first_start_falls_back_to_the_global_choice_until_a_project_saves_its_own() -> nexus_workbench::Result<()> {
    use nexus_workbench::session_choice::Preference;

    let s = setup_with("global-first-start", bounded(), false, |config| Config {
        subsessions_first_start: Preference::Here,
        ..config
    });
    let wb = &s.workbench;
    assert_eq!(wb.session_start_preference(&s.project)?, Preference::Here);
    assert_eq!(wb.session_start_overrides(false)?, 0);

    wb.set_session_start_preference(&s.project, Preference::Ask)?;
    assert_eq!(wb.session_start_preference(&s.project)?, Preference::Ask);
    assert_eq!(wb.session_start_overrides(false)?, 1);

    assert_eq!(wb.session_start_overrides(true)?, 0);
    assert_eq!(wb.session_start_preference(&s.project)?, Preference::Here);
    wb.shutdown().await;
    Ok(())
}

use std::time::{Duration, Instant};

use nexus_workbench::event::{Sender, SessionAgent, WbEvent};
use nexus_workbench::linked::results::{RESULTS_HEADER, receipt_of};
use nexus_workbench::orchestra::{TOKEN_VAR, Timing, allowed_tools};
use nexus_workbench::{Config, Limits, NewThread, Wait, WaitMode};
use serde_json::{Value, json};
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// What a stand-in was started with.
struct Launched {
    args: Vec<String>,
    env: serde_json::Map<String, Value>,
}

impl Launched {
    fn env(&self, name: &str) -> Option<&str> {
        self.env.get(name).and_then(Value::as_str)
    }
}

/// Where the server is for one run, and its token.
#[derive(Clone)]
struct Endpoint {
    port: u16,
    path: String,
    token: String,
}

/// A session at work whose run has the tools.
struct Lead {
    s: Setup,
    id: String,
    at: Endpoint,
}

fn record(s: &Setup) -> std::path::PathBuf {
    s.root.join("launch.json")
}

/// Every launch from here on records what it was started with in `record(s)`.
fn record_launches(s: &Setup) {
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_LAUNCH_RECORD".into(), record(s).to_string_lossy().into_owned());
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("FAKE_RECORD_SECRETS".into(), "1".into());
}

/// The launch recorded last, once `ready` holds for it.
async fn launched(s: &Setup, ready: impl Fn(&Launched) -> bool) -> Launched {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = std::fs::read_to_string(record(s))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Launched {
                args: value["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect(),
                env: value["env"].as_object().cloned().unwrap_or_default(),
            });
        if let Some(found) = found.filter(&ready) {
            return found;
        }
        assert!(Instant::now() < deadline, "no launch recorded");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The server a launch was given, from Claude Code's `--mcp-config` or Codex's `-c` override.
fn endpoint(launch: &Launched) -> Option<Endpoint> {
    let url = launch.args.iter().find_map(|arg| {
        if let Some(config) = arg.strip_prefix("--mcp-config=") {
            let config: Value = serde_json::from_str(config).ok()?;
            return config.pointer("/mcpServers/nexus_app/url")?.as_str().map(String::from);
        }
        let table = arg.strip_prefix("mcp_servers.nexus_app={url=\"")?;
        table.split_once('"').map(|(url, _)| url.to_string())
    })?;
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let (port, path) = rest.split_once('/')?;
    Some(Endpoint {
        port: port.parse().ok()?,
        path: format!("/{path}"),
        token: launch.env(TOKEN_VAR)?.to_string(),
    })
}

fn timing() -> Timing {
    Timing {
        header: Duration::from_millis(300),
        body: Duration::from_millis(300),
        keep_alive: Duration::from_millis(50),
    }
}

fn setup_timed(name: &str, timing: Timing) -> Setup {
    let s = setup_with(name, bounded(), true, |config| Config {
        orchestra_timing: timing,
        ..config
    });
    s.workbench
        .set_session_start_preference(&s.project, nexus_workbench::session_choice::Preference::New)
        .expect("valid test fixture");
    record_launches(&s);
    s
}

/// A root with orchestration on running `prompt`, and the server its run was given. With `relay`, its turn is still
/// under way; otherwise its first turn has ended and its agent waits for the next message.
async fn lead_with(name: &str, timing: Timing, prompt: &str) -> Lead {
    let s = setup_timed(name, timing);
    let id = if prompt.contains("relay") {
        root_at_work(&s, new_root(&s.project, prompt)).await.id
    } else {
        root(&s, new_root(&s.project, prompt)).await.id
    };
    let at = endpoint(&launched(&s, |l| endpoint(l).is_some()).await).unwrap_or_else(|| panic!("no server"));
    Lead { s, id, at }
}

async fn lead(name: &str) -> Lead {
    lead_with(name, timing(), "Plan the release").await
}

async fn done(lead: Lead) {
    lead.s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&lead.s.root);
}

/// A request's bytes: `head` lines (each `Name: value`) after the request line, then `body`.
fn raw(method: &str, path: &str, head: &[String], body: &str) -> Vec<u8> {
    let mut text = format!("{method} {path} HTTP/1.1\r\n");
    for line in head {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.push_str("\r\n");
    text.push_str(body);
    text.into_bytes()
}

/// The head of a well-formed call on `at`, for a body of `length` bytes.
fn good_head(at: &Endpoint, length: usize) -> Vec<String> {
    vec![
        format!("Host: 127.0.0.1:{}", at.port),
        format!("Authorization: Bearer {}", at.token),
        "Content-Type: application/json".into(),
        format!("Content-Length: {length}"),
        "Connection: close".into(),
    ]
}

/// `head` with the line for `name` replaced by `line` (or dropped, when `line` is empty).
fn with(head: &[String], name: &str, line: &str) -> Vec<String> {
    let mut head: Vec<String> = head
        .iter()
        .filter(|l| {
            !l.to_ascii_lowercase()
                .starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
        .cloned()
        .collect();
    if !line.is_empty() {
        head.push(line.into());
    }
    head
}

struct Answer {
    status: u16,
    head: String,
    body: String,
}

/// Sends `bytes` and reads the answer to the end (the server closes after it).
async fn exchange(port: u16, bytes: &[u8]) -> Answer {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    stream.write_all(bytes).await.unwrap_or_else(|e| panic!("{e}"));
    read_answer(&mut stream).await
}

async fn read_answer(stream: &mut TcpStream) -> Answer {
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut got))
        .await
        .unwrap_or_else(|_| panic!("no answer: {}", String::from_utf8_lossy(&got)))
        .unwrap_or_else(|e| panic!("{e}"));
    let text = String::from_utf8_lossy(&got).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    Answer {
        status,
        head: head.to_string(),
        body: body.to_string(),
    }
}

/// A JSON-RPC message posted well-formed to `at`.
async fn post(at: &Endpoint, message: &Value) -> Answer {
    let body = message.to_string();
    exchange(at.port, &raw("POST", &at.path, &good_head(at, body.len()), &body)).await
}

fn ping() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
}

/// The JSON-RPC answer in a JSON body.
fn rpc(answer: &Answer) -> Value {
    assert_eq!(answer.status, 200, "{}\n{}", answer.head, answer.body);
    serde_json::from_str(&answer.body).unwrap_or_else(|e| panic!("{e}: {}", answer.body))
}

#[tokio::test]
async fn a_host_other_than_the_loopback_and_its_port_is_forbidden() {
    let lead = lead("host").await;
    let at = &lead.at;
    let body = ping().to_string();
    for host in [
        format!("Host: localhost:{}", at.port),
        format!("Host: 127.0.0.1:{}", at.port + 1),
        "Host: 127.0.0.1".into(),
        "Host: evil.test".into(),
    ] {
        let head = with(&good_head(at, body.len()), "Host", &host);
        let answer = exchange(at.port, &raw("POST", &at.path, &head, &body)).await;
        assert_eq!(answer.status, 403, "{host}");
    }
    assert_eq!(rpc(&post(at, &ping()).await)["result"], json!({}));
    done(lead).await;
}

#[tokio::test]
async fn an_origin_is_forbidden_and_no_answer_carries_a_cors_header() {
    let lead = lead("origin").await;
    let at = &lead.at;
    let body = ping().to_string();
    let mut head = good_head(at, body.len());
    head.push("Origin: http://127.0.0.1:3000".into());
    let refused = exchange(at.port, &raw("POST", &at.path, &head, &body)).await;
    assert_eq!(refused.status, 403);
    let options = exchange(at.port, &raw("OPTIONS", &at.path, &good_head(at, 0), "")).await;
    let ok = post(at, &ping()).await;
    for answer in [&refused, &options, &ok] {
        assert!(
            !answer.head.to_ascii_lowercase().contains("access-control"),
            "{}",
            answer.head
        );
    }
    done(lead).await;
}

#[tokio::test]
async fn an_unknown_agent_id_is_unauthorized() {
    let lead = lead("unknown-id").await;
    let at = &lead.at;
    let body = ping().to_string();
    for path in ["/mcp/0123456789abcdef0123456789abcdef", "/mcp/", "/", "/other"] {
        let answer = exchange(at.port, &raw("POST", path, &good_head(at, body.len()), &body)).await;
        assert_eq!(answer.status, 401, "{path}");
    }
    let bare = with(&good_head(at, body.len()), "Authorization", "");
    assert_eq!(
        exchange(at.port, &raw("POST", &at.path, &bare, &body)).await.status,
        401
    );
    done(lead).await;
}

#[tokio::test]
async fn a_wrong_token_is_unauthorized_before_any_body_byte() {
    let lead = lead("wrong-token").await;
    let at = &lead.at;
    // The head promises a body that never comes: the answer can't have waited for it.
    let head = with(
        &good_head(at, 100),
        "Authorization",
        &format!("Authorization: Bearer {}", "f".repeat(64)),
    );
    let mut stream = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    stream
        .write_all(&raw("POST", &at.path, &head, ""))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let wrong = read_answer(&mut stream).await;
    assert_eq!(wrong.status, 401);
    // The same answer as an unknown id's.
    let unknown = exchange(at.port, &raw("POST", "/mcp/nobody", &good_head(at, 0), "")).await;
    assert_eq!((unknown.status, unknown.body), (401, wrong.body));
    done(lead).await;
}

#[tokio::test]
async fn get_and_delete_are_not_allowed() {
    let lead = lead("methods").await;
    let at = &lead.at;
    for method in ["GET", "DELETE", "PUT"] {
        let answer = exchange(at.port, &raw(method, &at.path, &good_head(at, 0), "")).await;
        assert_eq!(answer.status, 405, "{method}");
        assert!(
            answer.head.contains("allow: POST") || answer.head.contains("Allow: POST"),
            "{}",
            answer.head
        );
    }
    done(lead).await;
}

#[tokio::test]
async fn a_body_with_no_length_is_refused() {
    let lead = lead("no-length").await;
    let at = &lead.at;
    let head = with(&good_head(at, 0), "Content-Length", "");
    assert_eq!(exchange(at.port, &raw("POST", &at.path, &head, "")).await.status, 411);
    done(lead).await;
}

#[tokio::test]
async fn a_body_over_128_kib_is_refused_unread() {
    let lead = lead("too-large").await;
    let at = &lead.at;
    // Nothing of it is sent: the length alone refuses it.
    let head = good_head(at, 128 * 1024 + 1);
    assert_eq!(exchange(at.port, &raw("POST", &at.path, &head, "")).await.status, 413);
    done(lead).await;
}

#[tokio::test]
async fn a_body_that_is_not_json_is_refused() {
    let lead = lead("not-json").await;
    let at = &lead.at;
    let body = ping().to_string();
    for kind in ["Content-Type: text/plain", "Content-Type: application/jsonx", ""] {
        let head = with(&good_head(at, body.len()), "Content-Type", kind);
        assert_eq!(
            exchange(at.port, &raw("POST", &at.path, &head, &body)).await.status,
            415,
            "{kind:?}"
        );
    }
    let charset = with(
        &good_head(at, body.len()),
        "Content-Type",
        "Content-Type: application/json; charset=utf-8",
    );
    assert_eq!(
        exchange(at.port, &raw("POST", &at.path, &charset, &body)).await.status,
        200
    );
    // One JSON object: not a batch, not text.
    for body in ["[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]", "ping", "1"] {
        let answer = exchange(at.port, &raw("POST", &at.path, &good_head(at, body.len()), body)).await;
        assert_eq!(answer.status, 400, "{body}");
    }
    done(lead).await;
}

#[tokio::test]
async fn a_chunked_body_is_refused() {
    let lead = lead("chunked").await;
    let at = &lead.at;
    let body = ping().to_string();
    let chunked = format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len());
    let alone = with(&good_head(at, 0), "Content-Length", "Transfer-Encoding: chunked");
    let answer = exchange(at.port, &raw("POST", &at.path, &alone, &chunked)).await;
    assert_eq!(answer.status, 411);
    // With a length too: HTTP lets the chunking win and drops the length, so it is refused the same way.
    let mut both = good_head(at, chunked.len());
    both.push("Transfer-Encoding: chunked".into());
    let answer = exchange(at.port, &raw("POST", &at.path, &both, &chunked)).await;
    assert!([400, 411].contains(&answer.status), "{}", answer.status);
    done(lead).await;
}

#[tokio::test]
async fn a_request_takes_no_place_in_flight_before_its_body_is_read() {
    // The fifth tool call's 429 is `http::tests`: four calls held open at once need a slow tool, and there is none.
    let lead = lead_with(
        "in-flight",
        Timing {
            body: Duration::from_secs(10),
            ..timing()
        },
        "Plan the release",
    )
    .await;
    let at = &lead.at;
    // More requests than the cap, authenticated, whose bodies are still on their way.
    let mut held = Vec::new();
    for _ in 0..6 {
        let mut stream = TcpStream::connect(("127.0.0.1", at.port))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        stream
            .write_all(&raw("POST", &at.path, &good_head(at, 50), ""))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        held.push(stream);
    }
    // None of them takes a place: a tool call, a cancel and a ping all go through.
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "list_sessions", "arguments": {}}});
    assert_eq!(rpc(&post(at, &call).await)["result"]["isError"], false);
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 9}});
    assert_eq!(post(at, &cancel).await.status, 202);
    assert_eq!(rpc(&post(at, &ping()).await)["result"], json!({}));
    drop(held);
    done(lead).await;
}

#[tokio::test]
async fn a_slow_head_is_closed() {
    let lead = lead("slow-head").await;
    let at = &lead.at;
    let mut stream = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let partial = format!("POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", at.path, at.port);
    stream
        .write_all(partial.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let started = Instant::now();
    let mut got = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut got)).await;
    assert!(closed.is_ok(), "still open after {:?}", started.elapsed());
    assert!(!String::from_utf8_lossy(&got).contains(" 200 "), "{got:?}");
    done(lead).await;
}

#[tokio::test]
async fn a_slow_body_is_closed() {
    let lead = lead("slow-body").await;
    let at = &lead.at;
    let mut stream = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    stream
        .write_all(&raw("POST", &at.path, &good_head(at, 20), "{\""))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let answer = read_answer(&mut stream).await;
    assert_eq!(answer.status, 408);
    done(lead).await;
}

#[tokio::test]
async fn the_seventeenth_unauthenticated_connection_is_refused() {
    let lead = lead_with(
        "connections",
        Timing {
            header: Duration::from_secs(10),
            ..timing()
        },
        "Plan the release",
    )
    .await;
    let at = &lead.at;
    let mut idle = Vec::new();
    for _ in 0..16 {
        idle.push(
            TcpStream::connect(("127.0.0.1", at.port))
                .await
                .unwrap_or_else(|e| panic!("{e}")),
        );
    }
    let mut seventeenth = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut got = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), seventeenth.read_to_end(&mut got)).await;
    assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "{closed:?}");
    // An idle one that goes gives its place back.
    drop(idle.pop());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut stream = TcpStream::connect(("127.0.0.1", at.port))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let body = ping().to_string();
        if stream
            .write_all(&raw("POST", &at.path, &good_head(at, body.len()), &body))
            .await
            .is_ok()
        {
            let mut got = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut got)).await;
            if String::from_utf8_lossy(&got).starts_with("HTTP/1.1 200") {
                break;
            }
        }
        assert!(Instant::now() < deadline, "no place came back");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    done(lead).await;
}

#[tokio::test]
async fn app_tool_instructions_and_claude_allowlist_match_discovery() {
    let limits = Limits {
        max_depth: Some(1),
        ..bounded()
    };
    let s = setup_with("tool-advertisement", limits, true, |config| Config {
        terminal: true,
        ..config
    });
    record_launches(&s);

    let lead = root(&s, new_root(&s.project, "Plan the release")).await;
    assert_app_tools(&s, "orchestration on", true).await;

    std::fs::remove_file(record(&s)).expect("clear the lead's launch record");
    let child = start(&s, &lead.id, SessionAgent::Claude, "Child", "Check the release").await;
    assert!(thread(&s.workbench, &child.id).at_depth_limit);
    assert_app_tools(&s, "at the depth limit", false).await;

    std::fs::remove_file(record(&s)).expect("clear the child's launch record");
    let mut standalone = new_root(&s.project, "Check the release");
    standalone.orchestration = false;
    root(&s, standalone).await;
    assert_app_tools(&s, "orchestration off", false).await;

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

async fn assert_app_tools(s: &Setup, case: &str, crew: bool) {
    let launch = launched(s, |l| endpoint(l).is_some()).await;
    let at = endpoint(&launch).expect("nexus_app MCP server");
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
    let response = rpc(&post(&at, &init).await);
    let instructions = response["result"]["instructions"].as_str().expect("MCP instructions");
    assert!(instructions.contains("terminal_run"), "{case}");
    assert_eq!(instructions.contains("starting subsessions"), crew, "{case}");

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let response = rpc(&post(&at, &list).await);
    let names: Vec<_> = response["result"]["tools"]
        .as_array()
        .expect("MCP tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("MCP tool name"))
        .collect();
    assert!(names.contains(&"terminal_run"), "{case}");
    for name in nexus_workbench::orchestra::tools::NAMES {
        assert_eq!(names.contains(&name), crew, "{case}: {name}");
    }
    let mut expected: Vec<_> = names.iter().map(|name| format!("mcp__nexus_app__{name}")).collect();
    let mut allowed: Vec<_> = launch
        .args
        .iter()
        .find_map(|arg| arg.strip_prefix("--allowedTools="))
        .expect("Claude Code tool allowance")
        .split(',')
        .collect();
    expected.sort_unstable();
    allowed.sort_unstable();
    assert_eq!(allowed, expected, "{case}");
}

#[tokio::test]
async fn speaks_mcp_over_json_rpc() {
    let lead = lead("json-rpc").await;
    let at = &lead.at;
    let init = |version: &str| {
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})
    };
    let answer = rpc(&post(at, &init("2025-06-18")).await);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["capabilities"], json!({"tools": {}}));
    assert_eq!(answer["result"]["serverInfo"]["name"], "nexus_app");
    let instructions = answer["result"]["instructions"].as_str().expect("MCP instructions");
    assert!(instructions.contains("starting subsessions"));
    assert!(instructions.contains("may run another agent") && instructions.contains("on the person's own sign-in"));
    assert!(instructions.contains("can start subsessions of its own"));
    assert!(!instructions.contains("terminal_run"));
    assert_eq!(
        rpc(&post(at, &init("2025-03-26")).await)["result"]["protocolVersion"],
        "2025-03-26"
    );
    assert_eq!(
        rpc(&post(at, &init("1999-01-01")).await)["result"]["protocolVersion"],
        "2025-11-25"
    );
    let note = post(at, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    assert_eq!((note.status, note.body.as_str()), (202, ""));
    let tools = rpc(&post(at, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await);
    let names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(|name| format!("mcp__nexus_app__{name}")))
        .collect();
    assert_eq!(names, allowed_tools());
    let unknown = rpc(&post(at, &json!({"jsonrpc": "2.0", "id": 3, "method": "server/discover"})).await);
    assert_eq!(unknown["error"]["code"], -32601);
    let call = |id: u64, name: &str, args: Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let no_tool = rpc(&post(at, &call(4, "rm_rf", json!({}))).await);
    assert_eq!(no_tool["error"]["code"], -32602);
    let listed = rpc(&post(at, &call(5, "list_sessions", json!({}))).await);
    assert_eq!(listed["result"]["isError"], false);
    let text: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap_or_default()).unwrap_or_default();
    assert_eq!(text["sessions"], json!([]));
    assert_eq!(text["limits"]["starts_left"], 8);
    let refused = rpc(&post(at, &call(6, "start_session", json!({"agent": "gemini", "prompt": "x"}))).await);
    assert_eq!(refused["result"]["isError"], true);
    let text: Value =
        serde_json::from_str(refused["result"]["content"][0]["text"].as_str().unwrap_or_default()).unwrap_or_default();
    assert_eq!(text["code"], "bad_input");
    // A wait always answers as a stream; from a session with no turn under way, its result is a refusal.
    let idle = post(at, &call(7, "wait_sessions", json!({}))).await;
    assert_eq!(idle.status, 200);
    let idle: Value = idle
        .body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .and_then(|data| serde_json::from_str(data).ok())
        .unwrap_or_else(|| panic!("{}", idle.body));
    assert_eq!(idle["id"], 7);
    assert_eq!(idle["result"]["isError"], true, "{idle}");
    // One with bad input is refused at once, as JSON.
    let bad = rpc(&post(at, &call(8, "wait_sessions", json!({"timeout_s": "soon"}))).await);
    assert_eq!(bad["result"]["isError"], true, "{bad}");
    assert_eq!(
        rpc(&post(at, &json!({"jsonrpc": "1.0", "id": 10, "method": "ping"})).await)["error"]["code"],
        -32600
    );
    done(lead).await;
}

#[tokio::test]
async fn a_token_from_an_older_run_is_unauthorized() {
    let lead = lead("older-run").await;
    let old = lead.at.clone();
    assert_eq!(post(&old, &ping()).await.status, 200);
    lead.s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
    // Refused from the moment the stop began, before its agent is gone.
    assert_eq!(post(&old, &ping()).await.status, 401);
    until_stopped(&lead.s.workbench, &lead.id).await;
    lead.s
        .workbench
        .send(&lead.id, "Again")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let new = endpoint(&launched(&lead.s, |l| endpoint(l).is_some_and(|at| at.token != old.token)).await)
        .unwrap_or_else(|| panic!("no server"));
    assert_ne!(new.path, old.path);
    assert_eq!(new.port, old.port, "one server per workbench");
    assert_eq!(post(&old, &ping()).await.status, 401);
    let mixed = Endpoint {
        token: old.token.clone(),
        ..new.clone()
    };
    assert_eq!(post(&mixed, &ping()).await.status, 401);
    assert_eq!(post(&new, &ping()).await.status, 200);
    done(lead).await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_agent_that_exits_by_itself_loses_its_grant_at_once() {
    let s = setup_timed("exits", timing());
    // It exits as it reads its prompt, and leaves a process out of its group that holds its output: the run reads
    // that output for up to `DRAIN` more before it ends. That process has the token too.
    let lead = s
        .workbench
        .create_thread(new_root(&s.project, "orphan holdout detach"))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let at = endpoint(&launched(&s, |l| endpoint(l).is_some()).await).unwrap_or_else(|| panic!("no server"));
    let pid_file = s.root.join("repo").join("orphan.pid");
    until("the agent exits", || pid_file.exists()).await;
    let exited = Instant::now();
    loop {
        let status = post(&at, &ping()).await.status;
        if status == 401 {
            break;
        }
        assert_eq!(status, 200);
        assert!(exited.elapsed() < Duration::from_secs(10), "the token still works");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let took = exited.elapsed();
    until_stopped(&s.workbench, &lead.id).await;
    if let Some(pid) = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|pid| pid.trim().parse::<i32>().ok())
    {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL);
    }
    // Refused when the agent exited, not when the run ended after its output.
    assert!(
        took < nexus_workbench::process::DRAIN / 2,
        "refused only after {took:?}"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A wait from the lead's run, posted by hand: the open stream, read until its first bytes.
async fn open_wait(at: &Endpoint, ids: &[&str], progress: Option<&str>) -> TcpStream {
    let mut params = json!({"name": "wait_sessions", "arguments": {"session_ids": ids, "timeout_s": 60}});
    if let Some(token) = progress {
        params["_meta"] = json!({"progressToken": token});
    }
    let body = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": params}).to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", at.port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    stream
        .write_all(&raw("POST", &at.path, &good_head(at, body.len()), &body))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let head = read_until(&mut stream, ": open").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("content-type: text/event-stream"),
        "{head}"
    );
    stream
}

/// Reads until what was read holds `text`, for up to ten seconds.
async fn read_until(stream: &mut TcpStream, text: &str) -> String {
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&got).contains(text) {
        let mut buffer = [0u8; 4096];
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.read(&mut buffer)).await {
            Ok(Ok(read)) if read > 0 => got.extend_from_slice(&buffer[..read]),
            other => panic!("{other:?} before {text:?}; read {:?}", String::from_utf8_lossy(&got)),
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test]
async fn a_dropped_wait_stream_puts_its_result_back() {
    let lead = lead_with("dropped-wait", timing(), "relay Plan the release").await;
    let s = &lead.s;
    let a = start(s, &lead.id, SessionAgent::Claude, "Builder", "Build it").await;
    let b = start(s, &lead.id, SessionAgent::Claude, "Holder", "slow Hold on").await;
    wait_for(&s.workbench, &a.id, |e| turn_ends(e).len() == 1).await;
    // The wait holds a's result and waits on for b; then the agent's connection goes.
    let stream = open_wait(&lead.at, &[&a.id, &b.id], None).await;
    until("the wait is open", || {
        !thread(&s.workbench, &lead.id).waits_on.is_empty()
    })
    .await;
    drop(stream);
    until("the wait is dropped", || {
        thread(&s.workbench, &lead.id).waits_on.is_empty()
    })
    .await;
    // Back to pending: the next wait carries it.
    let next = s
        .workbench
        .wait_children(
            &lead.id,
            Wait {
                session_ids: Some(vec![a.id.clone()]),
                timeout: Duration::from_secs(30),
                mode: WaitMode::Any,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(next.results.len(), 1, "{next:?}");
    assert_eq!(next.results[0].session_id, a.id);
    assert!(has_result(&s.workbench, &lead.id, &a.id));
    s.workbench.interrupt(&b.id).unwrap_or_else(|e| panic!("{e}"));
    end_relay(s, &lead.id).await;
    done(lead).await;
}

#[tokio::test]
async fn a_long_wait_keeps_its_stream_alive() {
    let lead = lead_with("keep-alive", timing(), "relay Plan the release").await;
    let s = &lead.s;
    let b = start(s, &lead.id, SessionAgent::Claude, "Holder", "slow Hold on").await;
    // With the call's progress token, each keep-alive is a progress notification.
    let mut stream = open_wait(&lead.at, &[&b.id], Some("p-1")).await;
    let read = read_until(&mut stream, "notifications/progress").await;
    assert!(read.contains("\"progressToken\":\"p-1\""), "{read}");
    drop(stream);
    until("the wait is dropped", || {
        thread(&s.workbench, &lead.id).waits_on.is_empty()
    })
    .await;
    // Without one, a comment.
    let mut stream = open_wait(&lead.at, &[&b.id], None).await;
    read_until(&mut stream, ": keep-alive").await;
    // The result last, then the stream ends.
    s.workbench.interrupt(&b.id).unwrap_or_else(|e| panic!("{e}"));
    let read = read_until(&mut stream, "</nexus-session-result>").await;
    assert!(read.contains("\"jsonrpc\":\"2.0\",\"id\":9"), "{read}");
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
    assert!(ended.is_ok(), "the stream stayed open");
    end_relay(s, &lead.id).await;
    done(lead).await;
}

#[tokio::test]
async fn each_agent_is_started_with_the_server_and_its_token_only_in_its_environment() {
    let s = setup_timed("injection", timing());
    s.host
        .extra
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert("NO_PROXY".into(), "corp.test".into());
    for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus] {
        let _ = std::fs::remove_file(record(&s));
        let lead = root(
            &s,
            NewThread {
                name: None,
                agent,
                ..new_root(&s.project, "Plan the release")
            },
        )
        .await;
        let launch = launched(&s, |_| true).await;
        let at = endpoint(&launch).unwrap_or_else(|| panic!("{agent:?} has no server"));
        let id = at.path.strip_prefix("/mcp/").unwrap_or_default();
        assert_eq!(id.len(), 32);
        let url = format!("http://127.0.0.1:{}/mcp/{id}", at.port);
        match agent {
            // Nexus's agent takes Claude Code's options.
            SessionAgent::Claude | SessionAgent::Nexus => {
                let config = format!(
                    "--mcp-config={{\"mcpServers\":{{\"nexus_app\":{{\"type\":\"http\",\"url\":\"{url}\",\
                     \"headers\":{{\"Authorization\":\"Bearer ${{NEXUS_APP_MCP_TOKEN}}\"}}}}}}}}"
                );
                let allowed = format!("--allowedTools={}", allowed_tools().join(","));
                assert_eq!(launch.args[launch.args.len() - 2..], [config, allowed]);
                assert_eq!(launch.env("MCP_TOOL_TIMEOUT"), Some("660000"));
            }
            SessionAgent::Codex => {
                let server = format!(
                    "mcp_servers.nexus_app={{url=\"{url}\",bearer_token_env_var=\"NEXUS_APP_MCP_TOKEN\",\
                     tool_timeout_sec=660,startup_timeout_sec=10,default_tools_approval_mode=\"approve\"}}"
                );
                let at_server = launch.args.iter().position(|arg| *arg == server).unwrap_or_else(|| {
                    panic!("{:?}", launch.args);
                });
                assert_eq!(launch.args[at_server - 1], "-c");
                assert_eq!(launch.args.last().map(String::as_str), Some("app-server"));
            }
            #[cfg(feature = "cursor")]
            SessionAgent::Cursor => unreachable!("not among the agents this test runs"),
            #[cfg(feature = "opencode")]
            SessionAgent::Opencode => unreachable!("not among the agents this test runs"),
            SessionAgent::Antigravity => unreachable!("not among the agents this test runs"),
        }
        assert_eq!(
            allowed_tools(),
            [
                "mcp__nexus_app__start_session",
                "mcp__nexus_app__message_session",
                "mcp__nexus_app__wait_sessions",
                "mcp__nexus_app__list_sessions",
                "mcp__nexus_app__stop_session",
                "mcp__nexus_app__handoff_session",
                "mcp__nexus_app__archive_session",
                "mcp__nexus_app__subscribe_session",
                "mcp__nexus_app__schedule_session",
                "mcp__nexus_app__list_schedules",
                "mcp__nexus_app__cancel_schedule",
            ]
        );
        assert_eq!(at.token.len(), 64);
        assert!(launch.args.iter().all(|arg| !arg.contains(&at.token)), "{agent:?}");
        assert_eq!(launch.env("NO_PROXY"), Some("corp.test,127.0.0.1"));
        assert_eq!(launch.env("no_proxy"), Some("corp.test,127.0.0.1"));
        assert_eq!(post(&at, &ping()).await.status, 200);
        s.workbench.stop(&lead.id).unwrap_or_else(|e| panic!("{e}"));
        until_stopped(&s.workbench, &lead.id).await;
    }
    // A session with orchestration off gets nothing.
    let _ = std::fs::remove_file(record(&s));
    root(
        &s,
        NewThread {
            name: None,
            orchestration: false,
            ..new_root(&s.project, "Plan the release")
        },
    )
    .await;
    let launch = launched(&s, |_| true).await;
    assert!(endpoint(&launch).is_none());
    assert!(launch.env(TOKEN_VAR).is_none());
    assert_eq!(launch.env("NO_PROXY"), Some("corp.test"));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_folder_whose_mcp_json_names_nexus_app_starts_claude_without_the_tools() {
    let s = setup_timed("mcp-json", timing());
    let project = s.workbench.project(&s.project).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        std::path::Path::new(&project.path).join(".mcp.json"),
        r#"{"mcpServers": {"nexus_app": {"type": "http", "url": "http://127.0.0.1:1/mcp"}}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let claude = root(&s, new_root(&s.project, "Plan the release")).await;
    let launch = launched(&s, |_| true).await;
    assert!(endpoint(&launch).is_none(), "{:?}", launch.args);
    assert!(launch.env(TOKEN_VAR).is_none());
    assert!(launch.args.iter().all(|arg| !arg.contains("--allowedTools")));
    let events = s.workbench.events(&claude.id, 0).unwrap_or_default();
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, WbEvent::Notice { text } if text.contains(".mcp.json"))),
        "{events:#?}"
    );
    // Codex does not read that file: it gets them.
    let _ = std::fs::remove_file(record(&s));
    root(
        &s,
        NewThread {
            name: None,
            agent: SessionAgent::Codex,
            ..new_root(&s.project, "Plan the release")
        },
    )
    .await;
    assert!(endpoint(&launched(&s, |_| true).await).is_some());
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A lead of `agent` that starts a child through the server, waits for it, and reads its result whole.
async fn end_to_end(name: &str, agent: SessionAgent) {
    let s = setup_timed(name, Timing::default());
    let lead = s
        .workbench
        .create_thread(NewThread {
            name: None,
            agent,
            ..new_root(&s.project, "orchestrate")
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() == 1).await;
    let results = tool_results(&events);
    assert_eq!(results.len(), 2, "{events:#?}");
    let (started, error) = &results[0];
    assert!(!error, "{started}");
    let started: Value = serde_json::from_str(started).unwrap_or_else(|e| panic!("{e}: {started}"));
    assert_eq!(started["status"], "starting");
    let child = started["session_id"].as_str().unwrap_or_default().to_string();
    let child_summary = thread(&s.workbench, &child);
    assert_eq!(child_summary.parent_thread_id.as_deref(), Some(lead.id.as_str()));
    assert_eq!(child_summary.agent, agent);
    let (answer, error) = &results[1];
    assert!(!error, "{answer}");
    assert!(receipt_of(answer).is_some(), "{answer}");
    assert!(answer.contains(RESULTS_HEADER), "{answer}");
    let reply = match agent {
        SessionAgent::Claude => "Message 1 of this session.",
        SessionAgent::Codex => "Message 1 of this thread.",
        SessionAgent::Nexus => "You said:",
        #[cfg(feature = "cursor")]
        SessionAgent::Cursor => unreachable!("not among the agents this test runs"),
        #[cfg(feature = "opencode")]
        SessionAgent::Opencode => unreachable!("not among the agents this test runs"),
        SessionAgent::Antigravity => unreachable!("not among the agents this test runs"),
    };
    assert!(
        answer.contains(&format!("session=\"{child}\"")) && answer.contains(reply),
        "{answer}"
    );
    // The tool result held the answer whole: the lead read it.
    until("the result is read", || !has_result(&s.workbench, &lead.id, &child)).await;
    assert_eq!(thread(&s.workbench, &lead.id).pending_results, 0);
    let events = s.workbench.events(&lead.id, 0).unwrap_or_default();
    assert!(
        users(&events).iter().all(|(_, from)| from.is_none()),
        "no result message"
    );
    // The lead called the two tools, in order, and nothing else.
    let names: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            WbEvent::Tool { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        ["mcp__nexus_app__start_session", "mcp__nexus_app__wait_sessions"]
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The person's one sentence to a session of `from`, and its handoff through the server to a session of `to`: the
/// old session is archived, and the new one starts with the handoff alone, from the old session.
async fn one_sentence_handoff(name: &str, from: SessionAgent, to: SessionAgent, sentence: &str) {
    let s = setup_timed(name, Timing::default());
    let old = s
        .workbench
        .create_thread(NewThread {
            name: None,
            agent: from,
            ..new_root(&s.project, sentence)
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &old.id, |e| turn_ends(e).len() == 1).await;
    let results = tool_results(&events);
    assert_eq!(results.len(), 1, "{events:#?}");
    let (answer, error) = &results[0];
    assert!(!error, "{answer}");
    let answer: Value = serde_json::from_str(answer).unwrap_or_else(|e| panic!("{e}: {answer}"));
    let next_id = answer["session_id"].as_str().unwrap_or_default().to_string();
    let next = thread(&s.workbench, &next_id);
    assert_eq!(next.agent, to);
    assert_eq!(next.handoff_from.map(|from| from.thread_id), Some(old.id.clone()));
    assert!(thread(&s.workbench, &old.id).archived);
    let events = wait_for(&s.workbench, &next_id, |e| turn_ends(e).len() == 1).await;
    let users = users(&events);
    assert_eq!(users.len(), 1, "only the handoff: {users:?}");
    assert!(!users[0].0.contains(sentence), "the conversation is not sent again");
    assert_eq!(
        users[0].1,
        Some(Sender::Session {
            thread_id: old.id.clone(),
            title: thread(&s.workbench, &old.id).title,
        })
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_claude_session_hands_off_to_codex_in_one_sentence() {
    one_sentence_handoff(
        "e2e-handoff-codex",
        SessionAgent::Claude,
        SessionAgent::Codex,
        "Spawn a Codex session, send a handoff there and archive yourself",
    )
    .await;
}

#[tokio::test]
async fn a_codex_session_hands_off_to_claude_in_one_sentence() {
    one_sentence_handoff(
        "e2e-handoff-claude",
        SessionAgent::Codex,
        SessionAgent::Claude,
        "Spawn a Claude session, send a handoff there and archive yourself",
    )
    .await;
}

#[tokio::test]
async fn a_claude_lead_starts_a_child_through_the_server_and_reads_its_result() {
    end_to_end("e2e-claude", SessionAgent::Claude).await;
}

#[tokio::test]
async fn a_codex_lead_starts_a_child_through_the_server_and_reads_its_result() {
    end_to_end("e2e-codex", SessionAgent::Codex).await;
}

#[tokio::test]
async fn a_nexus_lead_starts_a_child_through_the_server_and_reads_its_result() {
    end_to_end("e2e-nexus", SessionAgent::Nexus).await;
}

fn start_call(key: &str) -> Value {
    json!({"jsonrpc":"2.0","id":991,"method":"tools/call","params":{"name":"start_session","arguments":{"agent":"claude","title":"Sample child","prompt":"Build the changelog","key":key},"_meta":{"progressToken":"choice-test"}}})
}
fn stream_result(answer: &Answer) -> Value {
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer
        .body
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|v| serde_json::from_str::<Value>(v).ok())
        .find(|v| v.get("result").is_some())
        .unwrap_or_else(|| panic!("no result: {}", answer.body))
}
fn result_data(answer: &Answer) -> Value {
    let rpc = stream_result(answer);
    assert_eq!(rpc["result"]["isError"], false, "{rpc}");
    serde_json::from_str(
        rpc["result"]["content"][0]["text"]
            .as_str()
            .expect("valid test fixture"),
    )
    .expect("valid test fixture")
}
async fn pending_choice(lead: &Lead) -> nexus_workbench::session_choice::StartChoice {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(choice) = thread(&lead.s.workbench, &lead.id).start_choices.into_iter().next() {
            return choice;
        }
        assert!(Instant::now() < deadline, "no choice");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn open_start(lead: &Lead, key: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", lead.at.port))
        .await
        .expect("valid test fixture");
    let body = start_call(key).to_string();
    stream
        .write_all(&raw("POST", &lead.at.path, &good_head(&lead.at, body.len()), &body))
        .await
        .expect("valid test fixture");
    stream
}
fn ask(lead: &Lead) {
    lead.s
        .workbench
        .set_session_start_preference(&lead.s.project, nexus_workbench::session_choice::Preference::Ask)
        .expect("valid test fixture");
}

/// A `tools/call` of `name` with `arguments`.
fn tool_call(name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":992,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}

/// A `tools/call` answer: whether it is a tool error, and the JSON in its text.
fn tool_answer(answer: &Answer) -> (bool, Value) {
    let result = &rpc(answer)["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let data = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (result["isError"] == true, data)
}

#[tokio::test]
async fn a_lead_messages_its_child_through_the_server() {
    let lead = lead("message-http").await;
    let c = finished_child(&lead.s, &lead.id, child(SessionAgent::Claude, "Build it")).await;
    let args = json!({"session_id": c.id, "text": "And the tests"});
    let (error, data) = tool_answer(&post(&lead.at, &tool_call("message_session", &args)).await);
    assert!(!error, "{data}");
    assert_eq!(data, json!({"session_id": c.id, "status": "started"}));
    let events = wait_for(&lead.s.workbench, &c.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(users(&events)[1].0, "And the tests");
    // The lead is not its own child.
    let args = json!({"session_id": lead.id, "text": "Hi"});
    let (error, data) = tool_answer(&post(&lead.at, &tool_call("message_session", &args)).await);
    assert!(error, "{data}");
    assert_eq!(data["code"], "not_your_session", "{data}");
    done(lead).await;
}

#[tokio::test]
async fn a_lead_stops_its_running_child_through_the_server() {
    let lead = lead("stop-http").await;
    let c = start(&lead.s, &lead.id, SessionAgent::Claude, "Holder", "slow Hold on").await;
    wait_for(&lead.s.workbench, &c.id, |e| {
        e.iter().any(|e| matches!(e.event, WbEvent::TurnStart { .. }))
    })
    .await;
    let (error, data) = tool_answer(&post(&lead.at, &tool_call("stop_session", &json!({"session_id": c.id}))).await);
    assert!(!error, "{data}");
    assert_eq!(
        data,
        json!({"session_id": c.id, "status": "stopped", "was_running": true})
    );
    until_stopped(&lead.s.workbench, &c.id).await;
    // A parent's stop is no halt: the lead may message the child again.
    assert_eq!(lead.s.workbench.halt(&c.id).unwrap_or_else(|e| panic!("{e}")), None);
    let args = json!({"session_id": c.id, "text": "Go on"});
    let (error, data) = tool_answer(&post(&lead.at, &tool_call("message_session", &args)).await);
    assert!(!error, "{data}");
    done(lead).await;
}

/// A fresh app has no catalog until it asks for one: a start through the server loads it first, as `start_child` does,
/// instead of refusing the first child with `unpriced_model`.
#[tokio::test]
async fn a_start_loads_the_price_catalog_before_it_admits_the_child() {
    let lead = lead("choice-prices").await;
    lead.s.workbench.set_orchestration_limits(nexus_workbench::Limits {
        crew_spend_usd: Some(20.0),
        ..Default::default()
    });
    lead.s.host.no_prices.store(true, std::sync::atomic::Ordering::SeqCst);
    lead.s
        .host
        .prices_on_ensure
        .store(true, std::sync::atomic::Ordering::SeqCst);
    ask(&lead);
    let _stream = open_start(&lead, "priced").await;
    assert_eq!(pending_choice(&lead).await.title, "Sample child");
    done(lead).await;
}

#[tokio::test]
async fn a_start_waits_for_the_person_and_here_is_structured_and_idempotent() {
    use nexus_workbench::session_choice::Preference;
    let lead = lead("choice-here").await;
    ask(&lead);
    let mut stream = open_start(&lead, "here-once").await;
    let choice = pending_choice(&lead).await;
    assert_eq!(choice.title, "Sample child");
    assert_eq!(
        choice.permission_mode,
        nexus_workbench::event::PermissionMode::AcceptEdits
    );
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("valid test fixture")
            .sessions
            .is_empty()
    );
    // Long enough for progress frames, which keep both agents' MCP connections alive.
    tokio::time::sleep(Duration::from_millis(120)).await;
    lead.s
        .workbench
        .answer_session_start(&lead.id, &choice.id, Preference::Here, true)
        .expect("valid test fixture");
    let answer = read_answer(&mut stream).await;
    assert!(answer.body.contains("notifications/progress"));
    assert_eq!(result_data(&answer)["choice"], "here");
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("valid test fixture")
            .sessions
            .is_empty()
    );
    assert_eq!(
        lead.s
            .workbench
            .session_start_preference(&lead.s.project)
            .expect("valid test fixture"),
        Preference::Here
    );
    ask(&lead);
    assert_eq!(
        result_data(&post(&lead.at, &start_call("here-once")).await)["choice"],
        "here"
    );
    assert!(thread(&lead.s.workbench, &lead.id).start_choices.is_empty());
    done(lead).await;
}

#[tokio::test]
async fn new_starts_exactly_one_child_and_remembered_new_still_enforces_plan_mode() {
    use nexus_workbench::session_choice::Preference;
    let lead = lead("choice-new").await;
    ask(&lead);
    let mut stream = open_start(&lead, "new-once").await;
    let choice = pending_choice(&lead).await;
    lead.s
        .workbench
        .answer_session_start(&lead.id, &choice.id, Preference::New, true)
        .expect("valid test fixture");
    let started = result_data(&read_answer(&mut stream).await);
    let id = started["session_id"].as_str().expect("valid test fixture").to_string();
    // The answer says what the child's agent runs with: Claude Code's own setting for the child's mode.
    let mode = thread(&lead.s.workbench, &id).permission_mode;
    assert_eq!(started["runsAs"], json!({"--permission-mode": mode.as_str()}));
    let retried = result_data(&post(&lead.at, &start_call("new-once")).await);
    assert_eq!(retried["session_id"], id);
    assert_eq!(retried["runsAs"], started["runsAs"]);
    assert_eq!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("valid test fixture")
            .sessions
            .len(),
        1
    );
    lead.s
        .workbench
        .update(
            &lead.id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(nexus_workbench::event::PermissionMode::Plan),
                ..Default::default()
            },
        )
        .expect("valid test fixture");
    let refused = stream_result(&post(&lead.at, &start_call("plan-refused")).await);
    assert_eq!(refused["result"]["isError"], true);
    done(lead).await;
}

#[tokio::test]
async fn cancelling_a_pending_start_removes_the_card_and_never_creates_a_child() {
    let lead = lead("choice-cancel").await;
    ask(&lead);
    let mut stream = open_start(&lead, "cancelled").await;
    let choice = pending_choice(&lead).await;
    post(
        &lead.at,
        &json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":991}}),
    )
    .await;
    let _ = read_answer(&mut stream).await;
    assert!(thread(&lead.s.workbench, &lead.id).start_choices.is_empty());
    assert!(
        lead.s
            .workbench
            .answer_session_start(
                &lead.id,
                &choice.id,
                nexus_workbench::session_choice::Preference::New,
                false
            )
            .is_err()
    );
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("valid test fixture")
            .sessions
            .is_empty()
    );
    done(lead).await;
}

#[tokio::test]
async fn stopping_the_parent_revokes_its_pending_start() {
    let lead = lead("choice-stop").await;
    ask(&lead);
    let mut stream = open_start(&lead, "stopped").await;
    pending_choice(&lead).await;
    lead.s.workbench.stop(&lead.id).expect("valid test fixture");
    let answer = stream_result(&read_answer(&mut stream).await);
    assert_eq!(answer["result"]["isError"], true);
    assert!(thread(&lead.s.workbench, &lead.id).start_choices.is_empty());
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("valid test fixture")
            .sessions
            .is_empty()
    );
    done(lead).await;
}

#[tokio::test]
async fn concurrent_retries_share_one_choice_and_one_child() {
    let lead = lead("choice-race").await;
    ask(&lead);
    let mut first = open_start(&lead, "same-key").await;
    let choice = pending_choice(&lead).await;
    let mut second = open_start(&lead, "same-key").await;
    assert_eq!(thread(&lead.s.workbench, &lead.id).start_choices.len(), 1);
    lead.s
        .workbench
        .answer_session_start(
            &lead.id,
            &choice.id,
            nexus_workbench::session_choice::Preference::New,
            false,
        )
        .expect("answer");
    let first = result_data(&read_answer(&mut first).await);
    let second = result_data(&read_answer(&mut second).await);
    assert_eq!(first["session_id"], second["session_id"]);
    assert_eq!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("children")
            .sessions
            .len(),
        1
    );
    done(lead).await;
}

#[tokio::test]
async fn another_sign_in_cannot_answer_an_old_runs_choice_or_use_its_preference() {
    let lead = lead("choice-identity").await;
    ask(&lead);
    let mut stream = open_start(&lead, "old-sign-in").await;
    let choice = pending_choice(&lead).await;
    *lead.s.host.choice_identity.lock().expect("identity") = Some("another-person".into());
    assert!(
        lead.s
            .workbench
            .answer_session_start(
                &lead.id,
                &choice.id,
                nexus_workbench::session_choice::Preference::New,
                true
            )
            .is_err()
    );
    assert_eq!(
        lead.s
            .workbench
            .session_start_preference(&lead.s.project)
            .expect("preference"),
        nexus_workbench::session_choice::Preference::Ask
    );
    assert_eq!(
        stream_result(&read_answer(&mut stream).await)["result"]["isError"],
        true
    );
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("children")
            .sessions
            .is_empty()
    );
    done(lead).await;
}

#[tokio::test]
async fn disconnecting_a_start_discards_its_choice_without_starting_later() {
    let lead = lead("choice-disconnect").await;
    ask(&lead);
    let stream = open_start(&lead, "disconnected").await;
    let choice = pending_choice(&lead).await;
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !thread(&lead.s.workbench, &lead.id).start_choices.is_empty() {
        assert!(Instant::now() < deadline, "disconnected choice remained");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        lead.s
            .workbench
            .answer_session_start(
                &lead.id,
                &choice.id,
                nexus_workbench::session_choice::Preference::New,
                false
            )
            .is_err()
    );
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("children")
            .sessions
            .is_empty()
    );
    done(lead).await;
}

#[tokio::test]
async fn a_bad_key_is_refused_before_a_choice_or_child_exists() {
    let lead = lead("choice-key").await;
    ask(&lead);
    let refused = stream_result(&post(&lead.at, &start_call("")).await);
    assert_eq!(refused["result"]["isError"], true);
    assert!(thread(&lead.s.workbench, &lead.id).start_choices.is_empty());
    assert!(
        lead.s
            .workbench
            .list_children(&lead.id)
            .expect("children")
            .sessions
            .is_empty()
    );
    done(lead).await;
}

#[tokio::test]
async fn raising_the_parent_while_its_choice_is_open_cannot_raise_the_reviewed_child_mode() {
    use nexus_workbench::event::PermissionMode;
    let lead = lead("choice-mode").await;
    ask(&lead);
    let mut stream = open_start(&lead, "reviewed-mode").await;
    let choice = pending_choice(&lead).await;
    assert_eq!(choice.permission_mode, PermissionMode::AcceptEdits);
    lead.s
        .workbench
        .update(
            &lead.id,
            nexus_workbench::ThreadPatch {
                permission_mode: Some(PermissionMode::BypassPermissions),
                ..Default::default()
            },
        )
        .expect("change mode");
    let deadline = Instant::now() + Duration::from_secs(5);
    while thread(&lead.s.workbench, &lead.id).permission_mode != PermissionMode::BypassPermissions {
        assert!(Instant::now() < deadline, "parent did not accept its mode change");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    lead.s
        .workbench
        .answer_session_start(
            &lead.id,
            &choice.id,
            nexus_workbench::session_choice::Preference::New,
            false,
        )
        .expect("answer");
    let answer = result_data(&read_answer(&mut stream).await);
    let id = answer["session_id"].as_str().expect("child id");
    assert_eq!(
        thread(&lead.s.workbench, id).permission_mode,
        PermissionMode::AcceptEdits
    );
    done(lead).await;
}
