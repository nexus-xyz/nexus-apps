//! What the UI asks of Nexus, as one interface: the workspace and its views hold a [`Backend`], not the Gateway
//! client itself. Each call starts the work and returns at once; the answer comes back later as a
//! [`crate::gateway::Event`] on the channel the backend was made with. So the views depend on these calls and those
//! events only, and something other than `nexus-client` over HTTP can stand behind them: a fake that answers from
//! fixtures in a view's test, or a local engine process later (as Zeron's UI talks to its daemon). The calls are
//! grouped by the region of the UI that makes them.

use crate::dictation::Dictation;
use serde_json::Value;

use crate::gateway::{Gateway, Verb};

/// Process-local authority from an accepted Account read; never serialized or placed on the wire.
#[derive(Clone)]
pub struct ExecutionAuthority {
    epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    expected: u64,
}
impl ExecutionAuthority {
    pub(crate) fn from_read(epoch: std::sync::Arc<std::sync::atomic::AtomicU64>, expected: u64) -> Self {
        Self { epoch, expected }
    }
    pub(crate) fn current(&self) -> bool {
        self.epoch.load(std::sync::atomic::Ordering::SeqCst) == self.expected
    }
    pub(crate) fn matches(&self, epoch: &std::sync::Arc<std::sync::atomic::AtomicU64>) -> bool {
        std::sync::Arc::ptr_eq(&self.epoch, epoch) && self.current()
    }
}

#[derive(Clone)]
pub enum AccountCredential {
    SignedIn(ExecutionAuthority),
    #[cfg(test)]
    Fixture,
}

/// A Gateway call's answer, awaited on the UI's executor.
pub type Request = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + 'static>>;
/// A fresh call whose HTTP status remains available to capability-sensitive features.
pub type DetailedRequest = std::pin::Pin<Box<dyn std::future::Future<Output = nexus_client::Result<Value>> + 'static>>;
/// The app's current authenticated client, read in memory when a session starts.
pub type SessionClient = std::sync::Arc<dyn Fn() -> Option<nexus_client::Client> + Send + Sync>;

/// A saved-provider child starts or takes input before or after a token change.
pub(crate) static SESSION_AUTH_TRANSITION: std::sync::RwLock<()> = std::sync::RwLock::new(());
/// A page query's saved answer and its independent background refresh.
pub struct Query {
    pub cached: Option<Value>,
    pub cached_generation: Option<(std::sync::Arc<std::sync::atomic::AtomicU64>, u64)>,
    pub refresh: Request,
    pub generation: Option<(std::sync::Arc<std::sync::atomic::AtomicU64>, u64)>,
}
impl Query {
    pub fn take_cached(&mut self) -> Option<Value> {
        if self
            .cached_generation
            .as_ref()
            .is_some_and(|(epoch, expected)| epoch.load(std::sync::atomic::Ordering::SeqCst) != *expected)
        {
            self.cached = None;
        }
        self.cached.take()
    }

    pub fn current(&self) -> bool {
        self.generation
            .as_ref()
            .is_none_or(|(generation, expected)| generation.load(std::sync::atomic::Ordering::SeqCst) == *expected)
    }
}
impl From<Request> for Query {
    fn from(refresh: Request) -> Self {
        Self {
            cached: None,
            cached_generation: None,
            refresh,
            generation: None,
        }
    }
}
impl std::future::Future for Query {
    type Output = Result<Value, String>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        if !self.current() {
            return std::task::Poll::Ready(Err("The signed-in account changed.".into()));
        }
        self.refresh.as_mut().poll(cx)
    }
}

/// A Gateway call's answer as text, for what Gateway writes out as Markdown.
pub type TextRequest = std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + 'static>>;

/// Session sharing keeps the credential captured when the sheet opens.
pub trait SessionArtifacts: Send + Sync {
    fn share(&self, body: Value) -> Request;
    fn stop(&self, session_key: &str) -> Request;
}

pub trait Backend: Send + Sync {
    // Where it is.

    /// The Gateway's address; also the Code sessions' scope (`<gateway>|<org>`).
    fn url(&self) -> &str;
    /// A fingerprint of the signed-in credential, which names the session browser's website profile; `None` while
    /// signed out or when a pinned execution side was revoked.
    fn marker(&self) -> Option<String>;
    /// The live credential generation captured by UI account contexts.
    fn account_credential(&self) -> Option<AccountCredential> {
        #[cfg(test)]
        return Some(AccountCredential::Fixture);
        #[cfg(not(test))]
        None
    }
    /// Share the active sign-in with the session runner, including later sign-outs and token changes.
    fn session_client(&self) -> SessionClient {
        std::sync::Arc::new(|| None)
    }
    /// Nexus UI's site (the UI's address without `/app`).
    fn site_url(&self) -> String;
    /// The first refusal of this app's version (`revoked.rs`). Backends that never refuse it have none.
    fn revoked(&self) -> Option<std::sync::Arc<crate::revoked::Latch>> {
        None
    }

    fn session_artifacts(&self) -> Option<std::sync::Arc<dyn SessionArtifacts>> {
        None
    }

    /// Read-only team-session events. Backends without a live transport can omit it.
    fn session_stream(&self, _thread: &str) -> Option<crate::gateway::team_sessions::SessionStream> {
        None
    }

    // Any call, for the pages of `pages/`: they read Gateway themselves.

    /// One Gateway call, answered to the caller (`pages::fetch`).
    fn request(&self, verb: Verb, path: String, body: Option<Value>) -> Request;
    /// The readiness caller bounds this read; Gateway also cancels its HTTP call at the deadline.
    fn start_budget_snapshot(&self) -> Request {
        self.request(Verb::Get, "/v1/spend".into(), None)
    }
    /// `request` with Gateway's typed error (its `code`): the same cache and account guards.
    fn request_typed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        self.request_detailed(verb, path, body)
    }
    fn request_detailed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        let request = self.request(verb, path, body);
        Box::pin(async move {
            request
                .await
                .map_err(|message| nexus_client::error::Transport::Other(message).into())
        })
    }

    /// A read that can show the scoped saved answer before it refreshes.
    fn query(&self, path: String) -> Query {
        self.request(Verb::Get, path, None).into()
    }

    /// The cached skill lists settle together before the slash menu opens.
    fn skill_query(&self) -> Query {
        self.skill_catalog().into()
    }
    /// Chat's personal and library skill lists. A missing optional library is represented by null.
    fn skill_catalog(&self) -> Request {
        let own = self.request(Verb::Get, "/v1/skills".into(), None);
        let library = self.request(Verb::Get, "/v1/skill-library".into(), None);
        Box::pin(async move {
            let (own, library) = futures::join!(own, library);
            Ok(serde_json::json!({"own":own?,"library":library?}))
        })
    }
    /// A GET whose answer is text, not JSON (a chat's Markdown export).
    fn text(&self, path: String) -> TextRequest;

    /// One call to the agent API at `base` (`agent.rs`): it is not Gateway, but takes the same sign-in. `None`: this
    /// app has no agent API, and the answer says so.
    fn agent(&self, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream;
    /// [`Backend::agent`] with another account's own token (`peer_tokens`); a refusal of it never signs the person out.
    /// A backend with no other accounts answers as `agent` does.
    fn agent_as(&self, base: Option<String>, _token: String, call: crate::agent::Call) -> crate::agent::Stream {
        self.agent(base, call)
    }
    /// The other accounts' tokens held this launch, in memory, by organization key (`account_states.rs`).
    fn peer_tokens(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    /// A held token the agent API refused: it is not asked again.
    fn drop_peer(&self, _org: &str) {}

    // Signing in, and who is signed in (the sidebar's person row and Inbox count).

    /// Use the stored token, if there is one: `SignedIn` or `SignedOut`.
    fn restore(&self);
    /// Who is signed in, when the stored token was already read before the window (`Gateway::preload`). None: not
    /// known yet, and `restore`'s event says it.
    fn saved_sign_in(&self) -> Option<bool> {
        None
    }
    /// The same backend at another Gateway, with its own events, signed out until `restore`. This one stops what it
    /// was doing.
    fn switched(
        &self,
        url: &str,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    );
    /// Mark a newly chosen, signed-out Gateway as resolved before sharing it with any windows.
    fn resolve_signed_out(&self) {}
    /// Another window on the same Gateway: the same client and sign-in, with its own events and its own answer.
    /// `SignedIn` and `SignedOut` reach every window.
    fn window(
        &self,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    );
    /// A dedicated chat execution side bound to the authority at creation. Unlike a UI window, it must
    /// never adopt replacement credentials for queued follow-ups. Refuse stale or foreign Account-read authority.
    fn execution_window(
        &self,
        _authority: &ExecutionAuthority,
    ) -> Option<(
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    )> {
        None
    }
    /// Start a device sign-in: `SignInCode`, then `SignedIn` or `SignInFailed`.
    fn sign_in(&self);
    fn cancel_sign_in(&self);
    /// Sign in with an API key: `SignedIn` or `SignInFailed`.
    fn sign_in_with_key(&self, key: String);
    /// Switch to another of the person's organizations: `SignedIn` again, or `SwitchFailed`.
    fn switch_org(&self, id: String);
    /// [`Backend::switch_org`] that keeps the token it leaves, in memory, under `leaving`, and drops `entering`'s.
    fn switch_org_keeping(&self, id: String, _leaving: Option<String>, _entering: Option<String>) {
        self.switch_org(id)
    }
    fn sign_out(&self);
    /// `Account`.
    fn load_account(&self);
    /// `OrgSettings`.
    fn load_org_settings(&self);
    /// `Inbox`.
    fn load_inbox(&self);

    // The Inbox and the docs.

    /// `Notifications`: the first page, or the one after `cursor`.
    fn load_notifications(&self, request: crate::inbox::PageRequest);
    /// `NotificationMarked`.
    fn mark_notification(&self, id: String, mark: crate::inbox::Mark);
    /// `NotificationsRead`: every unread notification, `known` and those on pages not loaded yet.
    fn read_all_notifications(&self, known: Vec<String>);
    /// `DocsIndex`.
    fn docs_index(&self);
    /// `Docs`: a docs page by its path under `/docs` (empty for the first).
    fn docs_page(&self, path: String);

    // Chats (the sidebar's sessions, the transcript and the composer).

    /// `Sessions`.
    fn list_sessions(&self);
    /// `Opened`.
    fn open_session(&self, id: String) -> u64;
    /// No event: chat `id` read into the warm cache, so opening it shows that copy at once.
    fn preload_session(&self, _id: String) {}
    /// `Models`.
    fn list_models(&self);
    /// `Spend` and `FreeStartMax`: whether Nexus can pay the next call, for the composer's pay notice.
    fn load_pays(&self);
    /// `Providers`.
    fn list_providers(&self);
    /// `Suggested`.
    fn suggest(&self, session: String);
    /// Send `prompt`, with `attachments`, as turn `turn`, with the composer's `choices` (mode, team, web search):
    /// `Created` for a new session, `Delta`s, then `Answered` or `AnswerFailed`.
    fn ask(
        &self,
        turn: u64,
        session: Option<String>,
        prompt: String,
        model: Option<String>,
        attachments: crate::attachments::Draft,
        choices: crate::gateway::ChatChoices,
    );
    /// Stop turn `turn`, if it is still answering.
    fn stop(&self, turn: u64);
    /// Rewind chat `session` to before its message `message`, reading its `attachments` first: `Rewound` with `ask`.
    fn rewind(&self, ask: u64, session: String, message: String, text: String, attachments: Vec<String>);
    /// Start a dictation: `Dictation` events with `id`, until the returned handle is dropped.
    fn dictate(&self, id: u64) -> Dictation;
    /// Voice mode's ears (`voice/ears.rs`): the microphone and the speech service, until the handle is dropped. A
    /// backend with no speech service cannot hear.
    fn hear(&self) -> crate::voice::Hearing {
        crate::voice::Hearing::unavailable()
    }

    // The palette.

    /// `Searched` with `seq`.
    fn search(&self, seq: u64, query: String);

    // The Library and the page panel.

    /// `Library`.
    fn load_library(&self, org: Option<String>);
    /// `Framed`: where to load a shared artifact.
    fn frame_artifact(&self, id: String, path: String);
    /// `Framed`: where to load one of the person's own pages.
    fn frame_page(&self, id: String, path: String);
    /// `Framed`: one of the person's own pages at the version a chat answer names.
    fn frame_page_version(&self, id: String, version: u32, path: String);
    /// Ask for that page's address ahead of the click, as a pointer rests on its card. A later `frame_page_version`
    /// for it answers at once. No event comes from this.
    fn prefetch_frame_page_version(&self, _id: String, _version: u32) {}
    /// `ChatText`: a shared chat's Markdown.
    fn chat_text(&self, id: String, title: String, path: String);

    // Settings.

    /// `NotificationSettings`.
    fn load_notification_settings(&self);
    /// `NotificationSettings`: every setting, after the change.
    fn set_notification_setting(&self, group: String, channel: String, on: bool);
    /// `Tokens`.
    fn list_tokens(&self);
    /// `TokenCreated`, then `Tokens`.
    fn create_token(&self, label: String);
    /// `Tokens`.
    fn revoke_token(&self, id: String);
    /// `Tokens`, once all are revoked. A backend that cannot batch revokes each in turn.
    fn revoke_tokens(&self, ids: Vec<String>) {
        for id in ids {
            self.revoke_token(id);
        }
    }
}

/// The real backend: Nexus Gateway over HTTP, through `nexus-client`.
impl Backend for Gateway {
    fn session_stream(&self, thread: &str) -> Option<crate::gateway::team_sessions::SessionStream> {
        Some(Gateway::session_stream(self, thread))
    }
    fn skill_catalog(&self) -> Request {
        Box::pin(Gateway::skill_catalog(self))
    }
    fn skill_query(&self) -> Query {
        Gateway::skill_query(self)
    }
    fn url(&self) -> &str {
        Gateway::url(self)
    }
    fn marker(&self) -> Option<String> {
        Gateway::marker(self)
    }
    fn account_credential(&self) -> Option<AccountCredential> {
        Gateway::account_credential(self)
    }
    fn session_client(&self) -> SessionClient {
        Gateway::session_client(self)
    }
    fn site_url(&self) -> String {
        Gateway::site_url(self)
    }
    fn revoked(&self) -> Option<std::sync::Arc<crate::revoked::Latch>> {
        Some(Gateway::revoked(self))
    }
    fn query(&self, path: String) -> Query {
        Gateway::query(self, path)
    }
    fn request(&self, verb: Verb, path: String, body: Option<Value>) -> Request {
        Box::pin(Gateway::request(self, verb, path, body))
    }
    fn start_budget_snapshot(&self) -> Request {
        Box::pin(Gateway::start_budget_snapshot(self))
    }
    fn request_typed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        Box::pin(Gateway::request_typed(self, verb, path, body))
    }
    fn request_detailed(&self, verb: Verb, path: String, body: Option<Value>) -> DetailedRequest {
        Box::pin(Gateway::request_detailed(self, verb, path, body))
    }
    fn session_artifacts(&self) -> Option<std::sync::Arc<dyn SessionArtifacts>> {
        Some(Gateway::session_artifacts(self))
    }
    fn text(&self, path: String) -> TextRequest {
        Box::pin(Gateway::text(self, path))
    }
    fn agent(&self, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        Gateway::agent(self, base, call)
    }
    fn agent_as(&self, base: Option<String>, token: String, call: crate::agent::Call) -> crate::agent::Stream {
        Gateway::agent_as(self, base, token, call)
    }
    fn peer_tokens(&self) -> Vec<(String, String)> {
        Gateway::peer_tokens(self)
    }
    fn drop_peer(&self, org: &str) {
        Gateway::drop_peer(self, org)
    }
    fn restore(&self) {
        Gateway::restore(self)
    }
    fn saved_sign_in(&self) -> Option<bool> {
        Gateway::saved_sign_in(self)
    }
    fn switched(
        &self,
        url: &str,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    ) {
        let (gateway, events) = Gateway::switched(self, url);
        (std::sync::Arc::new(gateway), events)
    }
    fn resolve_signed_out(&self) {
        Gateway::resolve_signed_out(self);
    }
    fn window(
        &self,
    ) -> (
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    ) {
        let (gateway, events) = Gateway::window(self);
        (std::sync::Arc::new(gateway), events)
    }
    fn execution_window(
        &self,
        authority: &ExecutionAuthority,
    ) -> Option<(
        std::sync::Arc<dyn Backend>,
        futures::channel::mpsc::UnboundedReceiver<crate::gateway::Event>,
    )> {
        Gateway::execution_window(self, authority)
            .map(|(gateway, events)| (std::sync::Arc::new(gateway) as std::sync::Arc<dyn Backend>, events))
    }
    fn sign_in(&self) {
        Gateway::sign_in(self)
    }
    fn cancel_sign_in(&self) {
        Gateway::cancel_sign_in(self)
    }
    fn switch_org(&self, id: String) {
        Gateway::switch_org(self, id)
    }
    fn switch_org_keeping(&self, id: String, leaving: Option<String>, entering: Option<String>) {
        Gateway::switch_org_keeping(self, id, leaving, entering)
    }
    fn sign_in_with_key(&self, key: String) {
        Gateway::sign_in_with_key(self, key)
    }
    fn sign_out(&self) {
        Gateway::sign_out(self)
    }
    fn load_account(&self) {
        Gateway::load_account(self)
    }
    fn load_org_settings(&self) {
        Gateway::load_org_settings(self)
    }
    fn load_inbox(&self) {
        Gateway::load_inbox(self)
    }
    fn load_notifications(&self, request: crate::inbox::PageRequest) {
        Gateway::load_notifications(self, request)
    }
    fn mark_notification(&self, id: String, mark: crate::inbox::Mark) {
        Gateway::mark_notification(self, id, mark)
    }
    fn read_all_notifications(&self, known: Vec<String>) {
        Gateway::read_all_notifications(self, known)
    }
    fn docs_index(&self) {
        Gateway::docs_index(self)
    }
    fn docs_page(&self, path: String) {
        Gateway::docs_page(self, path)
    }
    fn list_sessions(&self) {
        Gateway::list_sessions(self)
    }
    fn open_session(&self, id: String) -> u64 {
        Gateway::open_session(self, id)
    }
    fn preload_session(&self, id: String) {
        Gateway::preload_session(self, id)
    }
    fn list_models(&self) {
        Gateway::list_models(self)
    }
    fn load_pays(&self) {
        Gateway::load_pays(self)
    }
    fn list_providers(&self) {
        Gateway::list_providers(self)
    }
    fn suggest(&self, session: String) {
        Gateway::suggest(self, session)
    }
    fn ask(
        &self,
        turn: u64,
        session: Option<String>,
        prompt: String,
        model: Option<String>,
        attachments: crate::attachments::Draft,
        choices: crate::gateway::ChatChoices,
    ) {
        Gateway::ask(self, turn, session, prompt, model, attachments, choices)
    }
    fn stop(&self, turn: u64) {
        Gateway::stop(self, turn)
    }
    fn rewind(&self, ask: u64, session: String, message: String, text: String, attachments: Vec<String>) {
        Gateway::rewind(self, ask, session, message, text, attachments)
    }
    fn dictate(&self, id: u64) -> Dictation {
        Gateway::dictate(self, id)
    }
    fn hear(&self) -> crate::voice::Hearing {
        Gateway::hear(self)
    }
    fn search(&self, seq: u64, query: String) {
        Gateway::search(self, seq, query)
    }
    fn load_library(&self, org: Option<String>) {
        Gateway::load_library(self, org)
    }
    fn frame_artifact(&self, id: String, path: String) {
        Gateway::frame_artifact(self, id, path)
    }
    fn frame_page(&self, id: String, path: String) {
        Gateway::frame_page(self, id, path)
    }
    fn frame_page_version(&self, id: String, version: u32, path: String) {
        Gateway::frame_page_version(self, id, version, path)
    }
    fn prefetch_frame_page_version(&self, id: String, version: u32) {
        Gateway::prefetch_frame_page_version(self, id, version)
    }
    fn chat_text(&self, id: String, title: String, path: String) {
        Gateway::chat_text(self, id, title, path)
    }
    fn load_notification_settings(&self) {
        Gateway::load_notification_settings(self)
    }
    fn set_notification_setting(&self, group: String, channel: String, on: bool) {
        Gateway::set_notification_setting(self, group, channel, on)
    }
    fn list_tokens(&self) {
        Gateway::list_tokens(self)
    }
    fn create_token(&self, label: String) {
        Gateway::create_token(self, label)
    }
    fn revoke_token(&self, id: String) {
        Gateway::revoke_tokens(self, vec![id])
    }
    fn revoke_tokens(&self, ids: Vec<String>) {
        Gateway::revoke_tokens(self, ids)
    }
}
