//! Golden tests of `agent_launch`: Claude Code and Codex on each route, and Antigravity on its plan. Each
//! case gives fixed inputs and states the whole result: every argument in order and every variable of the environment.
//! The per-agent driver moves must leave these tests passing unchanged.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use nexus_client::agent_entries::Agent;
use nexus_client::agents::{LaunchOptions, Route, agent_launch};

/// What a person's shell might hold: the wrapper's Gateway, every variable an agent's launch removes or replaces, the
/// agents' profile folders, model and routes, and a git setting that needs quoting (a quote and a line break).
fn shell() -> HashMap<String, String> {
    [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/ada"),
        ("NEXUS_URL", "https://nexus.example"),
        ("NEXUS_UI_URL", "https://app.nexus.example"),
        ("NEXUS_TEAM", "platform"),
        ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
        ("CODEX_HOME", "/home/ada/.codex-work"),
        ("ANTHROPIC_MODEL", "claude-own-model"),
        ("NEXUS_CLAUDE_AUTH", "subscription"),
        ("NEXUS_CODEX_AUTH", "gateway"),
        ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
        ("ANTHROPIC_API_KEY", "sk-ant-own"),
        ("ANTHROPIC_AUTH_TOKEN", "own-token"),
        ("ANTHROPIC_BASE_URL", "https://proxy.example"),
        ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("CLAUDE_CODE_USE_VERTEX", "1"),
        ("CLAUDE_CODE_USE_FOUNDRY", "1"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
        ("OPENAI_API_KEY", "sk-own"),
        ("NEXUS_API_KEY", "nx_stale"),
        ("GEMINI_API_KEY", "gm-own"),
        ("GOOGLE_API_KEY", "go-own"),
        ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
        ("GOOGLE_CLOUD_PROJECT", "ada-project"),
        ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
        ("AGY_ADC_AUTH", "1"),
        ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
        ("OTEL_LOG_USER_PROMPTS", "1"),
        ("OTEL_LOG_TOOL_DETAILS", "1"),
        ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://otel.example"),
        ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "x-own=1"),
        (
            "GIT_CONFIG_PARAMETERS",
            "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'",
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn vars(items: &[(&str, &str)]) -> BTreeMap<String, String> {
    items
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// One run with the fixed inputs: a core URL with a trailing slash, a session id that needs percent-encoding, a team,
/// a model and a Codex catalog, with the organization's search off.
fn golden(agent: Agent, route: Route, model: &str, args: &[&str]) -> (Vec<String>, BTreeMap<String, String>) {
    golden_with_search(agent, route, model, args, false)
}

/// [`golden`] with the organization's search as `web_search` says.
fn golden_with_search(
    agent: Agent,
    route: Route,
    model: &str,
    args: &[&str],
    web_search: bool,
) -> (Vec<String>, BTreeMap<String, String>) {
    let (args, env) = agent_launch(
        agent,
        LaunchOptions {
            route,
            core_url: "https://core.example/",
            api_key: "nx_golden",
            session_id: "ags_01 x",
            team: Some("web"),
            model: Some(model),
            model_catalog: Some(Path::new("/home/ada/.nexus/codex-models-0123456789abcdef.json")),
            web_search,
            args: strings(args),
            env: shell(),
        },
    );
    (args, env.into_iter().collect())
}

/// One run with the least input: no team, an empty model, no catalog and no environment.
fn least(agent: Agent, route: Route, env: &[(&str, &str)]) -> (Vec<String>, BTreeMap<String, String>) {
    let (args, env) = agent_launch(
        agent,
        LaunchOptions {
            route,
            core_url: "https://core.example",
            api_key: "nx_golden",
            session_id: "s",
            team: Some(""),
            model: Some(""),
            model_catalog: None,
            web_search: false,
            args: Vec::new(),
            env: env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        },
    );
    (args, env.into_iter().collect())
}

#[test]
fn claude_code_through_gateway() {
    let (args, env) = golden(Agent::Claude, Route::Gateway, "claude-opus-5-5", &["-p", "hi"]);
    assert_eq!(args, strings(&["-p", "hi"]));
    assert_eq!(
        env,
        vars(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/ada"),
            ("NEXUS_TEAM", "platform"),
            ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
            ("CODEX_HOME", "/home/ada/.codex-work"),
            ("ANTHROPIC_MODEL", "claude-own-model"),
            ("NEXUS_CLAUDE_AUTH", "subscription"),
            ("NEXUS_CODEX_AUTH", "gateway"),
            ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
            ("ANTHROPIC_AUTH_TOKEN", "nx_golden"),
            ("ANTHROPIC_BASE_URL", "https://core.example/v1/anthropic"),
            (
                "ANTHROPIC_CUSTOM_HEADERS",
                "x-nexus-session: ags_01 x\nx-nexus-team: web"
            ),
            ("CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK", "1"),
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
            ("OPENAI_API_KEY", "sk-own"),
            ("NEXUS_API_KEY", "nx_stale"),
            ("GEMINI_API_KEY", "gm-own"),
            ("GOOGLE_API_KEY", "go-own"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
            ("GOOGLE_CLOUD_PROJECT", "ada-project"),
            ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
            ("AGY_ADC_AUTH", "1"),
            ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
            ("OTEL_LOG_USER_PROMPTS", "1"),
            ("OTEL_LOG_TOOL_DETAILS", "1"),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://otel.example"),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "x-own=1"),
            (
                "GIT_CONFIG_PARAMETERS",
                "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'"
            ),
        ])
    );
}

#[test]
fn claude_code_on_its_plan() {
    let (args, env) = golden(Agent::Claude, Route::Subscription, "claude-opus-5-5", &["-p", "hi"]);
    assert_eq!(args, strings(&["-p", "hi"]));
    assert_eq!(
        env,
        vars(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/ada"),
            ("NEXUS_TEAM", "platform"),
            ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
            ("CODEX_HOME", "/home/ada/.codex-work"),
            ("ANTHROPIC_MODEL", "claude-own-model"),
            ("NEXUS_CLAUDE_AUTH", "subscription"),
            ("NEXUS_CODEX_AUTH", "gateway"),
            ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
            ("OPENAI_API_KEY", "sk-own"),
            ("NEXUS_API_KEY", "nx_stale"),
            ("GEMINI_API_KEY", "gm-own"),
            ("GOOGLE_API_KEY", "go-own"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
            ("GOOGLE_CLOUD_PROJECT", "ada-project"),
            ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
            ("AGY_ADC_AUTH", "1"),
            ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
            ("OTEL_LOGS_EXPORTER", "otlp"),
            ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json"),
            (
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                "https://core.example/v1/agents/sessions/ags_01%20x/usage"
            ),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "Authorization=Bearer nx_golden"),
            (
                "GIT_CONFIG_PARAMETERS",
                "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'"
            ),
        ])
    );
}

#[test]
fn codex_through_gateway() {
    let (args, env) = golden(Agent::Codex, Route::Gateway, "gpt-6-sol", &["exec", "hi"]);
    // The git setting comes first, before Nexus's provider; the person's own arguments come last.
    assert_eq!(
        args,
        strings(&[
            "-c",
            r#"shell_environment_policy.set.GIT_CONFIG_PARAMETERS="'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'""#,
            "-c",
            r#"model_provider="nexus""#,
            "-c",
            r#"model_providers.nexus.name="Nexus""#,
            "-c",
            r#"model_providers.nexus.base_url="https://core.example/v1/openai/v1""#,
            "-c",
            r#"model_providers.nexus.env_key="NEXUS_API_KEY""#,
            "-c",
            r#"model_providers.nexus.wire_api="responses""#,
            "-c",
            r#"model_providers.nexus.http_headers={ "x-nexus-session" = "ags_01 x", "x-nexus-team" = "web" }"#,
            "-c",
            "features.shell_snapshot=false",
            "-c",
            r#"web_search="disabled""#,
            "-c",
            r#"model_catalog_json="/home/ada/.nexus/codex-models-0123456789abcdef.json""#,
            "-c",
            r#"model="gpt-6-sol""#,
            "exec",
            "hi",
        ])
    );
    assert_eq!(
        env,
        vars(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/ada"),
            ("NEXUS_TEAM", "platform"),
            ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
            ("CODEX_HOME", "/home/ada/.codex-work"),
            ("ANTHROPIC_MODEL", "claude-own-model"),
            ("NEXUS_CLAUDE_AUTH", "subscription"),
            ("NEXUS_CODEX_AUTH", "gateway"),
            ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
            ("ANTHROPIC_API_KEY", "sk-ant-own"),
            ("ANTHROPIC_AUTH_TOKEN", "own-token"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example"),
            ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
            ("CLAUDE_CODE_USE_FOUNDRY", "1"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
            ("NEXUS_API_KEY", "nx_golden"),
            ("GEMINI_API_KEY", "gm-own"),
            ("GOOGLE_API_KEY", "go-own"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
            ("GOOGLE_CLOUD_PROJECT", "ada-project"),
            ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
            ("AGY_ADC_AUTH", "1"),
            ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
            ("OTEL_LOG_USER_PROMPTS", "1"),
            ("OTEL_LOG_TOOL_DETAILS", "1"),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://otel.example"),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "x-own=1"),
            (
                "GIT_CONFIG_PARAMETERS",
                "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'"
            ),
        ])
    );
}

#[test]
fn codex_on_its_plan() {
    let (args, env) = golden(Agent::Codex, Route::Subscription, "gpt-6-sol", &["exec", "hi"]);
    // No team header and no catalog on the plan: Codex keeps its own provider.
    assert_eq!(
        args,
        strings(&[
            "-c",
            r#"shell_environment_policy.set.GIT_CONFIG_PARAMETERS="'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'""#,
            "-c",
            r#"forced_login_method="chatgpt""#,
            "-c",
            "otel.log_user_prompt=false",
            "-c",
            r#"otel.exporter={ otlp-http = { endpoint = "https://core.example/v1/agents/sessions/ags_01%20x/usage", protocol = "json" } }"#,
            "-c",
            "features.shell_snapshot=false",
            "-c",
            r#"model="gpt-6-sol""#,
            "exec",
            "hi",
        ])
    );
    assert_eq!(
        env,
        vars(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/ada"),
            ("NEXUS_TEAM", "platform"),
            ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
            ("CODEX_HOME", "/home/ada/.codex-work"),
            ("ANTHROPIC_MODEL", "claude-own-model"),
            ("NEXUS_CLAUDE_AUTH", "subscription"),
            ("NEXUS_CODEX_AUTH", "gateway"),
            ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
            ("ANTHROPIC_API_KEY", "sk-ant-own"),
            ("ANTHROPIC_AUTH_TOKEN", "own-token"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example"),
            ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
            ("CLAUDE_CODE_USE_FOUNDRY", "1"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
            ("NEXUS_API_KEY", "nx_golden"),
            ("GEMINI_API_KEY", "gm-own"),
            ("GOOGLE_API_KEY", "go-own"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/home/ada/adc.json"),
            ("GOOGLE_CLOUD_PROJECT", "ada-project"),
            ("CLOUDSDK_CORE_PROJECT", "ada-sdk-project"),
            ("AGY_ADC_AUTH", "1"),
            ("GOOGLE_GEMINI_BASE_URL", "https://gemini.example"),
            ("OTEL_LOG_USER_PROMPTS", "1"),
            ("OTEL_LOG_TOOL_DETAILS", "1"),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://otel.example"),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "authorization=Bearer nx_golden"),
            (
                "GIT_CONFIG_PARAMETERS",
                "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'"
            ),
        ])
    );
}

/// An empty team or model is none, and a core URL without a trailing slash is used as it is.
#[test]
fn least_input_on_each_route() {
    assert_eq!(
        least(Agent::Claude, Route::Gateway, &[]),
        (
            Vec::new(),
            vars(&[
                ("ANTHROPIC_AUTH_TOKEN", "nx_golden"),
                ("ANTHROPIC_BASE_URL", "https://core.example/v1/anthropic"),
                ("ANTHROPIC_CUSTOM_HEADERS", "x-nexus-session: s"),
                ("CLAUDE_CODE_SKIP_FAST_MODE_ORG_CHECK", "1"),
            ])
        )
    );
    assert_eq!(
        least(Agent::Claude, Route::Subscription, &[]),
        (
            Vec::new(),
            vars(&[
                ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
                ("OTEL_LOGS_EXPORTER", "otlp"),
                ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json"),
                (
                    "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                    "https://core.example/v1/agents/sessions/s/usage"
                ),
                ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "Authorization=Bearer nx_golden"),
            ])
        )
    );
    assert_eq!(
        least(Agent::Codex, Route::Gateway, &[]),
        (
            strings(&[
                "-c",
                r#"model_provider="nexus""#,
                "-c",
                r#"model_providers.nexus.name="Nexus""#,
                "-c",
                r#"model_providers.nexus.base_url="https://core.example/v1/openai/v1""#,
                "-c",
                r#"model_providers.nexus.env_key="NEXUS_API_KEY""#,
                "-c",
                r#"model_providers.nexus.wire_api="responses""#,
                "-c",
                r#"model_providers.nexus.http_headers={ "x-nexus-session" = "s" }"#,
                "-c",
                "features.shell_snapshot=false",
                "-c",
                r#"web_search="disabled""#,
            ]),
            vars(&[("NEXUS_API_KEY", "nx_golden")])
        )
    );
    assert_eq!(
        least(Agent::Codex, Route::Subscription, &[]),
        (
            strings(&[
                "-c",
                r#"forced_login_method="chatgpt""#,
                "-c",
                "otel.log_user_prompt=false",
                "-c",
                r#"otel.exporter={ otlp-http = { endpoint = "https://core.example/v1/agents/sessions/s/usage", protocol = "json" } }"#,
                "-c",
                "features.shell_snapshot=false",
            ]),
            vars(&[
                ("NEXUS_API_KEY", "nx_golden"),
                ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "authorization=Bearer nx_golden"),
            ])
        )
    );
}

/// The person's own `ANTHROPIC_MODEL` wins over the model Nexus chose on both routes; an empty one does not.
#[test]
fn claude_codes_model_variable() {
    for route in Route::ALL {
        let model = |own: &str| {
            let (_, env) = agent_launch(
                Agent::Claude,
                LaunchOptions {
                    route,
                    core_url: "u",
                    api_key: "k",
                    session_id: "s",
                    team: None,
                    model: Some("claude-opus-5-5"),
                    model_catalog: None,
                    web_search: false,
                    args: Vec::new(),
                    env: HashMap::from([("ANTHROPIC_MODEL".to_string(), own.to_string())]),
                },
            );
            env["ANTHROPIC_MODEL"].clone()
        };
        assert_eq!(model("mine"), "mine", "{route:?}");
        assert_eq!(model(""), "claude-opus-5-5", "{route:?}");
    }
}

/// An empty git setting is still carried, and Claude Code and Antigravity never get the setting as an argument.
#[test]
fn codex_carries_an_empty_git_setting() {
    for route in Route::ALL {
        let (args, env) = least(Agent::Codex, route, &[("GIT_CONFIG_PARAMETERS", "")]);
        assert_eq!(
            args[..2],
            strings(&["-c", r#"shell_environment_policy.set.GIT_CONFIG_PARAMETERS="""#]),
            "{route:?}"
        );
        assert_eq!(env["GIT_CONFIG_PARAMETERS"], "", "{route:?}");
        for agent in [Agent::Claude, Agent::Antigravity] {
            let (args, env) = least(agent, route, &[("GIT_CONFIG_PARAMETERS", "")]);
            assert!(args.is_empty(), "{agent} {route:?}");
            assert_eq!(env["GIT_CONFIG_PARAMETERS"], "", "{agent} {route:?}");
        }
    }
}

/// With the organization's search on, Codex through Gateway is not told to turn its search off. No other launch reads
/// the switch.
#[test]
fn codex_searches_through_gateway_when_the_organization_lets_it() {
    let (args, env) = golden_with_search(Agent::Codex, Route::Gateway, "gpt-6-sol", &["exec", "hi"], true);
    assert_eq!(
        args,
        strings(&[
            "-c",
            r#"shell_environment_policy.set.GIT_CONFIG_PARAMETERS="'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'""#,
            "-c",
            r#"model_provider="nexus""#,
            "-c",
            r#"model_providers.nexus.name="Nexus""#,
            "-c",
            r#"model_providers.nexus.base_url="https://core.example/v1/openai/v1""#,
            "-c",
            r#"model_providers.nexus.env_key="NEXUS_API_KEY""#,
            "-c",
            r#"model_providers.nexus.wire_api="responses""#,
            "-c",
            r#"model_providers.nexus.http_headers={ "x-nexus-session" = "ags_01 x", "x-nexus-team" = "web" }"#,
            "-c",
            "features.shell_snapshot=false",
            "-c",
            r#"model_catalog_json="/home/ada/.nexus/codex-models-0123456789abcdef.json""#,
            "-c",
            r#"model="gpt-6-sol""#,
            "exec",
            "hi",
        ])
    );
    assert_eq!(
        env,
        golden(Agent::Codex, Route::Gateway, "gpt-6-sol", &["exec", "hi"]).1
    );
    for (agent, route, model) in [
        (Agent::Codex, Route::Subscription, "gpt-6-sol"),
        (Agent::Claude, Route::Gateway, "claude-opus-5-5"),
        (Agent::Claude, Route::Subscription, "claude-opus-5-5"),
        (Agent::Antigravity, Route::Subscription, "gemini-3.8-flash-high"),
    ] {
        assert_eq!(
            golden_with_search(agent, route, model, &["-p", "hi"], true),
            golden(agent, route, model, &["-p", "hi"]),
            "{agent} {route:?}"
        );
    }
}

/// Antigravity on its plan: the model first as `--model`, then the person's arguments. Its environment loses only what
/// would bill an API key or a cloud project, and the wrapper's Gateway. Nexus adds no key, no usage export, no team
/// header and no git setting as an argument; the other agents' variables stay as the person set them.
#[test]
fn antigravity_on_its_plan() {
    let (args, env) = golden(
        Agent::Antigravity,
        Route::Subscription,
        "gemini-3.8-flash-high",
        &["-p", "hi"],
    );
    assert_eq!(args, strings(&["--model", "gemini-3.8-flash-high", "-p", "hi"]));
    assert_eq!(
        env,
        vars(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/ada"),
            ("NEXUS_TEAM", "platform"),
            ("CLAUDE_CONFIG_DIR", "/home/ada/.claude-work"),
            ("CODEX_HOME", "/home/ada/.codex-work"),
            ("ANTHROPIC_MODEL", "claude-own-model"),
            ("NEXUS_CLAUDE_AUTH", "subscription"),
            ("NEXUS_CODEX_AUTH", "gateway"),
            ("NEXUS_ANTIGRAVITY_AUTH", "subscription"),
            ("ANTHROPIC_API_KEY", "sk-ant-own"),
            ("ANTHROPIC_AUTH_TOKEN", "own-token"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example"),
            ("ANTHROPIC_CUSTOM_HEADERS", "x-own: 1"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
            ("CLAUDE_CODE_USE_FOUNDRY", "1"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-own"),
            ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
            ("OPENAI_API_KEY", "sk-own"),
            ("NEXUS_API_KEY", "nx_stale"),
            ("OTEL_LOG_USER_PROMPTS", "1"),
            ("OTEL_LOG_TOOL_DETAILS", "1"),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://otel.example"),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "x-own=1"),
            (
                "GIT_CONFIG_PARAMETERS",
                "'user.name=Ada \"A\"' 'hook.x.command=one\ntwo'"
            ),
        ])
    );
    // It has no Gateway route: a caller that asks for one still gets the plan's launch.
    assert_eq!(
        golden(
            Agent::Antigravity,
            Route::Gateway,
            "gemini-3.8-flash-high",
            &["-p", "hi"]
        ),
        (args, env)
    );
    for route in Route::ALL {
        assert_eq!(
            least(Agent::Antigravity, route, &[]),
            (Vec::new(), BTreeMap::new()),
            "{route:?}"
        );
    }
}

/// Antigravity's model argument: a model the person named wins (`--model x`, `--model=x`). `-m` is not agy's, and a
/// `--model` after `--` or with nothing after it names none, so the model still goes first.
#[test]
fn antigravity_model_argument() {
    let launch = |args: &[&str], model: &str| golden(Agent::Antigravity, Route::Subscription, model, args).0;
    let model = "gemini-3.8-flash-high";
    assert_eq!(
        launch(&["--model", "mine", "-p", "hi"], model),
        strings(&["--model", "mine", "-p", "hi"])
    );
    assert_eq!(launch(&["--model=mine"], model), strings(&["--model=mine"]));
    assert_eq!(
        launch(&["-m", "mine"], model),
        strings(&["--model", model, "-m", "mine"])
    );
    assert_eq!(
        launch(&["--", "--model", "mine"], model),
        strings(&["--model", model, "--", "--model", "mine"])
    );
    assert_eq!(launch(&["--model"], model), strings(&["--model", model, "--model"]));
    assert_eq!(launch(&["-p", "hi"], ""), strings(&["-p", "hi"]));
}

#[test]
fn opencode_uses_acp_and_preserves_the_configured_provider_environment() {
    let args = vec!["acp".into()];
    let env = HashMap::from([
        ("OPENCODE_CONFIG_DIR".into(), "/work/profile".into()),
        ("OPENAI_API_KEY".into(), "provider-key".into()),
    ]);
    let launch = agent_launch(
        Agent::Opencode,
        LaunchOptions {
            route: Route::Subscription,
            core_url: "https://gateway.invalid",
            api_key: "fake-gateway-key",
            session_id: "fake-session",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: args.clone(),
            env: env.clone(),
        },
    );
    assert_eq!(launch, (args, env));
}
