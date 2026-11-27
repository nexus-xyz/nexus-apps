//! What every agent's launch shares: the route it runs on, the options a client gives, and the model flags in its
//! arguments. Each agent's own arguments and environment are its [`LocalAgentT::launch`](crate::LocalAgentT::launch).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use nexus_agents::Agent;
use nexus_providers::{Credential, Secret, UsageSink};

use crate::agents::local;

/// The session header the gateway attributes spend to (`SESSION_HEADER` in the gateway).
pub const SESSION_HEADER: &str = "x-nexus-session";
/// The team header (`TEAM_HEADER` in the gateway): the caller must be in the team.
pub const TEAM_HEADER: &str = "x-nexus-team";

/// How an agent reaches its models (`nexus_routing::routing::agent_access`'s routes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Through Nexus Gateway: every call reserved, metered and checked.
    Gateway,
    /// On the person's own plan (Claude, ChatGPT), signed in with the agent's own login.
    Subscription,
}

impl Route {
    /// Every route, in the order clients show them.
    pub const ALL: [Route; 2] = [Route::Gateway, Route::Subscription];

    pub fn as_str(self) -> &'static str {
        match self {
            Route::Gateway => "gateway",
            Route::Subscription => "subscription",
        }
    }

    pub fn parse(text: &str) -> Option<Route> {
        Route::ALL.into_iter().find(|route| route.as_str() == text)
    }

    /// What a session records for the route (`via`).
    pub fn via(self) -> &'static str {
        match self {
            Route::Gateway => "nexus",
            Route::Subscription => "subscription",
        }
    }
}

pub struct LaunchOptions<'a> {
    pub route: Route,
    pub core_url: &'a str,
    pub api_key: &'a str,
    pub session_id: &'a str,
    pub team: Option<&'a str>,
    pub model: Option<&'a str>,
    /// Codex through Gateway: its catalog without Responses Lite, when it needs one (`model_catalog_json`).
    pub model_catalog: Option<&'a Path>,
    /// Codex through Gateway: whether the organization lets a hosted search run (`search_allowed` in nexus-client).
    /// Without it Codex is told not to send its search tool, which Gateway would refuse on every request.
    pub web_search: bool,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// The model the user named for the agent, if any: `--model x` / `--model=x`, and for Codex `-m x` or `-c model=x`.
pub fn named_model(agent: Agent, args: &[String]) -> Option<String> {
    let short = local(agent).takes_short_model_flags();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" {
            break;
        }
        if let Some(inline) = arg.strip_prefix("--model=").filter(|m| !m.is_empty()) {
            return Some(inline.to_string());
        }
        if arg == "--model" || (short && arg == "-m") {
            return args.get(i + 1).cloned();
        }
        if short
            && (arg == "-c" || arg == "--config")
            && let Some(model) = args.get(i + 1).and_then(|next| config_model(next))
        {
            return Some(model);
        }
    }
    None
}

/// `model = "x"` (quotes optional) in a Codex `-c` override.
fn config_model(text: &str) -> Option<String> {
    let rest = text.strip_prefix("model")?;
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let rest = rest.strip_suffix('"').unwrap_or(rest);
    (!rest.is_empty() && !rest.contains('"')).then(|| rest.to_string())
}

/// The agent's arguments with the model it was given replaced.
pub fn replace_model(agent: Agent, args: &[String], model: &str) -> Vec<String> {
    let short = local(agent).takes_short_model_flags();
    let mut out = args.to_vec();
    for i in 0..out.len() {
        let arg = out[i].clone();
        if arg == "--" {
            break;
        }
        if arg.starts_with("--model=") {
            out[i] = format!("--model={model}");
            return out;
        }
        if arg == "--model" || (short && arg == "-m") {
            if i + 1 < out.len() {
                out[i + 1] = model.to_string();
            } else {
                out.push(model.to_string());
            }
            return out;
        }
        if short
            && (arg == "-c" || arg == "--config")
            && out.get(i + 1).is_some_and(|next| {
                next.strip_prefix("model")
                    .is_some_and(|r| r.trim_start().starts_with('='))
            })
        {
            out[i + 1] = format!("model={}", toml(model));
            return out;
        }
    }
    out
}

/// A TOML basic string, for Codex's `-c key=value` overrides.
pub(crate) fn toml(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// A path segment, percent-encoded (`encodeURIComponent`).
fn segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Where a subscription run's agent reports its usage (OpenTelemetry logs; prompts redacted).
pub(crate) fn usage_url(usage: &UsageSink) -> String {
    format!(
        "{}/v1/agents/sessions/{}/usage",
        usage.core_url,
        segment(&usage.session_id)
    )
}

/// The credential a launch on `route` runs on. Through Gateway: the session Gateway opened, with the caller's Nexus key.
/// On the plan: the agent's own login, reporting each call's usage to that session. `core_url` loses one trailing
/// slash, and an empty team is none.
pub fn credential(route: Route, core_url: &str, api_key: &str, session_id: &str, team: Option<&str>) -> Credential {
    let core_url = core_url.strip_suffix('/').unwrap_or(core_url).to_string();
    let token = Secret::new(api_key.to_string());
    match route {
        Route::Gateway => Credential::Gateway {
            core_url,
            token,
            session_id: session_id.to_string(),
            team: team.filter(|team| !team.is_empty()).map(String::from),
        },
        Route::Subscription => Credential::AgentLogin {
            account_dir: None,
            usage: Some(UsageSink {
                core_url,
                token,
                session_id: session_id.to_string(),
            }),
        },
    }
}

/// [`credential`] for `options`.
pub(crate) fn credential_of(options: &LaunchOptions) -> Credential {
    credential(
        options.route,
        options.core_url,
        options.api_key,
        options.session_id,
        options.team,
    )
}

/// What a credential puts in one agent's run: the variables it sets, the ones it removes so nothing competes with it,
/// and the arguments it adds before the agent's own. A value borrows the credential where it can (a key) and is owned
/// where the agent builds it (a URL, a header list).
pub struct CredentialEnv<'c> {
    pub set: Vec<(&'static str, Cow<'c, str>)>,
    pub remove: Vec<&'static str>,
    pub args: Vec<String>,
}

impl CredentialEnv<'_> {
    /// Removes, then sets, in `env`, and gives the arguments to put first.
    pub(crate) fn apply(self, env: &mut HashMap<String, String>) -> Vec<String> {
        for name in self.remove {
            env.remove(name);
        }
        for (name, value) in self.set {
            env.insert(name.to_string(), value.into_owned());
        }
        self.args
    }
}

/// `args` with `first` before them.
pub(crate) fn after(first: Vec<String>, args: Vec<String>) -> Vec<String> {
    let mut out = first;
    out.extend(args);
    out
}

/// Each override as `-c <override>`, before `args`.
pub(crate) fn with_overrides(overrides: Vec<String>, args: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = overrides.into_iter().flat_map(|o| ["-c".to_string(), o]).collect();
    out.extend(args);
    out
}
