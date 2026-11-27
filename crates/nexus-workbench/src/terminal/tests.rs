use super::*;

#[test]
fn bounded_output_reports_the_gap_and_preserves_byte_cursors() {
    let mut output = Output::default();
    output.push(&vec![b'x'; OUTPUT_LIMIT + 42]);
    let (bytes, next, truncated) = output.read(0, 17);
    assert_eq!(bytes, vec![b'x'; 17]);
    assert_eq!(next, 59);
    assert!(truncated);
    assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
    let (_, next, truncated) = output.read(next, 12);
    assert_eq!(next, 71);
    assert!(!truncated);
}

#[test]
fn control_bytes_are_valid_input_and_commands_reject_nul() {
    assert_eq!(
        text(&json!({"data":"\r"}), "data", 5).expect("control byte input"),
        "\r"
    );
    assert_eq!(
        text(&json!({"data":"\u{3}"}), "data", 5).expect("control byte input"),
        "\u{3}"
    );
    assert!(text(&json!({"command":"echo\u{0}bad"}), "command", 50).is_err());
}

#[test]
fn terminal_ownership_and_account_boundaries_are_enforced() {
    let terminals = Terminals::default();
    terminals.context("org", Some("person-a".into()));
    let args = json!({"command":"echo hello","request_id":"one"});
    let entry = terminals
        .stage("org", "thread", PathBuf::from("/unused"), &args)
        .expect("stage fixture execution");
    assert!(terminals.find("other-org", "thread", &entry.info().id).is_err());
    assert!(terminals.find("org", "other-thread", &entry.info().id).is_err());
    assert_eq!(
        terminals
            .stage("org", "thread", PathBuf::from("/unused"), &args)
            .expect("stage fixture execution")
            .info()
            .id,
        entry.info().id
    );
    assert!(
        terminals
            .stage(
                "org",
                "thread",
                PathBuf::from("/unused"),
                &json!({"command":"echo different","request_id":"one"})
            )
            .is_err()
    );
    terminals.context("org", Some("person-b".into()));
    assert!(terminals.find("org", "thread", &entry.info().id).is_err());
    assert_eq!(entry.info().status, "cancelled");
}

#[test]
fn a_marked_command_carries_the_commit_trailer_hook_and_never_the_apps_secrets() {
    use nexus_client::commit_trailer::{Surface, VARIABLE, parameters};
    let app = || {
        [("PATH", "/bin"), ("NEXUS_MCP_TOKEN", "secret"), ("TAURI_X", "1")].map(|(k, v)| (k.to_string(), v.to_string()))
    };
    let plain = process::environment(app(), false, false);
    assert!(!plain.contains_key(VARIABLE));
    assert!(!plain.contains_key("NEXUS_MCP_TOKEN") && !plain.contains_key("TAURI_X"));
    assert_eq!(plain["PATH"], "/bin");
    let marked = process::environment(app(), true, false);
    assert_eq!(marked[VARIABLE], parameters(None, Surface::Desktop));
    assert!(!marked.contains_key("NEXUS_MCP_TOKEN"));
    // A desktop started from a Nexus session keeps that session's hook.
    let outer = parameters(None, Surface::Cli);
    let nested = process::environment([(VARIABLE.to_string(), outer.clone())], true, false);
    assert_eq!(nested[VARIABLE], outer);
}

/// A command gets only the variables a shell needs, never one it was not told about, whatever its name.
#[test]
fn a_command_gets_only_the_allowed_variables() {
    let app = [
        ("PATH", "/bin"),
        ("HOME", "/home/me"),
        ("LC_ALL", "C.UTF-8"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("SSH_AUTH_SOCK", "/tmp/agent"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("NEXUS_API_KEY", "nx_secret"),
        ("ANTHROPIC_API_KEY", "sk_secret"),
        ("GITHUB_TOKEN", "gh_secret"),
        ("AWS_SECRET_ACCESS_KEY", "aws_secret"),
        ("LD_PRELOAD", "/tmp/evil.so"),
        ("NEXUS_MCP_TOKEN", "mcp_secret"),
        ("NEXUS_SOMETHING_NEW", "x"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("no_proxy", "localhost"),
        ("XAUTHORITY", "/run/user/1000/xauth"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()));
    let env = process::environment(app, false, false);
    let mut keys: Vec<_> = env.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "COLORTERM",
            "DBUS_SESSION_BUS_ADDRESS",
            "HOME",
            "HTTPS_PROXY",
            "LC_ALL",
            "PATH",
            "SSH_AUTH_SOCK",
            "TERM",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "XDG_RUNTIME_DIR",
            "no_proxy"
        ]
    );
    assert!(env.values().all(|value| !value.contains("secret")), "{env:?}");
}

#[test]
fn a_kept_commit_mark_belongs_to_one_sign_in() {
    let terminals = Terminals::default();
    terminals.context("org", Some("person-a".into()));
    assert!(!terminals.marks_commits());
    // An answer for a sign-in that was replaced while it was asked is not kept.
    terminals.keep_marks_commits("org", &Some("person-b".into()));
    assert!(!terminals.marks_commits());
    terminals.keep_marks_commits("org", &Some("person-a".into()));
    assert!(terminals.marks_commits());
    terminals.context("org", Some("person-a".into()));
    assert!(terminals.marks_commits());
    terminals.context("other-org", Some("person-a".into()));
    assert!(!terminals.marks_commits());
}

#[test]
fn an_evicted_execution_is_not_run_again_on_retry() {
    let terminals = Terminals::default();
    let args = json!({"command":"echo once","request_id":"one"});
    let entry = terminals
        .stage("", "thread", PathBuf::from("/unused"), &args)
        .expect("stage fixture execution");
    entry.state().info.status = "completed".into();
    // Output eviction leaves an execution receipt.
    terminals.0.lock().expect("terminal registry lock").entries.clear();
    let error = terminals
        .stage("", "thread", PathBuf::from("/unused"), &args)
        .err()
        .expect("expired execution must be refused");
    assert!(error.starts_with("terminal_execution_expired"));
}

#[test]
fn input_requires_the_current_lease_full_access_and_agent_control() {
    let terminals = Terminals::default();
    let entry = terminals
        .stage(
            "",
            "t",
            PathBuf::from("/unused"),
            &json!({"command":"echo x","request_id":"x"}),
        )
        .expect("stage fixture execution");
    let mut state = entry.state();
    state.info.owner = "agent".into();
    state.info.lease = 3;
    let args = json!({"lease":3});
    for mode in [
        PermissionMode::Plan,
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
    ] {
        assert!(agent_write(&state, mode, &args).is_err());
    }
    assert!(agent_write(&state, PermissionMode::Auto, &args).is_ok());
    assert!(agent_write(&state, PermissionMode::BypassPermissions, &args).is_ok());
    state.info.lease = 4;
    assert!(agent_write(&state, PermissionMode::BypassPermissions, &args).is_err());
    state.info.owner = "user".into();
    assert!(agent_write(&state, PermissionMode::BypassPermissions, &json!({"lease":4})).is_err());
    state.info.owner = "agent".into();
    state.info.private = true;
    assert!(agent_write(&state, PermissionMode::BypassPermissions, &json!({"lease":4})).is_err());
}

#[test]
fn cursor_queries_are_found_whole_and_split_across_reads() {
    let mut tail = Vec::new();
    assert_eq!(process::cursor_queries(&mut tail, b"\x1b[6n\x1b[?25l"), 1);
    assert_eq!(process::cursor_queries(&mut tail, b"plain output"), 0);
    assert_eq!(process::cursor_queries(&mut tail, b"a\x1b["), 0);
    assert_eq!(process::cursor_queries(&mut tail, b"6nb\x1b[6n"), 2);
    assert_eq!(process::cursor_queries(&mut tail, b"\x1b"), 0);
    assert_eq!(process::cursor_queries(&mut tail, b"["), 0);
    assert_eq!(process::cursor_queries(&mut tail, b"6"), 0);
    assert_eq!(process::cursor_queries(&mut tail, b"n"), 1);
    assert_eq!(process::cursor_queries(&mut tail, b"\x1b[6m\x1b[16n"), 0);
    assert!(tail.len() <= 3);
}
