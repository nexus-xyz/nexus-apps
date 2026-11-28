//! Updates, on `nexus-desktop-updates`: one coordinator per process, the desktop app's channels, manifest format and
//! signature check, and its feed, `downloads.nexus.inc/desktop/<channel>/latest.json`. This app reads its package
//! under `platforms["<os>-<arch>-<format>"]` (`manifest::package_target`); older builds read the bare `<os>-<arch>`
//! keys, which the feed no longer lists.
//!
//! The channel: a verified employee session on Nexus's Gateway reads the internal channel, anyone else the stable one,
//! and an internal build never moves to stable. A public build reads stable when the employee check fails for any
//! reason, and the check's answer is held for a while, so focus and wake do not call Gateway each time
//! (`held_access::channel`). A verified
//! update waits until the person chooses Restart to update in Settings › App or in the tray's menu (`tray.rs`, from
//! [`offered`]); `install.rs` installs it, including the macOS app bundle from its signed `.dmg`.
//!
//! The release puts the update keys in `NEXUS_UPDATER_PUBKEY` at build time (the release workflow): the active key,
//! then the next key, separated by whitespace (`nexus_desktop_updates::verify`).
//! A build without it, or one run from `target/`, has no updater, and says so.

pub mod install;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nexus_desktop_updates::http::{self, Mode, Prepared};
use nexus_desktop_updates::{App, Coordinator, Disabled, Failure, Phase, Snapshot, Trigger};

/// The update keys: the base64 of one or more minisign public key files, separated by whitespace.
const PUBKEY: Option<&str> = option_env!("NEXUS_UPDATER_PUBKEY");

/// The update key this build carries, `None` in a keyless build. It also checks the Nexus host (`machines.rs`).
pub fn verification_key() -> Option<&'static str> {
    PUBKEY.map(str::trim).filter(|key| !key.is_empty())
}

/// Every packaged platform downloads and verifies the update before offering Restart to update.
const MODE: Mode = Mode::Install;

struct Updates {
    automatic: AtomicBool,
    /// A check was asked for before the workspace said which session it is for (a refusal of this version at launch,
    /// `revoked.rs`): the first [`session`] runs it as a manual check, so it runs with automatic updates off too.
    asked: AtomicBool,
    coordinator: Mutex<Coordinator>,
    /// The ready release's verified bytes.
    pending: Mutex<Option<Prepared>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The Gateway and whether someone is signed in to it: the session the channel is decided for.
    session: Mutex<Option<(String, Option<String>)>>,
    /// The employee check's last answer, held so a focus or a wake does not ask Gateway again.
    access: nexus_desktop_updates::held_access::Held,
    runtime: tokio::runtime::Handle,
    snapshots: tokio::sync::watch::Sender<Snapshot>,
}

static UPDATES: OnceLock<Updates> = OnceLock::new();

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Why this build does not update itself, in the Nexus app's order.
fn disabled(packaged: bool, pubkey: Option<&str>, version: &str) -> Option<Disabled> {
    // The Nexus app's rule: an administrator's Gateway or any forced key means the organization manages updates.
    let policy = crate::managed::policy();
    if policy.managed.is_some() || !policy.forced.is_empty() {
        return Some(Disabled::Managed);
    }
    if !packaged {
        return Some(Disabled::NoUpdater);
    }
    if pubkey.is_none_or(|key| key.trim().is_empty()) {
        return Some(Disabled::MissingKey);
    }
    nexus_desktop_updates::Channel::of(version)
        .is_none()
        .then_some(Disabled::InvalidConfig)
}

fn updates() -> Option<&'static Updates> {
    UPDATES.get()
}

/// Start the updater with the app: a check now, then every few minutes the coordinator decides whether one is due.
pub fn start() {
    let exe = std::env::current_exe().ok();
    let packaged = !cfg!(debug_assertions) && exe.as_deref().is_some_and(install::packaged);
    if packaged && let Some(exe) = &exe {
        install::clean_up(exe);
    }
    let mut coordinator = Coordinator::of(
        App::Desktop,
        crate::channel::VERSION.to_string(),
        disabled(packaged, PUBKEY, crate::channel::VERSION),
        now(),
    );
    stand_in(&mut coordinator);
    let snapshot = coordinator.snapshot.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("nexus-updates")
        .enable_all()
        .build()
        .expect("the updates runtime starts");
    // As the Gateway's: the runtime lives as long as the process.
    let runtime = Box::leak(Box::new(runtime)).handle().clone();
    let updates = Updates {
        automatic: AtomicBool::new(crate::paths::automatic_updates()),
        asked: AtomicBool::new(false),
        coordinator: Mutex::new(coordinator),
        pending: Mutex::new(None),
        task: Mutex::new(None),
        session: Mutex::new(None),
        access: Default::default(),
        runtime: runtime.clone(),
        snapshots: tokio::sync::watch::Sender::new(snapshot),
    };
    if UPDATES.set(updates).is_err() {
        return;
    }
    runtime.spawn(async {
        let mut previous = now();
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let current = now();
            // Wall time includes sleep. A delayed tick is a wake.
            let trigger = if current.saturating_sub(previous) > 90_000 {
                Trigger::Wake
            } else {
                Trigger::Background
            };
            previous = current;
            request(trigger);
        }
    });
}

/// A development build's stand-in for a verified update, for layout work and screenshots, as the Nexus app's
/// `?update=ready`: `NEXUS_GPUI_UPDATE=ready` shows 0.9.0 as ready and checks nothing.
/// Restart says no update is ready, since there is nothing to install. A release build ignores it.
fn standing_in() -> bool {
    cfg!(debug_assertions) && std::env::var("NEXUS_GPUI_UPDATE").is_ok_and(|value| value == "ready")
}

fn stand_in(coordinator: &mut Coordinator) {
    if !standing_in() {
        return;
    }
    let snapshot = &mut coordinator.snapshot;
    snapshot.phase = Phase::Ready;
    snapshot.disabled_reason = None;
    snapshot.last_check = Some(now());
    snapshot.next_check = None;
    snapshot.available_version = Some("0.9.0".into());
    snapshot.ready_version = Some("0.9.0".into());
    coordinator.changed();
}

/// The update state now.
pub fn snapshot() -> Option<Snapshot> {
    updates().map(|updates| lock(&updates.coordinator).snapshot.clone())
}

/// Each new update state, for a page that shows it.
pub fn subscribe() -> Option<tokio::sync::watch::Receiver<Snapshot>> {
    updates().map(|updates| updates.snapshots.subscribe())
}

/// Where to download the ready release in the browser, on a platform that only announces it.
pub fn download_url() -> Option<String> {
    let updates = updates()?;
    match &*lock(&updates.pending) {
        Some(Prepared::Announced { url }) => Some(url.clone()),
        _ => None,
    }
}

/// The verified version that Restart to update installs, as the Nexus app's `Updates::offered`: the tray's menu shows
/// Restart to Update while there is one. An announcement without verified bytes never offers a restart.
pub fn offered() -> Option<String> {
    let updates = updates()?;
    let snapshot = lock(&updates.coordinator).snapshot.clone();
    let verified = matches!(&*lock(&updates.pending), Some(Prepared::Verified { .. }));
    offer(&snapshot, verified)
}

/// The version to offer for `snapshot`, when the ready release is `verified` bytes.
fn offer(snapshot: &Snapshot, verified: bool) -> Option<String> {
    snapshot
        .ready_version
        .clone()
        .filter(|_| snapshot.phase == Phase::Ready && verified)
}

fn publish(updates: &Updates, coordinator: &mut Coordinator) {
    coordinator.changed();
    updates.snapshots.send_replace(coordinator.snapshot.clone());
}

/// The person signed in, out, or moved to another Gateway. The first call at launch only records the session: the
/// launch check takes this account's credential, if there is one. A later change drops the old offer.
pub fn session(gateway: &str, token: Option<String>) {
    let Some(updates) = updates() else {
        return;
    };
    let current = Some((gateway.to_string(), token));
    let previous = std::mem::replace(&mut *lock(&updates.session), current.clone());
    if previous.is_none() {
        let asked = updates.asked.swap(false, Ordering::SeqCst);
        request(if asked { Trigger::Manual } else { Trigger::Launch });
        return;
    }
    if previous == current {
        return;
    }
    let mut coordinator = lock(&updates.coordinator);
    if coordinator.snapshot.phase == Phase::Installing {
        return;
    }
    if let Some(task) = lock(&updates.task).take() {
        task.abort();
    }
    *lock(&updates.pending) = None;
    coordinator.session_changed(now());
    publish(updates, &mut coordinator);
    drop(coordinator);
    if automatic() {
        request(Trigger::Manual);
    }
}

/// Not now on the app window's toast (`workspace/update_toast.rs`, which offers Restart to update too), as the Nexus
/// app's `update_later`: the ready version is set aside until the next launch or a
/// newer one (`Snapshot::toast_offer`). Settings keeps offering it.
pub fn later() {
    let Some(updates) = updates() else {
        return;
    };
    let mut coordinator = lock(&updates.coordinator);
    coordinator.defer();
    publish(updates, &mut coordinator);
}

/// Settings' Check for updates.
pub fn check() {
    request(Trigger::Manual);
}

pub fn automatic() -> bool {
    updates().is_none_or(|updates| updates.automatic.load(Ordering::SeqCst))
}

/// Persist before changing behavior. A failed save keeps the previous preference in every window.
pub fn set_automatic(enabled: bool) -> Result<(), String> {
    crate::paths::save_automatic_updates(enabled)?;
    if let Some(updates) = updates() {
        updates.automatic.store(enabled, Ordering::SeqCst);
        let mut coordinator = lock(&updates.coordinator);
        publish(updates, &mut coordinator);
    }
    if enabled {
        request(Trigger::Launch);
    }
    Ok(())
}

fn should_check(automatic: bool, trigger: Trigger) -> bool {
    automatic || matches!(trigger, Trigger::Manual)
}

fn session_changed() -> Failure {
    Failure::new("session_changed", "The signed-in account changed. Check again.", false)
}

pub fn request(trigger: Trigger) {
    let Some(updates) = updates() else {
        return;
    };
    if !should_check(updates.automatic.load(Ordering::SeqCst), trigger) {
        return;
    }
    if standing_in() {
        return;
    }
    // The channel depends on the session: no check before the workspace says which one (`session`), which then runs
    // a manual one that was asked for.
    let session = lock(&updates.session);
    let Some((gateway, token)) = session.clone() else {
        // Under the session's lock, so the first `session` cannot miss it.
        if matches!(trigger, Trigger::Manual) {
            updates.asked.store(true, Ordering::SeqCst);
        }
        return;
    };
    drop(session);
    let mut coordinator = lock(&updates.coordinator);
    let revision = coordinator.snapshot.revision;
    if !coordinator.begin(trigger, now()) {
        // A clock correction can rebase the displayed deadlines without starting another check.
        if coordinator.snapshot.revision != revision {
            publish(updates, &mut coordinator);
        }
        return;
    }
    publish(updates, &mut coordinator);
    let installed = coordinator.snapshot.installed_version.clone();
    let generation = coordinator.generation;
    drop(coordinator);
    let task = updates.runtime.spawn(async move {
        let manual = matches!(trigger, Trigger::Manual);
        let result = check_feed(updates, generation, gateway, token, &installed, manual).await;
        let mut coordinator = lock(&updates.coordinator);
        if coordinator.generation != generation {
            return;
        }
        let result = result.map(|release| {
            release.map(|release| {
                *lock(&updates.pending) = Some(release.artifact);
                release.version
            })
        });
        // Per-process jitter avoids synchronized timers.
        coordinator.finish(
            result,
            now(),
            (now() ^ u64::from(std::process::id())) % App::Desktop.jitter_ms(),
        );
        if coordinator.snapshot.ready_version.is_none() {
            *lock(&updates.pending) = None;
        }
        publish(updates, &mut coordinator);
    });
    *lock(&updates.task) = Some(task);
}

async fn check_feed(
    updates: &'static Updates,
    generation: u64,
    gateway: String,
    token: Option<String>,
    installed: &str,
    manual: bool,
) -> Result<Option<nexus_desktop_updates::Release<Prepared>>, Failure> {
    let (channel, authorization) =
        nexus_desktop_updates::held_access::channel(&updates.access, &gateway, token.as_deref(), installed, manual)
            .await?;
    let ready = {
        let mut coordinator = lock(&updates.coordinator);
        if coordinator.generation != generation {
            return Err(session_changed());
        }
        coordinator.select_channel(channel);
        if coordinator.snapshot.ready_version.is_none() {
            *lock(&updates.pending) = None;
        }
        publish(updates, &mut coordinator);
        coordinator.snapshot.ready_version.clone()
    };
    let feed = http::Feed {
        client: http::client()?,
        app: App::Desktop,
        endpoint: channel.feed(App::Desktop).to_string(),
        target: nexus_desktop_updates::manifest::package_target().unwrap_or_default(),
        installed: installed.to_string(),
        pubkey: PUBKEY.unwrap_or_default().to_string(),
        authorization,
        mode: MODE,
        cache: Some(nexus_desktop_updates::cache::Cache::new(
            crate::paths::cache_dir().join("updates"),
        )),
        progress: Box::new(move |version, received, total, verifying| {
            let mut coordinator = lock(&updates.coordinator);
            if coordinator.generation == generation {
                coordinator.progress(version, received, total, verifying);
                publish(updates, &mut coordinator);
            }
        }),
    };
    nexus_desktop_updates::fetch_channel(&feed, installed, ready.as_deref(), Some(channel)).await
}

/// The verified release to install, taken for Restart to update. The state moves to installing.
pub struct Staged {
    version: String,
    bytes: Vec<u8>,
}

/// Take the ready release for installation, or say why there is none.
pub fn take_ready() -> Result<Staged, String> {
    let updates = updates().ok_or("This build has no updater.")?;
    let mut coordinator = lock(&updates.coordinator);
    if coordinator.snapshot.phase != Phase::Ready {
        return Err("No verified update is ready.".into());
    }
    let version = coordinator
        .snapshot
        .ready_version
        .clone()
        .ok_or("No verified update is ready.")?;
    let bytes = match lock(&updates.pending).take() {
        Some(Prepared::Verified { bytes, .. }) => bytes,
        Some(announced @ Prepared::Announced { .. }) => {
            *lock(&updates.pending) = Some(announced);
            return Err("Download the new version in the browser.".into());
        }
        None => return Err("No verified update is ready.".into()),
    };
    coordinator.snapshot.phase = Phase::Installing;
    publish(updates, &mut coordinator);
    Ok(Staged { version, bytes })
}

/// Install a staged release off the UI thread: what starts the new version once this copy quits.
pub fn install(staged: &Staged) -> Result<install::Relaunch, String> {
    let exe = std::env::current_exe().map_err(|error| format!("This copy's path is unknown: {error}."))?;
    install::install(&exe, &staged.bytes, &staged.version)
}

/// An installation failed: keep the staged release for another try, and count it toward the manual fallback.
pub fn install_failed(staged: Staged, message: String) {
    let Some(updates) = updates() else {
        return;
    };
    let mut coordinator = lock(&updates.coordinator);
    *lock(&updates.pending) = Some(Prepared::Verified {
        bytes: staged.bytes,
        url: String::new(),
    });
    coordinator.snapshot.phase = Phase::Ready;
    coordinator.install_failed(Failure::new(
        "install_failed",
        format!("Installation failed: {message} Try again or contact support."),
        false,
    ));
    publish(updates, &mut coordinator);
}

/// Restart refused before installing (Code sessions still running): say why, and keep the offer.
pub fn refused(message: String) {
    let Some(updates) = updates() else {
        return;
    };
    let mut coordinator = lock(&updates.coordinator);
    coordinator.snapshot.error = Some(Failure::new("active_sessions", message, false));
    publish(updates, &mut coordinator);
}

/// The status line under Updates, as the Nexus app's `updateStatus`.
pub fn status(snapshot: &Snapshot, announced: bool) -> String {
    let channel = snapshot
        .channel
        .map(|channel| match channel {
            nexus_desktop_updates::Channel::Stable => "stable",
            nexus_desktop_updates::Channel::Internal => "internal",
            nexus_desktop_updates::Channel::Beta => "beta",
        })
        .unwrap_or("configured");
    match snapshot.phase {
        Phase::Disabled => snapshot
            .disabled_reason
            .clone()
            .unwrap_or_else(|| "Updates are unavailable in this build.".into()),
        Phase::Checking => "Checking for updates…".into(),
        Phase::Downloading => format!(
            "Downloading Nexus {}…",
            snapshot.available_version.as_deref().unwrap_or_default()
        ),
        Phase::Verifying => "Verifying the update signature…".into(),
        Phase::Ready if announced => format!(
            "Nexus {} is available. Download it, then replace this app with it.",
            snapshot.ready_version.as_deref().unwrap_or_default()
        ),
        Phase::Ready => format!(
            "Nexus {} is ready. Restart when your work is finished.",
            snapshot.ready_version.as_deref().unwrap_or_default()
        ),
        Phase::Installing => "Installing the update and restarting…".into(),
        Phase::Error => "The update could not be prepared.".into(),
        Phase::Idle if snapshot.last_check.is_some() => {
            format!("No newer update is published to the {channel} channel.")
        }
        Phase::Idle if automatic() => "Updates are checked automatically.".into(),
        Phase::Idle => "Choose Check for updates to check manually.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_updates_default_to_background_checks_but_manual_checks_always_work() {
        for trigger in [
            Trigger::Launch,
            Trigger::Background,
            Trigger::Wake,
            Trigger::Reconnect,
            Trigger::Focus,
        ] {
            assert!(should_check(true, trigger));
            assert!(!should_check(false, trigger));
        }
        assert!(should_check(false, Trigger::Manual));
    }

    #[test]
    fn a_build_without_a_package_or_a_key_says_why() {
        assert_eq!(disabled(false, Some("key"), "0.1.2"), Some(Disabled::NoUpdater));
        assert_eq!(disabled(true, None, "0.1.2"), Some(Disabled::MissingKey));
        assert_eq!(disabled(true, Some(" "), "0.1.2"), Some(Disabled::MissingKey));
        assert_eq!(disabled(true, Some("key"), "0.1.2-beta.1"), None);
        assert_eq!(disabled(true, Some("key"), "0.1.2-rc.1"), Some(Disabled::InvalidConfig));
        assert_eq!(disabled(true, Some("key"), "0.1.2"), None);
        assert_eq!(disabled(true, Some("key"), "0.1.3-internal.4"), None);
    }

    #[test]
    fn the_status_line_names_this_app_and_what_to_do() {
        let mut snapshot = Coordinator::of(App::Desktop, "0.1.2".into(), None, 0).snapshot;
        assert_eq!(status(&snapshot, false), "Updates are checked automatically.");
        snapshot.last_check = Some(1);
        snapshot.channel = Some(nexus_desktop_updates::Channel::Internal);
        assert_eq!(
            status(&snapshot, false),
            "No newer update is published to the internal channel."
        );
        snapshot.phase = Phase::Ready;
        snapshot.ready_version = Some("0.1.3".into());
        assert!(status(&snapshot, false).ends_with("Restart when your work is finished."));
        assert!(status(&snapshot, true).contains("Download it"));
        // A broken build offers the desktop app's download page, which gives this app.
        let broken = Coordinator::of(App::Desktop, "0.1.2".into(), Some(Disabled::MissingKey), 0);
        assert!(broken.snapshot.manual.is_some());
    }

    #[test]
    fn only_a_verified_ready_update_is_offered() {
        let mut snapshot = Coordinator::of(App::Desktop, "0.1.2".into(), None, 0).snapshot;
        assert_eq!(offer(&snapshot, true), None);
        snapshot.phase = Phase::Downloading;
        snapshot.ready_version = Some("0.1.3".into());
        assert_eq!(offer(&snapshot, true), None);
        snapshot.phase = Phase::Ready;
        assert_eq!(offer(&snapshot, true), Some("0.1.3".into()));
        // An announcement without verified bytes must never offer a restart.
        assert_eq!(offer(&snapshot, false), None);
        // Installing: the offer is taken.
        snapshot.phase = Phase::Installing;
        assert_eq!(offer(&snapshot, true), None);
    }

    /// A feed served from a temp folder: `latest.json` and the release file, as the downloads Worker serves them.
    struct Folder {
        dir: std::path::PathBuf,
        address: String,
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Serve a signed release of `version`: `update.txt`, its `.sig` and `test.pub` from `release`, the Nexus app's
    /// fixtures unless `NEXUS_GPUI_UPDATE_RELEASE` names another folder (signed with its own minisign key).
    fn folder_feed(release: &std::path::Path, version: &str) -> (Folder, String) {
        use std::io::{BufRead as _, BufReader, Write as _};
        let dir = std::env::temp_dir().join(format!("nexus-gpui-tray-feed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(version)).expect("feed folder");
        std::fs::copy(release.join("update.txt"), dir.join(version).join("update.txt")).expect("release file");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("feed port");
        let address = format!("http://{}", listener.local_addr().expect("address"));
        let signature = std::fs::read_to_string(release.join("update.txt.sig")).expect("signature");
        let manifest = serde_json::json!({
            "version": version,
            "platforms": { "test": { "url": format!("{address}/{version}/update.txt"), "signature": signature.trim() } }
        });
        std::fs::write(dir.join("latest.json"), manifest.to_string()).expect("manifest");
        let root = dir.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().expect("stream"));
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                let (status, body) = match std::fs::read(root.join(path)) {
                    Ok(body) if !path.contains("..") => ("200 OK", body),
                    _ => ("404 Not Found", Vec::new()),
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        let pubkey = std::fs::read_to_string(release.join("test.pub")).expect("public key");
        (Folder { dir, address }, pubkey)
    }

    /// The updater's own path from a feed to the tray: the release is downloaded and its signature checked, the
    /// coordinator makes it ready, and only then is Restart to Update offered. An announcement alone cannot offer it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_verified_update_from_a_feed_is_offered_for_restart() {
        let release = std::env::var_os("NEXUS_GPUI_UPDATE_RELEASE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/nexus-desktop-updates/fixtures")
            });
        let version = std::env::var("NEXUS_GPUI_UPDATE_VERSION").unwrap_or_else(|_| "1.0.0".into());
        let (folder, pubkey) = folder_feed(&release, &version);
        assert_eq!(MODE, Mode::Install);
        for mode in [MODE, Mode::Announce] {
            let feed = http::Feed {
                client: http::client().expect("client"),
                app: App::Desktop,
                endpoint: format!("{}/latest.json", folder.address),
                target: "test".into(),
                installed: "0.1.2".into(),
                pubkey: pubkey.clone(),
                authorization: None,
                mode,
                cache: None,
                progress: Box::new(|_, _, _, _| {}),
            };
            let mut coordinator = Coordinator::of(App::Desktop, "0.1.2".into(), None, 1);
            assert!(coordinator.begin(Trigger::Manual, 2));
            assert_eq!(offer(&coordinator.snapshot, true), None);
            let fetched = nexus_desktop_updates::fetch_channel(
                &feed,
                "0.1.2",
                None,
                Some(nexus_desktop_updates::Channel::Stable),
            )
            .await
            .expect("the feed answers")
            .expect("a newer release");
            let verified = matches!(fetched.artifact, Prepared::Verified { .. });
            assert_eq!(verified, mode == Mode::Install);
            coordinator.finish(Ok(Some(fetched.version)), 3, 0);
            assert_eq!(coordinator.snapshot.phase, Phase::Ready);
            let offered = offer(&coordinator.snapshot, verified);
            match mode {
                Mode::Install => assert_eq!(offered.as_deref(), Some(version.as_str())),
                Mode::Announce => assert_eq!(offered, None),
            }
        }
    }
}
