//! Session sync's acceptance test: the CLI is device A. It pairs from the terminal and a trusted device B approves it by
//! its code; then a `nexus claude` session, and its resumption, reach B through a real Gateway on Postgres, and B
//! decrypts every event. Gateway stores ciphertext only.
//!
//! It runs the real CLI (`run_cli`), engine, adapter and `nexus_client::e2e`, with a stand-in `claude` that writes a
//! transcript as Claude Code does. Gateway is a Nexus Gateway binary, started as a process. Gateway is not in this
//! repository, so the test is ignored by default. Run it with `--ignored` when `NEXUS_SYNC_E2E_GATEWAY` names that
//! binary and `TEST_DATABASE_URL` a Postgres; without them it returns early.
//!
//! ```sh
//! NEXUS_SYNC_E2E_GATEWAY=/path/to/nexus-gateway \
//!   TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres \
//!   cargo test --locked -p nexus-cli --test session_sync_e2e -- --ignored --nocapture
//! ```
#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use nexus_cli::{Capture, Captured, Context, run_cli};
use nexus_client::ClientOptions;
use nexus_client::blocking::Client;
use nexus_client::e2e::{self, DeviceKey};
use serde_json::{Value, json};

/// A stand-in `claude`: appends this run's turn to the transcript of `--session-id` or `--resume`, as Claude Code does.
const CLAUDE: &str = r#"#!/bin/sh
[ "$1" = --version ] && echo "9.9.9 (stand-in)" && exit 0
id=""; prompt=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id|--resume) id="$2"; shift 2 ;;
    -p) prompt="$2"; shift 2 ;;
    *) shift ;;
  esac
done
dir="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/$(pwd | sed 's/[^A-Za-z0-9]/-/g')"
mkdir -p "$dir"
f="$dir/$id.jsonl"
n=$(cat "$f" 2>/dev/null | wc -l | tr -d ' ')
at=2026-11-05T10:00:0$((n % 10)).000Z
usage='"usage":{"input_tokens":12,"output_tokens":34,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}'
{
  echo "{\"type\":\"user\",\"uuid\":\"u$n\",\"timestamp\":\"$at\",\"message\":{\"role\":\"user\",\"content\":\"$prompt\"}}"
  echo "{\"type\":\"assistant\",\"uuid\":\"a$n\",\"timestamp\":\"$at\",\"message\":{\"id\":\"m$n\",\"model\":\"claude-opus-5-5\",\"stop_reason\":\"tool_use\",$usage,\"content\":[{\"type\":\"tool_use\",\"id\":\"t$n\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}"
  echo "{\"type\":\"user\",\"uuid\":\"r$n\",\"timestamp\":\"$at\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t$n\",\"content\":\"test result: ok\"}]}}"
  echo "{\"type\":\"assistant\",\"uuid\":\"b$n\",\"timestamp\":\"$at\",\"message\":{\"id\":\"n$n\",\"model\":\"claude-opus-5-5\",\"stop_reason\":\"end_turn\",$usage,\"content\":[{\"type\":\"text\",\"text\":\"Done: $prompt\"}]}}"
} >> "$f"
exit 0
"#;

/// The Gateway process, killed when the test ends.
struct Gateway(std::process::Child, u16);

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_gateway(bin: &str, database: &str) -> Gateway {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("address")
        .port();
    let child = std::process::Command::new(bin)
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("DATABASE_URL", database)
        .env(
            "NEXUS_DB_SCHEMA",
            format!("cli_sync_e2e_{}", uuid::Uuid::new_v4().simple()),
        )
        .env("NEXUS_PROVIDER", "mock")
        .env("NEXUS_SECRET", "cli-sync-e2e-secret")
        .env("NEXUS_UI_SECRET", "ui-secret")
        .env("NEXUS_RATE_LIMIT_PER_MINUTE", "100000")
        .env("NEXUS_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("start nexus-gateway");
    let gateway = Gateway(child, port);
    let probe = Client::new(ClientOptions::new(&url(&gateway)));
    for _ in 0..1800 {
        if probe.get("/readyz").is_ok() {
            return gateway;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("nexus-gateway did not become ready in three minutes");
}

fn url(gateway: &Gateway) -> String {
    format!("http://127.0.0.1:{}", gateway.1)
}

fn person(gateway: &str, key: &str) -> Client {
    Client::new(ClientOptions {
        api_key: Some(key.to_string()),
        ..ClientOptions::new(gateway)
    })
}

fn b64u(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .expect("base64url")
}

fn key32(text: &str) -> [u8; 32] {
    b64u(text).try_into().expect("32 bytes")
}

/// One `nexus` command as device A: its own home, config and environment.
fn nexus(home: &Path, argv: &[&str], env: &HashMap<String, String>) -> (i32, String, String) {
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let work = home.join("acme");
    std::fs::create_dir_all(&work).expect("work");
    let ctx = Context {
        config_path: home.join("nexus").join("config.json"),
        io: Box::new(capture.clone()),
        color: false,
        env: env.clone(),
        poll_ms: Some(200),
        hostname: Some("laptop".into()),
        cwd: work,
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let out = capture.0.out.borrow().join("\n");
    let err = capture.0.err.borrow().join("\n");
    (code, out, err)
}

/// Device B reads a thread as Nexus UI does: its copy at each version, the sender's signature checked against the
/// device list, then each event opened. Returns each event's plaintext.
fn read_as_b(client: &Client, thread: &str, b: (&str, &DeviceKey)) -> Vec<Value> {
    let devices = client.get("/v1/devices").expect("devices")["devices"].clone();
    let mut keys: HashMap<i64, e2e::ContentKey> = HashMap::new();
    let mut opened = Vec::new();
    let page = client
        .get(&format!("/v1/threads/{thread}/events?after=-1&limit=500"))
        .expect("events");
    for event in page["events"].as_array().expect("events") {
        let version = event["keyVersion"].as_i64().expect("sealed");
        let key = keys.entry(version).or_insert_with(|| {
            let copy = client
                .get(&format!("/v1/threads/{thread}/keys?device={}&version={version}", b.0))
                .expect("B's copy");
            let epoch = e2e::epoch_of(version as i32).expect("epoch");
            let locked = b64u(copy["lockedKey"].as_str().expect("locked"));
            let sender = devices
                .as_array()
                .expect("list")
                .iter()
                .find(|device| device["id"] == copy["senderDeviceId"])
                .expect("the sender is listed");
            let message = e2e::thread_key_message(thread, epoch, b.0, &locked).expect("message");
            assert!(e2e::verify(
                &key32(sender["signingKey"].as_str().expect("key")),
                &message,
                &b64u(copy["signature"].as_str().expect("signature"))
            ));
            b.1.unlock_content_key(thread, epoch, b.0, &locked).expect("unlock")
        });
        let seq = event["seq"].as_u64().expect("seq");
        let plaintext = key
            .open_event(thread, seq, &b64u(event["sealed"].as_str().expect("sealed")))
            .expect("B opens the event");
        opened.push(serde_json::from_slice(&plaintext).expect("plaintext"));
    }
    opened
}

#[test]
#[ignore = "requires Nexus Gateway"]
fn a_cli_session_reaches_a_trusted_device_paired_by_code() {
    let (Ok(bin), Ok(database)) = (
        std::env::var("NEXUS_SYNC_E2E_GATEWAY"),
        std::env::var("TEST_DATABASE_URL"),
    ) else {
        eprintln!("skipped: set NEXUS_SYNC_E2E_GATEWAY and TEST_DATABASE_URL to run the CLI's two-device test");
        return;
    };
    let gateway = start_gateway(&bin, &database);
    let gateway_url = url(&gateway);
    let home = std::env::temp_dir().join(format!("nexus-cli-sync-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join("nexus")).expect("home");

    // A person whose organization lets content sync for them alone.
    let signup = Client::new(ClientOptions::new(&gateway_url))
        .post("/v1/tenants", json!({ "name": "Acme", "email": "ada@acme.test" }))
        .expect("sign up");
    let api_key = signup["apiKey"].as_str().expect("api key").to_string();
    let direct = person(&gateway_url, &api_key);
    direct
        .put("/v1/orgs/settings", json!({ "sessionContent": "user" }))
        .expect("content policy");
    std::fs::write(
        home.join("nexus/config.json"),
        json!({ "url": gateway_url, "apiKey": api_key }).to_string(),
    )
    .expect("config");
    let claude = home.join("claude");
    std::fs::write(&claude, CLAUDE).expect("stand-in");
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let env: HashMap<String, String> = [
        ("HOME", home.display().to_string()),
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("NEXUS_CLAUDE_BIN", claude.display().to_string()),
        ("NEXUS_DEVICE_KEY_STORE", "file".into()),
        ("NEXUS_SESSION_SYNC", "1".into()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();

    // B, the person's first device (a browser), is the trust root.
    let b_key = DeviceKey::generate();
    let b = direct
        .post(
            "/v1/devices",
            json!({
                "name": "browser",
                "encryptionKey": e2e::b64u(&b_key.encryption_key()),
                "signingKey": e2e::b64u(&b_key.signing_key()),
            }),
        )
        .expect("register B")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // A pairs from the terminal; B approves the pending device whose keys have the code A shows.
    let approver = {
        let (gateway_url, api_key, b) = (gateway_url.clone(), api_key.clone(), b.clone());
        let b_secret = b_key.secret_bytes();
        std::thread::spawn(move || {
            let b_key = DeviceKey::from_secret_bytes(&b_secret);
            let client = person(&gateway_url, &api_key);
            for _ in 0..300 {
                let devices = client.get("/v1/devices").expect("devices")["devices"].clone();
                if let Some(pending) = devices
                    .as_array()
                    .expect("list")
                    .iter()
                    .find(|d| d["state"] == "pending")
                {
                    let id = pending["id"].as_str().expect("id");
                    let (encryption, signing) = (
                        key32(pending["encryptionKey"].as_str().expect("key")),
                        key32(pending["signingKey"].as_str().expect("key")),
                    );
                    let code = e2e::short_code(&e2e::fingerprint(&encryption, Some(&signing)));
                    let message = e2e::approval_message(id, &encryption, &signing).expect("message");
                    client
                        .post(
                            &format!("/v1/devices/{id}/approve"),
                            json!({ "approverDeviceId": b, "signature": e2e::b64u(&b_key.sign(&message)) }),
                        )
                        .expect("approve");
                    return code;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("A never registered");
        })
    };
    // The person checks that B, the approver A names, shows this code on its own screen.
    let b_code = e2e::short_code(&b_key.fingerprint());
    let (code, out, err) = nexus(&home, &["devices", "pair", "--approver-code", &b_code], &env);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("is confirmed on this computer"), "{out}");
    let approved_code = approver.join().expect("approver");
    let grouped = approved_code
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(out.contains(&grouped), "A showed the code B approved: {out}");
    assert!(out.contains("Approved."), "{out}");
    let (_, listed, _) = nexus(&home, &["devices", "--json"], &env);
    let listed: Value = serde_json::from_str(&listed).expect("json");
    let a = listed["thisDevice"].as_str().expect("A is listed").to_string();
    assert_ne!(a, b);

    // A runs a session, then resumes it: one thread, synced to the end.
    let (code, out, err) = nexus(&home, &["claude", "--no-skills", "-p", "Fix the flaky test"], &env);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(err.contains("Synced to Nexus UI"), "{err}");
    let threads = direct.get("/v1/threads").expect("threads")["threads"].clone();
    let thread = threads[0]["threadId"].as_str().expect("a synced thread").to_string();
    let conversation = std::fs::read_dir(home.join(".claude/projects"))
        .expect("projects")
        .flatten()
        .flat_map(|project| std::fs::read_dir(project.path()).expect("project").flatten())
        .map(|file| PathBuf::from(file.file_name()))
        .next()
        .expect("a transcript");
    let conversation = conversation.file_stem().expect("id").to_string_lossy().to_string();
    let (code, _, err) = nexus(
        &home,
        &["claude", "--no-skills", "--resume", &conversation, "-p", "and the docs"],
        &env,
    );
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("Synced to Nexus UI"), "{err}");
    let threads = direct.get("/v1/threads").expect("threads")["threads"].clone();
    assert_eq!(
        threads.as_array().expect("list").len(),
        1,
        "the resumed conversation is the same thread"
    );

    // B decrypts every event, in order.
    let opened = read_as_b(&direct, &thread, (&b, &b_key));
    let events: Vec<Value> = opened
        .iter()
        .map(|plaintext| serde_json::from_str(plaintext["event_json"].as_str().expect("event json")).expect("event"))
        .collect();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["kind"].as_str().expect("kind"))
        .collect();
    let turn = [
        "user",
        "turnStart",
        "tool",
        "usage",
        "toolResult",
        "text",
        "usage",
        "turnEnd",
    ];
    let expected: Vec<&str> = ["session"]
        .iter()
        .chain(turn.iter())
        .chain(["session"].iter())
        .chain(turn.iter())
        .copied()
        .collect();
    assert_eq!(kinds, expected);
    assert_eq!(events[1]["text"], "Fix the flaky test");
    assert_eq!(events[10]["text"], "and the docs");
    assert!(events[0]["nexusSessionId"].is_string(), "{}", events[0]);
    assert_eq!(events[9]["resumed"], true);
    assert!(
        opened
            .iter()
            .all(|plaintext| plaintext["thread"]["title"] == "Fix the flaky test")
    );
    assert_eq!(opened[0]["thread"]["project_name"], "acme");

    // Gateway holds ciphertext and usage only.
    let raw = direct
        .get(&format!("/v1/threads/{thread}/events?after=-1&limit=500"))
        .expect("events")
        .to_string();
    for secret in ["flaky", "the docs", "cargo test", "acme"] {
        assert!(!raw.contains(secret), "{secret} reached Gateway");
    }
    drop(gateway);
    let _ = std::fs::remove_dir_all(home);
}
