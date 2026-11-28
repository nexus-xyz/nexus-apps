use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct FakeClock(u64);
impl FakeClock {
    fn advance(&mut self, ms: u64) {
        self.0 += ms;
    }
}

struct FakeFeed {
    version: Option<&'static str>,
    error: Option<Failure>,
    invalid_signature: bool,
    checked: AtomicUsize,
    downloaded: AtomicUsize,
}

impl FakeFeed {
    fn new(version: Option<&'static str>) -> Self {
        Self {
            version,
            error: None,
            invalid_signature: false,
            checked: AtomicUsize::new(0),
            downloaded: AtomicUsize::new(0),
        }
    }
}

impl Feed for FakeFeed {
    type Candidate = ();
    type Prepared = Vec<u8>;
    async fn check(&self) -> Result<Option<Release<()>>, Failure> {
        self.checked.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(self.version.map(|version| Release {
            version: version.into(),
            artifact: (),
        }))
    }
    async fn prepare(&self, _: Release<()>) -> Result<Vec<u8>, Failure> {
        self.downloaded.fetch_add(1, Ordering::SeqCst);
        if self.invalid_signature {
            return Err(Failure::new("verification_failed", "Invalid signature", false));
        }
        Ok(vec![1, 2, 3])
    }
}

async fn run(c: &mut Coordinator, f: &FakeFeed, clock: &FakeClock, trigger: Trigger) {
    if !c.begin(trigger, clock.0) {
        return;
    }
    let result = fetch(f, &c.snapshot.installed_version, c.snapshot.ready_version.as_deref()).await;
    c.finish(result.map(|r| r.map(|r| r.version)), clock.0, 0);
}

#[tokio::test]
async fn launch_is_immediate_and_activity_checks_share_one_deadline() {
    let mut clock = FakeClock(10_000);
    let mut c = Coordinator::new("0.8.0".into(), None, clock.0);
    let feed = FakeFeed::new(None);
    assert_eq!(c.snapshot.next_check, Some(clock.0));
    run(&mut c, &feed, &clock, Trigger::Launch).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 1);
    for trigger in [
        Trigger::Launch,
        Trigger::Wake,
        Trigger::Reconnect,
        Trigger::Focus,
        Trigger::Background,
    ] {
        run(&mut c, &feed, &clock, trigger).await;
    }
    assert_eq!(feed.checked.load(Ordering::SeqCst), 1);
    clock.advance(THROTTLE_MS);
    run(&mut c, &feed, &clock, Trigger::Focus).await;
    clock.advance(THROTTLE_MS);
    run(&mut c, &feed, &clock, Trigger::Wake).await;
    clock.advance(THROTTLE_MS);
    run(&mut c, &feed, &clock, Trigger::Reconnect).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 4);
    clock.advance(PERIOD_MS);
    run(&mut c, &feed, &clock, Trigger::Background).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn each_app_checks_at_launch_and_downloads_at_its_background_deadline() {
    for (app, period) in [(App::Desktop, 30 * 60_000)] {
        let mut clock = FakeClock(10_000);
        let mut c = Coordinator::of(app, "0.8.0".into(), None, clock.0);
        let mut feed = FakeFeed::new(None);
        run(&mut c, &feed, &clock, Trigger::Launch).await;
        assert_eq!(feed.checked.load(Ordering::SeqCst), 1);
        assert_eq!(c.snapshot.next_check, Some(clock.0 + period));

        clock.advance(period - 1);
        feed.version = Some("0.9.0");
        run(&mut c, &feed, &clock, Trigger::Background).await;
        assert_eq!(feed.checked.load(Ordering::SeqCst), 1);
        assert_eq!(feed.downloaded.load(Ordering::SeqCst), 0);

        clock.advance(1);
        run(&mut c, &feed, &clock, Trigger::Background).await;
        assert_eq!(feed.checked.load(Ordering::SeqCst), 2);
        assert_eq!(feed.downloaded.load(Ordering::SeqCst), 1);
        assert_eq!(c.snapshot.ready_version.as_deref(), Some("0.9.0"));
        assert_eq!(c.snapshot.phase, Phase::Ready);
    }
}

#[test]
fn each_app_bounds_its_jitter_and_waits_for_the_full_background_deadline() {
    for (app, period, cap) in [(App::Desktop, 30 * 60_000, 5 * 60_000)] {
        assert_eq!(app.jitter_ms(), cap);
        for jitter in [0, 60_000, u64::MAX] {
            let mut c = Coordinator::of(app, "0.8.0".into(), None, 10_000);
            assert!(c.begin(Trigger::Launch, 10_000));
            c.finish(Ok(None), 10_000, jitter);
            let due = 10_000 + period + jitter.min(cap);
            assert_eq!(c.snapshot.next_check, Some(due));
            assert!(!c.begin(Trigger::Background, due - 1));
            assert!(c.begin(Trigger::Background, due));
        }
    }
}

#[tokio::test]
async fn gpui_retries_permanent_errors_on_its_regular_schedule() {
    let mut clock = FakeClock(0);
    let mut c = Coordinator::of(App::Desktop, "0.8.0".into(), None, clock.0);
    let mut feed = FakeFeed::new(None);
    feed.error = Some(Failure::new("bad_manifest", "Invalid manifest", false));
    run(&mut c, &feed, &clock, Trigger::Launch).await;
    assert_eq!(c.snapshot.next_check, Some(30 * 60_000));
    clock.advance(30 * 60_000 - 1);
    run(&mut c, &feed, &clock, Trigger::Background).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 1);
    clock.advance(1);
    feed.error = None;
    run(&mut c, &feed, &clock, Trigger::Background).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 2);
    assert_eq!(c.snapshot.phase, Phase::Idle);
    assert_eq!(c.snapshot.next_check, Some(clock.0 + 30 * 60_000));
}

#[tokio::test]
async fn concurrent_windows_coalesce_until_download_and_verification_finish() {
    let c = Arc::new(Mutex::new(Coordinator::new("0.8.0".into(), None, 0)));
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let mut tasks = vec![];
    for _ in 0..16 {
        let c = c.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            c.lock().unwrap().begin(Trigger::Manual, 0)
        }));
    }
    let mut started = 0;
    for task in tasks {
        if task.await.unwrap() {
            started += 1;
        }
    }
    assert_eq!(started, 1);
    let mut c = c.lock().unwrap();
    c.progress("0.9.0", 50, Some(100), false);
    assert!(!c.begin(Trigger::Manual, PERIOD_MS));
    c.progress("0.9.0", 100, Some(100), true);
    assert!(!c.begin(Trigger::Manual, PERIOD_MS));
    c.finish(Ok(Some("0.9.0".into())), PERIOD_MS, JITTER_MS + 1);
    assert_eq!(c.snapshot.next_check, Some(2 * PERIOD_MS + JITTER_MS));
}

#[tokio::test]
async fn network_backoff_is_bounded_and_manual_retry_bypasses_it() {
    for (app, period) in [(App::Desktop, 30 * 60_000)] {
        let mut clock = FakeClock(0);
        let mut c = Coordinator::of(app, "0.8.0".into(), None, 0);
        let mut feed = FakeFeed::new(None);
        feed.error = Some(Failure::new("network", "offline", true));
        for expected in [
            60_000,
            120_000,
            240_000,
            480_000,
            960_000,
            1_920_000,
            RETRY_MAX_MS,
            RETRY_MAX_MS,
        ] {
            run(&mut c, &feed, &clock, Trigger::Manual).await;
            assert_eq!(c.snapshot.next_check, Some(clock.0 + expected));
            assert!(!c.begin(Trigger::Reconnect, clock.0 + expected - 1));
            clock.advance(expected);
        }
        feed.error = None;
        run(&mut c, &feed, &clock, Trigger::Background).await;
        assert_eq!(c.snapshot.phase, Phase::Idle);
        assert_eq!(c.snapshot.next_check, Some(clock.0 + period));
    }
}

#[tokio::test]
async fn only_newer_builds_from_the_installed_channel_are_downloaded() {
    for (installed, offered, downloads, error) in [
        ("0.8.0", "0.9.0", 1, false),
        ("0.8.0", "0.8.0", 0, false),
        ("0.8.0", "0.7.9", 0, false),
        ("0.8.0", "0.9.0-internal.2", 0, true),
        ("0.8.0-internal.1", "0.8.0", 0, true),
        ("0.8.0-internal.1", "0.8.0-internal.2", 1, false),
        ("0.8.0", "bad", 0, true),
        ("0.8.0", "0.9.0-beta.1", 0, true),
        ("0.9.0-beta.1", "0.9.0-beta.2", 1, false),
        ("0.9.0-beta.2", "0.9.0-beta.1", 0, false),
        ("0.9.0-beta.1", "0.9.0", 0, true),
        ("0.9.0-beta.1", "0.9.0-internal.3", 0, true),
        ("0.8.0-internal.1", "0.9.0-beta.1", 0, true),
        ("0.9.0-beta.x", "0.9.0-beta.2", 0, true),
    ] {
        let feed = FakeFeed::new(Some(offered));
        assert_eq!(
            fetch(&feed, installed, None).await.is_err(),
            error,
            "{installed} -> {offered}"
        );
        assert_eq!(feed.downloaded.load(Ordering::SeqCst), downloads);
    }
    assert!(Channel::Stable.endpoint().contains("/desktop/stable/"));
    assert!(Channel::Internal.endpoint().contains("/desktop/internal/"));
    assert!(Channel::Beta.endpoint().contains("/desktop-oss/beta/"));
    for channel in [Channel::Stable, Channel::Internal, Channel::Beta] {
        let folder = format!("https://downloads.nexus.inc/{}/latest.json", channel.folder());
        assert_eq!(channel.endpoint(), folder);
    }
}

#[test]
fn a_beta_build_keeps_its_channel_without_an_employee_check() {
    assert_eq!(Channel::of("0.12.0-beta.3"), Some(Channel::Beta));
    assert_eq!(Channel::of("0.12.0-beta"), None);
    assert_eq!(Channel::fixed("0.12.0-beta.3"), Some(Channel::Beta));
    assert_eq!(Channel::fixed("0.12.0"), None);
    assert_eq!(Channel::fixed("0.12.0-internal.4"), None);
    let failure = Failure::new("employee_sign_in_required", "Sign in.", false);
    assert_eq!(unverified_channel("0.12.0-beta.3", failure.clone()), Ok(Channel::Beta));
    let offline = Failure::new("network", "Offline.", true);
    assert_eq!(unverified_channel("0.12.0-beta.3", offline), Ok(Channel::Beta));
    let broken = Coordinator::new("0.12.0-beta.3".into(), Some(Disabled::MissingKey), 0);
    let manual = broken.snapshot.manual.expect("beta fallback");
    // A beta stays on the beta: the stable download page would move its tester off it without saying so.
    assert_eq!(manual.download_url, Some(BETA_DOWNLOAD_PAGE));
    assert_eq!(manual.install_command, None);
}

#[tokio::test]
async fn invalid_signature_never_becomes_ready_and_later_survives_an_unchanged_feed() {
    let clock = FakeClock(0);
    let mut c = Coordinator::new("0.8.0".into(), None, 0);
    let mut feed = FakeFeed::new(Some("0.9.0"));
    feed.invalid_signature = true;
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    assert_eq!(c.snapshot.phase, Phase::Error);
    assert!(c.snapshot.ready_version.is_none());
    assert_eq!(c.snapshot.next_check, Some(PERIOD_MS));
    feed.invalid_signature = false;
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    c.defer();
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    assert_eq!(c.snapshot.phase, Phase::Ready);
    assert!(c.snapshot.deferred);
    assert_eq!(feed.downloaded.load(Ordering::SeqCst), 2);
    feed.version = Some("0.10.0");
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    assert!(!c.snapshot.deferred);
    assert_eq!(c.snapshot.ready_version.as_deref(), Some("0.10.0"));
    feed.error = Some(Failure::new("network", "offline", true));
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    assert_eq!(c.snapshot.phase, Phase::Ready);
    assert_eq!(c.snapshot.ready_version.as_deref(), Some("0.10.0"));

    // A newer but invalid artifact must not replace the verified download already waiting for consent.
    feed.error = None;
    feed.version = Some("0.11.0");
    feed.invalid_signature = true;
    run(&mut c, &feed, &clock, Trigger::Manual).await;
    assert_eq!(c.snapshot.phase, Phase::Ready);
    assert_eq!(c.snapshot.ready_version.as_deref(), Some("0.10.0"));
    assert_eq!(c.snapshot.error.as_ref().map(|e| e.code), Some("verification_failed"));

    // Once installation owns that download, no window can start a replacement check.
    c.snapshot.phase = Phase::Installing;
    for trigger in [Trigger::Manual, Trigger::Reconnect, Trigger::Background] {
        assert!(!c.begin(trigger, PERIOD_MS));
    }
}

#[tokio::test]
async fn managed_builds_never_contact_the_feed_even_on_manual_checks() {
    let mut c = Coordinator::new("0.8.0".into(), Some(Disabled::Managed), 0);
    let feed = FakeFeed::new(Some("0.9.0"));
    run(&mut c, &feed, &FakeClock(PERIOD_MS), Trigger::Manual).await;
    assert_eq!(feed.checked.load(Ordering::SeqCst), 0);
    assert_eq!(c.snapshot.phase, Phase::Disabled);
}

#[test]
fn correcting_the_clock_before_launch_does_not_delay_the_first_check() {
    let ahead = 7 * 24 * 60 * 60_000;
    let mut c = Coordinator::new("0.8.0".into(), None, ahead);
    assert!(c.begin(Trigger::Launch, 1_000));
    assert_eq!(c.snapshot.next_check, Some(1_000));
    assert_eq!(c.snapshot.last_check, Some(1_000));
}

#[test]
fn correcting_the_clock_keeps_the_remaining_success_throttle_and_period() {
    let ahead = 7 * 24 * 60 * 60_000;
    for (trigger, wait) in [
        (Trigger::Focus, THROTTLE_MS),
        (Trigger::Wake, THROTTLE_MS),
        (Trigger::Reconnect, THROTTLE_MS),
        (Trigger::Background, PERIOD_MS),
    ] {
        let mut c = Coordinator::new("0.8.0".into(), None, ahead);
        assert!(c.begin(Trigger::Manual, ahead));
        c.finish(Ok(None), ahead, 0);
        assert!(!c.begin(Trigger::Background, ahead + 60_000));
        assert!(!c.begin(trigger, 1_000));
        assert_eq!(c.snapshot.next_check, Some(1_000 + PERIOD_MS - 60_000));
        assert_eq!(c.snapshot.last_check, Some(0));
        assert!(!c.begin(trigger, 1_000 + wait - 60_000 - 1));
        assert!(c.begin(trigger, 1_000 + wait - 60_000));
    }
}

#[test]
fn correcting_the_clock_keeps_the_remaining_retry_delay() {
    let ahead = 7 * 24 * 60 * 60_000;
    let mut c = Coordinator::new("0.8.0".into(), None, ahead);
    assert!(c.begin(Trigger::Manual, ahead));
    c.finish(Err(Failure::new("network", "offline", true)), ahead, 0);
    assert!(!c.begin(Trigger::Background, ahead + 10_000));
    assert!(!c.begin(Trigger::Reconnect, 1_000));
    assert_eq!(c.snapshot.next_check, Some(51_000));
    assert!(!c.begin(Trigger::Reconnect, 50_999));
    assert!(c.begin(Trigger::Reconnect, 51_000));
}

#[test]
fn correcting_the_clock_during_a_download_cannot_start_a_second_check() {
    let ahead = 7 * 24 * 60 * 60_000;
    let mut c = Coordinator::new("0.8.0".into(), None, ahead);
    assert!(c.begin(Trigger::Manual, ahead));
    c.progress("0.9.0", 10, Some(100), false);
    for trigger in [Trigger::Manual, Trigger::Wake, Trigger::Reconnect, Trigger::Background] {
        assert!(!c.begin(trigger, 1_000));
        assert_eq!(c.snapshot.phase, Phase::Downloading);
    }
    c.finish(Ok(Some("0.9.0".into())), 1_000, 0);
    assert_eq!(c.snapshot.last_check, Some(1_000));
    assert_eq!(c.snapshot.next_check, Some(1_000 + PERIOD_MS));
    assert_eq!(c.snapshot.phase, Phase::Ready);
}

#[test]
fn account_change_clears_old_offers_and_retry_delays() {
    let mut coordinator = Coordinator::new("0.8.0-internal.1".into(), None, 0);
    assert!(coordinator.begin(Trigger::Launch, 0));
    let old = coordinator.generation;
    coordinator.finish(Ok(Some("0.8.0-internal.2".into())), 1, 0);
    assert_eq!(coordinator.snapshot.phase, Phase::Ready);
    coordinator.defer();
    coordinator.session_changed(2);
    assert_ne!(coordinator.generation, old);
    assert!(coordinator.snapshot.ready_version.is_none());
    assert!(coordinator.snapshot.available_version.is_none());
    assert!(!coordinator.snapshot.deferred);
    assert!(coordinator.begin(Trigger::Launch, 2));
    coordinator.finish(Err(Failure::new("employee_sign_in_required", "Sign in", false)), 3, 0);
    coordinator.session_changed(4);
    assert!(coordinator.begin(Trigger::Launch, 4));
}

#[tokio::test]
async fn authorized_channel_changes_allow_only_newer_matching_releases() {
    for (installed, selected, remote, downloads, error) in [
        ("0.8.0", Channel::Internal, "0.9.0-internal.2", 1, false),
        ("0.8.0", Channel::Internal, "0.8.0-internal.2", 0, false),
        ("0.8.0-internal.1", Channel::Stable, "0.8.0", 1, false),
        ("0.8.0-internal.1", Channel::Stable, "0.7.9", 0, false),
        ("0.8.0", Channel::Stable, "0.9.0-internal.2", 0, true),
        ("0.8.0", Channel::Internal, "0.9.0", 0, true),
    ] {
        let feed = FakeFeed::new(Some(remote));
        assert_eq!(
            fetch_channel(&feed, installed, None, Some(selected)).await.is_err(),
            error
        );
        assert_eq!(feed.downloaded.load(Ordering::SeqCst), downloads);
    }
}

#[test]
fn losing_employee_access_removes_a_staged_internal_offer() {
    for code in ["employee_sign_in_required", "employee_access_required"] {
        let mut c = Coordinator::new("0.8.0-internal.1".into(), None, 0);
        c.finish(Ok(Some("0.8.0-internal.2".into())), 1, 0);
        c.finish(Err(Failure::new(code, "Denied", false)), 2, 0);
        assert_eq!(c.snapshot.phase, Phase::Error);
        assert!(c.snapshot.ready_version.is_none());
    }
    let mut c = Coordinator::new("0.8.0".into(), None, 0);
    c.select_channel(Channel::Internal);
    c.finish(Ok(Some("0.9.0-internal.2".into())), 1, 0);
    c.select_channel(Channel::Stable);
    assert!(c.snapshot.ready_version.is_none());
    assert!(c.snapshot.available_version.is_none());
}

#[test]
fn internal_builds_report_account_failures_instead_of_moving_to_stable() {
    for code in ["employee_sign_in_required", "employee_access_required"] {
        let failure = Failure::new(code, "Use a Nexus account on api.nexus.inc", false);
        assert_eq!(
            unverified_channel("0.8.0-internal.347", failure.clone()),
            Err(failure.clone())
        );
        assert_eq!(unverified_channel("0.8.0", failure), Ok(Channel::Stable));
    }
    let outage = Failure::new("employee_verification_unavailable", "Try again", true);
    for installed in ["0.8.0", "0.8.0-internal.347"] {
        assert_eq!(unverified_channel(installed, outage.clone()), Err(outage.clone()));
    }
}

fn fail(c: &mut Coordinator, code: &'static str, retryable: bool, now: u64) {
    assert!(c.begin(Trigger::Manual, now));
    c.finish(Err(Failure::new(code, "failed", retryable)), now, 0);
}

#[test]
fn a_broken_build_offers_the_download_page_or_the_internal_installer() {
    for disabled in [Disabled::InvalidConfig, Disabled::MissingKey] {
        let stable = Coordinator::new("0.8.0".into(), Some(disabled), 0);
        let manual = stable.snapshot.manual.expect("stable fallback");
        assert_eq!(manual.download_url, Some(DOWNLOAD_PAGE));
        assert_eq!(manual.install_command, None);
        assert_eq!(stable.snapshot.disabled_reason.as_deref(), Some(disabled.message()));

        let internal = Coordinator::new("0.8.0-internal.362".into(), Some(disabled), 0);
        let manual = internal.snapshot.manual.expect("internal fallback");
        assert_eq!(manual.download_url, INTERNAL_INSTALL.is_none().then_some(DOWNLOAD_PAGE));
        assert_eq!(manual.install_command, INTERNAL_INSTALL);
    }
}

#[test]
fn managed_package_manager_and_dev_builds_offer_no_download() {
    for disabled in [Disabled::Managed, Disabled::PackageManager, Disabled::NoUpdater] {
        let mut c = Coordinator::new("0.8.0".into(), Some(disabled), 0);
        assert_eq!(c.snapshot.manual, None);
        // A disabled updater never starts a check, so it can never count failures either.
        assert!(!c.begin(Trigger::Manual, 1));
        c.session_changed(2);
        assert_eq!(c.snapshot.manual, None);
    }
}

#[test]
fn one_network_error_offers_nothing_but_repeated_failures_do() {
    let mut c = Coordinator::new("0.8.0".into(), None, 0);
    fail(&mut c, "network", true, 1);
    assert_eq!(c.snapshot.manual, None);
    fail(&mut c, "verification_failed", false, 2);
    assert_eq!(c.snapshot.manual, None);
    fail(&mut c, "update_failed", false, 3);
    assert_eq!(
        c.snapshot.manual.as_ref().and_then(|m| m.download_url),
        Some(DOWNLOAD_PAGE)
    );
    // The fallback survives an account change and clears once a check succeeds.
    c.session_changed(4);
    assert!(c.snapshot.manual.is_some());
    assert!(c.begin(Trigger::Manual, 5));
    c.finish(Ok(None), 5, 0);
    assert_eq!(c.snapshot.manual, None);
}

#[test]
fn account_errors_do_not_count_toward_the_fallback() {
    let mut c = Coordinator::new("0.8.0-internal.1".into(), None, 0);
    for now in 1..=5 {
        fail(&mut c, "employee_sign_in_required", false, now);
    }
    assert_eq!(c.snapshot.manual, None);
    fail(&mut c, "release_not_found", false, 6);
    fail(&mut c, "network", true, 7);
    fail(&mut c, "employee_access_required", false, 8);
    assert_eq!(c.snapshot.manual, None);
    fail(&mut c, "network", true, 9);
    assert_eq!(
        c.snapshot.manual.as_ref().and_then(|m| m.install_command),
        INTERNAL_INSTALL
    );
}

#[test]
fn failed_installs_count_and_the_channel_picks_the_fallback() {
    let mut c = Coordinator::new("0.8.0".into(), None, 0);
    for _ in 0..STUCK_AFTER {
        c.install_failed(Failure::new("install_failed", "Installation failed", false));
    }
    assert_eq!(
        c.snapshot.manual.as_ref().and_then(|m| m.download_url),
        Some(DOWNLOAD_PAGE)
    );
    // A verified employee on a public build moves to the internal feed, and to its installer.
    c.select_channel(Channel::Internal);
    assert_eq!(
        c.snapshot.manual.as_ref().and_then(|m| m.install_command),
        INTERNAL_INSTALL
    );
}

#[test]
fn the_fallback_names_a_newer_version_from_the_manifest() {
    let mut c = Coordinator::new("0.8.0".into(), Some(Disabled::InvalidConfig), 0);
    c.latest("0.7.0");
    assert_eq!(c.snapshot.manual.as_ref().unwrap().latest_version, None);
    c.latest("0.9.0");
    assert_eq!(
        c.snapshot.manual.as_ref().unwrap().latest_version.as_deref(),
        Some("0.9.0")
    );

    let mut quiet = Coordinator::new("0.8.0".into(), None, 0);
    quiet.latest("0.9.0");
    assert_eq!(quiet.snapshot.manual, None);

    assert_eq!(
        manifest_version(r#"{"version":"0.9.0","platforms":{}}"#).as_deref(),
        Some("0.9.0")
    );
    assert_eq!(manifest_version(r#"{"version":"v0.9.0"}"#).as_deref(), Some("0.9.0"));
    assert_eq!(manifest_version(r#"{"version":"latest"}"#), None);
    assert_eq!(manifest_version("<html>"), None);
}

/// The ready toast's offer.
mod toast_offer {
    use super::*;

    fn ready() -> Snapshot {
        let mut snapshot = Coordinator::new("0.8.0-internal.1".into(), None, 0).snapshot;
        snapshot.revision = 5;
        snapshot.channel = Some(Channel::Internal);
        snapshot.phase = Phase::Ready;
        snapshot.last_check = Some(100);
        snapshot.next_check = Some(1000);
        snapshot.available_version = Some("0.8.0-internal.2".into());
        snapshot.ready_version = Some("0.8.0-internal.2".into());
        snapshot.downloaded = 100;
        snapshot.total = Some(100);
        snapshot
    }

    #[test]
    fn offers_a_verified_update_until_not_now_then_not_again_for_that_version() {
        assert_eq!(ready().toast_offer(), Some("0.8.0-internal.2"));
        let deferred = Snapshot {
            deferred: true,
            ..ready()
        };
        assert_eq!(deferred.toast_offer(), None);
        // A check that finds nothing newer keeps the version, and the core leaves `deferred` as it was.
        let checked = Snapshot {
            revision: 7,
            deferred: true,
            last_check: Some(200),
            ..ready()
        };
        assert_eq!(checked.toast_offer(), None);
    }

    #[test]
    fn offers_a_newer_version_again_after_not_now() {
        let newer = Snapshot {
            revision: 9,
            ready_version: Some("0.8.0-internal.3".into()),
            available_version: Some("0.8.0-internal.3".into()),
            ..ready()
        };
        assert_eq!(newer.toast_offer(), Some("0.8.0-internal.3"));
    }

    #[test]
    fn holds_through_a_background_check_and_a_restart_not_through_a_newer_download() {
        assert_eq!(
            Snapshot {
                phase: Phase::Checking,
                ..ready()
            }
            .toast_offer(),
            Some("0.8.0-internal.2")
        );
        assert_eq!(
            Snapshot {
                phase: Phase::Installing,
                ..ready()
            }
            .toast_offer(),
            Some("0.8.0-internal.2")
        );
        let downloading = Snapshot {
            phase: Phase::Downloading,
            available_version: Some("0.8.0-internal.3".into()),
            ..ready()
        };
        assert_eq!(downloading.toast_offer(), None);
    }

    #[test]
    fn offers_nothing_before_a_verified_update_or_when_updates_are_off() {
        assert_eq!(
            Snapshot {
                phase: Phase::Idle,
                ready_version: None,
                ..ready()
            }
            .toast_offer(),
            None
        );
        assert_eq!(
            Snapshot {
                phase: Phase::Error,
                ready_version: None,
                ..ready()
            }
            .toast_offer(),
            None
        );
        assert_eq!(
            Snapshot {
                phase: Phase::Disabled,
                ..ready()
            }
            .toast_offer(),
            None
        );
    }

    #[test]
    fn the_coordinator_dismisses_a_version_and_offers_the_next() {
        let mut c = Coordinator::new("0.8.0".into(), None, 0);
        assert!(c.begin(Trigger::Manual, 0));
        c.finish(Ok(Some("0.9.0".into())), 0, 0);
        assert_eq!(c.snapshot.toast_offer(), Some("0.9.0"));
        c.defer();
        assert_eq!(c.snapshot.toast_offer(), None);
        assert!(c.begin(Trigger::Manual, THROTTLE_MS));
        c.finish(Ok(Some("0.10.0".into())), THROTTLE_MS, 0);
        assert_eq!(c.snapshot.toast_offer(), Some("0.10.0"));
    }
}
