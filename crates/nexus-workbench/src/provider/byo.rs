//! Configured local providers. This layer can check a model endpoint without launching an agent.

use std::sync::Arc;
use std::time::Duration;

use nexus_agents::Agent;

use nexus_providers::{ApiKeySetting, Funding, KeyRef, KeySource, KeyVendor, Room, Secret, ServerSetting};

use super::*;

const SIGN_OUT: &str = "Sign out of Nexus to use this provider on this computer.";

fn check_launch<'a>(cx: &LaunchContext<'a>, id: &InstanceId) -> Result<&'a Arc<dyn KeySource>, ProviderError> {
    if !cx.request.scope.is_local() {
        return Err(ProviderError::Refused(SIGN_OUT.into()));
    }
    let keys = match &cx.route {
        LaunchRoute::Gateway(_) => return Err(ProviderError::Refused(SIGN_OUT.into())),
        LaunchRoute::LocalSubscription(_) => return Err(ProviderError::Refused(saved_provider_refusal(false))),
        LaunchRoute::SavedProvider { keys } => *keys,
    };
    if cx.request.provider.as_ref() != Some(id) || cx.request.account_dir.is_some() {
        return Err(ProviderError::Refused(saved_provider_refusal(false)));
    }
    if matches!(
        cx.request.required_payer,
        Some(crate::computer::BillingPayer::Gateway | crate::computer::BillingPayer::Plan)
    ) {
        return Err(ProviderError::Refused(
            "This session requires a different payer.".into(),
        ));
    }
    Ok(keys)
}

pub fn saved_provider_refusal(signed_in: bool) -> String {
    if signed_in {
        SIGN_OUT.into()
    } else {
        "This computer cannot start a local provider session.".into()
    }
}

pub fn server_base_url(address: &str) -> Option<String> {
    let url = url::Url::parse(address.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host == "localhost.",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback()),
        None => false,
    }
}

/// Normalize an address and check whether it can carry a saved server key.
pub fn checked_server_base_url(address: &str, keyed: bool) -> Result<String, ProviderError> {
    let base = server_base_url(address).ok_or_else(invalid_server)?;
    let url = url::Url::parse(&base).map_err(|_| invalid_server())?;
    if keyed && url.scheme() == "http" && !is_loopback(&url) {
        return Err(ProviderError::Refused(
            "Use HTTPS or a loopback address for a server with a saved key.".into(),
        ));
    }
    Ok(base)
}

fn health_for_agent(agent: Agent, machine: &MachineFacts) -> Health {
    if machine
        .agents
        .iter()
        .any(|found| found.installed && found.agent.coding() == Some(agent))
    {
        Health::Ready
    } else {
        Health::Setup(ProviderError::AgentMissing(agent))
    }
}

fn offers(
    agent: Agent,
    funding: Funding,
    models: impl Iterator<Item = (String, String)>,
    cx: &OfferContext<'_>,
) -> Vec<ModelOffer> {
    let mut seen = std::collections::HashSet::new();
    models
        .filter_map(|(id, name)| {
            if id.is_empty() || id == "default" || !seen.insert(id.clone()) {
                return None;
            }
            Some(ModelOffer {
                agent,
                catalog: (cx.catalog)(&id),
                agent_model: id,
                name,
                funding,
                room: Room::Unknown,
            })
        })
        .collect()
}

async fn read_key(keys: Option<&Arc<dyn KeySource>>, key: &KeyRef) -> Result<Option<Secret>, ProviderError> {
    let keys = keys
        .ok_or_else(|| ProviderError::KeychainUnread("Nexus cannot open the OS key store.".into()))?
        .clone();
    let key = key.clone();
    tokio::task::spawn_blocking(move || keys.read(&key))
        .await
        .map_err(|_| ProviderError::KeychainUnread("Nexus could not read the provider key.".into()))?
}

async fn check_models(
    base: &str,
    vendor: Option<KeyVendor>,
    key: Option<&Secret>,
) -> Result<Vec<String>, ProviderError> {
    let base = checked_server_base_url(base, key.is_some())?;
    let endpoint = format!("{base}/models");
    let unreachable = || ProviderError::Unreachable {
        endpoint: endpoint.clone(),
    };
    nexus_client::install_tls();
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if url::Url::parse(&base).is_ok_and(|url| is_loopback(&url)) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| unreachable())?;
    let mut request = client.get(&endpoint).timeout(Duration::from_secs(5));
    if let Some(key) = key {
        request = if vendor == Some(KeyVendor::Anthropic) {
            request
                .header("x-api-key", key.expose())
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(key.expose())
        };
    }
    let response = request.send().await.map_err(|_| unreachable())?;
    match response.status().as_u16() {
        200..=299 if vendor.is_none() => Ok(model_ids(response).await),
        200..=299 => Ok(vec![]),
        401 | 403 => Err(ProviderError::Refused(
            "The provider refused this credential. Check the saved key.".into(),
        )),
        _ => Err(unreachable()),
    }
}

async fn model_ids(mut response: reqwest::Response) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct ModelList {
        data: Vec<Model>,
    }
    #[derive(serde::Deserialize)]
    struct Model {
        id: String,
    }

    // A successful endpoint can omit a usable list. Bound the optional body independently of its headers.
    const MAX_BYTES: usize = 1024 * 1024;
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_BYTES - body.len() => body.extend_from_slice(&chunk),
            Ok(None) => break,
            _ => return vec![],
        }
    }
    let Ok(list) = serde_json::from_slice::<ModelList>(&body) else {
        return vec![];
    };
    let mut ids: Vec<_> = list
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| !id.trim().is_empty())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn probe_health(result: Result<Vec<String>, ProviderError>) -> Health {
    match result {
        Ok(_) => Health::Ready,
        Err(ProviderError::Unreachable { endpoint }) => Health::Unreachable { endpoint },
        Err(error) => Health::Setup(error),
    }
}

pub struct ApiKeyProvider {
    setting: ApiKeySetting,
}

impl ApiKeyProvider {
    pub fn new(setting: ApiKeySetting) -> Self {
        Self { setting }
    }

    fn agent(&self) -> Agent {
        match self.setting.vendor {
            KeyVendor::Anthropic => Agent::Claude,
            KeyVendor::Openai => Agent::Codex,
        }
    }
}

impl Provider for ApiKeyProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ApiKey
    }
    fn id(&self) -> &InstanceId {
        &self.setting.id
    }
    fn name(&self) -> String {
        format!("{} key", self.setting.vendor.name())
    }
    fn agents(&self) -> Vec<SessionAgent> {
        session_agent(self.agent()).into_iter().collect()
    }

    fn offers<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a OfferContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<ModelOffer>, ProviderError>> {
        Box::pin(async move {
            if agent.coding() != Some(self.agent()) {
                return Ok(vec![]);
            }
            let models = cx.installed.iter().map(|model| {
                (
                    model.resolved_id.as_ref().unwrap_or(&model.id).clone(),
                    model.name.clone(),
                )
            });
            Ok(offers(self.agent(), Funding::LocalKey, models, cx))
        })
    }

    fn refusal(&self, agent: SessionAgent, _cx: &RefusalContext<'_>) -> Result<(), ProviderError> {
        if agent.coding() == Some(self.agent()) {
            Ok(())
        } else {
            Err(not_offered(agent, self.kind()))
        }
    }

    fn prepare<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Prepared, ProviderError>> {
        Box::pin(async move {
            check_launch(cx, self.id())?;
            if agent.coding() != Some(self.agent()) {
                return Err(not_offered(agent, self.kind()));
            }
            crate::agent_launch::prepare_saved_key(cx.request).map_err(ProviderError::Refused)
        })
    }

    fn credential<'a>(
        &'a self,
        agent: SessionAgent,
        _prepared: &'a Prepared,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Credential, ProviderError>> {
        Box::pin(async move {
            let keys = check_launch(cx, self.id())?;
            if agent.coding() != Some(self.agent()) {
                return Err(not_offered(agent, self.kind()));
            }
            let vendor = self.setting.vendor;
            let secret = read_key(Some(keys), &self.setting.key)
                .await?
                .ok_or(ProviderError::KeyMissing(vendor))?;
            Ok(Credential::VendorKey { vendor, secret })
        })
    }

    fn local_health(&self, cx: &HealthContext<'_>) -> Health {
        health_for_agent(self.agent(), cx.machine)
    }

    fn probe<'a>(&'a self, cx: &'a HealthContext<'_>) -> BoxFuture<'a, Health> {
        Box::pin(async move {
            let vendor = self.setting.vendor;
            let key = match read_key(cx.keys, &self.setting.key).await {
                Ok(Some(key)) => key,
                Ok(None) => return Health::Setup(ProviderError::KeyMissing(vendor)),
                Err(error) => return Health::Setup(error),
            };
            let base = match vendor {
                KeyVendor::Anthropic => "https://api.anthropic.com/v1",
                KeyVendor::Openai => "https://api.openai.com/v1",
            };
            probe_health(check_models(base, Some(vendor), Some(&key)).await)
        })
    }
}

fn server_model(models: &[String], selected: Option<&str>) -> Result<String, ProviderError> {
    let selected = selected.filter(|model| !model.trim().is_empty());
    match (selected, models) {
        (Some(model), _) if !model.trim().is_empty() => Ok(model.into()),
        (None, [model]) if !model.trim().is_empty() => Ok(model.clone()),
        _ => Err(ProviderError::Refused(
            "Choose a model for this server before starting the session.".into(),
        )),
    }
}

pub struct OpenAiCompatibleProvider {
    setting: ServerSetting,
}

impl OpenAiCompatibleProvider {
    pub fn new(setting: ServerSetting) -> Self {
        Self { setting }
    }
}

impl Provider for OpenAiCompatibleProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }
    fn id(&self) -> &InstanceId {
        &self.setting.id
    }
    fn name(&self) -> String {
        self.setting.name.clone()
    }
    fn agents(&self) -> Vec<SessionAgent> {
        session_agent(Agent::Opencode).into_iter().collect()
    }

    fn offers<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a OfferContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<ModelOffer>, ProviderError>> {
        Box::pin(async move {
            if agent.coding() != Some(Agent::Opencode) {
                return Ok(vec![]);
            }
            let models = self.setting.models.iter().map(|name| (name.clone(), name.clone()));
            Ok(offers(Agent::Opencode, Funding::OwnServer, models, cx))
        })
    }

    fn refusal(&self, agent: SessionAgent, _cx: &RefusalContext<'_>) -> Result<(), ProviderError> {
        if agent.coding() == Some(Agent::Opencode) {
            Ok(())
        } else {
            Err(not_offered(agent, self.kind()))
        }
    }

    fn prepare<'a>(
        &'a self,
        agent: SessionAgent,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Prepared, ProviderError>> {
        Box::pin(async move {
            check_launch(cx, self.id())?;
            if agent.coding() != Some(Agent::Opencode) {
                return Err(not_offered(agent, self.kind()));
            }
            checked_server_base_url(&self.setting.base_url, self.setting.key.is_some())?;
            let model = server_model(&self.setting.models, cx.request.model.as_deref())?;
            Ok(Prepared::Local {
                model: Some(model),
                args: cx.request.args.clone(),
            })
        })
    }

    fn credential<'a>(
        &'a self,
        agent: SessionAgent,
        _prepared: &'a Prepared,
        cx: &'a LaunchContext<'a>,
    ) -> BoxFuture<'a, Result<Credential, ProviderError>> {
        Box::pin(async move {
            let keys = check_launch(cx, self.id())?;
            if agent.coding() != Some(Agent::Opencode) {
                return Err(not_offered(agent, self.kind()));
            }
            let base_url = checked_server_base_url(&self.setting.base_url, self.setting.key.is_some())?;
            let secret = match &self.setting.key {
                None => None,
                Some(key) => Some(read_key(Some(keys), key).await?.ok_or_else(|| {
                    ProviderError::Refused(
                        "No key is saved for this server. Add its key in Settings under Local providers.".into(),
                    )
                })?),
            };
            let provider_id = nexus_local_agents::agents::opencode_server_provider(&cx.request.thread_id);
            Ok(Credential::Server {
                provider_id,
                base_url,
                secret,
            })
        })
    }

    fn local_health(&self, cx: &HealthContext<'_>) -> Health {
        if let Err(error) = checked_server_base_url(&self.setting.base_url, self.setting.key.is_some()) {
            return Health::Setup(error);
        }
        health_for_agent(Agent::Opencode, cx.machine)
    }

    fn probe<'a>(&'a self, cx: &'a HealthContext<'_>) -> BoxFuture<'a, Health> {
        Box::pin(async move { probe_health(self.check(cx).await) })
    }
}

impl OpenAiCompatibleProvider {
    /// Check the saved endpoint and return the model IDs it advertises, if any.
    pub async fn check(&self, cx: &HealthContext<'_>) -> Result<Vec<String>, ProviderError> {
        let base = checked_server_base_url(&self.setting.base_url, self.setting.key.is_some())?;
        let key = match self.setting.key.as_ref() {
            Some(reference) => Some(read_key(cx.keys, reference).await?.ok_or_else(|| {
                ProviderError::Refused("The server key is missing. Add it in Local providers.".into())
            })?),
            None => None,
        };
        check_models(&base, None, key.as_ref()).await
    }
}

fn invalid_server() -> ProviderError {
    ProviderError::Refused("Use an HTTP or HTTPS server address without credentials, a query, or a fragment.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_client::{Client, ClientOptions};

    #[test]
    fn configured_providers_enter_the_set_only_behind_the_flag() {
        let settings = ProviderSettings {
            api_keys: vec![ApiKeySetting {
                id: InstanceId::new("key:1".into()),
                vendor: KeyVendor::Anthropic,
                key: KeyRef::new("reference".into()),
            }],
            servers: vec![ServerSetting {
                id: InstanceId::new("server:1".into()),
                name: "Local".into(),
                base_url: "http://localhost:1234/v1".into(),
                key: None,
                models: vec!["model".into()],
            }],
            ..ProviderSettings::default()
        };
        let machine = MachineFacts {
            agents: vec![MachineAgent {
                agent: SessionAgent::Claude,
                installed: true,
                login: nexus_local_agents::Login::Plan,
                accounts: vec![],
            }],
        };
        let client = Client::new(ClientOptions::new("http://localhost:1"));
        let off = ProviderSet::from_settings(&settings, &machine, Some(&client), false);
        assert!(off.get(&settings.api_keys[0].id).is_none());
        assert!(off.get(&settings.servers[0].id).is_none());
        let on = ProviderSet::from_settings(&settings, &machine, Some(&client), true);
        for kind in ProviderKind::ALL {
            assert!(on.iter().any(|provider| provider.kind() == kind), "{kind:?}");
        }
        let key = on.get(&settings.api_keys[0].id).expect("stored key");
        assert_eq!(key.agents(), [SessionAgent::Claude]);
        assert_eq!(
            key.local_health(&HealthContext {
                machine: &machine,
                keys: None
            }),
            Health::Ready
        );
    }

    #[test]
    fn server_model_uses_the_saved_choice_or_a_single_default() {
        assert_eq!(
            server_model(&["team/model".into()], None).expect("single saved model"),
            "team/model"
        );
        assert_eq!(
            server_model(&["later".into()], Some("team/model")).expect("model saved with the session"),
            "team/model"
        );
        assert!(server_model(&[], None).is_err());
        assert!(server_model(&["a".into(), "b".into()], None).is_err());
    }

    #[test]
    fn server_addresses_exclude_credentials_and_non_http_schemes() {
        assert_eq!(
            server_base_url("http://localhost:123/v1/").as_deref(),
            Some("http://localhost:123/v1")
        );
        for url in [
            "file:///tmp/model",
            "http://alice:secret@example.com",
            "https://example.com/?key=secret",
        ] {
            assert_eq!(server_base_url(url), None);
        }
    }

    #[cfg(feature = "opencode")]
    #[tokio::test]
    async fn server_with_a_key_refuses_cleartext_before_reading_the_key() {
        for base in ["http://server.invalid/v1", "http://localhost.example/v1"] {
            assert_eq!(checked_server_base_url(base, false).expect("keyless address"), base);
            let provider = OpenAiCompatibleProvider::new(ServerSetting {
                id: InstanceId::new("server:1".into()),
                name: "Server".into(),
                base_url: base.into(),
                key: Some(KeyRef::new("fake-key-reference".into())),
                models: vec![],
            });
            let reason =
                ProviderError::Refused("Use HTTPS or a loopback address for a server with a saved key.".into());
            let health = HealthContext {
                machine: &MachineFacts::default(),
                keys: None,
            };
            assert_eq!(provider.probe(&health).await, Health::Setup(reason.clone()));
            let request = LaunchRequest {
                thread_id: "test".into(),
                agent: SessionAgent::Opencode,
                cwd: ".".into(),
                model: None,
                via: Via::Subscription,
                required_payer: None,
                provider: Some(provider.id().clone()),
                account_dir: None,
                args: vec![],
                scope: crate::session_scope::SessionScope::Local,
                prompt_counts: None,
            };
            let keys: Arc<dyn KeySource> = Arc::new(crate::saved_provider_tests::Keys::default());
            let launch = LaunchContext {
                request: &request,
                route: LaunchRoute::SavedProvider { keys: &keys },
                program: Path::new("opencode"),
                env: &HashMap::new(),
                home: None,
            };
            assert_eq!(provider.prepare(SessionAgent::Opencode, &launch).await, Err(reason));
        }
    }

    #[tokio::test]
    async fn a_keyless_http_server_can_be_checked() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("server");
        let addr = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (conn, _) = listener.accept().await.expect("request");
            let mut conn = tokio::io::BufReader::new(conn);
            let mut line = String::new();
            conn.read_line(&mut line).await.expect("request line");
            assert!(line.starts_with("GET /v1/models "));
            loop {
                line.clear();
                assert_ne!(conn.read_line(&mut line).await.expect("header"), 0, "unexpected EOF");
                if line == "\r\n" {
                    break;
                }
                assert!(!line.to_ascii_lowercase().starts_with("authorization:"));
            }
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("reply");
        });
        let provider = OpenAiCompatibleProvider::new(ServerSetting {
            id: InstanceId::new("server:1".into()),
            name: "Server".into(),
            base_url: format!("http://{addr}/v1"),
            key: None,
            models: vec![],
        });
        assert_eq!(
            provider
                .probe(&HealthContext {
                    machine: &MachineFacts::default(),
                    keys: None
                })
                .await,
            Health::Ready
        );
        server.await.expect("keyless server");
    }

    #[tokio::test]
    async fn server_check_stays_on_its_configured_origin() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("server");
        let addr = listener.local_addr().expect("address");
        let target = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("redirect target");
        let redirected = target.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.expect("check");
            let mut buf = vec![0; 1024];
            let n = conn.read(&mut buf).await.expect("read");
            let request = std::str::from_utf8(&buf[..n]).expect("HTTP");
            assert!(request.starts_with("GET /v1/models "));
            assert!(request.to_ascii_lowercase().contains("authorization: bearer fake-key"));
            let reply = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{redirected}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            conn.write_all(reply.as_bytes()).await.expect("respond");
        });
        assert!(matches!(
            check_models(
                &format!("http://{addr}/v1"),
                None,
                Some(&Secret::new("fake-key".into()))
            )
            .await,
            Err(ProviderError::Unreachable { .. })
        ));
        server.await.expect("fake server");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), target.accept())
                .await
                .is_err()
        );
    }
}
