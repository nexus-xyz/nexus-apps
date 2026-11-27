//! A coding agent on the person's own plan, signed in with the agent's own login or an extra account's folder.

use futures::future::BoxFuture;
use nexus_client::auto_route;
use nexus_local_agents::Login;
use nexus_local_agents::launch::{Route, credential};
use nexus_providers::{
    AccountHandle, Credential, Funding, Health, InstanceId, ModelOffer, ProviderError, ProviderKind, Room,
};

use super::{
    HealthContext, LaunchContext, LaunchRoute, OfferContext, Prepared, Provider, RefusalContext, not_offered,
    open_gateway_session, session_agent, sign_in, signed_out,
};
use crate::agent_launch::account_of;
use crate::event::{SessionAgent, Via};

/// One plan login of one agent: its own (`account` None) or an extra account.
pub struct CliSubscriptionProvider {
    id: InstanceId,
    agent: nexus_agents::Agent,
    account: Option<String>,
}

impl CliSubscriptionProvider {
    pub fn new(agent: nexus_agents::Agent, account: Option<&str>) -> CliSubscriptionProvider {
        let id = match account {
            Some(account) => InstanceId::subscription_account(agent, account),
            None => InstanceId::subscription_own(agent),
        };
        CliSubscriptionProvider {
            id,
            agent,
            account: account.map(String::from),
        }
    }

    /// The plan's models, as [`Provider::offers`] gives them, read now. Each model the agent runs is offered once,
    /// under the id it runs (its resolved id, else its own), in the agent's order. The agent's `default` entry and an
    /// empty id are left out. A model Gateway's catalog lacks is still offered, with no catalog id, and a model with no
    /// reading has `Unknown` room: Auto's request decides what to do with each (`agent_launch::plan_of`).
    pub fn plan_offers(&self, cx: &OfferContext<'_>) -> Vec<ModelOffer> {
        let mut offers: Vec<ModelOffer> = Vec::new();
        for model in cx.installed {
            let runs = model.resolved_id.as_deref().unwrap_or(&model.id);
            if model.id == "default" || runs.is_empty() || offers.iter().any(|offer| offer.agent_model == runs) {
                continue;
            }
            let room = match cx.windows {
                Some(windows) => auto_route::room(windows, &model.name, cx.now),
                None => Room::Unknown,
            };
            offers.push(ModelOffer {
                agent: self.agent,
                agent_model: runs.to_string(),
                catalog: (cx.catalog)(runs),
                name: model.name.clone(),
                // Auto's request names one account, the one the launch starts on.
                funding: Funding::Plan {
                    account: AccountHandle(0),
                },
                room,
            });
        }
        offers
    }

    fn runs(&self, agent: SessionAgent) -> bool {
        agent.coding() == Some(self.agent)
    }

    /// Whether this login starts `cx`'s session: its agent, on this account (the request's account folder, or none for
    /// the agent's own login).
    fn launches(&self, agent: SessionAgent, cx: &LaunchContext<'_>) -> Result<(), ProviderError> {
        if !self.runs(agent) {
            return Err(not_offered(agent, self.kind()));
        }
        if account_of(cx.request) != self.account {
            return Err(ProviderError::Refused(format!(
                "This session starts on another {} account than {}.",
                self.agent.entry().identity.name,
                self.name()
            )));
        }
        Ok(())
    }
}

impl Provider for CliSubscriptionProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CliSubscription
    }

    fn id(&self) -> &InstanceId {
        &self.id
    }

    fn name(&self) -> String {
        let name = self.agent.entry().identity.name;
        match &self.account {
            Some(account) => format!("{name} · {account}"),
            None => name.to_string(),
        }
    }

    fn agents(&self) -> Vec<SessionAgent> {
        session_agent(self.agent).into_iter().collect()
    }

    fn offers<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a OfferContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<ModelOffer>, ProviderError>> {
        let offers = if self.runs(agent) {
            self.plan_offers(cx)
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(offers) })
    }

    fn refusal(&self, agent: SessionAgent, cx: &RefusalContext<'_>) -> Result<(), ProviderError> {
        if !self.runs(agent) {
            return Err(not_offered(agent, self.kind()));
        }
        crate::harness::harness(agent)
            .launch_refusal(Via::Subscription, cx.home)
            .map_err(ProviderError::Refused)
    }

    /// Signed in, as a plan session starts today: the organization's policy, the model and a Gateway session for its
    /// usage reports. Without a Nexus sign-in the host can start a local subscription.
    fn prepare<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Prepared, ProviderError>> {
        Box::pin(async move {
            self.launches(agent, cx)?;
            match &cx.route {
                LaunchRoute::Gateway(gateway) => {
                    return open_gateway_session(gateway, agent, Via::Subscription, cx).await;
                }
                LaunchRoute::LocalSubscription(_) => {}
                LaunchRoute::SavedProvider { .. } => return Err(signed_out(agent)),
            }
            if cx.request.agent != agent
                || cx.request.via != Via::Subscription
                || !crate::agent_launch::supports_local_subscription(agent)
            {
                return Err(not_offered(agent, self.kind()));
            }
            self.refusal(agent, &RefusalContext { home: cx.home })?;
            crate::agent_launch::prepare_local(cx.request).map_err(ProviderError::Refused)
        })
    }

    /// The agent's own login, or the extra account's folder, reporting usage to the Gateway session when there is one.
    fn credential<'a>(
        &'a self,
        agent: SessionAgent,
        prepared: &'a Prepared,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Credential, ProviderError>> {
        Box::pin(async move {
            self.launches(agent, cx)?;
            let account_dir = cx.request.account_dir.clone();
            let (gateway, session_id) = match (&cx.route, prepared) {
                (LaunchRoute::Gateway(gateway), Prepared::Gateway { session_id, .. }) => (gateway, session_id),
                (LaunchRoute::LocalSubscription(_), Prepared::Local { .. }) => {
                    return Ok(Credential::AgentLogin {
                        account_dir,
                        usage: None,
                    });
                }
                (LaunchRoute::Gateway(_), Prepared::Local { .. })
                | (LaunchRoute::LocalSubscription(_), Prepared::Gateway { .. })
                | (LaunchRoute::SavedProvider { .. }, Prepared::Gateway { .. } | Prepared::Local { .. }) => {
                    return Err(signed_out(agent));
                }
            };
            let client = gateway.client;
            let (core_url, key) = sign_in(client, agent)?;
            match credential(Route::Subscription, core_url, key, session_id, None) {
                Credential::AgentLogin { usage, .. } => Ok(Credential::AgentLogin { account_dir, usage }),
                other => Ok(other),
            }
        })
    }

    fn local_health(&self, cx: &HealthContext<'_>) -> Health {
        let found = cx
            .machine
            .agents
            .iter()
            .find(|found| found.agent.coding() == Some(self.agent));
        let Some(found) = found.filter(|found| found.installed) else {
            return Health::Setup(ProviderError::AgentMissing(self.agent));
        };
        if self.account.is_some() {
            // An extra account's login is read when its session starts.
            return Health::Ready;
        }
        match found.login {
            Login::SignedOut => Health::Setup(ProviderError::SignedOut(self.agent)),
            Login::ApiKey => Health::Setup(ProviderError::WrongLogin(self.agent)),
            Login::Plan | Login::Unknown => Health::Ready,
        }
    }

    /// No network: a plan login is checked on this computer.
    fn probe<'a>(&'a self, cx: &'a HealthContext<'a>) -> BoxFuture<'a, Health> {
        let health = self.local_health(cx);
        Box::pin(async move { health })
    }
}
