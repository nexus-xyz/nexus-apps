//! Running Claude Code or Codex through Nexus: the arguments and environment that point an agent's model traffic at
//! Nexus Gateway's gateway, so every call is reserved, metered, attributed and checked by Gateway. Shared by `nexus
//! claude` / `nexus codex` (the CLI, `agents.ts` in TypeScript) and Nexus for desktop, which both open the agent
//! session, start the agent with what [`agent_launch`] gives and end the session when it exits.
//!
//! Each agent's own arguments and environment are its `LocalAgentT::launch`, in
//! `nexus-local-agents/src/agents/<agent>.rs`; this module adds what every launch shares.

use std::collections::HashMap;

use crate::agent_entries::{Agent, AgentEntry};
use nexus_local_agents::agents::local;
pub use nexus_local_agents::launch::{LaunchOptions, Route, SESSION_HEADER, TEAM_HEADER, named_model, replace_model};
pub use nexus_local_agents::{CLAUDE_NO_USAGE_ENV, CODEX_KEY_ENV, MissingBinary, find_agent, route_variable};

/// Variables the installed `nexus` wrapper sets to the Gateway it came from. They are removed from the agent's
/// environment, so what the agent runs (tests, scripts) never inherits that Gateway; a `nexus` it runs sets them again.
pub const LAUNCHER_VARS: &[&str] = &["NEXUS_URL", "NEXUS_UI_URL"];

/// Whether an organization's settings (`GET /v1/orgs/settings`) let a hosted web search run: the engine is on, and no
/// rule Gateway cannot apply to a search (zero data retention, a domain list) is set. As `webSearchOffered` in Nexus UI.
pub fn search_allowed(settings: &serde_json::Value) -> bool {
    let listed = |key: &str| {
        settings["webSearch.domains"][key]
            .as_array()
            .is_some_and(|domains| !domains.is_empty())
    };
    settings["webSearch.engine"] == "borrowed"
        && settings["webSearch.requireZdr"] != true
        && !listed("allow")
        && !listed("deny")
}

/// Whether `entry`'s agent runs on `route`: every agent runs on the person's plan, and on Gateway only with a
/// `gateway` binding. Every client checks it before [`agent_launch`] (`require_route` in `nexus-cli`, `requireRoute`
/// in agents.ts).
pub fn route_supported(entry: &AgentEntry, route: Route) -> bool {
    match route {
        Route::Gateway => entry.billing.gateway.is_some(),
        Route::Subscription => true,
    }
}

/// The agent's arguments and full environment for one run on `options.route`, which [`route_supported`] allows.
pub fn agent_launch(agent: Agent, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
    build_launch(agent, options, None)
}

/// Runs on the provider's local subscription credential without a Gateway session.
pub fn subscription_launch(
    agent: Agent,
    credential: nexus_providers::Credential,
    model: Option<&str>,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> (Vec<String>, HashMap<String, String>) {
    build_launch(
        agent,
        LaunchOptions {
            route: Route::Subscription,
            core_url: "",
            api_key: "",
            session_id: "",
            team: None,
            model,
            model_catalog: None,
            web_search: false,
            args,
            env,
        },
        Some(credential),
    )
}

fn build_launch(
    agent: Agent,
    mut options: LaunchOptions,
    credential: Option<nexus_providers::Credential>,
) -> (Vec<String>, HashMap<String, String>) {
    for name in LAUNCHER_VARS {
        options.env.remove(*name);
    }
    let local = local(agent);
    let (mut args, env) = match credential {
        Some(credential) => local.launch_with_credential(options, credential),
        None => local.launch(options),
    };
    // The commit hook reaches the agent's command shells: an agent whose shells do not inherit it gets it set there.
    if let Some(value) = env.get(crate::commit_trailer::VARIABLE)
        && let Some(set) = local.shell_variable(crate::commit_trailer::VARIABLE, value)
    {
        args.splice(0..0, set);
    }
    (args, env)
}

/// `auto`, `default`, `nexus/auto`, `nexus/default`: let Gateway choose.
pub fn is_auto_name(model: Option<&str>) -> bool {
    model
        .is_some_and(|m| ["auto", "default", "nexus/auto", "nexus/default"].contains(&m.trim().to_lowercase().as_str()))
}

/// A policy id (`anthropic/claude-fable-5.1`) as the agent names the model (`claude-fable-5-1`).
pub fn provider_model_id(agent: Agent, id: &str) -> String {
    local(agent).provider_model_id(id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn codex_carries_the_commit_hook_into_restricted_command_environments() {
        use crate::commit_trailer::{Surface, VARIABLE, mark};

        for route in [Route::Gateway, Route::Subscription] {
            for agent in [Agent::Codex, Agent::Claude] {
                for marked in [false, true] {
                    let mut env = map(&[("PATH", "/bin")]);
                    if marked {
                        // Existing git settings need the same quoting as the inline, multiline hook.
                        env.insert(VARIABLE.into(), "'user.name=Someone \"quoted\"'".into());
                        mark(&mut env, Surface::Desktop);
                    }
                    let forwarded = v(&[
                        "-c",
                        "shell_environment_policy.inherit=\"core\"",
                        "-c",
                        "shell_environment_policy.set.KEEP=\"kept\"",
                        "app-server",
                    ]);
                    let (args, launched) = agent_launch(
                        agent,
                        LaunchOptions {
                            route,
                            core_url: "https://core.example",
                            api_key: "nx_key",
                            session_id: "sess_1",
                            team: None,
                            model: None,
                            model_catalog: None,
                            web_search: false,
                            args: forwarded.clone(),
                            env: env.clone(),
                        },
                    );
                    assert!(args.ends_with(&forwarded));
                    let setting = args
                        .iter()
                        .find_map(|arg| arg.strip_prefix("shell_environment_policy.set.GIT_CONFIG_PARAMETERS="));
                    if marked && agent == Agent::Codex {
                        let decoded: String =
                            serde_json::from_str(setting.unwrap_or_default()).unwrap_or_else(|error| panic!("{error}"));
                        assert_eq!(decoded, env[VARIABLE]);
                        assert_eq!(launched[VARIABLE], decoded);
                    } else {
                        assert!(setting.is_none());
                    }
                }
            }
        }
    }

    #[test]
    fn finds_and_replaces_models() {
        assert_eq!(
            named_model(Agent::Claude, &v(&["-p", "x", "--model", "claude-opus-5-5"])).as_deref(),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            named_model(Agent::Claude, &v(&["--model=opus"])).as_deref(),
            Some("opus")
        );
        assert_eq!(named_model(Agent::Claude, &v(&["-m", "x"])), None);
        assert_eq!(
            named_model(Agent::Codex, &v(&["exec", "-m", "gpt-6-astra", "hi"])).as_deref(),
            Some("gpt-6-astra")
        );
        assert_eq!(
            named_model(Agent::Codex, &v(&["-c", "model=\"gpt-6-sol\""])).as_deref(),
            Some("gpt-6-sol")
        );
        assert_eq!(
            provider_model_id(Agent::Claude, "anthropic/claude-fable-5.1"),
            "claude-fable-5-1"
        );
        assert_eq!(provider_model_id(Agent::Codex, "openai/gpt-5.3-codex"), "gpt-5.3-codex");
        for name in ["auto", "default", "nexus/auto", "NEXUS/DEFAULT"] {
            assert!(is_auto_name(Some(name)));
        }
        assert_eq!(
            replace_model(Agent::Claude, &v(&["--model", "auto", "-p", "hi"]), "m"),
            v(&["--model", "m", "-p", "hi"])
        );
        assert_eq!(
            replace_model(Agent::Claude, &v(&["--model=auto"]), "m"),
            v(&["--model=m"])
        );
        assert_eq!(
            replace_model(Agent::Codex, &v(&["-c", "model=auto"]), "g"),
            v(&["-c", "model=\"g\""])
        );
    }

    #[test]
    fn no_agent_inherits_the_gateway_the_wrapper_set() {
        for agent in Agent::ALL {
            for route in [Route::Gateway, Route::Subscription] {
                let (_, env) = agent_launch(
                    agent,
                    LaunchOptions {
                        core_url: "https://core.example",
                        api_key: "nx_key",
                        session_id: "sess_1",
                        team: None,
                        model: None,
                        model_catalog: None,
                        web_search: false,
                        route,
                        args: Vec::new(),
                        env: map(&[
                            ("NEXUS_URL", "https://nexus.example"),
                            ("NEXUS_UI_URL", "https://app.nexus.example"),
                            ("NEXUS_TEAM", "platform"),
                        ]),
                    },
                );
                let route = route.as_str();
                assert!(!env.contains_key("NEXUS_URL"), "{agent} on {route}");
                assert!(!env.contains_key("NEXUS_UI_URL"), "{agent} on {route}");
                assert_eq!(env["NEXUS_TEAM"], "platform", "{agent} on {route}");
            }
        }
    }

    /// `CLAUDE_CODE_OAUTH_TOKEN` (a setup token, the plan's sign-in) reaches Claude Code on the subscription route and is
    /// removed on the Gateway route, where it would compete with `ANTHROPIC_AUTH_TOKEN`.
    #[test]
    fn the_plan_token_survives_only_on_the_subscription_route() {
        let launch = |route| {
            agent_launch(
                Agent::Claude,
                LaunchOptions {
                    core_url: "https://core.example",
                    api_key: "nx_key",
                    session_id: "sess_1",
                    team: None,
                    model: None,
                    model_catalog: None,
                    web_search: false,
                    route,
                    args: Vec::new(),
                    env: map(&[
                        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-x"),
                        ("ANTHROPIC_API_KEY", "sk-ant-own"),
                        ("GH_TOKEN", "gh"),
                    ]),
                },
            )
            .1
        };
        let plan = launch(Route::Subscription);
        assert_eq!(
            plan.get("CLAUDE_CODE_OAUTH_TOKEN").map(String::as_str),
            Some("sk-ant-oat01-x")
        );
        assert!(!plan.contains_key("ANTHROPIC_API_KEY"));
        let gateway = launch(Route::Gateway);
        assert!(!gateway.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(gateway["ANTHROPIC_AUTH_TOKEN"], "nx_key");
        assert_eq!(gateway["GH_TOKEN"], "gh");
    }

    #[test]
    fn codex_searches_through_gateway_only_when_the_organization_lets_it() {
        let launch = |web_search: bool| {
            agent_launch(
                Agent::Codex,
                LaunchOptions {
                    core_url: "https://core.example",
                    api_key: "nx_key",
                    session_id: "sess_1",
                    team: None,
                    model: None,
                    model_catalog: None,
                    web_search,
                    route: Route::Gateway,
                    args: v(&["exec", "hi"]),
                    env: HashMap::new(),
                },
            )
            .0
        };
        let disabled = "web_search=\"disabled\"".to_string();
        assert!(launch(false).contains(&disabled));
        assert!(!launch(true).iter().any(|arg| arg.starts_with("web_search=")));

        let on = serde_json::json!({ "webSearch.engine": "borrowed", "webSearch.requireZdr": false,
            "webSearch.domains": { "allow": [], "deny": [] } });
        assert!(search_allowed(&on));
        for (key, value) in [
            ("webSearch.engine", serde_json::json!("off")),
            ("webSearch.requireZdr", serde_json::json!(true)),
            (
                "webSearch.domains",
                serde_json::json!({ "allow": ["docs.example"], "deny": [] }),
            ),
            (
                "webSearch.domains",
                serde_json::json!({ "allow": [], "deny": ["spam.example"] }),
            ),
        ] {
            let mut settings = on.clone();
            settings[key] = value;
            assert!(!search_allowed(&settings), "{key}");
        }
        assert!(
            !search_allowed(&serde_json::json!({})),
            "settings it cannot read keep search off"
        );
    }

    #[test]
    fn points_the_agents_at_core() {
        let (args, env) = agent_launch(
            Agent::Claude,
            LaunchOptions {
                core_url: "https://core.example/",
                api_key: "nx_key",
                session_id: "sess_1",
                team: None,
                model: Some("claude-haiku-5-5"),
                model_catalog: None,
                web_search: false,
                route: Route::Gateway,
                args: v(&["-p", "hi"]),
                env: map(&[
                    ("PATH", "/bin"),
                    ("ANTHROPIC_API_KEY", "sk-ant-own"),
                    ("CLAUDE_CODE_USE_VERTEX", "1"),
                    ("ANTHROPIC_AUTH_TOKEN", "old"),
                ]),
            },
        );
        assert_eq!(args, v(&["-p", "hi"]));
        assert_eq!(
            env,
            map(&[
                ("PATH", "/bin"),
                ("ANTHROPIC_BASE_URL", "https://core.example/v1/anthropic"),
                ("ANTHROPIC_AUTH_TOKEN", "nx_key"),
                ("ANTHROPIC_CUSTOM_HEADERS", "x-nexus-session: sess_1"),
                ("CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK", "1"),
                ("ANTHROPIC_MODEL", "claude-haiku-5-5"),
            ])
        );

        let (_, env) = agent_launch(
            Agent::Claude,
            LaunchOptions {
                core_url: "https://core.example",
                api_key: "nx_key",
                session_id: "sess_1",
                team: Some("web"),
                model: None,
                model_catalog: None,
                web_search: false,
                route: Route::Gateway,
                args: vec![],
                env: HashMap::new(),
            },
        );
        assert_eq!(
            env.get("ANTHROPIC_CUSTOM_HEADERS").map(String::as_str),
            Some("x-nexus-session: sess_1\nx-nexus-team: web")
        );

        let (args, env) = agent_launch(
            Agent::Codex,
            LaunchOptions {
                core_url: "https://core.example",
                api_key: "nx_key",
                session_id: "sess_1",
                team: None,
                model: None,
                model_catalog: None,
                web_search: false,
                route: Route::Gateway,
                args: v(&["exec", "hi"]),
                env: map(&[("OPENAI_API_KEY", "sk-own")]),
            },
        );
        assert_eq!(env, map(&[("NEXUS_API_KEY", "nx_key")]));
        assert_eq!(
            args,
            v(&[
                "-c",
                "model_provider=\"nexus\"",
                "-c",
                "model_providers.nexus.name=\"Nexus\"",
                "-c",
                "model_providers.nexus.base_url=\"https://core.example/v1/openai/v1\"",
                "-c",
                "model_providers.nexus.env_key=\"NEXUS_API_KEY\"",
                "-c",
                "model_providers.nexus.wire_api=\"responses\"",
                "-c",
                "model_providers.nexus.http_headers={ \"x-nexus-session\" = \"sess_1\" }",
                "-c",
                "web_search=\"disabled\"",
                "exec",
                "hi",
            ])
        );

        let (args, _) = agent_launch(
            Agent::Codex,
            LaunchOptions {
                core_url: "u",
                api_key: "k",
                session_id: "s",
                team: None,
                model: Some("gpt-6-sol"),
                model_catalog: Some(Path::new("/h/.nexus/factory/codex-models-1.json")),
                web_search: false,
                route: Route::Gateway,
                args: vec![],
                env: HashMap::new(),
            },
        );
        assert_eq!(
            args[args.len() - 4..],
            v(&[
                "-c",
                "model_catalog_json=\"/h/.nexus/factory/codex-models-1.json\"",
                "-c",
                "model=\"gpt-6-sol\"",
            ])
        );
    }

    /// The same environment as `agentLaunch` in interfaces/src/agents.ts on the subscription route.
    #[test]
    fn subscription_keeps_claudes_own_login_and_reports_usage() {
        let (args, env) = agent_launch(
            Agent::Claude,
            LaunchOptions {
                core_url: "https://core.example/",
                api_key: "nx_key",
                session_id: "sess 1",
                team: Some("web"),
                model: Some("claude-opus-5-5"),
                model_catalog: None,
                web_search: false,
                route: Route::Subscription,
                args: v(&["-p", "hi"]),
                env: map(&[
                    ("PATH", "/bin"),
                    ("ANTHROPIC_API_KEY", "sk-ant-own"),
                    ("ANTHROPIC_BASE_URL", "https://elsewhere"),
                    ("ANTHROPIC_AUTH_TOKEN", "old"),
                    ("ANTHROPIC_CUSTOM_HEADERS", "x: y"),
                    ("CLAUDE_CODE_USE_BEDROCK", "1"),
                    ("CLAUDE_CODE_USE_VERTEX", "1"),
                    ("CLAUDE_CODE_USE_FOUNDRY", "1"),
                    ("OTEL_LOG_USER_PROMPTS", "1"),
                    ("OTEL_LOG_TOOL_DETAILS", "1"),
                ]),
            },
        );
        assert_eq!(args, v(&["-p", "hi"]));
        assert_eq!(
            env,
            map(&[
                ("PATH", "/bin"),
                ("ANTHROPIC_MODEL", "claude-opus-5-5"),
                ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
                ("OTEL_LOGS_EXPORTER", "otlp"),
                ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json"),
                (
                    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                    "https://core.example/v1/agents/sessions/sess%201/usage"
                ),
                ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "Authorization=Bearer nx_key"),
            ])
        );
    }

    /// Same as `agentLaunch` in interfaces/src/agents.ts: the opt-out leaves telemetry off, so Code Mode can run.
    #[test]
    fn subscription_reports_no_usage_when_the_person_opts_out() {
        let (_, env) = agent_launch(
            Agent::Claude,
            LaunchOptions {
                core_url: "https://core.example/",
                api_key: "nx_key",
                session_id: "sess 1",
                team: None,
                model: None,
                model_catalog: None,
                web_search: false,
                route: Route::Subscription,
                args: v(&[]),
                env: map(&[
                    ("PATH", "/bin"),
                    (CLAUDE_NO_USAGE_ENV, "1"),
                    ("OTEL_LOG_USER_PROMPTS", "1"),
                ]),
            },
        );
        assert_eq!(env, map(&[("PATH", "/bin"), (CLAUDE_NO_USAGE_ENV, "1")]));
    }

    #[test]
    fn subscription_keeps_codex_on_its_chatgpt_login_with_usage_to_the_session_and_the_key_out_of_argv() {
        let (args, env) = agent_launch(
            Agent::Codex,
            LaunchOptions {
                core_url: "https://core.example/",
                api_key: "nx_key",
                session_id: "sess_1",
                team: Some("web"),
                model: Some("gpt-5.3-codex"),
                model_catalog: Some(Path::new("/h/codex-models-1.json")),
                web_search: false,
                route: Route::Subscription,
                args: v(&["exec", "hi"]),
                env: map(&[("PATH", "/bin"), ("OPENAI_API_KEY", "sk-own")]),
            },
        );
        assert_eq!(
            env,
            map(&[
                ("PATH", "/bin"),
                ("NEXUS_API_KEY", "nx_key"),
                ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "authorization=Bearer nx_key"),
            ])
        );
        assert_eq!(
            args,
            v(&[
                "-c",
                "forced_login_method=\"chatgpt\"",
                "-c",
                "otel.log_user_prompt=false",
                "-c",
                "otel.exporter={ otlp-http = { endpoint = \"https://core.example/v1/agents/sessions/sess_1/usage\", protocol = \"json\" } }",
                "-c",
                "model=\"gpt-5.3-codex\"",
                "exec",
                "hi",
            ])
        );
        // Codex keeps its own provider on the plan: no Gateway provider, team header or catalog, and no key in argv.
        let argv = args.join(" ");
        assert!(!argv.contains("nx_key") && !argv.contains("model_provider") && !argv.contains("model_catalog_json"));
    }

    /// Antigravity runs on the person's Google sign-in: every variable that would bill an API key, Vertex or ADC, or
    /// move its endpoint, is removed on either route, nothing points it at Gateway, and its arguments are kept.
    #[test]
    fn antigravity_runs_without_what_would_bill_an_api_key() {
        let billing = Agent::Antigravity.entry().billing.plan_overrides;
        assert_eq!(billing.len(), 7);
        let mut env = map(&[("PATH", "/bin"), ("OPENAI_API_KEY", "sk-own")]);
        for name in billing {
            env.insert(name.to_string(), "set".into());
        }
        for route in Route::ALL {
            let (args, launched) = agent_launch(
                Agent::Antigravity,
                LaunchOptions {
                    route,
                    core_url: "https://core.example",
                    api_key: "nx_key",
                    session_id: "sess_1",
                    team: Some("web"),
                    model: None,
                    model_catalog: None,
                    web_search: false,
                    args: v(&["--conversation", "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d"]),
                    env: env.clone(),
                },
            );
            assert_eq!(args, v(&["--conversation", "0b9c8a1e-6f4d-4b2a-9c3e-1d2f3a4b5c6d"]));
            assert_eq!(launched, map(&[("PATH", "/bin"), ("OPENAI_API_KEY", "sk-own")]));
        }
    }

    #[test]
    fn antigravity_takes_a_model_as_its_own_flag() {
        let launch = |model: Option<&str>, args: &[&str]| {
            agent_launch(
                Agent::Antigravity,
                LaunchOptions {
                    route: Route::Subscription,
                    core_url: "u",
                    api_key: "k",
                    session_id: "s",
                    team: None,
                    model,
                    model_catalog: None,
                    web_search: false,
                    args: v(args),
                    env: HashMap::new(),
                },
            )
            .0
        };
        assert_eq!(
            launch(Some("gemini-3.8-flash-high"), &["-p", "hi"]),
            v(&["--model", "gemini-3.8-flash-high", "-p", "hi"])
        );
        // The person's own flag wins, and is not given twice.
        assert_eq!(
            launch(Some("gemini-3.8-flash-high"), &["--model", "claude-sonnet-4-6"]),
            v(&["--model", "claude-sonnet-4-6"])
        );
        assert_eq!(
            named_model(Agent::Antigravity, &v(&["--model=claude-sonnet-4-6"])).as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(named_model(Agent::Antigravity, &v(&["-m", "x"])), None);
        assert!(!route_supported(Agent::Antigravity.entry(), Route::Gateway));
        assert!(route_supported(Agent::Antigravity.entry(), Route::Subscription));
    }

    #[test]
    fn routes_parse_by_name_and_record_their_via() {
        assert_eq!(Route::parse("subscription"), Some(Route::Subscription));
        assert_eq!(Route::parse("Gateway"), None);
        assert_eq!(Route::ALL.map(Route::via), ["nexus", "subscription"]);
        assert_eq!(route_variable(Agent::Codex), "NEXUS_CODEX_AUTH");
    }
}
