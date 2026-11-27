//! Staff-only commands, `nexus triage` included, are in a build with `staff` (a default feature: every build Nexus
//! ships) and in no open-source build (`--no-default-features`). CI runs this file both ways.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nexus_cli::catalog::{command_catalog, help_groups};
use nexus_cli::{Capture, Captured, Context, SEMVER, run_cli};
use serde_json::json;

/// The commands only Nexus's own people may use. A new staff-only command goes here and behind `staff`.
const STAFF_COMMANDS: &[&str] = &["triage"];

fn listed_in_the_catalog(name: &str) -> bool {
    command_catalog(SEMVER)["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .any(|command| command["name"] == name)
}

fn listed_in_help(name: &str) -> bool {
    help_groups()
        .iter()
        .flat_map(|group| group.rows.iter())
        .any(|(usage, _)| usage.split_whitespace().next() == Some(name))
}

#[test]
fn staff_commands_are_listed_only_in_a_staff_build() {
    for name in STAFF_COMMANDS {
        assert_eq!(
            listed_in_the_catalog(name),
            cfg!(feature = "staff"),
            "`nexus help --json`: {name}"
        );
        assert_eq!(listed_in_help(name), cfg!(feature = "staff"), "`nexus help`: {name}");
    }
}

/// A Gateway that answers every request 404 and keeps each request line.
fn gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buffer = [0u8; 8192];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..read]);
            log.lock()
                .expect("log")
                .push(head.lines().next().unwrap_or_default().to_string());
            let body = r#"{"error":{"message":"not found","code":"not_found"}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, requests)
}

/// `nexus triage` reaches Gateway only in a staff build. In the open-source build it is no command: it asks for no
/// sign-in check, sends no report and makes no triage folder.
#[test]
fn only_a_staff_build_runs_a_staff_command() {
    let dir: PathBuf = std::env::temp_dir().join(format!("nexus-open-build-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, requests) = gateway();
    let config_path = dir.join("config.json");
    std::fs::write(&config_path, json!({ "url": url, "apiKey": "nx_test" }).to_string()).expect("config");
    std::fs::write(dir.join("report.md"), "A report.").expect("report");
    for argv in [&["triage"][..], &["triage", "send", "report.md", "--yes"]] {
        let capture = Capture(std::rc::Rc::new(Captured::default()));
        let ctx = Context {
            config_path: config_path.clone(),
            io: Box::new(capture.clone()),
            color: false,
            env: HashMap::from([("HOME".to_string(), dir.display().to_string())]),
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
        let err = capture.0.err.borrow().join(
            "
",
        );
        // The stand-in Gateway answers 404, so the staff build fails too, after it reached Gateway.
        assert_ne!(code, 0, "{argv:?}: {err}");
    }
    let requests = requests.lock().expect("log").clone();
    let staff_routes = ["GET /v1/tenants/me", "POST /v1/diagnostics/feedback"];
    for route in staff_routes {
        assert_eq!(
            requests.iter().any(|line| line.starts_with(route)),
            cfg!(feature = "staff"),
            "{route}: {requests:?}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}
