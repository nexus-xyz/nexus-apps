//! The app's side of Nexus Gateway: every call goes through `nexus-client` on a tokio runtime of its own, and each
//! answer comes back to the UI as an [`Event`] on a channel. GPUI runs its own executor, not tokio, and reqwest needs
//! tokio, so the two meet only at the channel.
//!
//! The token lives in the OS credential store under the Nexus app's service and account (the Gateway URL), so a
//! person signed in to one app is signed in to the other. Signing out here signs out there too.
//!
//! Every window shares one client, so one sign-in: a sign-in, a sign-out or an organization switch in one window reaches
//! all of them ([`Gateway::window`]).

#[cfg(test)]
mod chat_authority_tests;
#[cfg(test)]
mod credential_tests;
mod queries;
#[cfg(test)]
mod revoked_tests;
pub(crate) mod team_sessions;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nexus_client::device::{self, LoginError};
use nexus_client::{Client, ClientOptions, config, sse};
use serde_json::{Value, json};

use crate::dictation::{self, Dictation, DictationEvent, Problem, Socket};
use crate::internal_access::Verdict;
use crate::model::{AUTO, Message, ModelChoice, PageBlock, SessionSummary, text, title_from};

/// `x-nexus-client` on every Gateway call from this build: `desktop/<version>`, the desktop app's surface and this
/// build's own version ([`crate::channel::VERSION`], `0.11.0-internal.42` for an internal build). Gateway's desktop
/// revocations (`desktop_revoked.rs`), logs and metrics read it.
pub fn client() -> String {
    format!("desktop/{}", crate::channel::VERSION)
}

/// `user-agent` on this build's own HTTP calls: `nexus-desktop/<version>`.
pub fn user_agent() -> String {
    format!("nexus-desktop/{}", crate::channel::VERSION)
}

/// The options every Gateway call from this build starts from: [`client`] and [`user_agent`].
pub fn client_options(gateway: &str, token: Option<String>) -> ClientOptions {
    ClientOptions {
        api_key: token,
        client: Some(client()),
        user_agent: Some(user_agent()),
        ..ClientOptions::new(gateway)
    }
}

/// A chat as `GET /v1/chat/sessions/{id}` answers it, for [`Event::Opened`].
#[derive(Debug, Clone)]
pub struct OpenedChat {
    pub id: String,
    pub messages: Vec<Message>,
    pub archived: bool,
    /// The team its spend goes to (`team`), when it names one.
    pub team: Option<String>,
    /// Whether the model may search the web in it (`webSearch`).
    pub web_search: bool,
}

/// What a question goes with, besides its text: the composer's choices (Nexus UI's `ComposerState`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatChoices {
    /// The team the spend goes to (`team`); none sends no team, and Gateway takes the person's default.
    pub team: Option<String>,
    /// Whether the model may search the web (`webSearch`), a setting of the session.
    pub web_search: bool,
    /// The session's `webSearch` differs from [`ChatChoices::web_search`]: set it first (`PATCH`), as Nexus UI does.
    pub patch_web_search: bool,
}

impl OpenedChat {
    /// Parse a session detail response, including a rewind response.
    pub fn from_detail(id: String, detail: &Value) -> OpenedChat {
        let messages = detail
            .get("messages")
            .and_then(Value::as_array)
            .map(|messages| messages.iter().filter_map(Message::from_json).collect())
            .unwrap_or_default();
        OpenedChat {
            id,
            messages,
            archived: detail.get("archived").and_then(Value::as_bool).unwrap_or(false),
            team: text(detail, "team").filter(|team| !team.is_empty()),
            web_search: detail.get("webSearch").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A query reply is ignored after its credential generation ends.
    ReadSnapshot {
        epoch: Arc<AtomicU64>,
        expected: u64,
        event: Box<Event>,
        revision: Option<(Arc<AtomicU64>, u64)>,
    },
    /// Every chat reply carries the dispatch identity, including cold reads and failures.
    ChatRead {
        read: u64,
        id: String,
        cached: bool,
        chat: Result<OpenedChat, String>,
    },
    SignedIn,
    SignedOut,
    /// A sign-in started: the code to check and the approval page, which the UI opens in the browser.
    SignInCode {
        cancelled: Arc<AtomicBool>,
        user_code: String,
        link: String,
        /// Seconds until the code expires (ten minutes from Gateway).
        expires_in: f64,
    },
    DeviceSignInFailed {
        cancelled: Arc<AtomicBool>,
        error: String,
    },
    SignInFailed(String),
    /// Switching to another organization failed; still signed in to this one.
    SwitchFailed(String),
    Sessions(Result<Vec<SessionSummary>, String>),
    /// The session, its messages, and whether it is archived.
    Opened(Result<OpenedChat, String>),
    /// A first message made its session.
    Created {
        turn: u64,
        id: String,
        title: String,
    },
    /// Gateway stored turn `turn`'s question (`nexus.question`): its id and attachments, for Rewind and Edit.
    Asked {
        turn: u64,
        question: crate::model::Stored,
    },
    /// A rewind ([`Gateway::rewind`]) ended: the chat as Gateway keeps it now and the prompt it gives back, or why
    /// not, with the original prompt text so a closed editor cannot lose it.
    Rewound {
        ask: u64,
        outcome: Result<(OpenedChat, nexus_desktop_core::chat_rewind::RestoredPrompt), (String, String)>,
    },
    Delta {
        turn: u64,
        text: String,
    },
    /// The stored answer, which replaces the streamed text, the page it wrote, if any, and the web pages it read.
    Answered {
        turn: u64,
        content: String,
        page: Option<PageBlock>,
        sources: Vec<nexus_desktop_core::chat::WebSource>,
    },
    /// The answer started writing or editing a page (`nexus.tool`, `write_page` or `edit_page`, running).
    WritingPage {
        turn: u64,
    },
    /// The answer started searching the web (`nexus.tool`, `web_search`, running). It ends with the next text.
    SearchingWeb {
        turn: u64,
    },
    /// The organization's settings (`GET /v1/orgs/settings`), for web search in chat; `None` when they could not be read.
    OrgSettings(Option<Value>),
    AnswerFailed {
        turn: u64,
        error: String,
    },
    /// The models the person may chat with; empty when they could not be read (Auto still works).
    Models(Vec<ModelChoice>),
    /// `GET /v1/spend`, for the composer's pay notice (`pay_notice.rs`); `None` when it could not be read.
    Spend(Option<Value>),
    /// `GET /v1/people/me/budget`: the person's own month, for the sidebar's budget card; `None` when it could not be read.
    OwnBudget(Option<Value>),
    /// The free start's price cap per million output tokens (`GET /v1/billing`'s `pricing.freeStartMaxOutputUsd`):
    /// the model menu marks the models it pays for. `None` for a member, or when it could not be read.
    FreeStartMax(Option<f64>),
    /// A suggested next prompt for `session`, after its answer.
    Suggested {
        session: String,
        suggestion: String,
    },
    /// Who is signed in, and to which organization.
    Account(Account),
    /// What a dictation (numbered, so a stale one's events are dropped) reports.
    Dictation {
        id: u64,
        event: DictationEvent,
    },
    /// Gateway's search results for the palette's query `seq`.
    Searched {
        seq: u64,
        results: Result<Vec<SearchHit>, String>,
    },
    /// How many inbox items are unread: the first page's, 50 or more when more pages follow.
    Inbox(usize),
    /// Cached and fresh answers of one Inbox page share a request token. None means the Inbox is unavailable.
    Notifications {
        request: crate::inbox::PageRequest,
        page: Result<Option<crate::inbox::Page>, String>,
    },
    /// A notification after Mark read or Done.
    NotificationMarked(Result<crate::inbox::Item, String>),
    /// The notifications after Read all, and why any of them did not save.
    NotificationsRead(Vec<Result<crate::inbox::Item, String>>),
    /// Nexus UI's docs, by section, from its `/llms.txt`.
    DocsIndex(Result<Vec<crate::docs::Section>, String>),
    /// A docs page as Markdown (`<ui>/docs/<path>.md`), with its path.
    Docs(Result<(String, String), String>),
    /// Where to load an artifact (`GET /v1/artifacts/{id}/frame`), for the page panel.
    Framed(Result<PageFrame, String>),
    /// The Library's lists (shared with me, shared by me, pages).
    Library(Result<crate::library::Lists, String>),
    /// A shared chat as Markdown (`GET /v1/artifacts/{id}.md`), for the page panel.
    ChatText(Result<ChatText, String>),
    /// Every model of the catalog, for the Models view; empty when it could not be read.
    Catalog(Vec<crate::catalog::CatalogModel>),
    /// The Gateway's providers with their rates, for the Models view's Providers tab; or why they could not be read.
    Providers(Result<Vec<crate::catalog::CatalogProvider>, String>),
    /// The person's notification channels per group (`GET` or `PUT /v1/notifications/settings`).
    NotificationSettings(Result<Vec<crate::settings::ChannelSetting>, String>),
    /// The person's API keys and sign-ins (`GET /v1/auth/tokens`).
    Tokens(Result<Vec<crate::settings::ApiToken>, String>),
    /// A key just made, with the key itself, shown once; or why it was not.
    TokenCreated(Result<crate::settings::NewToken, String>),
}

/// A shared chat to read in the page panel.
#[derive(Debug, Clone)]
pub struct ChatText {
    pub title: String,
    pub markdown: String,
    /// Nexus UI's path for it (`/artifacts/<id>`), for Open in browser.
    pub path: String,
}

/// A signed address that serves a page for a few minutes, on Nexus's user-content origin.
#[derive(Debug, Clone, PartialEq)]
pub struct PageFrame {
    pub url: String,
    pub title: Option<String>,
    /// Where the page lives in Nexus UI, to open it in the browser.
    pub path: String,
    /// The page and version the address serves, as `page@version`: the name of its picture (`web/snapshot.rs`).
    pub version_key: Option<String>,
    /// The Gateway path that signs a fresh address for the same page, for a load tried again once this one expired.
    pub mint: Option<String>,
}

/// The `page@version` a frame answer names, if it names both.
fn version_key(response: &Value) -> Option<String> {
    let page = text(response, "pageId")?;
    let version = response.get("version")?.as_i64()?;
    Some(format!("{page}@{version}"))
}

/// Chat's mode, as Gateway names it.
const CHAT_MODE: &str = "chat";

/// `GET /v1/tenants/me`, as the sidebar shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub user_id: Option<String>,
    /// Stable person identity for app-local scoped preferences. None for an organization API key.
    pub person_id: Option<String>,
    pub name: String,
    pub email: String,
    pub org_name: String,
    /// The organization's logo (`tenant.imageUrl`), never a personal account's (`logoFor` in Nexus UI's `org-logo.tsx`).
    pub org_logo: Option<String>,
    /// The organization's segment in Nexus UI's URLs and the Code sessions' scope: its slug, else its id.
    pub org: String,
    /// The person's own account (`kind: personal`), not a company.
    pub personal: bool,
    /// Presence waits until Gateway identifies the organization's kind.
    pub organization_kind: Option<String>,
    /// The teams the person is in, for the composer's team chip.
    pub teams: Vec<nexus_desktop_core::chat::Team>,
    /// The person's default team's slug, where spend goes when a chat names none.
    pub default_team: Option<String>,
    /// Their role here (`owner`, `admin` or `member`); empty when Gateway names none.
    pub role: String,
    /// What the role allows (`members.manage`), which decides the Settings rows they see.
    pub permissions: Vec<String>,
}

impl Account {
    pub fn from_json(me: &Value) -> Option<Account> {
        let tenant = me.get("tenant")?;
        let user = me.get("user");
        let field = |value: Option<&Value>, name: &str| {
            value
                .and_then(|value| value.get(name))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(String::from)
        };
        let email = field(user, "email").unwrap_or_default();
        let name = field(user, "name")
            .or_else(|| {
                email
                    .split('@')
                    .next()
                    .filter(|part| !part.is_empty())
                    .map(String::from)
            })
            .unwrap_or_else(|| "You".into());
        let org = field(Some(tenant), "slug").or_else(|| field(Some(tenant), "id"))?;
        Some(Account {
            user_id: field(user, "id"),
            person_id: field(user, "id").or_else(|| field(user, "email")),
            name,
            email,
            org_name: field(Some(tenant), "name").unwrap_or_else(|| org.clone()),
            org_logo: field(Some(tenant), "imageUrl")
                .filter(|_| field(Some(tenant), "kind").as_deref() != Some("personal")),
            personal: field(Some(tenant), "kind").as_deref() == Some("personal"),
            organization_kind: field(Some(tenant), "kind"),
            teams: nexus_desktop_core::chat::teams(me),
            default_team: nexus_desktop_core::chat::default_team(me),
            role: field(Some(me), "role").unwrap_or_default(),
            permissions: me
                .get("permissions")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default(),
            org,
        })
    }
}

/// One result of `GET /v1/search`, as the palette lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The group's label: Sessions, Artifacts, People, Teams, Models.
    pub group: String,
    /// `session`, `artifact`, `member`, `invitation`, `team`, `model` or `policy`.
    pub entity: String,
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Nexus UI's path for it (`/app/<org>/sessions/<id>#m-<message>`).
    pub url: String,
}

impl SearchHit {
    /// The hits of a search response, group by group.
    pub fn from_response(response: &Value) -> Vec<SearchHit> {
        let Some(groups) = response.get("groups").and_then(Value::as_array) else {
            return Vec::new();
        };
        groups
            .iter()
            .flat_map(|group| {
                let label = text(group, "label").unwrap_or_default();
                group
                    .get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(move |hit| {
                        Some(SearchHit {
                            group: label.clone(),
                            entity: text(hit, "entity").unwrap_or_default(),
                            id: text(hit, "id")?,
                            title: text(hit, "title").filter(|title| !title.is_empty())?,
                            subtitle: text(hit, "subtitle").filter(|subtitle| !subtitle.is_empty()),
                            url: text(hit, "url").unwrap_or_default(),
                        })
                    })
            })
            .collect()
    }
}

/// One window's handle on the Gateway. Every window shares the client, so one sign-in, through [`Inner`]; each has its
/// own channel of events and its own answer through [`Side`] ([`Gateway::window`]).
#[derive(Clone)]
pub struct Gateway {
    inner: Arc<Inner>,
    side: Arc<Side>,
}

/// The Gateway the app uses now, for the usage signals' loop (`Gateway::start`): the newest one made.
static SIGNALS_INNER: Mutex<std::sync::Weak<Inner>> = Mutex::new(std::sync::Weak::new());

/// What every window shares: the runtime, the address and the signed-in client.
struct Inner {
    runtime: tokio::runtime::Handle,
    url: String,
    ui_url: String,
    /// Every call goes through this client; its observer reports a refusal of this version to `revoked`.
    client: Mutex<Client>,
    /// The first refusal of this version (`revoked.rs`), for every window.
    revoked: Arc<crate::revoked::Latch>,
    /// Every window shares one restoration. Opening another window must not reread a failing keychain.
    restored: AtomicBool,
    /// The startup read of the saved sign-in passed its deadline and still waits on the keychain (`preload`). Lock
    /// order: this -> client.
    restore_pending: Mutex<bool>,
    cache: Arc<crate::warm_cache::WarmCache>,
    cache_epoch: Arc<AtomicU64>,
    #[cfg(test)]
    fixture_account: AtomicBool,
    cache_revision: Arc<AtomicU64>,
    chat_read_sequence: AtomicU64,
    read_versions: Mutex<std::collections::HashMap<String, queries::ReadVersion>>,
    device_attempt: Mutex<Option<DeviceAttempt>>,
    /// Order durable credential changes without holding the UI's cancellation lock across keychain I/O.
    credential_writes: tokio::sync::Mutex<()>,
    /// Page addresses asked for before the click (`frame_prefetch.rs`).
    frames: Mutex<crate::frame_prefetch::Prefetched>,
    /// Every window's side, for the events all of them take (signed in, signed out) and for stopping every answer at a
    /// Gateway switch. A closed window's side drops out.
    sides: Mutex<Vec<std::sync::Weak<Side>>>,
    /// The tokens of the other accounts this launch has been in, by organization key (`Account::org`). They live in
    /// memory only, never in the keychain, and the switcher reads each account's Assistant with its own
    /// (`workspace/account_states.rs`). The sign-in's token is the one in `client`, never one of these.
    peers: Mutex<std::collections::HashMap<String, String>>,
}

/// The browser sign-in and the window entitled to cancel it.
struct DeviceAttempt {
    cancelled: Arc<AtomicBool>,
    owner: std::sync::Weak<Side>,
}

/// One window's own part: where its events go, and the turn it is answering.
struct Side {
    /// Dedicated execution owners keep the creation epoch even through A -> B -> A credential changes.
    execution_epoch: Option<u64>,
    events: UnboundedSender<Event>,
    /// Keep the cancellation flag through event delivery, after the shared in-flight slot is released.
    latest_device_attempt: Mutex<Option<Arc<AtomicBool>>>,
    /// Each turn still answering, including chats left running while another page is visible.
    answering: Mutex<std::collections::HashMap<u64, tokio::task::AbortHandle>>,
}

impl Side {
    fn new_device_cancellation(&self) -> Option<Arc<AtomicBool>> {
        let mut latest = self.latest_device_attempt.lock().ok()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        if let Some(previous) = latest.replace(cancelled.clone()) {
            previous.store(true, Ordering::SeqCst);
        }
        Some(cancelled)
    }

    /// Registration and revocation share this lock; an old epoch cannot appear after abort drained the side.
    fn register(&self, turn: u64, handle: tokio::task::AbortHandle, epoch: &AtomicU64, expected: u64) {
        let Ok(mut answering) = self.answering.lock() else {
            handle.abort();
            return;
        };
        answering.retain(|_, handle| !handle.is_finished());
        if epoch.load(Ordering::SeqCst) != expected {
            handle.abort();
        } else if let Some(previous) = answering.insert(turn, handle) {
            previous.abort();
        }
    }

    fn finished(&self, turn: u64, task: tokio::task::Id) {
        if let Ok(mut answering) = self.answering.lock()
            && answering.get(&turn).is_some_and(|handle| handle.id() == task)
        {
            answering.remove(&turn);
        }
    }

    fn stop(&self, turn: u64) {
        if let Ok(mut answering) = self.answering.lock()
            && let Some(handle) = answering.remove(&turn)
        {
            handle.abort();
        }
    }
    fn abort(&self) {
        if let Ok(mut answering) = self.answering.lock() {
            for (_, answer) in answering.drain() {
                answer.abort();
            }
        }
    }
}

/// Where dictation and voice mode stream speech: a short-lived token's socket, or why there is none.
async fn voice_socket(client: Client) -> Result<Socket, Problem> {
    match client.post("/v1/chat/voice-token", json!({})).await {
        Ok(token) => Socket::from_json(&token).ok_or(Problem::Failed),
        Err(nexus_client::Error::Core(error)) if error.status == 503 => Err(Problem::Unavailable),
        Err(_) => Err(Problem::Failed),
    }
}

impl Gateway {
    /// The Gateway in `NEXUS_URL`, else the Nexus app's, else production, signed out until [`Gateway::restore`].
    pub fn start() -> (Gateway, UnboundedReceiver<Event>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nexus-gateway")
            .enable_all()
            .build()
            .expect("the Gateway runtime starts");
        // The runtime lives as long as the process. Tasks hold clones of the Gateway, and a runtime dropped by the
        // last one, on its own worker thread, would panic.
        let runtime = Box::leak(Box::new(runtime)).handle().clone();
        let env = |name: &str| std::env::var(name).ok();
        // `NEXUS_URL`, else the Gateway the Nexus app is set to, else production.
        // The organization's locked Gateway, else `NEXUS_URL`, else one picked at sign-in on an internal build, else
        // the Nexus app's choice as its `settings::gateway_url` makes it (the person's, the managed one, production).
        let policy = crate::managed::policy();
        let url = crate::managed::locked_gateway(policy)
            .or_else(|| env("NEXUS_URL").filter(|url| !url.is_empty()))
            .or_else(crate::paths::app_gateway_url)
            .unwrap_or_else(|| {
                nexus_desktop_core::settings::gateway_url(&crate::desktop_settings::load(), policy.managed.as_ref())
            });
        // The usage signals go with the signed-in client of the Gateway in use, after a switch too. Not in
        // the unit tests: their windows start a Gateway each, and the loop would outlive the test binary's last test.
        #[cfg(not(test))]
        runtime.spawn(nexus_desktop_core::signals::run(
            nexus_desktop_core::signals::App::DesktopGpui,
            || {
                let inner = SIGNALS_INNER.lock().ok()?.upgrade()?;
                inner.client.lock().ok().map(|client| client.clone())
            },
        ));
        // The open-source app's daily_active: only with `NEXUS_DESKTOP_DAILY_ACTIVE` on, only while usage
        // data is on, only to production, never with the account's token; signed out too.
        #[cfg(not(test))]
        if nexus_desktop_core::daily_active::enabled()
            && let Some(dir) = crate::paths::own_data_dir()
        {
            runtime.spawn(nexus_desktop_core::daily_active::run(dir, || {
                let inner = SIGNALS_INNER.lock().ok()?.upgrade()?;
                inner.client.lock().ok().map(|client| client.clone())
            }));
        }
        Gateway::at(runtime, &url)
    }

    /// An isolated signed-out backend for view tests; no saved settings or keychain is read.
    #[cfg(test)]
    pub(crate) fn for_view_test(runtime: tokio::runtime::Handle) -> Gateway {
        Self::for_view_test_at(runtime, "https://gateway.example.test")
    }

    /// A local fake Gateway for view tests; no saved settings or keychain is read.
    #[cfg(test)]
    pub(crate) fn for_view_test_at(runtime: tokio::runtime::Handle, url: &str) -> Gateway {
        let gateway = Self::at(runtime, url).0;
        gateway.inner.fixture_account.store(true, Ordering::SeqCst);
        gateway
    }

    /// A loopback view fixture with explicit credentials and its real event receiver. No saved auth is read.
    #[cfg(test)]
    pub(crate) fn for_authenticated_view_test_at(
        runtime: tokio::runtime::Handle,
        url: &str,
        key: &str,
    ) -> (Gateway, UnboundedReceiver<Event>) {
        let (gateway, events) = Self::at(runtime, url);
        gateway.set_token(Some(key.to_string()));
        (gateway, events)
    }

    /// A Gateway at `url` on `runtime`, signed out, with its own channel of events. Its clients report a refusal of
    /// this version to the app's latch (`revoked.rs`); a unit test's Gateway has a latch of its own.
    fn at(runtime: tokio::runtime::Handle, url: &str) -> (Gateway, UnboundedReceiver<Event>) {
        #[cfg(not(test))]
        let revoked = crate::revoked::app().clone();
        #[cfg(test)]
        let revoked = crate::revoked::Latch::new(|_| {});
        Self::at_with(runtime, url, revoked)
    }

    fn at_with(
        runtime: tokio::runtime::Handle,
        url: &str,
        revoked: Arc<crate::revoked::Latch>,
    ) -> (Gateway, UnboundedReceiver<Event>) {
        let url = url.trim_end_matches('/').to_string();
        let env = |name: &str| std::env::var(name).ok();
        let ui_url = config::ui_url(&url, &env);
        let options = client_options(&url, None);
        let inner = Arc::new(Inner {
            runtime,
            ui_url,
            client: Mutex::new(Client::new(options).observed(revoked.observer())),
            revoked,
            restored: AtomicBool::new(false),
            restore_pending: Mutex::new(false),
            cache: Arc::new(crate::warm_cache::WarmCache::new()),
            cache_epoch: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            fixture_account: AtomicBool::new(false),
            cache_revision: Arc::new(AtomicU64::new(0)),
            chat_read_sequence: AtomicU64::new(0),
            read_versions: Mutex::new(Default::default()),
            device_attempt: Mutex::default(),
            credential_writes: tokio::sync::Mutex::new(()),
            frames: Mutex::default(),
            sides: Mutex::new(Vec::new()),
            peers: Mutex::default(),
            url,
        });
        if let Ok(mut current) = SIGNALS_INNER.lock() {
            *current = Arc::downgrade(&inner);
        }
        Gateway::side_of(inner)
    }

    /// A new side on `inner`, with its own channel.
    fn side_of(inner: Arc<Inner>) -> (Gateway, UnboundedReceiver<Event>) {
        Self::side_of_authority(inner, None)
    }

    fn side_of_authority(inner: Arc<Inner>, execution_epoch: Option<u64>) -> (Gateway, UnboundedReceiver<Event>) {
        let (events, receiver) = unbounded();
        let side = Arc::new(Side {
            execution_epoch,
            events,
            latest_device_attempt: Mutex::default(),
            answering: Mutex::new(std::collections::HashMap::new()),
        });
        if let Ok(mut sides) = inner.sides.lock() {
            sides.retain(|side| side.strong_count() > 0);
            sides.push(Arc::downgrade(&side));
        }
        (Gateway { inner, side }, receiver)
    }

    fn account_epoch(&self) -> crate::backend::ExecutionAuthority {
        crate::backend::ExecutionAuthority::from_read(
            self.inner.cache_epoch.clone(),
            self.inner.cache_epoch.load(Ordering::SeqCst),
        )
    }

    pub(crate) fn account_credential(&self) -> Option<crate::backend::AccountCredential> {
        let client = self.inner.client.lock().expect("client lock");
        if client.options().api_key.is_some() {
            return Some(crate::backend::AccountCredential::SignedIn(self.account_epoch()));
        }
        #[cfg(test)]
        if self.inner.fixture_account.load(Ordering::SeqCst) {
            return Some(crate::backend::AccountCredential::Fixture);
        }
        None
    }

    /// Another window on this Gateway: the same client and sign-in, with its own events and its own answer.
    pub fn window(&self) -> (Gateway, UnboundedReceiver<Event>) {
        Gateway::side_of(self.inner.clone())
    }

    /// An app-owned chat executor pinned to the exact credential generation at creation.
    pub fn execution_window(
        &self,
        authority: &crate::backend::ExecutionAuthority,
    ) -> Option<(Gateway, UnboundedReceiver<Event>)> {
        let _client = self.inner.client.lock().expect("client lock");
        if !authority.matches(&self.inner.cache_epoch) {
            return None;
        }
        let expected = self.inner.cache_epoch.load(Ordering::SeqCst);
        Some(Self::side_of_authority(self.inner.clone(), Some(expected)))
    }

    /// Settings' Gateway switch, as the Nexus app's (`commands.rs`, `point_at`): a new client for `url` on the same
    /// runtime, with its own events, signed out until [`Gateway::restore`] reads `url`'s keychain entry. This one
    /// stops its sign-in and every window's answer: their late events go to channels no one reads. The other windows
    /// take sides of the new one ([`Gateway::window`], `windows.rs`).
    pub fn switched(&self, url: &str) -> (Gateway, UnboundedReceiver<Event>) {
        self.cancel_all_device_sign_in();
        {
            let _transition = crate::backend::SESSION_AUTH_TRANSITION
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Client capture uses this same lock. No new-epoch ask can register inside the old abort fanout.
            let _client = self.inner.client.lock().expect("client lock");
            self.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
            for side in self.sides() {
                side.abort();
            }
        }
        Gateway::at_with(self.inner.runtime.clone(), url, self.inner.revoked.clone())
    }

    /// The windows' sides still open.
    fn sides(&self) -> Vec<Arc<Side>> {
        self.inner
            .sides
            .lock()
            .map(|sides| sides.iter().filter_map(std::sync::Weak::upgrade).collect())
            .unwrap_or_default()
    }

    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Who is signed in (`GET /v1/tenants/me`).
    pub fn load_account(&self) {
        self.read_event("/v1/tenants/me", |answer, _| {
            answer
                .ok()
                .as_ref()
                .and_then(Account::from_json)
                .map(Event::Account)
                .into_iter()
                .collect()
        });
    }

    /// The organization's settings (`GET /v1/orgs/settings`), which say whether chat may search the web.
    pub fn load_org_settings(&self) {
        self.read_event("/v1/orgs/settings", |answer, _| vec![Event::OrgSettings(answer.ok())]);
    }

    /// How many inbox items are unread (`GET /v1/notifications?unread=true`, the first page).
    pub fn load_inbox(&self) {
        self.read_event("/v1/notifications?unread=true", |answer, _| {
            let Ok(value) = answer else { return vec![] };
            let page = crate::inbox::Page::from_json(&value);
            let unread = page.items.iter().filter(|item| !item.read).count();
            vec![Event::Inbox(if page.next_cursor.is_some() {
                unread.max(50)
            } else {
                unread
            })]
        });
    }

    /// A page of the Inbox (`GET /v1/notifications`, 50 a page, Done items left out): the first, or the one after
    /// `cursor`.
    pub fn load_notifications(&self, request: crate::inbox::PageRequest) {
        let mut query = Vec::new();
        if let Some(cursor) = &request.cursor {
            query.push(format!("cursor={}", nexus_client::url::enc(cursor)));
        }
        if request.unread {
            query.push("unread=true".to_string());
        }
        let path = if query.is_empty() {
            "/v1/notifications".to_string()
        } else {
            format!("/v1/notifications?{}", query.join("&"))
        };
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let epoch = self.inner.cache_epoch.clone();
        let version = self.read_version(&client, &path).epoch;
        let initial_version = version.load(Ordering::SeqCst);
        let key = crate::warm_cache::key(&client, &path);
        let (mut values, _) = self.inner.cache.snapshot(&[key], &self.inner.cache_revision);
        let saved = values.pop().flatten();
        if let Some(saved) = &saved {
            self.send_snapshot(
                &epoch,
                expected,
                Some((version.clone(), initial_version)),
                Event::Notifications {
                    request: request.clone(),
                    page: Ok(Some(crate::inbox::Page::from_json(saved))),
                },
            );
        }
        let refresh = self.request_result(Verb::Get, path, None);
        let gateway = self.clone();
        self.inner.runtime.spawn(async move {
            let reply = refresh.await;
            let answer = reply.answer;
            if epoch.load(Ordering::SeqCst) != expected {
                return;
            }
            let page = match answer {
                Ok(value) => {
                    if saved.as_ref() == Some(&value) {
                        return;
                    }
                    Ok(Some(crate::inbox::Page::from_json(&value)))
                }
                Err(nexus_client::Error::Core(error)) if error.status == 403 || error.status == 404 => Ok(None),
                Err(_) if saved.is_some() => return,
                Err(error) => Err(error.to_string()),
            };
            gateway.send_snapshot(
                &epoch,
                expected,
                Some(reply.version),
                Event::Notifications { request, page },
            );
        });
    }

    /// Mark a notification read, or put it away (`POST /v1/notifications/{id}/read` or `/done`).
    pub fn mark_notification(&self, id: String, mark: crate::inbox::Mark) {
        let gateway = self.clone();
        let mut write = self.begin_write("/v1/notifications");
        let client = write.client();
        self.inner.runtime.spawn(async move {
            let path = format!("/v1/notifications/{}/{}", nexus_client::url::enc(&id), mark.path());
            let item = client
                .post(&path, json!({}))
                .await
                .map_err(|error| error.to_string())
                .and_then(|item| crate::inbox::Item::from_json(&item).ok_or_else(|| "Gateway sent no item.".into()));
            write.finish();
            let (epoch, expected) = write.scope();
            gateway.send_read(epoch, expected, Event::NotificationMarked(item));
        });
    }

    /// Read all: mark read every unread notification, those on screen (`known`) and those on pages not yet loaded.
    /// Gateway has no bulk mark, so each is one `POST /v1/notifications/{id}/read`, a few at a time.
    pub fn read_all_notifications(&self, known: Vec<String>) {
        /// The unread pages read for ids: 50 a page, so up to 1,000 notifications.
        const MAX_PAGES: usize = 20;
        /// The marks in flight at once.
        const PARALLEL: usize = 8;
        let gateway = self.clone();
        let mut write = self.begin_write("/v1/notifications");
        let client = write.client();
        self.inner.runtime.spawn(async move {
            use futures::StreamExt;
            let mut results = Vec::new();
            let mut ids = known;
            let mut cursor: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let path = match &cursor {
                    Some(cursor) => format!(
                        "/v1/notifications?unread=true&cursor={}",
                        nexus_client::url::enc(cursor)
                    ),
                    None => "/v1/notifications?unread=true".to_string(),
                };
                match client.get(&path).await {
                    Ok(value) => {
                        let page = crate::inbox::Page::from_json(&value);
                        ids.extend(page.items.into_iter().filter(|item| !item.read).map(|item| item.id));
                        cursor = page.next_cursor;
                    }
                    Err(error) => {
                        results.push(Err(error.to_string()));
                        break;
                    }
                }
                if cursor.is_none() {
                    break;
                }
            }
            let mut seen = std::collections::HashSet::new();
            ids.retain(|id| seen.insert(id.clone()));
            let marks = futures::stream::iter(ids)
                .map(|id| {
                    let client = client.clone();
                    async move {
                        let path = format!(
                            "/v1/notifications/{}/{}",
                            nexus_client::url::enc(&id),
                            crate::inbox::Mark::Read.path()
                        );
                        client
                            .post(&path, json!({}))
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|item| {
                                crate::inbox::Item::from_json(&item).ok_or_else(|| "Gateway sent no item.".into())
                            })
                    }
                })
                .buffer_unordered(PARALLEL)
                .collect::<Vec<_>>()
                .await;
            results.extend(marks);
            write.finish();
            let (epoch, expected) = write.scope();
            gateway.send_read(epoch, expected, Event::NotificationsRead(results));
        });
    }

    /// Nexus UI's docs index (`<ui>/llms.txt`), read without the person's token: the docs are public.
    pub fn docs_index(&self) {
        let gateway = self.clone();
        let site = self.site();
        self.inner.runtime.spawn(async move {
            let index = read_text(&site, "/llms.txt")
                .await
                .map(|text| crate::docs::sections(&text));
            gateway.send(Event::DocsIndex(index));
        });
    }

    /// A docs page as Markdown: `/docs.md` for the first, `/docs/<path>.md` for the rest.
    pub fn docs_page(&self, path: String) {
        let gateway = self.clone();
        let site = self.site();
        self.inner.runtime.spawn(async move {
            let page = read_text(&site, &crate::docs::markdown_path(&path))
                .await
                .map(|markdown| (path, markdown));
            gateway.send(Event::Docs(page));
        });
    }

    /// Both lists must settle before Chat offers a skill name.
    pub fn skill_catalog(&self) -> impl std::future::Future<Output = Result<Value, String>> + 'static {
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let reads = self.request_batch(vec!["/v1/skills".into(), "/v1/skill-library".into()]);
        let gateway = self.clone();
        async move {
            let replies = reads.await?;
            let [own, library] = replies.as_slice() else {
                return Err("The skill lists did not both answer.".into());
            };
            let own = own.answer.clone().map_err(|error| error.to_string())?;
            let value = optional_skill_library(library.answer.clone())?;
            if let Some(key) = crate::warm_cache::key(&client, "/v1/skill-library") {
                gateway.inner.cache.put_guarded(&key, &value, || {
                    gateway.inner.cache_epoch.load(Ordering::SeqCst) == expected
                        && gateway.inner.cache_revision.load(Ordering::SeqCst) == library.cache_version
                });
            }
            Ok(json!({"own":own,"library":value}))
        }
    }

    pub fn skill_query(&self) -> crate::backend::Query {
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let keys = ["/v1/skills", "/v1/skill-library"].map(|path| crate::warm_cache::key(&client, path));
        let (values, stamp) = self.inner.cache.snapshot(&keys, &self.inner.cache_revision);
        let cached = match (&values[0], &values[1]) {
            (Some(own), Some(library)) => Some(json!({"own":own,"library":library})),
            _ => None,
        };
        crate::backend::Query {
            cached,
            cached_generation: Some((self.inner.cache_revision.clone(), stamp)),
            refresh: Box::pin(self.skill_catalog()),
            generation: Some((self.inner.cache_epoch.clone(), expected)),
        }
    }

    /// One Gateway call with the person's token, awaited where the UI needs the answer: a page view does
    /// `cx.spawn(async move |this, cx| { let answer = request.await; ... })` (`pages::fetch`). The call runs on the
    /// Gateway's runtime, so a page that closes first drops only the answer.
    pub fn request(
        &self,
        verb: Verb,
        path: impl Into<String>,
        body: Option<Value>,
    ) -> impl std::future::Future<Output = Result<Value, String>> + 'static {
        let read = self.request_typed(verb, path, body);
        async move { read.await.map_err(|error| error.to_string()) }
    }

    /// `request` with Gateway's typed error (its `code`), for a caller that branches on it. A write keeps the same
    /// guards: it moves the cached reads on and refuses an answer for an account that changed.
    pub fn request_typed(
        &self,
        verb: Verb,
        path: impl Into<String>,
        body: Option<Value>,
    ) -> impl std::future::Future<Output = nexus_client::Result<Value>> + 'static {
        let read = self.request_result(verb, path.into(), body);
        async move { read.await.answer }
    }

    /// The saved value arrives independently of the refresh; stale accounts never update a live page.
    pub fn query(&self, path: String) -> crate::backend::Query {
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let keys = [crate::warm_cache::key(&client, &path)];
        let (mut values, stamp) = self.inner.cache.snapshot(&keys, &self.inner.cache_revision);
        let cached = values.pop().flatten();
        crate::backend::Query {
            cached,
            cached_generation: Some((self.inner.cache_revision.clone(), stamp)),
            refresh: Box::pin(self.request(Verb::Get, path, None)),
            generation: Some((self.inner.cache_epoch.clone(), expected)),
        }
    }

    /// The channel-based views use the same saved read and refresh contract as page queries.
    fn read_event(
        &self,
        path: &str,
        mut event: impl FnMut(Result<Value, String>, bool) -> Vec<Event> + Send + 'static,
    ) {
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let epoch = self.inner.cache_epoch.clone();
        let version = self.read_version(&client, path).epoch;
        let expected_version = version.load(Ordering::SeqCst);
        let cached = crate::warm_cache::key(&client, path).and_then(|key| self.inner.cache.get(&key));
        let warmed = cached.is_some();
        let saved_value = cached.clone();
        if let Some(value) = cached
            && epoch.load(Ordering::SeqCst) == expected
        {
            for event in event(Ok(value), true) {
                self.send_snapshot(&epoch, expected, Some((version.clone(), expected_version)), event);
            }
        }
        let refresh = self.request_result(Verb::Get, path.to_string(), None);
        let gateway = self.clone();
        self.inner.runtime.spawn(async move {
            let reply = refresh.await;
            let answer = reply.answer.map_err(|error| error.to_string());
            if epoch.load(Ordering::SeqCst) != expected
                || warmed && (answer.is_err() || answer.as_ref().ok() == saved_value.as_ref())
            {
                return;
            }
            for event in event(answer, false) {
                gateway.send_snapshot(&epoch, expected, Some(reply.version.clone()), event);
            }
        });
    }

    fn send_read(&self, epoch: &Arc<AtomicU64>, expected: u64, event: Event) {
        self.send_snapshot(epoch, expected, None, event);
    }

    /// The client with the credential generation it signs with, read together under the client's lock.
    fn signed_client(&self) -> (Client, u64) {
        let client = self.inner.client.lock().expect("client lock");
        (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
    }

    /// An answer made with the sign-in of generation `signed`: the window drops it when the sign-in changed before it
    /// lands (`Event::ReadSnapshot`), as an organization switch does.
    fn send_signed(&self, signed: u64, event: Event) {
        self.send_read(&self.inner.cache_epoch, signed, event);
    }
    fn send_snapshot(
        &self,
        epoch: &Arc<AtomicU64>,
        expected: u64,
        revision: Option<(Arc<AtomicU64>, u64)>,
        event: Event,
    ) {
        self.send(Event::ReadSnapshot {
            epoch: epoch.clone(),
            expected,
            event: Box::new(event),
            revision,
        });
    }

    /// The start-readiness spend check has its own deadline, below the client's general request timeout.
    pub fn start_budget_snapshot(&self) -> impl std::future::Future<Output = Result<Value, String>> + 'static {
        let client = self.client();
        let task = self.inner.runtime.spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(25), client.get("/v1/spend"))
                .await
                .map_err(|_| "The budget check timed out.".to_string())?
                .map_err(|error| error.to_string())
        });
        async move { task.await.map_err(|error| error.to_string())? }
    }

    /// Always fresh. Presence must not read an online snapshot from a warm cache.
    pub fn request_detailed(
        &self,
        verb: Verb,
        path: String,
        body: Option<Value>,
    ) -> impl std::future::Future<Output = nexus_client::Result<Value>> + 'static {
        let client = self.client();
        let task = self.inner.runtime.spawn(async move {
            let body = body.unwrap_or_else(|| json!({}));
            match verb {
                Verb::Get => client.get(&path).await,
                Verb::Post => client.post(&path, body).await,
                Verb::Put => client.put(&path, body).await,
                Verb::Patch => client.patch(&path, body).await,
                Verb::Delete => client.delete(&path).await,
            }
        });
        async move {
            task.await
                .map_err(|error| nexus_client::Error::from(nexus_client::error::Transport::Other(error.to_string())))?
        }
    }

    /// One call to the agent API at `base` (`agent.rs`), with the person's token: its head, then its body as it
    /// arrives. Without a base or a sign-in, the answer is a machine-readable 503 or 401, and an API that does not
    /// answer is a 502. Dropping the stream stops the call.
    ///
    /// With `NEXUS_AGENT_VIA_GATEWAY=on` the call goes to this Gateway's `/v1/agent/*` instead, `base` or
    /// not, with the same token and the `Idempotency-Key` every write carries. Gateway's own refusals come back in the
    /// agent API's shape ([`nexus_desktop_core::agent::gateway_failure`]).
    pub fn agent(&self, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        self.agent_with(nexus_desktop_core::agent::via_gateway(), base, None, call)
    }

    /// [`Gateway::agent`] with the Gateway path on or off, as `NEXUS_AGENT_VIA_GATEWAY` would set it.
    #[cfg(test)]
    pub(crate) fn agent_via(&self, via: bool, base: Option<String>, call: crate::agent::Call) -> crate::agent::Stream {
        self.agent_with(via, base, None, call)
    }

    /// [`Gateway::agent`] with another account's own token (`peer_tokens`). A refusal of that token is not a refusal of
    /// this version: it never reaches the `revoked` latch, so a stale peer cannot sign the person out.
    pub fn agent_as(&self, base: Option<String>, token: String, call: crate::agent::Call) -> crate::agent::Stream {
        self.agent_with(nexus_desktop_core::agent::via_gateway(), base, Some(token), call)
    }

    fn agent_with(
        &self,
        via: bool,
        base: Option<String>,
        peer: Option<String>,
        call: crate::agent::Call,
    ) -> crate::agent::Stream {
        use crate::agent::{Chunk, Stream};
        use nexus_desktop_core::agent::{SIGNED_OUT, UNCONFIGURED, UNREACHABLE, failure_body, gateway_failure, target};
        let (out, chunks) = unbounded();
        let fail = |out: &UnboundedSender<Chunk>, (status, error, detail): (u16, &str, &str)| {
            let _ = out.unbounded_send(Chunk::Head {
                status,
                content_type: "application/json".into(),
            });
            let _ = out.unbounded_send(Chunk::Data(failure_body(error, detail).into_bytes()));
            let _ = out.unbounded_send(Chunk::End);
        };
        let client = self.client();
        let own = peer.is_none();
        let (method, path, body) = call.request();
        let routed = target(via, base.as_deref(), &client.options().base_url, &path);
        let (Some((base, path)), Some(token)) = (routed, peer.or_else(|| client.options().api_key.clone())) else {
            fail(
                &out,
                if base.is_none() && !via {
                    UNCONFIGURED
                } else {
                    SIGNED_OUT
                },
            );
            return Stream { chunks, _stop: None };
        };
        let agent = Client::with_http(client_options(&base, Some(token)), client.http().clone());
        let agent = if own {
            agent.observed(self.inner.revoked.observer())
        } else {
            agent
        };
        let awake = matches!(call, crate::agent::Call::Ask { .. }).then(crate::keep_awake::Request::start);
        let task = self.inner.runtime.spawn(async move {
            let _awake = awake;
            use futures::StreamExt as _;
            let headers = [
                ("content-type".to_string(), "application/json".to_string()),
                ("accept".to_string(), "text/event-stream".to_string()),
            ];
            match agent
                .stream(method, &path, &headers, body.map(bytes::Bytes::from))
                .await
            {
                Ok((status, head, body)) => {
                    let content_type = head
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                        .map(|(_, value)| value.clone())
                        .unwrap_or_default();
                    let _ = out.unbounded_send(Chunk::Head { status, content_type });
                    let mut body = std::pin::pin!(body);
                    if via && status >= 400 {
                        // A refusal is short: read it whole, then hand on Gateway's own in the agent API's shape.
                        let mut whole = Vec::new();
                        while let Some(piece) = body.next().await {
                            match piece {
                                Ok(bytes) => whole.extend_from_slice(&bytes),
                                Err(_) => {
                                    let _ = out.unbounded_send(Chunk::Broken);
                                    return;
                                }
                            }
                        }
                        let whole = gateway_failure(&whole).map_or(whole, String::into_bytes);
                        let _ = out.unbounded_send(Chunk::Data(whole));
                        let _ = out.unbounded_send(Chunk::End);
                        return;
                    }
                    while let Some(piece) = body.next().await {
                        match piece {
                            Ok(bytes) => {
                                if out.unbounded_send(Chunk::Data(bytes.to_vec())).is_err() {
                                    return;
                                }
                            }
                            Err(_) => {
                                let _ = out.unbounded_send(Chunk::Broken);
                                return;
                            }
                        }
                    }
                    let _ = out.unbounded_send(Chunk::End);
                }
                Err(_) => fail(&out, UNREACHABLE),
            }
        });
        /// Stops the call when the pane drops its stream.
        struct Stop(tokio::task::AbortHandle);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        Stream {
            chunks,
            _stop: Some(Box::new(Stop(task.abort_handle()))),
        }
    }

    /// Nexus UI's site, for a page that links into it.
    pub fn site_url(&self) -> String {
        nexus_desktop_core::web_url::site(&self.inner.ui_url)
    }

    /// A client for Nexus UI's site, on the same connections, with no token. The site is the UI's address without
    /// `/app`: the docs are at `https://nexus.inc/docs`, and `/app/docs` is the web app's own page.
    fn site(&self) -> Client {
        let ui = self.inner.ui_url.trim_end_matches('/');
        let site = ui.strip_suffix("/app").unwrap_or(ui);
        Client::with_http(ClientOptions::new(site), self.client().http().clone())
    }

    /// Start a dictation; its events come back as [`Event::Dictation`] with `id`.
    pub fn dictate(&self, id: u64) -> Dictation {
        let client = self.client();
        let gateway = self.clone();
        let socket = voice_socket(client);
        dictation::start(&self.inner.runtime, socket, move |event| {
            gateway.send(Event::Dictation { id, event })
        })
    }

    /// Open voice mode's ears: dictation's microphone, and a new speech-to-text token whenever the call needs one.
    pub fn hear(&self) -> crate::voice::Hearing {
        let client = self.client();
        let token: crate::voice::ears::Token = std::sync::Arc::new(move || {
            let client = client.clone();
            Box::pin(async move { voice_socket(client).await.map(|socket| socket.url) })
        });
        crate::voice::ears::hear(&self.inner.runtime, token)
    }

    /// Search Gateway for the palette (`GET /v1/search`): chats, artifacts, people, teams and models.
    pub fn search(&self, seq: u64, query: String) {
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let path = format!(
                "/v1/search?q={}&types=chats,artifacts,people,teams,models&limit=6",
                nexus_client::url::enc(&query)
            );
            let results = client
                .get(&path)
                .await
                .map(|response| SearchHit::from_response(&response))
                .map_err(|error| error.to_string());
            gateway.send_signed(signed, Event::Searched { seq, results });
        });
    }

    /// Where to load artifact `id` (`GET /v1/artifacts/{id}/frame`); `path` is its Nexus UI address.
    pub fn frame_artifact(&self, id: String, path: String) {
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let mint = format!("/v1/artifacts/{}/frame", nexus_client::url::enc(&id));
            let frame = client
                .get(&mint)
                .await
                .map_err(|error| error.to_string())
                .and_then(|response| {
                    let url = text(&response, "url").ok_or("Gateway sent no address for it.")?;
                    Ok(PageFrame {
                        url,
                        title: text(&response, "title"),
                        path,
                        version_key: version_key(&response),
                        mint: Some(mint),
                    })
                });
            gateway.send_signed(signed, Event::Framed(frame));
        });
    }

    /// Where to load your page `id` at `version`, the one a chat answer names (`GET /v1/pages/{id}/versions/{version}/frame`).
    pub fn frame_page_version(&self, id: String, version: u32, path: String) {
        // The pointer rested on the card first: Gateway has answered already.
        let asked = self
            .inner
            .frames
            .lock()
            .ok()
            .and_then(|mut frames| frames.take(&(id.clone(), version), std::time::Instant::now()));
        let mint = format!("/v1/pages/{}/versions/{version}/frame", nexus_client::url::enc(&id));
        if let Some((url, title, version_key)) = asked {
            self.send_signed(
                self.signed_client().1,
                Event::Framed(Ok(PageFrame {
                    url,
                    title,
                    path,
                    version_key,
                    mint: Some(mint),
                })),
            );
            return;
        }
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let frame = client
                .get(&mint)
                .await
                .map_err(|error| error.to_string())
                .and_then(|response| {
                    let url = text(&response, "url").ok_or("Gateway sent no address for it.")?;
                    Ok(PageFrame {
                        url,
                        title: text(&response, "title"),
                        path,
                        version_key: version_key(&response),
                        mint: Some(mint),
                    })
                });
            gateway.send_signed(signed, Event::Framed(frame));
        });
    }

    /// Ask where to load page `id` at `version` before the click, while the pointer rests on its card: the click then
    /// loads the page without the round trip. Nothing is sent to the window; a failed answer is dropped, and the
    /// click asks for itself.
    pub fn prefetch_frame_page_version(&self, id: String, version: u32) {
        let key = (id.clone(), version);
        let Some(era) = self
            .inner
            .frames
            .lock()
            .ok()
            .and_then(|mut frames| frames.ask(&key, std::time::Instant::now()))
        else {
            return;
        };
        let inner = self.inner.clone();
        let client = self.client();
        self.inner.runtime.spawn(async move {
            let answer = client
                .get(&format!(
                    "/v1/pages/{}/versions/{version}/frame",
                    nexus_client::url::enc(&id)
                ))
                .await
                .ok()
                .and_then(|response| {
                    text(&response, "url").map(|url| (url, text(&response, "title"), version_key(&response)))
                });
            if let Ok(mut frames) = inner.frames.lock() {
                frames.answered(&key, era, answer, std::time::Instant::now());
            }
        });
    }

    /// Where to load your page `id` at its latest version (`GET /v1/pages/{id}/versions/latest/frame`).
    pub fn frame_page(&self, id: String, path: String) {
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let frame = client
                .get(&format!(
                    "/v1/pages/{}/versions/latest/frame",
                    nexus_client::url::enc(&id)
                ))
                .await
                .map_err(|error| error.to_string())
                .and_then(|response| {
                    let url = text(&response, "url").ok_or("Gateway sent no address for it.")?;
                    // The version it showed, not the latest: a page tried again is the one that failed.
                    let version = (response.get("version").and_then(Value::as_i64))
                        .map_or_else(|| "latest".to_string(), |version| version.to_string());
                    let mint = format!("/v1/pages/{}/versions/{version}/frame", nexus_client::url::enc(&id));
                    Ok(PageFrame {
                        url,
                        title: text(&response, "title"),
                        path,
                        version_key: version_key(&response),
                        mint: Some(mint),
                    })
                });
            gateway.send_signed(signed, Event::Framed(frame));
        });
    }

    /// The Library's three lists, read at once so each tab can show its count, as the Nexus app does
    /// (`library.tsx`). A Gateway without Pages answers 404 there: an empty Pages list, not an error.
    pub fn load_library(&self, org: Option<String>) {
        const PATHS: [&str; 3] = [
            "/v1/artifacts?scope=shared_with_me&limit=50",
            "/v1/artifacts?scope=shared_by_me&status=all&limit=50",
            "/v1/pages?limit=100",
        ];
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let epoch = self.inner.cache_epoch.clone();
        let version = self.read_version(&client, PATHS[0]).epoch;
        let expected_version = version.load(Ordering::SeqCst);
        let keys = PATHS.map(|path| crate::warm_cache::key(&client, path));
        let (saved, _) = self.inner.cache.snapshot(&keys, &self.inner.cache_revision);
        let warmed = saved[0].is_some() && saved[1].is_some();
        if let (Some(with), Some(by)) = (&saved[0], &saved[1]) {
            self.send_snapshot(
                &epoch,
                expected,
                Some((version.clone(), expected_version)),
                Event::Library(Ok(crate::library::Lists::from_responses(
                    with,
                    by,
                    saved[2].as_ref(),
                    org.as_deref(),
                ))),
            );
        }
        let reads = self.request_batch(PATHS.iter().map(|path| path.to_string()).collect());
        let gateway = self.clone();
        self.inner.runtime.spawn(async move {
            let lists = match reads.await {
                Ok(replies) => match replies.as_slice() {
                    [with, by, pages] => match (&with.answer, &by.answer) {
                        (Ok(with), Ok(by)) => Ok(crate::library::Lists::from_responses(
                            with,
                            by,
                            pages.answer.as_ref().ok(),
                            org.as_deref(),
                        )),
                        (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
                    },
                    _ => Err("The library lists did not all answer.".into()),
                },
                Err(error) => Err(error),
            };
            if warmed && lists.is_err() {
                return;
            }
            gateway.send_snapshot(
                &epoch,
                expected,
                Some((version, expected_version)),
                Event::Library(lists),
            );
        });
    }

    /// A Gateway answer read as text rather than JSON, as a chat's Markdown export
    /// (`GET /v1/chat/sessions/{id}/export.md`).
    pub fn text(&self, path: impl Into<String>) -> impl std::future::Future<Output = Result<String, String>> + 'static {
        let client = self.client();
        let path = path.into();
        let task = self.inner.runtime.spawn(async move {
            let exchange = client
                .exchange(nexus_client::Method::GET, &path, &[], None)
                .await
                .map_err(|error| error.to_string())?;
            if (200..300).contains(&exchange.status) {
                Ok(String::from_utf8_lossy(&exchange.body).into_owned())
            } else {
                Err(format!("Gateway answered {}.", exchange.status))
            }
        });
        async move { task.await.map_err(|error| error.to_string())? }
    }

    /// A shared chat as Gateway writes it out (`GET /v1/artifacts/{id}.md`).
    pub fn chat_text(&self, id: String, title: String, path: String) {
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let path_md = format!("/v1/artifacts/{}.md", nexus_client::url::enc(&id));
            let read = client
                .exchange(nexus_client::Method::GET, &path_md, &[], None)
                .await
                .map_err(|error| error.to_string())
                .and_then(|exchange| {
                    let body = String::from_utf8_lossy(&exchange.body).into_owned();
                    if (200..300).contains(&exchange.status) {
                        Ok(ChatText {
                            title,
                            markdown: body,
                            path,
                        })
                    } else {
                        Err(format!("Gateway answered {}.", exchange.status))
                    }
                });
            gateway.send_signed(signed, Event::ChatText(read));
        });
    }

    /// A fingerprint of the signed-in credential (`nexus_desktop_core::secrets::marker`): tells two sign-ins apart,
    /// reveals nothing usable. `None` while signed out or when a pinned execution side was revoked.
    pub fn marker(&self) -> Option<String> {
        let client = self.inner.client.lock().expect("client lock");
        if self
            .side
            .execution_epoch
            .is_some_and(|pinned| pinned != self.inner.cache_epoch.load(Ordering::SeqCst))
        {
            return None;
        }
        client
            .options()
            .api_key
            .as_deref()
            .map(nexus_desktop_core::secrets::marker)
    }

    fn frozen_session_artifacts(&self) -> FrozenSessionArtifacts {
        FrozenSessionArtifacts {
            client: self.client(),
            runtime: self.inner.runtime.clone(),
            gateway: self.clone(),
        }
    }

    pub fn session_artifacts(&self) -> Arc<dyn crate::backend::SessionArtifacts> {
        Arc::new(self.frozen_session_artifacts())
    }

    fn client(&self) -> Client {
        self.inner.client.lock().expect("client lock").clone()
    }

    /// The first refusal of this version, shared by every window (`workspace/revoked_notice.rs`).
    pub fn revoked(&self) -> Arc<crate::revoked::Latch> {
        self.inner.revoked.clone()
    }

    /// Sessions use the same sign-in as the app. Keychain access is only needed to restore it.
    pub(crate) fn session_client(&self) -> crate::backend::SessionClient {
        let inner = self.inner.clone();
        Arc::new(move || {
            let client = inner.client.lock().expect("client lock").clone();
            client.options().api_key.as_ref()?;
            Some(client)
        })
    }

    fn set_token(&self, token: Option<String>) {
        // An empty saved sign-in is no sign-in: kept, it signed the window in and every call went out as
        // `Bearer ` with no token, which Gateway answers 401 `missing_credential`.
        let token = token.filter(|token| !token.trim().is_empty());
        let signed_out = token.is_none();
        let changed;
        {
            let _transition = crate::backend::SESSION_AUTH_TRANSITION
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut client = self.inner.client.lock().expect("client lock");
            changed = signed_out || client.options().api_key != token;
            if changed {
                self.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
            }
            *client = client.with_api_key(token);
            self.inner.restored.store(true, Ordering::SeqCst);
            // Signed out: no other account's token stays either.
            if signed_out && let Ok(mut peers) = self.inner.peers.lock() {
                peers.clear();
            }
            if changed {
                // Lock order: client -> sides -> Side. Abort schedules cancellation; it never joins a worker.
                // A new client capture must wait until every old turn has been canceled.
                for side in self.sides() {
                    side.abort();
                }
            }
        }
        if changed {
            self.inner.cache_revision.fetch_add(1, Ordering::SeqCst);
            self.inner.read_versions.lock().expect("read versions").clear();
        }
        if signed_out {
            self.inner.cache.wipe();
        }
        // Page addresses belong to the signed-in organization.
        if changed {
            if let Ok(mut frames) = self.inner.frames.lock() {
                frames.clear();
            }
        }
    }

    /// To this window.
    fn send(&self, event: Event) {
        let _ = self.side.events.unbounded_send(event);
    }

    /// To every window: who is signed in is the same in all of them.
    fn send_all(&self, event: impl Fn() -> Event) {
        for side in self.sides() {
            let _ = side.events.unbounded_send(event());
        }
    }

    /// Restore the saved sign-in, if any, and open its connection ahead of the first call.
    pub fn restore(&self) {
        let url = self.inner.url.clone();
        self.restore_with(move || saved_token(&url));
    }

    /// A Gateway chosen while Local starts out resolved with no credential. A later window or
    /// preload must not read this Gateway's saved sign-in until the person explicitly signs in.
    pub(crate) fn resolve_signed_out(&self) {
        debug_assert!(
            self.client().options().api_key.is_none(),
            "resolve only a fresh Gateway"
        );
        self.inner.restored.store(true, Ordering::SeqCst);
    }

    /// Read the saved sign-in before the first window opens, off the UI thread: the keychain is a D-Bus call on Linux
    /// and can be slow. Waits at most `deadline`. Within it, the first frame already knows who is signed in
    /// ([`Gateway::saved_sign_in`]). Past it, the windows start on the startup shell, and the answer reaches every
    /// window when the keychain gives it, as `SignedIn` or `SignedOut`.
    pub fn preload(&self, deadline: std::time::Duration) {
        self.preload_with(deadline, {
            let url = self.inner.url.clone();
            move || saved_token(&url)
        });
    }

    fn preload_with(&self, deadline: std::time::Duration, saved: impl FnOnce() -> Option<String> + Send + 'static) {
        if self.inner.restored.load(Ordering::SeqCst) {
            return;
        }
        let (sender, answer) = std::sync::mpsc::channel();
        let read = std::thread::Builder::new()
            .name("nexus-keychain".into())
            .spawn(move || {
                let _ = sender.send(saved());
            });
        if read.is_err() {
            // No thread: `restore` reads it as before.
            return;
        }
        match answer.recv_timeout(deadline) {
            Ok(token) => {
                if !self.inner.restored.load(Ordering::SeqCst) {
                    self.set_token(token);
                    self.verify_saved();
                }
            }
            Err(_) => {
                let mut pending = self.inner.restore_pending.lock().expect("restore lock");
                *pending = true;
                self.inner.restored.store(true, Ordering::SeqCst);
                drop(pending);
                let gateway = self.clone();
                let late = std::thread::Builder::new()
                    .name("nexus-keychain-late".into())
                    .spawn(move || gateway.finish_late_restore(answer.recv().ok().flatten()));
                if late.is_err() {
                    self.finish_late_restore(None);
                }
            }
        }
    }

    /// The keychain answered after the startup deadline: every window learns who is signed in now.
    fn finish_late_restore(&self, token: Option<String>) {
        let mut pending = self.inner.restore_pending.lock().expect("restore lock");
        self.set_token(token);
        *pending = false;
        let signed_in = self.client().options().api_key.is_some();
        self.send_all(|| if signed_in { Event::SignedIn } else { Event::SignedOut });
        drop(pending);
        self.verify_saved();
    }

    /// Who is signed in, when the saved sign-in was already read (`preload`) and nothing waits on the keychain. None
    /// before then: the window starts on the startup shell until `SignedIn` or `SignedOut` comes.
    pub fn saved_sign_in(&self) -> Option<bool> {
        let pending = self.inner.restore_pending.lock().ok()?;
        if *pending || !self.inner.restored.load(Ordering::SeqCst) {
            return None;
        }
        Some(self.client().options().api_key.is_some())
    }

    fn restore_with(&self, saved: impl FnOnce() -> Option<String>) {
        let pending = self.inner.restore_pending.lock().expect("restore lock");
        // The startup read is still waiting on the keychain: its answer goes to every window then.
        if *pending {
            return;
        }
        let read = !self.inner.restored.swap(true, Ordering::SeqCst);
        if read {
            self.set_token(saved());
        }
        drop(pending);
        if read {
            self.verify_saved();
        }
        let client = self.client();
        self.send(if client.options().api_key.is_some() {
            Event::SignedIn
        } else {
            Event::SignedOut
        });
        if warms(client.options().api_key.as_deref()) {
            self.inner.runtime.spawn(async move {
                let _ = client.warm().await;
            });
        }
    }

    /// An internal build keeps a saved sign-in only for a verified employee (`internal_access.rs`). A refusal signs
    /// out every window and forgets the token; an unknown answer keeps the session and asks again later.
    fn verify_saved(&self) {
        if !crate::internal_access::applies(crate::internal_access::gated(), &self.inner.url) {
            return;
        }
        let (token, authority) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.options().api_key.clone(), self.account_epoch())
        };
        let Some(token) = token else {
            return;
        };
        let gateway = self.clone();
        self.inner.runtime.spawn(async move {
            let mut attempt = 0;
            while authority.current() {
                match crate::internal_access::check(&gateway.inner.url, &token).await {
                    Verdict::Allowed => return,
                    Verdict::Refused => return gateway.refuse_saved(&authority).await,
                    Verdict::Unknown => {
                        tokio::time::sleep(crate::internal_access::retry_after(attempt)).await;
                        attempt += 1;
                    }
                }
            }
        });
    }

    /// Forget a refused saved sign-in only while it still owns the session: a switch, a sign-out or another sign-in
    /// since the check moved the account epoch.
    async fn refuse_saved(&self, authority: &crate::backend::ExecutionAuthority) {
        let _write = self.inner.credential_writes.lock().await;
        if !authority.current() {
            return;
        }
        crate::keychain::forget_token(&self.inner.url);
        self.set_token(None);
        self.send_all(|| Event::SignedOut);
        self.send_all(|| Event::SignInFailed(crate::internal_access::REFUSED.into()));
    }

    /// Start a device sign-in (`nexus auth login`'s flow) and wait for its approval in the browser.
    pub fn sign_in(&self) {
        let Some(cancelled) = self.begin_device_attempt() else {
            if self.marker().is_none()
                && let Some(cancelled) = self.side.new_device_cancellation()
            {
                self.send(Event::DeviceSignInFailed {
                    cancelled,
                    error: "Finish signing in from the other Nexus window first.".into(),
                });
            }
            return;
        };
        let gateway = self.clone();
        let client = self.client().with_api_key(None);
        self.inner.runtime.spawn(async move {
            let label = format!("Nexus for desktop on {}", crate::computer_name::name_or_os());
            let client_id = Some(device::DESKTOP_CLIENT_ID);
            let failed = |error| {
                gateway.send(Event::DeviceSignInFailed {
                    cancelled: cancelled.clone(),
                    error,
                });
                gateway.clear_device_attempt(&cancelled);
            };
            let started = device::start(&client, &label, client_id, &gateway.inner.ui_url).await;
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            let code = match started {
                Ok(code) => code,
                Err(error) => {
                    nexus_desktop_core::signals::sign_in_failed(&LoginError::Failed(error.clone()));
                    return failed(friendly(&error));
                }
            };
            gateway.send(Event::SignInCode {
                cancelled: cancelled.clone(),
                user_code: code.user_code.clone(),
                link: code.link.clone(),
                expires_in: code.expires_in,
            });
            let waited = device::wait(&client, &code, || cancelled.load(Ordering::SeqCst)).await;
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            if let Err(error) = &waited {
                nexus_desktop_core::signals::sign_in_failed(error);
            }
            match waited {
                Ok(grant) => {
                    let verdict = crate::internal_access::check(&gateway.inner.url, &grant.token).await;
                    if verdict == Verdict::Allowed
                        && gateway.finish_device_sign_in(&cancelled, grant.token.clone()).await
                    {
                        return;
                    }
                    // This grant never became the session. Revoke it after a refusal or a cancellation.
                    let _ = client
                        .with_api_key(Some(grant.token))
                        .post("/v1/auth/logout", json!({}))
                        .await;
                    if verdict != Verdict::Allowed && !cancelled.load(Ordering::SeqCst) {
                        failed(verdict.message().into());
                    }
                }
                Err(LoginError::Cancelled) => gateway.clear_device_attempt(&cancelled),
                Err(LoginError::Denied) => failed("Sign-in was denied. Try again for a new code.".into()),
                Err(LoginError::Expired) => failed("The code expired. Try again for a new one.".into()),
                Err(LoginError::Failed(error)) => failed(friendly(&error)),
            }
        });
    }

    fn begin_device_attempt(&self) -> Option<Arc<AtomicBool>> {
        let mut current = self.inner.device_attempt.lock().ok()?;
        if self.marker().is_some() {
            return None;
        }
        let owner = Arc::downgrade(&self.side);
        if current.as_ref().is_some_and(|attempt| {
            attempt.owner.upgrade().is_some_and(|side| !side.events.is_closed()) && !attempt.owner.ptr_eq(&owner)
        }) {
            return None;
        }
        let cancelled = self.side.new_device_cancellation()?;
        if let Some(previous) = current.replace(DeviceAttempt {
            cancelled: cancelled.clone(),
            owner,
        }) {
            previous.cancelled.store(true, Ordering::SeqCst);
        }
        Some(cancelled)
    }

    fn clear_device_attempt(&self, cancelled: &Arc<AtomicBool>) {
        if let Ok(mut current) = self.inner.device_attempt.lock()
            && current
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(&attempt.cancelled, cancelled))
        {
            *current = None;
        }
    }

    /// Whether the grant became the session.
    async fn finish_device_sign_in(&self, cancelled: &Arc<AtomicBool>, token: String) -> bool {
        let _write = self.inner.credential_writes.lock().await;
        {
            // Cancel can land while the final device poll is returning its grant.
            let Ok(current) = self.inner.device_attempt.lock() else {
                return false;
            };
            if !current
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(&attempt.cancelled, cancelled))
                || cancelled.load(Ordering::SeqCst)
            {
                return false;
            }
            self.set_token(Some(token.clone()));
        }
        crate::keychain::save_token(&self.inner.url, &token);
        self.allow_future_restore();
        // The services started by SignedIn read their credentials from the keychain.
        self.send_all(|| Event::SignedIn);
        self.clear_device_attempt(cancelled);
        true
    }

    fn allow_future_restore(&self) {
        if crate::paths::gateway_restore_suppressed(self.url())
            && let Err(error) = crate::paths::set_gateway_restore_suppressed(self.url(), false)
        {
            eprintln!("nexus-desktop-gpui: could not remember Gateway sign-in: {error}");
        }
    }

    /// Switch to another of the person's organizations (`POST /v1/orgs/switch`, as the Nexus app's `switch_org`): its
    /// token replaces this one in the keychain, then `SignedIn` loads everything again for it.
    pub fn switch_org(&self, id: String) {
        self.switch_org_keeping(id, None, None);
    }

    /// [`Gateway::switch_org`] that keeps the token it leaves, in memory, under `leaving` (the organization's key), so
    /// the switcher can read that account's Assistant while the person is in another. `entering` is the key of the
    /// organization it goes to: its kept token, if any, is dropped, because the new one is the sign-in's.
    pub fn switch_org_keeping(&self, id: String, leaving: Option<String>, entering: Option<String>) {
        let gateway = self.clone();
        let client = self.client();
        let authority = self.account_epoch();
        self.inner.runtime.spawn(async move {
            let Some(token) = switch_org_token(&client, id).await else {
                return;
            };
            match token {
                Ok(token) => {
                    let verdict = crate::internal_access::check(&gateway.inner.url, &token).await;
                    let _write = gateway.inner.credential_writes.lock().await;
                    if !authority.current() {
                        return;
                    }
                    if verdict != Verdict::Allowed {
                        return gateway.send(Event::SwitchFailed(verdict.message().into()));
                    }
                    gateway.keep_peer(leaving, entering);
                    crate::keychain::save_token(&gateway.inner.url, &token);
                    // The other organization's pages must not show this one's pictures.
                    crate::web::WebView::clear_snapshots();
                    gateway.set_token(Some(token));
                    gateway.send_all(|| Event::SignedIn);
                }
                Err(error) if authority.current() => gateway.send(Event::SwitchFailed(error)),
                Err(_) => {}
            }
        });
    }

    /// The sign-in's token goes to `leaving`'s place among the peers, and `entering`'s leaves it.
    fn keep_peer(&self, leaving: Option<String>, entering: Option<String>) {
        let Some(current) = self.client().options().api_key.clone() else {
            return;
        };
        let Ok(mut peers) = self.inner.peers.lock() else { return };
        if let Some(entering) = entering {
            peers.remove(&entering);
        }
        if let Some(leaving) = leaving {
            peers.insert(leaving, current);
        }
    }

    /// The other accounts' tokens held this launch, by organization key.
    pub fn peer_tokens(&self) -> Vec<(String, String)> {
        self.inner
            .peers
            .lock()
            .map(|peers| peers.iter().map(|(org, token)| (org.clone(), token.clone())).collect())
            .unwrap_or_default()
    }

    /// A peer's token the agent API refused: it is dropped, so it is not asked again.
    pub fn drop_peer(&self, org: &str) {
        if let Ok(mut peers) = self.inner.peers.lock() {
            peers.remove(org);
        }
    }

    /// Sign in with an API key: kept only when `GET /v1/tenants/me` takes it, so a wrong key changes nothing.
    pub fn sign_in_with_key(&self, key: String) {
        let key = key.trim().to_string();
        if key.is_empty() {
            return self.send(Event::SignInFailed("Paste an API key first.".into()));
        }
        let gateway = self.clone();
        let client = self.client().with_api_key(Some(key.clone()));
        self.inner.runtime.spawn(async move {
            let me = client.get("/v1/tenants/me").await;
            if let Err(error) = &me {
                nexus_desktop_core::signals::key_sign_in_failed(error);
            }
            match me {
                // The person's own key is not revoked on a refusal: it may serve them elsewhere.
                Ok(_) => {
                    let verdict = crate::internal_access::check(&gateway.inner.url, &key).await;
                    if verdict != Verdict::Allowed {
                        return gateway.send(Event::SignInFailed(verdict.message().into()));
                    }
                    let _write = gateway.inner.credential_writes.lock().await;
                    crate::keychain::save_token(&gateway.inner.url, &key);
                    gateway.allow_future_restore();
                    gateway.set_token(Some(key));
                    gateway.send_all(|| Event::SignedIn);
                }
                Err(nexus_client::Error::Core(error)) if matches!(error.status, 401 | 403) => gateway.send(
                    Event::SignInFailed("That key didn't work. Check it and try again.".into()),
                ),
                Err(error) => gateway.send(Event::SignInFailed(friendly(&error))),
            }
        });
    }

    pub fn cancel_sign_in(&self) {
        if let Ok(mut current) = self.inner.device_attempt.lock()
            && let Ok(mut latest) = self.side.latest_device_attempt.lock()
            && let Some(cancelled) = latest.take()
        {
            cancelled.store(true, Ordering::SeqCst);
            if current
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(&attempt.cancelled, &cancelled))
            {
                *current = None;
            }
        }
    }

    fn cancel_all_device_sign_in(&self) {
        if let Ok(mut current) = self.inner.device_attempt.lock()
            && let Some(attempt) = current.take()
        {
            attempt.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Sign out, as the Nexus app's `sign_out`: first, while the token still works, this computer's notification device
    /// is revoked (`path_notices.rs`, a few seconds at most, and only when this build runs it); then the keychain
    /// entry goes.
    pub fn sign_out(&self) {
        let gateway = self.clone();
        self.inner.runtime.spawn(async move {
            // Reserve sign-out before revocation waits on the network. An earlier organization
            // switch either already completed or must wait and see this credential invalidated.
            let _write = gateway.inner.credential_writes.lock().await;
            gateway.cancel_all_device_sign_in();
            let client = gateway.client();
            crate::path_notices::sign_out(&gateway.inner.url, &client).await;
            crate::keychain::forget_token(&gateway.inner.url);
            gateway.set_token(None);
            gateway.send_all(|| Event::SignedOut);
        });
    }

    /// The 50 most recent chat sessions, archived ones left out.
    pub fn list_sessions(&self) {
        self.read_event("/v1/chat/sessions?limit=50&archived=exclude", |answer, _| {
            vec![Event::Sessions(answer.map(|page| {
                page.get("sessions")
                    .and_then(Value::as_array)
                    .map(|sessions| sessions.iter().filter_map(SessionSummary::from_json).collect())
                    .unwrap_or_default()
            }))]
        });
    }

    pub fn open_session(&self, id: String) -> u64 {
        let read = self.inner.chat_read_sequence.fetch_add(1, Ordering::SeqCst);
        self.read_event(
            &format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id)),
            move |answer, cached| {
                let chat = answer.map(|detail| OpenedChat::from_detail(id.clone(), &detail));
                vec![Event::ChatRead {
                    read,
                    id: id.clone(),
                    cached,
                    chat,
                }]
            },
        );
        read
    }

    /// Rewind chat `session` to before its message `message` (`session-view.tsx`'s `rewindMessage`): read each of the
    /// message's `attachments` first, so a failed read cuts nothing, then `POST /v1/chat/sessions/{id}/rewind`.
    /// [`Event::Rewound`] answers with the chat as Gateway keeps it now and the prompt to give back (`text` and the
    /// attachments). `ask` numbers the rewind; a late answer restores the prompt instead of sending it.
    pub fn rewind(&self, ask: u64, session: String, message: String, text: String, attachments: Vec<String>) {
        let gateway = self.clone();
        let mut write = self.begin_write(&format!("/v1/chat/sessions/{}", nexus_client::url::enc(&session)));
        let client = write.client();
        self.inner.runtime.spawn(async move {
            let at = nexus_client::url::enc(&session);
            let reads = attachments.iter().map(|attachment| {
                let path = format!(
                    "/v1/chat/sessions/{at}/attachments/{}",
                    nexus_client::url::enc(attachment)
                );
                let client = &client;
                async move { client.get(&path).await.map_err(|error| error.to_string()) }
            });
            let read = futures::future::join_all(reads).await;
            let outcome = async {
                let prompt = nexus_desktop_core::chat_rewind::restored_prompt(&text, read)?;
                let detail = client
                    .post(
                        &format!("/v1/chat/sessions/{at}/rewind"),
                        json!({ "messageId": message }),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                Ok((OpenedChat::from_detail(session, &detail), prompt))
            }
            .await;
            let outcome = outcome.map_err(|error| (error, text));
            write.finish();
            let (epoch, expected) = write.scope();
            gateway.send_read(epoch, expected, Event::Rewound { ask, outcome });
        });
    }

    /// The chat models the person may pick, for the composer's model menu.
    pub fn list_models(&self) {
        self.read_event("/v1/models?status=all", |answer, _| {
            let page = answer.ok();
            let choices = page
                .as_ref()
                .and_then(|page| page.get("data").and_then(Value::as_array))
                .map(|models| models.iter().filter_map(ModelChoice::from_json).collect())
                .unwrap_or_default();
            vec![
                Event::Models(choices),
                Event::Catalog(page.as_ref().map(crate::catalog::parse).unwrap_or_default()),
            ]
        });
    }

    /// Whether Nexus can pay the next call (`GET /v1/spend`'s `nexusPays`) and, for an owner, the free start's price
    /// cap: the composer's pay notice and the model menu's "Free start" marks.
    pub fn load_pays(&self) {
        self.read_event("/v1/spend", |answer, _| vec![Event::Spend(answer.ok())]);
        self.read_event("/v1/people/me/budget", |answer, _| vec![Event::OwnBudget(answer.ok())]);
        self.read_event("/v1/billing", |answer, _| {
            let cap = answer
                .ok()
                .and_then(|billing| match &billing["pricing"]["freeStartMaxOutputUsd"] {
                    Value::String(text) => text.parse().ok(),
                    other => other.as_f64(),
                });
            vec![Event::FreeStartMax(cap)]
        });
    }

    /// The providers Gateway calls, with their rates, for the Models view's Providers tab.
    pub fn list_providers(&self) {
        self.read_event("/v1/providers", |answer, _| {
            vec![Event::Providers(
                answer.map(|page| crate::catalog::parse_providers(&page)),
            )]
        });
    }

    /// The person's notification channels per group.
    pub fn load_notification_settings(&self) {
        self.read_event("/v1/notifications/settings", |answer, _| {
            vec![Event::NotificationSettings(
                answer.map(|value| crate::settings::channel_settings(&value)),
            )]
        });
    }

    /// Turn one group's email or push on or off. Gateway answers every setting, which replaces the list.
    pub fn set_notification_setting(&self, group: String, channel: String, on: bool) {
        let gateway = self.clone();
        let mut write = self.begin_write("/v1/notifications/settings");
        let client = write.client();
        self.inner.runtime.spawn(async move {
            let body = json!({ "settings": [{ "group": group, "channel": channel, "setting": if on { "on" } else { "off" } }] });
            let settings = client
                .put("/v1/notifications/settings", body)
                .await
                .map(|response| crate::settings::channel_settings(&response))
                .map_err(|error| error.to_string());
            write.finish();
            let (epoch,expected)=write.scope();
            gateway.send_read(epoch,expected,Event::NotificationSettings(settings));
        });
    }

    /// The person's API keys and sign-ins.
    pub fn list_tokens(&self) {
        self.read_event("/v1/auth/tokens", |answer, _| {
            vec![Event::Tokens(answer.map(|response| crate::settings::tokens(&response)))]
        });
    }

    /// Make a personal API key named `label`. The key comes back once; the list is read again after.
    pub fn create_token(&self, label: String) {
        let gateway = self.clone();
        let mut write = self.begin_write("/v1/auth/tokens");
        let client = write.client();
        self.inner.runtime.spawn(async move {
            let made = client
                .post("/v1/auth/tokens", json!({ "label": label }))
                .await
                .map_err(|error| error.to_string())
                .and_then(|response| {
                    crate::settings::NewToken::from_json(&response).ok_or_else(|| "Gateway sent no key.".to_string())
                });
            write.finish();
            let (epoch, expected) = write.scope();
            gateway.send_read(epoch, expected, Event::TokenCreated(made));
            gateway.list_tokens();
        });
    }

    /// Revoke keys or sign-ins one after another, then read the list again once. A failure does not stop the rest.
    pub fn revoke_tokens(&self, ids: Vec<String>) {
        let gateway = self.clone();
        let mut write = self.begin_write("/v1/auth/tokens");
        let client = write.client();
        self.inner.runtime.spawn(async move {
            let mut failed = 0;
            let mut first_error = None;
            for id in &ids {
                let path = format!("/v1/auth/tokens/{}", nexus_client::url::enc(id));
                if let Err(error) = client.delete(&path).await {
                    failed += 1;
                    first_error.get_or_insert(error.to_string());
                }
            }
            write.finish();
            let (epoch, expected) = write.scope();
            if let Some(error) = first_error {
                let what = if ids.len() == 1 {
                    "The key was not revoked".to_string()
                } else {
                    format!("{failed} of {} sign-ins were not revoked", ids.len())
                };
                gateway.send_read(epoch, expected, Event::Tokens(Err(format!("{what}: {error}"))));
            }
            gateway.list_tokens();
        });
    }

    /// A suggested next prompt for `session` (`POST /v1/chat/sessions/{id}/suggest`, as the CLI asks). An error, or a
    /// reply that is not one short line, suggests nothing.
    pub fn suggest(&self, session: String) {
        let gateway = self.clone();
        let (client, signed) = self.signed_client();
        self.inner.runtime.spawn(async move {
            let path = format!("/v1/chat/sessions/{}/suggest", nexus_client::url::enc(&session));
            let Ok(suggested) = client.post(&path, json!({})).await else {
                return;
            };
            if let Some(suggestion) = text(&suggested, "suggestion").and_then(|reply| clean_suggestion(&reply)) {
                gateway.send_signed(signed, Event::Suggested { session, suggestion });
            }
        });
    }

    /// Stop turn `turn`'s answer where it is. Gateway keeps what it stored.
    pub fn stop(&self, turn: u64) {
        self.side.stop(turn);
    }

    /// Ask `prompt` as turn `turn`, with `model` (Auto when `None`) and `attachments` (`attachments::chat_body`): in
    /// `session`, or in a new session titled by the prompt (or by its first attachment, with no text). `choices` give
    /// the mode, the team and web search ([`chat_message_body`], [`new_chat_body`]), as Nexus UI sends them. The answer
    /// streams back as [`Event::Delta`]s, then [`Event::Answered`] with the stored text and its sources.
    pub fn ask(
        &self,
        turn: u64,
        session: Option<String>,
        prompt: String,
        model: Option<String>,
        attachments: crate::attachments::Draft,
        choices: ChatChoices,
    ) {
        let gateway = self.clone();
        let new_session = session.is_none();
        let write_path = session
            .as_ref()
            .map(|id| format!("/v1/chat/sessions/{}", nexus_client::url::enc(id)))
            .unwrap_or_else(|| "/v1/chat/sessions".into());
        let mut write = self.begin_write(&write_path);
        let client = write.client();
        let (epoch, expected) = {
            let (epoch, expected) = write.scope();
            (epoch.clone(), expected)
        };
        // prepare() captured client and expected together under the client mutex. Comparing that immutable
        // capture with the owner's creation epoch prevents a queued old owner from borrowing NEW credentials.
        // A later transition is fenced by the live epoch checks and atomic Side registration below.
        if self.side.execution_epoch.is_some_and(|pinned| pinned != expected) {
            return;
        }
        let auth = epoch.clone();
        let awake = crate::keep_awake::Request::start();
        let task = self.inner.runtime.spawn(async move {
            let _awake = awake;
            let model = model.filter(|model| model != AUTO);
            let mut body = crate::attachments::chat_body(&prompt, model.as_deref(), &attachments);
            chat_message_body(&mut body, &choices);
            let answered = async {
                chat_authority(&auth, expected)?;
                let id = match session {
                    Some(id) => {
                        // Web search is the session's: the choice changed it since the last question.
                        if choices.patch_web_search {
                            let path = format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id));
                            client.patch(&path, json!({ "webSearch": choices.web_search })).await?;
                            chat_authority(&auth, expected)?;
                        }
                        id
                    }
                    None => {
                        let named = attachments.all().next().map(|attachment| attachment.name.as_str());
                        let title = title_from(if prompt.trim().is_empty() {
                            named.unwrap_or_default()
                        } else {
                            &prompt
                        });
                        let create = new_chat_body(&title, &choices);
                        let made = client.post("/v1/chat/sessions", create).await?;
                        chat_authority(&auth, expected)?;
                        let id = text(&made, "id").unwrap_or_default();
                        gateway.send_read(
                            &auth,
                            expected,
                            Event::Created {
                                turn,
                                id: id.clone(),
                                title,
                            },
                        );
                        id
                    }
                };
                chat_authority(&auth, expected)?;
                let _created_write = new_session
                    .then(|| gateway.begin_write(&format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id))));
                let mut streamed = String::new();
                // `npm run measure`'s view from the core: the request starts, and its first token comes off the wire.
                let sent = nexus_desktop_core::metrics::now_ms();
                let mut first = true;
                let mut stored = false;
                chat_authority(&auth, expected)?;
                let outcome = client
                    .stream_chat_message(&id, body, |chunk| {
                        if !stored && let Some(question) = chunk.pointer("/nexus/question") {
                            stored = true;
                            gateway.send_read(
                                &auth,
                                expected,
                                Event::Asked {
                                    turn,
                                    question: crate::model::Stored::from_message(question),
                                },
                            );
                        }
                        let tool = chunk.pointer("/nexus/tool");
                        let writing = tool.and_then(|tool| tool.get("name")).and_then(Value::as_str);
                        let running =
                            tool.and_then(|tool| tool.get("status")).and_then(Value::as_str) == Some("running");
                        if matches!(writing, Some("write_page" | "edit_page")) && running {
                            gateway.send_read(&auth, expected, Event::WritingPage { turn });
                        }
                        if writing == Some("web_search") && running {
                            gateway.send_read(&auth, expected, Event::SearchingWeb { turn });
                        }
                        if let Some(delta) = sse::delta(chunk) {
                            if first {
                                first = false;
                                crate::measure::mark(nexus_desktop_core::metrics::FIRST_TOKEN_CORE);
                                crate::measure::mark_at(nexus_desktop_core::metrics::SEND_CORE, sent);
                            }
                            streamed.push_str(delta);
                            gateway.send_read(
                                &auth,
                                expected,
                                Event::Delta {
                                    turn,
                                    text: delta.to_string(),
                                },
                            );
                        }
                    })
                    .await?;
                let stored = crate::model::content(&outcome.message);
                let page = PageBlock::from_message(&outcome.message);
                let sources = nexus_desktop_core::chat::web_sources(&outcome.message);
                Ok::<_, nexus_client::Error>((if stored.is_empty() { streamed } else { stored }, page, sources))
            }
            .await;
            gateway.side.finished(turn, tokio::task::id());
            write.finish();
            gateway.send_read(
                &auth,
                expected,
                match answered {
                    Ok((content, page, sources)) => Event::Answered {
                        turn,
                        content,
                        page,
                        sources,
                    },
                    Err(error) => Event::AnswerFailed {
                        turn,
                        error: crate::pay_notice::refusal(&error),
                    },
                },
            );
        });
        self.side.register(turn, task.abort_handle(), &epoch, expected);
    }
}

/// The client's credential was captured with this epoch before any chat write began.
fn chat_authority(epoch: &AtomicU64, expected: u64) -> Result<(), nexus_client::Error> {
    if epoch.load(Ordering::SeqCst) == expected {
        Ok(())
    } else {
        Err(nexus_client::error::Transport::Other("The signed-in account changed.".into()).into())
    }
}

/// A question's mode and team, as Nexus UI sends them on every message (`session-view.tsx`, `panel.tsx`): the mode
/// always, the team when there is one.
pub fn chat_message_body(body: &mut Value, choices: &ChatChoices) {
    body["mode"] = json!(CHAT_MODE);
    if let Some(team) = choices.team.as_deref().filter(|team| !team.is_empty()) {
        body["team"] = json!(team);
    }
}

/// A new session's body (`POST /v1/chat/sessions`): the title, the mode,
/// the team when there is one, and `webSearch` only when it is on.
pub fn new_chat_body(title: &str, choices: &ChatChoices) -> Value {
    let mut create = json!({ "title": title });
    chat_message_body(&mut create, choices);
    if choices.web_search {
        create["webSearch"] = json!(true);
    }
    create
}

/// A reply as a prompt: its first line without quotes; nothing when it is empty or too long to be one (as Nexus UI's
/// `cleanSuggestion`).
fn clean_suggestion(reply: &str) -> Option<String> {
    let quotes: &[char] = &['"', '\'', '\u{201c}', '\u{201d}'];
    let line = reply.trim().lines().next()?.trim_matches(quotes).trim();
    (!line.is_empty() && line.chars().count() <= 120).then(|| line.to_string())
}

/// A text file from a site, as it is; an error for any answer but 2xx.
async fn read_text(client: &Client, path: &str) -> Result<String, String> {
    let exchange = client
        .exchange(nexus_client::Method::GET, path, &[], None)
        .await
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&exchange.body);
    // A web page where a text file should be: the site's fallback route, not the file.
    let html = text
        .trim_start()
        .get(..9)
        .is_some_and(|start| start.eq_ignore_ascii_case("<!doctype"));
    if (200..300).contains(&exchange.status) && !html {
        Ok(text.into_owned())
    } else if html {
        Err("The site sent a web page, not the docs.".into())
    } else {
        Err(format!("The site answered {}.", exchange.status))
    }
}

fn optional_skill_library(answer: nexus_client::Result<Value>) -> Result<Value, String> {
    match answer {
        Ok(value) => Ok(value),
        Err(nexus_client::Error::Core(error)) if error.status == 404 => Ok(Value::Null),
        Err(error) => Err(error.to_string()),
    }
}

/// `POST /v1/orgs/switch` for [`Gateway::switch_org_keeping`]: the new organization's token, or why there is none.
/// None, and no call, when the client has no token: Gateway would only answer 401.
async fn switch_org_token(client: &Client, id: String) -> Option<Result<String, String>> {
    if client.options().api_key.as_deref().is_none_or(str::is_empty) {
        eprintln!("nexus-desktop-gpui: organization switch skipped: no sign-in token");
        return None;
    }
    Some(
        client
            .post("/v1/orgs/switch", json!({ "org": id }))
            .await
            .map_err(|error| friendly(&error))
            .and_then(|answer| {
                answer
                    .get("token")
                    .and_then(Value::as_str)
                    // An empty token would be saved and sent as `Bearer `, so it is dropped.
                    .filter(|token| !token.is_empty())
                    .map(String::from)
                    .ok_or_else(|| "Gateway didn't answer a token for that organization.".to_string())
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::{Account, clean_suggestion};
    use serde_json::json;

    #[test]
    fn a_frame_names_its_picture_by_page_and_version() {
        let frame = json!({ "url": "https://x", "pageId": "pag_1", "version": 3 });
        assert_eq!(super::version_key(&frame).as_deref(), Some("pag_1@3"));
        assert_eq!(
            super::version_key(&json!({ "url": "https://x", "pageId": "pag_1" })),
            None
        );
    }

    #[test]
    fn skills_only_fall_back_for_a_missing_library_route() {
        let error = |status| {
            nexus_client::Error::Core(Box::new(nexus_client::error::core_error(
                status,
                &json!({"error":{"message":"unavailable"}}),
            )))
        };
        assert_eq!(
            super::optional_skill_library(Err(error(404))).unwrap(),
            serde_json::Value::Null
        );
        for status in [401, 403, 409, 500, 503] {
            assert!(super::optional_skill_library(Err(error(status))).is_err());
        }
        assert_eq!(
            super::optional_skill_library(Ok(json!({"skills":[]}))).unwrap(),
            json!({"skills":[]})
        );
    }

    /// Every call from this build says it is the desktop app at this build's version, `desktop/<version>`, read off the wire.
    #[test]
    fn calls_carry_the_client_header() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                .unwrap();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });
        nexus_client::install_tls();
        let client = nexus_client::Client::new(super::client_options(&url, Some("key".into())));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(client.get("/v1/tenants/me")).unwrap();
        let request = server.join().unwrap();
        let version = crate::channel::VERSION.to_ascii_lowercase();
        assert!(
            request.contains(&format!("x-nexus-client: desktop/{version}\r\n")),
            "{request}"
        );
        assert!(
            request.contains(&format!("user-agent: nexus-desktop/{version}\r\n")),
            "{request}"
        );
        assert_eq!(super::client(), format!("desktop/{}", crate::channel::VERSION));
    }

    /// An organization switch never goes out without a token, and an empty token in its answer is no token,
    /// so it is never saved or used.
    #[test]
    fn an_org_switch_needs_a_token_and_answers_a_real_one() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let switch = |token: Option<&str>| {
            let client = nexus_client::Client::new(super::client_options(&url, token.map(String::from)));
            runtime.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    super::switch_org_token(&client, "org_2".into()),
                )
                .await
                .expect("a switch with no server answering must not be sent")
            })
        };

        // No token, or an empty one: nothing is sent.
        assert!(switch(None).is_none());
        assert!(switch(Some("")).is_none());
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err(), "a tokenless switch reached the Gateway");
        listener.set_nonblocking(false).unwrap();

        let answer = |body: &'static str| {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..read]);
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(body.as_bytes()).unwrap();
            })
        };

        // An empty token in the answer is refused, not kept.
        let server = answer(r#"{"token":""}"#);
        assert!(matches!(switch(Some("key")), Some(Err(_))));
        server.join().unwrap();

        let server = answer(r#"{"token":"key_2"}"#);
        assert_eq!(switch(Some("key")), Some(Ok("key_2".to_string())));
        server.join().unwrap();
    }

    /// The Assistant's calls (`agent.rs`): the person's token as a bearer, the answer streamed back as it arrives, and the
    /// three failures the app answers itself, each in the agent API's own shape.
    #[test]
    fn the_agent_api_takes_the_token_and_streams_back() {
        use crate::agent::{Call, Chunk};
        use std::io::{Read as _, Write as _};

        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let read = |stream: crate::agent::Stream| {
            let mut chunks = stream.chunks;
            runtime.block_on(async move {
                use futures::StreamExt as _;
                let mut all = Vec::new();
                while let Some(chunk) = chunks.next().await {
                    all.push(chunk);
                }
                all
            })
        };
        let body = |chunks: &[Chunk]| {
            chunks
                .iter()
                .filter_map(|chunk| match chunk {
                    Chunk::Data(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
                    _ => None,
                })
                .collect::<String>()
        };
        let head = |chunks: &[Chunk]| match chunks.first() {
            Some(Chunk::Head { status, content_type }) => (*status, content_type.clone()),
            other => panic!("no head: {other:?}"),
        };
        let ask = || Call::Ask {
            body: nexus_desktop_core::agent::ask_body("hi", None),
        };
        let (gateway, _events) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");

        // No agent API: the 503 the pane reads as "not available here".
        let chunks = read(gateway.agent(None, ask()));
        assert_eq!(head(&chunks).0, 503);
        assert!(body(&chunks).contains("agents_api_unconfigured"));
        // Signed out: 401.
        let chunks = read(gateway.agent(Some("http://127.0.0.1:9".into()), ask()));
        assert_eq!(head(&chunks).0, 401);

        gateway.set_token(Some("tok".into()));
        // Nothing answers: 502, the core's `agents_api_unreachable` problem.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let nobody = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let chunks = read(gateway.agent(Some(nobody), ask()));
        assert_eq!(head(&chunks).0, 502);
        assert!(body(&chunks).contains("agents_api_unreachable"));

        // A fake agent API: it checks the call and answers a recorded stream.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !String::from_utf8_lossy(&request).contains("\"stream\":true}") {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            let sse = "event: started\ndata: {\"conversation\":\"c1\"}\n\nevent: done\ndata: {\"conversation\":\"c1\",\"messages\":[]}\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                sse.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let chunks = read(gateway.agent(Some(base), ask()));
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /agent/ask HTTP/1.1\r\n"), "{request}");
        assert!(
            request.to_ascii_lowercase().contains("authorization: bearer tok\r\n"),
            "{request}"
        );
        assert_eq!(head(&chunks), (200, "text/event-stream".into()));
        assert!(body(&chunks).contains("event: done"));
        assert_eq!(chunks.last(), Some(&Chunk::End));
    }

    /// `NEXUS_AGENT_VIA_GATEWAY=on`: the call goes to Gateway's `/v1/agent/*` with Gateway's token and an
    /// `Idempotency-Key`, with no agent API URL, and the stream arrives event by event. Gateway's own refusal reads as
    /// the agent API's.
    #[test]
    fn through_gateway_the_agent_call_goes_to_v1_agent() {
        use crate::agent::{Call, Chunk};
        use std::io::{Read as _, Write as _};

        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let read = |stream: crate::agent::Stream, mut seen: Box<dyn FnMut(&Chunk) + Send>| {
            let mut chunks = stream.chunks;
            runtime.block_on(async move {
                use futures::StreamExt as _;
                let mut all = Vec::new();
                while let Some(chunk) = chunks.next().await {
                    seen(&chunk);
                    all.push(chunk);
                }
                all
            })
        };
        // A fake Gateway: the ask streams one event, and the next only once the app has read the first; then an
        // errand's action, refused in Gateway's own shape.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (read_first, first_read) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for answer in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !String::from_utf8_lossy(&request).ends_with('}') {
                    let read = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..read]);
                }
                if answer == 0 {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\nevent: started\ndata: {\"conversation\":\"c1\"}\n\n")
                        .unwrap();
                    stream.flush().unwrap();
                    first_read.recv().unwrap();
                    stream
                        .write_all(b"event: done\ndata: {\"conversation\":\"c1\",\"messages\":[]}\n\n")
                        .unwrap();
                } else {
                    let refusal = r#"{"error":{"message":"the agent service did not answer","type":"api_error","code":"agents_api_unreachable"}}"#;
                    write!(
                        stream,
                        "HTTP/1.1 502 Bad Gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{refusal}",
                        refusal.len()
                    )
                    .unwrap();
                }
                requests.push(String::from_utf8(request).unwrap().to_ascii_lowercase());
            }
            requests
        });
        let (gateway, _events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("tok".into()));

        let ask = Call::Ask {
            body: nexus_desktop_core::agent::ask_body("hi", None),
        };
        let mut read_first = Some(read_first);
        let streamed = read(
            gateway.agent_via(true, None, ask),
            Box::new(move |chunk| {
                if matches!(chunk, Chunk::Data(bytes) if String::from_utf8_lossy(bytes).contains("started")) {
                    // The first event arrived before the second was sent: nothing buffers the stream.
                    if let Some(first) = read_first.take() {
                        first.send(()).unwrap();
                    }
                }
            }),
        );
        assert!(
            matches!(streamed.first(), Some(Chunk::Head { status: 200, .. })),
            "{streamed:?}"
        );
        assert_eq!(streamed.last(), Some(&Chunk::End));

        let act = Call::Act {
            path: "/agent/errands/i1".into(),
            body: r#"{"action":"done"}"#.into(),
        };
        let refused = read(
            gateway.agent_via(true, Some("https://agents.test".into()), act),
            Box::new(|_| {}),
        );
        assert!(
            matches!(refused.first(), Some(Chunk::Head { status: 502, .. })),
            "{refused:?}"
        );
        let Some(Chunk::Data(body)) = refused.get(1) else {
            panic!("{refused:?}");
        };
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["error"], "agents_api_unreachable");
        assert_eq!(
            nexus_desktop_core::agent::problem_of(502, Some(&body)).title,
            "Nexus is off"
        );

        let requests = server.join().unwrap();
        assert!(
            requests[0].starts_with("post /v1/agent/ask http/1.1\r\n"),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with("post /v1/agent/errands/i1 http/1.1\r\n"),
            "{}",
            requests[1]
        );
        for request in &requests {
            assert!(request.contains("authorization: bearer tok\r\n"), "{request}");
            assert!(request.contains("idempotency-key: "), "{request}");
        }
    }

    /// Every window shares the sign-in: a sign-out reaches each open window, and a closed window's side drops out.
    /// A search sent before an organization switch answers after it: the window must see it as the old sign-in's.
    #[test]
    fn an_answer_of_an_earlier_sign_in_arrives_stale() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (switched, after_switch) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            after_switch.recv().unwrap();
            let body = r#"{"groups":[]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("nxc_org_a".into()));
        gateway.search(7, "plan".into());
        gateway.set_token(Some("nxc_org_b".into()));
        switched.send(()).unwrap();
        server.join().unwrap();
        let event = runtime.block_on(async {
            use futures::StreamExt as _;
            tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
                .await
                .expect("an answer")
                .expect("an open channel")
        });
        let super::Event::ReadSnapshot {
            epoch, expected, event, ..
        } = event
        else {
            panic!("an answer with no sign-in generation: {event:?}");
        };
        assert!(matches!(*event, super::Event::Searched { seq: 7, .. }));
        assert_ne!(epoch.load(std::sync::atomic::Ordering::SeqCst), expected);
    }

    #[test]
    fn who_is_signed_in_reaches_every_window() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (first, mut first_events) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        let (second, mut second_events) = first.window();
        let (third, third_events) = first.window();
        assert!(std::sync::Arc::ptr_eq(&first.inner, &second.inner));
        drop((third, third_events));
        second.send_all(|| super::Event::SignedOut);
        assert!(matches!(first_events.try_recv(), Ok(super::Event::SignedOut)));
        assert!(matches!(second_events.try_recv(), Ok(super::Event::SignedOut)));
        assert_eq!(first.sides().len(), 2);
        // A window's own answers stay its own.
        second.send(super::Event::Inbox(3));
        assert!(first_events.try_recv().is_err());
        assert!(matches!(second_events.try_recv(), Ok(super::Event::Inbox(3))));
    }

    #[test]
    fn session_client_follows_the_active_sign_in_without_reading_the_keychain() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (gateway, _) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        let session = gateway.session_client();
        assert!(session().is_none());

        // Nothing is saved in the OS store: the active app sign-in alone authorizes sessions.
        gateway.set_token(Some("nxc_active".into()));
        let first = session().expect("the app is signed in");
        assert_eq!(first.options().api_key.as_deref(), Some("nxc_active"));
        assert_eq!(first.options().base_url, gateway.url());

        let (window, _) = gateway.window();
        window.set_token(Some("nxc_other_org".into()));
        assert_eq!(session().unwrap().options().api_key.as_deref(), Some("nxc_other_org"));
        window.set_token(None);
        assert!(session().is_none(), "sign-out must also sign the runner out");
        assert_eq!(
            first.options().api_key.as_deref(),
            Some("nxc_active"),
            "a running agent keeps its snapshot"
        );

        let (other, _) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:10");
        other.set_token(Some("nxc_other_gateway".into()));
        assert!(session().is_none(), "another Gateway cannot restore this sign-in");
    }

    #[test]
    fn restoring_another_window_keeps_the_active_sign_in_and_does_not_undo_sign_out() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (gateway, _) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        gateway.restore_with(|| Some("nxc_saved".into()));
        let (window, mut events) = gateway.window();
        // A later OS-store failure must not replace the shared active credential with None.
        window.restore_with(|| panic!("an open app must not reread the OS store"));
        assert!(matches!(events.try_recv(), Ok(super::Event::SignedIn)));
        assert_eq!(
            gateway.session_client()().unwrap().options().api_key.as_deref(),
            Some("nxc_saved")
        );

        gateway.set_token(None);
        // A leftover OS-store entry must not sign another window in after explicit sign-out.
        window.restore_with(|| panic!("sign-out must not restore a stale OS-store entry"));
        assert!(matches!(events.try_recv(), Ok(super::Event::SignedOut)));
        assert!(gateway.session_client()().is_none());
    }

    /// An empty saved sign-in restores as signed out, so the window sends no account call without a token.
    #[test]
    fn an_empty_saved_sign_in_restores_signed_out() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for saved in ["", "  "] {
            let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
            gateway.restore_with(|| Some(saved.into()));
            assert!(matches!(events.try_recv(), Ok(super::Event::SignedOut)), "{saved:?}");
            assert_eq!(gateway.client().options().api_key, None, "{saved:?}");
            assert!(gateway.session_client()().is_none(), "{saved:?}");
        }
    }

    /// Startup reads the saved sign-in before the window: the window knows who is signed in from its first frame, and
    /// its own restore does not read the keychain again.
    #[test]
    fn a_preloaded_sign_in_is_known_before_the_window() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        assert_eq!(gateway.saved_sign_in(), None);
        gateway.preload_with(std::time::Duration::from_secs(5), || Some("nxc_saved".into()));
        assert_eq!(gateway.saved_sign_in(), Some(true));
        gateway.restore_with(|| panic!("the window must not reread the OS store"));
        assert!(matches!(events.try_recv(), Ok(super::Event::SignedIn)));

        let (signed_out, _) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:10");
        signed_out.preload_with(std::time::Duration::from_secs(5), || None);
        assert_eq!(signed_out.saved_sign_in(), Some(false));
    }

    /// A keychain slower than the deadline: the window starts unknown, its restore waits, and the late answer reaches
    /// every window once.
    #[test]
    fn a_slow_keychain_answers_every_window_when_it_can() {
        nexus_client::install_tls();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        let (release, wait) = std::sync::mpsc::channel::<()>();
        gateway.preload_with(std::time::Duration::from_millis(10), move || {
            let _ = wait.recv();
            Some("nxc_late".into())
        });
        assert_eq!(gateway.saved_sign_in(), None);
        let (window, mut window_events) = gateway.window();
        window.restore_with(|| panic!("the keychain read is already running"));
        assert!(events.try_recv().is_err());
        assert!(window_events.try_recv().is_err());

        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while gateway.saved_sign_in().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(gateway.saved_sign_in(), Some(true));
        assert!(matches!(events.try_recv(), Ok(super::Event::SignedIn)));
        assert!(matches!(window_events.try_recv(), Ok(super::Event::SignedIn)));
        assert!(window_events.try_recv().is_err());
    }

    /// A fake Gateway on loopback: answers each request by its path from `routes` (404 for any other), until
    /// [`stop_fake`] calls `/stop`; returns each request's first line and body, in order, `/stop` left out.
    fn fake_gateway_by_path(
        routes: Vec<(&'static str, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let (head, body) = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::to_string)
                            })
                            .and_then(|length| length.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break (text[..end].to_string(), text[end + 4..].to_string());
                        }
                    }
                };
                let line = head.lines().next().unwrap_or_default().to_string();
                let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                if path == "/stop" {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                    break;
                }
                let answer = routes.iter().find(|(route, _)| *route == path).map(|(_, body)| *body);
                let response = match answer {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => {
                        let body = r#"{"error":{"code":"not_found","message":"attachment unavailable"}}"#;
                        format!(
                            "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                };
                stream.write_all(response.as_bytes()).unwrap();
                seen.push((line, body));
            }
            seen
        });
        (url, server)
    }

    /// End [`fake_gateway_by_path`]'s loop.
    fn stop_fake(url: &str) {
        use std::io::{Read as _, Write as _};
        let mut stream = std::net::TcpStream::connect(url.trim_start_matches("http://")).unwrap();
        stream.write_all(b"GET /stop HTTP/1.1\r\nhost: fake\r\n\r\n").unwrap();
        let _ = stream.read(&mut [0u8; 64]);
    }

    fn current_scoped_event(event: super::Event) -> super::Event {
        let super::Event::ReadSnapshot {
            epoch,
            expected,
            event,
            revision,
        } = event
        else {
            panic!("rewind reply must carry its account scope")
        };
        assert_eq!(epoch.load(std::sync::atomic::Ordering::SeqCst), expected);
        assert!(
            revision
                .as_ref()
                .is_none_or(|(epoch, expected)| epoch.load(std::sync::atomic::Ordering::SeqCst) == *expected)
        );
        *event
    }

    /// Rewind (`session-view.tsx`'s `rewindMessage`) against a fake Gateway: every attachment is read before the
    /// rewind call, the chat comes back with the rewound messages marked, and the prompt carries the files.
    #[test]
    fn a_rewind_reads_the_attachments_then_rewinds() {
        nexus_client::install_tls();
        let detail = r#"{"id":"s1","messages":[
            {"id":"m1","role":"user","content":"First","createdAt":"2026-11-07T10:00:00Z"},
            {"id":"m2","role":"assistant","content":"Answer","createdAt":"2026-11-07T10:00:01Z"},
            {"id":"m3","role":"user","content":"Second","rewound":true,"attachments":[{"id":"a1","name":"chart.png","type":"image","mediaType":"image/png","bytes":3}]},
            {"id":"m4","role":"assistant","content":"Later","rewound":true}]}"#;
        let (url, server) = fake_gateway_by_path(vec![
            (
                "/v1/chat/sessions/s1/attachments/a1",
                r#"{"id":"a1","name":"chart.png","mediaType":"image/png","bytes":3,"data":"YWJj"}"#,
            ),
            ("/v1/chat/sessions/s1/rewind", detail),
        ]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("tok".into()));
        gateway.rewind(7, "s1".into(), "m3".into(), "Second".into(), vec!["a1".into()]);
        let event = runtime
            .block_on(async {
                use futures::StreamExt as _;
                events.next().await
            })
            .map(current_scoped_event);
        stop_fake(&url);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "GET /v1/chat/sessions/s1/attachments/a1 HTTP/1.1");
        assert_eq!(seen[1].0, "POST /v1/chat/sessions/s1/rewind HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&seen[1].1).unwrap(),
            json!({ "messageId": "m3" })
        );
        let Some(super::Event::Rewound {
            ask: 7,
            outcome: Ok((chat, prompt)),
        }) = event
        else {
            panic!("no rewind: {event:?}");
        };
        assert_eq!(chat.id, "s1");
        assert_eq!(chat.messages.len(), 4);
        assert!(!chat.messages[0].stored.rewound);
        assert_eq!(chat.messages[0].stored.id.as_deref(), Some("m1"));
        assert!(chat.messages[2].stored.rewound && chat.messages[3].stored.rewound);
        assert_eq!(chat.messages[2].stored.attachments[0].id, "a1");
        assert_eq!(prompt.text, "Second");
        assert_eq!(prompt.images[0].bytes, b"abc");
    }

    /// An attachment that does not read fails the rewind before Gateway is asked to cut anything.
    #[test]
    fn a_failed_attachment_read_cuts_nothing() {
        nexus_client::install_tls();
        // The rewind route answers, so a rewind call would land and show in what the fake saw.
        let (url, server) = fake_gateway_by_path(vec![("/v1/chat/sessions/s1/rewind", r#"{"id":"s1","messages":[]}"#)]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("tok".into()));
        gateway.rewind(1, "s1".into(), "m3".into(), "Second".into(), vec!["gone".into()]);
        // The event comes after any call the rewind makes, so what the fake saw by then is all of it.
        let event = runtime
            .block_on(async {
                use futures::StreamExt as _;
                events.next().await
            })
            .map(current_scoped_event);
        stop_fake(&url);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].0.starts_with("GET /v1/chat/sessions/s1/attachments/gone "));
        assert!(
            matches!(
                event,
                Some(super::Event::Rewound {
                    ask: 1,
                    outcome: Err((_, ref text))
                }) if text == "Second"
            ),
            "{event:?}"
        );
    }

    #[test]
    fn reads_the_account() {
        let me = json!({ "tenant": { "id": "t1", "name": "Nexus", "slug": "nexus-xyz" }, "user": { "email": "ana@example.com" } });
        let account = Account::from_json(&me).expect("an account");
        assert_eq!(account.name, "ana");
        assert_eq!(account.org, "nexus-xyz");
        assert_eq!(account.org_name, "Nexus");
        assert_eq!(account.user_id, None);
        assert_eq!(account.organization_kind, None);
        let mut identified = me;
        identified["user"]["id"] = json!("u-ana");
        identified["tenant"]["kind"] = json!("company");
        let identified = Account::from_json(&identified).expect("an identified account");
        assert_eq!(identified.user_id.as_deref(), Some("u-ana"));
        assert_eq!(identified.organization_kind.as_deref(), Some("company"));
        assert_eq!(Account::from_json(&json!({})), None);
    }

    /// A fake Gateway on a local port: it answers each call from `answer` and keeps every call's method, path and body.
    fn fake_gateway(
        calls: usize,
        answer: fn(&str, &str) -> (&'static str, String),
    ) -> (
        String,
        std::thread::JoinHandle<Vec<(String, String, serde_json::Value)>>,
    ) {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..calls {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                let head_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(at) = request.windows(4).position(|end| end == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_string();
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().to_string())
                    })
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
                while request.len() < head_end + length {
                    let read = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..read]);
                }
                let mut words = head.split_whitespace();
                let (method, path) = (words.next().unwrap().to_string(), words.next().unwrap().to_string());
                let body = serde_json::from_slice(&request[head_end..head_end + length]).unwrap_or_default();
                let (content_type, reply) = answer(&method, &path);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                seen.push((method, path, body));
            }
            seen
        });
        (url, server)
    }

    /// A stored answer's stream: it searches the web, writes, and is stored with the page it read.
    fn answer_stream() -> String {
        let chunks = [
            json!({ "nexus": { "session": { "id": "s1" }, "question": { "id": "q" } }, "choices": [] }),
            json!({ "nexus": { "tool": { "name": "web_search", "status": "running" } }, "choices": [] }),
            json!({ "choices": [{ "index": 0, "delta": { "content": "Hi" }, "finish_reason": null }] }),
            json!({ "nexus": { "message": { "role": "assistant", "content": "Hi", "sources": [
                { "title": "Release notes", "url": "https://docs.nexus.xyz/releases", "query": "nexus" }
            ] } }, "choices": [] }),
        ];
        let mut sse: String = chunks.iter().map(|chunk| format!("data: {chunk}\n\n")).collect();
        sse.push_str("data: [DONE]\n\n");
        sse
    }

    fn drain(events: &mut futures::channel::mpsc::UnboundedReceiver<super::Event>) -> Vec<super::Event> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut all = Vec::new();
        runtime.block_on(async {
            use futures::StreamExt as _;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while let Some(event) = events.next().await {
                    let super::Event::ReadSnapshot {
                        epoch,
                        expected,
                        revision,
                        event,
                    } = event
                    else {
                        panic!("stream event has no credential envelope: {event:?}");
                    };
                    assert_eq!(
                        epoch.load(std::sync::atomic::Ordering::SeqCst),
                        expected,
                        "stream event must belong to the current sign-in"
                    );
                    assert!(
                        revision.as_ref().is_none_or(|(version, expected)| version
                            .load(std::sync::atomic::Ordering::SeqCst)
                            == *expected),
                        "stream event must carry a current resource version"
                    );
                    let event = *event;
                    let done = matches!(event, super::Event::Answered { .. } | super::Event::AnswerFailed { .. });
                    all.push(event);
                    if done {
                        break;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("stream never completed; received events: {all:?}"));
        });
        all
    }

    /// A new chat goes to Gateway in two calls: the session made with its mode, team and web search,
    /// then the message with its mode and team. The answer's search shows, and its web sources come back with it.
    #[test]
    fn a_new_chat_sends_its_mode_team_and_web_search() {
        nexus_client::install_tls();
        let (url, server) = fake_gateway(2, |method, path| match (method, path) {
            ("POST", "/v1/chat/sessions") => ("application/json", json!({ "id": "s1" }).to_string()),
            _ => ("text/event-stream", answer_stream()),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("tok".into()));
        let choices = super::ChatChoices {
            team: Some("growth".into()),
            web_search: true,
            patch_web_search: false,
        };
        gateway.ask(1, None, "What shipped?".into(), None, Default::default(), choices);
        let seen = drain(&mut events);
        let calls = server.join().unwrap();
        assert_eq!(calls[0].0, "POST");
        assert_eq!(calls[0].1, "/v1/chat/sessions");
        assert_eq!(
            calls[0].2,
            json!({ "title": "What shipped?", "mode": "chat", "team": "growth", "webSearch": true })
        );
        assert_eq!(calls[1].1, "/v1/chat/sessions/s1/messages");
        assert_eq!(calls[1].2["mode"], "chat");
        assert_eq!(calls[1].2["team"], "growth");
        assert!(
            seen.iter()
                .any(|event| matches!(event, super::Event::SearchingWeb { turn: 1 }))
        );
        let sources = seen.iter().find_map(|event| match event {
            super::Event::Answered { sources, .. } => Some(sources.clone()),
            _ => None,
        });
        let sources = sources.expect("an answer");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].label(), "Release notes");
    }

    /// Gateway updates web search before sending to an open chat. It reads the session first to get its version.
    #[test]
    fn an_open_chat_sets_its_web_search_before_it_asks() {
        nexus_client::install_tls();
        let (url, server) = fake_gateway(3, |method, _| match method {
            "GET" => ("application/json", json!({ "id": "s1" }).to_string()),
            "PATCH" => (
                "application/json",
                json!({ "id": "s1", "webSearch": false }).to_string(),
            ),
            _ => ("text/event-stream", answer_stream()),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (gateway, mut events) = super::Gateway::at(runtime.handle().clone(), &url);
        gateway.set_token(Some("tok".into()));
        let choices = super::ChatChoices {
            team: None,
            web_search: false,
            patch_web_search: true,
        };
        gateway.ask(
            2,
            Some("s1".into()),
            "How is auth done?".into(),
            None,
            Default::default(),
            choices,
        );
        drain(&mut events);
        let calls = server.join().unwrap();
        assert_eq!(
            (calls[0].0.as_str(), calls[0].1.as_str()),
            ("GET", "/v1/chat/sessions/s1")
        );
        assert_eq!(
            (calls[1].0.as_str(), calls[1].1.as_str()),
            ("PATCH", "/v1/chat/sessions/s1")
        );
        assert_eq!(calls[1].2, json!({ "webSearch": false }));
        assert_eq!(calls[2].2["mode"], "chat");
        assert!(calls[2].2.get("team").is_none());
    }

    #[test]
    fn reads_the_teams_with_the_account() {
        let me = json!({
            "tenant": { "id": "t1", "name": "Nexus", "slug": "nexus-xyz" },
            "teams": [{ "id": "1", "slug": "growth", "name": "Growth", "role": "member" }],
            "team": { "id": "1", "slug": "growth", "name": "Growth" },
        });
        let account = Account::from_json(&me).expect("an account");
        assert_eq!(account.teams.len(), 1);
        assert_eq!(account.default_team.as_deref(), Some("growth"));
    }

    #[test]
    fn suggestions_are_one_short_line() {
        assert_eq!(
            clean_suggestion("  \"Add tests for it\"\nmore"),
            Some("Add tests for it".into())
        );
        assert_eq!(clean_suggestion("   "), None);
        assert_eq!(clean_suggestion(&"word ".repeat(40)), None);
    }
}

/// The HTTP method of a [`Gateway::request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// The sign-in saved for `gateway`. `npm run measure` signs in with a key from its environment, in memory
/// (`measure.rs`).
fn saved_token(gateway: &str) -> Option<String> {
    crate::measure::key().or_else(|| {
        if crate::paths::gateway_restore_suppressed(gateway) {
            return None;
        }
        keychain_token(gateway)
    })
}

/// `gateway`'s sign-in, as this run holds it (`keychain.rs`): the keychain is read once per launch. A sign-in the
/// keychain would not give reads as none; the sign-in screen says why (`keychain::sign_in_notice`).
pub fn keychain_token(gateway: &str) -> Option<String> {
    crate::keychain::token(gateway).ok().flatten()
}

/// A failed call in the sign-in's words: an unreachable Gateway says so, a paused sign-in says how long to wait
/// (`nexus-client`'s wording), anything else as Gateway said it.
fn friendly(error: &nexus_client::Error) -> String {
    match error {
        nexus_client::Error::Transport(_) => "Can't reach the Gateway. Check the address and your connection.".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod friendly_tests {
    use super::*;
    use nexus_client::error::CoreError;

    fn refused(status: u16, code: &str, interval: Option<f64>) -> nexus_client::Error {
        CoreError {
            status,
            message: "as Gateway said it".into(),
            code: Some(code.into()),
            interval,
            details: None,
        }
        .into()
    }

    #[test]
    fn a_paused_sign_in_says_how_long_to_wait() {
        assert_eq!(
            friendly(&refused(503, "cutover_paused", Some(600.0))),
            "Sign-in is paused for maintenance; try again in 10 minutes."
        );
        assert_eq!(friendly(&refused(503, "draining", Some(5.0))), "as Gateway said it");
    }
}

#[cfg(test)]
mod live_turn_tests {
    use super::*;
    #[tokio::test]
    async fn stop_cancels_one_turn_and_sign_out_cancels_every_remaining_turn() {
        let (events, _receiver) = unbounded();
        let first = tokio::spawn(std::future::pending::<()>());
        let second = tokio::spawn(std::future::pending::<()>());
        let side = Side {
            execution_epoch: None,
            events,
            latest_device_attempt: Mutex::default(),
            answering: Mutex::new(std::collections::HashMap::from([
                (1, first.abort_handle()),
                (2, second.abort_handle()),
            ])),
        };
        side.stop(1);
        assert!(first.await.expect_err("the selected turn stops").is_cancelled());
        assert!(!second.is_finished(), "another chat keeps running");
        side.stop(1); // A late Stop cannot affect the other request.
        assert!(!second.is_finished());
        side.abort();
        assert!(second.await.expect_err("sign-out stops remaining turns").is_cancelled());
        assert!(side.answering.lock().expect("test mutex").is_empty());
    }
}

struct FrozenSessionArtifacts {
    client: Client,
    runtime: tokio::runtime::Handle,
    gateway: Gateway,
}

impl crate::backend::SessionArtifacts for FrozenSessionArtifacts {
    fn share(&self, body: Value) -> crate::backend::Request {
        let client = self.client.clone();
        let write = self.gateway.begin_frozen_write(client.clone(), "/v1/artifacts");
        let task = self.runtime.spawn(async move {
            let _write = write;
            client
                .post("/v1/artifacts/sessions", body)
                .await
                .map_err(|error| error.to_string())
        });
        Box::pin(async move { task.await.map_err(|error| error.to_string())? })
    }

    fn stop(&self, session_key: &str) -> crate::backend::Request {
        let client = self.client.clone();
        let path = format!("/v1/artifacts/sessions/{}", nexus_client::url::enc(session_key));
        let write = self.gateway.begin_frozen_write(client.clone(), "/v1/artifacts");
        let task = self.runtime.spawn(async move {
            let _write = write;
            client.delete(&path).await.map_err(|error| error.to_string())
        });
        Box::pin(async move { task.await.map_err(|error| error.to_string())? })
    }
}

#[cfg(test)]
mod artifact_credential_tests {
    #[test]
    fn a_delayed_share_keeps_the_original_account() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (gateway, _) = super::Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
        gateway.set_token(Some("test-org-one".into()));
        let frozen = gateway.frozen_session_artifacts();
        gateway.set_token(Some("test-org-two".into()));
        assert_eq!(frozen.client.options().api_key.as_deref(), Some("test-org-one"));
        assert_eq!(gateway.client().options().api_key.as_deref(), Some("test-org-two"));
    }
}

fn warms(token: Option<&str>) -> bool {
    token.is_some_and(|token| !token.is_empty())
}

#[cfg(test)]
#[test]
fn restore_warms_only_with_a_token() {
    assert!(!warms(None));
    assert!(!warms(Some("")));
    assert!(warms(Some("token")));
}

#[cfg(test)]
#[test]
fn cancelled_device_grant_cannot_replace_a_sign_in() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (gateway, mut events) = Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
    let attempt = gateway.begin_device_attempt().expect("attempt");
    gateway.cancel_sign_in();
    runtime.block_on(gateway.finish_device_sign_in(&attempt, "cancelled-test-grant".into()));
    assert!(gateway.marker().is_none());
    assert!(events.try_recv().is_err());
}

#[cfg(test)]
#[test]
fn another_window_cannot_cancel_or_replace_the_device_sign_in() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (first, _first_events) = Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
    let (other, mut events) = first.window();
    let active = first.begin_device_attempt().expect("first sign-in");
    other.sign_in();
    assert!(matches!(events.try_recv(), Ok(Event::DeviceSignInFailed { .. })));
    other.cancel_sign_in();
    assert!(!active.load(Ordering::SeqCst));
    first.cancel_sign_in();
    assert!(active.load(Ordering::SeqCst));
    let other_active = other.begin_device_attempt().expect("the other window can now sign in");
    other.clear_device_attempt(&other_active);
    other.cancel_sign_in();
    assert!(
        other_active.load(Ordering::SeqCst),
        "a queued failure is still cancellable"
    );
}

#[cfg(test)]
#[test]
fn a_held_account_context_cannot_dispatch_after_replacing_its_credential() {
    use crate::workspace::gateway_access::AccountGateway;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("count Gateway requests");
    listener.set_nonblocking(true).expect("set nonblocking");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (gateway, _) = Gateway::at(
        runtime.handle().clone(),
        &format!("http://{}", listener.local_addr().unwrap()),
    );
    assert!(gateway.account_credential().is_none());
    gateway.set_token(Some("first-fixture-token".into()));
    let held = AccountGateway::fixture(Arc::new(gateway.clone()));
    assert!(held.current());

    gateway.set_token(None);
    assert!(gateway.account_credential().is_none());
    gateway.set_token(Some("first-fixture-token".into()));
    assert!(
        !held.current(),
        "returning to the same token does not revive the handle"
    );
    let answer = runtime.block_on(held.request(Verb::Post, "/v1/diagnostics/feedback".into(), None));
    assert_eq!(answer.unwrap_err(), "The signed-in account changed.");
    assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[cfg(test)]
#[test]
fn a_gateway_chosen_while_local_stays_signed_out_in_a_later_window() {
    use crate::workspace::gateway_access::GatewayAccess;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (first, _) = Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
    let (gateway, _) = GatewayAccess::new(Arc::new(first)).choose_gateway("http://127.0.0.1:10", false);
    assert_eq!(gateway.saved_sign_in(), Some(false));
    let (later, mut events) = gateway.window();
    assert_eq!(later.saved_sign_in(), Some(false));
    later.restore();
    assert!(matches!(events.try_recv(), Ok(Event::SignedOut)));
    assert!(later.marker().is_none());

    let (second, _) = Gateway::at(runtime.handle().clone(), "http://127.0.0.1:11");
    second.resolve_signed_out();
    second.preload_with(std::time::Duration::ZERO, || panic!("must not read the keychain"));
    second.restore_with(|| panic!("must not read the keychain"));
    assert_eq!(second.saved_sign_in(), Some(false));
    second.set_token(Some("explicit-sign-in-fixture".into()));
    assert_eq!(second.saved_sign_in(), Some(true));
}
