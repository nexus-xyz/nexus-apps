//! Native programs, authentication kinds and Gateway policy resolved into bounded display choices.
use super::*;
use crate::api::AgentInfo;
use crate::event::Effort;
use nexus_client::local_agents::Login;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct GatewayChoices {
    access: Option<Value>,
    policy: Option<Value>,
    catalog: Option<Value>,
}

fn label(s: &str, limit: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(limit).collect()
}
fn vendor(agent: SessionAgent) -> &'static str {
    match agent {
        SessionAgent::Claude => "anthropic",
        SessionAgent::Codex => "openai",
        SessionAgent::Nexus => "nexus",
        SessionAgent::Antigravity => "google",
        #[cfg(feature = "cursor")]
        SessionAgent::Cursor => "cursor",
        #[cfg(feature = "opencode")]
        SessionAgent::Opencode => "opencode",
    }
}
fn agent_name(agent: SessionAgent) -> &'static str {
    crate::driver::label(agent)
}
fn model_id(agent: SessionAgent, value: &str) -> String {
    if value.contains('/') || agent == SessionAgent::Nexus {
        value.into()
    } else {
        format!("{}/{}", vendor(agent), value)
    }
}
fn valid_model(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 256 && !id.chars().any(char::is_control)
}
fn installed_models(agent: SessionAgent, models: Option<&[crate::agent_models::Model]>) -> Vec<ModelChoice> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = model_id(agent, &m.id);
            if !valid_model(&id) || !seen.insert(id.clone()) {
                return None;
            }
            let mut efforts = m.efforts.clone();
            efforts.sort();
            efforts.dedup();
            efforts.truncate(8);
            Some(ModelChoice {
                id,
                name: label(&m.name, 256),
                efforts,
            })
        })
        .take(128)
        .collect()
}
fn catalog_models(
    agent: SessionAgent,
    raw: &Value,
    installed: Option<&[crate::agent_models::Model]>,
) -> Vec<ModelChoice> {
    let mut seen = HashSet::new();
    raw.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            if !valid_model(id) || id.starts_with("nexus/") || !seen.insert(id) {
                return None;
            }
            if agent == SessionAgent::Nexus && (m["allowed"] != true || m["status"] != "supported") {
                return None;
            }
            let native = installed.and_then(|models| {
                models.iter().find(|m| {
                    model_id(agent, &m.id) == id || m.resolved_id.as_ref().is_some_and(|r| model_id(agent, r) == id)
                })
            });
            let mut efforts = m
                .pointer("/reasoning/efforts")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|v| v.as_str().and_then(Effort::parse))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| native.map(|m| m.efforts.clone()).unwrap_or_default());
            efforts.sort();
            efforts.dedup();
            efforts.truncate(8);
            Some(ModelChoice {
                id: id.into(),
                name: label(m["name"].as_str().unwrap_or(id), 256),
                efforts,
            })
        })
        .take(128)
        .collect()
}

impl GatewayChoices {
    fn route(
        &self,
        agent: SessionAgent,
        installed: bool,
        discovered: Option<&[crate::agent_models::Model]>,
    ) -> RouteChoices {
        let entry = self
            .access
            .as_ref()
            .and_then(|a| a["agents"].as_array())
            .and_then(|agents| agents.iter().find(|a| a["agent"] == agent.as_str()));
        let gateway = entry
            .and_then(|a| a["routes"].as_array())
            .and_then(|routes| routes.iter().find(|r| r["route"] == "gateway"));
        let known = self
            .catalog
            .as_ref()
            .map(|c| catalog_models(agent, c, discovered))
            .unwrap_or_default();
        let allowed = self
            .policy
            .as_ref()
            .and_then(|p| p["agents"][agent.as_str()]["allowedModels"].as_array());
        let mut seen = HashSet::new();
        let models = if agent == SessionAgent::Nexus {
            known
        } else {
            allowed
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| {
                    valid_model(id) && !id.contains(['*', '?']) && !id.starts_with("nexus/") && seen.insert(*id)
                })
                .take(128)
                .map(|id| {
                    known.iter().find(|m| m.id == id).cloned().unwrap_or(ModelChoice {
                        id: id.into(),
                        name: label(id, 256),
                        efforts: vec![],
                    })
                })
                .collect()
        };
        let state = if !installed {
            Availability::Unavailable
        } else if agent == SessionAgent::Nexus {
            match (self.catalog.is_some(), models.is_empty()) {
                (false, _) => Availability::Unknown,
                (true, true) => Availability::Unavailable,
                _ => Availability::Usable,
            }
        } else {
            match gateway.and_then(|r| r["available"].as_bool()) {
                Some(true) if allowed.is_some() => Availability::Usable,
                Some(false) => Availability::Unavailable,
                _ => Availability::Unknown,
            }
        };
        let payer = if agent == SessionAgent::Nexus {
            "Nexus account".into()
        } else {
            nexus_client::agent_setup::payer_label(gateway)
        };
        RouteChoices {
            via: Via::Nexus,
            payer: BillingPayer::Gateway,
            label: label(&payer, 256),
            state,
            models,
            default_model: self
                .policy
                .as_ref()
                .and_then(|p| p["agents"][agent.as_str()]["defaultModel"].as_str())
                .filter(|id| valid_model(id))
                .map(String::from),
            detail: match state {
                Availability::Usable => None,
                Availability::Unavailable => {
                    Some("This agent or Gateway billing is unavailable on this computer.".into())
                }
                Availability::Unknown => Some("Could not verify Gateway access. Try again.".into()),
            },
        }
    }
    fn subscriptions(&self) -> Option<bool> {
        self.access.as_ref().and_then(|a| a["agentSubscriptions"].as_bool())
    }
}

fn direct_state(
    installed: bool,
    policy: Option<bool>,
    catalog_known: bool,
    login: Login,
    expected: Login,
) -> (Availability, Option<String>) {
    let (state, detail) = if !installed {
        (Availability::Unavailable, "Install this agent on the computer first.")
    } else if policy == Some(false) {
        (
            Availability::Unavailable,
            "Your organization does not allow direct provider billing.",
        )
    } else if policy.is_none() {
        (
            Availability::Unknown,
            "Could not verify your organization's direct billing policy. Try again.",
        )
    } else if login == Login::Unknown || !catalog_known {
        (
            Availability::Unknown,
            "Could not verify this agent's sign-in or model catalog. Check the computer and try again.",
        )
    } else if login != expected {
        (
            Availability::Unavailable,
            match login {
                Login::ApiKey => {
                    "This agent is signed in with an API key. Sign in to your plan on the computer to use your subscription."
                }
                Login::Plan => "This agent is signed in to a plan, not an API key.",
                _ => "Sign in to this agent on the computer first.",
            },
        )
    } else {
        return (Availability::Usable, None);
    };
    (state, Some(detail.into()))
}

fn profile_env(
    agent: SessionAgent,
    env: &HashMap<String, String>,
    profiles: &AccountProfiles,
) -> HashMap<String, String> {
    let mut env = env.clone();
    if let Some(account) = profiles.get(&agent)
        && let Some(variable) = crate::accounts::account_variable(agent)
    {
        env.insert(variable.into(), account.to_string_lossy().into_owned());
    }
    env
}

async fn agent_choices(
    info: AgentInfo,
    gateway: &GatewayChoices,
    env: &HashMap<String, String>,
    profiles: &AccountProfiles,
) -> (AgentChoices, Login) {
    let agent = info.agent;
    let installed = info.path.is_some();
    let mut auth_env = profile_env(agent, env, profiles);
    let login = if let (Some(program), Some(client_agent)) = (info.path.as_ref(), agent.coding()) {
        let program = PathBuf::from(program);
        let cwd = auth_env
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let probe_env = auth_env.clone();
        tokio::task::spawn_blocking(move || {
            // An agent without a sign-in check (`None`) cannot be verified here either.
            nexus_client::local_agents::login_with(client_agent, &program, &probe_env, &cwd)
                .map_or(Login::Unknown, |(login, _, _)| login)
        })
        .await
        .unwrap_or(Login::Unknown)
    } else {
        Login::Unknown
    };
    if let Some(client_agent) = agent.coding() {
        for name in client_agent.entry().billing.plan_overrides {
            auth_env.remove(*name);
        }
    }
    let path = auth_env.get("PATH").map(String::as_str).unwrap_or_default();
    let discovered = if let (Some(program), Some(coding)) = (info.path.as_deref(), agent.coding()) {
        nexus_local_agents::agents::local(coding)
            .models(Path::new(program), path, info.version.as_deref(), Some(&auth_env))
            .await
    } else {
        None
    };
    let mut routes = vec![gateway.route(agent, installed, discovered.as_deref())];
    if let Some(coding) = agent.coding() {
        let models = installed_models(agent, discovered.as_deref());
        let plan = format!("Your {} plan", coding.entry().identity.plan);
        for (payer, expected, label) in [
            (BillingPayer::Plan, Login::Plan, plan.as_str()),
            (BillingPayer::ApiKey, Login::ApiKey, "Provider API key on this computer"),
        ] {
            let (state, detail) = direct_state(
                installed,
                gateway.subscriptions(),
                discovered.is_some(),
                login,
                expected,
            );
            routes.push(RouteChoices {
                via: Via::Subscription,
                payer,
                label: label.into(),
                detail,
                state,
                models: models.clone(),
                default_model: None,
            });
        }
    }
    (
        AgentChoices {
            agent,
            name: agent_name(agent).into(),
            routes,
        },
        login,
    )
}

/// Execution choices for a host that passes one Claude profile.
pub fn native_choices(
    client: Option<nexus_client::Client>,
    agents: Vec<AgentInfo>,
    env: HashMap<String, String>,
    claude_account: Option<PathBuf>,
) -> BoxFuture<'static, Result<NativeChoices>> {
    let profiles = claude_account
        .into_iter()
        .map(|dir| (SessionAgent::Claude, dir))
        .collect();
    native_choices_by_account(client, agents, env, profiles)
}

/// Shared by active native hosts, with their own discovery environment and selected profile folders.
pub fn native_choices_by_account(
    client: Option<nexus_client::Client>,
    agents: Vec<AgentInfo>,
    env: HashMap<String, String>,
    profiles: AccountProfiles,
) -> BoxFuture<'static, Result<NativeChoices>> {
    Box::pin(async move {
        let Some(client) = client else {
            return Err("Sign in on this computer to read its execution settings.".into());
        };
        let fetch = |path: &'static str| {
            let client = client.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(5), client.get(path))
                    .await
                    .ok()
                    .and_then(|r| r.ok())
            }
        };
        let (access, policy, catalog) = tokio::join!(
            fetch("/v1/agents/access"),
            fetch("/v1/agents/policy"),
            fetch("/v1/models?status=all")
        );
        let gateway = GatewayChoices {
            access,
            policy,
            catalog,
        };
        let entries = futures::future::join_all(
            agents
                .into_iter()
                .take(3)
                .map(|info| agent_choices(info, &gateway, &env, &profiles)),
        )
        .await;
        let logins = entries.iter().map(|(a, l)| (a.agent, *l)).collect();
        Ok(NativeChoices {
            agents: entries.into_iter().map(|(a, _)| a).collect(),
            logins,
        })
    })
}

/// Refuse a changed authentication type before the host opens or resumes a billable session.
pub async fn check_launch_payer(
    request: &crate::LaunchRequest,
    program: PathBuf,
    mut env: HashMap<String, String>,
) -> Result<()> {
    if let Some(dir) = &request.account_dir
        && let Some(variable) = crate::accounts::account_variable(request.agent)
    {
        env.insert(variable.into(), dir.to_string_lossy().into_owned());
    }
    check_spawn_payer(
        request.required_payer,
        request.agent,
        request.via,
        program,
        env,
        request.cwd.clone(),
    )
    .await
}

pub(crate) async fn check_spawn_payer(
    payer: Option<BillingPayer>,
    agent: SessionAgent,
    via: Via,
    program: PathBuf,
    env: HashMap<String, String>,
    cwd: PathBuf,
) -> Result<()> {
    let Some(payer) = payer else {
        return Ok(());
    };
    if payer == BillingPayer::Gateway {
        return if via == Via::Nexus {
            Ok(())
        } else {
            Err("This session's selected payer no longer matches its route.".into())
        };
    }
    if via != Via::Subscription {
        return Err("This session's selected payer no longer matches its route.".into());
    }
    let agent = agent.coding().ok_or("This agent cannot use direct provider billing.")?;
    let login = tokio::task::spawn_blocking(move || {
        nexus_client::local_agents::login_with(agent, &program, &env, &cwd)
            .map_or(Login::Unknown, |(login, _, _)| login)
    })
    .await
    .map_err(|_| "Could not verify this computer's provider sign-in.")?;
    if matches!(
        (payer, login),
        (BillingPayer::Plan, Login::Plan) | (BillingPayer::ApiKey, Login::ApiKey)
    ) {
        Ok(())
    } else {
        Err("The provider sign-in changed. Restore this session's selected billing on the computer before continuing. Nexus did not switch who pays.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_apply_only_the_requested_agent_profile() {
        let env = HashMap::from([("PATH".into(), "bin".into())]);
        let profiles = AccountProfiles::from([
            (SessionAgent::Claude, PathBuf::from("claude-profile")),
            (SessionAgent::Codex, PathBuf::from("codex-profile")),
        ]);
        for (agent, key, folder) in [
            (SessionAgent::Claude, "CLAUDE_CONFIG_DIR", "claude-profile"),
            (SessionAgent::Codex, "CODEX_HOME", "codex-profile"),
        ] {
            let mut expected = env.clone();
            expected.insert(key.into(), folder.into());
            assert_eq!(profile_env(agent, &env, &profiles), expected);
        }
        assert_eq!(profile_env(SessionAgent::Nexus, &env, &profiles), env);
        let only_claude = AccountProfiles::from([(SessionAgent::Claude, PathBuf::from("claude-profile"))]);
        assert_eq!(profile_env(SessionAgent::Codex, &env, &only_claude), env);
    }
}
