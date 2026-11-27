//! Golden tests of the plan-tier reads (`usage_signals::refresh_plan`) for each agent: what each read runs,
//! with which environment and in which folder, and the tier each answer saves. Antigravity's tier is never read. A
//! stand-in agent records what it was given; Linux only, because it reads its own starting environment from `/proc`.
//!
//! A read runs only once `usage_signals::enable` lets the process send, and that switch is process-wide: the other tests
//! of this binary must see no extra calls. So each test runs itself again in a process of its own, and does the work
//! only there.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nexus_cli::usage_signals::{PLANS_FILE, enable, refresh_plan};
use nexus_client::agent_entries::Agent;
use serde_json::{Value, json};

/// Names the folder of a test that runs in its own process.
const OWN_PROCESS: &str = "NEXUS_PLAN_TIER_FOLDER";

/// The folder to work in when this is the test's own process; otherwise runs `test` again in a process of its own,
/// checks it passed, and gives `None`.
fn own_process(test: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(OWN_PROCESS) {
        return Some(PathBuf::from(dir));
    }
    let dir = std::env::temp_dir().join(format!("nexus-plan-tier-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let out = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(OWN_PROCESS, &dir)
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

/// Writes the stand-in agent: it records its arguments, environment and folder in `$REPORT`, reads `lines` lines of
/// stdin into `$REPORT/stdin`, then prints `$REPORT/stdout`. Only this process runs, so no fork can catch the file
/// open for writing.
fn stand_in(dir: &Path, lines: usize) -> PathBuf {
    let bin = dir.join("agent");
    std::fs::write(
        &bin,
        format!(
            r#"#!/bin/sh
printf '%s\0' "$@" > "$REPORT/args"
cat /proc/$$/environ > "$REPORT/env"
pwd -P > "$REPORT/cwd"
: > "$REPORT/stdin"
n=0
while [ "$n" -lt {lines} ] && IFS= read -r line; do
  printf '%s\n' "$line" >> "$REPORT/stdin"
  n=$((n + 1))
done
cat "$REPORT/stdout"
"#
        ),
    )
    .expect("agent");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::create_dir_all(dir.join("report")).expect("report");
    bin
}

/// The person's environment: what bills elsewhere for either agent, and what stays.
fn person(dir: &Path) -> HashMap<String, String> {
    [
        ("PATH", "/usr/bin:/bin".to_string()),
        ("HOME", dir.display().to_string()),
        ("REPORT", dir.join("report").display().to_string()),
        ("ANTHROPIC_API_KEY", "sk-ant-own".to_string()),
        ("ANTHROPIC_BASE_URL", "https://proxy.example".to_string()),
        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own".to_string()),
        ("OPENAI_API_KEY", "sk-own".to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// What the stand-in saw: its arguments, environment, folder and stdin.
fn seen(dir: &Path) -> (Vec<String>, BTreeMap<String, String>, PathBuf, String) {
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
        String::from_utf8_lossy(&read("stdin")).into_owned(),
    )
}

/// Starts a read for `agent` whose answer is `stdout`, with a tier a day old saved, and waits until the saved entry
/// for `agent` has the `plan` given: a tier, or none once the read forgot it.
fn read_tier(dir: &Path, agent: Agent, bin: &Path, stdout: &str, plan: Option<&str>) -> Value {
    let config = dir.join("config.json");
    std::fs::write(dir.join("report/stdout"), stdout).expect("stdout");
    std::fs::write(
        dir.join(PLANS_FILE),
        json!({ agent.as_str(): { "plan": "free", "at": 1 } }).to_string(),
    )
    .expect("plans");
    refresh_plan(&config, agent, bin, &person(dir));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let saved: Value = std::fs::read_to_string(dir.join(PLANS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let entry = saved[agent.as_str()].clone();
        let stamped = entry["at"].as_u64().is_some_and(|at| at > 1);
        if stamped && entry.get("plan").and_then(Value::as_str) == plan {
            return saved;
        }
        assert!(Instant::now() < deadline, "{agent}: {saved}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The saved file's entries with their times left out, so the rest is compared whole.
fn without_times(mut saved: Value) -> Value {
    if let Some(entries) = saved.as_object_mut() {
        for entry in entries.values_mut() {
            if let Some(entry) = entry.as_object_mut() {
                entry.remove("at");
            }
        }
    }
    saved
}

#[test]
fn claude_codes_tier_comes_from_auth_status() {
    let Some(dir) = own_process("plan_tier::claude_codes_tier_comes_from_auth_status") else {
        return;
    };
    enable();
    let bin = stand_in(&dir, 0);
    let saved = read_tier(
        &dir,
        Agent::Claude,
        &bin,
        r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"Max","email":"ada@example.com"}"#,
        Some("max"),
    );
    assert_eq!(without_times(saved), json!({ "claude": { "plan": "max" } }));
    let (args, env, cwd, stdin) = seen(&dir);
    assert_eq!(args, ["auth", "status", "--json"]);
    // The plan's own token stays; what bills elsewhere goes.
    let mut expected: BTreeMap<String, String> = person(&dir).into_iter().collect();
    expected.remove("ANTHROPIC_API_KEY");
    expected.remove("ANTHROPIC_BASE_URL");
    assert_eq!(env, expected);
    // In the config's own folder, never the project's.
    assert_eq!(cwd, std::fs::canonicalize(&dir).expect("canonical"));
    assert_eq!(stdin, "");

    // A plan that names no tier saves `unknown`; an API key forgets the tier.
    let saved = read_tier(
        &dir,
        Agent::Claude,
        &bin,
        r#"{"loggedIn":true,"authMethod":"oauth_token"}"#,
        Some("unknown"),
    );
    assert_eq!(without_times(saved), json!({ "claude": { "plan": "unknown" } }));
    let saved = read_tier(
        &dir,
        Agent::Claude,
        &bin,
        r#"{"loggedIn":true,"authMethod":"console"}"#,
        None,
    );
    assert_eq!(without_times(saved), json!({ "claude": {} }));
    std::fs::write(dir.join("done"), "").expect("done");
}

#[test]
fn codexs_tier_comes_from_its_app_server() {
    let Some(dir) = own_process("plan_tier::codexs_tier_comes_from_its_app_server") else {
        return;
    };
    enable();
    let bin = stand_in(&dir, 4);
    let answer = |account: Value| {
        format!(
            "{}\n{}\n",
            json!({ "id": 2, "result": { "account": account } }),
            json!({ "id": 3, "result": { "rateLimits": {} } })
        )
    };
    let saved = read_tier(
        &dir,
        Agent::Codex,
        &bin,
        &answer(json!({ "type": "chatgpt", "email": "ada@example.com", "planType": "business" })),
        Some("team"),
    );
    assert_eq!(without_times(saved), json!({ "codex": { "plan": "team" } }));
    let (args, env, cwd, stdin) = seen(&dir);
    assert_eq!(args, ["app-server"]);
    let mut expected: BTreeMap<String, String> = person(&dir).into_iter().collect();
    expected.remove("OPENAI_API_KEY");
    assert_eq!(env, expected);
    // In the temporary folder (`nexus_workbench::plans::codex`), never the project's.
    assert_eq!(cwd, std::fs::canonicalize(std::env::temp_dir()).expect("canonical"));
    assert_eq!(
        stdin,
        [
            r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"nexus-desktop","version":"0.1.0"}}}"#,
            r#"{"method":"initialized"}"#,
            r#"{"id":2,"method":"account/read","params":{}}"#,
            r#"{"id":3,"method":"account/rateLimits/read"}"#,
            "",
        ]
        .join("\n")
    );

    // A ChatGPT sign-in without a tier saves `unknown`; an API key forgets the tier.
    let saved = read_tier(
        &dir,
        Agent::Codex,
        &bin,
        &answer(json!({ "type": "chatgpt" })),
        Some("unknown"),
    );
    assert_eq!(without_times(saved), json!({ "codex": { "plan": "unknown" } }));
    let saved = read_tier(&dir, Agent::Codex, &bin, &answer(json!({ "type": "apiKey" })), None);
    assert_eq!(without_times(saved), json!({ "codex": {} }));
    // An error from `account/read` forgets the tier too.
    let saved = read_tier(
        &dir,
        Agent::Codex,
        &bin,
        "{\"id\":2,\"error\":{\"message\":\"boom\"}}\n{\"id\":3,\"result\":{}}\n",
        None,
    );
    assert_eq!(without_times(saved), json!({ "codex": {} }));
    std::fs::write(dir.join("done"), "").expect("done");
}

/// Antigravity's tier is not read: `agy` never runs for it, and a saved entry a day old stays as it is.
#[test]
fn antigravitys_tier_is_never_read() {
    let Some(dir) = own_process("plan_tier::antigravitys_tier_is_never_read") else {
        return;
    };
    enable();
    let bin = stand_in(&dir, 0);
    let saved = json!({ "antigravity": { "plan": "free", "at": 1 } }).to_string();
    std::fs::write(dir.join(PLANS_FILE), &saved).expect("plans");
    refresh_plan(&dir.join("config.json"), Agent::Antigravity, &bin, &person(&dir));
    // A read stamps the saved entry before it starts, on this thread.
    assert_eq!(std::fs::read_to_string(dir.join(PLANS_FILE)).expect("plans"), saved);
    assert!(!dir.join("report/args").exists(), "agy ran");
    std::fs::write(dir.join("done"), "").expect("done");
}
