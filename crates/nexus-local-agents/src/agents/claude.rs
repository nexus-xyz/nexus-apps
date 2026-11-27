//! Claude Code on this machine: `claude auth status --json` read, and its launch, through Gateway or on the plan.
//!
//! Through Gateway: `ANTHROPIC_BASE_URL=<core>/v1/anthropic`, `ANTHROPIC_AUTH_TOKEN=<your Nexus key>`, the session (and
//! the team) in `ANTHROPIC_CUSTOM_HEADERS`; the user's own `ANTHROPIC_API_KEY` and the Bedrock, Vertex and Foundry
//! switches are removed so it cannot bypass Nexus. On the plan it keeps its own login and reports each call's usage
//! (not its content) to the session over OpenTelemetry.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nexus_agents::{Agent, AgentT, Claude};
use nexus_providers::{Credential, ProviderError};
use serde_json::Value;

use super::LocalAgentT;
use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions, SESSION_HEADER, TEAM_HEADER, after, credential_of, usage_url};

/// Claude Code's sign-ins that bill the Claude plan: its own login, and the long-lived token `claude setup-token` makes.
const CLAUDE_PLAN_METHODS: [&str; 2] = ["claude.ai", "oauth_token"];

/// Set to `1`, a Claude plan run reports no usage to its session, so Claude Code runs without telemetry.
pub const CLAUDE_NO_USAGE_ENV: &str = "NEXUS_CLAUDE_NO_USAGE";

/// The name Claude Code reports for a plan. Its initialize answer may omit the sign-in method.
pub fn claude_plan(account: &Value) -> Option<String> {
    if let Some(method) = account.get("authMethod")
        && !method
            .as_str()
            .is_some_and(|method| CLAUDE_PLAN_METHODS.contains(&method))
    {
        return None;
    }
    account
        .get("subscriptionType")
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(String::from)
}

/// Claude Code's `auth status --json` (its stdout): logged in, how, and the plan; nothing else is read.
pub fn read_claude_status(output: &str) -> (Login, Option<String>) {
    let Ok(status) = serde_json::from_str::<Value>(output.trim()) else {
        return (Login::Unknown, None);
    };
    let method = status.get("authMethod").and_then(Value::as_str).unwrap_or_default();
    match status.get("loggedIn") {
        Some(Value::Bool(false)) => (Login::SignedOut, None),
        Some(Value::Bool(true)) if CLAUDE_PLAN_METHODS.contains(&method) => (Login::Plan, claude_plan(&status)),
        Some(Value::Bool(true)) => (Login::ApiKey, None),
        _ => (Login::Unknown, None),
    }
}

/// The account Claude Code's `auth status --json` names (its email), so a run on the wrong profile shows at once.
pub fn read_claude_account(output: &str) -> Option<String> {
    let status = serde_json::from_str::<Value>(output.trim()).ok()?;
    status
        .get("email")
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .map(String::from)
}

impl LocalAgentT for Claude {
    fn model_catalog<'a>(
        &self,
        _program: &'a std::path::Path,
        _env: &'a HashMap<String, String>,
        _dir: &'a std::path::Path,
    ) -> super::CatalogFuture<'a> {
        Box::pin(async { None })
    }

    fn models<'a>(
        &self,
        program: &'a std::path::Path,
        path: &'a str,
        _version: Option<&'a str>,
        env: Option<&'a HashMap<String, String>>,
    ) -> super::ModelFuture<'a> {
        Box::pin(crate::claude_models::discover_with_env(program, path, env))
    }

    /// No file settles Claude Code's sign-in: its check reads it.
    fn settled_login(&self, _home: Option<&str>) -> Option<Login> {
        None
    }

    fn read_status(&self, stdout: &str, _stderr: &str) -> (Login, Option<String>) {
        read_claude_status(stdout)
    }

    fn read_account(&self, stdout: &str) -> Option<String> {
        read_claude_account(stdout)
    }

    fn launch(&self, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
        let credential = credential_of(&options);
        self.launch_with_credential(options, credential)
    }

    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        mut credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>) {
        // The person may opt out: Claude Code turns off features (Code Mode) while telemetry is on. The session then
        // records no usage.
        if let Credential::AgentLogin { usage, .. } = &mut credential
            && options.env.get(CLAUDE_NO_USAGE_ENV).is_some_and(|v| v == "1")
        {
            *usage = None;
        }
        let mut env = options.env;
        let args = super::route_env(Agent::Claude, &credential).apply(&mut env);
        claude_model(&mut env, options.model);
        (after(args, options.args), env)
    }

    /// The person's usage opt-out (`NEXUS_CLAUDE_NO_USAGE`) is an `AgentLogin` with no usage sink: whoever builds the
    /// credential applies it, as `launch` does.
    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        match credential {
            Credential::VendorKey {
                vendor: nexus_providers::KeyVendor::Anthropic,
                secret,
            } => {
                let mut remove = Claude.entry().billing.plan_overrides.to_vec();
                remove.extend(["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK"]);
                Ok(CredentialEnv {
                    set: vec![
                        ("ANTHROPIC_API_KEY", Cow::Borrowed(secret.expose())),
                        ("ANTHROPIC_BASE_URL", Cow::Borrowed("https://api.anthropic.com")),
                        ("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", Cow::Borrowed("1")),
                        ("DISABLE_TELEMETRY", Cow::Borrowed("1")),
                    ],
                    remove,
                    args: Vec::new(),
                })
            }
            Credential::Gateway {
                core_url,
                token,
                session_id,
                team,
            } => {
                // What would send Claude Code past Nexus. Three of them are set again below, to Gateway. The plan's own
                // long-lived token (`claude setup-token`) is not in `plan_overrides`, because a subscription run and a
                // status check need it. Here it would compete with the gateway token, so it goes.
                let mut remove = Claude.entry().billing.plan_overrides.to_vec();
                remove.push("CLAUDE_CODE_OAUTH_TOKEN");
                // One header per line, as Claude Code reads them.
                let mut headers = vec![format!("{SESSION_HEADER}: {session_id}")];
                if let Some(team) = team.as_deref().filter(|t| !t.is_empty()) {
                    headers.push(format!("{TEAM_HEADER}: {team}"));
                }
                Ok(CredentialEnv {
                    set: vec![
                        ("ANTHROPIC_BASE_URL", Cow::Owned(format!("{core_url}/v1/anthropic"))),
                        ("ANTHROPIC_AUTH_TOKEN", Cow::Borrowed(token.expose())),
                        ("ANTHROPIC_CUSTOM_HEADERS", Cow::Owned(headers.join("\n"))),
                        // Fast mode (`/fast`, Nexus Desktop's Speed) is priced and checked by Gateway, not by the
                        // claude.ai organization. Claude Code cannot ask that organization through Gateway, so without
                        // this it reports fast mode as turned off by the organization and never sends
                        // `speed: "fast"` (claude 2.1.328).
                        ("CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK", Cow::Borrowed("1")),
                    ],
                    remove,
                    args: Vec::new(),
                })
            }
            Credential::AgentLogin { usage, .. } => {
                // An API key or a gateway token takes precedence over the plan's login, so none may reach Claude Code.
                // Usage, not content: each call's event goes to the session; prompts and tool details stay redacted.
                let mut remove = Claude.entry().billing.plan_overrides.to_vec();
                remove.extend(["OTEL_LOG_USER_PROMPTS", "OTEL_LOG_TOOL_DETAILS"]);
                let mut set: Vec<(&'static str, Cow<'c, str>)> =
                    super::account_dir_env(Agent::Claude, credential).into_iter().collect();
                // Only logs are set here; metrics or traces export only where the person's own OTEL_* variables already
                // say.
                if let Some(usage) = usage {
                    set.extend([
                        ("CLAUDE_CODE_ENABLE_TELEMETRY", Cow::Borrowed("1")),
                        ("OTEL_LOGS_EXPORTER", Cow::Borrowed("otlp")),
                        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", Cow::Borrowed("http/json")),
                        ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", Cow::Owned(usage_url(usage))),
                        (
                            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
                            Cow::Owned(format!("Authorization=Bearer {}", usage.token.expose())),
                        ),
                    ]);
                }
                Ok(CredentialEnv {
                    set,
                    remove,
                    args: Vec::new(),
                })
            }
            other => Err(ProviderError::NotOffered {
                agent: Agent::Claude,
                kind: other.kind(),
            }),
        }
    }

    /// Claude Code's command shells inherit its environment.
    fn shell_variable(&self, _name: &str, _value: &str) -> Option<[String; 2]> {
        None
    }

    fn takes_short_model_flags(&self) -> bool {
        false
    }

    fn provider_model_id(&self, id: &str) -> String {
        id.strip_prefix("anthropic/").unwrap_or(id).replace('.', "-")
    }

    fn takes_instructions(&self) -> bool {
        true
    }

    /// Claude Code keeps only one `--append-system-prompt`, so `text` joins one already given (a recording's, or the
    /// user's). A run that is not a conversation (`mcp list`, `--version`) gets none.
    fn with_instructions(&self, args: &[String], text: &str) -> Vec<String> {
        let own = args.iter().position(|a| a == "--").unwrap_or(args.len());
        if !starts_conversation(&args[..own]) {
            return args.to_vec();
        }
        let mut out = args.to_vec();
        for i in 0..own {
            if out[i] == "--append-system-prompt" && i + 1 < own {
                out[i + 1] = format!("{}\n\n{text}", out[i + 1]);
                return out;
            }
            if let Some(prompt) = out[i].strip_prefix("--append-system-prompt=") {
                out[i] = format!("--append-system-prompt={prompt}\n\n{text}");
                return out;
            }
        }
        out.splice(0..0, ["--append-system-prompt".to_string(), text.to_string()]);
        out
    }

    fn takes_recording(&self) -> bool {
        true
    }

    /// `claude "<prompt>"` starts an interactive conversation on the prompt.
    fn takes_prompt_argument(&self) -> bool {
        true
    }

    /// The system prompt says how to read the recording. With nothing else to say, a first prompt: look at it and say
    /// what you understood.
    fn with_recording(&self, args: &[String], prompt: &str, _sheet: &Path) -> Vec<String> {
        let mut out = vec!["--append-system-prompt".to_string(), prompt.to_string()];
        out.extend(args.iter().cloned());
        if args.is_empty() {
            out.push(
                "Look at my screen recording first (your instructions say where it is), then tell me what you understood."
                    .to_string(),
            );
        }
        out
    }

    fn memory(&self, home: &Path, repo: &Path) -> std::io::Result<Vec<nexus_agent_memory::NativeEntry>> {
        crate::native_memory::claude::read(home, repo)
    }

    fn write_memory(&self, home: &Path, repo: &Path, entries: &[nexus_agent_memory::Entry]) -> std::io::Result<()> {
        crate::native_memory::claude::write(home, repo, entries)
    }

    fn keeps_transcript(&self) -> bool {
        true
    }

    /// Its newest transcript in this folder written since `since`, for `--continue` and a `--resume` picked in its own
    /// list.
    fn newest_conversation(&self, home: &Path, cwd: &Path, since: SystemTime) -> Option<String> {
        let dir = home.join("projects").join(claude_project_name(cwd));
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                let id = name.strip_suffix(".jsonl")?.to_string();
                (modified >= since && is_session_id(&id)).then_some((modified, id))
            })
            .max()
            .map(|(_, id)| id)
    }

    /// `<home>/projects/<folder>/<id>.jsonl`, once it is a file.
    fn transcript(&self, home: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
        let path = home
            .join("projects")
            .join(claude_project_name(cwd))
            .join(format!("{id}.jsonl"));
        Some(path).filter(|path| path.is_file())
    }
}

/// Claude Code's subcommands and flags that start no conversation: no `--session-id`, so Nexus UI shows no resume
/// command that would fail. Matched anywhere before `--`, so a prompt word or flag value that happens to be one only
/// costs that run its resume command.
pub const CLAUDE_NO_CONVERSATION: [&str; 27] = [
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "config",
    "doctor",
    "gateway",
    "import",
    "install",
    "logs",
    "mcp",
    "migrate-installer",
    "plugin",
    "plugins",
    "purge",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "kill",
    "ultrareview",
    "update",
    "upgrade",
    "-v",
    "--version",
    "-h",
    "--help",
];

/// Whether Claude Code would start a conversation, for the instructions: a subcommand (`mcp list`) only counts as the
/// first argument and a flag (`--version`) anywhere, so a prompt word such as `logs` keeps them.
fn starts_conversation(args: &[String]) -> bool {
    let named = |arg: &String| CLAUDE_NO_CONVERSATION.contains(&arg.as_str());
    !args.first().is_some_and(named) && !args.iter().any(|arg| arg.starts_with('-') && named(arg))
}

/// The project slug used by the installed Claude Code 2.1.326 bundle's project lookup.
pub fn claude_project_name(cwd: &Path) -> String {
    // Claude truncates the sanitized slug at 200 UTF-16 units and hashes the original path.
    const LIMIT: usize = 200;
    let path = cwd.to_string_lossy();
    let mut slug = String::new();
    let mut units = 0;
    let mut hash = 0i32;
    for unit in path.encode_utf16() {
        if units < LIMIT {
            let character = u8::try_from(unit).ok().filter(u8::is_ascii_alphanumeric);
            slug.push(character.map_or('-', char::from));
        }
        units += 1;
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    if units > LIMIT {
        slug.push('-');
        let mut digits = Vec::new();
        let mut value = hash.unsigned_abs();
        loop {
            digits.push(char::from_digit(value % 36, 36).expect("base-36 digit"));
            value /= 36;
            if value == 0 {
                break;
            }
        }
        slug.extend(digits.into_iter().rev());
    }
    slug
}

/// A session id as Claude Code names its transcript: never a path.
pub fn is_session_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
}

/// Claude Code's model, unless the person already set `ANTHROPIC_MODEL`.
fn claude_model(env: &mut HashMap<String, String>, model: Option<&str>) {
    if let Some(model) = model.filter(|m| !m.is_empty())
        && env.get("ANTHROPIC_MODEL").is_none_or(|m| m.is_empty())
    {
        env.insert("ANTHROPIC_MODEL".into(), model.into());
    }
}
