//! `daily_active`: the one usage event of the open-source desktop app (PRIVACY.md, "What the app sends:
//! install data"). While usage data is on ([`crate::usage`]), the app sends each day it was used once, to
//! `POST /v1/public/daily-active` on production Nexus Gateway, with no account token.
//!
//! - **The install ID** is a random UUID v4 in the app's data folder ([`ID_FILE`]), made when usage data is turned on.
//!   Turning it off deletes it ([`forget`]); turning it on again makes a new one.
//! - **The day** ([`STATE_FILE`]) counts the minutes the app ran and the modes seen. It is sent once it has
//!   [`SEND_AFTER_MINUTES`], or at the next start when the app closed before that, within [`LATE_DAYS`]. That late
//!   day gets one attempt: the file holds one day, so today's record replaces it whether or not the attempt went.
//! - **Only production.** With the app pointed at another Gateway, nothing is recorded or sent.
//! - **No token.** The request goes on a client of its own, made with no key, so it never carries the account's.
//! - **Off by default** ([`FLAG`]): until `NEXUS_DESKTOP_DAILY_ACTIVE=on`, an official build neither asks the
//!   first-start question nor runs the loop, and makes no install ID.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nexus_client::{Client, ClientOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::settings::DEFAULT_GATEWAY;

pub const PATH: &str = "/v1/public/daily-active";
/// Turns on the first-start question and the daily send. Off unless set to `on`, `1` or `true`.
pub const FLAG: &str = "NEXUS_DESKTOP_DAILY_ACTIVE";

/// Whether [`FLAG`] is on in this process, read once: the question, the loop and the install ID agree for the whole
/// run, whatever the environment does later.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| flag_on(std::env::var(FLAG).ok().as_deref()))
}

fn flag_on(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true"))
}
pub const ID_FILE: &str = "install-id";
pub const STATE_FILE: &str = "usage-day.json";
/// A day is sent once the app ran this long that day: long enough to see how it reaches models.
pub const SEND_AFTER_MINUTES: u32 = 5;
/// The oldest day the Gateway takes, in days before today.
pub const LATE_DAYS: i64 = 7;
/// How often the loop looks, and the unit of [`Record::minutes`].
const TICK: Duration = Duration::from_secs(60);
/// After a network failure, the next try waits this long.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
const TIMEOUT: Duration = Duration::from_secs(5);
const DAY_MS: i64 = 86_400_000;

/// How the app reached models: the contract's `modes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    OwnSubscription,
    OwnKey,
    OwnModel,
    Cloud,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::OwnSubscription => "own_subscription",
            Mode::OwnKey => "own_key",
            Mode::OwnModel => "own_model",
            Mode::Cloud => "cloud",
        }
    }
}

/// One day of use, as kept between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Days since 1970-01-01, UTC.
    pub day: i64,
    pub minutes: u32,
    pub modes: BTreeSet<String>,
    pub sent: bool,
}

/// Whether `record` goes now, on `today`: an unsent day once it ran [`SEND_AFTER_MINUTES`], or any unsent earlier day
/// the Gateway still takes.
pub fn due(record: &Record, today: i64) -> bool {
    !record.sent
        && record.minutes > 0
        && (record.minutes >= SEND_AFTER_MINUTES || (record.day < today && today - record.day <= LATE_DAYS))
}

// ---- The install ID ------------------------------------------------------------------------------------------------

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.hyphenated().to_string() == id)
}

/// This install's ID, if usage data made one.
pub fn install_id(dir: &Path) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(ID_FILE)).ok()?;
    let id = id.trim();
    valid_id(id).then(|| id.to_string())
}

/// This install's ID, made now when there is none.
pub fn ensure_install_id(dir: &Path) -> Option<String> {
    if let Some(id) = install_id(dir) {
        return Some(id);
    }
    let id = uuid::Uuid::new_v4().hyphenated().to_string();
    std::fs::create_dir_all(dir).ok()?;
    crate::atomic_file::write_atomic_private(&dir.join(ID_FILE), id.as_bytes()).ok()?;
    Some(id)
}

/// Deletes the install ID and the day not sent yet. Returns the ID, to show the person one last time.
pub fn forget(dir: &Path) -> Option<String> {
    let mut generation = files();
    *generation += 1;
    let id = install_id(dir);
    let _ = std::fs::remove_file(dir.join(ID_FILE));
    let _ = std::fs::remove_file(dir.join(STATE_FILE));
    pending().clear();
    id
}

/// The lock over the ID and day files, with a count [`forget`] raises: the loop's file steps run under it and drop
/// their work when the count moved. Never held across a request. A request already on the wire when usage data is
/// turned off still arrives (at most [`TIMEOUT`]).
fn files() -> std::sync::MutexGuard<'static, u64> {
    static FILES: Mutex<u64> = Mutex::new(0);
    FILES.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---- The day -------------------------------------------------------------------------------------------------------

fn pending() -> std::sync::MutexGuard<'static, BTreeSet<Mode>> {
    static PENDING: Mutex<BTreeSet<Mode>> = Mutex::new(BTreeSet::new());
    PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Notes a mode seen now, for today's record. Nothing is noted while usage data is off.
pub fn note(mode: Mode) {
    if crate::usage::allowed() {
        pending().insert(mode);
    }
}

fn load(dir: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(dir.join(STATE_FILE)).ok()?).ok()
}

fn save(dir: &Path, record: &Record) {
    if let Ok(bytes) = serde_json::to_vec(record) {
        let _ = crate::atomic_file::write_atomic_private(&dir.join(STATE_FILE), &bytes);
    }
}

/// `x86_64` or `arm64`, else None: the contract's `arch`.
pub fn arch(raw: &str) -> Option<&'static str> {
    match raw {
        "x86_64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("arm64"),
        _ => None,
    }
}

/// The OS version's major and minor parts (`10.0.22631` is `10.0`), else None: the contract's `os_version`.
pub fn major_minor(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split('.').take(2).collect();
    let number = |part: &&str| (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit());
    parts.iter().all(number).then(|| parts.join("."))
}

/// The release channel of `version`: no prerelease is `stable`, `internal.N` and `beta.N` name theirs.
pub fn channel(version: &str) -> &'static str {
    let version = version.split('+').next().unwrap_or(version);
    match version
        .split_once('-')
        .map(|(_, pre)| pre.split('.').next().unwrap_or(pre))
    {
        None => "stable",
        Some("internal") => "internal",
        Some("beta") => "beta",
        Some(_) => "other",
    }
}

fn day_string(day: i64) -> String {
    crate::signals::rfc3339(u64::try_from(day.saturating_mul(DAY_MS)).unwrap_or(0))[..10].to_string()
}

/// The event for `record`, or None when this computer is outside the contract's lists.
pub fn body(
    id: &str,
    record: &Record,
    version: &str,
    os: &str,
    os_version: Option<&str>,
    arch: Option<&str>,
) -> Option<Value> {
    if !matches!(os, "macos" | "linux" | "windows") {
        return None;
    }
    let mut body = json!({
        "install_id": id,
        "day": day_string(record.day),
        "app_version": crate::signals::app_version(version),
        "channel": channel(version),
        "os": os,
        "arch": arch?,
        "modes": record.modes,
    });
    if let (Some(version), Some(fields)) = (os_version.and_then(major_minor), body.as_object_mut()) {
        fields.insert("os_version".into(), Value::from(version));
    }
    Some(body)
}

/// Whether the client's Gateway is production Nexus Gateway.
pub fn on_production(client: &Client) -> bool {
    client.options().base_url.trim_end_matches('/') == DEFAULT_GATEWAY
}

/// Sends `body` on a client with no API key. True when the Gateway answered, whatever it said: a refusal is not
/// retried. It keeps its `Idempotency-Key`, as every client that must not retry does (`retry: false`); the Gateway
/// ignores the key on a public route.
async fn send(body: Value) -> bool {
    let client = Client::new(ClientOptions {
        retry: false,
        timeout: TIMEOUT,
        ..ClientOptions::new(DEFAULT_GATEWAY)
    });
    matches!(
        tokio::time::timeout(TIMEOUT, client.post(PATH, body)).await,
        Ok(Ok(_) | Err(nexus_client::Error::Core(_)))
    )
}

fn today() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64);
    ms.div_euclid(DAY_MS)
}

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// The loop the shell runs once on its runtime. `client` is the app's current client, signed in or not.
pub async fn run(dir: PathBuf, client: impl Fn() -> Option<Client> + Send + 'static) {
    if DIR.set(dir.clone()).is_err() {
        return;
    }
    let mut retry_at: Option<std::time::Instant> = None;
    loop {
        tokio::time::sleep(TICK).await;
        let Some(client) = client().filter(on_production) else {
            continue;
        };
        let signed_in = client.signed_in();
        // Under the lock: the check, the ID, and the day, so nothing comes back after `forget`.
        let (generation, sends) = {
            let generation = files();
            if !crate::usage::allowed() {
                continue;
            }
            let Some(id) = ensure_install_id(&dir) else {
                continue;
            };
            let today = today();
            let mut record = load(&dir).unwrap_or(Record {
                day: today,
                ..Record::default()
            });
            let mut sends = Vec::new();
            if record.day != today {
                if due(&record, today) {
                    sends.push(record.clone());
                }
                record = Record {
                    day: today,
                    ..Record::default()
                };
            }
            record.minutes = record.minutes.saturating_add(1);
            if signed_in {
                record.modes.insert(Mode::Cloud.as_str().into());
            }
            record.modes.extend(
                std::mem::take(&mut *pending())
                    .into_iter()
                    .map(|mode| mode.as_str().to_string()),
            );
            if due(&record, today) {
                sends.push(record.clone());
            }
            save(&dir, &record);
            (
                *generation,
                sends.into_iter().map(|day| (id.clone(), day)).collect::<Vec<_>>(),
            )
        };
        for (id, day) in sends {
            if retry_at.is_some_and(|at| std::time::Instant::now() < at) {
                break;
            }
            let answered = match event(&id, &day) {
                Some(body) => send(body).await,
                None => true,
            };
            if !answered {
                retry_at = Some(std::time::Instant::now() + RETRY_AFTER);
                break;
            }
            retry_at = None;
            let generation_now = files();
            if *generation_now != generation || !crate::usage::allowed() {
                break;
            }
            // Mark the day sent: today's record, or nothing for a past day (today's record replaced it).
            if let Some(mut kept) = load(&dir).filter(|kept| kept.day == day.day) {
                kept.sent = true;
                save(&dir, &kept);
            }
        }
    }
}

fn event(id: &str, record: &Record) -> Option<Value> {
    let os_version = crate::signals::os_version();
    body(
        id,
        record,
        crate::app::version(),
        crate::signals::os(),
        os_version.as_deref(),
        arch(std::env::consts::ARCH),
    )
}

#[cfg(test)]
#[path = "daily_active_tests.rs"]
mod tests;
