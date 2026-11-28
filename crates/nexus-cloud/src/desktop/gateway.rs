//! Cloud machines through Gateway: Nexus
//! holds the provider account, so the app never sees a provider key. Every call goes to Gateway's
//! `/v1/cloud/machines` with the person's Nexus sign-in (`nexus_client::Client`: the token from the keychain, the
//! organization it belongs to, an `Idempotency-Key` on every write), and Gateway calls boat.
//!
//! [`GatewayCloud`] is `nexus_cloud::Provider` for the cloud link (wake, wait, register this device's SSH key, pin the
//! host key), plus what only Gateway has: the person's machines ([`GatewayCloud::list`]), and a create for one desktop
//! session that answers the session's machine at once, or a place in the queue when Nexus has no room yet.
//! Gateway decides when a machine sleeps, from its host's heartbeat.

use std::time::Duration;

use crate::{Error, ExecOutput, Machine, Provider, Result, Size, Spec, SshAccess, State};
use futures::future::BoxFuture;
use nexus_client::{Client, CoreError, Error as ClientError, Method, Transport};
use nexus_remote::relay::RelayLink;
use serde_json::{Value, json};

use super::history::{HistoryPage, Page};

/// Gateway's machine (`{id, name, sessionId, title, provider, size, state, error, createdAt, sleepingSince, queuedUntil,
/// stopReason, hostOnline}`).
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayMachine {
    pub id: String,
    pub name: String,
    /// The desktop session it is for (`t_<request id>`); None on a machine made before sessions had their own.
    pub session_id: Option<String>,
    /// The session's title, for the other devices' sidebars.
    pub title: Option<String>,
    pub provider: String,
    pub size: String,
    /// The screens' `CloudState` (`queued`, `creating`, `ready`, `sleeping`…).
    pub state: String,
    pub error: Option<String>,
    /// Unix ms.
    pub created_at: Option<i64>,
    pub sleeping_since: Option<i64>,
    /// While queued: when Nexus expects room for it (unix ms).
    pub queued_until: Option<i64>,
    /// Why it stopped (`idle`, `resize`, …), while it is stopped.
    pub stop_reason: Option<String>,
    /// Whether its host's line to Gateway is up (the relay), so the app reaches it through Gateway.
    /// None from a Gateway that does not say (before the relay): the app reaches it over SSH at once.
    pub host_online: Option<bool>,
}

/// Whether cloud machines can be used at all, as Gateway answered: the screens' `CloudAvailability`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// 503 `cloud_unavailable`: this Gateway has no provider account.
    Unconfigured,
    /// 403: the person's role may not use cloud machines in this organization. An organization without cloud machines
    /// (403 `cloud_not_enabled`) is [`Unavailable::Unconfigured`].
    NotAllowed,
    /// No Nexus sign-in on this computer.
    SignedOut,
    /// 503 with any other code: Gateway or the provider is struggling. The waits retry; this says so in Settings.
    Degraded,
}

impl Unavailable {
    pub fn reason(&self) -> &'static str {
        match self {
            Unavailable::Unconfigured => "unconfigured",
            Unavailable::NotAllowed => "not_allowed",
            Unavailable::SignedOut => "signed_out",
            Unavailable::Degraded => "degraded",
        }
    }

    /// What a refused call means for availability, if anything.
    pub fn of(error: &Error) -> Option<Unavailable> {
        match error {
            Error::Api { code, .. } if code == "cloud_unavailable" || code == "cloud_not_enabled" => {
                Some(Unavailable::Unconfigured)
            }
            Error::Api { status: 403, .. } => Some(Unavailable::NotAllowed),
            Error::Api { status: 503, .. } => Some(Unavailable::Degraded),
            Error::Unauthorized(_) => Some(Unavailable::SignedOut),
            _ => None,
        }
    }
}

/// The time Gateway sends as unix ms or RFC 3339, as unix ms.
fn time(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(text) => super::parse_time(text),
        _ => None,
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

impl GatewayMachine {
    pub fn from_json(value: &Value) -> Option<GatewayMachine> {
        let id = text(value, "id").filter(|id| valid_id(id))?;
        Some(GatewayMachine {
            name: text(value, "name").unwrap_or_else(|| "Cloud".into()),
            provider: text(value, "provider").unwrap_or_else(|| "boat".into()),
            size: text(value, "size").unwrap_or_else(|| Size::default().as_str().into()),
            state: text(value, "state").unwrap_or_else(|| "starting".into()),
            error: text(value, "error"),
            session_id: text(value, "sessionId").filter(|s| !s.is_empty()),
            title: text(value, "title").filter(|t| !t.trim().is_empty()),
            created_at: time(value.get("createdAt")),
            sleeping_since: time(value.get("sleepingSince")),
            queued_until: time(value.get("queuedUntil")),
            stop_reason: text(value, "stopReason").filter(|r| !r.is_empty()),
            // Present: a bool, and anything else counts as false.
            host_online: value.get("hostOnline").map(|online| online.as_bool().unwrap_or(false)),
            id,
        })
    }

    /// As nexus-cloud's [`Machine`], for the link's waits. A state this app does not know yet counts as starting, so a
    /// wait goes on rather than failing. A queued machine counts as being created (`provider_state` keeps `queued`).
    pub fn machine(&self) -> Machine {
        let state = match self.state.as_str() {
            "creating" | "queued" => State::Creating,
            "ready" => State::Ready,
            "sleeping" => State::Sleeping,
            "stopping" => State::Stopping,
            "failed" => State::Failed {
                message: self
                    .error
                    .clone()
                    .unwrap_or_else(|| "the provider gave no reason".into()),
            },
            "gone" => State::Gone,
            _ => State::Starting,
        };
        Machine {
            id: self.id.clone(),
            name: self.name.clone(),
            state,
            provider_state: self.state.clone(),
            size: Size::parse(&self.size),
            labels: Default::default(),
            stopped_at: self.sleeping_since.map(|ms| ms.to_string()),
            data_expired_at: None,
        }
    }
}

const SIGN_IN: &str = "Sign in to Nexus again to use your cloud machine.";

/// Gateway's machine ids go in a path: letters, digits, `_` and `-` only.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn checked(id: &str) -> Result<&str> {
    if valid_id(id) {
        Ok(id)
    } else {
        Err(Error::Other(format!("{id:?} is not a cloud machine id.")))
    }
}

/// A failed Gateway call as nexus-cloud's [`Error`]: the provider's codes Gateway passes on (`machine_not_running`)
/// stay, so the link still waits for a machine that is starting.
pub fn error(error: ClientError) -> Error {
    match error {
        ClientError::Core(core) => {
            let CoreError {
                status, message, code, ..
            } = *core;
            let code = code.unwrap_or_default();
            match (status, code.as_str()) {
                (401, _) => Error::Unauthorized(SIGN_IN.into()),
                (404, _) => Error::NotFound,
                (409, "snapshot_data_expired") => Error::DataExpired,
                (503, "cloud_unavailable") => Error::Api {
                    status,
                    code,
                    message: "Cloud machines aren't available on this Nexus yet.".into(),
                },
                (503, _) => Error::Api {
                    status,
                    code,
                    message: "Cloud machines are having trouble right now. Nexus keeps trying.".into(),
                },
                (402, "budget_exceeded") => Error::OverBudget,
                (403, "cloud_not_enabled") => Error::Api {
                    status,
                    code,
                    message: "Cloud machines are not turned on for this organization.".into(),
                },
                (403, _) => Error::Api {
                    status,
                    code,
                    message: "Your role in this organization can't use cloud machines.".into(),
                },
                _ => Error::Api { status, code, message },
            }
        }
        ClientError::Transport(Transport::Timeout) => Error::Network("Nexus did not answer in time.".into()),
        ClientError::Transport(other) => Error::Network(ClientError::Transport(other).to_string()),
    }
}

/// The create's header with the person's own GitHub token: Gateway's `warm::GITHUB_TOKEN_HEADER`.
const GITHUB_TOKEN_HEADER: &str = "x-nexus-github-token";

/// Where Gateway keeps the person's Claude setup token: write-only, never read back.
const CLAUDE_TOKEN_PATH: &str = "/v1/secrets/me/claude-token";

/// The cloud machines API of the Gateway the app is signed in to.
#[derive(Clone)]
pub struct GatewayCloud {
    client: Client,
}

impl GatewayCloud {
    /// Over the app's sign-in; None when signed out.
    pub fn new(client: Client) -> Option<GatewayCloud> {
        client.signed_in().then_some(GatewayCloud { client })
    }

    /// The person's machines in the current organization.
    pub async fn list(&self) -> Result<Vec<GatewayMachine>> {
        let answer = self.client.get("/v1/cloud/machines").await.map_err(error)?;
        let items = match &answer {
            Value::Array(items) => items.clone(),
            other => ["machines", "data"]
                .iter()
                .find_map(|key| other.get(key).and_then(Value::as_array).cloned())
                .unwrap_or_default(),
        };
        Ok(items.iter().filter_map(GatewayMachine::from_json).collect())
    }

    /// The machine for desktop session `session_id`: Gateway makes one, or answers the one the session has. It answers
    /// at once (`creating`, or `queued` when Nexus has no room yet), and it becomes ready later. `repo` is the
    /// repository the session clones: Gateway may start the machine from a saved copy of it, only when
    /// GitHub lets the person read it. `github` is the person's own GitHub token, which Gateway asks GitHub that with
    /// (`x-nexus-github-token`). It goes only with a repository.
    pub async fn create_machine(
        &self,
        session_id: &str,
        title: &str,
        repo: Option<&str>,
        github: Option<&str>,
    ) -> Result<GatewayMachine> {
        let title = title.trim();
        let mut body = json!({ "sessionId": session_id, "title": (!title.is_empty()).then_some(title) });
        let mut headers = Vec::new();
        if let Some(repo) = repo.map(str::trim).filter(|repo| !repo.is_empty()) {
            body["repo"] = json!(repo);
            if let Some(github) = github.map(str::trim).filter(|github| !github.is_empty()) {
                headers.push((GITHUB_TOKEN_HEADER.to_string(), github.to_string()));
            }
        }
        let answer = self
            .client
            .call_with(Method::POST, "/v1/cloud/machines", &headers, Some(body))
            .await
            .map_err(error)?;
        GatewayMachine::from_json(&answer).ok_or_else(|| Error::Other("Nexus answered no machine.".into()))
    }

    pub async fn machine(&self, id: &str) -> Result<GatewayMachine> {
        let id = checked(id)?;
        let answer = self
            .client
            .get(&format!("/v1/cloud/machines/{id}"))
            .await
            .map_err(error)?;
        GatewayMachine::from_json(&answer).ok_or_else(|| Error::Other("Nexus answered no machine.".into()))
    }

    /// One page of thread `thread`'s history on machine `id`: the events the machine sent Gateway, readable
    /// while it sleeps. Reading it never wakes the machine. A 404 (a Gateway without history yet, or a machine that is
    /// not the person's) reads as nothing stored, so the app reads the machine as before.
    pub async fn history(&self, id: &str, thread: &str, page: Page, limit: usize) -> Result<HistoryPage> {
        let (id, thread) = (checked(id)?, checked(thread)?);
        let query = page.query(limit);
        match self
            .client
            .get(&format!("/v1/cloud/machines/{id}/history/{thread}/events?{query}"))
            .await
            .map_err(error)
        {
            Ok(answer) => Ok(HistoryPage::from_json(&answer)),
            Err(Error::NotFound) => Ok(HistoryPage {
                enabled: true,
                ..HistoryPage::default()
            }),
            Err(error) => Err(error),
        }
    }

    /// The app's way to machine `id`'s host through Gateway (`GET …/connect`, the relay), with the
    /// same sign-in and client headers as every call here.
    pub fn connect(&self, id: &str) -> Result<RelayLink> {
        let id = checked(id)?;
        let options = self.client.options();
        let token = options
            .api_key
            .as_deref()
            .ok_or_else(|| Error::Unauthorized(SIGN_IN.into()))?;
        let mut link = RelayLink::new(self.client.base(), id, token).map_err(Error::Other)?;
        link.headers.extend(options.headers.iter().cloned());
        if let Some(client) = &options.client {
            link.headers.push((nexus_client::CLIENT_HEADER.into(), client.clone()));
        }
        if let Some(agent) = &options.user_agent {
            link.headers.push(("user-agent".into(), agent.clone()));
        }
        Ok(link)
    }

    /// Keeps the person's Claude setup token on Gateway: every cloud machine of theirs gets it from there,
    /// whichever device started it. False when this Gateway does not keep it yet (it answers 404).
    pub async fn put_claude_token(&self, token: &str) -> Result<bool> {
        let body = json!({ "token": token.trim() });
        match self
            .client
            .call_with(Method::PUT, CLAUDE_TOKEN_PATH, &[], Some(body))
            .await
            .map_err(error)
        {
            Ok(_) => Ok(true),
            Err(Error::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Removes the person's Claude setup token from Gateway. A 404 (none kept, or a Gateway that keeps none) is done.
    pub async fn delete_claude_token(&self) -> Result<()> {
        match self
            .client
            .call_with(Method::DELETE, CLAUDE_TOKEN_PATH, &[], None)
            .await
            .map_err(error)
        {
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn action(&self, id: &str, action: &'static str) -> Result<()> {
        let id = checked(id)?;
        let path = match action {
            "wake" => {
                self.client
                    .post(&format!("/v1/cloud/machines/{id}/wake"), json!({}))
                    .await
            }
            _ => {
                self.client
                    .post(&format!("/v1/cloud/machines/{id}/sleep"), json!({}))
                    .await
            }
        };
        path.map(|_| ()).map_err(error)
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let id = checked(id)?;
        match self
            .client
            .delete(&format!("/v1/cloud/machines/{id}"))
            .await
            .map_err(error)
        {
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn access(&self, id: &str, public_key: &str) -> Result<SshAccess> {
        let id = checked(id)?;
        let answer = self
            .client
            .post(
                &format!("/v1/cloud/machines/{id}/access"),
                json!({ "publicKey": public_key.trim() }),
            )
            .await
            .map_err(error)?;
        let port = answer
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or(22);
        match (text(&answer, "host"), text(&answer, "user"), text(&answer, "hostKey")) {
            (Some(host), Some(user), Some(host_key)) if !host.is_empty() && host_key.starts_with("ssh-") => {
                Ok(SshAccess {
                    host,
                    port,
                    user,
                    host_key,
                })
            }
            _ => Err(Error::Other("Nexus answered no SSH address for the machine.".into())),
        }
    }

    async fn link(&self, id: &str, body: Value) -> Result<String> {
        let id = checked(id)?;
        let answer = self
            .client
            .post(&format!("/v1/cloud/machines/{id}/links"), body)
            .await
            .map_err(error)?;
        text(&answer, "url")
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .ok_or_else(|| Error::Other("Nexus answered no link.".into()))
    }
}

impl Provider for GatewayCloud {
    fn create(&self, spec: Spec) -> BoxFuture<'_, Result<String>> {
        // Gateway makes a machine for a desktop session only; the spec's name stands for the session.
        Box::pin(async move { Ok(self.create_machine(&spec.name, &spec.name, None, None).await?.id) })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Machine>> {
        Box::pin(async move {
            match self.machine(id).await {
                Ok(machine) => Ok(machine.machine()),
                Err(Error::NotFound) => Ok(Machine {
                    id: id.to_string(),
                    name: String::new(),
                    state: State::Gone,
                    provider_state: "gone".into(),
                    size: None,
                    labels: Default::default(),
                    stopped_at: None,
                    data_expired_at: None,
                }),
                Err(error) => Err(error),
            }
        })
    }

    fn sleep<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.action(id, "sleep"))
    }

    fn wake<'a>(&'a self, id: &'a str, _size: Option<Size>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.action(id, "wake"))
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.remove(id))
    }

    fn authorize_ssh<'a>(&'a self, id: &'a str, public_key: &'a str) -> BoxFuture<'a, Result<SshAccess>> {
        Box::pin(self.access(id, public_key))
    }

    fn preview<'a>(&'a self, id: &'a str, port: u16, _public: bool) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.link(id, json!({ "kind": "port", "port": port })))
    }

    fn desktop<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.link(id, json!({ "kind": "desktop" })))
    }

    /// Gateway runs no command on the machine: the app reaches it over SSH.
    fn exec<'a>(&'a self, _id: &'a str, _command: &'a str, _timeout: Duration) -> BoxFuture<'a, Result<ExecOutput>> {
        Box::pin(async { Err(Error::Other("Commands go to a cloud machine over SSH.".into())) })
    }

    /// Gateway decides when a machine sleeps (from its host's heartbeat): there is no stop key.
    fn mint_stop_key<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async { Ok(None) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gateway_machines_with_either_time_shape() {
        let machine = GatewayMachine::from_json(&json!({
            "id": "cm_1", "name": "cloud", "provider": "boat", "size": "large", "state": "sleeping", "error": null,
            "createdAt": "2026-11-01T12:00:00Z", "sleepingSince": 1_762_000_000_000_i64,
        }))
        .unwrap_or_else(|| panic!("not read"));
        assert_eq!(machine.created_at, crate::desktop::parse_time("2026-11-01T12:00:00Z"));
        assert_eq!(machine.sleeping_since, Some(1_762_000_000_000));
        assert_eq!(machine.machine().state, State::Sleeping);
        assert_eq!(
            crate::desktop::parse_time(machine.machine().stopped_at.as_deref().unwrap_or_default()),
            Some(1_762_000_000_000)
        );
        assert_eq!(
            GatewayMachine::from_json(&json!({ "id": "cm_2", "state": "something-new" })).map(|m| m.machine().state),
            Some(State::Starting)
        );
        let queued = GatewayMachine::from_json(&json!({
            "id": "cm_3", "sessionId": "t_abc", "title": "Fix the login", "state": "queued",
            "queuedUntil": "2026-11-04T12:00:00Z", "stopReason": null,
        }))
        .unwrap_or_else(|| panic!("not read"));
        assert_eq!(queued.session_id.as_deref(), Some("t_abc"));
        assert_eq!(queued.title.as_deref(), Some("Fix the login"));
        assert_eq!(queued.queued_until, crate::desktop::parse_time("2026-11-04T12:00:00Z"));
        let waiting = queued.machine();
        assert_eq!(waiting.state, State::Creating, "a wait goes on through the queue");
        assert_eq!(waiting.provider_state, "queued");
        assert_eq!(
            GatewayMachine::from_json(&json!({ "id": "cm_4", "state": "sleeping", "stopReason": "resize" }))
                .and_then(|m| m.stop_reason),
            Some("resize".into())
        );
        assert_eq!(queued.host_online, None, "a Gateway before the relay");
        let online = |value: Value| GatewayMachine::from_json(&json!({ "id": "cm_5", "hostOnline": value }));
        assert_eq!(online(json!(true)).and_then(|m| m.host_online), Some(true));
        assert_eq!(online(json!(false)).and_then(|m| m.host_online), Some(false));
        assert_eq!(online(json!(null)).and_then(|m| m.host_online), Some(false));
        assert_eq!(
            GatewayMachine::from_json(&json!({ "id": "../x" })),
            None,
            "an id goes in a path"
        );
    }

    #[test]
    fn refusals_say_why_and_keep_the_provider_codes() {
        let core = |status: u16, code: &str| {
            ClientError::from(CoreError {
                status,
                message: "raw".into(),
                code: Some(code.into()),
                interval: None,
                details: None,
            })
        };
        let unavailable = error(core(503, "cloud_unavailable"));
        assert_eq!(Unavailable::of(&unavailable), Some(Unavailable::Unconfigured));
        assert!(!unavailable.is_retryable(), "no provider account is not a blip");
        let role = error(core(403, "forbidden"));
        assert_eq!(Unavailable::of(&role), Some(Unavailable::NotAllowed));
        assert!(role.to_string().contains("role"));
        let not_enabled = error(core(403, "cloud_not_enabled"));
        assert_eq!(
            not_enabled.to_string(),
            "Cloud machines are not turned on for this organization."
        );
        assert_eq!(Unavailable::of(&not_enabled), Some(Unavailable::Unconfigured));
        assert!(!not_enabled.is_retryable());
        assert_eq!(
            Unavailable::of(&error(core(401, "unauthorized"))),
            Some(Unavailable::SignedOut)
        );
        assert_eq!(error(core(404, "not_found")), Error::NotFound);
        assert!(error(core(400, "machine_not_running")).is_retryable());
        let budget = error(core(402, "budget_exceeded"));
        assert_eq!(budget, Error::OverBudget);
        assert!(budget.to_string().contains("over its budget"));
        assert!(!budget.is_retryable(), "a spent budget is not a blip");
        assert_eq!(Unavailable::of(&budget), None, "a running machine still works");
        let trouble = error(core(503, "provider_down"));
        assert!(trouble.is_retryable(), "still retried");
        assert_eq!(Unavailable::of(&trouble), Some(Unavailable::Degraded));
        assert!(trouble.to_string().contains("trouble"));
    }
}
