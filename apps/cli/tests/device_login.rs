//! `nexus auth login`'s device sign-in names the CLI's OAuth client: `POST /v1/auth/device` carries
//! `client_id: nxc_cli`. While the operator pauses sign-in (503 `cutover_paused`), it says how long to wait.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::{Value, json};

const STOP_HERE: (&str, &str, &str) = (
    "400 Bad Request",
    "",
    r#"{"error":{"message":"stop here","code":"invalid_request"}}"#,
);

/// Each request's path and JSON body, in arrival order.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// A Gateway that keeps each request's path and JSON body, and refuses each with `refusal` (status line, extra
/// headers, body).
fn gateway(refusal: (&'static str, &'static str, &'static str)) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            reader.read_line(&mut line).expect("request line");
            let path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("header");
                let header = header.trim();
                if header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).expect("body");
            kept.lock()
                .expect("seen")
                .push((path, serde_json::from_slice(&body).unwrap_or(Value::Null)));
            let (status, extra, answer) = refusal;
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{answer}",
                answer.len()
            );
        }
    });
    (url, seen)
}

fn context(dir: &std::path::Path, captured: &std::rc::Rc<Captured>) -> Context {
    Context {
        config_path: dir.join("config.json"),
        io: Box::new(Capture(captured.clone())),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: Some("laptop".into()),
        cwd: dir.to_path_buf(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    }
}

#[test]
fn device_login_sends_the_cli_client_id() {
    let dir = std::env::temp_dir().join(format!("nexus-device-login-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, seen) = gateway(STOP_HERE);
    let ctx = context(&dir, &std::rc::Rc::new(Captured::default()));
    let argv: Vec<String> = ["auth", "login", "--url", url.as_str(), "--no-browser"]
        .map(String::from)
        .to_vec();
    assert_ne!(run_cli(&argv, &ctx), 0);
    let starts: Vec<Value> = seen
        .lock()
        .expect("seen")
        .iter()
        .filter(|(path, _)| path == "/v1/auth/device")
        .map(|(_, body)| body.clone())
        .collect();
    assert_eq!(
        starts,
        [json!({ "client": "nexus CLI on laptop", "client_id": "nxc_cli" })]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_paused_sign_in_says_how_long_to_wait_and_asks_once() {
    let dir = std::env::temp_dir().join(format!("nexus-device-login-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, seen) = gateway((
        "503 Service Unavailable",
        "Retry-After: 600\r\n",
        r#"{"error":{"message":"sign-in is paused","type":"server_error","code":"cutover_paused"}}"#,
    ));
    let captured = std::rc::Rc::new(Captured::default());
    let ctx = context(&dir, &captured);
    let argv: Vec<String> = ["auth", "login", "--url", url.as_str(), "--no-browser"]
        .map(String::from)
        .to_vec();
    assert_eq!(run_cli(&argv, &ctx), 1);
    let printed = format!("{:?}{:?}", captured.out.borrow(), captured.err.borrow());
    assert!(
        printed.contains("Sign-in is paused for maintenance; try again in 10 minutes."),
        "{printed}"
    );
    assert_eq!(seen.lock().expect("seen").len(), 1, "asked once, not retried");
    std::fs::remove_dir_all(&dir).ok();
}
