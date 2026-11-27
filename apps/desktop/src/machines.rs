//! Servers: Code sessions on a person's own Linux servers over SSH. Each server in the Nexus app's `settings.json`
//! (`machines` of kind `ssh`) gets one connection while the app runs: nexus-remote's [`Remote`] over its [`SshLink`].
//! The link installs or updates the Nexus host there (the desktop app's own version, `NEXUS_HOST_VERSION`, from the
//! downloads host or a local `NEXUS_HOST_BINARY`), starts it, reads its token and says hello. The relay checks the
//! host's protocol and version, waits for Retry after disconnection, and catches every followed session up from its
//! event log.
//!
//! A server answers the same `wb_*` calls as this computer's workbench and pushes the same `wb:*` events. [`Machines`]
//! keeps what the windows show of each: its projects and sessions (`wb_overview`, then `wb:projects` and `wb:thread`)
//! and the events of each session a window opened (`wb_thread_page`, then `wb:event`). A window reads them as it reads
//! the store (`code.rs`), and sends its commands with [`Machines::call`]. Moments that need the person notify as this
//! computer's do (`notify.rs`).
//!
//! Cloud machines (kind `cloud`) are one more machine here: their sessions, calls and events go
//! the same way, but their relay is the cloud machines' (`cloud.rs`, `nexus_cloud::desktop`), which wake them for a
//! call the person makes, set them up and follow Gateway's list. Until it runs (this build does not own the store),
//! a cloud machine is listed as the Nexus app saved it, and its calls say so ([`NOT_HERE`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use nexus_desktop_core::notify::{AgentNames, Notifier as _};
use nexus_desktop_core::servers::ServerCheck;
use nexus_desktop_core::settings::{MachineSetting, Settings};
use nexus_remote::client::Pushed;
use nexus_remote::install::{Installer, Source};
use nexus_remote::login::Logins;
use nexus_remote::machine::{Backoff, Link, Remote, Sink, SshLink, Status, relayable};
use nexus_remote::ssh::{Target, valid_destination};
use nexus_workbench::event::{Delta, Project, StoredEvent, ThreadStatus, ThreadSummary, WbEvent};
use serde_json::{Value, json};

use crate::cloud::{Cloud, CloudInfo, CloudProgress};

mod server_tools;
pub use server_tools::ServerTools;

use crate::code::{self, Block, CodeProject, CodeSession, Transcript};

/// The Nexus host this build installs: the desktop app's version (`build.rs`).
pub const HOST_VERSION: &str = env!("NEXUS_HOST_VERSION");

/// Where this build's host comes from, checked with the updater's key this build carries (`None` in a keyless build,
/// which skips the signature).
pub fn host_source() -> Source {
    Source::app(HOST_VERSION, crate::updates::verification_key())
}

/// A server's connection or what it holds changed: read [`Machines`] again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub machine: String,
    /// Only a session's streamed answer moved (`stream.rs`): the open view draws again, nothing is read.
    pub streamed: bool,
}

/// A server for the sidebar and Settings › Servers.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub ssh: String,
    pub status: Status,
    /// Sessions working or waiting there: "Update now" stops them.
    pub busy: usize,
}

/// A cloud machine for the sidebar, Settings › Cloud and its session's page: its provider state as Gateway last
/// said ([`CloudInfo`]), where its relay is, and the last step of its create or wake.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudView {
    pub id: String,
    pub name: String,
    pub info: CloudInfo,
    pub status: Status,
    /// The latest step while it is created or woken, or the one that failed (`cloud_progress`).
    pub progress: Option<CloudProgress>,
    /// Its projects and sessions were read once it connected.
    pub read: bool,
}

/// What a cloud machine's call answers while this build does not run the cloud machines.
pub const NOT_HERE: &str =
    "Cloud sessions run in the Nexus app while it is open. Quit it, or take over from Code, to run them here.";

/// How a server's connection is made: over SSH in the app, a stand-in in the tests.
pub type Links = Arc<dyn Fn(&MachineSetting) -> Arc<dyn Link> + Send + Sync>;

/// The servers, their connections and what the windows read of them.
pub struct Machines {
    state: Mutex<State>,
    listeners: Mutex<Vec<UnboundedSender<Changed>>>,
    logins: Logins,
    links: Links,
    backoff: Backoff,
    runtime: tokio::runtime::Handle,
    /// The Gateway the app is signed in to: a server's sign-in goes to it.
    gateway: Mutex<String>,
    account_client: Mutex<Option<nexus_client::Client>>,
    /// The cloud machines' relays, once they run here (`cloud.rs`).
    cloud: Mutex<Option<Cloud>>,
    /// The GitHub sign-in running on each cloud machine (`cloud:github`), its latest word.
    github: Mutex<HashMap<String, Value>>,
    /// The server a chat asked to sign in to Nexus (its error's button): Settings › Remote servers starts it.
    sign_in_asked: Mutex<Option<String>>,
    /// Each server's dev-server forwards, for its sessions' Browser (`machines/previews.rs`). Apart from the state's
    /// lock: a relay's status closes them while it holds its own.
    previews: Mutex<HashMap<String, Arc<nexus_remote::forward::Forwards>>>,
    /// What answers the servers' agents' calls of the app's tools (`server_tools.rs`).
    server_tools: server_tools::ServerToolsCell,
    me: Weak<Machines>,
}

#[derive(Default)]
struct State {
    /// The servers and the cloud machines, in the order they were saved.
    servers: Vec<Server>,
}

/// How a machine is reached.
enum Reach {
    /// A server, over its own SSH relay.
    Ssh(Remote),
    /// A cloud machine, through the cloud machines (`Cloud`), which hold its relay.
    Cloud,
}

/// Where a machine's calls go, taken out of the state's lock.
enum Caller {
    Remote(Remote),
    Cloud(Cloud, String),
    /// A cloud machine while the cloud machines do not run here.
    Away,
}

impl Caller {
    async fn call(&self, call: &str, args: Value) -> Result<Value, String> {
        match self {
            Caller::Remote(remote) => remote.call(call, args).await,
            Caller::Cloud(cloud, id) => cloud.call(id, call, args).await,
            Caller::Away => Err(NOT_HERE.into()),
        }
    }
}

struct Server {
    setting: MachineSetting,
    reach: Reach,
    /// Its projects and sessions were read once connected (and again at each connect).
    read: bool,
    projects: Vec<Project>,
    threads: HashMap<String, ThreadSummary>,
    /// The sessions a window opened, by id.
    followed: HashMap<String, Followed>,
    /// What changed while the connect's `wb_overview` read was on its way. It is newer than that read, so the read
    /// leaves it as it is.
    since_read: Option<SinceRead>,
}

/// The projects and sessions that a call's answer or a push changed after `wb_overview` was asked for.
#[derive(Default)]
struct SinceRead {
    /// A `wb:projects` push came: the host's whole list, newer than the read's.
    projects_pushed: bool,
    projects: HashSet<String>,
    threads: HashSet<String>,
}

impl Server {
    /// Keep the connect's read of the projects and sessions, under what changed since it was asked for.
    fn read_overview(&mut self, projects: Vec<Project>, threads: Vec<ThreadSummary>) {
        let since = self.since_read.take().unwrap_or_default();
        if !since.projects_pushed {
            let mut read = projects;
            for id in &since.projects {
                read.retain(|project| project.id != *id);
                read.extend(self.projects.iter().find(|project| project.id == *id).cloned());
            }
            self.projects = read;
        }
        let mut read: HashMap<String, ThreadSummary> =
            threads.into_iter().map(|thread| (thread.id.clone(), thread)).collect();
        for id in &since.threads {
            read.remove(id);
            if let Some(thread) = self.threads.get(id) {
                read.insert(id.clone(), thread.clone());
            }
        }
        self.threads = read;
        self.read = true;
    }
}

#[derive(Default)]
struct Followed {
    /// By event number: a page from the machine, one from Gateway and a pushed event are one log, so the second copy
    /// of a number replaces the first.
    events: BTreeMap<i64, (i64, WbEvent)>,
    /// What Gateway's history held for it (a cloud machine's session, `history.rs`); None until asked, and on a server.
    history: Option<HistoryRead>,
    /// The first page arrived.
    loaded: bool,
    /// Reading it failed; read again once the server is connected again.
    error: Option<String>,
    /// A read is on its way.
    loading: bool,
    /// Older events exist before the first one held.
    more: bool,
    /// A read of the page before the first event held is on its way.
    loading_older: bool,
    /// The last event number Gateway's latest page held (0: none).
    gateway_last: i64,
    /// Events between Gateway's last and the first one held are missing: older pages come from the machine until
    /// they meet Gateway's.
    gap: bool,
}

/// The app's servers, for every window.
pub fn get() -> Arc<Machines> {
    static MACHINES: OnceLock<Arc<Machines>> = OnceLock::new();
    MACHINES
        .get_or_init(|| {
            let links: Links = Arc::new(|machine: &MachineSetting| {
                Arc::new(SshLink {
                    installer: installer(&machine.ssh),
                }) as Arc<dyn Link>
            });
            Machines::new(links, Backoff::default(), crate::code_run::runtime())
        })
        .clone()
}

/// How to reach `ssh` outside the connection (a check, a sign-in), and to install the host there.
fn installer(ssh: &str) -> Installer {
    Installer {
        target: Target::Ssh {
            destination: ssh.to_string(),
        },
        source: host_source(),
        fetch: Some(host_fetch()),
    }
}

/// The host's downloads, with the sign-in now: a server's and a cloud machine's.
pub fn host_fetch() -> nexus_remote::install::Fetch {
    nexus_desktop_updates::access::host_fetch(Arc::new(|| get().download_sign_in()))
}

/// Where a relay's events go: back into [`Machines`], tagged with the server.
struct MachineSink {
    machines: Weak<Machines>,
    machine: String,
    /// Sub-agents' names, so a notification says which one asks.
    agents: AgentNames,
}

impl Sink for MachineSink {
    fn push(&self, push: &Pushed) {
        if let Some(machines) = self.machines.upgrade()
            && !machines.app_tool(&self.machine, push)
        {
            machines.heard(&self.machine, push, &self.agents);
        }
    }

    fn status(&self, status: &Status) {
        if let Some(machines) = self.machines.upgrade() {
            // No lock of the state here: the relay may hold its own while it says its status.
            if matches!(status, Status::Connected { .. }) {
                machines.connected(&self.machine);
            }
            if matches!(status, Status::Unreachable { .. } | Status::Closed) {
                machines.close_previews(Some(&self.machine));
            }
            machines.changed(&self.machine);
        }
    }
}

impl Machines {
    pub fn new(links: Links, backoff: Backoff, runtime: tokio::runtime::Handle) -> Arc<Machines> {
        Arc::new_cyclic(|me| Machines {
            state: Mutex::new(State::default()),
            listeners: Mutex::new(Vec::new()),
            logins: Logins::default(),
            links,
            backoff,
            runtime,
            gateway: Mutex::new(String::new()),
            account_client: Mutex::new(None),
            cloud: Mutex::new(None),
            github: Mutex::new(HashMap::new()),
            sign_in_asked: Mutex::new(None),
            previews: Mutex::new(HashMap::new()),
            server_tools: Mutex::default(),
            me: me.clone(),
        })
    }

    /// The cloud machines run here from now on: their relays, wakes and setup are `cloud`'s.
    pub fn set_cloud(&self, cloud: Cloud) {
        if let Ok(mut slot) = self.cloud.lock() {
            *slot = Some(cloud);
        }
        self.changed("");
    }

    /// The cloud machines, once they run here.
    pub fn cloud_machines(&self) -> Option<Cloud> {
        self.cloud.lock().ok().and_then(|cloud| cloud.clone())
    }

    /// Where machine `id`'s relay events and statuses go: back here, as a server's do.
    pub fn sink(&self, machine: &str) -> Arc<dyn Sink> {
        Arc::new(MachineSink {
            machines: self.me.clone(),
            machine: machine.to_string(),
            agents: AgentNames::default(),
        })
    }

    /// What the cloud machines said (`cloud:*`, `nexus_cloud::desktop`'s events), with the settings as they saved
    /// them: a machine added, forgotten or changed shows; a step or a GitHub code is read again.
    pub fn cloud_event(&self, event: &str, payload: &Value, settings: &Settings) {
        let machine = payload["machine"].as_str().unwrap_or_default().to_string();
        match event {
            "cloud:machines" | "cloud:state" => self.sync(settings),
            "cloud:github" => {
                if let Ok(mut github) = self.github.lock() {
                    github.insert(machine.clone(), payload.clone());
                }
            }
            _ => {}
        }
        self.changed(&machine);
    }

    /// The latest word of the GitHub sign-in on cloud machine `machine` (`cloud:github`): `{state: code, code, link}`,
    /// `{state: done, user}` or `{state: failed, error}`. Taken once read when it is the last.
    pub fn github_login(&self, machine: &str) -> Option<Value> {
        let mut github = self.github.lock().ok()?;
        let login = github.get(machine).cloned()?;
        if login["state"] != "code" {
            github.remove(machine);
        }
        Some(login)
    }

    /// The Gateway the app signs in to. Another one closes the servers' previews: they belonged to the last sign-in.
    pub fn set_gateway(&self, gateway: &str) {
        let changed = match self.gateway.lock() {
            Ok(mut current) if *current != gateway => {
                *current = gateway.to_string();
                true
            }
            _ => false,
        };
        if changed {
            self.close_previews(None);
        }
    }

    fn gateway(&self) -> String {
        self.gateway.lock().map(|gateway| gateway.clone()).unwrap_or_default()
    }

    pub fn set_account_client(&self, client: Option<nexus_client::Client>) {
        if let Ok(mut current) = self.account_client.lock() {
            *current = client;
        }
    }

    fn download_sign_in(&self) -> (String, Option<String>) {
        if let Some(client) = self.account_client.lock().ok().and_then(|client| client.clone()) {
            let opts = client.options();
            return (opts.base_url.clone(), opts.api_key.clone());
        }
        (self.gateway(), None)
    }

    /// Hears every change from now on, until the receiver is dropped.
    pub fn subscribe(&self) -> UnboundedReceiver<Changed> {
        let (sender, receiver) = unbounded();
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push(sender);
        }
        receiver
    }

    fn changed(&self, machine: &str) {
        self.tell(machine, false);
    }

    fn tell(&self, machine: &str, streamed: bool) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|listener| {
                listener
                    .unbounded_send(Changed {
                        machine: machine.to_string(),
                        streamed,
                    })
                    .is_ok()
            });
        }
    }

    /// Connects to every server `settings` lists that is not connected yet, and closes those it no longer lists. Their
    /// hosts and sessions keep running there. The cloud machines it lists join too; their relays are the cloud
    /// machines' (`cloud.rs`).
    pub fn sync(&self, settings: &Settings) {
        self.sync_settings(settings, true);
    }

    /// Connect configured SSH hosts without restoring any account-backed machines.
    pub fn sync_device(&self, settings: &Settings) {
        self.sync_settings(settings, false);
    }

    fn sync_settings(&self, settings: &Settings, include_cloud: bool) {
        let saved: Vec<&MachineSetting> = settings
            .machines
            .iter()
            .filter(|machine| {
                (machine.kind == "ssh" && valid_destination(machine.ssh.trim()))
                    || (include_cloud && machine.kind == "cloud")
            })
            .collect();
        let mut gone = Vec::new();
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let mut servers = Vec::with_capacity(saved.len());
            let mut old: Vec<Server> = std::mem::take(&mut state.servers);
            for setting in saved {
                let kept = old.iter().position(|server| {
                    server.setting.id == setting.id
                        && match server.reach {
                            // The same server at the same address keeps its connection; a new address connects again.
                            Reach::Ssh(_) => setting.kind == "ssh" && server.setting.ssh == setting.ssh,
                            Reach::Cloud => setting.kind == "cloud",
                        }
                });
                match kept {
                    Some(at) => {
                        let mut server = old.remove(at);
                        server.setting = setting.clone();
                        servers.push(server);
                    }
                    None => servers.push(self.start(setting)),
                }
            }
            gone.extend(old);
            state.servers = servers;
        }
        for server in gone {
            if let Reach::Ssh(remote) = &server.reach {
                remote.close();
            }
            // Removed, or at another address: its forwards reach the old one.
            self.close_previews(Some(&server.setting.id));
            self.logins.cancel(&server.setting.id);
            self.changed(&server.setting.id);
        }
    }

    /// Starts the connection to `setting`. The state's lock is held: the relay's first status can only land after it.
    /// A cloud machine's relay is the cloud machines' own.
    fn start(&self, setting: &MachineSetting) -> Server {
        let reach = if setting.kind == "cloud" {
            Reach::Cloud
        } else {
            let _inside = self.runtime.enter();
            Reach::Ssh(Remote::start_manual(
                (self.links)(setting),
                self.sink(&setting.id),
                self.backoff,
            ))
        };
        Server {
            setting: setting.clone(),
            reach,
            read: false,
            projects: Vec::new(),
            threads: HashMap::new(),
            followed: HashMap::new(),
            since_read: None,
        }
    }

    /// The app is quitting: the connections close; the hosts and their sessions keep running.
    pub fn shutdown(&self) {
        if let Ok(state) = self.state.lock() {
            for server in &state.servers {
                if let Reach::Ssh(remote) = &server.reach {
                    remote.close();
                }
            }
        }
        if let Some(cloud) = self.cloud_machines() {
            cloud.shutdown();
        }
        self.close_previews(None);
    }

    /// Where machine `machine`'s calls go. Read under the state's lock, then let go of it: the cloud machines take
    /// their own locks, and their events come back here.
    fn caller(&self, machine: &str) -> Result<Caller, String> {
        let reach = self.with_server(machine, |server| match &server.reach {
            Reach::Ssh(remote) => Some(remote.clone()),
            Reach::Cloud => None,
        });
        match reach {
            None => Err("No such machine.".into()),
            Some(Some(remote)) => Ok(Caller::Remote(remote)),
            Some(None) => Ok(match self.cloud_machines() {
                Some(cloud) => Caller::Cloud(cloud, machine.to_string()),
                None => Caller::Away,
            }),
        }
    }

    /// The machines connected now, by name, for the worktree sweep: each server's relay, and each cloud machine's
    /// while it is awake.
    pub fn connected_now(&self) -> Vec<(String, Remote)> {
        let saved: Vec<(String, String)> = match self.state.lock() {
            Ok(state) => state
                .servers
                .iter()
                .map(|server| (server.setting.id.clone(), server.setting.name.clone()))
                .collect(),
            Err(_) => return Vec::new(),
        };
        saved
            .into_iter()
            .filter_map(|(id, name)| Some((name, self.remote(&id).ok()?)))
            .filter(|(_, remote)| matches!(remote.status(), Status::Connected { .. }))
            .collect()
    }

    /// Whether machine `machine` is connected and its host names `feature` in its hello (`host.features`).
    pub fn has_feature(&self, machine: &str, feature: &str) -> bool {
        self.remote(machine).is_ok_and(|remote| {
            matches!(remote.status(), Status::Connected { host, .. } if host.features.iter().any(|named| named == feature))
        })
    }

    /// The name machine `machine`'s host gives itself, while it is connected (the Connection pane's execution host).
    pub fn connected_as(&self, machine: &str) -> Option<String> {
        match self.remote(machine).ok()?.status() {
            Status::Connected { host, .. } => Some(host.machine),
            _ => None,
        }
    }

    /// Machine `machine`'s relay: a server's, or a cloud machine's once it has one.
    fn remote(&self, machine: &str) -> Result<Remote, String> {
        match self.caller(machine)? {
            Caller::Remote(remote) => Ok(remote),
            Caller::Cloud(cloud, id) => cloud
                .remote(&id)
                .ok_or_else(|| "Not connected to this machine yet.".to_string()),
            Caller::Away => Err(NOT_HERE.into()),
        }
    }

    fn with_server<T>(&self, machine: &str, read: impl FnOnce(&mut Server) -> T) -> Option<T> {
        let mut state = self.state.lock().ok()?;
        state
            .servers
            .iter_mut()
            .find(|server| server.setting.id == machine)
            .map(read)
    }

    /// The servers, in the order they were added.
    pub fn servers(&self) -> Vec<ServerView> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .servers
            .iter()
            .filter_map(|server| {
                let Reach::Ssh(remote) = &server.reach else {
                    return None;
                };
                Some(ServerView {
                    id: server.setting.id.clone(),
                    name: server.setting.name.clone(),
                    ssh: server.setting.ssh.clone(),
                    status: remote.status(),
                    busy: server
                        .threads
                        .values()
                        .filter(|thread| {
                            matches!(
                                thread.status,
                                ThreadStatus::Running | ThreadStatus::Approval | ThreadStatus::Starting
                            )
                        })
                        .count(),
                })
            })
            .collect()
    }

    /// The cloud machines, in the order they were saved: each as the cloud machines see it now, or as the Nexus app
    /// saved it while they do not run here.
    pub fn cloud(&self) -> Vec<CloudView> {
        let saved: Vec<(MachineSetting, bool)> = match self.state.lock() {
            Ok(state) => state
                .servers
                .iter()
                .filter(|server| matches!(server.reach, Reach::Cloud))
                .map(|server| (server.setting.clone(), server.read))
                .collect(),
            Err(_) => return Vec::new(),
        };
        let cloud = self.cloud_machines();
        saved
            .into_iter()
            .map(|(setting, read)| {
                let (id, name) = (setting.id.clone(), setting.name.clone());
                let (info, status, progress) = match &cloud {
                    Some(cloud) => {
                        let progress = cloud.progress(&id);
                        let view = cloud.view(setting);
                        (view.cloud, view.status, progress)
                    }
                    None => (setting.cloud.as_ref().map(CloudInfo::from), Status::Closed, None),
                };
                CloudView {
                    id,
                    name,
                    info: info.unwrap_or_else(|| CloudInfo::from(&Default::default())),
                    status,
                    progress,
                    read,
                }
            })
            .collect()
    }

    /// Unfinished work on connected servers, including linked sessions hidden from the sidebar.
    pub(crate) fn has_active_work(&self) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .servers
                .iter()
                .any(|server| server.threads.values().any(crate::keep_awake::summary_active))
        })
    }

    /// Every server's projects with their sessions, server by server in the order they were added.
    pub fn projects(&self) -> Vec<CodeProject> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .servers
            .iter()
            .flat_map(|server| {
                code::group(
                    server.projects.clone(),
                    server.threads.values().cloned(),
                    Some(&server.setting.id),
                )
            })
            .collect()
    }

    /// Every server's archived sessions.
    pub fn archived_sessions(&self) -> Vec<CodeSession> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state
            .servers
            .iter()
            .flat_map(|server| {
                server
                    .threads
                    .values()
                    .filter(|thread| thread.archived && !thread.side_chat)
                    .map(|thread| {
                        let mut session = CodeSession::of(thread.clone());
                        session.machine = Some(server.setting.id.clone());
                        session
                    })
            })
            .collect()
    }

    /// Session `thread`'s transcript on server `machine`: every event held, from the first page on. None while its
    /// first page is on its way: this starts it. `older` reads the page before the first event held too.
    pub fn transcript(&self, machine: &str, thread: &str, older: bool) -> Option<Result<Transcript, String>> {
        let (start, before, answer) = self.with_server(machine, |server| {
            let followed = server.followed.entry(thread.to_string()).or_default();
            if let Some(error) = &followed.error {
                return (false, None, Some(Err(error.clone())));
            }
            if !followed.loaded {
                let start = !followed.loading;
                followed.loading = true;
                return (start, None, None);
            }
            let first = followed.events.keys().next().copied();
            let before = first.filter(|_| older && followed.more && !followed.loading_older);
            if before.is_some() {
                followed.loading_older = true;
            }
            let more = followed.more;
            let shown = followed
                .events
                .iter()
                .map(|(seq, (at, event))| (*seq, *at, event.clone()))
                .collect::<Vec<_>>();
            (false, before, Some(Ok((shown, more))))
        })?;
        if start {
            self.read_page(machine, thread, None);
        }
        if let Some(before) = before {
            self.read_page(machine, thread, Some(before));
        }
        // Reducing a long transcript must not hold the mutex that UI status reads use.
        answer.map(|answer| {
            answer.map(|(events, more)| Transcript {
                more,
                ..Transcript::of(events)
            })
        })
    }

    /// Reads session `thread`'s latest events, then follows it; with `before`, the page before that event. A cloud
    /// machine's session reads Gateway's history first (`history.rs`), so it opens while its machine sleeps.
    fn read_page(&self, machine: &str, thread: &str, before: Option<i64>) {
        if !self.read_history(machine, thread, before) {
            self.relay_page(machine, thread, before);
        }
    }

    /// [`Machines::read_page`] from the machine itself, over its relay.
    fn relay_page(&self, machine: &str, thread: &str, before: Option<i64>) {
        let (Ok(remote), Some(machines)) = (self.caller(machine), self.me.upgrade()) else {
            return;
        };
        let (machine, thread) = (machine.to_string(), thread.to_string());
        self.runtime.spawn(async move {
            let page = remote_page(&remote, &thread, before).await;
            // The latest page of a session shown from Gateway: Gateway can lag the machine by more than a page.
            let hole = match (&page, before) {
                (Ok((events, _)), None) => events.first().map(|event| event.0).filter(|first| {
                    machines
                        .with_server(&machine, |server| {
                            server
                                .followed
                                .get(&thread)
                                .is_some_and(|followed| followed.hole_before(*first))
                        })
                        .unwrap_or(false)
                }),
                _ => None,
            };
            machines.with_server(&machine, |server| {
                let followed = server.followed.entry(thread.clone()).or_default();
                match page {
                    Ok((events, more)) => followed.take_page(events, more, before.is_some()),
                    // An older page that failed leaves what is shown alone; the session asks again when it opens again.
                    Err(_) if before.is_some() => followed.loading_older = false,
                    // A catch-up after Gateway's history was shown leaves it shown.
                    Err(_) if followed.loaded => {}
                    Err(error) => {
                        followed.loading = false;
                        followed.error = Some(error);
                    }
                }
            });
            machines.changed(&machine);
            if let Some(first) = hole {
                history::bridge(&machines, &remote, &machine, &thread, first).await;
                machines.changed(&machine);
            }
        });
    }

    /// Server `machine` connected: read its projects and sessions, and read again the sessions that failed to read.
    fn connected(&self, machine: &str) {
        let (Ok(remote), Some(machines)) = (self.caller(machine), self.me.upgrade()) else {
            return;
        };
        let retry: Vec<String> = self
            .with_server(machine, |server| {
                server
                    .followed
                    .iter_mut()
                    .filter(|(_, followed)| followed.error.is_some())
                    .map(|(id, followed)| {
                        followed.error = None;
                        followed.loading = true;
                        id.clone()
                    })
                    .collect()
            })
            .unwrap_or_default();
        for thread in retry {
            self.read_page(machine, &thread, None);
        }
        // Sessions shown from Gateway's history while the machine slept: what it logged since Gateway's copy.
        let kept: Vec<String> = self
            .with_server(machine, |server| {
                server
                    .followed
                    .iter()
                    .filter(|(_, followed)| followed.loaded && followed.history == Some(HistoryRead::Kept))
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for thread in kept {
            self.relay_page(machine, &thread, None);
        }
        self.with_server(machine, |server| server.since_read = Some(SinceRead::default()));
        self.hand_mcp_servers_to(machine);
        let machine = machine.to_string();
        self.runtime.spawn(async move {
            let Ok(overview) = remote.call("wb_overview", json!({})).await else {
                machines.with_server(&machine, |server| server.since_read = None);
                return;
            };
            let projects = serde_json::from_value::<Vec<Project>>(overview["projects"].clone()).unwrap_or_default();
            let threads = serde_json::from_value::<Vec<ThreadSummary>>(overview["threads"].clone()).unwrap_or_default();
            machines.with_server(&machine, |server| server.read_overview(projects, threads));
            machines.changed(&machine);
        });
    }

    /// What server `machine` pushed: its projects, a session's summary, or a followed session's event.
    fn heard(&self, machine: &str, push: &Pushed, agents: &AgentNames) {
        let mut notify = None;
        let mut streamed = false;
        let known = self.with_server(machine, |server| match push.event.as_str() {
            "wb:projects" => {
                if let Ok(projects) = serde_json::from_value::<Vec<Project>>(push.payload.clone()) {
                    server.projects = projects;
                    if let Some(since) = &mut server.since_read {
                        since.projects_pushed = true;
                    }
                }
                true
            }
            "wb:thread" => {
                if let Ok(thread) = serde_json::from_value::<ThreadSummary>(push.payload.clone()) {
                    if let Some(since) = &mut server.since_read {
                        since.threads.insert(thread.id.clone());
                    }
                    server.threads.insert(thread.id.clone(), thread);
                }
                true
            }
            "wb:event" => {
                let Ok(stored) = serde_json::from_value::<StoredEvent>(push.payload.clone()) else {
                    return false;
                };
                if let Some(body) =
                    crate::notify::message(&stored.event, agents.read(&stored.thread_id, &stored.event).as_deref())
                {
                    let title = server
                        .threads
                        .get(&stored.thread_id)
                        .map(|thread| thread.title.clone())
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| "Code session".into());
                    notify = Some((stored.thread_id.clone(), title, body));
                }
                crate::stream::event(&stored.thread_id, &stored.event);
                match server.followed.get_mut(&stored.thread_id) {
                    Some(followed) => {
                        followed.events.insert(stored.seq, (stored.at, stored.event));
                        true
                    }
                    None => false,
                }
            }
            "wb:delta" => {
                if let Ok(delta) = serde_json::from_value::<Delta>(push.payload.clone())
                    && crate::stream::delta(&delta)
                {
                    streamed = true;
                }
                false
            }
            _ => false,
        });
        if streamed {
            self.tell(machine, true);
        }
        if let Some((thread, title, body)) = notify {
            crate::notify::GpuiNotifier.show(Some(machine), &thread, &title, &body);
        }
        if known == Some(true) {
            self.changed(machine);
        }
    }

    /// Session `thread` on machine `machine`, as last read.
    pub fn thread(&self, machine: &str, thread: &str) -> Option<ThreadSummary> {
        self.with_server(machine, |server| server.threads.get(thread).cloned())
            .flatten()
    }

    /// Machine `machine`'s projects, as last read.
    pub fn projects_on(&self, machine: &str) -> Vec<Project> {
        self.with_server(machine, |server| server.projects.clone())
            .unwrap_or_default()
    }

    /// Clones `source` (a URL or GitHub's `owner/repo`) into `<parent>/<its name>` on server `machine`
    /// (`wb_project_clone`), as the Nexus app's `cloneProject` does. Answers the project's id; the project joins the
    /// server's list at once ([`Machines::call`]). Where it stands is [`Machines::clone_progress`] of `request`.
    pub async fn clone_project(
        &self,
        machine: &str,
        source: &str,
        parent: &str,
        request: &str,
    ) -> Result<String, String> {
        let args = json!({ "source": source, "parent": parent, "requestId": request });
        let project = self
            .call(machine, "wb_project_clone", args)
            .await
            .map_err(|_| "The server's answer was lost.".to_string())??;
        serde_json::from_value::<Project>(project)
            .map(|project| project.id)
            .map_err(|_| "The server answered something else.".to_string())
    }

    /// Git's progress on clone `request` on server `machine` (`wb_project_clone_progress`): its stage and percentage.
    /// None before git said anything, or when the ask failed (the next ask reads it again).
    pub async fn clone_progress(&self, machine: &str, request: &str) -> Option<(String, Option<u8>)> {
        let asked = self
            .call(machine, "wb_project_clone_progress", json!({ "requestId": request }))
            .await;
        clone_stage(&asked.ok()?.ok()?)
    }

    /// Lets go of machine `machine` and what was read of it: a cloud machine deleted here (its setting is gone too).
    pub fn forget(&self, machine: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.servers.retain(|server| server.setting.id != machine);
        }
        self.changed(machine);
    }

    /// The server a session or a project is on; None when no server has it (this computer's). A cloud session is on
    /// its own machine even before that machine's sessions were read (it sleeps): its message then wakes it there.
    pub fn machine_of(&self, id: &str) -> Option<String> {
        let state = self.state.lock().ok()?;
        state
            .servers
            .iter()
            .find(|server| server.threads.contains_key(id) || server.projects.iter().any(|project| project.id == id))
            .or_else(|| {
                state.servers.iter().find(|server| {
                    server
                        .setting
                        .cloud
                        .as_ref()
                        .is_some_and(|cloud| cloud.session_id.as_deref() == Some(id))
                })
            })
            .map(|server| server.setting.id.clone())
    }

    /// Whether `machine` is a cloud machine, not a server reached over SSH.
    pub fn is_cloud(&self, machine: &str) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .servers
                .iter()
                .any(|server| server.setting.id == machine && matches!(server.reach, Reach::Cloud))
        })
    }

    /// A `wb_*` call on server `machine`, with the arguments this computer's workbench takes. What it answers about a
    /// session (a new one, a change) is kept at once; the server's own push follows.
    pub fn call(&self, machine: &str, call: &str, args: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (answer, answered) = oneshot::channel();
        // A sleeping cloud machine wakes for a call the person makes, not for a read (`Cloud::call`).
        let remote = relayable(call).and_then(|()| self.caller(machine));
        let Ok(remote) = remote else {
            let _ = answer.send(remote.map(|_| Value::Null));
            return answered;
        };
        let (machine, call, me) = (machine.to_string(), call.to_string(), self.me.clone());
        self.runtime.spawn(async move {
            let thread = args.get("threadId").and_then(Value::as_str).map(str::to_string);
            let result = remote.call(&call, args).await;
            if let (Ok(value), Some(machines)) = (&result, me.upgrade()) {
                machines.answered(&machine, &call, thread.as_deref(), value);
            }
            let _ = answer.send(result);
        });
        answered
    }

    /// The person's action on a session's shared terminal on server `machine`, from the window that checked it: the
    /// host's `host_terminal`, which it takes only over SSH, where the hello carried its token. A cloud machine has no
    /// shared terminal, and its calls go through Gateway (`Cloud::call`), so this refuses it.
    pub fn terminal(
        &self,
        machine: &str,
        thread: &str,
        op: &str,
        args: Value,
    ) -> oneshot::Receiver<Result<Value, String>> {
        let (answer, answered) = oneshot::channel();
        let remote = match self.caller(machine) {
            Ok(remote) => remote,
            Err(error) => {
                let _ = answer.send(Err(error));
                return answered;
            }
        };
        let call = json!({ "threadId": thread, "op": op, "args": args });
        self.runtime.spawn(async move {
            let _ = answer.send(remote.call(nexus_workbench::terminal::SERVICE_CALL, call).await);
        });
        answered
    }

    /// A message's files as machine `machine` takes them: small ones whole, larger ones uploaded there in pieces, for
    /// session `thread_id` only.
    pub async fn upload(
        &self,
        machine: &str,
        thread_id: &str,
        draft: &crate::attachments::Draft,
    ) -> Result<Vec<nexus_workbench::attachments::FileInput>, String> {
        crate::attachments::sendable_files(
            thread_id,
            draft,
            |chunk| {
                let answered = self.call(machine, "wb_attachment_chunk", json!(chunk));
                async move {
                    let value = answered
                        .await
                        .map_err(|_| "The server's answer was lost.".to_string())??;
                    serde_json::from_value::<nexus_workbench::attachments::Progress>(value)
                        .map_err(|error| error.to_string())
                }
            },
            |upload_id| {
                let answered = self.call(
                    machine,
                    "wb_attachment_cancel",
                    json!({ "uploadId": upload_id, "threadId": thread_id }),
                );
                async move {
                    answered
                        .await
                        .map_err(|_| "The server's answer was lost.".to_string())?
                        .map(drop)
                }
            },
        )
        .await
    }

    /// Keeps what a call's answer says about the server's sessions.
    fn answered(&self, machine: &str, call: &str, thread: Option<&str>, value: &Value) {
        let kept = self.with_server(machine, |server| {
            // A project added or changed there joins its list at once; the server's `wb:projects` brings the same.
            if call.starts_with("wb_project_")
                && let Ok(project) = serde_json::from_value::<Project>(value.clone())
            {
                if let Some(since) = &mut server.since_read {
                    since.projects.insert(project.id.clone());
                }
                match server.projects.iter_mut().find(|known| known.id == project.id) {
                    Some(known) => *known = project,
                    None => server.projects.push(project),
                }
                return true;
            }
            if call == "wb_thread_delete" {
                if let Some(thread) = thread {
                    if let Some(since) = &mut server.since_read {
                        since.threads.insert(thread.to_string());
                    }
                    server.threads.remove(thread);
                    server.followed.remove(thread);
                }
                return true;
            }
            match serde_json::from_value::<ThreadSummary>(value.clone()) {
                Ok(summary) => {
                    if let Some(since) = &mut server.since_read {
                        since.threads.insert(summary.id.clone());
                    }
                    server.threads.insert(summary.id.clone(), summary);
                    true
                }
                Err(_) => false,
            }
        });
        if kept == Some(true) {
            self.changed(machine);
        }
    }

    /// "Retry": connect again now. A cloud machine wakes when it sleeps.
    pub fn reconnect(&self, machine: &str) -> Result<(), String> {
        match self.caller(machine)? {
            Caller::Remote(remote) => {
                remote.reconnect();
                Ok(())
            }
            Caller::Cloud(cloud, id) => cloud.wake(&id),
            Caller::Away => Err(NOT_HERE.into()),
        }
    }

    /// "Update now": the host's running sessions stop as when the app quits; each shows "Continue" afterwards.
    pub fn update(&self, machine: &str) -> oneshot::Receiver<Result<(), String>> {
        let (answer, answered) = oneshot::channel();
        match self.remote(machine) {
            Ok(remote) => {
                self.runtime.spawn(async move {
                    let _ = answer.send(remote.update().await);
                });
            }
            Err(error) => {
                let _ = answer.send(Err(error));
            }
        }
        answered
    }

    /// Reaches an SSH host before it is added ("Test connection"): ssh's own reason when it cannot.
    pub fn test(&self, ssh: &str) -> oneshot::Receiver<Result<ServerCheck, String>> {
        let ssh = ssh.trim().to_string();
        let (answer, answered) = oneshot::channel();
        if !valid_destination(&ssh) {
            let _ = answer.send(Err(format!("{ssh:?} is not an SSH host.")));
            return answered;
        }
        self.runtime.spawn(async move {
            let probe = nexus_remote::probe::probe(&Target::Ssh { destination: ssh }).await;
            let _ = answer.send(probe.map(|probe| ServerCheck::of(&probe)));
        });
        answered
    }

    /// What server `machine` has now (its sign-ins): the host's Nexus sign-in, Claude Code and Codex.
    pub fn probe(&self, machine: &str) -> oneshot::Receiver<Result<ServerCheck, String>> {
        let (answer, answered) = oneshot::channel();
        let Some(ssh) = self.ssh_of(machine) else {
            let _ = answer.send(Err("No such machine.".into()));
            return answered;
        };
        self.runtime.spawn(async move {
            let probe = nexus_remote::probe::probe(&Target::Ssh { destination: ssh }).await;
            let _ = answer.send(probe.map(|probe| ServerCheck::of(&probe)));
        });
        answered
    }

    /// Makes server `machine`'s host start on boot (`nexus-host service install --keep-running`). The host running
    /// now and its sessions go on.
    pub fn start_on_boot(&self, machine: &str) -> oneshot::Receiver<Result<(), String>> {
        let (answer, answered) = oneshot::channel();
        let Some(ssh) = self.ssh_of(machine) else {
            let _ = answer.send(Err("No such machine.".into()));
            return answered;
        };
        self.runtime.spawn(async move {
            let _ = answer.send(nexus_remote::probe::start_on_boot(&installer(&ssh)).await);
        });
        answered
    }

    /// Server `machine`'s SSH address; None for a cloud machine, reached where its last connect was.
    fn ssh_of(&self, machine: &str) -> Option<String> {
        self.with_server(machine, |server| match server.reach {
            Reach::Ssh(_) => Some(server.setting.ssh.clone()),
            Reach::Cloud => None,
        })
        .flatten()
    }

    /// Signs server `machine`'s host in to Nexus with its own device token. The code to approve goes to `codes`
    /// (`link`, `code`); answers once approved (the organization, when named) and the host restarted, then reconnects.
    pub fn sign_in(
        &self,
        machine: &str,
        gateway: &str,
        codes: UnboundedSender<(String, String)>,
    ) -> oneshot::Receiver<Result<Option<String>, String>> {
        let (answer, answered) = oneshot::channel();
        let (Some(ssh), Ok(remote), Some(machines)) = (self.ssh_of(machine), self.remote(machine), self.me.upgrade())
        else {
            let _ = answer.send(Err("No such machine.".into()));
            return answered;
        };
        let (machine, gateway) = (machine.to_string(), gateway.to_string());
        self.runtime.spawn(async move {
            let signed = machines
                .logins
                .sign_in(&machine, installer(&ssh), gateway, move |link, code| {
                    let _ = codes.unbounded_send((link.to_string(), code.to_string()));
                })
                .await;
            if signed.is_ok() {
                remote.reconnect();
            }
            let _ = answer.send(signed);
        });
        answered
    }

    /// Asks Settings › Remote servers to sign server `machine` in to Nexus: it opens that server's Sign-ins and starts
    /// the sign-in, now if the page is open, else when it opens.
    pub fn ask_sign_in(&self, machine: &str) {
        if let Ok(mut asked) = self.sign_in_asked.lock() {
            *asked = Some(machine.to_string());
        }
        self.changed(machine);
    }

    /// The server a chat asked to sign in, taken once read.
    pub fn take_sign_in_ask(&self) -> Option<String> {
        self.sign_in_asked.lock().ok()?.take()
    }

    /// Stops server `machine`'s sign-in on its way.
    pub fn cancel_sign_in(&self, machine: &str) {
        self.logins.cancel(machine);
    }
}

/// Whether a server in `status` clones a repository there: connected, and its host says it does (`clone.ts`
/// `cloneOffered`).
pub fn clones(status: &Status) -> bool {
    matches!(status, Status::Connected { host, .. } if host.features.iter().any(|feature| feature == nexus_workbench::clone::FEATURE))
}

/// A `wb_project_clone_progress` answer as a stage and a percentage; None while git said nothing yet (`null`).
fn clone_stage(progress: &Value) -> Option<(String, Option<u8>)> {
    let stage = progress.get("stage")?.as_str()?.to_string();
    let percent = progress
        .get("percent")
        .and_then(Value::as_u64)
        .and_then(|percent| u8::try_from(percent).ok());
    Some((stage, percent))
}

/// The hosts the person's `~/.ssh/config` names, for "Add a server".
pub fn ssh_hosts() -> Vec<String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    home.map(|home| nexus_remote::ssh_config::hosts(&home))
        .unwrap_or_default()
}

#[path = "machines/history.rs"]
mod history;
pub use history::{HistoryRead, OMITTED};

#[path = "machines/previews.rs"]
mod previews;

#[cfg(test)]
#[path = "machines/tests.rs"]
pub(crate) mod tests;

#[cfg(all(test, unix))]
#[path = "machines/sshd.rs"]
mod sshd;

#[path = "machines/pull_sessions.rs"]
mod pull_sessions;

/// Session `thread`'s page before `before` (the latest without it) from the machine behind `remote`, and whether
/// events exist before it.
async fn remote_page(
    remote: &Caller,
    thread: &str,
    before: Option<i64>,
) -> Result<(Vec<(i64, i64, WbEvent)>, bool), String> {
    let page = remote
        .call(
            "wb_thread_page",
            json!({ "threadId": thread, "beforeSeq": before, "limit": code::EVENTS }),
        )
        .await?;
    let more = page.get("more").and_then(Value::as_bool).unwrap_or(false);
    serde_json::from_value::<Vec<StoredEvent>>(page.get("events").cloned().unwrap_or(Value::Null))
        .map(|events| {
            let events = events
                .into_iter()
                .map(|stored| (stored.seq, stored.at, stored.event))
                .collect();
            (events, more)
        })
        .map_err(|error| format!("The server's answer to wb_thread_page: {error}"))
}
