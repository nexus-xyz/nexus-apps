//! `nexus agent ask` and `nexus agent chat` against a stand-in agent API: what each sends, and what it
//! prints. The TypeScript CLI's `agent-chat.test.ts` checks the same.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_me";

fn answer(conversation: &str) -> Value {
    json!({
        "conversation": conversation, "platform": { "name": "Nexus API", "markup": "markdown" },
        "messages": [{ "kind": "answer", "text": "The launch is on Friday.", "model": "anthropic/claude-sonnet-4-5", "costUsd": 0.0021,
            "sources": [{ "title": "Launch plan", "url": "http://ui.test/wiki/acme/web/launch", "kind": "wiki" }] }],
    })
}

fn sse(events: &[(&str, Value)]) -> Response {
    let text: String = events
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n: keepalive\n\n"))
        .collect();
    ([(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")], text).into_response()
}

/// The stand-in's answer to one ask: streamed when asked to stream, or JSON.
fn reply(body: &Value) -> Response {
    let message = body["message"].as_str().unwrap_or("");
    let conversation = body["conversation"].as_str().unwrap_or("c1");
    if message == "gateway down" {
        // Gateway's own refusal, in its own shape.
        let refusal = json!({ "error": { "message": "The agent service did not answer.", "type": "api_error", "code": "agents_api_unreachable" } });
        return (StatusCode::BAD_GATEWAY, axum::Json(refusal)).into_response();
    }
    if message == "busy" {
        let refusal = json!({ "error": "memory_unavailable", "detail": "The agent's memory cannot be read now." });
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(refusal)).into_response();
    }
    if message == "file an issue" {
        let pending = json!({ "kind": "pending_action", "card": "card_1", "write": { "tool": "linear.create_issue" },
            "approveUrl": "http://ui.test/app/approve/card_1", "detail": "Approve this write in Nexus UI. Nothing was written yet." });
        return axum::Json(json!({ "conversation": "c9", "platform": {}, "messages": [pending] })).into_response();
    }
    if body["stream"] != json!(true) {
        return axum::Json(answer(conversation)).into_response();
    }
    let mut done = answer(conversation);
    done["status"] = json!(200);
    sse(&[
        ("started", json!({ "conversation": conversation })),
        ("text", json!({ "delta": "The launch " })),
        ("text", json!({ "delta": "is on Friday." })),
        ("done", done),
    ])
}

/// Each call's path, headers and body.
type Seen = Arc<Mutex<Vec<(String, HeaderMap, Value)>>>;

fn agent_api() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move |headers: HeaderMap, request: Request| {
                let log = log.clone();
                async move {
                    let path = request.uri().path().to_string();
                    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .unwrap_or_default();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    log.lock().expect("log").push((path, headers, body.clone()));
                    reply(&body)
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            ready.send(listener.local_addr().expect("addr").port()).expect("ready");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    (format!("http://127.0.0.1:{}/", port.recv().expect("port")), seen)
}

struct Run {
    code: i32,
    out: String,
    err: String,
    written: String,
    seen: Vec<(String, HeaderMap, Value)>,
}

/// Where the CLI is told the agent is: nowhere, the agent API's own URL, or (also) through Gateway.
#[derive(Clone, Copy, PartialEq)]
enum Agent {
    Unset,
    Direct,
    ViaGateway,
}

fn run(argv: &[&str], configured: bool) -> Run {
    run_with(argv, "", configured)
}

fn run_with(argv: &[&str], stdin: &str, configured: bool) -> Run {
    run_as(argv, stdin, if configured { Agent::Direct } else { Agent::Unset })
}

fn run_as(argv: &[&str], stdin: &str, agent: Agent) -> Run {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nexus-agent-chat-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    let (url, seen) = agent_api();
    // Through Gateway, the stand-in is the CLI's Gateway; the agent API's own URL is set too, and never called.
    let gateway = if agent == Agent::ViaGateway {
        url.as_str()
    } else {
        "http://core.test"
    };
    let config = json!({ "url": gateway, "apiKey": KEY, "kind": "cli", "org": "acme" });
    std::fs::write(&config_path, config.to_string()).expect("config");
    let mut env = HashMap::new();
    if agent != Agent::Unset {
        env.insert("NEXUS_AGENTS_API_URL".to_string(), "http://127.0.0.1:9".to_string());
    }
    match agent {
        Agent::Direct => {
            env.insert("NEXUS_AGENTS_API_URL".to_string(), url);
        }
        Agent::ViaGateway => {
            env.insert("NEXUS_AGENT_VIA_GATEWAY".to_string(), "on".to_string());
        }
        Agent::Unset => {}
    }
    let capture = Capture(std::rc::Rc::new(Captured {
        stdin: stdin.to_string(),
        can_stream: true,
        ..Default::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env,
        poll_ms: Some(0),
        hostname: None,
        cwd: dir,
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let captured = &capture.0;
    let (out, err) = (captured.out.borrow().join("\n"), captured.err.borrow().join("\n"));
    let (written, seen) = (captured.written.borrow().clone(), seen.lock().expect("seen").clone());
    Run {
        code,
        out,
        err,
        written,
        seen,
    }
}

#[test]
fn ask_streams_the_answer_as_the_signed_in_person_then_its_sources() {
    let r = run(&["agent", "ask", "When", "is", "it?", "--model", "nexus/auto"], true);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen.len(), 1);
    let (path, headers, body) = &r.seen[0];
    assert_eq!(path, "/agent/ask");
    assert_eq!(headers["authorization"], format!("Bearer {KEY}").as_str());
    assert!(headers["x-nexus-client"].to_str().expect("client").starts_with("cli/"));
    assert!(headers.contains_key("idempotency-key"));
    assert_eq!(
        body,
        &json!({ "message": "When is it?", "model": "nexus/auto", "stream": true })
    );
    assert!(r.written.starts_with("The launch is on Friday.\n"), "{}", r.written);
    let sources = "[1] Launch plan  http://ui.test/wiki/acme/web/launch (wiki)";
    assert!(r.out.contains(sources), "{}", r.out);
    let footer = "anthropic/claude-sonnet-4-5 · $0.002100 · conversation c1";
    assert!(r.out.contains(footer), "{}", r.out);
}

#[test]
fn a_write_is_only_its_approval_link_and_json_is_the_apis_answer() {
    let shown = run(&["agent", "ask", "file an issue"], true);
    assert_eq!(shown.code, 0, "{}", shown.err);
    let approve = "Approve it in Nexus UI: http://ui.test/app/approve/card_1";
    assert!(shown.out.contains(approve), "{}", shown.out);
    assert!(shown.out.contains("The CLI never runs or confirms a write."));
    assert_eq!(shown.seen.len(), 1);

    let printed = run(&["agent", "ask", "When is the launch?", "--json"], true);
    assert_eq!(printed.code, 0, "{}", printed.err);
    assert_eq!(printed.seen[0].2["stream"], json!(false));
    let payload: Value = serde_json::from_str(&printed.out).expect("json");
    assert_eq!(payload["conversation"], "c1");
    assert_eq!(payload["status"], 200);
}

#[test]
fn each_failure_has_a_machine_readable_code() {
    let unconfigured = run(&["agent", "ask", "hello", "--json"], false);
    assert_eq!(unconfigured.code, 1);
    let error: Value = serde_json::from_str(&unconfigured.err).expect("json");
    assert_eq!(error["error"]["code"], "agents_api_unconfigured");
    assert_eq!(error["error"]["status"], 503);
    assert!(unconfigured.seen.is_empty());

    let busy = run(&["agent", "ask", "busy", "--json"], true);
    assert_eq!(busy.code, 1);
    let error: Value = serde_json::from_str(&busy.err).expect("json");
    assert_eq!(error["error"]["code"], "memory_unavailable");
    assert_eq!(error["error"]["hint"], "Try again in a moment.");

    let missing = run(&["agent", "ask"], true);
    assert_eq!(missing.code, 2);
    assert!(missing.seen.is_empty());
}

#[test]
fn conversation_continues_an_earlier_answer() {
    let r = run(&["agent", "ask", "And the date?", "--conversation", "c2"], true);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen[0].2["conversation"], "c2");
    assert!(r.out.contains("conversation c2"), "{}", r.out);
}

#[test]
fn chat_keeps_one_conversation_until_exit() {
    let r = run_with(
        &["agent", "chat"],
        "first question\n\nsecond question\nexit\nnot sent\n",
        true,
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let bodies: Vec<&Value> = r.seen.iter().map(|(_, _, body)| body).collect();
    let first = json!({ "message": "first question", "stream": true });
    let second = json!({ "message": "second question", "conversation": "c1", "stream": true });
    assert_eq!(bodies, vec![&first, &second]);

    let lines = run_with(&["agent", "chat", "--json", "--conversation", "c2"], "one\ntwo", true);
    assert_eq!(lines.code, 0, "{}", lines.err);
    let conversations: Vec<Value> = lines
        .out
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("json")["conversation"].clone())
        .collect();
    assert_eq!(conversations, vec![json!("c2"), json!("c2")]);
}

/// `NEXUS_AGENT_VIA_GATEWAY=on`: the ask goes to the CLI's own Gateway at `/v1/agent/ask`, with the same
/// sign-in and an `Idempotency-Key`, and still streams. Gateway's own refusal has its code.
#[test]
fn via_gateway_the_ask_goes_to_gateways_agent_route() {
    let r = run_as(&["agent", "ask", "When", "is", "it?"], "", Agent::ViaGateway);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen.len(), 1);
    let (path, headers, body) = &r.seen[0];
    assert_eq!(path, "/v1/agent/ask");
    assert_eq!(headers["authorization"], format!("Bearer {KEY}").as_str());
    assert!(headers.contains_key("idempotency-key"));
    assert_eq!(body["stream"], json!(true));
    assert!(
        r.written.starts_with(
            "The launch is on Friday.
"
        ),
        "{}",
        r.written
    );

    let down = run_as(&["agent", "ask", "gateway down", "--json"], "", Agent::ViaGateway);
    assert_eq!(down.code, 1);
    let error: Value = serde_json::from_str(&down.err).expect("json");
    assert_eq!(error["error"]["code"], "agents_api_unreachable");
    assert_eq!(error["error"]["status"], 502);
    assert_eq!(error["error"]["message"], "The agent service did not answer.");
    assert_eq!(error["error"]["hint"], "Try again in a moment.");
}
