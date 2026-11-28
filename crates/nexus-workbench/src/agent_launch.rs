//! Shared coding-agent lookup and Gateway model selection for desktop hosts.

use crate::driver::label;
use crate::event::{SessionAgent, SessionKind, Via};
use crate::provider::{
    CliSubscriptionProvider, LaunchContext, LaunchRoute, LocalProviders, LocalProvidersOptIn, OfferContext, Prepared,
    Provider,
};
use crate::session_scope::SessionScope;
use nexus_client::{Client, agents, auto_route};
use nexus_providers::{CatalogModelId, ModelOffer, Room};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// List prices per million tokens: (input, output), by the names the agents use.
pub type Prices = HashMap<String, (f64, f64)>;

/// Normalizes a model id to the key used in [`Prices`].
pub fn model_key(id: &str) -> String {
    id.strip_prefix("anthropic/")
        .unwrap_or(id)
        .replace('.', "-")
        .to_lowercase()
}

/// Gateway's priced models, read from one `GET /v1/models`: each one's list prices and its catalog id, under the
/// [`model_key`] of its catalog id and of the id its agent runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub prices: Prices,
    pub ids: HashMap<String, CatalogModelId>,
}

impl Catalog {
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    /// The catalog id of `model` (either id), when Gateway prices it.
    pub fn id(&self, model: &str) -> Option<CatalogModelId> {
        self.ids.get(&model_key(model)).cloned()
    }
}

/// Loads the catalog's priced models from Gateway. A model with no list price is left out, as Auto does not offer it.
pub async fn load_catalog(client: &Client) -> Catalog {
    match client.get("/v1/models?status=all").await {
        Ok(models) => catalog_of(&models),
        Err(_) => Catalog::default(),
    }
}

/// The [`Catalog`] of Gateway's model list (`{ "data": [...] }`).
pub fn catalog_of(models: &Value) -> Catalog {
    let mut catalog = Catalog::default();
    for model in models.get("data").and_then(Value::as_array).into_iter().flatten() {
        let (Some(input), Some(output)) = (
            model.pointer("/pricing/input").and_then(Value::as_f64),
            model.pointer("/pricing/output").and_then(Value::as_f64),
        ) else {
            continue;
        };
        // Gateway names every row by its catalog id. A priced row without one keeps its provider id, so it stays known
        // to Auto as its price makes it.
        let id = ["id", "provider_model"].into_iter().find_map(|name| {
            model
                .get(name)
                .and_then(Value::as_str)
                .and_then(CatalogModelId::from_gateway)
        });
        for name in ["id", "provider_model"] {
            if let Some(key) = model.get(name).and_then(Value::as_str).map(model_key) {
                catalog.prices.insert(key.clone(), (input, output));
                if let Some(id) = &id {
                    catalog.ids.insert(key, id.clone());
                }
            }
        }
    }
    catalog
}

/// Host-supplied state for Auto launch routing (`POST /v1/agents/route`).
#[derive(Default, Clone)]
pub struct AutoContext<'a> {
    /// Known catalog prices and ids; loaded and cached on demand when empty.
    pub prices: Option<&'a RwLock<Catalog>>,
    /// Already-discovered models for the agent; discovered on demand when empty on a plan launch.
    pub installed: Vec<crate::agent_models::Model>,
    /// The active plan account's last known limit windows; None when unread.
    pub plan_limits: Option<Vec<crate::plans::Limit>>,
    /// Current time in epoch milliseconds for checking limit window resets.
    pub now_ms: i64,
}

pub fn find(agent: SessionAgent, env: &HashMap<String, String>, cwd: &Path) -> Option<PathBuf> {
    match agent.kind() {
        SessionKind::Coding(coding) => nexus_local_agents::find_agent(coding, env, cfg!(windows), cwd)
            .ok()
            .flatten(),
        SessionKind::Nexus => crate::nexus_runtime::select(env).ready().map(Path::to_path_buf),
    }
}

/// The model as the agent names it: Claude Code and Codex take their provider's own id; Nexus's agent takes the
/// catalog id as Gateway does.
fn provider_model(agent: SessionAgent, model: &str) -> String {
    match agent.kind() {
        SessionKind::Coding(client) => agents::provider_model_id(client, model),
        SessionKind::Nexus => model.to_string(),
    }
}

/// The model the agent should run, by Gateway's rules, as the desktop app and `nexus claude` decide it.
pub async fn choose_model(client: &Client, agent: SessionAgent, asked: Option<&str>) -> Result<Option<String>, String> {
    let name = agent.as_str();
    if agents::is_auto_name(asked) {
        let check = client
            .post("/v1/agents/check", json!({ "agent": name, "model": "nexus/auto" }))
            .await
            .map_err(|e| e.to_string())?;
        let allowed = check.get("allowed").and_then(Value::as_bool).unwrap_or(false);
        let provider_model = check
            .get("providerModel")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty());
        return match (allowed, provider_model) {
            (true, Some(model)) => Ok(Some(model.to_string())),
            _ => Err(format!(
                "Gateway has no model for {}: {}.",
                label(agent),
                check.get("reason").and_then(Value::as_str).unwrap_or("not allowed")
            )),
        };
    }
    if let Some(model) = asked {
        return Ok(Some(provider_model(agent, model)));
    }
    let Ok(check) = client.post("/v1/agents/check", json!({ "agent": name })).await else {
        return Ok(None);
    };
    let chosen = check
        .pointer("/defaultFrom/level")
        .and_then(Value::as_str)
        .is_some_and(|level| ["organization", "team", "agent"].contains(&level));
    if !(chosen && check.get("allowed").and_then(Value::as_bool).unwrap_or(false)) {
        return Ok(None);
    }
    Ok(check
        .get("providerModel")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            check
                .get("model")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(|m| provider_model(agent, m))
        }))
}

/// Checks the payer and chooses the model before a Gateway session is opened.
pub async fn prepare(
    client: &Client,
    request: &crate::LaunchRequest,
    program: &Path,
    env: HashMap<String, String>,
    home: Option<&str>,
) -> Result<(Option<String>, Vec<String>), String> {
    prepare_with_auto(client, request, program, env, home, None).await
}

/// Checks the payer and chooses the model before a Gateway session is opened, routing `auto` through
/// `POST /v1/agents/route` when `auto` is supplied.
pub async fn prepare_with_auto(
    client: &Client,
    request: &crate::LaunchRequest,
    program: &Path,
    env: HashMap<String, String>,
    home: Option<&str>,
    auto: Option<AutoContext<'_>>,
) -> Result<(Option<String>, Vec<String>), String> {
    let agent = request.agent;
    let entry = agent.coding().map(|coding| coding.entry());
    let through_gateway = entry.is_none_or(|entry| entry.billing.gateway.is_some());
    crate::harness::harness(agent).launch_refusal(request.via, home)?;
    if request.via == Via::Subscription && !nexus_client::agent_setup::subscriptions_allowed(client).await? {
        return Err(if through_gateway {
            "Your organization does not allow sessions on a personal subscription. Start a new session \
             through Nexus instead."
                .into()
        } else {
            format!(
                "Your organization does not allow sessions on a personal subscription, and {} runs on one only.",
                label(agent),
            )
        });
    }
    let path_env = env.get("PATH").cloned().unwrap_or_default();
    let mut routed_effort = None;
    crate::computer::settings::check_launch_payer(request, program.to_path_buf(), env).await?;
    // agy's model is its own id, as `agy models` lists it. The session sets it as it opens
    // (`session/set_config_option`), never on agy's command line; none is agy's default.
    let model = if !through_gateway {
        request.model.clone()
    } else if agents::is_auto_name(request.model.as_deref())
        && let Some(auto) = auto
        && let Some(coding) = agent.coding()
    {
        match auto_launch(client, request, coding, program, &path_env, auto).await {
            auto_route::Launch::Run { model, effort } => {
                routed_effort = effort.as_deref().and_then(crate::event::Effort::parse);
                Some(model)
            }
            auto_route::Launch::Refuse(text) => return Err(text),
            auto_route::Launch::AsBefore => choose_model(client, request.agent, request.model.as_deref()).await?,
        }
    } else {
        choose_model(client, request.agent, request.model.as_deref()).await?
    };
    let mut args = request.args.clone();
    if through_gateway
        && let Some(model) = &model
        && request.model.is_some()
    {
        args = agents::replace_model(
            request
                .agent
                .coding()
                .ok_or_else(|| "This agent has no coding harness.".to_string())?,
            &args,
            model,
        );
    }
    if let (Some(effort), Some(coding)) = (routed_effort, agent.coding()) {
        args = with_effort(coding, &args, effort);
    }
    Ok((model, args))
}

async fn auto_launch(
    client: &Client,
    request: &crate::LaunchRequest,
    coding: nexus_agents::Agent,
    program: &Path,
    path_env: &str,
    auto: AutoContext<'_>,
) -> auto_route::Launch {
    let on_plan = request.via == Via::Subscription;
    let account = account_of(request);
    match auto_answer(
        client,
        coding,
        request.via,
        account.as_deref(),
        program,
        path_env,
        auto,
        request.prompt_counts,
    )
    .await
    {
        Some(answer) => auto_route::launch(answer, on_plan, coding.entry().identity.plan),
        None => auto_route::Launch::AsBefore,
    }
}

/// The room one plan login has per model, as a route request sends it: each installed model Gateway's catalog knows.
/// `None`: nothing to send (no model Gateway knows).
pub async fn plan_reading(
    client: &Client,
    coding: nexus_agents::Agent,
    account: Option<&str>,
    program: &Path,
    path_env: &str,
    auto: AutoContext<'_>,
) -> Option<Vec<(String, Room)>> {
    let catalog = match auto.prices {
        Some(known) => {
            let existing = known.read().map(|held| held.clone()).unwrap_or_default();
            if existing.is_empty() {
                let loaded = load_catalog(client).await;
                if !loaded.is_empty()
                    && let Ok(mut held) = known.write()
                {
                    *held = loaded.clone();
                }
                loaded
            } else {
                existing
            }
        }
        None => load_catalog(client).await,
    };
    let models = if auto.installed.is_empty() {
        nexus_local_agents::agents::local(coding)
            .models(program, path_env, None, None)
            .await
            .unwrap_or_default()
    } else {
        auto.installed
    };
    let windows: Option<Vec<auto_route::Window<'_>>> = auto.plan_limits.as_ref().map(|limits| {
        limits
            .iter()
            .map(|limit| auto_route::Window {
                label: &limit.label,
                models: &limit.models,
                used_percent: limit.used_percent,
                resets_at: limit.resets_at,
            })
            .collect()
    });
    let lookup = |id: &str| catalog.id(id);
    let cx = OfferContext {
        installed: &models,
        windows: windows.as_deref(),
        now: auto.now_ms,
        catalog: &lookup,
    };
    let offers = CliSubscriptionProvider::new(coding, account).plan_offers(&cx);
    Some(plan_of(&offers)).filter(|plan| !plan.is_empty())
}

/// Gateway's Auto answer for a launch of `coding` paid by `via`, as the launch asks it: the plan's offers with their
/// room when `via` is the plan, none through Gateway. A dry run: it opens no session. `None`: Auto cannot route here
/// (a plan with no model Gateway knows, or no answer), and the launch starts as before.
#[allow(clippy::too_many_arguments)]
pub async fn auto_answer(
    client: &Client,
    coding: nexus_agents::Agent,
    via: Via,
    account: Option<&str>,
    program: &Path,
    path_env: &str,
    auto: AutoContext<'_>,
    counts: Option<auto_route::Counts>,
) -> Option<auto_route::Answer> {
    let on_plan = via == Via::Subscription;
    let plan = if on_plan {
        Some(
            plan_reading(client, coding, account, program, path_env, auto)
                .await
                .unwrap_or_default(),
        )
    } else {
        None
    };
    if !auto_route::routable(on_plan, plan.as_deref()) {
        return None;
    }
    let body = auto_route::body_with(coding.as_str(), plan.as_deref(), None, counts);
    auto_route::route(client, body).await.ok()
}

/// A plan login's room per model, by the agent's own ids, as a route request sends it.
pub type PlanRooms = Vec<(String, Room)>;

/// One agent Auto may pick, with what its plan reading needs.
pub struct AcrossAgent<'a> {
    pub coding: nexus_agents::Agent,
    pub program: PathBuf,
    pub auto: AutoContext<'a>,
}

/// Gateway's answer when Auto picks the agent too: the agent it names and its answer, paid by `via` (each agent's plan
/// first when `via` is the plan). A dry run. `None`: no agent here, or no answer (a Gateway older than `agent: "auto"`).
pub async fn auto_answer_across(
    client: &Client,
    agents: Vec<AcrossAgent<'_>>,
    via: Via,
    path_env: &str,
    counts: Option<auto_route::Counts>,
) -> Option<(SessionAgent, auto_route::Answer)> {
    let mut readings: Vec<(nexus_agents::Agent, Option<PlanRooms>)> = Vec::new();
    for agent in agents {
        let plan = if via == Via::Subscription {
            plan_reading(client, agent.coding, None, &agent.program, path_env, agent.auto).await
        } else {
            None
        };
        readings.push((agent.coding, plan));
    }
    if readings.is_empty() {
        return None;
    }
    let listed: Vec<auto_route::AcrossReading<'_>> = readings
        .iter()
        .map(|(coding, plan)| (coding.as_str(), plan.as_deref()))
        .collect();
    let body = auto_route::body_across(&listed, counts);
    let (agent, answer) = auto_route::route_across(client, body).await.ok()?;
    let agent = SessionAgent::parse(&agent?)?;
    Some((agent, answer))
}

/// The agent's arguments with Auto's routed effort, unless the person already set one: Claude Code's `--effort`,
/// Codex's `model_reasoning_effort` override. Other agents keep their arguments.
pub fn with_effort(agent: nexus_agents::Agent, args: &[String], effort: crate::event::Effort) -> Vec<String> {
    let set = args
        .iter()
        .any(|arg| arg == "--effort" || arg.starts_with("--effort=") || arg.starts_with("model_reasoning_effort"));
    if set {
        return args.to_vec();
    }
    let mut out = args.to_vec();
    match agent {
        nexus_agents::Agent::Claude => {
            out.splice(0..0, ["--effort".to_string(), effort.as_str().to_string()]);
        }
        nexus_agents::Agent::Codex => {
            out.splice(
                0..0,
                [
                    "-c".to_string(),
                    format!("model_reasoning_effort={}", Value::from(effort.as_str())),
                ],
            );
        }
        _ => {}
    }
    out
}

/// The plan Auto's request sends, from the plan's offers: each model Gateway's catalog knows (Gateway refuses an id it
/// lacks), with its room. A model with no reading has room, as a launch on the plan has today: its agent reports the
/// limit when it meets one.
pub fn plan_of(offers: &[ModelOffer]) -> Vec<(String, Room)> {
    offers
        .iter()
        .filter(|offer| offer.catalog.is_some())
        .map(|offer| {
            let room = match offer.room {
                Room::Unknown => Room::Left,
                room => room,
            };
            (offer.agent_model.clone(), room)
        })
        .collect()
}

/// A desktop host admitted this launch on the Local ledger.
pub enum LocalLaunch<'a> {
    Subscription(SubscriptionAdmission),
    SavedProvider(&'a LocalProviders),
}

/// A subscription made it through the host's Local launch gate.
#[derive(Clone, Copy)]
pub struct SubscriptionAdmission(());

pub fn admit_local_launch<'a>(
    request: &crate::LaunchRequest,
    scope: &SessionScope,
    mode: &'a LocalProvidersOptIn,
) -> Option<LocalLaunch<'a>> {
    let local = mode.local_providers()?;
    if !allows_local_launch(request, scope) {
        return None;
    }
    Some(match &request.provider {
        Some(_) => LocalLaunch::SavedProvider(local),
        None => LocalLaunch::Subscription(SubscriptionAdmission(())),
    })
}

pub(crate) fn allows_local_launch(request: &crate::LaunchRequest, scope: &SessionScope) -> bool {
    scope.is_local()
        && request.scope.is_local()
        && match request.provider {
            Some(_) => request.agent.coding().is_some_and(|agent| {
                matches!(
                    agent,
                    nexus_agents::Agent::Claude | nexus_agents::Agent::Codex | nexus_agents::Agent::Opencode
                )
            }),
            None => request.via == Via::Subscription && supports_local_subscription(request.agent),
        }
}

pub(crate) fn supports_local_subscription(agent: SessionAgent) -> bool {
    agent.coding().is_some_and(|coding| {
        coding == nexus_agents::Agent::Opencode || crate::harness::harness(agent).plan_payer().is_some()
    })
}

pub(crate) fn prepare_local(request: &crate::LaunchRequest) -> Result<Prepared, String> {
    use crate::computer::BillingPayer;

    if matches!(
        request.required_payer,
        Some(BillingPayer::ApiKey | BillingPayer::Gateway)
    ) {
        return Err("Choose a personal subscription to start this session without a Nexus sign-in.".into());
    }
    prepare_saved_key(request)
}

pub(crate) fn prepare_saved_key(request: &crate::LaunchRequest) -> Result<Prepared, String> {
    if agents::is_auto_name(request.model.as_deref()) {
        return Err("Choose a model or use the agent's own default. Nexus Auto requires a sign-in.".into());
    }
    let model = request
        .model
        .as_deref()
        .map(|model| provider_model(request.agent, model));
    let mut args = request.args.clone();
    if let Some(agent) = request.agent.coding()
        && !crate::mcp_servers::in_session(request.agent)
        && let Some(model) = &model
    {
        args = agents::replace_model(agent, &args, model);
    }
    Ok(Prepared::Local { model, args })
}

/// Builds a local subscription spawn before the host grants access to its configured MCP servers.
pub async fn local_subscription(
    admission: SubscriptionAdmission,
    request: &crate::LaunchRequest,
    program: PathBuf,
    mut env: HashMap<String, String>,
    configure_mcp: impl FnOnce(&mut crate::Launch),
) -> Result<crate::Launch, String> {
    let agent = request.agent;
    let coding = agent
        .coding()
        .ok_or_else(|| format!("Sign in to Nexus to start {}.", label(agent)))?;
    let account = account_of(request);
    let provider = CliSubscriptionProvider::new(coding, account.as_deref());
    let cx = LaunchContext {
        request,
        route: LaunchRoute::LocalSubscription(admission),
        program: &program,
        env: &env,
        home: env.get("HOME").or_else(|| env.get("USERPROFILE")).map(String::as_str),
    };
    let prepared = provider.prepare(agent, &cx).await.map_err(|error| error.to_string())?;
    let credential = provider
        .credential(agent, &prepared, &cx)
        .await
        .map_err(|error| error.to_string())?;
    let Prepared::Local { model, args } = prepared else {
        unreachable!("the subscription provider has no Nexus client")
    };
    strip_gateway_env(&mut env);
    let mut launch = crate::Launch {
        attribution: None,
        spawn: crate::process::Spawn {
            program,
            args,
            env,
            cwd: request.cwd.clone(),
        },
        nexus_session: None,
        local_run: Some(format!("local_{}", uuid::Uuid::new_v4())),
        local_provider: None,
        model,
        session_mcp: crate::mcp_servers::SessionMcp::default(),
    };
    configure_mcp(&mut launch);
    (launch.spawn.args, launch.spawn.env) =
        agents::subscription_launch(coding, credential, None, launch.spawn.args, launch.spawn.env);
    Ok(launch)
}

pub(crate) fn strip_gateway_env(env: &mut HashMap<String, String>) {
    // The app may itself have been started by an agent with a Gateway usage sink.
    const GATEWAY_ENV: &[&str] = &[
        agents::CODEX_KEY_ENV,
        nexus_local_agents::agents::OPENCODE_KEY_ENV,
        "CLAUDE_CODE_ENABLE_TELEMETRY",
        "OTEL_LOGS_EXPORTER",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        crate::mcp_servers::APP_ADDRESS_VAR,
        crate::mcp_servers::APP_TOKEN_VAR,
    ];
    let matches_name = |name: &str, variable: &str| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case(variable)
        } else {
            name == variable
        }
    };
    let opencode_key = format!("{{env:{}}}", nexus_local_agents::agents::OPENCODE_KEY_ENV);
    env.retain(|name, value| {
        if GATEWAY_ENV.iter().any(|variable| matches_name(name, variable)) {
            return false;
        }
        // An inherited Gateway launch config would still select its provider after removing its key.
        !(matches_name(name, "OPENCODE_CONFIG_CONTENT")
            && serde_json::from_str::<serde_json::Value>(value).is_ok_and(|config| {
                config
                    .pointer("/provider/nexus/options/apiKey")
                    .and_then(serde_json::Value::as_str)
                    == Some(opencode_key.as_str())
            }))
    });
}

/// Checks the local login using the same environment as the child.
pub(crate) async fn check_local_subscription(
    agent: SessionAgent,
    payer: Option<crate::computer::BillingPayer>,
    spawn: &crate::process::Spawn,
) -> Result<(), String> {
    use crate::computer::BillingPayer;
    use nexus_local_agents::Login;

    if matches!(payer, Some(BillingPayer::ApiKey | BillingPayer::Gateway)) {
        return Err("This session's selected payer no longer matches its route.".into());
    }
    let Some(coding) = agent.coding() else {
        return Ok(());
    };
    // A login probe can outlive a stop request. Only the actual agent gets its launcher token.
    let mut spawn = spawn.clone();
    spawn.env.remove(crate::mcp_servers::APP_TOKEN_VAR);
    spawn.env.remove(crate::mcp_servers::APP_ADDRESS_VAR);
    let login = tokio::task::spawn_blocking(move || {
        nexus_local_agents::login_with(coding, &spawn.program, &spawn.env, &spawn.cwd).map(|(login, _, _)| login)
    })
    .await
    .map_err(|_| format!("Could not verify the {} subscription sign-in.", label(agent)))?;
    match login {
        Some(Login::Plan) => Ok(()),
        Some(Login::Unknown) | None if payer.is_none() => Ok(()),
        Some(Login::SignedOut) => Err(format!(
            "Sign in to {} with a subscription, then try again.",
            label(agent)
        )),
        Some(Login::ApiKey) => Err(format!("Switch {} to a subscription, then try again.", label(agent))),
        Some(Login::Unknown) | None => Err(format!("Could not verify the {} subscription sign-in.", label(agent))),
    }
}

/// Gateway's accepted session and the arguments prepared for it.
pub struct OpenedSession {
    pub session_id: String,
    pub model: Option<String>,
    pub team: Option<String>,
    pub access: Option<String>,
    pub args: Vec<String>,
}

/// Checks the payer and model, then opens the one Gateway session for this launch.
pub async fn open_session(
    client: &Client,
    request: &crate::LaunchRequest,
    program: &Path,
    env: HashMap<String, String>,
    home: Option<&str>,
    auto: Option<AutoContext<'_>>,
    facts: &crate::provider::SessionFacts,
) -> Result<OpenedSession, String> {
    let (model, args) = prepare_with_auto(client, request, program, env, home, auto).await?;
    let mut body = json!({
        "agent": request.agent.as_str(),
        "via": request.via.as_str(),
        "model": model,
    });
    if facts.usage_data {
        crate::plan_tier::add_to(&mut body, request.agent, request.account_dir.as_deref());
    }
    let opened = client
        .post("/v1/agents/sessions", body)
        .await
        .map_err(|error| format!("Nexus Gateway did not open the session: {error}"))?;
    let session_id = opened["id"].as_str().unwrap_or_default().to_string();
    if session_id.is_empty() && crate::harness::harness(request.agent).session_features().local_only {
        return Err("Nexus Gateway did not return a session ID.".into());
    }
    let team = opened["team"]
        .as_str()
        .filter(|team| !team.is_empty())
        .map(String::from);
    let access = opened["access"]
        .as_str()
        .filter(|access| ["byok", "nexus", "subscription"].contains(access))
        .map(String::from);
    Ok(OpenedSession {
        session_id,
        model,
        team,
        access,
        args,
    })
}

/// The extra plan account a launch starts on: its folder's name, the id `accounts::add` gave it. None: the agent's own
/// login.
pub(crate) fn account_of(request: &crate::LaunchRequest) -> Option<String> {
    request
        .account_dir
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

/// Builds the launch after accounts and tools have supplied their arguments and environment.
pub async fn launch(
    agent: nexus_agents::Agent,
    program: &Path,
    dir: &Path,
    eligible: bool,
    mut options: agents::LaunchOptions<'_>,
) -> (Vec<String>, HashMap<String, String>) {
    let catalog = if options.model_catalog.is_none() && options.route == agents::Route::Gateway {
        nexus_local_agents::agents::local(agent)
            .model_catalog(program, &options.env, dir)
            .await
    } else {
        None
    };
    if eligible {
        nexus_client::commit_trailer::mark(&mut options.env, nexus_client::commit_trailer::Surface::Desktop);
    }
    let model_catalog = options.model_catalog.or(catalog.as_deref());
    agents::agent_launch(
        agent,
        agents::LaunchOptions {
            model_catalog,
            ..options
        },
    )
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use nexus_agents::Agent;
    use nexus_client::agents::{LaunchOptions, Route};

    fn local_request(agent: SessionAgent) -> crate::LaunchRequest {
        crate::LaunchRequest {
            thread_id: "local".into(),
            agent,
            cwd: std::env::temp_dir(),
            model: None,
            via: Via::Subscription,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: vec![],
            scope: SessionScope::Local,
            prompt_counts: None,
        }
    }

    #[test]
    fn local_preparation_refuses_explicit_non_plan_payers_and_auto() {
        use crate::computer::BillingPayer;
        let mut request = local_request(SessionAgent::Claude);
        for payer in [BillingPayer::ApiKey, BillingPayer::Gateway] {
            request.required_payer = Some(payer);
            assert_eq!(
                prepare_local(&request),
                Err("Choose a personal subscription to start this session without a Nexus sign-in.".into())
            );
        }
        request.required_payer = None;
        for model in ["nexus/auto", "auto", "nexus/default", "default"] {
            request.model = Some(model.into());
            assert_eq!(
                prepare_local(&request),
                Err("Choose a model or use the agent's own default. Nexus Auto requires a sign-in.".into())
            );
        }
        request.model = None;
        assert_eq!(
            prepare_local(&request),
            Ok(Prepared::Local {
                model: None,
                args: vec![]
            })
        );
    }

    #[test]
    fn local_environment_drops_inherited_gateway_values_and_keeps_the_agents_login() {
        let incoming = [
            "NEXUS_API_KEY",
            "NEXUS_OPENCODE_API_KEY",
            "CLAUDE_CODE_ENABLE_TELEMETRY",
            "OTEL_LOGS_EXPORTER",
            "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
            "NEXUS_MCP_APP_ADDR",
            "NEXUS_MCP_APP_TOKEN",
        ];
        let mut env = HashMap::from([
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), "own-login".into()),
            ("PATH".into(), "/bin".into()),
        ]);
        env.extend(incoming.map(|name| (name.into(), "prior-run".into())));
        strip_gateway_env(&mut env);
        assert_eq!(
            env,
            HashMap::from([
                ("CLAUDE_CODE_OAUTH_TOKEN".into(), "own-login".into()),
                ("PATH".into(), "/bin".into()),
            ])
        );
    }

    #[test]
    fn local_environment_removes_only_the_inherited_gateway_opencode_config() {
        let variable = "OPENCODE_CONFIG_CONTENT";
        let gateway = r#"{"provider":{"nexus":{"options":{"apiKey":"{env:NEXUS_OPENCODE_API_KEY}"}}}}"#;
        let local = r#"{"model":"local/model"}"#;
        let mut env = HashMap::from([(variable.into(), gateway.into())]);
        strip_gateway_env(&mut env);
        assert!(env.is_empty());
        env.insert(variable.into(), local.into());
        strip_gateway_env(&mut env);
        assert_eq!(env.get(variable).map(String::as_str), Some(local));
    }

    fn fake_local_login(root: &Path, status: &str) -> crate::process::Spawn {
        let program = root.join(format!("agent-{}", uuid::Uuid::new_v4()));
        let launcher_token = crate::mcp_servers::APP_TOKEN_VAR;
        let script = format!(
            "#!/bin/sh\nif [ -n \"${{{launcher_token}}}\" ]; then printf 'leaked token'; exit 1; fi\nif [ \"$1\" = --version ]; then printf '1.1.12\\n'; else printf '%s\\n' '{status}'; fi\n"
        );
        crate::process::write_fake_script(&program, &script);
        crate::process::Spawn {
            program,
            args: vec![],
            cwd: root.to_path_buf(),
            env: HashMap::from([
                ("HOME".into(), root.to_string_lossy().into_owned()),
                (launcher_token.into(), "launcher-test-token".into()),
            ]),
        }
    }

    #[tokio::test]
    async fn local_login_accepts_an_unknown_login_for_each_supported_agent_without_a_payer() {
        use crate::computer::BillingPayer;
        let root = std::env::temp_dir().join(format!("nexus-local-login-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory");
        let unknown = fake_local_login(&root, "unrecognized status");
        assert!(!supports_local_subscription(SessionAgent::Nexus));
        #[cfg(feature = "opencode")]
        assert!(supports_local_subscription(SessionAgent::Opencode));
        for agent in SessionAgent::ALL.into_iter().filter(|agent| agent.coding().is_some()) {
            assert!(
                allows_local_launch(&local_request(agent), &SessionScope::Local),
                "{agent:?}"
            );
            assert_eq!(
                check_local_subscription(agent, None, &unknown).await,
                Ok(()),
                "{agent:?}"
            );
            assert!(
                check_local_subscription(agent, Some(BillingPayer::Plan), &unknown)
                    .await
                    .is_err(),
                "{agent:?}"
            );
        }
        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[tokio::test]
    async fn local_login_refuses_known_api_key_or_signed_out_status_and_accepts_a_plan() {
        use crate::computer::BillingPayer;
        let root = std::env::temp_dir().join(format!("nexus-local-login-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test directory");
        let agent = SessionAgent::Claude;
        let plan = fake_local_login(&root, r#"{"loggedIn":true,"authMethod":"claude.ai"}"#);
        let api = fake_local_login(&root, r#"{"loggedIn":true,"authMethod":"api_key"}"#);
        let signed_out = fake_local_login(&root, r#"{"loggedIn":false}"#);
        assert_eq!(
            check_local_subscription(agent, Some(BillingPayer::Plan), &plan).await,
            Ok(())
        );
        assert_eq!(
            check_local_subscription(agent, None, &api).await,
            Err("Switch Claude Code to a subscription, then try again.".into())
        );
        assert_eq!(
            check_local_subscription(agent, None, &signed_out).await,
            Err("Sign in to Claude Code with a subscription, then try again.".into())
        );
        for payer in [BillingPayer::ApiKey, BillingPayer::Gateway] {
            assert_eq!(
                check_local_subscription(agent, Some(payer), &plan).await,
                Err("This session's selected payer no longer matches its route.".into())
            );
        }
        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[tokio::test]
    async fn catalog_probe_precedes_commit_marking_and_explicit_catalog_skips_it() {
        let dir = std::env::temp_dir().join(format!("nexus-launch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temporary directory");
        let program = dir.join("codex");
        crate::process::write_fake_script(
            &program,
            "#!/bin/sh\n[ \"$GIT_CONFIG_PARAMETERS\" = original ] || exit 1\n: > \"$CATALOG_MARKER\"\nprintf '%s' '{\"models\":[{\"slug\":\"m\",\"use_responses_lite\":true}]}'\n",
        );
        let options = || LaunchOptions {
            route: Route::Gateway,
            core_url: "https://example.test",
            api_key: "test",
            session_id: "session",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: Vec::new(),
            env: HashMap::from([
                ("GIT_CONFIG_PARAMETERS".into(), "original".into()),
                (
                    "CATALOG_MARKER".into(),
                    dir.join("probed").to_string_lossy().into_owned(),
                ),
            ]),
        };
        let (args, env) = launch(Agent::Codex, &program, &dir, true, options()).await;
        assert!(args.iter().any(|arg| arg.contains("model_catalog_json")));
        assert!(env["GIT_CONFIG_PARAMETERS"].contains("Co-authored-by"));
        assert!(
            std::fs::read_dir(&dir)
                .expect("directory")
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("codex-models-"))
        );

        std::fs::remove_file(dir.join("probed")).expect("initial probe ran");
        let explicit = dir.join("explicit.json");
        let mut options = options();
        options.model_catalog = Some(&explicit);
        let (args, _) = launch(Agent::Codex, &program, &dir, false, options).await;
        assert!(args.iter().any(|arg| arg.contains("explicit.json")));
        assert!(!dir.join("probed").exists());
        std::fs::remove_dir_all(dir).expect("remove temporary directory");
    }

    pub(crate) async fn fake_gateway(
        handler: impl Fn(&str, &str) -> (u16, Value) + Send + Sync + 'static,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handler = std::sync::Arc::new(handler);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let mut header_end = None;
                    let mut content_len = 0usize;
                    while let Ok(n) = stream.read(&mut chunk).await {
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if header_end.is_none()
                            && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                        {
                            header_end = Some(pos + 4);
                            let head = String::from_utf8_lossy(&buf[..pos]);
                            for line in head.lines() {
                                if let Some((k, v)) = line.split_once(':')
                                    && k.eq_ignore_ascii_case("content-length")
                                {
                                    content_len = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                        if let Some(end) = header_end
                            && buf.len() >= end + content_len
                        {
                            break;
                        }
                    }
                    let Some(end) = header_end else { return };
                    let head = String::from_utf8_lossy(&buf[..end]);
                    let path = head
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let body = String::from_utf8_lossy(&buf[end..buf.len().min(end + content_len)]);
                    let (status, payload) = handler(path, &body);
                    let json = payload.to_string();
                    let resp = format!(
                        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                        json.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), task)
    }

    pub(crate) fn auto_request(via: Via) -> crate::LaunchRequest {
        crate::LaunchRequest {
            thread_id: "t".into(),
            agent: SessionAgent::Claude,
            cwd: std::env::temp_dir(),
            model: Some("auto".into()),
            via,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: vec!["--model".into(), "auto".into()],
            scope: "gw|org".into(),
            prompt_counts: Some(auto_route::Counts::of("```rust\nfn main() {}\n```", false)),
        }
    }

    fn opus_model() -> crate::agent_models::Model {
        crate::agent_models::Model {
            id: "opus".into(),
            name: "Opus 5.5".into(),
            efforts: Vec::new(),
            resolved_id: Some("claude-opus-5-5".into()),
            fast: false,
        }
    }

    #[tokio::test]
    async fn org_disabled_subscriptions_refuses_before_auto_route() {
        nexus_client::install_tls();
        let route_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = route_called.clone();
        let (url, server) = fake_gateway(move |path, _| match path {
            "/v1/orgs/settings" => (200, json!({ "agentSubscriptions": false })),
            "/v1/agents/route" => {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                (
                    200,
                    json!({ "result": "ask_for_paid_fallback", "model": "anthropic/claude-opus-5.5", "providerModel": "claude-opus-5-5" }),
                )
            }
            _ => (404, json!({})),
        })
        .await;
        let client = Client::new(nexus_client::ClientOptions::new(&url));
        let prices = RwLock::new(Catalog::default());
        let err = prepare_with_auto(
            &client,
            &auto_request(Via::Subscription),
            Path::new("/bin/claude"),
            HashMap::new(),
            None,
            Some(AutoContext {
                prices: Some(&prices),
                installed: vec![opus_model()],
                plan_limits: None,
                now_ms: 1_000,
            }),
        )
        .await
        .expect_err("org disabled subscriptions refuses");
        server.abort();
        assert!(
            err.contains("does not allow sessions on a personal subscription"),
            "{err}"
        );
        assert!(!route_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn prepare_with_auto_routes_plan_sends_counts_caches_prices_and_replaces_model_arg() {
        nexus_client::install_tls();
        let seen_body = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
        let seen = seen_body.clone();
        let (url, server) = fake_gateway(move |path, body| match path {
            "/v1/orgs/settings" => (200, json!({ "agentSubscriptions": true })),
            "/v1/models?status=all" => (
                200,
                json!({
                    "data": [{
                        "id": "anthropic/claude-opus-5.5",
                        "provider_model": "claude-opus-5-5",
                        "pricing": { "input": 15.0, "output": 75.0 }
                    }]
                }),
            ),
            "/v1/agents/route" => {
                *seen.lock().expect("seen lock") = serde_json::from_str(body).ok();
                (
                    200,
                    json!({ "result": "use_plan", "model": "anthropic/claude-opus-5.5", "providerModel": "claude-opus-5-5" }),
                )
            }
            _ => (404, json!({})),
        })
        .await;
        let client = Client::new(nexus_client::ClientOptions::new(&url));
        let prices = RwLock::new(Catalog::default());
        let (model, args) = prepare_with_auto(
            &client,
            &auto_request(Via::Subscription),
            Path::new("/bin/claude"),
            HashMap::new(),
            None,
            Some(AutoContext {
                prices: Some(&prices),
                installed: vec![opus_model()],
                plan_limits: None,
                now_ms: 1_000,
            }),
        )
        .await
        .expect("routed");
        server.abort();
        assert_eq!(model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(args, vec!["--model", "claude-opus-5-5"]);
        assert!(
            prices
                .read()
                .expect("prices read")
                .prices
                .contains_key("claude-opus-5-5")
        );
        let sent = seen_body.lock().expect("seen lock").clone().expect("route called");
        assert_eq!(
            sent,
            json!({
                "agent": "claude",
                "plans": [{ "account": 0, "models": [{ "id": "claude-opus-5-5", "room": "left" }] }],
                "counts": { "inputTokens": 6, "codeBlocks": 1, "images": false }
            })
        );
    }

    /// The whole request Auto sends for a plan launch with readings: each priced model the agent runs, once, in the
    /// order it lists them, with its room. `default` and an id Gateway does not price are left out, and a model whose
    /// scoped week is used up says so.
    #[tokio::test]
    async fn auto_sends_each_priced_plan_model_once_with_its_room() {
        nexus_client::install_tls();
        let seen_body = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
        let seen = seen_body.clone();
        let (url, server) = fake_gateway(move |path, body| match path {
            "/v1/orgs/settings" => (200, json!({ "agentSubscriptions": true })),
            "/v1/models?status=all" => (
                200,
                json!({
                    "data": [
                        {
                            "id": "anthropic/claude-opus-5.5",
                            "provider_model": "claude-opus-5-5",
                            "pricing": { "input": 15.0, "output": 75.0 }
                        },
                        {
                            "id": "anthropic/claude-fable-5.2",
                            "provider_model": "claude-fable-5-2",
                            "pricing": { "input": 5.0, "output": 25.0 }
                        },
                        { "id": "anthropic/claude-sonnet-9", "provider_model": "claude-sonnet-9" }
                    ]
                }),
            ),
            "/v1/agents/route" => {
                *seen.lock().expect("seen lock") = serde_json::from_str(body).ok();
                (
                    200,
                    json!({ "result": "use_plan", "model": "anthropic/claude-opus-5.5", "providerModel": "claude-opus-5-5" }),
                )
            }
            _ => (404, json!({})),
        })
        .await;
        let client = Client::new(nexus_client::ClientOptions::new(&url));
        let model = |id: &str, resolved: Option<&str>, name: &str| crate::agent_models::Model {
            id: id.into(),
            name: name.into(),
            efforts: Vec::new(),
            resolved_id: resolved.map(String::from),
            fast: false,
        };
        let limit = |label: &str, used: f64, resets_at: Option<i64>| crate::plans::Limit {
            models: Vec::new(),
            label: label.into(),
            used_percent: used,
            resets_at,
        };
        let prices = RwLock::new(Default::default());
        prepare_with_auto(
            &client,
            &auto_request(Via::Subscription),
            Path::new("/bin/claude"),
            HashMap::new(),
            None,
            Some(AutoContext {
                prices: Some(&prices),
                installed: vec![
                    model("default", Some("claude-opus-5-5"), "Default"),
                    model("opus", Some("claude-opus-5-5"), "Opus 5.5"),
                    model("claude-opus-5-5", None, "Opus 5.5"),
                    model("claude-fable-5-2", None, "Fable 5.2"),
                    model("claude-sonnet-9", None, "Sonnet 9"),
                ],
                plan_limits: Some(vec![
                    limit("Session", 40.0, Some(2_000)),
                    limit("Weekly · Fable", 100.0, None),
                ]),
                now_ms: 1_000,
            }),
        )
        .await
        .expect("routed");
        server.abort();
        let sent = seen_body.lock().expect("seen lock").clone().expect("route called");
        assert_eq!(
            sent,
            json!({
                "agent": "claude",
                "plans": [{
                    "account": 0,
                    "models": [
                        { "id": "claude-opus-5-5", "room": "left" },
                        { "id": "claude-fable-5-2", "room": "usedUp" }
                    ]
                }],
                "counts": { "inputTokens": 6, "codeBlocks": 1, "images": false }
            })
        );
    }

    #[tokio::test]
    async fn prepare_with_auto_refuses_paid_fallback_on_plan_and_runs_it_on_gateway() {
        nexus_client::install_tls();
        let (url, server) = fake_gateway(move |path, _| match path {
            "/v1/orgs/settings" => (200, json!({ "agentSubscriptions": true })),
            "/v1/models?status=all" => (
                200,
                json!({
                    "data": [{
                        "id": "anthropic/claude-opus-5.5",
                        "provider_model": "claude-opus-5-5",
                        "pricing": { "input": 15.0, "output": 75.0 }
                    }]
                }),
            ),
            "/v1/agents/route" => (
                200,
                json!({ "result": "ask_for_paid_fallback", "model": "anthropic/claude-sonnet-5.5", "providerModel": "claude-sonnet-5-5", "access": "byok" }),
            ),
            _ => (404, json!({})),
        })
        .await;
        let client = Client::new(nexus_client::ClientOptions::new(&url));
        let prices = RwLock::new(Catalog::default());
        let err = prepare_with_auto(
            &client,
            &auto_request(Via::Subscription),
            Path::new("/bin/claude"),
            HashMap::new(),
            None,
            Some(AutoContext {
                prices: Some(&prices),
                installed: vec![opus_model()],
                plan_limits: None,
                now_ms: 1_000,
            }),
        )
        .await
        .expect_err("paid fallback refused on plan");
        assert!(err.starts_with("Your Claude plan has no room left"), "{err}");
        assert!(!err.contains("  "), "{err}");

        let (model, args) = prepare_with_auto(
            &client,
            &auto_request(Via::Nexus),
            Path::new("/bin/claude"),
            HashMap::new(),
            None,
            Some(AutoContext {
                prices: Some(&prices),
                installed: Vec::new(),
                plan_limits: None,
                now_ms: 1_000,
            }),
        )
        .await
        .expect("gateway runs paid model");
        server.abort();
        assert_eq!(model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(args, vec!["--model", "claude-sonnet-5-5"]);
    }

    #[test]
    fn routed_effort_goes_on_the_command_line_unless_the_person_set_one() {
        use crate::event::Effort;
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(
            with_effort(
                nexus_agents::Agent::Claude,
                &args(&["--model", "claude-opus-5-5"]),
                Effort::High
            ),
            args(&["--effort", "high", "--model", "claude-opus-5-5"])
        );
        assert_eq!(
            with_effort(nexus_agents::Agent::Codex, &args(&["app-server"]), Effort::Medium),
            args(&["-c", "model_reasoning_effort=\"medium\"", "app-server"])
        );
        let picked = args(&["--effort", "low", "--model", "claude-opus-5-5"]);
        assert_eq!(with_effort(nexus_agents::Agent::Claude, &picked, Effort::High), picked);
        let codex_picked = args(&["-c", "model_reasoning_effort=\"low\"", "app-server"]);
        assert_eq!(
            with_effort(nexus_agents::Agent::Codex, &codex_picked, Effort::High),
            codex_picked
        );
    }
}
