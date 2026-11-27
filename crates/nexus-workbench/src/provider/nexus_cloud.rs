//! Nexus Cloud: every model call through Nexus Gateway, on the organization's models, budgets and policies.

use futures::future::BoxFuture;
use nexus_client::Client;
use nexus_local_agents::launch::{Route, credential};
use nexus_providers::{Credential, Funding, Health, InstanceId, ModelOffer, ProviderError, ProviderKind, Room};

use super::{
    HealthContext, LaunchContext, OfferContext, Prepared, Provider, RefusalContext, not_offered, open_gateway_session,
    sign_in,
};
use crate::event::{SessionAgent, Via};

/// The one Nexus Cloud provider, while signed in.
pub struct NexusCloudProvider {
    id: InstanceId,
    client: Client,
}

impl NexusCloudProvider {
    pub fn new(client: Client) -> NexusCloudProvider {
        NexusCloudProvider {
            id: InstanceId::nexus(),
            client,
        }
    }

    /// Nexus's own agent, and each coding agent Gateway has a route for.
    fn runs(&self, agent: SessionAgent) -> bool {
        agent
            .coding()
            .is_none_or(|coding| coding.entry().billing.gateway.is_some())
    }

    /// Whether this provider starts `agent` itself: a coding agent Gateway has a route for. Nexus's own agent runs on
    /// Cloud but starts on its own launch.
    fn launches(&self, agent: SessionAgent) -> Result<(), ProviderError> {
        if !self.runs(agent) {
            return Err(not_offered(agent, self.kind()));
        }
        if agent.coding().is_none() {
            return Err(ProviderError::Refused(format!(
                "{} starts on its own launch, not as a coding agent's session.",
                crate::driver::label(agent)
            )));
        }
        Ok(())
    }
}

impl Provider for NexusCloudProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::NexusCloud
    }

    fn id(&self) -> &InstanceId {
        &self.id
    }

    fn name(&self) -> String {
        ProviderKind::NexusCloud.entry().name.to_string()
    }

    fn agents(&self) -> Vec<SessionAgent> {
        SessionAgent::ALL
            .into_iter()
            .filter(|agent| self.runs(*agent))
            .collect()
    }

    /// The models the agent lists that Gateway's catalog prices, each once, paid through Gateway. Room is a plan's
    /// notion, so it is `Unknown`. Empty for Nexus's own agent, which runs on Gateway's model choice.
    fn offers<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a OfferContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<ModelOffer>, ProviderError>> {
        let mut offers: Vec<ModelOffer> = Vec::new();
        if let Some(coding) = agent.coding().filter(|_| self.runs(agent)) {
            for model in cx.installed {
                let runs = model.resolved_id.as_deref().unwrap_or(&model.id);
                if model.id == "default" || runs.is_empty() || offers.iter().any(|offer| offer.agent_model == runs) {
                    continue;
                }
                let Some(catalog) = (cx.catalog)(runs) else {
                    continue;
                };
                offers.push(ModelOffer {
                    agent: coding,
                    agent_model: runs.to_string(),
                    catalog: Some(catalog),
                    name: model.name.clone(),
                    funding: Funding::Gateway,
                    room: Room::Unknown,
                });
            }
        }
        Box::pin(async move { Ok(offers) })
    }

    fn refusal(&self, agent: SessionAgent, cx: &RefusalContext<'_>) -> Result<(), ProviderError> {
        if !self.runs(agent) {
            return Err(not_offered(agent, self.kind()));
        }
        crate::harness::harness(agent)
            .launch_refusal(Via::Nexus, cx.home)
            .map_err(ProviderError::Refused)
    }

    /// As a coding agent's session through Gateway starts today: the organization's policy, the model, and the Gateway
    /// session. Nexus's own agent starts on its own launch (`nexus::launch`), which opens no session here.
    fn prepare<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Prepared, ProviderError>> {
        Box::pin(async move {
            self.launches(agent)?;
            open_gateway_session(cx.route.gateway(agent)?, agent, Via::Nexus, cx).await
        })
    }

    /// Gateway's session, with the sign-in's key.
    fn credential<'a>(
        &'a self,
        agent: SessionAgent,
        prepared: &'a Prepared,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Credential, ProviderError>> {
        Box::pin(async move {
            self.launches(agent)?;
            let Prepared::Gateway { session_id, team, .. } = prepared else {
                return Err(ProviderError::Refused("Nexus Gateway did not open the session.".into()));
            };
            let (core_url, key) = sign_in(cx.route.gateway(agent)?.client, agent)?;
            Ok(credential(Route::Gateway, core_url, key, session_id, team.as_deref()))
        })
    }

    /// It exists only while signed in.
    fn local_health(&self, _cx: &HealthContext<'_>) -> Health {
        Health::Ready
    }

    /// Gateway's health route (`GET /healthz`), the only probe that calls a Nexus host.
    fn probe<'a>(&'a self, _cx: &'a HealthContext<'a>) -> BoxFuture<'a, Health> {
        Box::pin(async move {
            match self.client.get("/healthz").await {
                Ok(_) => Health::Ready,
                Err(_) => Health::Unreachable {
                    endpoint: self.client.options().base_url.clone(),
                },
            }
        })
    }
}
