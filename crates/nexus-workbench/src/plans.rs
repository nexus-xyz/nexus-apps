//! What is left of each plan's usage limits (Claude's five-hour session and weekly limits, Codex's), for Settings.
//! The agents report it themselves, so the app never reads their logins: Claude Code answers the `get_usage` control
//! request (the Agent SDK's `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET`, so it is read leniently) and
//! names the account in its `initialize` answer; Codex's app server answers `account/read` and
//! `account/rateLimits/read`. A probe starts the agent with no prompt, so no model call is made, and stops it after
//! the answers. Checked against `claude` 2.1.305 and `codex-cli` 0.163.0. Codex also reports the free limit resets
//! banked on the account, and uses one on `account/rateLimitResetCredit/consume` (checked against 0.167.0). Claude
//! Code has no such request.
//! Antigravity answers `agy -p "/usage" --output-format json` without a model turn ([`antigravity`]), checked against
//! agy 1.5.8.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::event::SessionAgent;
use crate::process::{self, Spawn};

/// How long a probe may take: the agent's start and one request to its vendor.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Claude Code with only the control protocol: no prompt, none of the person's settings (their hooks would run), no
/// transcript left behind.
const CLAUDE_PROBE: [&str; 9] = [
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--setting-sources",
    "",
    "--no-session-persistence",
];

/// One usage limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limit {
    /// The models the limit covers, as the agent names them ("Gemini 3.8 Flash"); empty for the whole plan. An
    /// Antigravity plan has one limit per model group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// `Session`, `Weekly`, `Weekly · Fable`, `Weekly · Gemini`.
    pub label: String,
    /// 0 to 100.
    pub used_percent: f64,
    /// Epoch milliseconds.
    pub resets_at: Option<i64>,
}

/// The free limit resets banked on a plan (Codex's `rateLimitResetCredits`). Using one clears the plan's limits now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resets {
    pub available: u32,
    /// When the first one expires, epoch milliseconds.
    pub expires_at: Option<i64>,
}

/// One plan's limits, as its agent reports them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanUsage {
    pub agent: SessionAgent,
    /// The agent's extra account ([`crate::accounts`]); None: the agent's own login.
    pub account: Option<String>,
    pub email: Option<String>,
    /// The plan as the agent names it: `Claude Max`, `Claude Team`, `plus`.
    pub plan: Option<String>,
    /// Signed in to a plan. False: signed out, or on an API key (which has no plan limits).
    pub signed_in: bool,
    /// The Claude account new sessions on the plan start on ([`crate::rotate`]); the host sets it.
    pub active: bool,
    pub limits: Vec<Limit>,
    /// The resets banked on the plan, when its agent reports them (Codex only).
    #[serde(default)]
    pub resets: Option<Resets>,
    /// Why the limits could not be read.
    pub error: Option<String>,
    /// Antigravity signed in through Google Cloud: its quota is the project's, which agy does not report
    /// ([`CLOUD_USAGE_NOTE`]). Known by agy's placeholder answer, not by its settings.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub google_cloud: bool,
    /// Signed in, but the agent said its limits are not available now (Claude Code's `rate_limits_available: false`,
    /// as it answers during a turn). [`crate::plan_book`] shows the login's last limits instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unavailable: bool,
    /// When the agent fetched these limits (epoch milliseconds), when that is known: Claude Code may answer from a
    /// cache that is hours old ([`claude_fetched_at`]). None: not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
}

impl PlanUsage {
    /// A login with no known plan or usage.
    pub fn signed_out(agent: SessionAgent, account: Option<String>) -> PlanUsage {
        PlanUsage {
            agent,
            account,
            email: None,
            plan: None,
            signed_in: false,
            active: false,
            limits: vec![],
            resets: None,
            error: None,
            google_cloud: false,
            unavailable: false,
            fetched_at: None,
        }
    }

    fn failed(agent: SessionAgent, account: Option<String>, error: String) -> PlanUsage {
        PlanUsage {
            error: Some(error),
            ..PlanUsage::signed_out(agent, account)
        }
    }
}

/// Claude Code's limits for the login in `config_dir` (None: its own login). `env` must hold nothing that bills
/// elsewhere (an API key, another base URL).
pub async fn claude(
    program: PathBuf,
    mut env: HashMap<String, String>,
    account: Option<String>,
    config_dir: Option<PathBuf>,
) -> PlanUsage {
    if let Some(dir) = &config_dir {
        env.insert("CLAUDE_CONFIG_DIR".into(), dir.to_string_lossy().into_owned());
    }
    let state = claude_state_file(&|name| env.get(name).cloned());
    let spawn = Spawn {
        program,
        args: CLAUDE_PROBE.iter().map(|arg| arg.to_string()).collect(),
        env,
        cwd: std::env::temp_dir(),
    };
    let lines = [
        crate::claude::initialize(),
        json!({"type": "control_request", "request_id": "usage", "request": {"subtype": "get_usage", "skip_behaviors": true}})
            .to_string(),
    ];
    let mut usage = match ask(&spawn, &lines, &["init", "usage"], claude_answer).await {
        Ok(answers) => match answers.get("init") {
            Some(init) => read_claude(account, init, answers.get("usage")),
            None => PlanUsage::failed(SessionAgent::Claude, account, "Claude Code did not answer.".into()),
        },
        Err(error) => PlanUsage::failed(SessionAgent::Claude, account, error),
    };
    if let Some(state) = state
        && let Ok(text) = tokio::fs::read_to_string(&state).await
        && let Ok(state) = serde_json::from_str::<Value>(&text)
    {
        usage.fetched_at = claude_fetched_at(&state, &usage);
    }
    // The next session on this login sends the plan's tier (`plan_tier`). A read that worked and found no plan forgets
    // the old tier; a failed read keeps it.
    if usage.plan.is_some() {
        crate::plan_tier::remember(SessionAgent::Claude, config_dir.as_deref(), usage.plan.as_deref());
    } else if usage.error.is_none() {
        crate::plan_tier::forget(SessionAgent::Claude, config_dir.as_deref());
    }
    usage
}

/// Where Claude Code keeps its state (`.claude.json`) for a login: in `CLAUDE_CONFIG_DIR`, else the home folder.
pub fn claude_state_file(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    ["CLAUDE_CONFIG_DIR", "HOME", "USERPROFILE"]
        .into_iter()
        .find_map(|name| env(name).filter(|dir| !dir.is_empty()))
        .map(|dir| PathBuf::from(dir).join(".claude.json"))
}

/// The account a Claude Code login last named, from its state (`oauthAccount.emailAddress`). Claude Code keeps it when
/// it blanks a sign-in it can no longer renew, so a login that reads signed out but still names an account was lost,
/// not never made. Claude Code's own state, not an interface: None when the field is missing or empty.
pub fn claude_last_email(state: &Value) -> Option<String> {
    state
        .pointer("/oauthAccount/emailAddress")
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .map(String::from)
}

/// When Claude Code fetched the limits it answered with. Its `get_usage` answer has no time, and it may come from the
/// `cachedUsageUtilization` in its `.claude.json` (`state`: in `CLAUDE_CONFIG_DIR`, else the home folder), which can be
/// hours old: another `claude` process writes the whole file back, an older copy of the cache included. The cache's
/// `fetchedAtMs` dates the answer when its five-hour and weekly numbers are the answer's. The file is Claude Code's own
/// state, not an interface: None when its fields are missing or differ.
pub fn claude_fetched_at(state: &Value, plan: &PlanUsage) -> Option<i64> {
    let cache = state.get("cachedUsageUtilization")?;
    let used = |label: &str| {
        plan.limits
            .iter()
            .find(|limit| limit.label == label)
            .map(|limit| limit.used_percent)
    };
    let cached = |window: &str| {
        cache
            .pointer(&format!("/utilization/{window}/utilization"))
            .and_then(Value::as_f64)
    };
    // One window: an old window's 4% is not a new window's 4%.
    let window = |label: &str, key: &str| {
        let answered = plan.limits.iter().find(|limit| limit.label == label)?.resets_at;
        let cached = cache
            .pointer(&format!("/utilization/{key}/resets_at"))
            .and_then(Value::as_str)
            .and_then(rfc3339_ms);
        Some(match (answered, cached) {
            (Some(answered), Some(cached)) => (answered - cached).abs() <= 60_000,
            (None, None) => true,
            _ => false,
        })
    };
    let session = used("Session")?;
    let same = |a: f64, b: f64| (a - b).abs() < 0.5;
    (cached("five_hour").is_some_and(|c| same(c, session))
        && window("Session", "five_hour") == Some(true)
        && cached("seven_day").zip(used("Weekly")).is_none_or(|(c, w)| same(c, w)))
    .then(|| cache.get("fetchedAtMs").and_then(Value::as_i64))
    .flatten()
}

/// Codex's app server for the login in `home` (None: its own login, `~/.codex`), and the lines that greet it and then
/// send `requests`.
fn codex_server(
    program: PathBuf,
    mut env: HashMap<String, String>,
    home: Option<&Path>,
    requests: &[Value],
) -> (Spawn, Vec<String>) {
    if let Some(home) = home {
        env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
    }
    let spawn = Spawn {
        program,
        args: vec!["app-server".into()],
        env,
        cwd: std::env::temp_dir(),
    };
    let hello = [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "nexus-desktop", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"method": "initialized"}),
    ];
    let lines = hello.iter().chain(requests).map(Value::to_string).collect();
    (spawn, lines)
}

/// Codex's limits for the login in `home` (None: its own login, `~/.codex`).
pub async fn codex(
    program: PathBuf,
    env: HashMap<String, String>,
    account: Option<String>,
    home: Option<PathBuf>,
) -> PlanUsage {
    let (spawn, lines) = codex_server(
        program,
        env,
        home.as_deref(),
        &[
            json!({"id": 2, "method": "account/read", "params": {}}),
            json!({"id": 3, "method": "account/rateLimits/read"}),
        ],
    );
    let usage = match ask(&spawn, &lines, &["2", "3"], codex_answer).await {
        Ok(answers) => match answers.get("2") {
            Some(answer) => read_codex(account, answer, answers.get("3")),
            None => PlanUsage::failed(SessionAgent::Codex, account, "Codex did not answer.".into()),
        },
        Err(error) => PlanUsage::failed(SessionAgent::Codex, account, error),
    };
    // Only a ChatGPT login has a plan. A read that worked and found none (an API key, signed out) forgets the old tier;
    // a failed read keeps it.
    if usage.signed_in {
        crate::plan_tier::remember(SessionAgent::Codex, home.as_deref(), usage.plan.as_deref());
    } else if usage.error.is_none() {
        crate::plan_tier::forget(SessionAgent::Codex, home.as_deref());
    }
    usage
}

/// Uses one of the resets banked on the Codex login in `home`, which clears its limits now. `attempt` names this try:
/// send the same one to retry it after a failure, so a retry cannot spend a second reset. Answers what happened.
pub async fn use_codex_reset(
    program: PathBuf,
    env: HashMap<String, String>,
    home: Option<PathBuf>,
    attempt: &str,
) -> Result<&'static str, String> {
    let (spawn, lines) = codex_server(
        program,
        env,
        home.as_deref(),
        &[json!({"id": 2, "method": "account/rateLimitResetCredit/consume", "params": {"idempotencyKey": attempt}})],
    );
    let answers = ask(&spawn, &lines, &["2"], codex_answer).await?;
    let answer = answers.get("2").ok_or("Codex did not answer.")?;
    if let Some(error) = error_text(answer) {
        return Err(format!("Codex could not use the reset ({error})."));
    }
    reset_outcome(answer.get("outcome").and_then(Value::as_str))
}

/// Codex's answer to a reset, as a sentence.
fn reset_outcome(outcome: Option<&str>) -> Result<&'static str, String> {
    match outcome {
        Some("reset") => Ok("Reset used. This account's Codex limits are clear."),
        Some("nothingToReset") => Ok("Nothing to reset right now."),
        Some("noCredit") => Ok("No reset is left on this account."),
        Some("alreadyRedeemed") => Ok("That reset was already used."),
        other => Err(format!(
            "Codex answered an unknown outcome ({}).",
            other.unwrap_or("none")
        )),
    }
}

/// What Settings and the composer say of an Antigravity plan signed in through Google Cloud ([`PlanUsage::google_cloud`])
/// that reports no limits. Gemini Enterprise shows the usage to its administrators on the Usage tab of "Usage & spend"
/// (https://docs.cloud.google.com/gemini/enterprise/docs/feature-usage).
pub const CLOUD_USAGE_NOTE: &str = "Antigravity doesn't report usage for a Google Cloud sign-in. Your organization's \
                                    admins see it in the Google Cloud console, under Gemini Enterprise › Usage & spend.";

/// Antigravity's limits for the person's Google sign-in, from `agy -p "/usage" --output-format json`, which answers
/// without a model turn: the same read as its sign-in check (`nexus_client::local_agents::antigravity_usage`), with no
/// browser opened. `env` must hold nothing that bills elsewhere; it runs in the home folder, which a sandboxed `agy`
/// sees, unlike the temporary folder.
pub async fn antigravity(program: PathBuf, env: HashMap<String, String>) -> PlanUsage {
    use nexus_client::local_agents::{Login, antigravity_cloud_project, antigravity_usage};
    let home = env
        .get("HOME")
        .or_else(|| env.get("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .cloned();
    let cwd = home.as_deref().map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let read = tokio::task::spawn_blocking(move || {
        (
            antigravity_usage(&program, &env, &cwd),
            antigravity_cloud_project(home.as_deref()),
        )
    })
    .await;
    let (read, cloud_project) = match read {
        Ok((read, project)) => (Ok(read), project),
        Err(error) => (Err(error), None),
    };
    let mut usage = PlanUsage::signed_out(SessionAgent::Antigravity, None);
    match read {
        // A Google Cloud sign-in answers placeholders, which are no limits. agy's settings keep the last Google Cloud
        // project after a personal sign-in, so the project only names a sign-in the answer shows to be Google Cloud's.
        Ok(Some((Login::Plan, groups))) if placeholders(&groups) => {
            usage.signed_in = true;
            usage.google_cloud = true;
            usage.plan = Some(match cloud_project {
                Some(project) => format!("Google Cloud · {project}"),
                None => "Google Cloud".into(),
            });
        }
        Ok(Some((Login::Plan, groups))) => {
            usage.signed_in = true;
            usage.plan = Some("Google AI".into());
            usage.limits = antigravity_limits(&groups);
        }
        // Signed out: no plan limits, as for the other agents.
        Ok(Some((Login::SignedOut, _))) => {}
        Ok(Some((Login::ApiKey, _))) => {
            usage.error = Some(
                "Antigravity is set to use an API key. Remove \"modelProvider\" from \
                 ~/.gemini/antigravity-cli/settings.json to use your Google AI plan."
                    .into(),
            )
        }
        Ok(Some((Login::Unknown, _))) => usage.error = Some("Antigravity did not report its quota.".into()),
        // Windows: the check never runs there (`local_agents::no_browser_dir`). The desktop says so; it is no failure.
        Ok(None) if cfg!(windows) => {}
        Ok(None) => {
            usage.error = Some(
                "Nexus could not run agy's quota check. It needs agy 1.1.11 or later, and a home folder it can \
                 write to."
                    .into(),
            );
        }
        Err(_) => usage.error = Some("The quota check stopped before it answered.".into()),
    }
    usage
}

/// Whether `/usage` answered placeholders, as it does for a Google Cloud sign-in
/// (`nexus-client/tests/fixtures/agy-1.5.11-usage-google-cloud.json`): every bucket is unused, has no window and resets
/// in 1970. A personal plan's buckets name their window.
fn placeholders(groups: &[nexus_client::local_agents::QuotaGroup]) -> bool {
    let unset = |at: Option<&str>| {
        at.is_none_or(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at.timestamp_millis() <= 0))
    };
    let mut buckets = groups.iter().flat_map(|group| &group.buckets).peekable();
    buckets.peek().is_some()
        && buckets
            .all(|bucket| bucket.remaining >= 1.0 && bucket.window.is_empty() && unset(bucket.resets_at.as_deref()))
}

/// Each model group's limits as [`Limit`]s: `Weekly · Gemini`, with the group's models, the share used, and the reset
/// as epoch milliseconds. A session finds its group by `models`, never by the label.
pub fn antigravity_limits(groups: &[nexus_client::local_agents::QuotaGroup]) -> Vec<Limit> {
    groups
        .iter()
        .flat_map(|group| {
            group.buckets.iter().map(move |bucket| {
                let mut window = bucket.window.clone();
                if let Some(first) = window.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                let window = if window.is_empty() { bucket.name.clone() } else { window };
                Limit {
                    models: group.models.clone(),
                    label: format!("{window} · {}", group_name(&group.name)),
                    used_percent: (1.0 - bucket.remaining) * 100.0,
                    resets_at: bucket
                        .resets_at
                        .as_deref()
                        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                        .map(|at| at.timestamp_millis())
                        // 1970: no window has started yet, not a reset that already passed.
                        .filter(|at| *at > 0),
                }
            })
        })
        .collect()
}

/// agy's group name without its closing "Models": "Gemini Models" is "Gemini" and "Claude and GPT models" is "Claude
/// and GPT", as Claude Code's own limits name their model ("Weekly · Fable"). The page then fits it on one line.
fn group_name(name: &str) -> &str {
    name.strip_suffix(" Models")
        .or_else(|| name.strip_suffix(" models"))
        .filter(|short| !short.is_empty())
        .unwrap_or(name)
}

/// Starts `spawn`, writes `lines` to it, and reads its output until `answer` has found each of `ids` or the output
/// ends; then closes its stdin and lets it end by itself ([`process::retire`]): it may be saving a renewed sign-in.
/// Fails when it cannot start or takes longer than [`PROBE_TIMEOUT`].
async fn ask(
    spawn: &Spawn,
    lines: &[String],
    ids: &[&str],
    answer: fn(&Value) -> Option<(String, Value)>,
) -> Result<HashMap<String, Value>, String> {
    let mut child =
        process::spawn(spawn).map_err(|error| format!("Could not start {}: {error}", spawn.program.display()))?;
    let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
    let read = async move {
        let (Some(mut stdin), Some(stdout)) = (stdin, stdout) else {
            return HashMap::new();
        };
        for line in lines {
            if stdin.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                return HashMap::new();
            }
        }
        let _ = stdin.flush().await;
        let mut output = BufReader::new(stdout).lines();
        let mut found = HashMap::new();
        while found.len() < ids.len()
            && let Ok(Some(line)) = output.next_line().await
        {
            if let Some((id, body)) = serde_json::from_str::<Value>(&line).ok().as_ref().and_then(answer)
                && ids.contains(&id.as_str())
            {
                found.insert(id, body);
            }
        }
        // stdin stays open until here: Claude Code ends when it closes.
        drop(stdin);
        found
    };
    // A timed-out read is dropped with its stdin, so the probe sees its input end either way. It is never killed on
    // the spot: a kill during a sign-in renewal costs the login.
    let found = tokio::time::timeout(PROBE_TIMEOUT, read).await;
    process::retire(child);
    found.map_err(|_| {
        format!(
            "{} did not answer within {} seconds.",
            spawn.program.display(),
            PROBE_TIMEOUT.as_secs()
        )
    })
}

/// A `control_response` by its `request_id`: the answer, or `{"error": …}`.
fn claude_answer(line: &Value) -> Option<(String, Value)> {
    if line.get("type").and_then(Value::as_str) != Some("control_response") {
        return None;
    }
    let response = line.get("response")?;
    let id = response.get("request_id")?.as_str()?.to_string();
    if response.get("subtype").and_then(Value::as_str) == Some("error") {
        return Some((id, json!({"error": response.get("error")})));
    }
    Some((id, response.get("response").cloned().unwrap_or(Value::Null)))
}

/// A JSON-RPC answer by its `id`: the result, or `{"error": …}`. Notifications and requests have a `method`.
fn codex_answer(line: &Value) -> Option<(String, Value)> {
    if line.get("method").is_some() {
        return None;
    }
    let id = line.get("id")?;
    let id = id
        .as_i64()
        .map(|n| n.to_string())
        .or_else(|| id.as_str().map(String::from))?;
    let body = match line.get("result") {
        Some(result) => result.clone(),
        None => json!({"error": line.pointer("/error/message")}),
    };
    Some((id, body))
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(String::from)
}

fn error_text(answer: &Value) -> Option<String> {
    let error = answer.get("error")?;
    Some(error.as_str().map_or_else(|| error.to_string(), String::from))
}

/// Claude Code's `initialize` and `get_usage` answers as a plan's limits: the account-wide session (five-hour) and
/// weekly limits, then each model's own weekly limit (`Weekly · Fable`) from the newer `limits` list.
pub fn read_claude(account: Option<String>, init: &Value, usage: Option<&Value>) -> PlanUsage {
    let who = init.get("account").unwrap_or(&Value::Null);
    let mut plan = PlanUsage {
        email: text(who.get("email")),
        plan: nexus_local_agents::claude_plan(who)
            .map(|plan| plan.trim().to_owned())
            .filter(|plan| !plan.is_empty()),
        ..PlanUsage::signed_out(SessionAgent::Claude, account)
    };
    plan.signed_in = plan.plan.is_some();
    if !plan.signed_in {
        return plan;
    }
    let Some(usage) = usage else {
        plan.error = Some("Claude Code did not report its limits.".into());
        return plan;
    };
    if let Some(error) = error_text(usage) {
        plan.error = Some(format!(
            "Claude Code could not report its limits ({error}). Update Claude Code."
        ));
        return plan;
    }
    if usage.get("rate_limits_available") != Some(&Value::Bool(true)) {
        plan.unavailable = true;
        return plan;
    }
    let limits = usage.get("rate_limits").unwrap_or(&Value::Null);
    for (key, label) in [("five_hour", "Session"), ("seven_day", "Weekly")] {
        let Some(limit) = limits.get(key) else { continue };
        if let Some(used) = limit.get("utilization").and_then(Value::as_f64) {
            plan.limits.push(Limit {
                models: Vec::new(),
                label: label.into(),
                used_percent: used.clamp(0.0, 100.0),
                resets_at: limit.get("resets_at").and_then(Value::as_str).and_then(rfc3339_ms),
            });
        }
    }
    for limit in limits.get("limits").and_then(Value::as_array).into_iter().flatten() {
        let model = limit.pointer("/scope/model/display_name").and_then(Value::as_str);
        let used = limit.get("percent").and_then(Value::as_f64);
        if limit.get("kind").and_then(Value::as_str) == Some("weekly_scoped")
            && let (Some(model), Some(used)) = (model, used)
        {
            plan.limits.push(Limit {
                models: Vec::new(),
                label: format!("Weekly · {model}"),
                used_percent: used.clamp(0.0, 100.0),
                resets_at: limit.get("resets_at").and_then(Value::as_str).and_then(rfc3339_ms),
            });
        }
    }
    plan
}

/// Codex's `account/read` and `account/rateLimits/read` results as a plan's limits. Only a ChatGPT login has a plan.
pub fn read_codex(account: Option<String>, answer: &Value, limits: Option<&Value>) -> PlanUsage {
    let who = answer.get("account").unwrap_or(&Value::Null);
    let mut plan = PlanUsage {
        email: text(who.get("email")),
        plan: text(who.get("planType")),
        signed_in: who.get("type").and_then(Value::as_str) == Some("chatgpt"),
        ..PlanUsage::signed_out(SessionAgent::Codex, account)
    };
    if !plan.signed_in {
        return plan;
    }
    let Some(limits) = limits else {
        plan.error = Some("Codex did not report its limits.".into());
        return plan;
    };
    if let Some(error) = error_text(limits) {
        plan.error = Some(format!("Codex could not report its limits ({error})."));
        return plan;
    }
    for key in ["primary", "secondary"] {
        let Some(window) = limits.pointer(&format!("/rateLimits/{key}")) else {
            continue;
        };
        if let Some(used) = window.get("usedPercent").and_then(Value::as_f64) {
            plan.limits.push(Limit {
                models: Vec::new(),
                label: window_label(window.get("windowDurationMins").and_then(Value::as_i64)),
                used_percent: used.clamp(0.0, 100.0),
                resets_at: window
                    .get("resetsAt")
                    .and_then(Value::as_i64)
                    .map(|seconds| seconds * 1000),
            });
        }
    }
    plan.resets = limits.get("rateLimitResetCredits").and_then(read_resets);
    plan
}

/// Codex's `rateLimitResetCredits`: how many, and when the first of the listed available ones expires. The list may be
/// missing or shorter than the count.
fn read_resets(credits: &Value) -> Option<Resets> {
    let available = credits.get("availableCount").and_then(Value::as_u64)?;
    let expires_at = credits
        .get("credits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|credit| credit.get("status").and_then(Value::as_str) == Some("available"))
        .filter_map(|credit| credit.get("expiresAt").and_then(Value::as_i64))
        .min()
        .map(|seconds| seconds * 1000);
    Some(Resets {
        available: u32::try_from(available).unwrap_or(u32::MAX),
        expires_at,
    })
}

/// Codex's windows by their length: five hours is the session, a week the weekly limit.
fn window_label(minutes: Option<i64>) -> String {
    match minutes {
        Some(300) => "Session".into(),
        Some(10_080) => "Weekly".into(),
        Some(minutes) => format!("{} h", minutes / 60),
        None => "Limit".into(),
    }
}

/// Epoch milliseconds of an RFC 3339 time, as Anthropic writes them (`2026-10-21T20:09:59.868119+00:00`, `…Z`).
fn rfc3339_ms(time: &str) -> Option<i64> {
    let (date, clock) = time.split_once('T')?;
    let (clock, offset_minutes) = match clock.strip_suffix('Z') {
        Some(clock) => (clock, 0),
        None => {
            let (clock, offset) = clock.split_at(clock.rfind(['+', '-'])?);
            let (hours, minutes) = offset[1..].split_once(':')?;
            let minutes = hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?;
            (clock, if offset.starts_with('-') { -minutes } else { minutes })
        }
    };
    let numbers = |text: &str, separator: char| -> Option<Vec<i64>> {
        let parts: Option<Vec<i64>> = text.split(separator).map(|part| part.parse().ok()).collect();
        parts.filter(|parts| parts.len() == 3)
    };
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let (ymd, hms) = (numbers(date, '-')?, numbers(whole, ':')?);
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let (month, day) = (ymd[1], ymd[2]);
    let year = if month <= 2 { ymd[0] - 1 } else { ymd[0] };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468;
    let seconds = days * 86_400 + hms[0] * 3600 + hms[1] * 60 + hms[2] - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that answers, then saves a renewed sign-in once its input ends, gets to save it. The probe
    /// was killed right after its answers, and a kill between the vendor's answer and the save left a spent token.
    #[cfg(unix)]
    #[test]
    fn a_probe_that_answered_ends_by_itself_and_finishes_what_it_was_saving() {
        let dir = std::env::temp_dir().join(format!("nexus-probe-retire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("probe folder");
        let saved = dir.join("saved");
        let script = dir.join("probe.sh");
        crate::process::write_fake_script(
            &script,
            &format!(
                "#!/bin/sh\nread a\nread b\n\
                 echo '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"init\",\"response\":{{}}}}}}'\n\
                 echo '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"usage\",\"response\":{{}}}}}}'\n\
                 cat > /dev/null\nsleep 1\necho token > '{}'\n",
                saved.display()
            ),
        );
        let spawn = Spawn {
            program: script,
            args: vec![],
            env: std::env::vars().collect(),
            cwd: dir.clone(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let lines = ["{}".to_string(), "{}".to_string()];
        let answers = runtime
            .block_on(ask(&spawn, &lines, &["init", "usage"], claude_answer))
            .expect("answers");
        assert_eq!(answers.len(), 2);
        // The answers come back at once; the probe goes on to save, with no runtime left to drive it.
        drop(runtime);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !saved.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(saved.exists(), "the probe was stopped before it saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lost_claude_login_still_names_its_account() {
        let state = json!({"oauthAccount": {"emailAddress": "ana@acme.dev", "organizationUuid": "o-1"}});
        assert_eq!(claude_last_email(&state).as_deref(), Some("ana@acme.dev"));
        for none in [
            json!({}),
            json!({"oauthAccount": null}),
            json!({"oauthAccount": {"emailAddress": ""}}),
        ] {
            assert_eq!(claude_last_email(&none), None, "{none}");
        }
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(
            claude_state_file(&env(&[("CLAUDE_CONFIG_DIR", "/p/work"), ("HOME", "/home/ana")])),
            Some(PathBuf::from("/p/work/.claude.json"))
        );
        assert_eq!(
            claude_state_file(&env(&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/home/ana")])),
            Some(PathBuf::from("/home/ana/.claude.json"))
        );
        assert_eq!(claude_state_file(&env(&[])), None);
    }

    #[test]
    fn reads_rfc3339_times() {
        assert_eq!(rfc3339_ms("2026-10-21T20:09:59.868119+00:00"), Some(1_792_613_399_868));
        assert_eq!(rfc3339_ms("2026-10-23T05:59:59.8+02:00"), Some(1_792_727_999_800));
        assert_eq!(rfc3339_ms("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        assert_eq!(rfc3339_ms("1969-12-31T23:59:59-00:30"), Some(1_799_000));
        for bad in ["", "2026-10-21", "2026-10-21T20:09", "soon"] {
            assert_eq!(rfc3339_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn reads_claude_codes_limits() {
        let init =
            json!({"account": {"email": "you@example.com", "organization": "Acme", "subscriptionType": "Claude Team"}});
        let usage = json!({
            "subscription_type": "team",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": {"utilization": 15, "resets_at": "2026-10-21T20:09:59.868119+00:00"},
                "seven_day": {"utilization": 52, "resets_at": "2024-02-29T00:00:00Z"},
                "seven_day_opus": null,
                "limits": [
                    {"kind": "session", "percent": 15, "resets_at": "2026-10-21T20:09:59.868119+00:00", "scope": null},
                    {"kind": "weekly_all", "percent": 52, "scope": null},
                    {"kind": "weekly_scoped", "percent": 29, "resets_at": null, "scope": {"model": {"id": null, "display_name": "Fable"}}}
                ]
            }
        });
        let plan = read_claude(Some("a_1".into()), &init, Some(&usage));
        assert_eq!(
            plan,
            PlanUsage {
                agent: SessionAgent::Claude,
                account: Some("a_1".into()),
                email: Some("you@example.com".into()),
                plan: Some("Claude Team".into()),
                signed_in: true,
                active: false,
                limits: vec![
                    Limit {
                        models: Vec::new(),
                        label: "Session".into(),
                        used_percent: 15.0,
                        resets_at: Some(1_792_613_399_868)
                    },
                    Limit {
                        models: Vec::new(),
                        label: "Weekly".into(),
                        used_percent: 52.0,
                        resets_at: Some(1_709_164_800_000)
                    },
                    Limit {
                        models: Vec::new(),
                        label: "Weekly · Fable".into(),
                        used_percent: 29.0,
                        resets_at: None
                    },
                ],
                resets: None,
                error: None,
                google_cloud: false,
                unavailable: false,
                fetched_at: None,
            }
        );
    }

    /// agy 1.5.11's `/usage` signed in through Google Cloud: every model at 100%, reset in 1970.
    const AGY_CLOUD_USAGE: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.11-usage-google-cloud.json");

    /// agy 1.5.8's `/usage` on a personal Google AI plan.
    const AGY_PERSONAL_USAGE: &str = include_str!("../../nexus-client/tests/fixtures/agy-1.5.8-usage.json");

    /// A home whose `agy` answers `/usage` with `usage`, and whose agy settings are `settings`.
    #[cfg(unix)]
    fn agy_home(settings: &str, usage: &str) -> (PathBuf, PathBuf, HashMap<String, String>) {
        let home = std::env::temp_dir().join(format!("nexus-plans-agy-{}", uuid::Uuid::new_v4()));
        let settings_file = nexus_client::local_agents::antigravity_settings(&home);
        std::fs::create_dir_all(settings_file.parent().expect("dir")).expect("dir");
        std::fs::write(&settings_file, settings).expect("write");
        let answer = home.join("usage.json");
        std::fs::write(&answer, usage).expect("write");
        let agy = home.join("agy");
        crate::process::write_fake_script(
            &agy,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.5.11; exit 0; fi\n/bin/cat '{}'\n",
                answer.display()
            ),
        );
        let env = HashMap::from([
            ("HOME".to_string(), home.to_string_lossy().into_owned()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ]);
        (home, agy, env)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn google_cloud_sign_in_shows_no_placeholder_limits() {
        let (home, agy, env) = agy_home(PROJECT, AGY_CLOUD_USAGE);
        let plan = antigravity(agy, env).await;
        assert!(plan.signed_in && plan.google_cloud && plan.error.is_none(), "{plan:?}");
        assert_eq!(plan.plan.as_deref(), Some("Google Cloud · ai-tools-example"));
        assert!(plan.limits.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// agy's settings after a Google Cloud sign-in: they keep the project after a personal sign-in.
    const PROJECT: &str = r#"{"gcp": {"project": "ai-tools-example", "location": "us"}}"#;

    #[cfg(unix)]
    #[tokio::test]
    async fn personal_sign_in_after_a_google_cloud_one_is_google_ai() {
        let (home, agy, env) = agy_home(PROJECT, AGY_PERSONAL_USAGE);
        let plan = antigravity(agy, env).await;
        assert!(plan.signed_in && !plan.google_cloud, "{plan:?}");
        assert_eq!(plan.plan.as_deref(), Some("Google AI"));
        assert_eq!(plan.limits.len(), 2);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn placeholders_are_a_google_cloud_sign_in_without_a_project_too() {
        let (home, agy, env) = agy_home("{}", AGY_CLOUD_USAGE);
        let plan = antigravity(agy, env).await;
        assert!(plan.signed_in && plan.google_cloud);
        assert_eq!(plan.plan.as_deref(), Some("Google Cloud"));
        assert!(plan.limits.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn reset_time_of_1970_is_no_window_yet() {
        use nexus_client::local_agents::{QuotaBucket, QuotaGroup};
        let bucket = |remaining: f64, window: &str| QuotaBucket {
            name: "Weekly Limit Remaining".into(),
            window: window.into(),
            remaining,
            resets_at: Some("1970-01-01T00:00:00Z".into()),
        };
        let group = |buckets| QuotaGroup {
            name: "Gemini Models".into(),
            models: vec!["Gemini 3.8 Flash".into()],
            buckets,
        };
        // A personal plan's weekly bucket with no window started: a limit with no reset, not one already past.
        let unused = [group(vec![bucket(1.0, "weekly")])];
        assert!(!placeholders(&unused));
        let limits = antigravity_limits(&unused);
        assert_eq!((limits.len(), limits[0].resets_at), (1, None));
        // Real numbers on a Google Cloud sign-in are shown.
        assert!(!placeholders(&[group(vec![bucket(0.8, "")])]));
        assert!(placeholders(&[group(vec![bucket(1.0, "")])]));
        assert!(!placeholders(&[]));
    }

    #[test]
    fn claude_codes_cache_dates_the_limits_it_answered_with() {
        let init = json!({"account": {"email": "you@example.com", "subscriptionType": "Claude Team"}});
        let answered = read_claude(
            None,
            &init,
            Some(&json!({"rate_limits_available": true, "rate_limits": {
                "five_hour": {"utilization": 17, "resets_at": "2026-11-18T15:50:00.005713+00:00"},
                "seven_day": {"utilization": 37, "resets_at": "2026-11-21T00:00:00.005733+00:00"}}})),
        );
        let state = |five_hour: f64, resets_at: &str| {
            json!({"cachedUsageUtilization": {"fetchedAtMs": 1_795_000_896_164_i64, "utilization": {
                "five_hour": {"utilization": five_hour, "resets_at": resets_at},
                "seven_day": {"utilization": 37}}}})
        };
        let window = "2026-11-18T15:50:00.005713+00:00";
        assert_eq!(
            claude_fetched_at(&state(17., window), &answered),
            Some(1_795_000_896_164)
        );
        // Its numbers are not the answer's: Claude Code fetched the answer itself, at a time it does not say.
        assert_eq!(claude_fetched_at(&state(18., window), &answered), None);
        // The same number of an earlier window is not the answer's either.
        assert_eq!(
            claude_fetched_at(&state(17., "2026-11-18T10:50:00.005713+00:00"), &answered),
            None
        );
        assert_eq!(claude_fetched_at(&json!({"numStartups": 3}), &answered), None);
    }

    #[test]
    fn signed_out_or_older_claude_code_has_no_limits() {
        let signed_out = read_claude(
            None,
            &json!({"account": {"tokenSource": "none"}}),
            Some(&json!({"rate_limits_available": false})),
        );
        assert!(!signed_out.signed_in && signed_out.limits.is_empty() && signed_out.error.is_none());

        let init = json!({"account": {"email": "you@example.com", "subscriptionType": "Claude Max"}});
        let mid_turn = read_claude(None, &init, Some(&json!({"rate_limits_available": false})));
        assert!(mid_turn.signed_in && mid_turn.unavailable && mid_turn.limits.is_empty() && mid_turn.error.is_none());

        let older = read_claude(
            None,
            &init,
            Some(&json!({"error": "Unknown control request subtype: get_usage"})),
        );
        assert!(older.signed_in && older.limits.is_empty());
        assert!(
            older
                .error
                .is_some_and(|error| error.contains("Unknown control request"))
        );
    }

    #[test]
    fn reads_codexs_limits() {
        let account = json!({"account": {"type": "chatgpt", "email": "you@example.com", "planType": "plus"}});
        let limits = json!({"rateLimits": {
            "primary": {"usedPercent": 7, "windowDurationMins": 300, "resetsAt": 1_792_605_814},
            "secondary": {"usedPercent": 120, "windowDurationMins": 10_080, "resetsAt": null}
        }});
        let plan = read_codex(Some("a_2".into()), &account, Some(&limits));
        assert!(plan.signed_in);
        assert_eq!(
            (plan.account.as_deref(), plan.plan.as_deref()),
            (Some("a_2"), Some("plus"))
        );
        assert_eq!(
            plan.limits,
            vec![
                Limit {
                    models: Vec::new(),
                    label: "Session".into(),
                    used_percent: 7.0,
                    resets_at: Some(1_792_605_814_000)
                },
                Limit {
                    models: Vec::new(),
                    label: "Weekly".into(),
                    used_percent: 100.0,
                    resets_at: None
                },
            ]
        );
        assert_eq!(plan.resets, None, "an older Codex reports no resets");
        let api_key = read_codex(None, &json!({"account": {"type": "apiKey"}}), Some(&limits));
        assert!(!api_key.signed_in && api_key.limits.is_empty());
        // Signed out: Codex answers no account, and refuses the limits.
        let refused = json!({"error": "codex account authentication required to read rate limits"});
        let signed_out = read_codex(
            None,
            &json!({"account": null, "requiresOpenaiAuth": true}),
            Some(&refused),
        );
        assert!(!signed_out.signed_in && signed_out.limits.is_empty() && signed_out.error.is_none());
    }

    #[test]
    fn reads_and_uses_codexs_banked_resets() {
        let account = json!({"account": {"type": "chatgpt", "email": "you@example.com", "planType": "plus"}});
        // As codex-cli 0.167.0 answers: the soonest expiry of the available ones counts, a redeemed one does not.
        let credit = |status: &str, expires: i64| json!({"id": "c", "resetType": "codexRateLimits", "status": status, "grantedAt": 1, "expiresAt": expires, "title": "Full reset"});
        let limits = json!({
            "rateLimits": {"primary": {"usedPercent": 93, "windowDurationMins": 10_080, "resetsAt": 1_795_128_957}},
            "rateLimitResetCredits": {"availableCount": 2, "credits": [
                credit("available", 1_797_113_927), credit("redeemed", 1_700_000_000), credit("available", 1_796_163_816)
            ]}
        });
        let plan = read_codex(None, &account, Some(&limits));
        assert_eq!(
            plan.resets,
            Some(Resets {
                available: 2,
                expires_at: Some(1_796_163_816_000)
            })
        );
        // Only the count, when Codex skips the details.
        let count_only = json!({"rateLimitResetCredits": {"availableCount": 1, "credits": null}});
        assert_eq!(
            read_codex(None, &account, Some(&count_only)).resets,
            Some(Resets {
                available: 1,
                expires_at: None
            })
        );

        assert_eq!(
            reset_outcome(Some("reset")),
            Ok("Reset used. This account's Codex limits are clear.")
        );
        assert_eq!(reset_outcome(Some("noCredit")), Ok("No reset is left on this account."));
        assert!(reset_outcome(Some("later")).is_err() && reset_outcome(None).is_err());
    }

    #[test]
    fn picks_answers_by_id() {
        let claude = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "usage", "response": {"a": 1}}});
        assert_eq!(claude_answer(&claude), Some(("usage".into(), json!({"a": 1}))));
        let refused =
            json!({"type": "control_response", "response": {"subtype": "error", "request_id": "usage", "error": "no"}});
        assert_eq!(claude_answer(&refused), Some(("usage".into(), json!({"error": "no"}))));
        assert_eq!(claude_answer(&json!({"type": "system", "subtype": "init"})), None);

        assert_eq!(
            codex_answer(&json!({"id": 3, "result": {"b": 2}})),
            Some(("3".into(), json!({"b": 2})))
        );
        assert_eq!(
            codex_answer(&json!({"id": 3, "error": {"message": "no"}})),
            Some(("3".into(), json!({"error": "no"})))
        );
        assert_eq!(codex_answer(&json!({"method": "account/updated", "params": {}})), None);
    }
}
