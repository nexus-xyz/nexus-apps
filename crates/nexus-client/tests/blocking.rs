//! The blocking client (the CLI's) retries a read on 502, 503 and 504, waits a 503's `Retry-After`, and
//! never sends a write without an idempotency key twice. A stand-in Gateway answers from a script.
#![cfg(feature = "blocking")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexus_client::blocking::{Client, Download, ExportFile};
use nexus_client::{ClientOptions, Error};
use serde_json::json;

/// Serves `answers` in order, one per connection (`connection: close`); answers the URL and the request lines seen.
fn serve(answers: Vec<(u16, &'static str, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for (status, extra, body) in answers {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("request line");
            log.lock().expect("log").push(line.trim().to_string());
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("header");
                if header.trim().is_empty() {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body_in = vec![0; length];
            reader.read_exact(&mut body_in).expect("body");
            let answer = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(answer.as_bytes()).expect("answer");
        }
    });
    (url, seen)
}

fn client(url: &str, idempotency: bool) -> Client {
    Client::new(ClientOptions {
        idempotency,
        ..ClientOptions::new(url)
    })
}

const DRAINING: &str = r#"{"error":{"message":"the gateway is stopping","code":"draining"}}"#;

#[test]
fn redirects_never_forward_gateway_request_bodies() {
    // A redirect stays visible even when its destination would accept the provider key in the POST body.
    for status in [307, 308] {
        let (destination, received) = serve(vec![(200, "", "{}")]);
        let location: &'static str = Box::leak(format!("Location: {destination}/capture\r\n").into_boxed_str());
        let (url, _) = serve(vec![(status, location, "")]);
        let client = Client::new(ClientOptions {
            api_key: Some("nx_test_credential".into()),
            ..ClientOptions::new(&url)
        });
        let (answer_status, _) = client
            .send(
                nexus_client::Method::POST,
                "/v1/keys",
                Some(&json!({ "apiKey": "private-provider-key" })),
            )
            .expect("redirect response");
        assert_eq!(answer_status.as_u16(), status);
        assert!(
            received.lock().expect("received").is_empty(),
            "no redirected request reaches another origin"
        );
    }
}

#[test]
fn a_read_is_retried_after_a_503() {
    let (url, seen) = serve(vec![(503, "", DRAINING), (200, "", r#"{"ok":true}"#)]);
    assert_eq!(
        client(&url, true).get("/v1/spend").expect("read"),
        json!({ "ok": true })
    );
    assert_eq!(seen.lock().expect("seen").len(), 2);
}

#[test]
fn a_read_waits_retry_after() {
    let (url, seen) = serve(vec![(503, "Retry-After: 1\r\n", DRAINING), (200, "", "{}")]);
    let started = Instant::now();
    client(&url, true).get("/v1/spend").expect("read");
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(seen.lock().expect("seen").len(), 2);
}

#[test]
fn a_write_without_a_key_is_sent_once() {
    let (url, seen) = serve(vec![(503, "", DRAINING), (200, "", "{}")]);
    let Err(Error::Core(error)) = client(&url, false).post("/v1/things", json!({})) else {
        panic!("503")
    };
    assert_eq!(error.status, 503);
    assert_eq!(seen.lock().expect("seen").len(), 1);
}

const PAUSED: &str = r#"{"error":{"message":"sign-in is paused","type":"server_error","code":"cutover_paused"}}"#;

#[test]
fn a_paused_write_is_sent_once_and_names_the_wait() {
    // A keyed write is otherwise retried on 503; the maintenance pause lasts minutes, so it is not.
    let (url, seen) = serve(vec![
        (503, "Retry-After: 600\r\n", PAUSED),
        (200, "", "{}"),
        (200, "", "{}"),
    ]);
    let Err(error) = client(&url, true).post("/v1/orgs/invitations", json!({})) else {
        panic!("503")
    };
    assert_eq!(
        error.to_string(),
        "Sign-in is paused for maintenance; try again in 10 minutes."
    );
    let Error::Core(error) = error else { panic!("core") };
    assert_eq!((error.status, error.interval), (503, Some(600.0)));
    assert_eq!(seen.lock().expect("seen").len(), 1);
}

#[test]
fn a_read_fails_with_the_status_after_three_tries() {
    let (url, seen) = serve(vec![(502, "", "<html>"), (504, "", "<html>"), (503, "", DRAINING)]);
    let Err(Error::Core(error)) = client(&url, true).get("/v1/spend") else {
        panic!("503")
    };
    assert_eq!((error.status, error.code.as_deref()), (503, Some("draining")));
    assert_eq!(seen.lock().expect("seen").len(), 3);
}

#[test]
fn a_read_is_retried_when_the_connection_is_refused() {
    // Reserve the port without listening, so another parallel fixture cannot take it during the retries.
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket.bind("127.0.0.1:0".parse().expect("address")).expect("bind");
    let port = socket.local_addr().expect("addr").port();
    let started = Instant::now();
    let result = client(&format!("http://127.0.0.1:{port}"), true).get("/v1/spend");
    assert!(matches!(result, Err(Error::Transport(_))));
    assert!(started.elapsed() >= Duration::from_millis(1_250));
    drop(socket);
}

/// Answers one connection with `head` (the status line and headers) and `body`, then closes it: a body shorter than
/// its `content-length` is a file cut part way. Counts the requests.
fn serve_raw(head: &'static str, body: &'static [u8]) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let count = Arc::new(Mutex::new(0));
    let seen = count.clone();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            *seen.lock().expect("count") += 1;
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let stream = reader.get_mut();
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    (url, count)
}

#[test]
fn a_download_gives_every_byte_and_the_export_id() {
    let body: &[u8] = b"id,cost\r\ncall_1,0.100000\r\n";
    let (url, count) = serve_raw(
        "HTTP/1.1 200 OK\r\ncontent-type: text/csv\r\nx-nexus-export-id: exp_1\r\ncontent-length: 26\r\nconnection: close\r\n\r\n",
        body,
    );
    assert_eq!(
        client(&url, true)
            .download("/v1/analytics/usage/export.csv")
            .expect("download"),
        Download::File(ExportFile {
            body: body.to_vec(),
            export_id: Some("exp_1".into()),
            failed: None,
        })
    );
    assert_eq!(*count.lock().expect("count"), 1);
}

#[test]
fn a_download_cut_after_the_headers_is_cut_and_sent_once() {
    let (url, count) = serve_raw(
        "HTTP/1.1 200 OK\r\ncontent-type: application/jsonl\r\ncontent-length: 500\r\nconnection: close\r\n\r\n",
        b"{\"id\":\"call_1\"}\n",
    );
    assert!(matches!(
        client(&url, true)
            .download("/v1/spend/calls/export.jsonl")
            .expect("download"),
        Download::Cut(_)
    ));
    assert_eq!(*count.lock().expect("count"), 1);
}

#[test]
fn a_refused_download_is_the_api_error_and_is_not_retried() {
    let (url, seen) = serve(vec![(503, "", DRAINING), (200, "", "")]);
    match client(&url, true).download("/v1/agents/sessions/export.csv") {
        Err(Error::Core(error)) => assert_eq!(
            (error.status, error.code.as_deref(), error.interval),
            (503, Some("draining"), None)
        ),
        other => panic!("expected the 503, got {other:?}"),
    }
    assert_eq!(seen.lock().expect("seen").len(), 1);
}

#[test]
fn a_refused_download_carries_its_retry_after_as_its_interval() {
    let limited = r#"{"error":{"message":"more than 120 requests a minute; slow down","type":"rate_limit_error","code":"rate_limited"}}"#;
    let (url, seen) = serve(vec![(429, "Retry-After: 42\r\n", limited)]);
    match client(&url, true).download("/v1/skills/skl_a/versions/1/export") {
        Err(Error::Core(error)) => assert_eq!(
            (error.status, error.code.as_deref(), error.interval),
            (429, Some("rate_limited"), Some(42.0))
        ),
        other => panic!("expected the 429, got {other:?}"),
    }
    assert_eq!(seen.lock().expect("seen").len(), 1);
}

#[test]
fn a_download_past_the_time_limit_is_timed_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else { return };
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                break;
            }
        }
        let _ = reader
            .get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/csv\r\ncontent-length: 500\r\n\r\nid,at\r\n");
        std::thread::sleep(Duration::from_secs(3));
    });
    let client = Client::new(ClientOptions {
        stream_timeout: Duration::from_millis(300),
        ..ClientOptions::new(&url)
    });
    assert_eq!(
        client.download("/v1/analytics/usage/export.csv").expect("download"),
        Download::TimedOut
    );
}
