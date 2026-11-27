//! Gateway connection and sign-in state. Only a signed-in workspace can acquire an account context.

use std::sync::Arc;

use futures::channel::mpsc::UnboundedReceiver;

use crate::{
    backend::{
        AccountCredential, Backend, DetailedRequest, ExecutionAuthority, Query, Request, SESSION_AUTH_TRANSITION,
        TextRequest,
    },
    gateway::{ChatChoices, Event, Verb},
};
use serde_json::Value;

use super::{Auth, Workspace};

/// The active backend. A connected URL exists even before the person signs in.
pub(crate) struct GatewayAccess {
    backend: Arc<dyn Backend>,
    credential: Option<AccountCredential>,
}

impl GatewayAccess {
    pub(crate) fn new(backend: Arc<dyn Backend>) -> Self {
        let credential = backend.account_credential();
        Self { backend, credential }
    }

    pub(crate) fn accept_sign_in(&mut self) {
        self.credential = self.backend.account_credential();
    }

    pub(crate) fn account(&self) -> Option<AccountGateway> {
        let credential = self.credential.clone()?;
        let account = AccountGateway {
            backend: self.backend.clone(),
            credential,
        };
        account.current().then_some(account)
    }

    pub(super) fn url(&self) -> &str {
        self.backend.url()
    }

    pub(super) fn site_url(&self) -> String {
        self.backend.site_url()
    }

    pub(super) fn marker(&self) -> Option<String> {
        self.backend.marker()
    }

    pub(super) fn signed_out_guard(&self) -> impl Fn() -> bool + Send + Sync + use<> {
        let backend = self.backend.clone();
        move || backend.marker().is_none()
    }

    pub(super) fn revoked(&self) -> Option<Arc<crate::revoked::Latch>> {
        self.backend.revoked()
    }

    pub(super) fn saved_sign_in(&self) -> Option<bool> {
        self.backend.saved_sign_in()
    }

    /// Restore a saved credential. The backend warms the connection only after finding a token.
    pub(super) fn restore_saved_sign_in(&self) {
        self.backend.restore();
    }

    pub(crate) fn choose_gateway(&self, url: &str, restore: bool) -> (Arc<dyn Backend>, UnboundedReceiver<Event>) {
        let (gateway, events) = self.backend.switched(url);
        if !restore {
            gateway.resolve_signed_out();
        }
        (gateway, events)
    }

    pub(super) fn begin_device_sign_in(&self) {
        self.backend.sign_in();
    }

    pub(super) fn cancel_device_sign_in(&self) {
        self.backend.cancel_sign_in();
    }

    pub(super) fn begin_key_sign_in(&self, key: String) {
        self.backend.sign_in_with_key(key);
    }

    /// The runner links to the credential when available. Its local-provider path needs none.
    pub(super) fn runner_sign_in(&self) -> crate::backend::SessionClient {
        self.backend.session_client()
    }
}

/// The authenticated capability held by Gateway views and signed-in pages. Its constructor
/// belongs to the Workspace connection owner.
#[derive(Clone)]
pub(crate) struct AccountGateway {
    backend: Arc<dyn Backend>,
    credential: AccountCredential,
}

// Dispatch and credential changes use the same lock. An open page cannot send a deferred call
// using a new account, even if the same credential is reinstalled later.
impl AccountGateway {
    fn dispatch<T>(&self, call: impl FnOnce(&dyn Backend) -> T) -> Option<T> {
        let _transition = SESSION_AUTH_TRANSITION
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.current() {
            return None;
        }
        Some(call(self.backend.as_ref()))
    }

    pub(crate) fn current(&self) -> bool {
        match &self.credential {
            AccountCredential::SignedIn(epoch) => epoch.current(),
            #[cfg(test)]
            AccountCredential::Fixture => true,
        }
    }

    pub(crate) fn url(&self) -> &str {
        self.backend.url()
    }

    pub(crate) fn site_url(&self) -> String {
        self.backend.site_url()
    }

    pub(crate) fn marker(&self) -> Option<String> {
        self.dispatch(|backend| backend.marker()).flatten()
    }

    pub(crate) fn matches_identity(&self, identity: &crate::windows::retention::Identity) -> bool {
        identity.matches_credential(self.url(), self.marker().as_deref())
    }

    pub(super) fn preview_code_route(
        &self,
        runner: &crate::code_run::CodeRunner,
        agent: nexus_workbench::event::SessionAgent,
        via: nexus_workbench::event::Via,
    ) -> Option<crate::code_run::Answer<Option<nexus_client::auto_route::Answer>>> {
        self.dispatch(|backend| {
            let client = backend.session_client()()?;
            Some(runner.auto_preview(client, agent, via))
        })
        .flatten()
    }

    pub(super) fn preview_code_route_across(
        &self,
        runner: &crate::code_run::CodeRunner,
        via: nexus_workbench::event::Via,
    ) -> Option<crate::code_run::Answer<Option<(nexus_workbench::event::SessionAgent, nexus_client::auto_route::Answer)>>>
    {
        self.dispatch(|backend| {
            let client = backend.session_client()()?;
            Some(runner.auto_preview_across(client, via))
        })
        .flatten()
    }

    pub(crate) fn request(&self, verb: Verb, path: String, body: Option<Value>) -> Request {
        self.dispatch(|gate| gate.request(verb, path, body))
            .unwrap_or_else(account_changed)
    }

    pub(crate) fn request_detailed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        self.dispatch(|gate| gate.request_detailed(verb, path, body))
            .unwrap_or_else(|| {
                Box::pin(async { Err(nexus_client::error::Transport::Other(ACCOUNT_CHANGED.into()).into()) })
            })
    }

    pub(crate) fn request_typed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        self.dispatch(|gate| gate.request_typed(verb, path, body))
            .unwrap_or_else(|| {
                Box::pin(async { Err(nexus_client::error::Transport::Other(ACCOUNT_CHANGED.into()).into()) })
            })
    }

    pub(crate) fn query(&self, path: String) -> Query {
        self.dispatch(|gate| gate.query(path))
            .unwrap_or_else(|| account_changed().into())
    }

    pub(crate) fn skill_query(&self) -> Query {
        self.dispatch(|gate| gate.skill_query())
            .unwrap_or_else(|| account_changed().into())
    }

    pub(crate) fn start_budget_snapshot(&self) -> Request {
        self.dispatch(|gate| gate.start_budget_snapshot())
            .unwrap_or_else(account_changed)
    }

    pub(crate) fn text(&self, path: String) -> TextRequest {
        self.dispatch(|gate| gate.text(path))
            .unwrap_or_else(|| Box::pin(async { Err(ACCOUNT_CHANGED.into()) }))
    }

    pub(crate) fn open_session(&self, id: String) -> Option<u64> {
        self.dispatch(|gate| gate.open_session(id))
    }

    pub(crate) fn peer_tokens(&self) -> Vec<(String, String)> {
        self.dispatch(|gate| gate.peer_tokens()).unwrap_or_default()
    }

    pub(crate) fn session_stream(&self, thread: &str) -> Option<crate::gateway::team_sessions::SessionStream> {
        self.dispatch(|gate| gate.session_stream(thread)).flatten()
    }

    pub(crate) fn session_artifacts(&self) -> Option<Arc<dyn crate::backend::SessionArtifacts>> {
        self.dispatch(|gate| gate.session_artifacts())
            .flatten()
            .map(|artifacts| {
                Arc::new(AccountArtifacts(self.clone(), artifacts)) as Arc<dyn crate::backend::SessionArtifacts>
            })
    }

    pub(crate) fn execution_window(
        &self,
        authority: &ExecutionAuthority,
    ) -> Option<(Arc<dyn Backend>, UnboundedReceiver<Event>)> {
        self.dispatch(|gate| gate.execution_window(authority)).flatten()
    }

    pub(crate) fn agent(&self, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        self.dispatch(|gate| gate.agent(base, call))
            .unwrap_or_else(closed_agent)
    }

    pub(crate) fn agent_as(
        &self,
        base: Option<String>,
        token: String,
        call: crate::agent::Call,
    ) -> crate::agent::Stream {
        self.dispatch(|gate| gate.agent_as(base, token, call))
            .unwrap_or_else(closed_agent)
    }

    pub(crate) fn dictate(&self, id: u64) -> Option<crate::dictation::Dictation> {
        self.dispatch(|gate| gate.dictate(id))
    }

    pub(crate) fn hear(&self) -> crate::voice::Hearing {
        self.dispatch(|gate| gate.hear())
            .unwrap_or_else(crate::voice::Hearing::unavailable)
    }

    fn live_client(&self) -> Option<nexus_client::Client> {
        self.dispatch(|gate| gate.session_client()()).flatten()
    }

    pub(super) fn update_workspace_services(&self, owns_store: bool) {
        let client = self.live_client();
        if owns_store && let Some(client) = &client {
            crate::cloud::start(client.clone());
            crate::path_notices::start(client.clone());
        }
        crate::cloud::session_changed(client.clone());
        crate::path_notices::session_changed(client);
    }

    pub(super) fn update_release_access(&self) {
        let token = self.live_client().and_then(|client| client.options().api_key.clone());
        crate::updates::session(self.url(), token);
    }

    pub(super) fn sync_machines(
        &self,
        machines: &crate::machines::Machines,
        settings: &nexus_desktop_core::settings::Settings,
    ) {
        if let Some(client) = self.live_client() {
            machines.set_account_client(Some(client));
            machines.sync(settings);
        }
    }

    pub(crate) fn clear_workspace_services() {
        crate::cloud::session_changed(None);
        crate::path_notices::session_changed(None);
    }
}

const ACCOUNT_CHANGED: &str = "The signed-in account changed.";

fn account_changed() -> Request {
    Box::pin(async { Err(ACCOUNT_CHANGED.into()) })
}

fn closed_agent() -> crate::agent::Stream {
    let (_, chunks) = futures::channel::mpsc::unbounded();
    crate::agent::Stream { chunks, _stop: None }
}

struct AccountArtifacts(AccountGateway, Arc<dyn crate::backend::SessionArtifacts>);

impl crate::backend::SessionArtifacts for AccountArtifacts {
    fn share(&self, body: Value) -> Request {
        self.0.dispatch(|_| self.1.share(body)).unwrap_or_else(account_changed)
    }

    fn stop(&self, session_key: &str) -> Request {
        self.0
            .dispatch(|_| self.1.stop(session_key))
            .unwrap_or_else(account_changed)
    }
}

macro_rules! account_events {
    ($(fn $name:ident($($arg:ident: $ty:ty),*);)*) => {
        impl AccountGateway {
            $(
                pub(crate) fn $name(&self, $($arg: $ty),*) {
                    self.dispatch(|gate| gate.$name($($arg),*));
                }
            )*
        }
    };
}

account_events! {
    fn sign_out();
    fn switch_org(id: String);
    fn switch_org_keeping(id: String, leaving: Option<String>, entering: Option<String>);
    fn load_account();
    fn load_org_settings();
    fn load_inbox();
    fn load_notifications(request: crate::inbox::PageRequest);
    fn mark_notification(id: String, mark: crate::inbox::Mark);
    fn read_all_notifications(known: Vec<String>);
    fn docs_index();
    fn docs_page(path: String);
    fn list_sessions();
    fn preload_session(id: String);
    fn list_models();
    fn load_pays();
    fn list_providers();
    fn suggest(session: String);
    fn ask(turn: u64, session: Option<String>, prompt: String, model: Option<String>, attachments: crate::attachments::Draft, choices: ChatChoices);
    fn stop(turn: u64);
    fn rewind(ask: u64, session: String, message: String, text: String, attachments: Vec<String>);
    fn search(seq: u64, query: String);
    fn load_library(org: Option<String>);
    fn frame_artifact(id: String, path: String);
    fn frame_page(id: String, path: String);
    fn frame_page_version(id: String, version: u32, path: String);
    fn prefetch_frame_page_version(id: String, version: u32);
    fn chat_text(id: String, title: String, path: String);
    fn load_notification_settings();
    fn set_notification_setting(group: String, channel: String, on: bool);
    fn list_tokens();
    fn create_token(label: String);
    fn revoke_tokens(ids: Vec<String>);
    fn drop_peer(org: &str);
}

#[cfg(test)]
impl AccountGateway {
    pub(crate) fn fixture(backend: Arc<dyn Backend>) -> Self {
        let credential = backend.account_credential().unwrap_or(AccountCredential::Fixture);
        Self { backend, credential }
    }
}

const ACCOUNT_CONTROL_REASON: &str = "Sign in to Nexus to use this.";

impl Workspace {
    pub(super) fn account_gateway(&self) -> Option<AccountGateway> {
        if !matches!(self.auth, Auth::SignedIn) {
            return None;
        }
        self.gateway.account()
    }

    /// Account tools on an account-free session cannot borrow authority from an adjacent organization.
    pub(super) fn session_account_gateway(&self) -> Option<AccountGateway> {
        if self.editing_local_code() {
            return None;
        }
        self.account_gateway()
    }

    pub(super) fn account_control_reason(&self) -> Option<&'static str> {
        self.account_gateway().is_none().then_some(ACCOUNT_CONTROL_REASON)
    }

    pub(super) fn session_account_control_reason(&self) -> Option<&'static str> {
        self.session_account_gateway()
            .is_none()
            .then_some(if self.account_gateway().is_some() {
                "Unavailable in sessions started without an account."
            } else {
                ACCOUNT_CONTROL_REASON
            })
    }

    pub(super) fn current_identity(
        &self,
        account: &crate::gateway::Account,
    ) -> Option<crate::windows::retention::Identity> {
        let gateway = self.account_gateway()?;
        crate::windows::retention::Identity::at(gateway.url(), gateway.marker()?, account)
    }

    pub(super) fn request_sign_out(&self) {
        if let Some(account) = self.account_gateway() {
            account.sign_out();
        }
    }
}
