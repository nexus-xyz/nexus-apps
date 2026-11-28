//! `nexus-cloud`: cloud machines for the desktop app. A cloud machine is a Linux VM a
//! provider runs for one person; the app reaches it over SSH like a person's own server, with an address, a key and a
//! host key it manages itself.
//!
//! - [`Provider`]: what the app needs from a provider, whoever it is: create, read, sleep, wake and delete a machine,
//!   register an SSH key and learn the host key, a preview link for a port, the desktop stream, run a command, and a
//!   key the machine can use to put itself to sleep. One trait, so the provider can change (Sprites, E2B) without
//!   the screens changing.
//! - [`boat`]: [`Provider`] over boat.dev's API (`https://boat.dev/api/v1`).
//! - [`keys`]: the SSH key pair the app makes for each machine, with the system `ssh-keygen`.
//! - [`link`]: [`link::CloudLink`], a `nexus_remote::machine::Link` that wakes the machine, re-registers the key and
//!   pins the new host key (both change on every wake), prepares the machine (Claude's token file, git's name,
//!   sleeping when idle), then installs and reaches `nexus-host` exactly as on a person's server.
//! - [`desktop`]: a desktop app's cloud machines, one per Code session, through Gateway (`/v1/cloud/machines`): their
//!   list, create, wake, sleep and delete, the relay to each one's host, its setup, and the Claude and GitHub
//!   sign-ins it gets. Nexus GPUI runs it over its own [`desktop::CloudEnv`].
//! - `fake` (feature `fake`): boat's API in memory, for the tests and the `fake-boat` binary; `fake_gateway`: Gateway's
//!   `/v1/cloud/machines` over it, as the app reaches cloud machines (Gateway holds Nexus's boat key).

pub mod boat;
pub mod desktop;
#[cfg(feature = "fake")]
pub mod fake;
#[cfg(feature = "fake")]
pub mod fake_gateway;
pub mod keys;
pub mod link;

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// Why a provider call failed, in words a person can act on ([`fmt::Display`]), with what the app may want to branch
/// on kept apart.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The provider answered with an error: its HTTP status, its code (`billing_required`, `machine_not_running`), and
    /// its own message.
    Api { status: u16, code: String, message: String },
    /// The provider's limit on machine starts: wait until `next_start_at` (an RFC 3339 time, when the provider says).
    RateLimited {
        message: String,
        next_start_at: Option<String>,
        retry_after_secs: Option<u64>,
    },
    /// A machine asleep for 14 days has lost its disk: it cannot be woken again, only deleted.
    DataExpired,
    /// Gateway's 402 `budget_exceeded`: the organization spent its budget, so nothing starts or wakes until it is raised.
    OverBudget,
    /// The machine is gone (deleted, or a create that never got a machine).
    NotFound,
    /// The API key was refused (wrong, revoked or expired).
    Unauthorized(String),
    /// The provider could not be reached, or answered something that is not its API.
    Network(String),
    /// Anything else: a timeout waiting for a state, a local file, `ssh-keygen`.
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api { message, code, .. } if message.is_empty() => write!(f, "The cloud provider refused: {code}."),
            Error::Api { message, .. } => write!(f, "{message}"),
            Error::RateLimited {
                next_start_at: Some(at),
                ..
            } => write!(
                f,
                "The cloud provider's limit on machine starts was reached; the next start is possible at {at}."
            ),
            Error::RateLimited { message, .. } => write!(f, "{message}"),
            Error::DataExpired => write!(
                f,
                "The machine's disk expired after 14 days asleep, so it cannot be woken. Delete it and create a new one."
            ),
            Error::OverBudget => write!(
                f,
                "Your org is over its budget, so a cloud machine can't start or wake."
            ),
            Error::NotFound => write!(f, "This cloud machine no longer exists."),
            Error::Unauthorized(message) => write!(f, "{message}"),
            Error::Network(message) | Error::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for String {
    fn from(error: Error) -> String {
        error.to_string()
    }
}

impl Error {
    /// Worth trying again in a few seconds: a machine that is still starting, securing or restoring when its SSH key
    /// is registered or a command is sent, a provider briefly out of machines, a dropped connection.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Api { status, code, .. } => {
                matches!(
                    code.as_str(),
                    "machine_not_running"
                        | "sandbox_restoring"
                        | "sandbox_securing"
                        | "sandbox_starting"
                        | "sandbox_not_ready"
                        | "ssh_host_key_unavailable"
                        | "sandbox_direct_failed"
                        | "idempotency_in_progress"
                ) || *status == 502
                    || (*status == 503 && code != "cloud_unavailable")
            }
            Error::Network(_) => true,
            _ => false,
        }
    }

    /// Whether waiting may fix it, so a connect's tries go on. Only a person can fix the rest, and the tries stop until
    /// "Try again": a refused sign-in, a spent budget, a lost disk, a gone machine, and any other 4xx but 408, 409 and
    /// 429 (a role that may not use cloud machines, an organization without them).
    pub fn heals(&self) -> bool {
        match self {
            Error::Api { status, code, .. } => {
                self.is_retryable()
                    || matches!(status, 408 | 409 | 429)
                    || (*status >= 500 && code != "cloud_unavailable")
            }
            Error::RateLimited { .. } | Error::Network(_) => true,
            Error::DataExpired | Error::OverBudget | Error::NotFound | Error::Unauthorized(_) => false,
            Error::Other(message) => nexus_remote::machine::heals(message),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A machine's size. boat's sizes: small 2 CPUs and 4 GB, default 4 and 8 GB, large 8 and 16 GB, xlarge 16 and 32 GB
/// (xlarge only on its $100 plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small,
    Default,
    /// What the app creates: builds and dev servers of a large repository want the room.
    #[default]
    Large,
    #[serde(rename = "xlarge")]
    XLarge,
}

impl Size {
    pub fn as_str(&self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Default => "default",
            Size::Large => "large",
            Size::XLarge => "xlarge",
        }
    }

    pub fn parse(value: &str) -> Option<Size> {
        match value {
            "small" => Some(Size::Small),
            "default" => Some(Size::Default),
            "large" => Some(Size::Large),
            "xlarge" => Some(Size::XLarge),
            _ => None,
        }
    }
}

/// What to create.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spec {
    /// The machine's display name at the provider (at most 120 characters on boat).
    pub name: String,
    pub size: Size,
    /// Labels to find it by later (`nexus.user`, `nexus.machine`): at most 20 on boat.
    pub labels: BTreeMap<String, String>,
}

/// Where a machine is, in the words the sidebar uses. Every provider state maps to one of these
/// ([`boat::state`] for boat's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum State {
    /// Being made: no machine yet.
    Creating,
    /// A machine was found and is booting (after a create or a wake).
    Starting,
    /// Running and reachable.
    Ready,
    /// Asleep: the disk is saved, no machine runs, nothing is billed. Waking starts a new machine from the disk.
    Sleeping,
    /// Saving the disk before sleeping.
    Stopping,
    /// Broken; the provider's reason.
    Failed { message: String },
    /// Deleted, or never got a machine.
    Gone,
}

/// A machine as the provider reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub state: State,
    /// The provider's own word for the state (`archived`), for logs and support.
    pub provider_state: String,
    pub size: Option<Size>,
    pub labels: BTreeMap<String, String>,
    /// When it last went to sleep (RFC 3339), while asleep.
    pub stopped_at: Option<String>,
    /// Set once a machine asleep for 14 days has lost its disk: waking it fails with [`Error::DataExpired`].
    pub data_expired_at: Option<String>,
}

/// How to reach a running machine over SSH, as the provider says after registering a public key. The address and the
/// host key are new after every wake.
#[derive(Debug, Clone, PartialEq)]
pub struct SshAccess {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The machine's SSH host key, `ssh-ed25519 AAAA…`, to pin before connecting.
    pub host_key: String,
}

/// What a command printed and how it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOutput {
    /// None when it ended on a signal or ran out of time.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// What the app needs from a cloud provider. Object-safe (each call returns a boxed future, like
/// `nexus_remote::machine::Link`), so the app holds an `Arc<dyn Provider>` and the tests swap in the fake.
pub trait Provider: Send + Sync + 'static {
    /// Starts a new machine and returns its id at once; it becomes [`State::Ready`] later ([`wait_ready`]).
    fn create(&self, spec: Spec) -> BoxFuture<'_, Result<String>>;
    /// The machine now. A machine the provider no longer knows is [`State::Gone`], not an error.
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Machine>>;
    /// Puts it to sleep: the disk is saved, the machine freed, its programs end.
    fn sleep<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Wakes a sleeping machine onto a new one, of `size` when given (else the size it had).
    fn wake<'a>(&'a self, id: &'a str, size: Option<Size>) -> BoxFuture<'a, Result<()>>;
    /// Deletes it and its disk, for good. A machine already gone is not an error.
    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Lets `public_key` (`ssh-ed25519 AAAA… comment`) in, and says where to connect and which host key to expect.
    /// Needed after every wake: the machine, its address and its host key are new.
    fn authorize_ssh<'a>(&'a self, id: &'a str, public_key: &'a str) -> BoxFuture<'a, Result<SshAccess>>;
    /// A web link to `port` on the machine (a dev server). Not `public`: the link carries a token, and anyone with the
    /// whole link can open it.
    fn preview<'a>(&'a self, id: &'a str, port: u16, public: bool) -> BoxFuture<'a, Result<String>>;
    /// A link to watch the machine's desktop in a browser (it carries a token; it must be opened within minutes).
    fn desktop<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<String>>;
    /// Runs `command` with `sh` in the machine's home, without SSH, for at most `timeout` (boat: 1 to 600 seconds).
    fn exec<'a>(&'a self, id: &'a str, command: &'a str, timeout: Duration) -> BoxFuture<'a, Result<ExecOutput>>;
    /// A key that can only read and stop this one machine, for the machine to put itself to sleep when idle while the
    /// laptop is closed. None when the provider (or this account's key) cannot make one: the app then sleeps it.
    fn mint_stop_key<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<String>>>;
}

/// How often [`wait_ready`] asks.
pub const POLL: Duration = Duration::from_millis(500);

/// Asks for the machine every [`POLL`] until it is [`State::Ready`], for at most `deadline`. `on_state` hears each
/// state as it changes (the first one too), for a status line. A machine that fails, is gone, or falls asleep while
/// waiting ends the wait with an error.
pub async fn wait_ready(
    provider: &dyn Provider,
    id: &str,
    deadline: Duration,
    mut on_state: impl FnMut(&State) + Send,
) -> Result<Machine> {
    let started = Instant::now();
    let mut last: Option<State> = None;
    loop {
        let machine = match provider.get(id).await {
            Ok(machine) => machine,
            Err(error) if error.is_retryable() && started.elapsed() < deadline => {
                tokio::time::sleep(POLL).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        if last.as_ref() != Some(&machine.state) {
            on_state(&machine.state);
            last = Some(machine.state.clone());
        }
        match &machine.state {
            State::Ready => return Ok(machine),
            State::Failed { message } => return Err(Error::Other(format!("The cloud machine failed: {message}"))),
            State::Gone => return Err(Error::NotFound),
            State::Sleeping if machine.data_expired_at.is_some() => return Err(Error::DataExpired),
            State::Sleeping => return Err(Error::Other("The cloud machine went to sleep while starting.".into())),
            State::Creating | State::Starting | State::Stopping => {}
        }
        if started.elapsed() >= deadline {
            return Err(Error::Other(format!(
                "The cloud machine was not ready within {} seconds.",
                deadline.as_secs()
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Asks for the machine every [`POLL`] until it is no longer [`State::Stopping`] (a sleep in progress must finish
/// before the machine can be woken), for at most `deadline`.
pub async fn wait_stopped(provider: &dyn Provider, id: &str, deadline: Duration) -> Result<Machine> {
    let started = Instant::now();
    loop {
        let machine = provider.get(id).await?;
        if machine.state != State::Stopping {
            return Ok(machine);
        }
        if started.elapsed() >= deadline {
            return Err(Error::Other(format!(
                "The cloud machine was still going to sleep after {} seconds.",
                deadline.as_secs()
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_which_errors_heal() {
        let api = |status: u16, code: &str| Error::Api {
            status,
            code: code.into(),
            message: String::new(),
        };
        for healing in [
            api(408, "timeout"),
            api(409, "host_offline"),
            api(429, "too_many_streams"),
            api(502, "bad_gateway"),
            api(503, "provider_down"),
            api(400, "machine_not_running"),
            Error::Network("reset".into()),
            Error::Other("The cloud machine did not wake in time.".into()),
        ] {
            assert!(healing.heals(), "{healing:?}");
        }
        for stopping in [
            api(403, "forbidden"),
            api(403, "cloud_not_enabled"),
            api(400, "bad_request"),
            api(503, "cloud_unavailable"),
            Error::Unauthorized("again".into()),
            Error::OverBudget,
            Error::DataExpired,
            Error::NotFound,
            Error::Other("The Nexus host 0.9.0 is not available: HTTP 404".into()),
        ] {
            assert!(!stopping.heals(), "{stopping:?}");
        }
    }

    #[test]
    fn states_serialize_for_the_sidebar() {
        assert_eq!(
            serde_json::to_value(State::Failed { message: "x".into() }).unwrap_or_default(),
            serde_json::json!({ "state": "failed", "message": "x" })
        );
        assert_eq!(
            serde_json::to_value(State::Sleeping).unwrap_or_default(),
            serde_json::json!({ "state": "sleeping" })
        );
    }

    #[test]
    fn sizes_round_trip_and_default_to_large() {
        for size in [Size::Small, Size::Default, Size::Large, Size::XLarge] {
            assert_eq!(Size::parse(size.as_str()), Some(size));
            assert_eq!(
                serde_json::to_value(size).unwrap_or_default(),
                serde_json::Value::String(size.as_str().into())
            );
        }
        assert_eq!(Spec::default().size, Size::Large);
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            Error::DataExpired.to_string(),
            "The machine's disk expired after 14 days asleep, so it cannot be woken. Delete it and create a new one."
        );
        assert!(
            Error::RateLimited {
                message: "x".into(),
                next_start_at: Some("2026-11-01T12:00:00Z".into()),
                retry_after_secs: None
            }
            .to_string()
            .contains("2026-11-01T12:00:00Z")
        );
        assert!(
            Error::Api {
                status: 400,
                code: "sandbox_restoring".into(),
                message: String::new()
            }
            .is_retryable()
        );
        assert!(
            !Error::Api {
                status: 402,
                code: "billing_required".into(),
                message: String::new()
            }
            .is_retryable()
        );
    }
}
