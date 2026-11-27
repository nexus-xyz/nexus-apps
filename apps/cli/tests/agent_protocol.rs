//! `nexus __agent` as the desktop starts it: this crate's built binary, a fake key, an empty config and a
//! local Gateway. The handshake answers before any sign-in or request; a session's `ready` names the same protocol,
//! signs in with the key the program passed, and a turn streams and ends. `fixtures/agent-handshake.json` is the line
//! the desktop's own tests read (`nexus-workbench`'s `nexus_runtime`), so the two sides agree on it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A Gateway on a free local port: `GET /v1/spend` is a budget, any other request a streamed answer. It keeps each
/// request's head, so a test can say what was asked and with which key.
fn gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("its address");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = heads.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || answer(stream, &seen));
        }
    });
    (format!("http://{address}"), heads)
}

fn answer(mut stream: TcpStream, seen: &Mutex<Vec<String>>) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 8192];
    let head = loop {
        let Ok(n) = stream.read(&mut buffer) else { return };
        if n == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..n]);
        let text = String::from_utf8_lossy(&request).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0usize);
            if request.len() >= end + 4 + length {
                break text[..end].to_string();
            }
        }
    };
    if let Ok(mut seen) = seen.lock() {
        seen.push(head.clone());
    }
    if head.starts_with("GET /v1/spend") {
        let body = r#"{"remainingUsd":4,"budgetUsd":5}"#;
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        return;
    }
    let chunk = |delta: Value, finish: Option<&str>| {
        let mut chunk =
            json!({ "model": "test/model", "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
        if finish.is_some() {
            chunk["nexus"] = json!({ "cost_usd": 0.001 });
        }
        format!("data: {chunk}\n\n")
    };
    let events = chunk(json!({ "content": "Hello from the Gateway." }), None)
        + &chunk(json!({}), Some("stop"))
        + "data: [DONE]\n\n";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{events}"
    );
}

/// A folder of its own, with nothing in it: the run's config, sessions and workspace.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-agent-protocol-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a folder");
    std::fs::canonicalize(&dir).expect("its real path")
}

/// The binary with an empty config of its own (`NEXUS_CONFIG` names a file that does not exist) and no update check.
fn nexus(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nexus"));
    command
        .current_dir(dir)
        .env("NEXUS_CONFIG", dir.join("config.json"))
        .env("NEXUS_NO_UPDATE_CHECK", "1")
        .env_remove("NEXUS_API_KEY")
        .env_remove("NEXUS_GATEWAY");
    command
}

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent-handshake.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("the fixture is JSON")
}

#[test]
fn the_handshake_needs_no_sign_in_and_sends_no_request() {
    let dir = folder("handshake");
    let (url, heads) = gateway();
    let output = nexus(&dir)
        .args(["__agent", "--handshake"])
        .env("NEXUS_URL", &url)
        .stdin(Stdio::null())
        .output()
        .expect("nexus runs");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one line: {stdout}");
    let line: Value = serde_json::from_str(lines[0]).expect("a JSON line");
    let wanted = fixture();
    assert_eq!(
        (&line["type"], &line["protocol"]),
        (&wanted["type"], &wanted["protocol"])
    );
    assert_eq!(line["version"], nexus_cli::VERSION);
    assert!(
        heads.lock().map(|heads| heads.is_empty()).unwrap_or(false),
        "no request"
    );
    assert!(!dir.join("config.json").exists(), "nothing written");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_signs_in_with_the_programs_key_and_runs_a_turn() {
    let dir = folder("session");
    let (url, heads) = gateway();
    let mut child = nexus(&dir)
        .args(["__agent", "--session", "desktop-1"])
        .env("NEXUS_URL", &url)
        .env("NEXUS_API_KEY", "nx-fake-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("nexus starts");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let mut next = |kind: &str, seen: &mut Vec<Value>| -> Value {
        for line in lines.by_ref() {
            let value: Value = serde_json::from_str(&line.expect("a line")).expect("a JSON line");
            seen.push(value.clone());
            if value["type"] == kind {
                return value;
            }
        }
        panic!("the agent ended before {kind}: {seen:#?}");
    };
    let mut seen = Vec::new();
    let ready = next("ready", &mut seen);
    assert_eq!(
        (
            &ready["protocol"],
            ready["session"].as_str(),
            ready["messages"].as_u64()
        ),
        (&fixture()["protocol"], Some("desktop-1"), Some(0))
    );
    for line in [
        json!({ "type": "approvals", "level": "auto" }),
        json!({ "type": "prompt", "text": "Reply OK without using tools.", "model": "test/model" }),
    ] {
        writeln!(stdin, "{line}").expect("a line in");
    }
    let done = next("done", &mut seen);
    let said: String = seen
        .iter()
        .filter(|line| line["type"] == "delta")
        .filter_map(|line| line["text"].as_str())
        .collect();
    assert_eq!(said, "Hello from the Gateway.", "{seen:#?}");
    assert_eq!(done["messages"], 2, "{done}");
    drop(stdin);
    assert!(child.wait().expect("it exits").success());
    let heads = heads.lock().map(|heads| heads.clone()).unwrap_or_default();
    let model_call = heads
        .iter()
        .find(|head| head.starts_with("POST "))
        .unwrap_or_else(|| panic!("a model call: {heads:#?}"));
    assert!(
        model_call.to_lowercase().contains("authorization: bearer nx-fake-key"),
        "{model_call}"
    );
    assert!(
        dir.join("agent-sessions/desktop-1.json").is_file(),
        "the conversation is kept"
    );
    let _ = std::fs::remove_dir_all(dir);
}
