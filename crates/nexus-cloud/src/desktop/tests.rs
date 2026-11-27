use super::*;

#[test]
fn times_parse_as_unix_ms() {
    assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_time("2026-11-01T12:00:00Z"), Some(1_793_534_400_000));
    assert_eq!(parse_time("2026-11-01T13:00:00+01:00"), Some(1_793_534_400_000));
    assert_eq!(parse_time("2026-11-01T12:00:00.250Z"), Some(1_793_534_400_250));
    assert_eq!(parse_time("1793534400250"), Some(1_793_534_400_250));
    assert_eq!(parse_time(""), None);
    assert_eq!(parse_time("yesterday"), None);
}

#[test]
fn ports_skip_ssh_boat_and_the_host() {
    let ss = "LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n\
              LISTEN 0 511 127.0.0.1:3000 0.0.0.0:* users:((\"node\",pid=42,fd=20))\n\
              LISTEN 0 511 [::1]:3000 [::]:*\n\
              LISTEN 0 50 *:6080 *:*\n\
              LISTEN 0 50 *:47990 *:*\n\
              LISTEN 0 5 127.0.0.1:41000 0.0.0.0:* users:((\"nexus-host\",pid=7,fd=9))\n\
              LISTEN 0 5 [::]:8000 [::]:* users:((\"python3\",pid=8,fd=3))\n\
              garbage\n";
    assert_eq!(
        parse_ports(ss, Some(41000)),
        vec![
            CloudPort {
                port: 3000,
                process: Some("node".into())
            },
            CloudPort {
                port: 8000,
                process: Some("python3".into())
            },
        ]
    );
}

#[test]
fn repos_newest_push_first() {
    let tsv = "acme/old\thttps://github.com/acme/old\tfalse\t2025-01-01T00:00:00Z\n\
               me/new\thttps://github.com/me/new\ttrue\t2026-10-31T00:00:00Z\n\
               me/never\thttps://github.com/me/never\tfalse\t\n\
               me/new\thttps://github.com/me/new\ttrue\t2026-10-31T00:00:00Z\n";
    let repos = parse_repos(tsv);
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["me/new", "acme/old", "me/never"]);
    assert!(repos[0].private && !repos[1].private);
    assert_eq!(repos[2].pushed_at, None);
}

#[test]
fn reads_the_device_code() {
    assert_eq!(
        parse_device_code("! First copy your one-time code: 36CF-5606"),
        (Some("36CF-5606".into()), None)
    );
    assert_eq!(
        parse_device_code("Open this URL to continue in your web browser: https://github.com/login/device"),
        (None, Some("https://github.com/login/device".into()))
    );
    assert_eq!(
        parse_device_code("- Press Enter to open https://github.com/login/device in your browser..."),
        (None, Some("https://github.com/login/device".into()))
    );
}

/// The machine gets the Claude token kept here and nothing else: no ChatGPT sign-in ever goes.
#[test]
fn agent_env_holds_only_the_claude_plan() {
    assert_eq!(
        agent_env(Some(" sk-ant-oat01-a \n".into())).as_deref(),
        Some("CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-a\n")
    );
    assert_eq!(agent_env(None), None);
}

#[test]
fn the_browser_config_is_claudes_mcp_format() {
    let config: Value = serde_json::from_str(&playwright_config()).unwrap_or_default();
    let args = config["mcpServers"]["browser"]["args"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(config["mcpServers"]["browser"]["command"], "npx");
    assert!(args.contains(&json!(PLAYWRIGHT_MCP)));
    assert!(args.contains(&json!("chrome")) && args.contains(&json!("--headless")));
    // In the session's own folder, so the reply can show what the browser saved.
    let at = args
        .iter()
        .position(|a| a == "--output-dir")
        .unwrap_or_else(|| panic!("no --output-dir"));
    assert_eq!(args.get(at + 1), Some(&json!(".nexus/artifacts")));
}

#[test]
fn the_agent_is_told_where_it_runs_in_a_few_plain_lines() {
    let lines = AGENT_INSTRUCTIONS.lines().count();
    assert!(lines <= 15, "{lines} lines");
    for needed in [
        "cloud machine",
        "Playwright",
        "0.0.0.0",
        "port",
        "gh pr create",
        "secrets",
    ] {
        assert!(AGENT_INSTRUCTIONS.contains(needed), "no {needed:?}");
    }
    assert!(
        !AGENT_INSTRUCTIONS.contains("<!--"),
        "it would end the app's block early"
    );
}

#[test]
fn a_cloud_machine_serializes_as_the_contract_says() {
    let setting = CloudSetting {
        provider: "boat".into(),
        size: "large".into(),
        state: CloudState::Sleeping,
        sleeping_since: Some(5),
        session_id: Some("t_1".into()),
        title: Some("Fix the login".into()),
        stop_reason: Some("idle".into()),
        ..CloudSetting::default()
    };
    assert_eq!(
        serde_json::to_value(CloudInfo::from(&setting)).unwrap_or_default(),
        json!({ "provider": "boat", "size": "large", "state": "sleeping", "error": null, "sleepingSince": 5,
                "sessionId": "t_1", "title": "Fix the login", "queuedUntil": null, "stopReason": "idle" })
    );
    // The repository goes out only when this device knows it.
    let filed = CloudSetting {
        repo: Some("https://github.com/acme/nexus".into()),
        ..setting
    };
    assert_eq!(
        serde_json::to_value(CloudInfo::from(&filed)).unwrap_or_default()["repo"],
        json!("https://github.com/acme/nexus")
    );
    assert_eq!(
        serde_json::to_value(CloudState::Queued).unwrap_or_default(),
        json!("queued")
    );
    // A setting saved by an older app (with its stop key fields) still reads.
    let old: CloudSetting = serde_json::from_value(json!({ "provider": "boat", "size": "large", "key": "/k",
        "knownHosts": "/h", "state": "ready", "selfSleepMins": 30, "stopApi": "https://boat.dev/api/v1" }))
    .unwrap_or_default();
    assert_eq!(old.state, CloudState::Ready);
    assert_eq!(
        serde_json::to_value(CloudStep::Install).unwrap_or_default(),
        json!("install")
    );
    assert!(reserved_port(47984) && reserved_port(22) && !reserved_port(3000));
}
