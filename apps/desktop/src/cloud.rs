//! Cloud machines: one Linux VM per Code session, which Nexus Gateway starts at its
//! provider. This build runs them through `nexus_cloud::desktop`: Gateway's list,
//! create, wake, sleep and delete, the relay to each one's host, the setup on every connect, and the Claude and GitHub
//! sign-ins it gets from this computer. Here: [`GpuiEnv`], what the cloud machines need from this build (the Nexus
//! app's settings.json and keychain, the sign-in, and `machines.rs` for their relays and events), and two steps the
//! screens take: a new cloud session ([`start_session`], `startCloudSession`) and its end ([`end_session`],
//! `endCloudSessions`). The words the screens show are the Nexus app's (`words`, from its `cloud-state.ts`).
//!
//! The cloud machines run only while this build owns the store ([`start`]): they write
//! settings.json. While the Nexus app runs, it runs them, and they are listed here as it saved them.

pub mod setup_line;
pub mod waiting;
pub mod words;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nexus_client::Client;
use nexus_desktop_core::secrets::Vault;
use nexus_desktop_core::settings::Settings;
use nexus_remote::install::{Fetch, Source};
use nexus_remote::machine::{Sink, Status};
use nexus_workbench::api::CreateArgs;
use nexus_workbench::event::{Project, ThreadStatus, ThreadSummary};
use serde_json::{Value, json};

pub use nexus_cloud::desktop::{
    Cloud, CloudAccount, CloudEnv, CloudInfo, CloudPort, CloudProgress, CloudRepo, CloudState, CloudStep,
    GITHUB_NOT_CONNECTED, Options,
};

use crate::code_run::NewSession;
use crate::machines::Machines;
use waiting::{Message, Waiting};

/// Where a new cloud session's repository is cloned on its machine (`clone.ts`'s `DEFAULT_PARENT`).
const CLONE_PARENT: &str = "~";

/// How often a session's start looks at its machine while it comes up.
const LOOK: Duration = Duration::from_millis(200);

/// How long a machine may take from leaving Gateway's queue to running Nexus. Past it, the start fails.
const STUCK: Duration = Duration::from_secs(180);

/// Why a start that took longer than [`STUCK`] failed.
pub const STUCK_ERROR: &str = "The machine did not start within 3 minutes.";

/// Why a start stops after the person ended its session.
const ENDED: &str = "The session was ended.";

/// nexus-cloud's words for any 403 from Gateway (`gateway::error`): the error code does not reach this build.
const ROLE_REFUSAL: &str = "Your role in this organization can't use cloud machines.";

/// What the cloud machines need from this build: the Nexus app's settings.json (written only while this build owns
/// the store) and keychain, the Gateway sign-in, and `machines.rs`, where their relays' events and their own go.
pub struct GpuiEnv {
    machines: Arc<Machines>,
    client: Mutex<Option<Client>>,
}

impl GpuiEnv {
    fn new(machines: Arc<Machines>, client: Client) -> GpuiEnv {
        GpuiEnv {
            machines,
            client: Mutex::new(Some(client)),
        }
    }
}

impl CloudEnv for GpuiEnv {
    fn settings(&self) -> Settings {
        crate::desktop_settings::load()
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), String> {
        crate::desktop_settings::update(|settings| {
            change(settings);
            Ok(())
        })
        .map(drop)
    }

    fn vault(&self) -> Vault {
        crate::keychain::vault()
    }

    fn gateway(&self) -> Option<Client> {
        self.client.lock().ok()?.clone()
    }

    fn emit(&self, event: &str, payload: Value) {
        self.machines.cloud_event(event, &payload, &self.settings());
    }

    fn sink(&self, machine: &str) -> Arc<dyn Sink> {
        self.machines.sink(machine)
    }

    fn source(&self) -> Source {
        crate::machines::host_source()
    }

    fn fetch(&self) -> Option<Fetch> {
        Some(crate::machines::host_fetch())
    }

    /// The Nexus app's data folder, so either app reaches a machine with the key the other made.
    fn data_dir(&self) -> PathBuf {
        crate::paths::data_dir().unwrap_or_else(|| std::env::temp_dir().join("nexus-desktop"))
    }
}

static RUNNING: OnceLock<(Cloud, Arc<GpuiEnv>)> = OnceLock::new();

/// Run the cloud machines here, once this build owns the store: their saved relays start (a sleeping one stays
/// asleep), and Gateway's list is asked for now and every 30 seconds. Once for the app's life.
pub fn start(client: Client) {
    if RUNNING.get().is_some() {
        return;
    }
    let machines = crate::machines::get();
    let env = Arc::new(GpuiEnv::new(machines.clone(), client));
    let runtime = crate::code_run::runtime();
    let _inside = runtime.enter();
    let cloud = Cloud::new(env.clone(), Options::default());
    if RUNNING.set((cloud.clone(), env)).is_ok() {
        machines.set_cloud(cloud.clone());
        cloud.start();
    }
}

/// The cloud machines, while they run here.
pub fn get() -> Option<Cloud> {
    RUNNING.get().map(|(cloud, _)| cloud.clone())
}

/// The sign-in changed (signed in or out, another organization or Gateway): the previous one's cloud machines end
/// here before this returns, and the new one's are asked for.
pub fn session_changed(client: Option<Client>) {
    if let Some((cloud, env)) = RUNNING.get() {
        if let Ok(mut current) = env.client.lock() {
            *current = client;
        }
        cloud.session_changed();
    }
}

/// A new cloud session (`startCloudSession`): its first message and choices, the repository to clone, and the request
/// id its session and machine are named from (`t_<request id>`), the same on a retry. `waiting` holds the messages
/// sent after the first while the machine starts; a clone shares them. `cloning` is where the repository's clone
/// stands while it runs, for the start's line; a clone shares it too.
#[derive(Debug, Clone)]
pub struct CloudStart {
    pub request_id: String,
    pub repo: String,
    pub new: NewSession,
    pub waiting: Waiting,
    pub cloning: CloneProgress,
}

impl CloudStart {
    /// The session's id, which names its machine at Gateway too.
    pub fn session_id(&self) -> String {
        format!("t_{}", self.request_id)
    }
}

/// Where a new cloud session's clone stands while it runs, as git last said it: its stage and percent. None before
/// the clone starts and after it ends. It also holds whether the machine's own setup failed and the start said so, so
/// Try again runs it again ([`attach_setup`]). Cloned, it is the same slot.
#[derive(Debug, Clone, Default)]
pub struct CloneProgress {
    progress: Arc<Mutex<Option<(String, Option<u8>)>>>,
    setup_failed: Arc<AtomicBool>,
}

impl CloneProgress {
    pub fn get(&self) -> Option<(String, Option<u8>)> {
        self.progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set(&self, progress: Option<(String, Option<u8>)>) {
        *self.progress.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = progress;
    }
}

/// What a cloud machine's own setup says (`wb_cloud_setup`, contract.ts `CloudSetup`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupView {
    state: String,
    repo: Option<String>,
    stage: Option<String>,
    percent: Option<u8>,
    project: Option<Project>,
    reason: Option<String>,
}

/// Waits for `machine`'s own setup of `start`'s repository (`wb_cloud_setup`), with its progress in
/// `start.cloning`, and answers its project. None when the machine has none (an older host, the setup off, another
/// repository) or its answer was lost: the app then clones as before, and the host joins that clone to a running
/// setup. A failure fails the start with its reason, once; Try again runs the setup again.
async fn attach_setup(machines: &Machines, machine: &str, start: &CloudStart) -> Result<Option<Project>, String> {
    if start.cloning.setup_failed.swap(false, Ordering::SeqCst)
        && answer(machines, machine, "wb_cloud_setup_retry", json!({}))
            .await
            .is_err()
    {
        return Ok(None);
    }
    let attached = loop {
        if start.waiting.ended() {
            break Err(ENDED.to_string());
        }
        let Some(view) = answer(machines, machine, "wb_cloud_setup", json!({}))
            .await
            .ok()
            .and_then(|view| serde_json::from_value::<SetupView>(view).ok())
        else {
            break Ok(None);
        };
        if !view.repo.as_deref().is_some_and(|repo| same_repo(repo, &start.repo)) {
            break Ok(None);
        }
        match view.state.as_str() {
            "ready" => break Ok(view.project),
            "failed" => {
                start.cloning.setup_failed.store(true, Ordering::SeqCst);
                let reason = view.reason.unwrap_or_else(|| "no reason given".into());
                break Err(format!("Setting up {} failed: {reason}", view.repo.unwrap_or_default()));
            }
            "setup" => start.cloning.set(Some((words::RUNNING_SETUP.into(), None))),
            "cloning" => start
                .cloning
                .set(Some((view.stage.unwrap_or_else(|| "Starting".into()), view.percent))),
            _ => break Ok(None),
        }
        tokio::time::sleep(CLONE_POLL).await;
    };
    start.cloning.set(None);
    attached
}

/// Whether `a` and `b` (a GitHub URL or `owner/name`) are the same repository.
fn same_repo(a: &str, b: &str) -> bool {
    fn key(text: &str) -> String {
        let text = text.trim().to_ascii_lowercase();
        let text = ["https://github.com/", "http://github.com/", "git@github.com:"]
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix))
            .unwrap_or(&text);
        let text = text.trim_end_matches('/');
        text.strip_suffix(".git").unwrap_or(text).to_string()
    }
    key(a) == key(b)
}

/// How often a new session's clone is asked where it stands.
const CLONE_POLL: Duration = Duration::from_millis(500);

/// Reads where clone `request` on `machine` stands every [`CLONE_POLL`] into `cloning`, until it is dropped.
async fn watch_clone(
    machines: &Machines,
    machine: &str,
    request: &str,
    cloning: &CloneProgress,
) -> std::convert::Infallible {
    loop {
        tokio::time::sleep(CLONE_POLL).await;
        if let Some(progress) = machines.clone_progress(machine, request).await {
            cloning.set(Some(progress));
        }
    }
}

/// Starts a cloud session: Gateway gives session `t_<request id>`
/// its own machine (`Cloud::create`: the same machine on a retry, queued while Nexus has no room), and this waits
/// until it runs and its host was read. Then the repository is cloned there (once, and through a lost connection:
/// [`clone_repo`]), and the session starts there with the first message. A machine that sets the repository up itself
/// at boot is waited for instead ([`attach_setup`]). The messages sent while it started follow, in order ([`Waiting`]). A retry after a failed step resumes the machine from it; a session already made gets the
/// message again only when it stopped. `settings` reads what the cloud machines saved. Answers the session's id.
pub async fn start_session(
    machines: &Machines,
    cloud: &Cloud,
    settings: &(dyn Fn() -> Settings + Sync),
    start: &CloudStart,
) -> Result<String, String> {
    let id = start.session_id();
    let mut clock = StartClock::new(&id);
    let title = words::session_title(&start.new.text);
    let machine = match cloud.create_for(&id, &title, Some(&start.repo)).await {
        Ok(made) => made.machine.id,
        Err(error) => return Err(refusal(error, || cloud.sync()).await),
    };
    clock.lap("create");
    machines.sync(&settings());
    // Try again after a failed step: the machine resumes from it, and that step's error no longer counts.
    let failed = cloud.progress(&machine).filter(|step| step.error.is_some());
    if failed.is_some() {
        cloud.retry(&machine)?;
    }
    running(machines, &machine, failed, &start.waiting).await?;
    clock.lap("machine");
    let new = &start.new;
    if let Some(previous) = machines.thread(&machine, &id) {
        if matches!(previous.status, ThreadStatus::Error | ThreadStatus::Interrupted) {
            send(machines, &machine, &id, &new.text, &new.attachments).await?;
        }
        send_waiting(machines, &machine, &id, &start.waiting).await?;
        return Ok(id);
    }
    let project = match machines.projects_on(&machine).into_iter().next() {
        Some(project) => {
            clock.lap("project-there");
            project.id
        }
        None => match attach_setup(machines, &machine, start).await? {
            Some(project) => {
                clock.lap("project-setup");
                project.id
            }
            None => {
                let project = clone_repo(machines, &machine, start).await?;
                clock.lap("project-clone");
                project
            }
        },
    };
    // Ended while it cloned: the session is not made.
    if start.waiting.ended() {
        return Err(ENDED.into());
    }
    let thread = nexus_workbench::creation_thread_id(&start.request_id)?;
    let files = machines.upload(&machine, &thread, &new.attachments).await?;
    let args = CreateArgs {
        request_id: Some(start.request_id.clone()),
        project_id: project,
        agent: new.agent.as_str().to_string(),
        model: new.model.clone(),
        permission_mode: new.permission_mode.clone(),
        effort: new.effort.map(|effort| effort.as_str().to_string()),
        via: new.via.as_str().to_string(),
        provider: None,
        images: crate::attachments::session_images(&new.attachments),
        text: new.text.clone(),
        worktree: None,
        orchestration: None,
        name: None,
        files,
    };
    let call = if args.files.is_empty() {
        "wb_thread_create"
    } else {
        "wb_thread_create_files"
    };
    let made = answer(machines, &machine, call, json!(args)).await?;
    let made = serde_json::from_value::<ThreadSummary>(made)
        .map(|thread| thread.id)
        .map_err(|error| format!("The machine's answer to {call}: {error}"))?;
    clock.lap("session");
    clock.report();
    send_waiting(machines, &machine, &made, &start.waiting).await?;
    Ok(made)
}

/// How long each step of one cloud start took, from the click to the session holding the first message. It goes to
/// the app's log as one line when the session is made, so a slow start says which step was slow. A failed start
/// writes nothing: its error says where it stopped.
struct StartClock {
    session: String,
    began: std::time::Instant,
    last: std::time::Instant,
    laps: Vec<(&'static str, std::time::Duration)>,
}

impl StartClock {
    fn new(session: &str) -> StartClock {
        let now = std::time::Instant::now();
        StartClock {
            session: session.to_string(),
            began: now,
            last: now,
            laps: Vec::new(),
        }
    }

    /// Ends step `name` now.
    fn lap(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        self.laps.push((name, now - self.last));
        self.last = now;
    }

    fn line(&self) -> String {
        let laps = self
            .laps
            .iter()
            .map(|(name, took)| format!("{name} {:.1}s", took.as_secs_f64()))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "cloud start {}: {laps}; first message in {:.1}s",
            self.session,
            (self.last - self.began).as_secs_f64()
        )
    }

    fn report(&self) {
        eprintln!("nexus-desktop-gpui: {}", self.line());
    }
}

/// Sends the messages that waited for session `id`, in order, each answered before the next: the host queues them
/// behind the running turn. A failed one goes back first, and the start fails with why; Try again sends it then.
/// Only an error that proves the message did not arrive puts it back. A lost answer does not: the host may have the
/// message, and sending it again would send it twice, so the error says to look in the session.
async fn send_waiting(machines: &Machines, machine: &str, id: &str, waiting: &Waiting) -> Result<(), String> {
    send_each(waiting, |message| async move {
        send(machines, machine, id, &message.text, &message.attachments).await
    })
    .await
}

/// `send_waiting`'s rule, with the one send given: each message once, in order; a failure that proves the message did
/// not arrive puts it back, a lost answer does not.
async fn send_each<F, Fut>(waiting: &Waiting, mut send: F) -> Result<(), String>
where
    F: FnMut(Message) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    while let Some(message) = waiting.next() {
        if let Err(error) = send(message.clone()).await {
            if error == LOST {
                return Err(format!(
                    "{error} Your last message may have reached the session, so it is not sent again: check the session."
                ));
            }
            waiting.put_back(message);
            return Err(error);
        }
    }
    Ok(())
}

/// One message to session `id`, with its files (`wb_send`, or `wb_send_files` when it has some).
async fn send(
    machines: &Machines,
    machine: &str,
    id: &str,
    text: &str,
    attachments: &crate::attachments::Draft,
) -> Result<(), String> {
    let files = machines.upload(machine, id, attachments).await?;
    let images = crate::attachments::session_images(attachments);
    let call = if files.is_empty() { "wb_send" } else { "wb_send_files" };
    let args = json!({ "threadId": id, "text": text, "images": images, "files": files });
    answer(machines, machine, call, args).await.map(drop)
}

/// What a refused create says. nexus-cloud words every 403 as the role's. Gateway lets any role that may use cloud
/// machines list them, and refuses only the create with 403 `cloud_not_enabled` when the organization has them off.
/// So a refused create whose list then works was the organization's (`list` asks Gateway's list again).
async fn refusal<F: Future<Output = Result<(), String>>>(error: String, list: impl FnOnce() -> F) -> String {
    if error != ROLE_REFUSAL {
        return error;
    }
    match list().await {
        Ok(()) => words::refusal("cloud_not_enabled").to_string(),
        Err(_) => error,
    }
}

/// What a call says when the machine's answer never came: the call may or may not have been done.
const LOST: &str = "The machine's answer was lost.";

/// A call on machine `machine`, awaited.
async fn answer(machines: &Machines, machine: &str, call: &str, args: Value) -> Result<Value, String> {
    machines.call(machine, call, args).await.map_err(|_| LOST.to_string())?
}

/// Whether a call's error says its answer was lost on the way, not that the call failed: the host may still be doing
/// it (nexus-host's server.rs runs each call on its own, and a dropped connection does not stop it).
fn lost(error: &str) -> bool {
    error == LOST || error == nexus_remote::client::LOST
}

/// How long a start waits for its machine's connection to come back after it dropped during the clone.
const RECONNECT: Duration = Duration::from_secs(120);

/// What a start says when the connection dropped during the clone and did not come back in [`RECONNECT`].
const NOT_BACK: &str = "It did not come back within 2 minutes.";

/// What a start says when the clone ended on the machine and made no project.
const CLONE_FAILED: &str = "The repository could not be cloned on the machine.";

/// Clones `repo` on machine `machine` (`wb_project_clone`) and answers its project's id. A lost answer is not a failed
/// clone: the host clones on. The start then waits for the connection to come back and asks the host how the clone
/// went ([`after_lost`]).
async fn clone_repo(machines: &Machines, machine: &str, start: &CloudStart) -> Result<String, String> {
    let request = uuid::Uuid::new_v4().to_string();
    let args = json!({ "source": start.repo, "parent": CLONE_PARENT, "requestId": request });
    // Git's progress is in `start.cloning` until the clone ends or its answer is lost.
    start.cloning.set(Some(("Starting".into(), None)));
    let clone = answer(machines, machine, "wb_project_clone", args);
    let watch = watch_clone(machines, machine, &request, &start.cloning);
    let cloned = match futures::future::select(Box::pin(clone), Box::pin(watch)).await {
        futures::future::Either::Left((cloned, _)) => cloned,
        futures::future::Either::Right((never, _)) => match never {},
    };
    start.cloning.set(None);
    match cloned {
        Ok(cloned) => serde_json::from_value::<Project>(cloned)
            .map(|project| project.id)
            .map_err(|error| format!("The machine's answer to wb_project_clone: {error}")),
        Err(error) if lost(&error) => {
            after_lost(&start.waiting, Patience::START, || look(machines, machine, &request)).await
        }
        Err(error) => Err(error),
    }
}

/// Where a clone whose answer was lost stands, as its machine says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cloning {
    /// The machine is not connected, or did not answer.
    Away,
    /// The host still clones.
    Running,
    /// The clone ended: the machine's project, or none when the clone failed.
    Ended(Option<String>),
}

/// How [`after_lost`] waits: at most `reconnect` with no answer from the machine, looking again after `first`, then
/// twice as long each time up to `most`.
#[derive(Debug, Clone, Copy)]
struct Patience {
    reconnect: Duration,
    first: Duration,
    most: Duration,
    /// How long a clone that ended with no project waits for its project before it counts as failed.
    grace: Duration,
}

impl Patience {
    const START: Patience = Patience {
        reconnect: RECONNECT,
        first: Duration::from_millis(500),
        most: Duration::from_secs(5),
        grace: Duration::from_secs(5),
    };
}

/// Asks machine `machine` how clone `request` stands, once it is connected again: still running
/// (`wb_project_clone_progress`), or ended with the machine's project, if any (`wb_overview`).
async fn look(machines: &Machines, machine: &str, request: &str) -> Cloning {
    let connected = machines
        .cloud()
        .into_iter()
        .any(|view| view.id == machine && matches!(view.status, Status::Connected { .. }));
    if !connected {
        return Cloning::Away;
    }
    match answer(
        machines,
        machine,
        "wb_project_clone_progress",
        json!({ "requestId": request }),
    )
    .await
    {
        Ok(Value::Null) => {}
        Ok(_) => return Cloning::Running,
        Err(_) => return Cloning::Away,
    }
    match answer(machines, machine, "wb_overview", json!({})).await {
        Ok(overview) => {
            let projects = serde_json::from_value::<Vec<Project>>(overview["projects"].clone()).unwrap_or_default();
            Cloning::Ended(projects.into_iter().next().map(|project| project.id))
        }
        Err(_) => Cloning::Away,
    }
}

/// Follows a clone whose answer was lost until it ends: answers its project's id. It fails when the machine does not
/// answer for [`Patience::reconnect`], when the clone ended with no project (for [`Patience::grace`]: the host adds the
/// project just after the clone ends), or when the session was ended.
async fn after_lost<F, Fut>(waiting: &Waiting, patience: Patience, mut look: F) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Cloning>,
{
    let mut answered = Instant::now();
    let mut backoff = patience.first;
    let mut ended_at: Option<Instant> = None;
    loop {
        if waiting.ended() {
            return Err(ENDED.into());
        }
        let pause = match look().await {
            Cloning::Ended(Some(project)) => return Ok(project),
            Cloning::Ended(None) if ended_at.is_some_and(|since| since.elapsed() >= patience.grace) => {
                return Err(CLONE_FAILED.into());
            }
            Cloning::Away => {
                if answered.elapsed() >= patience.reconnect {
                    return Err(format!("{} {NOT_BACK}", nexus_remote::client::LOST));
                }
                let pause = backoff;
                backoff = (backoff * 2).min(patience.most);
                pause
            }
            // Running, or ended with no project yet.
            seen => {
                ended_at = if seen == Cloning::Ended(None) {
                    ended_at.or_else(|| Some(Instant::now()))
                } else {
                    None
                };
                answered = Instant::now();
                backoff = patience.first;
                patience.first
            }
        };
        tokio::time::sleep(pause).await;
    }
}

/// The clock of a start that may get stuck: it runs from when the machine left Gateway's queue (a queued machine
/// waits as long as Nexus has no room), and starts over when it is queued again.
#[derive(Debug, Default)]
struct Deadline {
    since: Option<Instant>,
}

impl Deadline {
    /// Whether the start is past `limit`, with the machine `queued` or not at `now`.
    fn passed(&mut self, queued: bool, now: Instant, limit: Duration) -> bool {
        if queued {
            self.since = None;
            return false;
        }
        now.duration_since(*self.since.get_or_insert(now)) >= limit
    }
}

/// Resolves once cloud machine `id` runs and its host was read; fails when it fails or goes (`machineRunning`), when
/// it does not run [`STUCK`] after it left the queue, or when the session was ended. `retried` is the failed step a
/// retry started from: it stays the machine's last step until the next one.
async fn running(
    machines: &Machines,
    id: &str,
    retried: Option<CloudProgress>,
    waiting: &Waiting,
) -> Result<(), String> {
    let mut stale = retried;
    let mut deadline = Deadline::default();
    loop {
        if waiting.ended() {
            return Err(ENDED.into());
        }
        let machine = machines.cloud().into_iter().find(|machine| machine.id == id);
        // Once another step shows, the retry is on its way: a failure from then on counts, the same one too.
        if machine.as_ref().is_some_and(|machine| machine.progress != stale) {
            stale = None;
        }
        let failure = match &machine {
            None => Some("The machine for this session was deleted.".to_string()),
            Some(machine) if machine.info.state == CloudState::Gone => {
                Some("The machine for this session was deleted.".to_string())
            }
            Some(machine) if machine.info.state == CloudState::Failed => Some(
                machine
                    .info
                    .error
                    .clone()
                    .unwrap_or_else(|| "The machine for this session broke.".into()),
            ),
            Some(machine) => machine
                .progress
                .as_ref()
                .filter(|step| Some(*step) != stale.as_ref())
                .and_then(|step| step.error.clone()),
        };
        if let Some(failure) = failure {
            return Err(failure);
        }
        let queued = machine
            .as_ref()
            .is_some_and(|machine| machine.info.state == CloudState::Queued);
        if machine.is_some_and(|machine| matches!(machine.status, Status::Connected { .. }) && machine.read) {
            return Ok(());
        }
        if deadline.passed(queued, Instant::now(), STUCK) {
            return Err(STUCK_ERROR.into());
        }
        tokio::time::sleep(LOOK).await;
    }
}

/// How ending a cloud session went (`endCloudSessions`): its machine is deleted, or it holds commits that are on no
/// remote and waits for the person's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    Gone,
    /// The worktrees holding commits on no remote: nothing was deleted.
    Unpushed(Vec<String>),
}

/// Ends a cloud session (archive or delete) by deleting its machine: unless `force`,
/// a machine holding commits that are on no remote stays. A deleted one leaves this computer's list at once.
pub async fn end_session(machines: &Machines, cloud: &Cloud, machine: &str, force: bool) -> Result<Ended, String> {
    let unpushed = cloud.delete(machine, force).await?;
    if !unpushed.is_empty() {
        return Ok(Ended::Unpushed(unpushed));
    }
    machines.forget(machine);
    Ok(Ended::Gone)
}

#[cfg(test)]
mod tests;
