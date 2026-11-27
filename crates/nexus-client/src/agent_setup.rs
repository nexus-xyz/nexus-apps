//! Coding agents' setup (`agent-setup.ts` in `@nexus/kit`): what each route needs, as steps with their commands.

use serde::Serialize;
use serde_json::{Value, json};

use crate::agent_entries::{Agent, AgentEntry};
use crate::agents::Route;
use crate::local_agents::{LocalAgent, Login};
use crate::{Client, Error};

/// Whether Codex on `route` may run its hosted web search: through Gateway, when the organization's settings let it
/// ([`search_allowed`](crate::agents::search_allowed)). A settings read that fails keeps search off.
pub async fn codex_web_search(client: &Client, agent: Agent, route: Route) -> bool {
    agent == Agent::Codex
        && route == Route::Gateway
        && client
            .get("/v1/orgs/settings")
            .await
            .is_ok_and(|settings| crate::agents::search_allowed(&settings))
}

/// Read the organization's subscription policy. A failed or invalid answer is not a policy denial.
pub async fn subscriptions_allowed(client: &Client) -> Result<bool, String> {
    let settings = client.get("/v1/orgs/settings").await.map_err(|error| {
        let status = match &error {
            Error::Core(core) => format!(" (HTTP {})", core.status),
            _ => String::new(),
        };
        format!(
            "Could not check subscription access with Nexus Gateway at {}{status}: {error}",
            client.base()
        )
    })?;
    settings
        .get("agentSubscriptions")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            format!(
                "Nexus Gateway at {} returned invalid organization settings: agentSubscriptions must be a boolean.",
                client.base()
            )
        })
}

/// Each agent and its routes when Gateway has not answered: who pays on Gateway is unknown (`access: null`).
pub fn agent_catalog() -> Vec<Value> {
    Agent::ALL.iter().map(|agent| catalog_entry(agent.entry())).collect()
}

/// One agent in [`agent_catalog`]. An entry without a Gateway binding lists the Gateway route as unavailable, with
/// the reason, as Gateway's answer does for a route that cannot run.
fn catalog_entry(entry: &AgentEntry) -> Value {
    let gateway = match entry.billing.gateway {
        Some(binding) => json!({ "route": "gateway", "access": null, "available": true, "provider": binding.provider }),
        None => json!({
            "route": "gateway", "access": null, "available": false,
            "reason": entry.plan_only_reason(),
        }),
    };
    json!({
        "agent": entry.identity.id,
        "name": entry.identity.name,
        "install": entry.install.command,
        "route": "subscription",
        "from": "default",
        "routes": [
            gateway,
            {
                "route": "subscription", "access": "subscription", "available": true, "plan": entry.identity.plan,
                "vendor": entry.identity.vendor, "login": entry.sign_in.login, "loginStatus": entry.sign_in.login_status,
            },
        ],
    })
}

/// The fallback answer, shaped as Gateway's.
pub fn offline_agent_access() -> Value {
    let agents: Vec<Value> = Agent::ALL
        .into_iter()
        .filter(|agent| agent.entry().session.has_cli_launcher())
        .map(|agent| catalog_entry(agent.entry()))
        .collect();
    json!({ "agentSubscriptions": true, "agents": agents })
}

/// The agent's entry in Gateway's answer, else in [`agent_catalog`].
pub fn agent_entry(access: Option<&Value>, agent: Agent) -> Value {
    let agent = agent.as_str();
    access
        .and_then(|access| access.get("agents"))
        .and_then(Value::as_array)
        .and_then(|agents| agents.iter().find(|entry| entry["agent"] == agent))
        .cloned()
        .or_else(|| agent_catalog().into_iter().find(|entry| entry["agent"] == agent))
        .unwrap_or(Value::Null)
}

pub fn route_of(entry: &Value, route: Route) -> Option<&Value> {
    entry
        .get("routes")?
        .as_array()?
        .iter()
        .find(|view| view["route"] == route.as_str())
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn available(view: &Value) -> bool {
    view.get("available") == Some(&Value::Bool(true))
}

fn provider_label(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("Anthropic"),
        "openai" => Some("OpenAI"),
        _ => None,
    }
}

/// Who pays on a route, in a few words: "your organization's Anthropic key (BYOK)", "your Claude plan".
pub fn payer_label(view: Option<&Value>) -> String {
    let Some(view) = view else {
        return "unknown".into();
    };
    if view["route"] == Route::Subscription.as_str() {
        return format!("your {} plan", text(view, "plan").unwrap_or("own"));
    }
    let provider = text(view, "provider");
    let provider = provider.and_then(provider_label).or(provider).unwrap_or("the provider");
    match text(view, "access") {
        Some("byok") => format!("your organization's {provider} key (BYOK)"),
        Some("nexus") => "Nexus credit".into(),
        _ if available(view) => format!("your organization's {provider} key or Nexus credit"),
        _ => format!("no {provider} key yet"),
    }
}

/// A route in a few words: "Gateway", "your Claude plan".
pub fn route_label(entry: &Value, route: Route) -> String {
    match route {
        Route::Gateway => "Gateway".into(),
        Route::Subscription => format!(
            "your {} plan",
            route_of(entry, Route::Subscription)
                .and_then(|view| text(view, "plan"))
                .unwrap_or("own")
        ),
    }
}

/// One thing to do before an agent runs on a route. `done` is unset when the client cannot tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupStep {
    pub id: &'static str,
    /// Who does it: `you`, on your machine, or an owner or `admin` of the organization.
    pub who: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
}

/// What a client knows when it lists the steps; unset fields are unknown.
#[derive(Debug, Default, Clone, Copy)]
pub struct SetupOptions<'a> {
    /// What the client found on this machine.
    pub local: Option<&'a LocalAgent>,
    /// Whether the `nexus` CLI is signed in.
    pub signed_in: Option<bool>,
    pub agent_subscriptions: Option<bool>,
    /// The command that installs the CLI, for a client that is not the CLI: it then lists installing it first.
    pub cli_install: Option<&'a str>,
}

fn step(id: &'static str, who: &'static str, text: String, command: Option<String>, done: Option<bool>) -> SetupStep {
    SetupStep {
        id,
        who,
        text,
        command,
        done,
    }
}

/// What a person still has to do before `entry`'s agent runs on `route`, in order (`setupSteps` in agent-setup.ts).
pub fn setup_steps(entry: &Value, route: Route, options: SetupOptions) -> Vec<SetupStep> {
    let name = text(entry, "name").unwrap_or_default();
    let mut steps = Vec::new();
    if let Some(install) = options.cli_install {
        steps.push(step(
            "install_cli",
            "you",
            "Install the nexus CLI".into(),
            Some(install.into()),
            None,
        ));
    }
    steps.push(step(
        "install",
        "you",
        format!("Install {name}"),
        text(entry, "install").map(String::from),
        options.local.map(|local| local.installed),
    ));
    steps.push(step(
        "nexus_login",
        "you",
        "Sign in to Nexus".into(),
        Some("nexus auth login".into()),
        options.signed_in,
    ));
    let view = route_of(entry, route);
    if route == Route::Gateway {
        match view
            .filter(|view| !available(view))
            .map(|view| (view, text(view, "provider")))
        {
            Some((_, Some(provider))) => steps.push(step(
                "provider_key",
                "admin",
                format!(
                    "Add your organization's {} key (owners and admins)",
                    provider_label(provider).unwrap_or(provider)
                ),
                Some(format!("nexus keys add {provider} <key>")),
                Some(false),
            )),
            // No provider: the agent has no Gateway route (`catalog_entry`, Gateway's `gateway_route`), so no key makes
            // it run. The step says so, and keeps the route from reading as ready.
            Some((view, None)) => steps.push(step(
                "route_unsupported",
                "you",
                text(view, "reason")
                    .map(String::from)
                    .unwrap_or_else(|| format!("{name} has no Gateway route")),
                None,
                Some(false),
            )),
            None => {}
        }
        return steps;
    }
    if options.agent_subscriptions == Some(false) || view.is_some_and(|view| !available(view)) {
        steps.push(step(
            "allow_subscriptions",
            "admin",
            "Allow subscription mode (owners and admins)".into(),
            Some("nexus org settings set agentSubscriptions true".into()),
            Some(false),
        ));
    }
    let plan = view.and_then(|view| text(view, "plan")).unwrap_or("own");
    let login = view.and_then(|view| text(view, "login")).map(String::from);
    let local_login = options.local.and_then(|local| local.login);
    if local_login == Some(Login::ApiKey) {
        // A Gateway newer than this client may list an agent it does not know: then the step names no command.
        let sign_out = match Agent::parse(text(entry, "agent").unwrap_or_default()) {
            Some(agent) => format!("sign out ({})", agent.entry().sign_in.logout),
            None => "sign out".to_string(),
        };
        steps.push(step(
            "plan_relogin",
            "you",
            format!(
                "{name} is signed in with an API key, which bills the API, not your plan: {sign_out}, then sign in with your {plan} account"
            ),
            login,
            Some(false),
        ));
    } else {
        let done = match local_login {
            Some(Login::Plan) => Some(true),
            Some(Login::SignedOut) => Some(false),
            _ => None,
        };
        steps.push(step(
            "plan_login",
            "you",
            format!("Sign {name} in to your {plan} plan (once, on this machine)"),
            login,
            done,
        ));
    }
    steps
}

/// Whether every step is done; `None` when some cannot be checked from here.
pub fn setup_ready(steps: &[SetupStep]) -> Option<bool> {
    if steps.iter().any(|step| step.done == Some(false)) {
        return Some(false);
    }
    steps.iter().all(|step| step.done == Some(true)).then_some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(login: Option<Login>) -> LocalAgent {
        LocalAgent {
            agent: "claude".into(),
            installed: true,
            path: Some("/bin/claude".into()),
            version: None,
            login,
            plan: None,
        }
    }

    #[test]
    fn falls_back_to_the_catalog_when_gateway_has_not_answered() {
        let entry = agent_entry(None, Agent::Codex);
        assert_eq!(entry["name"], "Codex");
        assert_eq!(route_label(&entry, Route::Subscription), "your ChatGPT plan");
        assert_eq!(
            payer_label(route_of(&entry, Route::Gateway)),
            "your organization's OpenAI key or Nexus credit"
        );
        let answer = json!({ "agents": [{ "agent": "codex", "name": "Codex (Gateway)", "routes": [] }] });
        assert_eq!(agent_entry(Some(&answer), Agent::Codex)["name"], "Codex (Gateway)");
        assert_eq!(agent_entry(Some(&answer), Agent::Claude)["name"], "Claude Code");
    }

    #[test]
    fn offline_catalog_marks_gateway_unavailable_without_a_binding() {
        let bound = catalog_entry(Agent::Claude.entry());
        assert_eq!(
            route_of(&bound, Route::Gateway),
            Some(&json!({ "route": "gateway", "access": null, "available": true, "provider": "anthropic" }))
        );
        let claude = Agent::Claude.entry();
        let unbound = catalog_entry(&AgentEntry {
            billing: crate::agent_entries::Billing {
                gateway: None,
                ..claude.billing
            },
            ..*claude
        });
        assert_eq!(
            route_of(&unbound, Route::Gateway),
            Some(&json!({
                "route": "gateway", "access": null, "available": false,
                "reason": "Claude Code runs only on your Claude plan through Nexus for now",
            }))
        );
        assert_eq!(
            route_of(&unbound, Route::Subscription),
            route_of(&bound, Route::Subscription)
        );
    }

    /// Antigravity has no Gateway route: offline its gateway route is unavailable and names no provider, its steps on
    /// that route never suggest a key and never read as ready, and an API-key sign-in says how to undo it.
    #[test]
    fn antigravity_runs_only_on_its_plan() {
        let entry = agent_entry(None, Agent::Antigravity);
        assert_eq!(
            route_of(&entry, Route::Gateway),
            Some(&json!({
                "route": "gateway", "access": null, "available": false,
                "reason": "Antigravity runs only on your Google AI plan through Nexus for now",
            }))
        );
        let installed = LocalAgent {
            agent: "antigravity".into(),
            installed: true,
            path: Some("/bin/agy".into()),
            version: None,
            login: None,
            plan: None,
        };
        let options = SetupOptions {
            local: Some(&installed),
            signed_in: Some(true),
            ..SetupOptions::default()
        };
        let gateway = setup_steps(&entry, Route::Gateway, options);
        let last = gateway.last().expect("a step");
        assert_eq!(
            (last.id, last.command.as_deref(), last.done),
            ("route_unsupported", None, Some(false))
        );
        assert_eq!(
            last.text,
            "Antigravity runs only on your Google AI plan through Nexus for now"
        );
        assert_eq!(setup_ready(&gateway), Some(false));
        // Not checked: neither ready nor blocked.
        let plan = setup_steps(&entry, Route::Subscription, options);
        assert_eq!(plan.last().map(|step| (step.id, step.done)), Some(("plan_login", None)));
        assert_eq!(setup_ready(&plan), None);
        // `agy -p /usage` answered with the plan's quota: the sign-in step is done.
        let signed_in = LocalAgent {
            login: Some(Login::Plan),
            ..installed.clone()
        };
        let plan = setup_steps(
            &entry,
            Route::Subscription,
            SetupOptions {
                local: Some(&signed_in),
                ..options
            },
        );
        assert_eq!(
            plan.last().map(|step| (step.id, step.done)),
            Some(("plan_login", Some(true)))
        );
        let key = LocalAgent {
            login: Some(Login::ApiKey),
            ..installed.clone()
        };
        let relogin = setup_steps(
            &entry,
            Route::Subscription,
            SetupOptions {
                local: Some(&key),
                ..options
            },
        );
        let last = relogin.last().expect("a step");
        assert_eq!(last.id, "plan_relogin");
        assert!(
            last.text
                .contains("remove \"modelProvider\" from ~/.gemini/antigravity-cli/settings.json"),
            "{}",
            last.text
        );
        assert_eq!(
            payer_label(route_of(&entry, Route::Subscription)),
            "your Google AI plan"
        );
    }

    #[test]
    fn names_who_pays_on_each_route() {
        let view = |access: Value, available: bool| json!({ "route": "gateway", "access": access, "available": available, "provider": "anthropic" });
        assert_eq!(
            payer_label(Some(&view(json!("byok"), true))),
            "your organization's Anthropic key (BYOK)"
        );
        assert_eq!(payer_label(Some(&view(json!("nexus"), true))), "Nexus credit");
        assert_eq!(payer_label(Some(&view(Value::Null, false))), "no Anthropic key yet");
        assert_eq!(payer_label(None), "unknown");
    }

    #[test]
    fn lists_what_is_left_for_each_route() {
        let entry = agent_entry(None, Agent::Claude);
        let signed_out = local(Some(Login::SignedOut));
        let steps = setup_steps(
            &entry,
            Route::Subscription,
            SetupOptions {
                local: Some(&signed_out),
                signed_in: Some(true),
                agent_subscriptions: Some(false),
                cli_install: None,
            },
        );
        let ids: Vec<&str> = steps.iter().map(|step| step.id).collect();
        assert_eq!(ids, ["install", "nexus_login", "allow_subscriptions", "plan_login"]);
        assert_eq!(setup_ready(&steps), Some(false));
        assert_eq!(
            serde_json::to_value(&steps[3]).expect("json"),
            json!({
                "id": "plan_login", "who": "you", "text": "Sign Claude Code in to your Claude plan (once, on this machine)",
                "command": "claude auth login", "done": false,
            })
        );

        let key = local(Some(Login::ApiKey));
        let relogin = setup_steps(
            &entry,
            Route::Subscription,
            SetupOptions {
                local: Some(&key),
                ..Default::default()
            },
        );
        assert_eq!(relogin.last().map(|step| step.id), Some("plan_relogin"));
        // Nothing checked (a browser): unknown, not ready and not blocked.
        assert_eq!(
            setup_ready(&setup_steps(&entry, Route::Gateway, SetupOptions::default())),
            None
        );

        let refused = json!({ "agent": "claude", "name": "Claude Code", "routes": [
            { "route": "gateway", "access": null, "available": false, "provider": "anthropic" },
        ] });
        let steps = setup_steps(&refused, Route::Gateway, SetupOptions::default());
        assert_eq!(
            steps.last().and_then(|step| step.command.as_deref()),
            Some("nexus keys add anthropic <key>")
        );
    }
}
