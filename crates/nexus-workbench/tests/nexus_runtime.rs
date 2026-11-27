//! Which `nexus` runs Nexus's agent (`nexus_runtime`): the JavaScript CLI, a native CLI from before the
//! handshake or with an older protocol, and another program by that name are each told apart and passed over; the
//! first `nexus` that answers the handshake runs the agent, wherever it is on `PATH`; one replaced at the same path is
//! checked again; and the install from the Gateway turns the installer's layout native, or changes nothing.
//!
//! The stand-ins answer as the CLI's own handshake fixture says. With
//! `NEXUS_CLI_BIN` naming a build of the real CLI, the last two tests
//! check it, install it from a stand-in Gateway, and run a session on it against a local one.

#![cfg(unix)]

mod support;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nexus_workbench::event::{PermissionMode, SessionAgent, WbEvent};
use nexus_workbench::nexus_runtime::{self, Fetch, Runtime, Unusable};
use nexus_workbench::process::write_fake_script;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::*;

/// The JavaScript CLI as the installer lays it out: a wrapper that starts the bundle with node, which reads `__agent`
/// as a product name.
const JAVASCRIPT: &str = r#"#!/bin/sh
# exec node "$HOME/.nexus/lib/nexus.mjs" "$@"
case "$1" in
  --version) echo "nexus 0.21.0+d72f8fc" ;;
  *) echo "Unknown command or product \"$1\" (products: intelligence)" >&2; exit 2 ;;
esac
"#;

/// A native CLI from before the handshake: `__agent` starts, and says it is not signed in.
const NO_HANDSHAKE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "nexus 0.20.0+abc1234" ;;
  __agent) echo '{"type":"error","message":"Not signed in: run `nexus auth login`, or set NEXUS_API_KEY."}'; exit 2 ;;
  *) exit 2 ;;
esac
"#;

/// A native CLI whose protocol is older than the workbench's.
const OLD_PROTOCOL: &str = r#"#!/bin/sh
echo '{"type":"handshake","protocol":0,"version":"0.21.5+abc1234"}'
"#;

/// A native CLI whose protocol is newer than the workbench drives.
const NEW_PROTOCOL: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "nexus 0.30.0+abc1234" ;;
  *) echo '{"type":"handshake","protocol":2,"version":"0.30.0+abc1234"}' ;;
esac
"#;

/// Another program by the name.
const OTHER: &str = "#!/bin/sh\necho 'usage: nexus-network <command>' >&2\nexit 1\n";

/// The handshake line the CLI's own tests pin.
fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/cli/tests/fixtures/agent-handshake.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}"))
}

/// A native CLI that answers the handshake as the fixture says. With `HANDSHAKE_RECORD` set, it writes the
/// environment it was started with there.
fn compatible(version: &str) -> String {
    let mut line = fixture();
    line["version"] = json!(version);
    format!(
        "#!/bin/sh\n[ -n \"$HANDSHAKE_RECORD\" ] && /usr/bin/env > \"$HANDSHAKE_RECORD\"\nif [ \"$1 $2\" = \"__agent \
         --handshake\" ]; then echo '{line}'; exit 0; fi\necho \"nexus {version}\"\n"
    )
}

/// A folder of its own for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-runtime-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::canonicalize(&dir).unwrap_or_else(|e| panic!("{e}"))
}

/// `dir/<sub>/nexus`, running `body`.
fn nexus_in(dir: &Path, sub: &str, body: &str) -> PathBuf {
    let folder = dir.join(sub);
    std::fs::create_dir_all(&folder).unwrap_or_else(|e| panic!("{e}"));
    let path = folder.join("nexus");
    write_fake_script(&path, body);
    path
}

/// The agents' environment: these folders on `PATH`, and `home` as the home folder.
fn agents_env(home: &Path, path: &[&Path]) -> HashMap<String, String> {
    let path: Vec<String> = path.iter().map(|dir| dir.display().to_string()).collect();
    HashMap::from([
        ("PATH".to_string(), path.join(":")),
        ("HOME".to_string(), home.display().to_string()),
    ])
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("/"))
}

#[test]
fn nothing_on_path_is_missing() {
    let dir = folder("missing");
    let env = agents_env(&dir.join("home"), &[&dir.join("empty")]);
    assert_eq!(nexus_runtime::select(&env), Runtime::Missing);
    assert_eq!(nexus_workbench::nexus_runtime::info(&Runtime::Missing).path, None);
    let refusal = nexus_runtime::refusal(&Runtime::Missing, nexus_runtime::INSTALL_HERE);
    assert!(
        refusal.starts_with("Nexus's agent needs the native nexus CLI. None is installed."),
        "{refusal}"
    );
    assert!(refusal.contains("choose Install on the Nexus card"), "{refusal}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_kind_of_nexus_is_told_apart() {
    let dir = folder("kinds");
    let env = agents_env(&dir.join("home"), &[]);
    let kind = |sub: &str, body: &str| nexus_runtime::check(&nexus_in(&dir, sub, body), &env);
    assert!(matches!(
        kind("js", JAVASCRIPT),
        Runtime::Unusable { why: Unusable::JavaScript, version: Some(v), .. } if v == "0.21.0+d72f8fc"
    ));
    assert!(matches!(
        kind("no-handshake", NO_HANDSHAKE),
        Runtime::Unusable { why: Unusable::Outdated, version: Some(v), .. } if v == "0.20.0+abc1234"
    ));
    assert!(matches!(
        kind("old-protocol", OLD_PROTOCOL),
        Runtime::Unusable { why: Unusable::Outdated, version: Some(v), .. } if v == "0.21.5+abc1234"
    ));
    assert!(matches!(
        kind("new-protocol", NEW_PROTOCOL),
        Runtime::Unusable { why: Unusable::Other, version: Some(v), .. } if v == "0.30.0+abc1234"
    ));
    assert!(matches!(
        kind("other", OTHER),
        Runtime::Unusable {
            why: Unusable::Other,
            version: None,
            ..
        }
    ));
    let good = nexus_in(&dir, "good", &compatible("0.22.0+abc1234"));
    assert_eq!(
        nexus_runtime::check(&good, &env),
        Runtime::Ready {
            path: good.clone(),
            version: "0.22.0+abc1234".into(),
            protocol: nexus_runtime::PROTOCOL,
        }
    );
    let info = nexus_runtime::info(&nexus_runtime::check(&good, &env));
    assert_eq!((info.path.as_deref(), info.problem), (good.to_str(), None));
    let _ = std::fs::remove_dir_all(dir);
}

/// PATH conflicts: whatever comes first, the first `nexus` that answers runs the agent; with none, the first one found
/// is named. The installer's own folder counts when the app's `PATH` lacks it, and a folder on both is tried once.
#[test]
fn the_first_nexus_that_answers_runs_the_agent() {
    let dir = folder("conflicts");
    let home = dir.join("home");
    let js = nexus_in(&dir, "npm", JAVASCRIPT);
    let old = nexus_in(&dir, "cargo", NO_HANDSHAKE);
    let good = nexus_in(&dir, "local", &compatible("0.22.0+abc1234"));
    let picked = nexus_runtime::select(&agents_env(&home, &[parent(&js), parent(&old), parent(&good)]));
    assert_eq!(picked.ready(), Some(good.as_path()));

    let none = nexus_runtime::select(&agents_env(&home, &[parent(&js), parent(&old)]));
    assert!(matches!(&none, Runtime::Unusable { path, why: Unusable::JavaScript, .. } if *path == js));
    let said = nexus_runtime::problem(&none).unwrap_or_default();
    assert!(
        said.contains(&js.display().to_string()) && said.contains("JavaScript CLI"),
        "{said}"
    );
    assert!(
        nexus_runtime::info(&none).path.is_none(),
        "an unusable nexus is not shown as installed"
    );

    // The installer's ~/.nexus/bin/nexus, off the app's PATH, after a JavaScript CLI on it.
    let installed = nexus_in(&home, ".nexus/bin", &compatible("0.22.0+abc1234"));
    let env = agents_env(&home, &[parent(&js)]);
    assert_eq!(nexus_runtime::select(&env).ready(), Some(installed.as_path()));
    let both = agents_env(&home, &[parent(&installed), parent(&js)]);
    assert_eq!(nexus_runtime::candidates(&both, false), [installed.clone(), js.clone()]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_check_signs_in_nowhere() {
    let dir = folder("check-env");
    let good = nexus_in(&dir, "local", &compatible("0.22.0+abc1234"));
    let record = dir.join("env.txt");
    let mut env = agents_env(&dir.join("home"), &[parent(&good)]);
    env.insert("NEXUS_API_KEY".into(), "nx-real-key".into());
    env.insert("NEXUS_URL".into(), "https://gw.example".into());
    env.insert("HANDSHAKE_RECORD".into(), record.display().to_string());
    assert!(nexus_runtime::select(&env).ready().is_some());
    let seen = std::fs::read_to_string(&record).unwrap_or_default();
    assert!(!seen.contains("nx-real-key"), "{seen}");
    assert!(seen.contains("NEXUS_URL=http://127.0.0.1:9"), "{seen}");
    assert!(seen.contains("NEXUS_NO_UPDATE_CHECK=1"), "{seen}");
    // The empty config is in a folder of the check's own, not at a name another user could take first, and the folder
    // is gone once the check ends.
    let config = seen
        .lines()
        .find_map(|line| line.strip_prefix("NEXUS_CONFIG="))
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("no NEXUS_CONFIG: {seen}"));
    let folder = parent(&config).to_path_buf();
    assert!(
        folder
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("nexus-agent-handshake-")),
        "{}",
        config.display()
    );
    assert!(!folder.exists(), "{} is left", folder.display());
    let _ = std::fs::remove_dir_all(dir);
}

/// `nexus update` or an install replaces the file at the same path: the next check sees the new one, either way.
#[test]
fn a_nexus_replaced_at_the_same_path_is_checked_again() {
    let dir = folder("replaced");
    let path = nexus_in(&dir, "bin", JAVASCRIPT);
    let env = agents_env(&dir.join("home"), &[parent(&path)]);
    assert!(matches!(
        nexus_runtime::select(&env),
        Runtime::Unusable {
            why: Unusable::JavaScript,
            ..
        }
    ));
    write_fake_script(&path, &compatible("0.22.0+abc1234"));
    assert_eq!(nexus_runtime::select(&env).ready(), Some(path.as_path()));
    write_fake_script(&path, NO_HANDSHAKE);
    assert!(matches!(
        nexus_runtime::select(&env),
        Runtime::Unusable {
            why: Unusable::Outdated,
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(dir);
}

/// The installer's JavaScript layout: `bin/nexus` starts `lib/nexus.mjs` with node, and bakes in the URLs.
fn javascript_layout(home: &Path) -> PathBuf {
    let dir = home.join(".nexus");
    std::fs::create_dir_all(dir.join("lib")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.join("lib/nexus.mjs"), "/* @nexus/cli 0.21.0+d72f8fc */\n").unwrap_or_else(|e| panic!("{e}"));
    let wrapper = format!(
        "#!/bin/sh\nNEXUS_URL=\"${{NEXUS_URL:-https://gw.example}}\" NEXUS_UI_URL=\"${{NEXUS_UI_URL:-https://ui.gw.example}}\" \
         exec node \"{}\" \"$@\"\n",
        dir.join("lib/nexus.mjs").display()
    );
    nexus_in(&dir, "bin", &wrapper)
}

#[test]
fn install_turns_the_javascript_layout_native_and_keeps_its_urls() {
    let dir = folder("install-js");
    let home = dir.join("home");
    let wrapper = javascript_layout(&home);
    let npm = nexus_in(&dir, "npm", JAVASCRIPT);
    let env = agents_env(&home, &[parent(&npm), parent(&wrapper)]);
    let before = nexus_runtime::select(&env);
    assert!(
        matches!(
            before,
            Runtime::Unusable {
                why: Unusable::JavaScript,
                ..
            }
        ),
        "{before:?}"
    );

    let installed = nexus_runtime::install(
        &home.join(".nexus"),
        compatible("0.22.0+abc1234").as_bytes(),
        "https://gw.example",
        &env,
        false,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(installed.ready(), Some(wrapper.as_path()));
    let lib = home.join(".nexus/lib/nexus");
    let text = std::fs::read_to_string(&wrapper).unwrap_or_default();
    assert_eq!(
        text,
        format!(
            "#!/bin/sh\nNEXUS_URL=\"${{NEXUS_URL:-https://gw.example}}\" NEXUS_UI_URL=\"${{NEXUS_UI_URL:-https://ui.gw.example}}\" \
             exec \"{}\" \"$@\"\n",
            lib.display()
        )
    );
    assert!(home.join(".nexus/lib/nexus.mjs").is_file(), "the bundle stays");
    // The npm JavaScript CLI still comes first on PATH; the installer's native one runs the agent.
    assert_eq!(nexus_runtime::select(&env).ready(), Some(wrapper.as_path()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn install_refuses_a_cli_without_the_handshake_and_changes_nothing() {
    let dir = folder("install-refused");
    let home = dir.join("home");
    let wrapper = javascript_layout(&home);
    let before = std::fs::read_to_string(&wrapper).unwrap_or_default();
    let env = agents_env(&home, &[]);
    for body in [NO_HANDSHAKE, OLD_PROTOCOL, JAVASCRIPT] {
        let refused = nexus_runtime::install(&home.join(".nexus"), body.as_bytes(), "https://gw.example", &env, false);
        let error = refused.err().unwrap_or_default();
        assert!(
            error.contains("cannot run Nexus's agent yet") && error.ends_with("Nothing was changed."),
            "{error}"
        );
    }
    assert_eq!(std::fs::read_to_string(&wrapper).unwrap_or_default(), before);
    let lib: Vec<String> = std::fs::read_dir(home.join(".nexus/lib"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(lib, ["nexus.mjs"], "no binary and no staged file left");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn install_keeps_the_previous_binary_and_leaves_a_wrapper_not_its_own_alone() {
    let dir = folder("install-native");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let nexus = home.join(".nexus");
    // A first install writes the wrapper, with the Gateway's URL as its default.
    let first = nexus_runtime::install(
        &nexus,
        compatible("0.22.0+abc1234").as_bytes(),
        "https://gw.example",
        &env,
        false,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let wrapper = nexus.join("bin/nexus");
    assert_eq!(first.ready(), Some(wrapper.as_path()));
    assert!(
        std::fs::read_to_string(&wrapper)
            .unwrap_or_default()
            .contains("NEXUS_URL=\"${NEXUS_URL:-https://gw.example}\" exec ")
    );
    // A second keeps the first as nexus.previous, as `nexus update --rollback` reads it.
    let second = nexus_runtime::install(
        &nexus,
        compatible("0.23.0+def5678").as_bytes(),
        "https://gw.example",
        &env,
        false,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(&second, Runtime::Ready { version, .. } if version == "0.23.0+def5678"));
    assert!(
        std::fs::read_to_string(nexus.join("lib/nexus.previous"))
            .unwrap_or_default()
            .contains("0.22.0")
    );
    // A bin/nexus that is not the installer's is left alone: the binary itself runs the agent.
    write_fake_script(&wrapper, "#!/bin/sh\nexec /opt/my-build/nexus \"$@\"\n");
    let third = nexus_runtime::install(
        &nexus,
        compatible("0.24.0+0123456").as_bytes(),
        "https://gw.example",
        &env,
        false,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(third.ready(), Some(nexus.join("lib/nexus").as_path()));
    assert!(
        std::fs::read_to_string(&wrapper)
            .unwrap_or_default()
            .contains("/opt/my-build/nexus")
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A `bin/nexus` that is not text (a binary put there) is not the installer's wrapper: it is left alone.
#[test]
fn install_leaves_a_binary_wrapper_alone() {
    let dir = folder("install-binary-wrapper");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let nexus = home.join(".nexus");
    let wrapper = nexus.join("bin/nexus");
    std::fs::create_dir_all(nexus.join("bin")).unwrap_or_else(|e| panic!("{e}"));
    let binary = b"\x7fELF\xff\xfe\x00\x01";
    std::fs::write(&wrapper, binary).unwrap_or_else(|e| panic!("{e}"));
    let installed = nexus_runtime::install(
        &nexus,
        compatible("0.22.0+abc1234").as_bytes(),
        "https://gw.example",
        &env,
        false,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(installed.ready(), Some(nexus.join("lib/nexus").as_path()));
    assert_eq!(std::fs::read(&wrapper).unwrap_or_default(), binary);
    let _ = std::fs::remove_dir_all(dir);
}

/// A stand-in Gateway for `install_from`: `/cli/version?target=…` as given, and the binary at its URL.
fn gateway_serving(version: (u16, Value), binary: Vec<u8>) -> Fetch {
    Arc::new(move |url: String| {
        let answer = if url.contains("/cli/version?target=") {
            (version.0, version.1.to_string().into_bytes())
        } else if url.ends_with(&format!("nexus-{}", nexus_runtime::target())) {
            (200, binary.clone())
        } else {
            (404, b"{}".to_vec())
        };
        Box::pin(async move { Ok(answer) })
    })
}

fn published(binary: &[u8], target: &str) -> Value {
    let sha: String = Sha256::digest(binary).iter().map(|b| format!("{b:02x}")).collect();
    json!({
        "version": "0.22.0+abc1234", "sha256": sha, "minimum": "0.2.0", "target": target,
        "url": format!("https://downloads.example/cli/stable/0.22.0/nexus-{}", nexus_runtime::target()),
    })
}

#[tokio::test]
async fn install_from_the_gateway_checks_the_offer_the_checksum_and_the_handshake() {
    let dir = folder("install-from");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let target = nexus_runtime::target();
    let binary = compatible("0.22.0+abc1234").into_bytes();

    let unpublished = json!({ "error": { "code": "cli_target_unavailable", "type": "not_found", "message": "none" } });
    let error = nexus_runtime::install_from(
        "https://gw.example/",
        env.clone(),
        gateway_serving((404, unpublished), binary.clone()),
    )
    .await
    .err()
    .unwrap_or_default();
    assert_eq!(
        error,
        format!(
            "Nexus Gateway at https://gw.example does not publish the native nexus CLI for {target} yet, so Nexus's \
             agent cannot be installed. Use Claude Code or Codex until it does."
        )
    );
    let mut wrong = published(&binary, &target);
    wrong["sha256"] = json!("0".repeat(64));
    let error = nexus_runtime::install_from(
        "https://gw.example",
        env.clone(),
        gateway_serving((200, wrong), binary.clone()),
    )
    .await
    .err()
    .unwrap_or_default();
    assert!(
        error.contains("does not match its checksum. Nothing was changed."),
        "{error}"
    );
    let other = published(&binary, "plan9-mips");
    let error = nexus_runtime::install_from(
        "https://gw.example",
        env.clone(),
        gateway_serving((200, other), binary.clone()),
    )
    .await
    .err()
    .unwrap_or_default();
    assert!(error.contains("in a form this app does not read"), "{error}");
    assert!(!home.join(".nexus").exists(), "nothing was installed");

    let installed = nexus_runtime::install_from(
        "https://gw.example",
        env.clone(),
        gateway_serving((200, published(&binary, &target)), binary),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(installed.ready(), Some(home.join(".nexus/bin/nexus").as_path()));
    assert_eq!(nexus_runtime::select(&env), installed);
    let _ = std::fs::remove_dir_all(dir);
}

/// The real CLI, when `NEXUS_CLI_BIN` names a build of it.
fn real_cli() -> Option<PathBuf> {
    let path = std::env::var_os("NEXUS_CLI_BIN").map(PathBuf::from);
    if path.is_none() {
        eprintln!("NEXUS_CLI_BIN is not set: the real CLI's checks are skipped");
    }
    path
}

/// The real CLI answers the handshake, and the runtime the install produces from it answers too.
#[tokio::test]
async fn the_real_cli_answers_the_handshake_and_installs() {
    let Some(real) = real_cli() else { return };
    let dir = folder("real-install");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let checked = nexus_runtime::check(&real, &env);
    assert!(
        matches!(&checked, Runtime::Ready { protocol, .. } if *protocol >= nexus_runtime::PROTOCOL),
        "{checked:?}"
    );

    let binary = std::fs::read(&real).unwrap_or_else(|e| panic!("{e}"));
    let target = nexus_runtime::target();
    let installed = nexus_runtime::install_from(
        "https://gw.example",
        env.clone(),
        gateway_serving((200, published(&binary, &target)), binary),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let wrapper = home.join(".nexus/bin/nexus");
    assert_eq!(installed.ready(), Some(wrapper.as_path()));
    // The wrapper starts the binary; its handshake comes back through it, with the same version.
    assert!(
        matches!((&installed, &checked), (Runtime::Ready { version: a, .. }, Runtime::Ready { version: b, .. }) if a == b)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A Gateway on a free local port: `GET /v1/spend` is a budget, any other request a streamed answer.
fn local_gateway() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
    let address = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || answer(stream));
        }
    });
    format!("http://{address}")
}

fn answer(mut stream: TcpStream) {
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

/// The workbench drives the real CLI as the desktop does (its arguments, its lines, the app's MCP server), signed in
/// with a fake key to a local Gateway: the turn streams and ends, and the next message goes on in the same conversation.
#[tokio::test]
async fn a_session_runs_on_the_real_cli_against_a_local_gateway() {
    let Some(real) = real_cli() else { return };
    let s = setup("nexus-real-cli", Default::default());
    *s.host.nexus.lock().unwrap_or_else(|p| p.into_inner()) = Some(real);
    s.host.extra.lock().unwrap_or_else(|p| p.into_inner()).extend([
        ("NEXUS_URL".to_string(), local_gateway()),
        ("NEXUS_API_KEY".to_string(), "nx-fake-key".to_string()),
        (
            "NEXUS_CONFIG".to_string(),
            s.root.join("cli/config.json").display().to_string(),
        ),
        ("NEXUS_NO_UPDATE_CHECK".to_string(), "1".to_string()),
    ]);
    let new = nexus_workbench::NewThread {
        name: None,
        agent: SessionAgent::Nexus,
        permission_mode: PermissionMode::Auto,
        ..new_root(&s.project, "Reply OK without using tools.")
    };
    let created = s.workbench.create_thread(new).await.unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 1).await;
    let said = |events: &[nexus_workbench::event::StoredEvent]| {
        events
            .iter()
            .filter(|e| matches!(&e.event, WbEvent::Text { text, .. } if text == "Hello from the Gateway."))
            .count()
    };
    assert_eq!(said(&events), 1, "{events:#?}");
    assert!(
        !events.iter().any(|e| matches!(e.event, WbEvent::Error { .. })),
        "{events:#?}"
    );
    let launch = launches_of(&s, &created.id).pop().unwrap_or_else(|| panic!("a launch"));
    assert_eq!(launch.args[..2], ["__agent", "--session"]);

    s.workbench
        .send(&created.id, "And again.")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let events = wait_for(&s.workbench, &created.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(said(&events), 2, "{events:#?}");
    let kept = std::fs::read_dir(s.root.join("cli/agent-sessions"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(kept, 1, "one conversation, kept by the CLI");
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A CLI that speaks protocol 1 and then fails is not called too old: it is a CLI that does not run.
#[test]
fn a_handshake_that_then_fails_is_not_too_old() {
    let dir = folder("handshake-fails");
    let line = fixture();
    let path = nexus_in(&dir, "bin", &format!("#!/bin/sh\necho '{line}'\nexit 1\n"));
    let env = agents_env(&dir.join("home"), &[parent(&path)]);
    let checked = nexus_runtime::check(&path, &env);
    assert!(
        matches!(
            checked,
            Runtime::Unusable {
                why: Unusable::Other,
                ..
            }
        ),
        "{checked:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A `nexus` that has not changed is checked once, not on every page load: a hung one would cost 25 s each time.
#[test]
fn an_unchanged_nexus_is_checked_once() {
    let dir = folder("checked-once");
    let runs = dir.join("runs");
    let body = compatible("0.22.0+abc1234").replacen(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\necho run >> '{}'\n", runs.display()),
        1,
    );
    let path = nexus_in(&dir, "bin", &body);
    let env = agents_env(&dir.join("home"), &[parent(&path)]);
    let count = || std::fs::read_to_string(&runs).unwrap_or_default().lines().count();
    assert!(nexus_runtime::select(&env).ready().is_some());
    let once = count();
    assert!(once >= 1);
    assert!(nexus_runtime::select(&env).ready().is_some());
    assert_eq!(count(), once, "the second select ran it again");
    let _ = std::fs::remove_dir_all(dir);
}

/// Install replaces `lib/nexus` in one rename: a terminal or a session that starts `nexus` meanwhile always finds one.
#[test]
fn install_never_leaves_lib_nexus_missing() {
    let dir = folder("swap");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let nexus = home.join(".nexus");
    let install = |version: &str| {
        nexus_runtime::install(
            &nexus,
            compatible(version).as_bytes(),
            "https://gw.example",
            &env,
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    };
    install("0.22.0+abc1234");
    let program = nexus.join("lib/nexus");
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = {
        let (program, done) = (program.clone(), done.clone());
        std::thread::spawn(move || {
            let mut missing = 0;
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                if !program.exists() {
                    missing += 1;
                }
            }
            missing
        })
    };
    for n in 0..20 {
        install(&format!("0.22.{n}+abc1234"));
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(watcher.join().unwrap_or(-1), 0, "lib/nexus was missing");
    assert!(
        std::fs::read_to_string(nexus.join("lib/nexus.previous"))
            .unwrap_or_default()
            .contains("0.22.18+abc1234")
    );
    let left: Vec<_> = std::fs::read_dir(nexus.join("lib"))
        .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert_eq!(left.len(), 2, "only nexus and nexus.previous: {left:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Two Installs at once (the menu's and the new-session blocker's) both succeed: one waits for the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installs_at_once_take_turns() {
    let dir = folder("installs-at-once");
    let home = dir.join("home");
    let env = agents_env(&home, &[]);
    let target = nexus_runtime::target();
    let binary = compatible("0.22.0+abc1234").into_bytes();
    let installs: Vec<_> = (0..6)
        .map(|_| {
            let (env, fetch) = (
                env.clone(),
                gateway_serving((200, published(&binary, &target)), binary.clone()),
            );
            tokio::spawn(async move { nexus_runtime::install_from("https://gw.example", env, fetch).await })
        })
        .collect();
    for install in installs {
        let installed = install.await.unwrap_or_else(|e| panic!("{e}"));
        assert!(installed.is_ok(), "{installed:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
