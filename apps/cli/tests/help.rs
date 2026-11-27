//! `nexus <command> --help` for every built-in command (`commands.test.ts`): it prints help, exits 0 and
//! sends no request, signed in, with arguments after it too.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nexus_cli::{Capture, Captured, Context, VERSION, run_cli};
use serde_json::{Value, json};

/// A Gateway that answers every request 404 and counts them.
fn gateway() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            counted.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = r#"{"error":{"message":"not found"}}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, requests)
}

fn names(commands: &[Value]) -> Vec<String> {
    commands
        .iter()
        .flat_map(|command| {
            let own = command["name"].as_str().unwrap_or_default().to_string();
            let subcommands = command["subcommands"].as_array().map(Vec::as_slice).unwrap_or_default();
            std::iter::once(own).chain(names(subcommands))
        })
        .collect()
}

#[test]
fn every_command_prints_help_and_sends_no_request() {
    let catalog = nexus_cli::catalog::command_catalog(VERSION);
    let commands = catalog["commands"].as_array().expect("commands");
    let dir: PathBuf = std::env::temp_dir().join(format!("nexus-help-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, requests) = gateway();
    let config_path = dir.join("config.json");
    std::fs::write(&config_path, json!({ "url": url, "apiKey": "nx_test" }).to_string()).expect("config");
    let with_arguments = [
        "login",
        "logout",
        "team delete eng",
        "teams create eng",
        "members invite a@example.com",
        "chat hello",
        "keys add anthropic sk-test",
        "org switch acme",
    ];
    let argvs = names(commands)
        .into_iter()
        .filter(|name| !name.contains('<'))
        .chain(with_arguments.map(String::from));
    for name in argvs {
        let capture = Capture(std::rc::Rc::new(Captured::default()));
        let ctx = Context {
            config_path: config_path.clone(),
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
        let mut argv: Vec<String> = name.split(' ').map(String::from).collect();
        argv.push("--help".into());
        let before = requests.load(Ordering::SeqCst);
        let code = run_cli(&argv, &ctx);
        let out = capture.0.out.borrow().join("\n");
        assert_eq!(code, 0, "{name}: {out}");
        let top = match argv[0].as_str() {
            "login" | "logout" => "auth",
            "teams" => "team",
            other => other,
        };
        let usage = commands
            .iter()
            .find(|command| command["name"] == top)
            .and_then(|command| command["usage"].as_str())
            .expect("usage");
        let prefix: Vec<&str> = usage.split(' ').take(2).collect();
        assert!(out.contains(&prefix.join(" ")), "{name}: {out}");
        // The launchers' help reads the agent's route, and the agent policy to name the models you may use.
        if nexus_client::agent_entries::Agent::from_command(&name).is_none() {
            assert_eq!(requests.load(Ordering::SeqCst), before, "{name} sent a request");
        }
    }
}

#[test]
fn login_refuses_an_unknown_environment_and_both_url_and_env() {
    let dir: PathBuf = std::env::temp_dir().join(format!("nexus-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, requests) = gateway();
    for (argv, message) in [
        (vec!["auth", "login", "--env", "qa"], "Unknown environment: qa."),
        (
            vec!["auth", "login", "--env", "dev", "--url", url.as_str()],
            "Use --url or --env, not both.",
        ),
    ] {
        let capture = Capture(std::rc::Rc::new(Captured::default()));
        let ctx = Context {
            config_path: dir.join("config.json"),
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
        let argv: Vec<String> = argv.into_iter().map(String::from).collect();
        assert_eq!(run_cli(&argv, &ctx), 2, "{argv:?}");
        let err = capture.0.err.borrow().join("\n");
        assert!(err.contains(message), "{argv:?}: {err}");
    }
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn login_keeps_dev_for_employees_signed_in_to_production() {
    // A build without `NEXUS_DEV_GATEWAY_URL` (the open-source one) has no `dev` environment to keep.
    if nexus_client::config::DEV_GATEWAY_URL.is_none() {
        return;
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("nexus-env-staff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, requests) = gateway();
    // Signed in to another Gateway only: no production sign-in to check, so nothing is sent.
    std::fs::write(
        dir.join("config.json"),
        json!({ "url": url, "apiKey": "nx_test" }).to_string(),
    )
    .expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path: dir.join("config.json"),
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
    let argv: Vec<String> = ["auth", "login", "--env", "dev", "--name", "Acme"]
        .map(String::from)
        .to_vec();
    assert_eq!(run_cli(&argv, &ctx), 1);
    let err = capture.0.err.borrow().join("\n");
    assert!(err.contains("--env dev is for Nexus employees."), "{err}");
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn knows_nexuss_own_people_by_their_staff_mark_or_their_nexus_address() {
    use nexus_cli::cli::is_employee;
    assert!(is_employee(
        &json!({ "user": { "mark": "staff", "email": "ada@gmail.com" } })
    ));
    assert!(is_employee(&json!({ "user": { "email": "Ada@Nexus.xyz" } })));
    assert!(!is_employee(&json!({ "user": { "email": "ada@nexus.xyz.evil.test" } })));
    assert!(!is_employee(
        &json!({ "user": { "mark": "test", "email": "ada@acme.test" } })
    ));
    assert!(!is_employee(&json!({})));
}

#[test]
fn a_failed_sign_in_is_queued_as_a_client_error_with_codes_only() {
    let dir: PathBuf = std::env::temp_dir().join(format!("nexus-signin-error-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path: dir.join("config.json"),
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
    let argv: Vec<String> = ["auth", "login", "--url", "http://127.0.0.1:1", "--key", "nx_secret"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(run_cli(&argv, &ctx), 1);
    let queued: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("usage-errors.json")).expect("queued")).expect("json");
    assert_eq!(queued[0]["kind"], "sign_in");
    assert_eq!(queued[0]["component"], "sign_in");
    assert_eq!(queued[0]["code"], "unreachable");
    assert!(!queued.to_string().contains("nx_secret") && !queued.to_string().contains("127.0.0.1"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn organization_vault_fingerprints_and_invalid_vault_flags_never_contact_gateway() {
    let dir = std::env::temp_dir().join(format!("nexus-vault-fingerprint-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("folder");
    let public = dir.join("public-key");
    std::fs::write(&public, [0u8; 32]).expect("vault export");
    let (url, requests) = gateway();
    let config_path = dir.join("config.json");
    std::fs::write(&config_path, json!({ "url": url, "apiKey": "nx_test" }).to_string()).expect("config");
    for (flags, expected) in [
        (vec!["--vault-public-key", public.to_str().expect("path"), "--json"], 0),
        (vec!["--vault-public-key"], 2),
        (vec!["--vault-public-key="], 2),
        (
            vec!["--vault-public-key", public.to_str().expect("path"), "--fingerprint"],
            2,
        ),
    ] {
        let capture = Capture(std::rc::Rc::new(Captured::default()));
        let ctx = Context {
            config_path: config_path.clone(),
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
        let argv: Vec<String> = ["devices", "org-key"]
            .into_iter()
            .chain(flags)
            .map(String::from)
            .collect();
        assert_eq!(run_cli(&argv, &ctx), expected, "{}", capture.0.err.borrow().join("\n"));
        if expected == 0 {
            let output: Value = serde_json::from_str(&capture.0.out.borrow().join("\n")).expect("JSON");
            assert_eq!(
                output,
                json!({ "source": "vault", "fingerprint": "e10e33546b772652b9faffb62b970e29735472a6eba7a68fa7257edb7b8f8dce" })
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 0, "vault input must stay offline");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A Gateway whose `POST /v1/auth/device` answers `link` as `verification_uri_complete` (`{gateway}` is its own URL);
/// anything else answers 404, so the login ends at its first poll.
fn device_gateway(link: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let own = url.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let (status, body) = if request.starts_with("POST /v1/auth/device ") {
                let started = json!({
                    "device_code": "dc_1",
                    "user_code": "BCDF-GHJK",
                    "verification_uri_complete": link.replace("{gateway}", &own),
                    "interval": 0,
                    "expires_in": 600,
                });
                ("200 OK", started.to_string())
            } else {
                ("404 Not Found", r#"{"error":{"message":"not found"}}"#.to_string())
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

/// `nexus auth login` opens Gateway's approval link only on that Gateway's or Nexus UI's origin;
/// a link elsewhere is printed, never opened.
#[test]
fn the_device_login_opens_only_a_link_on_its_own_gateway_or_nexus_ui() {
    for (link, opens) in [
        ("{gateway}/device?code=BCDF-GHJK", true),
        ("https://evil.test/device?code=BCDF-GHJK&calc", false),
        ("http://localhost:1/device?code=BCDF-GHJK", false),
    ] {
        let dir = std::env::temp_dir().join(format!("nexus-device-open-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let url = device_gateway(link);
        let capture = Capture(std::rc::Rc::new(Captured {
            can_open: true,
            ..Captured::default()
        }));
        let ctx = Context {
            config_path: dir.join("config.json"),
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
        let argv: Vec<String> = ["auth", "login", "--url", url.as_str()].map(String::from).to_vec();
        assert_eq!(run_cli(&argv, &ctx), 1, "{link}");
        let expected = link.replace("{gateway}", &url);
        let out = capture.0.out.borrow().join("\n");
        assert!(out.contains(&expected), "{link}: {out}");
        let opened = capture.0.opened.borrow().clone();
        if opens {
            assert_eq!(opened, vec![expected], "{link}");
            assert!(out.contains("Opened your browser."), "{out}");
        } else {
            assert!(opened.is_empty(), "{link}: {opened:?}");
            assert!(out.contains("Open the link in a browser."), "{out}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
