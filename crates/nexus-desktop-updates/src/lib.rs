//! One updater per process. The host supplies time and a feed; neither windows nor tests own a timer.
//! Nexus for desktop uses it, on one feed per channel: `desktop/<channel>/latest.json` on downloads.nexus.inc, and
//! `desktop-oss/beta/latest.json` for the beta that the open-source repository releases. The GPUI
//! build reads its package under `platforms["<os>-<arch>-<format>"]` ([`manifest::package_target`]). Older Nexus
//! installs read `<os>-<arch>-<installer>`, then `<os>-<arch>`. The feed carries neither, so they never install a GPUI
//! package.
#[cfg(feature = "net")]
pub mod access;
pub mod cache;
#[cfg(feature = "net")]
pub mod held_access;
#[cfg(feature = "net")]
pub mod http;
pub mod manifest;
pub mod verify;

use semver::Version;
use serde::Serialize;

pub const THROTTLE_MS: u64 = 15 * 60_000;
pub const PERIOD_MS: u64 = 30 * 60_000;
pub const JITTER_MS: u64 = 5 * 60_000;
pub const RETRY_MAX_MS: u64 = 60 * 60_000;
/// Consecutive failed checks, downloads or installs before Settings offers a manual download. One network error is not
/// enough: a laptop that wakes offline recovers on its own.
pub const STUCK_AFTER: u32 = 3;
/// The public download page. It offers only the stable channel.
pub const DOWNLOAD_PAGE: &str = "https://nexus.inc/download";
/// The beta's download page: the open-source repository's releases, where each beta is a prerelease. A beta build sends
/// its users here, never to [`DOWNLOAD_PAGE`], which would move them off the beta without saying so.
pub const BETA_DOWNLOAD_PAGE: &str = "https://github.com/nexus-xyz/nexus-apps/releases";
/// The internal channel's installer (README.md, "Distribution"), for Nexus staff. It is configuration: Nexus's release
/// builds set `NEXUS_DESKTOP_INTERNAL_INSTALL`. A build without it (a local or open-source build) offers the download
/// page on the internal channel too.
pub const INTERNAL_INSTALL: Option<&str> = match option_env!("NEXUS_DESKTOP_INTERNAL_INSTALL") {
    Some(command) if !command.is_empty() => Some(command),
    _ => None,
};

/// The app whose feed this is: Nexus for desktop, in the `desktop/` folder on downloads.nexus.inc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum App {
    #[default]
    Desktop,
}

impl App {
    fn period_ms(self) -> u64 {
        match self {
            Self::Desktop => PERIOD_MS,
        }
    }

    /// Maximum jitter for a successful check. Hosts spread their timers within this window.
    pub fn jitter_ms(self) -> u64 {
        match self {
            Self::Desktop => JITTER_MS,
        }
    }

    /// The app's folder on downloads.nexus.inc and in the buckets.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Internal,
    /// The open-source repository's signed beta, `<x.y.z>-beta.<N>`. It is signed with that repository's updater key,
    /// so a beta build never reads the stable or internal feed, and those builds never read the beta feed.
    Beta,
}

impl Channel {
    pub fn of(version: &str) -> Option<Self> {
        let version = Version::parse(version).ok()?;
        let numbered = |prefix: &str| {
            version
                .pre
                .as_str()
                .strip_prefix(prefix)
                .is_some_and(|n| n.parse::<u64>().is_ok())
        };
        if version.pre.is_empty() {
            Some(Self::Stable)
        } else if numbered("internal.") {
            Some(Self::Internal)
        } else if numbered("beta.") {
            Some(Self::Beta)
        } else {
            None
        }
    }

    /// The channel a build checks without asking Gateway: beta builds stay on beta. Other builds ask Gateway whether
    /// the person may read the internal channel.
    pub fn fixed(installed: &str) -> Option<Self> {
        Self::of(installed).filter(|channel| *channel == Self::Beta)
    }

    /// The channel's folder on downloads.nexus.inc: its `latest.json` and one folder per version.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Stable => "desktop/stable",
            Self::Internal => "desktop/internal",
            Self::Beta => "desktop-oss/beta",
        }
    }

    /// Nexus for desktop's manifest for this channel.
    pub fn endpoint(self) -> &'static str {
        self.feed(App::Desktop)
    }

    /// `app`'s manifest for this channel.
    pub fn feed(self, app: App) -> &'static str {
        match (app, self) {
            (App::Desktop, Self::Stable) => "https://downloads.nexus.inc/desktop/stable/latest.json",
            (App::Desktop, Self::Internal) => "https://downloads.nexus.inc/desktop/internal/latest.json",
            (App::Desktop, Self::Beta) => "https://downloads.nexus.inc/desktop-oss/beta/latest.json",
        }
    }
}

/// Why automatic updates are off. Only a broken build offers a manual download: an organization or the system package
/// manager owns the updates of the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    Managed,
    PackageManager,
    NoUpdater,
    MissingKey,
    InvalidConfig,
}

impl Disabled {
    pub fn message(self) -> &'static str {
        match self {
            Self::Managed => {
                "Your organization manages updates. Contact your device administrator for a newer version."
            }
            Self::PackageManager => "Your system package manager manages updates. Use it to update Nexus.",
            Self::NoUpdater => "This build has no updater. Install a released desktop build to receive updates.",
            Self::MissingKey => "This build has no update verification key. Install a released desktop build.",
            Self::InvalidConfig => {
                "This build's update configuration is invalid. Install a released desktop build or contact support."
            }
        }
    }

    fn offers_download(self) -> bool {
        matches!(self, Self::MissingKey | Self::InvalidConfig)
    }
}

/// A way to the newest build when automatic updating is stuck: the stable download page, or the internal installer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manual {
    pub download_url: Option<&'static str>,
    pub install_command: Option<&'static str>,
    /// The channel's newest version, read from its `latest.json` without the updater, when it is newer than this build.
    pub latest_version: Option<String>,
}

/// The `version` of a channel manifest (`latest.json`).
pub fn manifest_version(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let version = value.get("version")?.as_str()?;
    Version::parse(version.trim_start_matches('v')).ok()?;
    Some(version.trim_start_matches('v').to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Disabled,
    Idle,
    Checking,
    Downloading,
    Verifying,
    Ready,
    Installing,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub installed_version: String,
    pub channel: Option<Channel>,
    pub phase: Phase,
    pub last_check: Option<u64>,
    pub next_check: Option<u64>,
    pub available_version: Option<String>,
    pub ready_version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<Failure>,
    pub disabled_reason: Option<String>,
    pub deferred: bool,
    pub manual: Option<Manual>,
}

impl Snapshot {
    /// The version the ready toast offers, or None. Not now ([`Coordinator::defer`]) sets
    /// `deferred` until the next launch, and [`Coordinator::finish`] clears it only when a newer version is ready, so a
    /// dismissed version stays dismissed. The offer holds through a background check and a restart, not through a
    /// newer download.
    pub fn toast_offer(&self) -> Option<&str> {
        let version = self.ready_version.as_deref().filter(|_| !self.deferred)?;
        matches!(self.phase, Phase::Ready | Phase::Checking | Phase::Installing).then_some(version)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

/// Where a failed check, download or install is reported, for the app's client errors: the shell sets
/// it at start. Only the failure's code leaves, never its message. The account and session states that are not
/// failures (`finish` does not count them as stuck) are not reported.
static FAILURE_REPORTER: std::sync::OnceLock<fn(&'static str)> = std::sync::OnceLock::new();

/// Sets the reporter of failed updates. The first call wins.
pub fn report_failures_to(reporter: fn(&'static str)) {
    let _ = FAILURE_REPORTER.set(reporter);
}

fn report_failure(code: &'static str) {
    if let Some(reporter) = FAILURE_REPORTER.get() {
        reporter(code);
    }
}

/// Without a verified employee session, a public build checks the stable feed and a beta build its beta feed. An
/// internal build reports the account failure instead: moving it to the stable feed would hide why its internal updates
/// stopped.
pub fn unverified_channel(installed: &str, failure: Failure) -> Result<Channel, Failure> {
    if let Some(channel) = Channel::fixed(installed) {
        return Ok(channel);
    }
    let account = matches!(failure.code, "employee_sign_in_required" | "employee_access_required");
    if account && Channel::of(installed) != Some(Channel::Internal) {
        Ok(Channel::Stable)
    } else {
        Err(failure)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Trigger {
    Launch,
    Background,
    Wake,
    Reconnect,
    Focus,
    Manual,
}

pub struct Coordinator {
    pub snapshot: Snapshot,
    /// Whose feed: it decides the manual fallback. Nexus GPUI has no download page or install script yet.
    app: App,
    /// An asynchronous result may publish only into the session that started it.
    pub generation: u64,
    earliest: u64,
    failures: u32,
    observed: u64,
    disabled: Option<Disabled>,
    /// Consecutive failures that a reinstall can fix. Account and session errors neither count nor reset it.
    stuck: u32,
}

impl Coordinator {
    /// The Nexus app's coordinator.
    pub fn new(version: String, disabled: Option<Disabled>, now: u64) -> Self {
        Self::of(App::Desktop, version, disabled, now)
    }

    pub fn of(app: App, version: String, disabled: Option<Disabled>, now: u64) -> Self {
        let mut coordinator = Self {
            app,
            generation: 0,
            snapshot: Snapshot {
                channel: Channel::of(&version),
                installed_version: version,
                phase: if disabled.is_some() {
                    Phase::Disabled
                } else {
                    Phase::Idle
                },
                disabled_reason: disabled.map(|d| d.message().to_string()),
                revision: 0,
                last_check: None,
                next_check: Some(now),
                available_version: None,
                ready_version: None,
                downloaded: 0,
                total: None,
                error: None,
                deferred: false,
                manual: None,
            },
            earliest: now,
            failures: 0,
            observed: now,
            disabled,
            stuck: 0,
        };
        coordinator.refresh_manual();
        coordinator
    }

    /// Offers a manual download for a broken build, or after `STUCK_AFTER` failures in a row. Never for a managed or
    /// package-manager install: those are disabled and never fail.
    fn refresh_manual(&mut self) {
        let offered = self.app == App::Desktop
            && match self.disabled {
                Some(disabled) => disabled.offers_download(),
                None => self.stuck >= STUCK_AFTER,
            };
        if !offered {
            self.snapshot.manual = None;
            return;
        }
        let channel = self
            .snapshot
            .channel
            .or_else(|| Channel::of(&self.snapshot.installed_version))
            .unwrap_or(Channel::Stable);
        let latest_version = self.snapshot.manual.take().and_then(|m| m.latest_version);
        self.snapshot.manual = Some(match channel {
            Channel::Stable => Manual {
                download_url: Some(DOWNLOAD_PAGE),
                install_command: None,
                latest_version,
            },
            // The download page offers stable only: a beta tester who reinstalled from it would leave the beta.
            Channel::Beta => Manual {
                download_url: Some(BETA_DOWNLOAD_PAGE),
                install_command: None,
                latest_version,
            },
            Channel::Internal => Manual {
                download_url: INTERNAL_INSTALL.is_none().then_some(DOWNLOAD_PAGE),
                install_command: INTERNAL_INSTALL,
                latest_version,
            },
        });
    }

    /// The version a plain read of the channel's `latest.json` found. Kept only when it is newer than this build.
    pub fn latest(&mut self, version: &str) {
        let newer = match (
            Version::parse(version),
            Version::parse(&self.snapshot.installed_version),
        ) {
            (Ok(remote), Ok(installed)) => remote > installed,
            (Ok(_), Err(_)) => true,
            _ => false,
        };
        if let Some(manual) = self.snapshot.manual.as_mut()
            && newer
        {
            manual.latest_version = Some(version.to_string());
            self.changed();
        }
    }

    /// A failed install after consent counts toward a manual download, like a failed download.
    pub fn install_failed(&mut self, error: Failure) {
        report_failure(error.code);
        self.stuck = self.stuck.saturating_add(1);
        self.snapshot.error = Some(error);
        self.refresh_manual();
        self.changed();
    }

    pub fn changed(&mut self) {
        self.snapshot.revision += 1;
    }

    /// Sign-out or an account change removes an old offer and permits an immediate new check.
    pub fn session_changed(&mut self, now: u64) {
        let generation = self.generation + 1;
        let revision = self.snapshot.revision + 1;
        // A broken build stays broken across accounts: keep the count toward a manual download.
        let stuck = self.stuck;
        *self = Self::of(self.app, self.snapshot.installed_version.clone(), self.disabled, now);
        self.generation = generation;
        self.snapshot.revision = revision;
        self.stuck = stuck;
        self.refresh_manual();
    }

    /// The host selects internal only after verifying the current employee session.
    pub fn select_channel(&mut self, channel: Channel) {
        if self.snapshot.channel != Some(channel) {
            self.snapshot.ready_version = None;
            self.snapshot.available_version = None;
            self.snapshot.downloaded = 0;
            self.snapshot.total = None;
            self.snapshot.deferred = false;
        }
        self.snapshot.channel = Some(channel);
        self.refresh_manual();
    }

    /// A corrected wall clock must not leave automatic checks waiting for the old future date.
    fn observe(&mut self, now: u64) {
        if now < self.observed {
            self.earliest = now.saturating_add(self.earliest.saturating_sub(self.observed));
            self.snapshot.next_check = self
                .snapshot
                .next_check
                .map(|deadline| now.saturating_add(deadline.saturating_sub(self.observed)));
            self.snapshot.last_check = self
                .snapshot
                .last_check
                .map(|checked| now.saturating_sub(self.observed.saturating_sub(checked)));
            self.changed();
        }
        self.observed = now;
    }

    /// The host holds one mutex around this decision, before it starts any asynchronous work.
    pub fn begin(&mut self, trigger: Trigger, now: u64) -> bool {
        self.observe(now);
        if matches!(
            self.snapshot.phase,
            Phase::Disabled | Phase::Checking | Phase::Downloading | Phase::Verifying | Phase::Installing
        ) {
            return false;
        }
        if !matches!(trigger, Trigger::Manual)
            && (now < self.earliest
                || (matches!(trigger, Trigger::Launch | Trigger::Background)
                    && now < self.snapshot.next_check.unwrap_or(u64::MAX)))
        {
            return false;
        }
        self.snapshot.phase = Phase::Checking;
        self.snapshot.last_check = Some(now);
        self.snapshot.error = None;
        self.changed();
        true
    }

    pub fn progress(&mut self, version: &str, downloaded: u64, total: Option<u64>, verifying: bool) {
        self.snapshot.available_version = Some(version.into());
        self.snapshot.phase = if verifying {
            Phase::Verifying
        } else {
            Phase::Downloading
        };
        self.snapshot.downloaded = downloaded;
        self.snapshot.total = total;
        self.changed();
    }

    pub fn finish(&mut self, result: Result<Option<String>, Failure>, now: u64, jitter: u64) {
        self.observe(now);
        let wait = match result {
            Ok(version) => {
                self.failures = 0;
                self.stuck = 0;
                if let Some(version) = version {
                    self.snapshot.ready_version = Some(version.clone());
                    self.snapshot.available_version = Some(version);
                    self.snapshot.deferred = false;
                }
                self.snapshot.phase = if self.snapshot.ready_version.is_some() {
                    Phase::Ready
                } else {
                    Phase::Idle
                };
                self.earliest = now.saturating_add(THROTTLE_MS);
                self.app.period_ms() + jitter.min(self.app.jitter_ms())
            }
            Err(error) => {
                if matches!(error.code, "employee_sign_in_required" | "employee_access_required") {
                    self.snapshot.ready_version = None;
                    self.snapshot.available_version = None;
                    self.snapshot.downloaded = 0;
                    self.snapshot.total = None;
                    self.snapshot.deferred = false;
                }
                self.failures = self.failures.saturating_add(1);
                if !matches!(
                    error.code,
                    "employee_sign_in_required" | "employee_access_required" | "session_changed" | "active_sessions"
                ) {
                    self.stuck = self.stuck.saturating_add(1);
                    report_failure(error.code);
                }
                let wait = if error.retryable {
                    (60_000u64.saturating_mul(1 << self.failures.min(7).saturating_sub(1))).min(RETRY_MAX_MS)
                } else {
                    self.app.period_ms()
                };
                self.snapshot.error = Some(error);
                self.snapshot.phase = if self.snapshot.ready_version.is_some() {
                    Phase::Ready
                } else {
                    Phase::Error
                };
                self.earliest = now.saturating_add(wait);
                wait
            }
        };
        self.snapshot.next_check = Some(now.saturating_add(wait));
        self.refresh_manual();
        self.changed();
    }

    pub fn defer(&mut self) {
        self.snapshot.deferred = true;
        self.changed();
    }
}

#[derive(Debug)]
pub struct Release<T> {
    pub version: String,
    pub artifact: T,
}

/// `prepare` must verify both the artifact signature and its signed version before returning bytes.
#[allow(async_fn_in_trait)]
pub trait Feed {
    type Candidate;
    type Prepared;
    async fn check(&self) -> Result<Option<Release<Self::Candidate>>, Failure>;
    async fn prepare(&self, release: Release<Self::Candidate>) -> Result<Self::Prepared, Failure>;
}

/// Channel and downgrade checks apply even if a feed announces a valid, signed artifact from another channel.
pub async fn fetch<F: Feed>(
    feed: &F,
    installed: &str,
    ready: Option<&str>,
) -> Result<Option<Release<F::Prepared>>, Failure> {
    fetch_channel(feed, installed, ready, Channel::of(installed)).await
}

/// A verified employee can move from a public installation to a newer internal build.
pub async fn fetch_channel<F: Feed>(
    feed: &F,
    installed: &str,
    ready: Option<&str>,
    channel: Option<Channel>,
) -> Result<Option<Release<F::Prepared>>, Failure> {
    let Some(release) = feed.check().await? else {
        return Ok(None);
    };
    let invalid = || {
        Failure::new(
            "invalid_release",
            "The release does not match this app's channel or version. Contact support.",
            false,
        )
    };
    let current = Version::parse(installed).map_err(|_| invalid())?;
    let remote = Version::parse(&release.version).map_err(|_| invalid())?;
    if Channel::of(installed).is_none() || channel.is_none() || channel != Channel::of(&release.version) {
        return Err(invalid());
    }
    if remote <= current || ready.and_then(|v| Version::parse(v).ok()).is_some_and(|v| remote <= v) {
        return Ok(None);
    }
    let version = release.version.clone();
    let artifact = feed.prepare(release).await?;
    Ok(Some(Release { version, artifact }))
}

#[cfg(test)]
mod tests;
