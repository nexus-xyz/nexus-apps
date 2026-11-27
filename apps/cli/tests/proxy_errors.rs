//! An error answer whose body is not JSON (a load balancer's HTML 502, a plain-text 401) keeps its HTTP
//! status, so the CLI still gives the hint for that status. A JSON error reads as before.

use std::collections::HashMap;
use std::path::PathBuf;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::json;

/// A stand-in Gateway that gives every request the same answer.
fn gateway(answer: fn() -> Response) -> String {
    let (ready, port) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = Router::new().fallback(move || async move { answer() });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            ready.send(listener.local_addr().expect("addr").port()).expect("ready");
            axum::serve(listener, app).await.expect("serve");
        });
    });
    format!("http://127.0.0.1:{}", port.recv().expect("port"))
}

/// Run `nexus org domains` against the stand-in; answers the exit code and stderr.
fn run(answer: fn() -> Response) -> (i32, String) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "nexus-proxy-errors-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(
        &config_path,
        json!({ "url": gateway(answer), "apiKey": "nx_proxy", "org": "acme" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
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
    let argv: Vec<String> = ["org", "domains"].iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let _ = std::fs::remove_dir_all(&dir);
    let err = capture.0.err.borrow().join("\n");
    (code, err)
}

fn html_502() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "text/html")],
        "<html><body><h1>502 Server Error</h1></body></html>",
    )
        .into_response()
}

fn text_401() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

fn json_502() -> Response {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(json!({ "error": { "message": "Code answers did not respond", "code": "bad_gateway" } })),
    )
        .into_response()
}

#[test]
fn an_html_502_keeps_its_status_and_hint() {
    let (code, err) = run(html_502);
    assert_ne!(code, 0);
    assert!(err.contains("HTTP 502"), "{err}");
    assert!(err.contains("did not answer"), "{err}");
    assert!(!err.contains("not JSON"), "{err}");
}

#[test]
fn a_text_401_asks_to_sign_in_again() {
    let (code, err) = run(text_401);
    assert_ne!(code, 0);
    assert!(err.contains("did not accept your API key"), "{err}");
    assert!(err.contains("nexus auth login"), "{err}");
}

#[test]
fn a_json_error_reads_as_before() {
    let (code, err) = run(json_502);
    assert_ne!(code, 0);
    assert!(err.contains("Code answers did not respond"), "{err}");
    assert!(err.contains("`nexus modules`"), "{err}");
}
