//! Golden tests of how Claude Code's, Codex's and Antigravity's sign-in is read on this machine: the status
//! command each runs, the exact environment and folder it runs with, and what each answer reads as. A stand-in agent
//! records what it was given; Linux only, because it reads its own starting environment from `/proc`.
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nexus_client::agent_entries::Agent;
use nexus_client::local_agents::{LocalAgent, Login, local_with, login_with};

/// Answers `--version` with `$REPORT/version`, else 2.1.303; any other run records its arguments, environment and
/// folder in `$REPORT`, then prints `$REPORT/stdout` and `$REPORT/stderr` and exits with `$REPORT/status`.
const STAND_IN: &str = r#"#!/bin/sh
[ "$1" = --version ] && { cat "$REPORT/version" 2>/dev/null || echo "2.1.303 (Claude Code)"; } && exit 0
printf '%s\0' "$@" > "$REPORT/args"
cat /proc/$$/environ > "$REPORT/env"
pwd -P > "$REPORT/cwd"
cat "$REPORT/stdout"
cat "$REPORT/stderr" >&2
exit "$(cat "$REPORT/status")"
"#;

/// Written once, before any agent starts: a fork while a test writes an executable makes exec fail (ETXTBSY).
fn stand_in() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("nexus-sign-in-golden-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let bin = dir.join("agent");
        std::fs::write(&bin, STAND_IN).expect("agent");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        bin
    })
}

/// What the stand-in saw: its arguments, its environment and its folder.
#[derive(Debug, PartialEq)]
struct Seen {
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: PathBuf,
}

type Read = Option<(Login, Option<String>, Option<String>)>;

/// One sign-in check: what it read, what the stand-in saw, and the folder it ran in, as given and as `pwd -P` names it.
struct Checked {
    read: Read,
    seen: Seen,
    dir: PathBuf,
    canonical: PathBuf,
}

/// The sign-in `agent`'s check reads when the stand-in prints `stdout` and `stderr` and exits with `status`. The run's
/// environment is the person's: every variable a status check removes, and some it keeps.
fn check(agent: Agent, stdout: &str, stderr: &str, status: i32) -> Checked {
    let dir = std::env::temp_dir().join(format!("nexus-sign-in-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("stdout"), stdout).expect("stdout");
    std::fs::write(dir.join("stderr"), stderr).expect("stderr");
    std::fs::write(dir.join("status"), status.to_string()).expect("status");
    let read = login_with(agent, stand_in(), &person(&dir), &dir);
    let checked = Checked {
        read,
        seen: seen(&dir),
        canonical: std::fs::canonicalize(&dir).expect("canonical"),
        dir,
    };
    let _ = std::fs::remove_dir_all(&checked.dir);
    checked
}

fn person(dir: &Path) -> HashMap<String, String> {
    let dir = dir.display().to_string();
    [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", dir.as_str()),
        ("REPORT", dir.as_str()),
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
        ("NEXUS_URL", "https://nexus.example"),
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

fn seen(dir: &Path) -> Seen {
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let fields = |bytes: Vec<u8>| -> Vec<String> {
        bytes
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(|field| String::from_utf8_lossy(field).into_owned())
            .collect()
    };
    Seen {
        args: fields(read("args")),
        env: fields(read("env"))
            .into_iter()
            .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect(),
        cwd: PathBuf::from(String::from_utf8_lossy(&read("cwd")).trim_end()),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

/// The person's environment without the variables named, as the stand-in in `dir` sees it.
fn person_without(dir: &Path, removed: &[&str]) -> BTreeMap<String, String> {
    person(dir)
        .into_iter()
        .filter(|(name, _)| !removed.contains(&name.as_str()))
        .collect()
}

#[test]
fn claude_codes_check_runs_auth_status_without_what_bills_elsewhere() {
    let checked = check(
        Agent::Claude,
        r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"ada@example.com"}"#,
        "",
        0,
    );
    assert_eq!(
        checked.read,
        Some((Login::Plan, Some("max".into()), Some("ada@example.com".into())))
    );
    assert_eq!(
        checked.seen,
        Seen {
            args: strings(&["auth", "status", "--json"]),
            // The plan's own token, the profile variables and the wrapper's Gateway stay.
            env: person_without(
                &checked.dir,
                &[
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "ANTHROPIC_BASE_URL",
                    "ANTHROPIC_CUSTOM_HEADERS",
                    "CLAUDE_CODE_USE_BEDROCK",
                    "CLAUDE_CODE_USE_VERTEX",
                    "CLAUDE_CODE_USE_FOUNDRY",
                ]
            ),
            cwd: checked.canonical,
        }
    );
}

#[test]
fn codexs_check_runs_login_status_without_its_api_key() {
    let checked = check(Agent::Codex, "Logged in using ChatGPT\n", "", 0);
    assert_eq!(checked.read, Some((Login::Plan, None, None)));
    assert_eq!(
        checked.seen,
        Seen {
            args: strings(&["login", "status"]),
            env: person_without(&checked.dir, &["OPENAI_API_KEY"]),
            cwd: checked.canonical,
        }
    );
}

/// Each answer of Claude Code's `auth status --json` (stdout, stderr, exit status) and what it reads as: the sign-in,
/// the plan, and the account only on the plan.
#[test]
fn claude_codes_answers() {
    let plan = |plan: Option<&str>, account: Option<&str>| {
        Some((Login::Plan, plan.map(String::from), account.map(String::from)))
    };
    let cases: [(&str, &str, i32, Read); 11] = [
        (
            r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"team","email":"ada@example.com","orgId":"o"}"#,
            "",
            0,
            plan(Some("team"), Some("ada@example.com")),
        ),
        (
            r#"{"loggedIn":true,"authMethod":"oauth_token"}"#,
            "",
            0,
            plan(None, None),
        ),
        (
            r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"","email":""}"#,
            "",
            0,
            plan(None, None),
        ),
        (
            "\n  {\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"pro\"}  \n",
            "",
            0,
            plan(Some("pro"), None),
        ),
        (
            r#"{"loggedIn":true,"authMethod":"console","email":"ada@example.com"}"#,
            "",
            0,
            Some((Login::ApiKey, None, None)),
        ),
        (r#"{"loggedIn":true}"#, "", 0, Some((Login::ApiKey, None, None))),
        // The exit status and stderr are not read.
        (
            r#"{"loggedIn":false,"authMethod":"none"}"#,
            "(node) a warning",
            1,
            Some((Login::SignedOut, None, None)),
        ),
        (r#"{"loggedIn":"yes"}"#, "", 0, Some((Login::Unknown, None, None))),
        ("null", "", 0, Some((Login::Unknown, None, None))),
        (
            "error: unknown command 'auth'",
            "",
            1,
            Some((Login::Unknown, None, None)),
        ),
        ("", "", 0, Some((Login::Unknown, None, None))),
    ];
    for (stdout, stderr, status, expected) in cases {
        assert_eq!(
            check(Agent::Claude, stdout, stderr, status).read,
            expected,
            "{stdout:?}"
        );
    }
}

/// Each answer of Codex's `login status`, on stdout or stderr, and what it reads as. An API key is named before
/// "not logged in" is.
#[test]
fn codexs_answers() {
    let cases: [(&str, &str, i32, Login); 8] = [
        ("", "Logged in using ChatGPT", 0, Login::Plan),
        ("LOGGED IN USING CHATGPT", "", 0, Login::Plan),
        ("Logged in using an API key - sk-proj-***abcd", "", 0, Login::ApiKey),
        ("Not logged in. Set an API key", "", 1, Login::ApiKey),
        ("", "Not logged in", 1, Login::SignedOut),
        ("", "error: unrecognized subcommand 'status'", 2, Login::Unknown),
        ("something new", "", 0, Login::Unknown),
        ("", "", 0, Login::Unknown),
    ];
    for (stdout, stderr, status, login) in cases {
        assert_eq!(
            check(Agent::Codex, stdout, stderr, status).read,
            Some((login, None, None)),
            "{stdout:?} {stderr:?}"
        );
    }
}

/// What would make `agy` bill an API key or a cloud project, and the display a sign-in would open a browser on.
const AGY_REMOVED: [&str; 11] = [
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
];

/// Antigravity's check runs `agy -p /usage --output-format json` without what bills elsewhere, and with no browser to
/// open: Nexus's no-op launchers under `$HOME/.cache` first on `PATH`, `BROWSER` set to one, and no display or desktop
/// session. The D-Bus session and the other agents' variables stay.
#[test]
fn antigravitys_check_runs_usage_without_a_browser() {
    let checked = check(Agent::Antigravity, include_str!("fixtures/agy-1.5.8-usage.json"), "", 0);
    assert_eq!(checked.read, Some((Login::Plan, None, None)));
    let no_browser = checked.dir.join(".cache/nexus/no-browser");
    let mut env = person_without(&checked.dir, &AGY_REMOVED);
    env.insert("PATH".into(), format!("{}:/usr/bin:/bin", no_browser.display()));
    env.insert("BROWSER".into(), no_browser.join("xdg-open").display().to_string());
    assert_eq!(
        checked.seen,
        Seen {
            args: strings(&["-p", "/usage", "--output-format", "json"]),
            env,
            cwd: checked.canonical,
        }
    );
}

/// Each answer of Antigravity's `/usage` (stdout, stderr, exit status) and what it reads as. Its sign-in prompt, on
/// either stream, is signed out; only `/usage`'s own answer with quota is the plan. It never names a plan or account.
#[test]
fn antigravitys_answers() {
    let cases: [(&str, &str, i32, Login); 8] = [
        (include_str!("fixtures/agy-1.5.8-usage.json"), "", 0, Login::Plan),
        // A Google Cloud sign-in answers placeholder quota, and reads as the plan too.
        (
            include_str!("fixtures/agy-1.5.11-usage-google-cloud.json"),
            "",
            0,
            Login::Plan,
        ),
        (
            include_str!("fixtures/agy-1.5.8-usage-signed-out.stdout"),
            include_str!("fixtures/agy-1.5.8-usage-signed-out.stderr"),
            1,
            Login::SignedOut,
        ),
        ("Authentication required", "", 1, Login::SignedOut),
        // Its error answer without the prompt is not known.
        (
            include_str!("fixtures/agy-1.5.8-usage-signed-out.stdout"),
            "",
            1,
            Login::Unknown,
        ),
        (
            r#"{"status":"SUCCESS","num_turns":1,"command":{"name":"usage","data":{"groups":[]}}}"#,
            "",
            0,
            Login::Unknown,
        ),
        ("not json", "", 0, Login::Unknown),
        ("", "", 0, Login::Unknown),
    ];
    for (stdout, stderr, status, login) in cases {
        assert_eq!(
            check(Agent::Antigravity, stdout, stderr, status).read,
            Some((login, None, None)),
            "{stdout:?} {stderr:?}"
        );
    }
}

/// Two Antigravity sign-ins are read without its check: a settings file that sets `modelProvider` is an API key, and
/// an agy older than 1.1.11 (whose `/usage` would be a prompt) is not checked. Neither runs `/usage`.
#[test]
fn antigravity_sign_ins_read_without_its_check() {
    let dir = std::env::temp_dir().join(format!("nexus-sign-in-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join(".gemini/antigravity-cli")).expect("dir");
    let settings = dir.join(".gemini/antigravity-cli/settings.json");
    std::fs::write(&settings, r#"{"modelProvider":"gemini"}"#).expect("settings");
    assert_eq!(
        login_with(Agent::Antigravity, stand_in(), &person(&dir), &dir),
        Some((Login::ApiKey, None, None))
    );
    // A null `modelProvider` is not set.
    std::fs::write(&settings, r#"{"modelProvider":null}"#).expect("settings");
    std::fs::write(dir.join("version"), "1.1.10\n").expect("version");
    assert_eq!(login_with(Agent::Antigravity, stand_in(), &person(&dir), &dir), None);
    std::fs::write(dir.join("version"), "1.1.11-beta\n").expect("version");
    assert_eq!(login_with(Agent::Antigravity, stand_in(), &person(&dir), &dir), None);
    assert!(!dir.join("args").exists(), "the check ran");
    let _ = std::fs::remove_dir_all(&dir);
}

/// What this machine reports of an agent: its version and sign-in, never the account.
#[test]
fn what_this_machine_reports() {
    let dir = std::env::temp_dir().join(format!("nexus-sign-in-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join("stdout"),
        r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"ada@example.com"}"#,
    )
    .expect("stdout");
    std::fs::write(dir.join("stderr"), "").expect("stderr");
    std::fs::write(dir.join("status"), "0").expect("status");
    let path = stand_in().display().to_string();
    let local = local_with(Agent::Claude, Some(stand_in()), &person(&dir), &dir);
    assert_eq!(
        local,
        LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: Some(path.clone()),
            version: Some("2.1.303".into()),
            login: Some(Login::Plan),
            plan: Some("max".into()),
        }
    );
    assert_eq!(
        serde_json::to_value(&local).expect("json"),
        serde_json::json!({ "agent": "claude", "installed": true, "path": path, "version": "2.1.303", "login": "plan", "plan": "max" })
    );
    std::fs::write(dir.join("stdout"), "Logged in using an API key").expect("stdout");
    assert_eq!(
        serde_json::to_value(local_with(Agent::Codex, Some(stand_in()), &person(&dir), &dir)).expect("json"),
        serde_json::json!({ "agent": "codex", "installed": true, "path": path, "version": "2.1.303", "login": "api_key" })
    );
    std::fs::write(dir.join("stdout"), include_str!("fixtures/agy-1.5.8-usage.json")).expect("stdout");
    assert_eq!(
        serde_json::to_value(local_with(Agent::Antigravity, Some(stand_in()), &person(&dir), &dir)).expect("json"),
        serde_json::json!({ "agent": "antigravity", "installed": true, "path": path, "version": "2.1.303", "login": "plan" })
    );
    assert_eq!(
        serde_json::to_value(local_with(Agent::Codex, None, &person(&dir), &dir)).expect("json"),
        serde_json::json!({ "agent": "codex", "installed": false })
    );
    let _ = std::fs::remove_dir_all(&dir);
}
