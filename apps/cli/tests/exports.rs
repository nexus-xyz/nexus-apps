//! `--csv` and `--jsonl` on `nexus spend`, `nexus analytics` and `nexus agents`, and `nexus spend calls`, against a
//! stand-in Gateway (`exports.test.ts`): the file is Gateway's bytes, unchanged, on stdout or in `--out`; the filters
//! pass through as the API names them; an export is sent once; a file cut part way writes nothing and fails; usage
//! errors (exit 2) come before anything is sent.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const KEY: &str = "nx_exports";
/// The writer's CSV: `\r\n` line ends, the last line's included.
const CSV: &str = "id,at,costUsd\r\ncall_1,2026-10-20T10:00:00.000Z,0.100000\r\n";
const JSONL: &str = "{\"id\":\"call_1\",\"costUsd\":\"0.100000\"}\n{\"manifest\":{\"exportId\":\"exp_2\",\"rows\":1,\"checksum\":\"sha256:00\",\"complete\":true,\"nextCursor\":null}}\n";
const STOPPED: &str = "{\"id\":\"call_1\"}\n{\"manifest\":{\"exportId\":\"exp_3\",\"rows\":100000,\"complete\":false,\"nextCursor\":\"pos.mac\"}}\n";

fn answer(status: u16, body: Value) -> Response {
    (StatusCode::from_u16(status).expect("status"), axum::Json(body)).into_response()
}

fn file(content_type: &'static str, id: &'static str, body: &'static str) -> Response {
    ([("content-type", content_type), ("x-nexus-export-id", id)], body).into_response()
}

fn route(path: &str, query: &str) -> Response {
    match path {
        _ if query.contains("from=2026-13-01") => answer(
            400,
            json!({ "error": { "message": "from must be a day, YYYY-MM-DD", "type": "invalid_request_error", "code": "invalid_request" } }),
        ),
        "/v1/spend/calls/export.csv" | "/v1/analytics/usage/export.csv" | "/v1/agents/sessions/export.csv" => {
            file("text/csv; charset=utf-8", "exp_1", CSV)
        }
        "/v1/spend/calls/export.jsonl" if query.contains("cursor=") => file("application/jsonl", "exp_3", STOPPED),
        "/v1/spend/calls/export.jsonl" | "/v1/analytics/usage/export.jsonl" | "/v1/agents/sessions/export.jsonl" => {
            file("application/jsonl", "exp_2", JSONL)
        }
        "/v1/spend/calls" if query.contains("model=rare") => answer(
            200,
            json!({ "from": "2026-10-01", "to": "2026-10-26", "calls": [], "nextCursor": "cur_2" }),
        ),
        "/v1/spend/calls" if query.contains("team=nobody") => answer(
            200,
            json!({ "from": "2026-10-01", "to": "2026-10-26", "calls": [], "nextCursor": null }),
        ),
        "/v1/spend/calls" => answer(
            200,
            json!({ "from": "2026-10-01", "to": "2026-10-26", "nextCursor": "next.mac", "calls": [
                { "id": "call_1", "at": "2026-10-20T10:00:00.000Z", "status": "settled", "model": "claude-sonnet", "provider": "anthropic",
                  "product": "chat", "capability": "ask", "userId": "usr_ada", "teamId": null, "costUsd": "0.100000" },
                { "id": "call_2", "at": "2026-10-20T09:00:00.000Z", "status": "released", "model": null, "provider": null,
                  "product": "chat", "capability": null, "userId": null, "teamId": null, "costUsd": "0.000000" }
            ] }),
        ),
        _ => answer(
            404,
            json!({ "error": { "message": "no such endpoint", "type": "not_found" } }),
        ),
    }
}

/// The stand-in Gateway; answers its URL and the paths (with queries) it was sent.
fn gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
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
                    let uri = request.uri().clone();
                    log.lock()
                        .expect("log")
                        .push(uri.path_and_query().map(|p| p.to_string()).unwrap_or_default());
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
                        return answer(401, json!({ "error": { "message": "invalid API key" } }));
                    }
                    route(uri.path(), uri.query().unwrap_or(""))
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            ready.send(listener.local_addr().expect("addr").port()).expect("ready");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    let port = port.recv().expect("port");
    (format!("http://127.0.0.1:{port}"), seen)
}

/// A Gateway that sends a file's headers and part of its body, then closes the connection, as Gateway does when a
/// file fails part way. Counts the requests.
fn cutting_gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            log.lock().expect("log").push(first.trim().to_string());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let stream = reader.get_mut();
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/csv\r\nx-nexus-export-id: exp_9\r\ntransfer-encoding: chunked\r\n\r\n10\r\nid,at,costUsd\r\nc\r\n",
            );
        }
    });
    (url, seen)
}

struct Run {
    code: i32,
    out: String,
    err: String,
    written: String,
    seen: Vec<String>,
    dir: PathBuf,
}

fn run(argv: &[&str]) -> Run {
    run_with(gateway(), argv, true)
}

fn run_with((url, seen): (String, Arc<Mutex<Vec<String>>>), argv: &[&str], signed_in: bool) -> Run {
    let dir = std::env::temp_dir().join(format!("nexus-exports-{}", uuid_like()));
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    let config = if signed_in {
        json!({ "url": url, "apiKey": KEY, "org": "acme" })
    } else {
        json!({ "url": url })
    };
    std::fs::write(&config_path, config.to_string()).expect("config");
    let capture = Capture(std::rc::Rc::new(Captured {
        can_stream: true,
        ..Captured::default()
    }));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let seen = seen.lock().expect("seen").clone();
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        written: capture.0.written.borrow().clone(),
        seen,
        dir,
    }
}

/// A directory name of its own per run: tests run in parallel, so the process id and a counter.
fn uuid_like() -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

#[test]
fn each_file_is_gateways_bytes_on_stdout() {
    for (argv, path) in [
        (vec!["spend", "--csv"], "/v1/spend/calls/export.csv"),
        (vec!["spend", "calls", "--csv"], "/v1/spend/calls/export.csv"),
        (vec!["analytics", "--csv"], "/v1/analytics/usage/export.csv"),
        (vec!["agents", "--csv"], "/v1/agents/sessions/export.csv"),
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 0, "{argv:?}: {}", r.err);
        assert_eq!(r.written, CSV, "{argv:?}");
        assert_eq!(r.out, "");
        assert_eq!(r.seen, vec![path.to_string()]);
        assert_eq!(
            r.err,
            "Export exp_1: its manifest (rows, checksum) is in the audit log: nexus audit --target exp_1 --json"
        );
    }
    for (argv, path) in [
        (vec!["spend", "--jsonl"], "/v1/spend/calls/export.jsonl"),
        (vec!["analytics", "--jsonl"], "/v1/analytics/usage/export.jsonl"),
        (vec!["agent", "--jsonl"], "/v1/agents/sessions/export.jsonl"),
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 0, "{argv:?}: {}", r.err);
        assert_eq!(r.written, JSONL, "{argv:?}");
        assert_eq!(r.seen, vec![path.to_string()]);
        assert_eq!(r.err, "");
    }
}

#[test]
fn the_filters_pass_through_as_the_api_names_them() {
    let r = run(&[
        "spend",
        "--csv",
        "--status",
        "failed,refused",
        "--team",
        "unassigned",
        "--model",
        "-",
        "--from",
        "2026-10-01",
        "--to",
        "2026-10-26",
        "--person",
        "usr_ada",
        "--provider",
        "anthropic",
        "--product",
        "chat/ask",
        "--session",
        "chs_1",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen,
        vec![
            "/v1/spend/calls/export.csv?from=2026-10-01&to=2026-10-26&team=unassigned&person=usr_ada&model=-&provider=anthropic&product=chat%2Fask&status=failed%2Crefused&session=chs_1"
        ]
    );
    let r = run(&[
        "agents", "--jsonl", "--agent", "claude", "--via", "nexus", "--status", "failed", "--team", "web",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen,
        vec!["/v1/agents/sessions/export.jsonl?team=web&agent=claude&via=nexus&status=failed"]
    );
    // An agent named by its command is filtered by its id.
    let r = run(&["agents", "--csv", "--agent", "agy"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen, vec!["/v1/agents/sessions/export.csv?agent=antigravity"]);
    let r = run(&[
        "analytics",
        "--csv",
        "--days",
        "3",
        "--to",
        "2026-10-26",
        "--model",
        "-",
    ]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.seen,
        vec!["/v1/analytics/usage/export.csv?from=2026-10-24&to=2026-10-26&model=-"]
    );
}

#[test]
fn days_with_an_unreadable_to_sends_to_as_typed() {
    let r = run(&["analytics", "--csv", "--days", "7", "--to", "2026-13-01"]);
    assert_eq!(r.seen, vec!["/v1/analytics/usage/export.csv?to=2026-13-01"]);
}

#[test]
fn an_invalid_filter_is_the_apis_400() {
    let r = run(&["spend", "--csv", "--from", "2026-13-01"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("from must be a day, YYYY-MM-DD"), "{}", r.err);
    assert_eq!(r.written, "");
}

#[test]
fn out_writes_the_same_bytes_to_a_file() {
    let r = run(&["analytics", "--csv", "--out", "usage.csv"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(std::fs::read(r.dir.join("usage.csv")).expect("file"), CSV.as_bytes());
    assert_eq!(r.out, format!("{} Wrote usage.csv ({} bytes).", ok(), CSV.len()));
    assert_eq!(r.written, "");
    let r = run(&["spend", "--jsonl", "--out", "calls.jsonl", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        std::fs::read(r.dir.join("calls.jsonl")).expect("file"),
        JSONL.as_bytes()
    );
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(
        printed,
        json!({ "path": "calls.jsonl", "bytes": JSONL.len(), "exportId": "exp_2" })
    );
}

/// The success mark without colors.
fn ok() -> String {
    nexus_cli::style::Style::new(false).ok.to_string()
}

#[test]
fn a_stopped_file_says_how_to_continue() {
    let r = run(&["spend", "--jsonl", "--cursor", "pos.mac"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.written, STOPPED);
    assert_eq!(
        r.err,
        "The file stops at 100000 rows, the limit. For the rest: the same command with --cursor pos.mac"
    );
}

#[test]
fn a_cut_file_writes_nothing_fails_and_is_sent_once() {
    let r = run_with(cutting_gateway(), &["analytics", "--csv"], true);
    assert_eq!(r.code, 1);
    assert!(
        r.err.ends_with(
            "The file is cut: Nexus Gateway closed the connection before the end. Nothing was written; run the command again."
        ),
        "{}",
        r.err
    );
    assert_eq!((r.written.as_str(), r.out.as_str()), ("", ""));
    assert_eq!(r.seen.len(), 1);
    let r = run_with(cutting_gateway(), &["spend", "--csv", "--out", "calls.csv"], true);
    assert_eq!(r.code, 1);
    assert!(!r.dir.join("calls.csv").exists());
}

#[test]
fn a_jsonl_file_without_its_manifest_is_cut() {
    let r = run_with(no_manifest_gateway(), &["analytics", "--jsonl"], true);
    assert_eq!(r.code, 1);
    assert!(
        r.err.ends_with(
            "The file is cut: its last line is not the manifest. Nothing was written; run the command again."
        ),
        "{}",
        r.err
    );
    assert_eq!(r.written, "");
}

/// A Gateway whose JSON Lines file has no manifest line.
fn no_manifest_gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            log.lock().expect("log").push(first.trim().to_string());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let body = "{\"id\":\"a\"}\n";
            let _ = reader.get_mut().write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/jsonl\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (url, seen)
}

#[test]
fn usage_errors_come_before_anything_is_sent() {
    for (argv, message) in [
        (vec!["spend", "--csv", "--jsonl"], "Choose one of --csv and --jsonl."),
        (vec!["agents", "--csv", "--jsonl"], "Choose one of --csv and --jsonl."),
        (
            vec!["agents", "use", "claude", "subscription", "--csv"],
            "--csv and --jsonl write the organization's agent sessions: use them with nexus agents alone.",
        ),
        (
            vec!["spend", "calls", "--by", "model"],
            "--by does not apply to the calls list. For the table: nexus spend --by team.",
        ),
        (
            vec!["agent", "list", "--jsonl"],
            "--csv and --jsonl write the organization's agent sessions: use them with nexus agents alone.",
        ),
        (vec!["analytics", "--csv", "--out"], "--out needs a file name."),
        (
            vec!["analytics", "--csv", "--by", "model"],
            "--by, --depth and --under do not apply to the file: it has one row per day, team, person, model, provider and product.",
        ),
        (
            vec!["analytics", "--jsonl", "--under", "eng"],
            "--by, --depth and --under do not apply to the file: it has one row per day, team, person, model, provider and product.",
        ),
        (
            vec!["spend", "--csv", "--by", "team"],
            "--by does not apply to the calls file. For the usage file: nexus analytics --csv.",
        ),
        (
            vec!["analytics", "--csv", "--days", "999"],
            "--days must be a whole number from 1 to 366.",
        ),
    ] {
        let r = run(&argv);
        assert_eq!(r.code, 2, "{argv:?}");
        assert!(r.err.contains(message), "{argv:?}: {}", r.err);
        assert!(r.seen.is_empty(), "{argv:?}: {:?}", r.seen);
    }
}

#[test]
fn help_wins_over_a_file_flag_on_agents() {
    for argv in [vec!["agents", "--help", "--csv"], vec!["agents", "help", "--jsonl"]] {
        let r = run(&argv);
        assert_eq!(r.code, 0, "{argv:?}: {}", r.err);
        assert!(r.out.contains("nexus agents"), "{argv:?}: {}", r.out);
        assert!(r.seen.is_empty(), "{argv:?}: {:?}", r.seen);
    }
}

#[test]
fn agents_files_need_the_sign_in() {
    let r = run_with(gateway(), &["agents", "--csv"], false);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("You are not logged in."), "{}", r.err);
    assert!(r.seen.is_empty());
}

#[test]
fn spend_calls_lists_each_call() {
    let r = run(&["spend", "calls", "--team", "web", "--limit", "2"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.seen, vec!["/v1/spend/calls?team=web&limit=2"]);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines[0], "Calls · 2026-10-01 to 2026-10-26");
    assert!(lines[2].trim_start().starts_with("Time"), "{}", r.out);
    assert!(r.out.contains("2026-10-20 10:00:00"), "{}", r.out);
    assert!(r.out.contains("chat/ask"), "{}", r.out);
    assert!(r.out.contains("0.100000"), "{}", r.out);
    assert!(r.out.contains("usr_ada"), "{}", r.out);
    assert_eq!(
        *lines.last().expect("line"),
        "More: add --cursor next.mac to the same command."
    );
}

#[test]
fn spend_calls_json_and_empty() {
    let r = run(&["spend", "calls", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let printed: Value = serde_json::from_str(&r.out).expect("json");
    assert_eq!(printed["calls"][0]["id"], "call_1");
    let r = run(&["spend", "calls", "--team", "nobody"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "No calls from 2026-10-01 to 2026-10-26.");
    let r = run(&["spend", "calls", "--model", "rare"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.out,
        "No calls on this page, from 2026-10-01 to 2026-10-26.\n\nMore: add --cursor cur_2 to the same command."
    );
}
