use super::*;

#[test]
fn sessions_wait_resolves_a_title_and_reads_one_turn() {
    let dir = temp("wait-title");
    let (socket, seen) = fake_app(&dir, false, None);
    seen.lock().expect("fake app state").sessions[0]["turns"] = json!([{"status": "completed"}]);

    let result = run(&dir, &socket, &["sessions", "wait", "Fix the tests"], "");
    assert_eq!(result.code, 0, "{}", result.err);
    assert_eq!(result.out, "session t_1 finished");
    assert_eq!(result.err, APPROVE);
    let seen = seen.lock().expect("fake app state");
    let call = seen.calls.last().expect("show call");
    assert_eq!(call["name"], "show_session");
    assert_eq!(call["args"], json!({"session_id": "t_1", "turns": 1}));
}

#[test]
fn sessions_wait_reports_an_unknown_session() {
    let dir = temp("wait-unknown");
    let (socket, seen) = fake_app(&dir, false, None);

    let result = run(&dir, &socket, &["sessions", "wait", "t_unknown"], "");
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert_eq!(
        result.err,
        format!("{APPROVE}\nsession t_unknown could not be read: No session t_unknown.")
    );
    let seen = seen.lock().expect("fake app state");
    assert_eq!(seen.calls.len(), 1);
    assert!(seen.calls.iter().all(|call| call["name"] == "show_session"));
}

const APPROVE: &str = "Approve “nexus CLI” in Nexus for desktop: it asks you now.";

#[test]
fn sessions_wait_reports_a_missing_listener_with_its_hint() {
    let dir = temp("wait-missing");
    let result = run(&dir, &dir.join("missing.sock"), &["sessions", "wait", "t_1"], "");
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert_eq!(
        result.err,
        "session t_1 could not be read: Nexus for desktop is not running, or its outside access is off.\n  → Open Nexus and turn on Settings › MCP servers › Outside access."
    );
}

#[test]
fn sessions_wait_reports_an_ambiguous_title_once() {
    let dir = temp("wait-ambiguous");
    let (socket, seen) = fake_app(&dir, false, None);
    titled(&seen, &[("t_1", "Shared title"), ("t_2", "Shared title")]);
    let result = run(&dir, &socket, &["sessions", "wait", "Shared title"], "");
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert_eq!(
        result.err,
        format!(
            "{APPROVE}\nsession Shared title could not be read: 2 sessions are titled “Shared title”: t_1, t_2. Name one by its id."
        )
    );
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.calls.len(), 2);
    assert_eq!(seen.calls[0]["name"], "list_sessions");
    assert_eq!(seen.calls[1]["name"], "show_session");
}

#[test]
fn sessions_wait_requests_access_once_after_a_saved_token_is_refused() {
    let dir = temp("wait-denied");
    let (socket, seen) = fake_app(&dir, true, None);
    std::fs::write(dir.join("desktop-access.json"), r#"{"tokens":{"nexus CLI":"old"}}"#).expect("save old token");
    let result = run(&dir, &socket, &["sessions", "wait", "Named session"], "");
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert_eq!(
        result.err,
        format!(
            "{APPROVE}\nsession Named session could not be read: Nexus for desktop did not grant access: The person denied access."
        )
    );
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.asked.len(), 1);
    assert!(seen.calls.is_empty());
}

#[test]
fn sessions_wait_does_not_request_access_again_after_a_new_token_is_revoked() {
    let dir = temp("wait-new-token-revoked");
    let (socket, seen) = fake_app(&dir, false, None);
    {
        let mut seen = seen.lock().expect("seen");
        seen.revoke_after_show = true;
        seen.sessions[0]["status"] = json!("running");
    }
    let result = run(&dir, &socket, &["sessions", "wait", "t_1"], "");
    assert_eq!(result.code, 2);
    assert!(result.out.is_empty());
    assert_eq!(
        result.err,
        format!("{APPROVE}\nsession t_1 could not be read: no access")
    );
    let seen = seen.lock().expect("seen");
    assert_eq!(seen.asked.len(), 1);
    assert_eq!(seen.calls.len(), 1);
}

#[test]
fn sessions_wait_help_explains_the_time_for_the_final_read() {
    let dir = temp("wait-help");
    let socket = dir.join("missing.sock");
    for args in [["sessions", "help"].as_slice(), &["sessions", "wait", "--help"]] {
        let result = run(&dir, &socket, args, "");
        assert_eq!(result.code, 0, "{}", result.err);
        assert!(result.err.is_empty());
        assert!(
            result
                .out
                .contains("It can run up to 3 seconds past --timeout for one last check.")
        );
    }
}
