//! How a session's agent is reached and paid. Each configured provider is one [`Provider`]: an agent's plan login, a
//! key, a model server, or Nexus Cloud. [`ProviderSet`] holds the ones this computer and its settings name. The values
//! they share (kinds, offers, credentials, settings) are `nexus-providers`'.
//!
//! A launch asks its provider in order: [`Provider::refusal`] before any process starts, [`Provider::prepare`] for the
//! model and the Gateway session when there is one, then [`Provider::credential`] for what the agent runs on. Each
//! agent turns the credential into its own variables (`LocalAgentT::credential_env`).

mod local_providers_opt_in;
pub use local_providers_opt_in::{LocalProviders, LocalProvidersOptIn};
mod byo;
mod nexus_cloud;
mod subscription;

pub(crate) use byo::is_loopback;
pub use byo::saved_provider_refusal;
pub use byo::{ApiKeyProvider, OpenAiCompatibleProvider, checked_server_base_url, server_base_url};
pub use nexus_cloud::NexusCloudProvider;
pub use nexus_providers::InstanceId;
pub use nexus_providers::KeySource;
pub use subscription::CliSubscriptionProvider;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use nexus_client::Client;
use nexus_client::auto_route::Window;
use nexus_providers::{
    CatalogModelId, Credential, Health, KindEntry, ModelOffer, ProviderError, ProviderKind, ProviderSettings,
};

use crate::LaunchRequest;
use crate::agent_launch::{AutoContext, SubscriptionAdmission};
use crate::agent_models::Model;
use crate::driver::label;
use crate::event::{SessionAgent, Via};

/// One configured provider. Every method is written by each implementation: there are no default bodies.
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn id(&self) -> &InstanceId;
    /// The name Settings and the composer show (`Claude Code`, `Claude Code · a_1700000000000`).
    fn name(&self) -> String;
    /// The session agents it runs.
    fn agents(&self) -> Vec<SessionAgent>;
    /// The models `agent` can run on it, with who pays and the room left. No network: a provider reads only `cx`.
    /// Empty for an agent it does not run.
    fn offers<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a OfferContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<ModelOffer>, ProviderError>>;
    /// Why `agent` cannot start on it, before any process starts. The agent's own checks are its harness's
    /// (`Harness::launch_refusal`), which this runs too.
    fn refusal(&self, agent: SessionAgent, cx: &RefusalContext<'_>) -> Result<(), ProviderError>;
    /// The model and the arguments the agent starts with. Signed in, it opens the session on Gateway
    /// (`POST /v1/agents/sessions`), the one Nexus call before a launch.
    fn prepare<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Prepared, ProviderError>>;
    /// What the agent runs on, for one spawn.
    fn credential<'a>(
        &'a self,
        agent: SessionAgent,
        prepared: &'a Prepared,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Credential, ProviderError>>;
    /// Whether it can run now, read on this computer only: the binary, the login, the key, the address.
    fn local_health(&self, cx: &HealthContext<'_>) -> Health;
    /// Whether it can run now, asking what it reaches. Only Nexus Cloud's calls a Nexus host.
    fn probe<'a>(&'a self, cx: &'a HealthContext<'a>) -> BoxFuture<'a, Health>;
}

/// What a provider reads to offer models.
pub struct OfferContext<'a> {
    /// The models the agent lists on this computer, in its order.
    pub installed: &'a [Model],
    /// The plan's last reading; None when this app has not read it.
    pub windows: Option<&'a [Window<'a>]>,
    /// Epoch milliseconds, for windows past their reset.
    pub now: i64,
    /// Gateway's catalog id for an agent's model id, when Gateway prices it.
    pub catalog: &'a (dyn Fn(&str) -> Option<CatalogModelId> + Send + Sync),
}

/// What a provider reads to refuse a launch early.
pub struct RefusalContext<'a> {
    /// The home folder the agent starts with.
    pub home: Option<&'a str>,
}

/// One launch, as the host gives it.
pub struct LaunchContext<'a> {
    pub request: &'a LaunchRequest,
    pub route: LaunchRoute<'a>,
    /// The agent's program, found on this computer.
    pub program: &'a Path,
    /// The environment the agent starts with.
    pub env: &'a HashMap<String, String>,
    pub home: Option<&'a str>,
}

pub enum LaunchRoute<'a> {
    Gateway(GatewayLaunch<'a>),
    LocalSubscription(SubscriptionAdmission),
    SavedProvider { keys: &'a Arc<dyn KeySource> },
}

pub struct GatewayLaunch<'a> {
    pub client: &'a Client,
    pub session: SessionFacts,
    pub auto: Option<AutoContext<'a>>,
}

impl<'a> LaunchRoute<'a> {
    fn gateway(&self, agent: SessionAgent) -> Result<&GatewayLaunch<'a>, ProviderError> {
        match self {
            Self::Gateway(gateway) => Ok(gateway),
            Self::LocalSubscription(_) | Self::SavedProvider { .. } => Err(signed_out(agent)),
        }
    }
}

/// What the host knows of a session as it opens. Gateway gets no login name and no folder name of this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    /// The person turned usage data on (`nexus_desktop_core::usage`): only then is the plan tier sent.
    pub usage_data: bool,
}

/// A launch after [`Provider::prepare`]. `args` already carry the model it chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// No Nexus call was made.
    Local { model: Option<String>, args: Vec<String> },
    /// Gateway opened the session.
    Gateway {
        session_id: String,
        model: Option<String>,
        team: Option<String>,
        args: Vec<String>,
    },
}

/// What this computer has of each agent, for [`ProviderSet::from_settings`] and the health checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineFacts {
    pub agents: Vec<MachineAgent>,
}

/// One coding agent on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineAgent {
    pub agent: SessionAgent,
    /// Its program was found.
    pub installed: bool,
    /// Its own login, as its sign-in check last read it.
    pub login: nexus_local_agents::Login,
    /// Its extra plan accounts, by the id `accounts::add` gave each.
    pub accounts: Vec<String>,
}

/// What a health check reads.
pub struct HealthContext<'a> {
    pub machine: &'a MachineFacts,
    pub keys: Option<&'a Arc<dyn KeySource>>,
}

/// The providers this computer and its settings name, in the order Settings lists them.
pub struct ProviderSet {
    providers: Vec<Box<dyn Provider>>,
}

impl ProviderSet {
    /// Every plan login of each coding agent on `machine` (its own, then each extra account), and Nexus Cloud when
    /// signed in. Include configured keys and servers when BYO is enabled.
    pub fn from_settings(
        settings: &ProviderSettings,
        machine: &MachineFacts,
        nexus: Option<&Client>,
        byo_enabled: bool,
    ) -> ProviderSet {
        let mut providers: Vec<Box<dyn Provider>> = Vec::new();
        for found in &machine.agents {
            let Some(agent) = found.agent.coding() else {
                continue;
            };
            providers.push(Box::new(CliSubscriptionProvider::new(agent, None)));
            for account in &found.accounts {
                providers.push(Box::new(CliSubscriptionProvider::new(agent, Some(account))));
            }
        }
        if byo_enabled {
            providers.extend(
                settings
                    .api_keys
                    .iter()
                    .cloned()
                    .map(|setting| Box::new(ApiKeyProvider::new(setting)) as Box<dyn Provider>),
            );
            providers.extend(
                settings
                    .servers
                    .iter()
                    .cloned()
                    .map(|setting| Box::new(OpenAiCompatibleProvider::new(setting)) as Box<dyn Provider>),
            );
        }
        if let Some(client) = nexus {
            providers.push(Box::new(NexusCloudProvider::new(client.clone())));
        }
        ProviderSet { providers }
    }

    pub fn get(&self, id: &InstanceId) -> Option<&dyn Provider> {
        self.iter().find(|provider| provider.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Provider> {
        self.providers.iter().map(|provider| provider.as_ref())
    }
}

/// What Settings shows of a kind before any instance of it exists.
pub trait ProviderUi {
    fn entry(&self) -> &'static KindEntry;
    /// One sentence on what the kind runs on.
    fn summary(&self) -> &'static str;
}

impl ProviderUi for ProviderKind {
    fn entry(&self) -> &'static KindEntry {
        ProviderKind::entry(*self)
    }

    fn summary(&self) -> &'static str {
        match self {
            ProviderKind::CliSubscription => "Each agent runs on your own plan, signed in with its own login.",
            ProviderKind::ApiKey => {
                "An agent runs on your own Anthropic or OpenAI key, kept in this computer's key store."
            }
            ProviderKind::OpenAiCompatible => {
                "An agent runs on a model server you choose, such as one on this computer."
            }
            ProviderKind::NexusCloud => "Agents run through Nexus Gateway, on your organization's models and policies.",
        }
    }
}

/// The session agent a coding agent is, when this build has it.
pub(crate) fn session_agent(agent: nexus_agents::Agent) -> Option<SessionAgent> {
    SessionAgent::ALL
        .into_iter()
        .find(|session| session.coding() == Some(agent))
}

/// The refusal for an agent a provider does not run.
pub(crate) fn not_offered(agent: SessionAgent, kind: ProviderKind) -> ProviderError {
    match agent.coding() {
        Some(coding) => ProviderError::NotOffered { agent: coding, kind },
        None => ProviderError::Refused(format!(
            "{} runs only on {}.",
            label(agent),
            ProviderKind::NexusCloud.entry().name
        )),
    }
}

/// The refusal for a launch with no Nexus sign-in, as the app words it today.
pub(crate) fn signed_out(agent: SessionAgent) -> ProviderError {
    ProviderError::Refused(format!("Sign in to Nexus to start {}.", label(agent)))
}

/// Today's launch on Gateway's side: the payer check and the model (`agent_launch::prepare_with_auto`), then the
/// session Gateway opens for it. `via` is the provider's; a request on another one is refused.
pub(crate) async fn open_gateway_session(
    gateway: &GatewayLaunch<'_>,
    agent: SessionAgent,
    via: Via,
    cx: &LaunchContext<'_>,
) -> Result<Prepared, ProviderError> {
    let request = cx.request;
    if request.agent != agent || request.via != via {
        return Err(ProviderError::Refused(format!(
            "This session does not start {} on {}.",
            label(agent),
            via.as_str()
        )));
    }
    let opened = crate::agent_launch::open_session(
        gateway.client,
        request,
        cx.program,
        cx.env.clone(),
        cx.home,
        gateway.auto.clone(),
        &gateway.session,
    )
    .await
    .map_err(ProviderError::Refused)?;
    Ok(Prepared::Gateway {
        session_id: opened.session_id,
        model: opened.model,
        team: opened.team,
        args: opened.args,
    })
}

/// The Nexus sign-in's address and key, for a Gateway credential.
pub(crate) fn sign_in(client: &Client, agent: SessionAgent) -> Result<(&str, &str), ProviderError> {
    let options = client.options();
    let key = options.api_key.as_deref().ok_or_else(|| signed_out(agent))?;
    Ok((&options.base_url, key))
}

#[cfg(test)]
mod tests;
