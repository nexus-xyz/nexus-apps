//! The usage signals the desktop app sends to Nexus Gateway (the collection contract): the activity
//! heartbeat (`POST /v1/diagnostics/heartbeat`) and client errors (`POST /v1/diagnostics/errors`). The plan tier on a
//! session's open is `nexus_workbench::plan_tier`.
//!
//! Each value comes from a fixed list or is a name from this code: never a message, a stack, a path, a URL, an email
//! or an account id. Every call is fire and forget: one attempt, 5 s at most, its answer ignored, never on the UI
//! thread. The Gateway stores nothing while its privacy switches are off, and the apps show nothing new.
//!
//! The shell feeds [`input`], [`focus`] and [`view`] from its windows and runs [`run`] on its runtime. Errors queue in
//! a bounded file in the app's data folder, so a panic is sent at the next start and a sign-in failure once signed in.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nexus_client::{Client, ClientOptions};
use serde_json::{Map, Value, json};

/// Each call's limit.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// A heartbeat counts one active minute: one a minute at most.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
/// The person acted this recently, or no heartbeat goes.
pub const ACTIVE_WITHIN: Duration = Duration::from_secs(60);
/// The most errors a request carries (the contract's limit).
pub const BATCH: usize = 20;
/// One batch a minute at most: under the Gateway's 30 entries a minute.
pub const ERRORS_EVERY: Duration = Duration::from_secs(60);
/// The most errors kept on disk; the oldest go first.
pub const MAX_QUEUED: usize = 100;
/// How often the loop looks.
const TICK: Duration = Duration::from_secs(10);
const QUEUE_FILE: &str = "client-errors.jsonl";
const COUNTED_CRASH_FILE: &str = "last-crash.counted";
/// The most of the queue file read: [`MAX_QUEUED`] entries are far under it.
const MAX_QUEUE_BYTES: u64 = 256 * 1024;

pub const HEARTBEAT_PATH: &str = "/v1/diagnostics/heartbeat";
pub const ERRORS_PATH: &str = "/v1/diagnostics/errors";

/// Which app sends: the contract's `app`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    /// An older Nexus desktop install.
    Desktop,
    /// Nexus GPUI.
    DesktopGpui,
}

impl App {
    pub fn as_str(self) -> &'static str {
        match self {
            App::Desktop => "desktop",
            App::DesktopGpui => "desktop-gpui",
        }
    }
}

/// What the person looks at: the contract's `surface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Code,
    Chat,
    Review,
    Pages,
    Other,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Code => "code",
            Surface::Chat => "chat",
            Surface::Review => "review",
            Surface::Pages => "pages",
            Surface::Other => "other",
        }
    }

    /// A surface by its contract name; anything else is `other`.
    pub fn parse(name: &str) -> Surface {
        match name {
            "code" => Surface::Code,
            "chat" => Surface::Chat,
            "review" => Surface::Review,
            "pages" => Surface::Pages,
            _ => Surface::Other,
        }
    }
}

/// The contract's error `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Crash,
    Render,
    Network,
    SignIn,
    Update,
    Agent,
    Storage,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Crash => "crash",
            Kind::Render => "render",
            Kind::Network => "network",
            Kind::SignIn => "sign_in",
            Kind::Update => "update",
            Kind::Agent => "agent",
            Kind::Storage => "storage",
            Kind::Other => "other",
        }
    }
}

/// The operating system: the contract's `os`.
pub fn os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// The OS release (`15.1`, `10.0.22631`, `24.04`), or None when it does not fit the contract's characters. Linux sends
/// only `VERSION_ID`, never the distribution's name. Blocking the first time: it may run a system command.
pub fn os_version() -> Option<String> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let raw = if cfg!(target_os = "linux") {
                std::fs::read_to_string("/etc/os-release").ok().and_then(|release| {
                    release
                        .lines()
                        .find_map(|line| line.strip_prefix("VERSION_ID="))
                        .map(|version| version.trim_matches('"').to_string())
                })
            } else {
                crate::crash::read_os_version()
            };
            raw.filter(|version| fits(version, 32, |c| c.is_ascii_alphanumeric() || "._-".contains(c)))
        })
        .clone()
}

/// The app's version as the contract takes it (`0.31.0+abc1234`), or `unknown`.
pub fn app_version(version: &str) -> String {
    if fits(version, 32, |c| c.is_ascii_alphanumeric() || ".+-".contains(c)) {
        version.to_string()
    } else {
        "unknown".into()
    }
}

/// A `component` or `code`: 1 to 64 of `[a-z0-9_.-]`. Anything else is `other`, so a slip never sends text.
pub fn name(raw: &str) -> String {
    if fits(raw, 64, |c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c)
    }) {
        raw.to_string()
    } else {
        "other".into()
    }
}

fn fits(text: &str, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    !text.is_empty() && text.len() <= max && text.chars().all(allowed)
}

/// RFC 3339 in UTC with milliseconds, from epoch milliseconds.
pub fn rfc3339(ms: u64) -> String {
    let (seconds, millis) = (ms / 1000, ms % 1000);
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// The heartbeat's body.
pub fn heartbeat_body(app: App, surface: Surface) -> Value {
    let mut body = json!({
        "surface": surface.as_str(),
        "app": app.as_str(),
        "appVersion": app_version(crate::app::version()),
        "os": os(),
    });
    if let (Some(version), Some(fields)) = (os_version(), body.as_object_mut()) {
        fields.insert("osVersion".into(), Value::from(version));
    }
    body
}

/// One queued error, as the contract's entry (without `osVersion`, added when it is sent).
pub fn error_entry(app: App, version: &str, kind: Kind, component: &str, code: &str, fatal: bool, at_ms: u64) -> Value {
    json!({
        "app": app.as_str(),
        "appVersion": app_version(version),
        "os": os(),
        "kind": kind.as_str(),
        "component": name(component),
        "code": name(code),
        "fatal": fatal,
        "occurredAt": rfc3339(at_ms),
    })
}

// ---- Activity ------------------------------------------------------------------------------------------------------

/// What the windows say: which have the focus, what each shows, and when the person last acted.
#[derive(Debug, Default)]
pub struct Activity {
    focused: HashSet<String>,
    surfaces: HashMap<String, Surface>,
    last_window: Option<String>,
    last_input: Option<Instant>,
    last_sent: Option<Instant>,
}

impl Activity {
    pub fn view(&mut self, window: &str, surface: Surface) {
        self.surfaces.insert(window.to_string(), surface);
    }

    pub fn input(&mut self, window: &str, now: Instant) {
        self.last_input = Some(now);
        self.last_window = Some(window.to_string());
    }

    pub fn focus(&mut self, window: &str, focused: bool) {
        if focused {
            self.focused.insert(window.to_string());
        } else {
            self.focused.remove(window);
        }
    }

    pub fn closed(&mut self, window: &str) {
        self.focused.remove(window);
        self.surfaces.remove(window);
    }

    /// The surface to send a heartbeat for now, if one is due: a window has the focus, the person acted within
    /// [`ACTIVE_WITHIN`], and the last one went [`HEARTBEAT_EVERY`] ago or more. The surface is the focused window's
    /// where the person last acted.
    pub fn due(&self, now: Instant) -> Option<Surface> {
        if self.focused.is_empty() {
            return None;
        }
        let acted = self.last_input?;
        if now.saturating_duration_since(acted) > ACTIVE_WITHIN {
            return None;
        }
        if self
            .last_sent
            .is_some_and(|sent| now.saturating_duration_since(sent) < HEARTBEAT_EVERY)
        {
            return None;
        }
        let window = self
            .last_window
            .as_ref()
            .filter(|window| self.focused.contains(*window))
            .or_else(|| self.focused.iter().next())?;
        Some(self.surfaces.get(window).copied().unwrap_or(Surface::Other))
    }

    pub fn sent(&mut self, now: Instant) {
        self.last_sent = Some(now);
    }
}

fn activity() -> MutexGuard<'static, Activity> {
    static ACTIVITY: OnceLock<Mutex<Activity>> = OnceLock::new();
    ACTIVITY
        .get_or_init(|| Mutex::new(Activity::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The person pressed a key, clicked or scrolled in `window`.
pub fn input(window: &str) {
    activity().input(window, Instant::now());
}

/// `window` took or lost the focus.
pub fn focus(window: &str, focused: bool) {
    activity().focus(window, focused);
}

/// `window` shows `surface` now.
pub fn view(window: &str, surface: Surface) {
    activity().view(window, surface);
}

/// `window` closed.
pub fn closed(window: &str) {
    activity().closed(window);
}

/// The window the person last acted in: a shell whose windows sign in to different organizations sends with its
/// credential, so the heartbeat counts in the organization the person looked at.
pub fn last_window() -> Option<String> {
    activity().last_window.clone()
}

// ---- The error queue -----------------------------------------------------------------------------------------------

/// The errors waiting to be sent, one JSON entry a line, at most [`MAX_QUEUED`].
#[derive(Debug, Clone)]
pub struct Queue {
    path: PathBuf,
}

impl Queue {
    pub fn new(dir: &Path) -> Queue {
        Queue {
            path: dir.join(QUEUE_FILE),
        }
    }

    fn read(&self) -> VecDeque<Value> {
        let size = std::fs::metadata(&self.path).map_or(0, |meta| meta.len());
        if size == 0 || size > MAX_QUEUE_BYTES {
            return VecDeque::new();
        }
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(Value::is_object)
            .collect()
    }

    fn write(&self, entries: &VecDeque<Value>) {
        if entries.is_empty() {
            let _ = std::fs::remove_file(&self.path);
            return;
        }
        let text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let partial = self.path.with_extension("jsonl.partial");
        if std::fs::write(&partial, text).is_ok() {
            let _ = std::fs::rename(&partial, &self.path);
        }
    }

    /// Adds an entry; past [`MAX_QUEUED`], the oldest goes.
    pub fn push(&self, entry: Value) {
        let _lock = queue_lock();
        let mut entries = self.read();
        entries.push_back(entry);
        while entries.len() > MAX_QUEUED {
            entries.pop_front();
        }
        self.write(&entries);
    }

    /// Takes up to [`BATCH`] entries off the front. They are gone whether or not they are sent.
    pub fn take(&self) -> Vec<Value> {
        let _lock = queue_lock();
        let mut entries = self.read();
        let batch: Vec<Value> = entries.drain(..entries.len().min(BATCH)).collect();
        self.write(&entries);
        batch
    }

    pub fn len(&self) -> usize {
        let _lock = queue_lock();
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn queue_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

static INSTALLED: OnceLock<(App, Queue)> = OnceLock::new();

/// At start, after `crash::install` and after the shell set [`crate::usage`]: where the queue is, and which app this
/// is. A panic the last run left is queued once, and the workbench's and the updater's failures are reported from now
/// on.
pub fn install(dir: &Path, app: App) {
    if app == App::Desktop {
        crate::usage::set(true);
    }
    if INSTALLED.set((app, Queue::new(dir))).is_err() {
        return;
    }
    if crate::usage::allowed() {
        queue_last_crash(dir, app);
    }
    nexus_workbench::faults::report_to(agent_failed);
}

/// The last run's panic, queued once: the crash prompt may leave the file for a later start.
fn queue_last_crash(dir: &Path, app: App) {
    let Some(crash) = crate::crash::peek(dir) else {
        return;
    };
    let counted = dir.join(COUNTED_CRASH_FILE);
    let stamp = crash.occurred_at_ms.to_string();
    if std::fs::read_to_string(&counted).is_ok_and(|seen| seen.trim() == stamp) {
        return;
    }
    let _ = std::fs::write(&counted, &stamp);
    // An older install aborts on any panic; GPUI ends only when its main thread panics.
    let fatal = app == App::Desktop || crash.name == "panic" || crash.name == "panic in main";
    let entry = error_entry(
        app,
        &crash.app_version,
        Kind::Crash,
        &panic_component(crash.location.as_deref()),
        "panic",
        fatal,
        crash.occurred_at_ms,
    );
    Queue::new(dir).push(entry);
}

/// The crate a panic happened in, by its place in the code: `nexus-workbench` for `crates/nexus-workbench/src/…`,
/// `app` for the app's own `src/…`, `dependency` otherwise. Only the name leaves, never the path.
pub fn panic_component(location: Option<&str>) -> String {
    let Some(location) = location else {
        return "unknown".into();
    };
    let path = location.replace('\\', "/");
    let segments: Vec<&str> = path.split('/').collect();
    if let Some(at) = segments.iter().position(|segment| *segment == "crates")
        && let Some(krate) = segments.get(at + 1)
        && krate.starts_with("nexus-")
    {
        return name(krate);
    }
    if path.starts_with("src/") {
        return "app".into();
    }
    "dependency".into()
}

/// The field that keeps whose error an entry is: the sign-in's fingerprint ([`crate::secrets::marker`]). It stays on
/// this computer: [`send_errors`] takes it off.
const OWNER_FIELD: &str = "_owner";

/// The sign-in errors are recorded under now, as [`run`] last saw it (None: signed out).
static OWNER: Mutex<Option<String>> = Mutex::new(None);

/// The fingerprint of `client`'s sign-in, never the token.
fn owner_of(client: &Client) -> Option<String> {
    client.options().api_key.as_deref().map(crate::secrets::marker)
}

fn current_owner() -> Option<String> {
    OWNER.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

fn set_owner(owner: Option<String>) {
    *OWNER.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = owner;
}

/// Queues an error (`component` and `code` are names from this code) under the person signed in now, so it is never
/// sent for another person or organization. One recorded while signed out (a failed sign-in, a panic queued at
/// start) has no owner and goes with the next sign-in. Nothing happens before [`install`], or while usage data is off.
pub fn record(kind: Kind, component: &str, code: &str, fatal: bool) {
    if !crate::usage::allowed() {
        return;
    }
    if let Some((app, queue)) = INSTALLED.get() {
        let mut entry = error_entry(*app, crate::app::version(), kind, component, code, fatal, now_ms());
        if let (Some(owner), Some(fields)) = (current_owner(), entry.as_object_mut()) {
            fields.insert(OWNER_FIELD.into(), Value::from(owner));
        }
        queue.push(entry);
    }
}

/// An agent did not start (`nexus_workbench::faults`).
pub fn agent_failed(component: &'static str, code: &'static str) {
    record(Kind::Agent, component, code, false);
}

/// The updater failed (`nexus_desktop_updates::report_failures_to`).
pub fn update_failed(code: &'static str) {
    record(Kind::Update, "updater", code, false);
}

/// A sign-in failed. A cancelled one is not an error.
pub fn sign_in_failed(error: &nexus_client::device::LoginError) {
    use nexus_client::device::LoginError;
    let code = match error {
        LoginError::Cancelled => return,
        LoginError::Denied => "denied".to_string(),
        LoginError::Expired => "expired".to_string(),
        LoginError::Failed(error) => error_code(error),
    };
    record(Kind::SignIn, "device_sign_in", &code, false);
}

/// A sign-in with an API key failed.
pub fn key_sign_in_failed(error: &nexus_client::Error) {
    record(Kind::SignIn, "key_sign_in", &error_code(error), false);
}

/// A failed Gateway call's code: the Gateway's machine-readable code, else its status, else the transport's kind.
pub fn error_code(error: &nexus_client::Error) -> String {
    use nexus_client::error::Transport;
    match error {
        nexus_client::Error::Core(core) => core
            .code
            .as_deref()
            .map(name)
            .filter(|code| code != "other")
            .unwrap_or_else(|| format!("status_{}", core.status)),
        nexus_client::Error::Transport(Transport::Unreachable(_)) => "unreachable".into(),
        nexus_client::Error::Transport(Transport::Timeout) => "timeout".into(),
        nexus_client::Error::Transport(Transport::Other(_)) => "transport".into(),
    }
}

// ---- Sending -------------------------------------------------------------------------------------------------------

/// The person's client as these calls use it: one attempt, with a fresh `Idempotency-Key` each (never reused across
/// heartbeats), and [`TIMEOUT`].
pub fn one_shot(client: &Client) -> Client {
    Client::with_http(
        ClientOptions {
            idempotency: true,
            retry: false,
            timeout: TIMEOUT,
            ..client.options().clone()
        },
        client.http().clone(),
    )
}

/// Sends one heartbeat. The answer is ignored.
pub async fn send_heartbeat(client: &Client, app: App, surface: Surface) {
    let client = one_shot(client);
    let call = client.post(HEARTBEAT_PATH, heartbeat_body(app, surface));
    let _ = tokio::time::timeout(TIMEOUT, call).await;
}

/// Sends the next batch of queued errors. The batch is gone whatever the answer: a failed one is dropped, and so is
/// an entry another sign-in recorded (another person, or another organization's token). Returns the entries taken.
pub async fn send_errors(client: &Client, queue: &Queue) -> usize {
    let batch = queue.take();
    let taken = batch.len();
    let sender = owner_of(client);
    let version = os_version();
    let errors: Vec<Value> = batch
        .into_iter()
        .filter_map(|mut entry| {
            let fields = entry.as_object_mut()?;
            let owner = fields.remove(OWNER_FIELD);
            if owner.is_some_and(|owner| owner.as_str() != sender.as_deref()) {
                return None;
            }
            if let Some(version) = &version {
                fields.insert("osVersion".into(), Value::from(version.clone()));
            }
            Some(entry)
        })
        .collect();
    if errors.is_empty() {
        return taken;
    }
    let mut body = Map::new();
    body.insert("errors".into(), Value::Array(errors));
    let client = one_shot(client);
    let call = client.post(ERRORS_PATH, Value::Object(body));
    let _ = tokio::time::timeout(TIMEOUT, call).await;
    taken
}

/// The loop each app runs once on its own runtime: a heartbeat when one is due, a batch of errors at most once a
/// minute. `client` is the signed-in client, or None while signed out (nothing is sent; errors wait).
pub async fn run(app: App, client: impl Fn() -> Option<Client> + Send + 'static) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut errors_sent: Option<Instant> = None;
    loop {
        tokio::time::sleep(TICK).await;
        let client = client().filter(Client::signed_in);
        // Whose errors are recorded from now on (`record`).
        set_owner(client.as_ref().and_then(owner_of));
        let Some(client) = client.filter(|_| crate::usage::allowed()) else {
            continue;
        };
        let now = Instant::now();
        let due = activity().due(now);
        if let Some(surface) = due {
            activity().sent(now);
            send_heartbeat(&client, app, surface).await;
        }
        if errors_sent.is_none_or(|sent| now.saturating_duration_since(sent) >= ERRORS_EVERY)
            && let Some((_, queue)) = INSTALLED.get()
            && send_errors(&client, queue).await > 0
        {
            errors_sent = Some(now);
        }
    }
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
