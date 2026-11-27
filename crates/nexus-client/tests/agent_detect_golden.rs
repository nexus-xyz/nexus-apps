//! Golden tests of the sign-in check `detect_in` runs, the path `detect_all` takes for each agent. Unlike
//! `login_with`'s check, its commands keep this process's environment and folder: they set `PATH` and `HOME` and drop
//! what bills elsewhere. So the test runs again in a process of its own, with an environment and a folder it sets. A
//! stand-in agent records what it was given; Linux only, because it reads its own starting environment from `/proc`.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nexus_client::agent_entries::Agent;
use nexus_client::local_agents::{LocalAgent, Login, detect_in};

/// Names the folder of the test's own process.
const OWN_PROCESS: &str = "NEXUS_DETECT_GOLDEN_FOLDER";

/// Answers `--version`; any other run records its arguments, environment and folder in `$REPORT`, then prints
/// `$REPORT/stdout`. Its `PATH` holds only the agent's folder (and agy's no-op launchers), so `cat` is named whole.
const STAND_IN: &str = r#"#!/bin/sh
[ "$1" = --version ] && echo "2.1.303 (stand-in)" && exit 0
printf '%s\0' "$@" > "$REPORT/args"
/bin/cat /proc/$$/environ > "$REPORT/env"
pwd -P > "$REPORT/cwd"
/bin/cat "$REPORT/stdout"
exit 0
"#;

/// The environment of the test's own process: what bills elsewhere for each agent, the display a browser would open
/// on, and what stays. Its `HOME` is not the person's: `detect_in` passes the one its lookup gives.
fn process_env(dir: &Path) -> BTreeMap<String, String> {
    let dir_text = dir.display().to_string();
    let report = dir.join("report").display().to_string();
    [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/process"),
        ("REPORT", report.as_str()),
        (OWN_PROCESS, dir_text.as_str()),
        ("NEXUS_URL", "https://nexus.example"),
        ("ANTHROPIC_API_KEY", "sk-ant-own"),
        ("ANTHROPIC_AUTH_TOKEN", "own-token"),
        ("ANTHROPIC_BASE_URL", "https://proxy.example"),
        ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("CLAUDE_CODE_USE_VERTEX", "1"),
        ("CLAUDE_CODE_USE_FOUNDRY", "1"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
        ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
        ("CODEX_HOME", "/home/ada/.codex-work"),
        ("OPENAI_API_KEY", "sk-own"),
        ("GEMINI_API_KEY", "gm-own"),
        ("GOOGLE_API_KEY", "go-own"),
        ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
        ("GOOGLE_CLOUD_PROJECT", "ada-project"),
        ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
        ("AGY_ADC_AUTH", "1"),
        ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("XDG_CURRENT_DESKTOP", "GNOME"),
        ("DESKTOP_SESSION", "gnome"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

/// The folder to work in when this is the test's own process; otherwise runs `test` again in a process of its own,
/// with exactly [`process_env`] and in the folder's `work`, checks it passed, and gives `None`.
fn own_process(test: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(OWN_PROCESS) {
        return Some(PathBuf::from(dir));
    }
    let dir = std::env::temp_dir().join(format!("nexus-detect-golden-{}", uuid::Uuid::new_v4()));
    for folder in ["work", "report", "home", "bin"] {
        std::fs::create_dir_all(dir.join(folder)).expect("dir");
    }
    let out = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(process_env(&dir))
        .current_dir(dir.join("work"))
        .output()
        .expect("run the test again");
    let done = dir.join("done").exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success() && done,
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    None
}

/// What the stand-in saw: its arguments, environment and folder.
fn seen(dir: &Path) -> (Vec<String>, BTreeMap<String, String>, PathBuf) {
    let read = |name: &str| std::fs::read(dir.join("report").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let fields = |bytes: Vec<u8>| -> Vec<String> {
        bytes
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect()
    };
    (
        fields(read("args")),
        fields(read("env"))
            .into_iter()
            .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect(),
        PathBuf::from(String::from_utf8_lossy(&read("cwd")).trim_end()),
    )
}

/// [`process_env`] without the variables named, with `PATH` and `HOME` as given.
fn expected_env(dir: &Path, removed: &[&str], path: &str) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = process_env(dir)
        .into_iter()
        .filter(|(name, _)| !removed.contains(&name.as_str()))
        .collect();
    env.insert("PATH".into(), path.into());
    env.insert("HOME".into(), dir.join("home").display().to_string());
    env
}

#[test]
fn detect_in_checks_each_agent_in_this_process_environment() {
    let Some(dir) = own_process("agent_detect_golden::detect_in_checks_each_agent_in_this_process_environment") else {
        return;
    };
    // Written in this process only, before any agent starts, so no fork can catch a file open for writing.
    let bin_dir = dir.join("bin");
    for name in ["claude", "codex", "agy"] {
        let bin = bin_dir.join(name);
        std::fs::write(&bin, STAND_IN).expect("agent");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    // The person's environment as the caller looks it up: only its `HOME`.
    let lookup: HashMap<String, String> = HashMap::from([("HOME".to_string(), dir.join("home").display().to_string())]);
    let env = |name: &str| lookup.get(name).cloned();
    let dirs = [bin_dir.clone()];
    let bins = bin_dir.display().to_string();
    // The agent's own folder first, then the folders searched.
    let path = format!("{bins}:{bins}");
    let cwd = std::fs::canonicalize(dir.join("work")).expect("canonical");
    let stdout = dir.join("report/stdout");
    let local = |agent: &str, bin: &str, login: Login, plan: Option<&str>| LocalAgent {
        agent: agent.into(),
        installed: true,
        path: Some(bin_dir.join(bin).display().to_string()),
        version: Some("2.1.303".into()),
        login: Some(login),
        plan: plan.map(String::from),
    };

    std::fs::write(
        &stdout,
        r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"ada@example.com"}"#,
    )
    .expect("stdout");
    assert_eq!(
        detect_in(Agent::Claude, &dirs, &env),
        local("claude", "claude", Login::Plan, Some("max"))
    );
    assert_eq!(
        seen(&dir),
        (
            vec!["auth".to_string(), "status".into(), "--json".into()],
            expected_env(
                &dir,
                &[
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_BASE_URL",
                    "ANTHROPIC_CUSTOM_HEADERS",
                    "CLAUDE_CODE_USE_BEDROCK",
                    "CLAUDE_CODE_USE_VERTEX",
                    "CLAUDE_CODE_USE_FOUNDRY",
                ],
                &path
            ),
            cwd.clone()
        )
    );

    std::fs::write(&stdout, "Logged in using ChatGPT\n").expect("stdout");
    assert_eq!(
        detect_in(Agent::Codex, &dirs, &env),
        local("codex", "codex", Login::Plan, None)
    );
    assert_eq!(
        seen(&dir),
        (
            vec!["login".to_string(), "status".into()],
            expected_env(&dir, &["OPENAI_API_KEY"], &path),
            cwd.clone()
        )
    );

    // agy's check gets the no-op launchers first on `PATH`, `BROWSER` set to one, and no display or desktop session.
    std::fs::write(&stdout, include_str!("fixtures/agy-1.5.8-usage.json")).expect("stdout");
    assert_eq!(
        detect_in(Agent::Antigravity, &dirs, &env),
        local("antigravity", "agy", Login::Plan, None)
    );
    let no_browser = dir.join("home/.cache/nexus/no-browser");
    let mut agy_env = expected_env(
        &dir,
        &[
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_PROJECT",
            "CLOUDSDK_CORE_PROJECT",
            "AGY_ADC_AUTH",
            "GOOGLE_GEMINI_BASE_URL",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "DESKTOP_SESSION",
        ],
        &format!("{}:{path}", no_browser.display()),
    );
    agy_env.insert("BROWSER".into(), no_browser.join("xdg-open").display().to_string());
    assert_eq!(
        seen(&dir),
        (
            vec![
                "-p".to_string(),
                "/usage".into(),
                "--output-format".into(),
                "json".into()
            ],
            agy_env,
            cwd
        )
    );

    // Not found in the folders: not installed, and nothing runs.
    assert_eq!(
        detect_in(Agent::Codex, &[], &env),
        LocalAgent {
            agent: "codex".into(),
            installed: false,
            path: None,
            version: None,
            login: None,
            plan: None,
        }
    );
    std::fs::write(dir.join("done"), "").expect("done");
}
