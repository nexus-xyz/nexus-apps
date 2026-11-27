//! The organization's notifications on this computer: limits, access requests, billing and security, the
//! kinds a person turns on for Push in Settings › Notifications. Gateway cannot reach a desktop app, so the app
//! registers as a `desktop` notification device and collects its deliveries (`POST
//! /v1/notifications/devices/{id}/collect`, each one handed over once), then shows each as a system notification.
//!
//! - **One device per computer and Gateway.** Its token is random, kept in the keychain (`<gateway>#notify-device`)
//!   and reused, so registering again answers the same device. Registering it in another organization moves it
//!   there: Gateway revokes the old row as `replaced`. On Windows and Linux an older Nexus install reads the same
//!   entry, so they are one device. On macOS, Nexus GPUI keeps its own token and never asks for the old app's: the
//!   token costs nothing to make again, and a keychain dialog does.
//! - **Follows the sign-in.** A new sign-in, organization or Gateway registers again. Sign-out revokes the device
//!   before the credential is dropped ([`PathNotices::sign_out`]), within [`SIGN_OUT_LIMIT`].
//! - **Collects every [`EVERY`]**, at once after a sign-in, and doubles the wait on a failure up to [`MOST`]. A 404
//!   means Gateway revoked the device: the next try registers again. Too many devices (409 `device_limit`) stops it
//!   for this sign-in.
//! - **The switch** (Settings, `path_notifications`, on by default) mutes this computer only. Collection goes on
//!   while it is off, so nothing waits on Gateway to expire later.
//!
//! The loop ([`PathNotices::run`]) needs no toolkit: the shell gives it a [`Host`], which says who is signed in and
//! shows a notice. Nexus GPUI's is its `path_notices.rs`, which runs the loop only while it owns the Code sessions
//! store, so one app on a computer collects at a time.
//!
//! Code sessions keep their own notices ([`crate::notify`]): their moments are not on the path yet.

use std::sync::Mutex;
use std::time::Duration;

use nexus_client::Client;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::secrets::Vault;

/// How often a registered device collects.
pub const EVERY: Duration = Duration::from_secs(30);
/// The longest wait after failures.
pub const MOST: Duration = Duration::from_secs(15 * 60);
/// The wait after `device_limit`, until a new sign-in.
const STOPPED: Duration = Duration::from_secs(60 * 60);
/// A full page (Gateway answers at most 50): collect the rest soon.
const FULL_PAGE: usize = 50;
const SOON: Duration = Duration::from_secs(1);
/// The most sign-out waits for Gateway to revoke the device.
pub const SIGN_OUT_LIMIT: Duration = Duration::from_secs(3);

/// One notice to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// The app page a click opens, when it is one.
    pub opens: Option<String>,
}

/// Why a call failed, as far as the loop cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    /// 404: the device is revoked or not this caller's.
    Gone,
    /// 409 `device_limit`: the person has too many devices.
    Limit,
    Other,
}

/// What a shell gives the loop.
pub trait Host: Send + Sync {
    /// The current sign-in's key: the Gateway, the organization and the credential's marker. None when signed out, or
    /// when this shell does not collect now.
    fn key(&self) -> Option<String>;
    /// The current sign-in's Gateway URL and client.
    fn session(&self) -> Option<(String, Client)>;
    /// Where the device token is kept.
    fn vault(&self) -> Vault;
    /// Whether this computer shows the notices now (`path_notifications`, on by default).
    fn showing(&self) -> bool;
    /// Shows one notice as a system notification.
    fn show(&self, notice: Notice);
}

/// What the loop needs from Gateway; tests use a fake.
trait Gateway {
    async fn register(&self, token: &str) -> Result<String, Fail>;
    async fn collect(&self, device: &str) -> Result<Vec<Notice>, Fail>;
}

/// The device's state for one sign-in: its id once registered, and how many calls failed in a row.
#[derive(Debug, Default)]
struct Poller {
    device: Option<String>,
    failures: u32,
    stopped: bool,
}

impl Poller {
    /// One step: register when needed, then collect, showing each notice through `show` when `showing`. The wait
    /// before the next step.
    async fn step(
        &mut self,
        gateway: &impl Gateway,
        token: &str,
        showing: bool,
        mut show: impl FnMut(Notice),
    ) -> Duration {
        if self.stopped {
            return STOPPED;
        }
        let device = match &self.device {
            Some(device) => device.clone(),
            None => match gateway.register(token).await {
                Ok(device) => {
                    self.device = Some(device.clone());
                    device
                }
                Err(Fail::Limit) => {
                    eprintln!(
                        "nexus-desktop: notifications: too many devices; this computer shows none until a new sign-in"
                    );
                    self.stopped = true;
                    return STOPPED;
                }
                Err(_) => return self.failed(),
            },
        };
        match gateway.collect(&device).await {
            Ok(notices) => {
                self.failures = 0;
                let full = notices.len() >= FULL_PAGE;
                if showing {
                    notices.into_iter().for_each(&mut show);
                }
                if full { SOON } else { EVERY }
            }
            Err(fail) => {
                if fail == Fail::Gone {
                    self.device = None;
                }
                self.failed()
            }
        }
    }

    fn failed(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        backoff(self.failures)
    }
}

/// The wait after `failures` failed calls in a row: [`EVERY`] doubled each time, at most [`MOST`].
pub fn backoff(failures: u32) -> Duration {
    let doubled = EVERY.saturating_mul(1u32 << failures.min(10));
    doubled.min(MOST)
}

/// The app page a notice opens: Gateway's path when it is one of the app's own (`/app` or below), else none.
pub fn opens(path: &str) -> Option<String> {
    let page = path.split(['?', '#']).next().unwrap_or_default();
    (page == "/app" || page.starts_with("/app/")).then(|| path.to_string())
}

/// A collected item as a notice, or None when it lacks a title.
fn notice(item: &Value) -> Option<Notice> {
    let text = |name: &str| item.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let title = text("title");
    if title.is_empty() {
        return None;
    }
    Some(Notice {
        title,
        body: text("body"),
        opens: opens(&text("path")),
    })
}

fn fail_of(error: &nexus_client::Error) -> Fail {
    match error {
        nexus_client::Error::Core(core) if core.status == 404 => Fail::Gone,
        nexus_client::Error::Core(core) if core.status == 409 && core.code.as_deref() == Some("device_limit") => {
            Fail::Limit
        }
        _ => Fail::Other,
    }
}

/// Gateway through the person's sign-in.
struct Live<'a>(&'a Client);

impl Gateway for Live<'_> {
    async fn register(&self, token: &str) -> Result<String, Fail> {
        let answer = self
            .0
            .post(
                "/v1/notifications/devices",
                json!({ "platform": "desktop", "token": token }),
            )
            .await
            .map_err(|error| fail_of(&error))?;
        answer
            .get("id")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or(Fail::Other)
    }

    async fn collect(&self, device: &str) -> Result<Vec<Notice>, Fail> {
        let path = format!("/v1/notifications/devices/{}/collect", nexus_client::url::enc(device));
        let answer = self.0.post(&path, json!({})).await.map_err(|error| fail_of(&error))?;
        let items = answer.get("items").and_then(Value::as_array).ok_or(Fail::Other)?;
        Ok(items.iter().filter_map(notice).collect())
    }
}

/// The keychain account of this computer's device token for `gateway`.
fn token_account(gateway: &str) -> String {
    format!("{gateway}#notify-device")
}

/// This computer's device token for `gateway`: the kept one, else a new random one, kept. None when the keychain
/// cannot keep it (a new token each launch would register a new device each time). Read with [`Vault::get_own`], so
/// it never shows a keychain dialog.
pub fn device_token(vault: &Vault, gateway: &str) -> Option<String> {
    let account = token_account(gateway);
    if let Some(token) = vault.get_own(&account).filter(|token| !token.is_empty()) {
        return Some(token);
    }
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    match vault.set(&account, &token) {
        Ok(()) => Some(token),
        Err(error) => {
            eprintln!("nexus-desktop: notifications: {error}");
            None
        }
    }
}

/// The device registered for the current sign-in, shared by the loop and [`PathNotices::sign_out`].
#[derive(Default)]
pub struct PathNotices {
    /// The sign-in (Gateway, organization and the credential's marker) and its device id, once registered.
    current: Mutex<Option<(String, Option<String>)>>,
    /// Held by the loop for a whole step, so [`PathNotices::sign_out`] reads the device id only once a registration in
    /// flight has stored it.
    busy: tokio::sync::Mutex<()>,
    nudge: Notify,
}

impl PathNotices {
    fn device_for(&self, sign_in: &str) -> Option<String> {
        let current = self.current.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        current
            .as_ref()
            .filter(|(key, _)| key == sign_in)
            .and_then(|(_, device)| device.clone())
    }

    fn set(&self, sign_in: Option<String>, device: Option<String>) {
        let mut current = self.current.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *current = sign_in.map(|key| (key, device));
    }

    /// `sign_in`'s device id once no step runs: a registration in flight finishes first. None after `limit`, or when
    /// it has none.
    async fn settled_device(&self, sign_in: &str, limit: Duration) -> Option<String> {
        let _step = tokio::time::timeout(limit, self.busy.lock()).await.ok()?;
        self.device_for(sign_in)
    }

    /// The sign-in changed: register or stop at once.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// The loop, for the app's life. It sleeps while `host` has no sign-in, and wakes at once on [`PathNotices::nudge`].
    pub async fn run(&self, host: &impl Host) {
        let mut poller = Poller::default();
        let mut for_key: Option<String> = None;
        loop {
            let key = host.key();
            if key != for_key {
                poller = Poller::default();
                for_key = key.clone();
                self.set(key.clone(), None);
            }
            let wait = match (key, host.session()) {
                (Some(key), Some((gateway, client))) => match device_token(&host.vault(), &gateway) {
                    None => MOST,
                    Some(token) => {
                        let showing = host.showing();
                        let _step = self.busy.lock().await;
                        let wait = poller
                            .step(&Live(&client), &token, showing, |notice| {
                                if still_signed_in(host.key().as_deref(), &key) {
                                    host.show(notice)
                                }
                            })
                            .await;
                        if still_signed_in(host.key().as_deref(), &key) {
                            self.set(Some(key), poller.device.clone());
                        }
                        wait
                    }
                },
                _ => MOST,
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.nudge.notified() => {}
            }
        }
    }

    /// Before sign-out drops the credential of `key` (None: signed out already): revoke this computer's device through
    /// `client`, so its notifications stop at once. A registration in flight finishes first, so its device is revoked
    /// too. Waits at most [`SIGN_OUT_LIMIT`] in all; a failure leaves Gateway to revoke it with the credential, or at
    /// the next registration.
    pub async fn sign_out(&self, key: Option<&str>, client: &Client) {
        let Some(key) = key else {
            return;
        };
        let revoke = async {
            let device = self.settled_device(key, SIGN_OUT_LIMIT).await?;
            let path = format!(
                "/v1/notifications/devices/{}?reason=signed_out",
                nexus_client::url::enc(&device)
            );
            client.delete(&path).await.ok()
        };
        let _ = tokio::time::timeout(SIGN_OUT_LIMIT, revoke).await;
        self.set(None, None);
    }
}

/// Whether a notice collected for `collected_for` may show now: the sign-in has not changed since. A notice from the
/// sign-in before a switch would open its page in the new organization.
fn still_signed_in(now: Option<&str>, collected_for: &str) -> bool {
    now == Some(collected_for)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    /// Answers queued in order; records each call.
    #[derive(Default)]
    struct Fake {
        registers: RefCell<VecDeque<Result<String, Fail>>>,
        collects: RefCell<VecDeque<Result<Vec<Notice>, Fail>>>,
        calls: RefCell<Vec<String>>,
    }

    impl Gateway for Fake {
        async fn register(&self, token: &str) -> Result<String, Fail> {
            self.calls.borrow_mut().push(format!("register {token}"));
            self.registers.borrow_mut().pop_front().unwrap_or(Err(Fail::Other))
        }

        async fn collect(&self, device: &str) -> Result<Vec<Notice>, Fail> {
            self.calls.borrow_mut().push(format!("collect {device}"));
            self.collects.borrow_mut().pop_front().unwrap_or(Ok(vec![]))
        }
    }

    fn notice(title: &str) -> Notice {
        Notice {
            title: title.into(),
            body: String::new(),
            opens: Some("/app?next=%2Fnotifications".into()),
        }
    }

    async fn step(poller: &mut Poller, fake: &Fake, showing: bool) -> (Duration, Vec<String>) {
        let mut shown = vec![];
        let wait = poller
            .step(fake, "tok", showing, |notice| shown.push(notice.title))
            .await;
        (wait, shown)
    }

    #[tokio::test]
    async fn registers_once_then_shows_each_collected_notice_once() {
        let fake = Fake::default();
        fake.registers.borrow_mut().push_back(Ok("ndv_1".into()));
        fake.collects.borrow_mut().extend([
            Ok(vec![notice("A spending limit changed"), notice("Payment failed")]),
            Ok(vec![]),
        ]);
        let mut poller = Poller::default();
        assert_eq!(
            step(&mut poller, &fake, true).await,
            (EVERY, vec!["A spending limit changed".into(), "Payment failed".into()])
        );
        assert_eq!(step(&mut poller, &fake, true).await, (EVERY, vec![]));
        assert_eq!(
            *fake.calls.borrow(),
            ["register tok", "collect ndv_1", "collect ndv_1"],
            "one registration for the sign-in"
        );
    }

    #[tokio::test]
    async fn the_switch_off_still_collects_and_shows_nothing() {
        let fake = Fake::default();
        fake.registers.borrow_mut().push_back(Ok("ndv_1".into()));
        fake.collects.borrow_mut().push_back(Ok(vec![notice("Payment failed")]));
        let mut poller = Poller::default();
        assert_eq!(step(&mut poller, &fake, false).await, (EVERY, vec![]));
        assert_eq!(*fake.calls.borrow(), ["register tok", "collect ndv_1"]);
    }

    #[tokio::test]
    async fn a_revoked_device_registers_again_after_a_backoff() {
        let fake = Fake::default();
        fake.registers
            .borrow_mut()
            .extend([Ok("ndv_1".into()), Ok("ndv_2".into())]);
        fake.collects
            .borrow_mut()
            .extend([Err(Fail::Gone), Ok(vec![notice("Access request")])]);
        let mut poller = Poller::default();
        assert_eq!(step(&mut poller, &fake, true).await, (backoff(1), vec![]));
        assert_eq!(
            step(&mut poller, &fake, true).await,
            (EVERY, vec!["Access request".into()])
        );
        assert_eq!(
            *fake.calls.borrow(),
            ["register tok", "collect ndv_1", "register tok", "collect ndv_2"]
        );
    }

    #[tokio::test]
    async fn too_many_devices_stops_without_more_calls() {
        let fake = Fake::default();
        fake.registers.borrow_mut().push_back(Err(Fail::Limit));
        let mut poller = Poller::default();
        assert_eq!(step(&mut poller, &fake, true).await, (STOPPED, vec![]));
        assert_eq!(step(&mut poller, &fake, true).await, (STOPPED, vec![]));
        assert_eq!(*fake.calls.borrow(), ["register tok"]);
    }

    #[tokio::test]
    async fn failures_back_off_and_a_success_resets_the_wait() {
        let fake = Fake::default();
        fake.registers
            .borrow_mut()
            .extend([Err(Fail::Other), Err(Fail::Other), Ok("ndv_1".into())]);
        let mut poller = Poller::default();
        assert_eq!(step(&mut poller, &fake, true).await.0, backoff(1));
        assert_eq!(step(&mut poller, &fake, true).await.0, backoff(2));
        assert_eq!(step(&mut poller, &fake, true).await.0, EVERY);
        assert_eq!(poller.failures, 0);
    }

    #[tokio::test]
    async fn a_full_page_collects_again_soon() {
        let fake = Fake::default();
        fake.registers.borrow_mut().push_back(Ok("ndv_1".into()));
        fake.collects
            .borrow_mut()
            .push_back(Ok((0..FULL_PAGE).map(|n| notice(&n.to_string())).collect()));
        let mut poller = Poller::default();
        let (wait, shown) = step(&mut poller, &fake, true).await;
        assert_eq!((wait, shown.len()), (SOON, FULL_PAGE));
    }

    #[test]
    fn the_wait_doubles_up_to_fifteen_minutes() {
        assert_eq!(backoff(1), Duration::from_secs(60));
        assert_eq!(backoff(2), Duration::from_secs(120));
        assert_eq!(backoff(5), MOST);
        assert_eq!(backoff(u32::MAX), MOST);
    }

    #[test]
    fn a_notice_opens_only_the_apps_own_pages() {
        assert_eq!(
            opens("/app?next=%2Fnotifications").as_deref(),
            Some("/app?next=%2Fnotifications")
        );
        assert_eq!(
            opens("/app/acme/usage?month=2026-10").as_deref(),
            Some("/app/acme/usage?month=2026-10")
        );
        for outside in ["https://evil.example/app", "//evil.example/app", "/apps", "/", ""] {
            assert_eq!(opens(outside), None, "{outside}");
        }
    }

    #[test]
    fn an_item_becomes_a_notice_with_its_body_when_it_has_one() {
        let report = json!({
            "deliveryId": "ndl_1", "kind": "usage.month_report", "eventId": "nev_1",
            "title": "October 2026 usage: $63.00", "body": "3 model calls.", "path": "/app?next=%2Fusage%3Fmonth%3D2026-10"
        });
        assert_eq!(
            super::notice(&report),
            Some(Notice {
                title: "October 2026 usage: $63.00".into(),
                body: "3 model calls.".into(),
                opens: Some("/app?next=%2Fusage%3Fmonth%3D2026-10".into()),
            })
        );
        assert_eq!(super::notice(&json!({ "path": "/app" })), None, "no title, no notice");
    }

    #[test]
    fn gateways_answers_map_to_what_the_loop_does() {
        let core = |status: u16, code: Option<&str>| {
            nexus_client::Error::from(nexus_client::error::CoreError {
                status,
                message: String::new(),
                code: code.map(String::from),
                interval: None,
                details: None,
            })
        };
        assert_eq!(fail_of(&core(404, None)), Fail::Gone);
        assert_eq!(fail_of(&core(409, Some("device_limit"))), Fail::Limit);
        assert_eq!(fail_of(&core(409, Some("conflict"))), Fail::Other);
        assert_eq!(fail_of(&core(503, None)), Fail::Other);
        assert_eq!(
            fail_of(&nexus_client::Error::Transport(nexus_client::error::Transport::Timeout)),
            Fail::Other
        );
    }

    #[tokio::test]
    async fn sign_out_waits_for_a_registration_in_flight() {
        let shared = PathNotices::default();
        shared.set(Some("acme".into()), None);
        assert_eq!(
            shared.device_for("acme"),
            None,
            "no device yet: the old read gave up here"
        );
        let step = shared.busy.lock().await;
        let registering = async {
            let _step = step;
            tokio::time::sleep(Duration::from_millis(50)).await;
            shared.set(Some("acme".into()), Some("ndv_1".into()));
        };
        let ((), settled) = tokio::join!(registering, shared.settled_device("acme", SIGN_OUT_LIMIT));
        assert_eq!(
            settled.as_deref(),
            Some("ndv_1"),
            "the device registered while sign-out waited"
        );
    }

    #[tokio::test]
    async fn sign_out_gives_up_on_a_step_past_its_limit() {
        let shared = PathNotices::default();
        shared.set(Some("acme".into()), Some("ndv_1".into()));
        let _held = shared.busy.lock().await;
        assert_eq!(shared.settled_device("acme", Duration::from_millis(20)).await, None);
    }

    #[test]
    fn a_notice_shows_only_for_the_sign_in_it_was_collected_for() {
        assert!(still_signed_in(Some("gw|acme\ndesktop:1"), "gw|acme\ndesktop:1"));
        assert!(
            !still_signed_in(Some("gw|globex\ndesktop:2"), "gw|acme\ndesktop:1"),
            "switched"
        );
        assert!(!still_signed_in(None, "gw|acme\ndesktop:1"), "signed out");
    }

    #[test]
    fn the_device_token_is_made_once_and_kept_per_gateway() {
        let vault = Vault::Memory(Default::default());
        let first = device_token(&vault, "https://gw.example").expect("a token");
        assert_eq!(first.len(), 64);
        assert_eq!(device_token(&vault, "https://gw.example"), Some(first.clone()));
        assert_ne!(device_token(&vault, "https://other.example"), Some(first));
        assert!(
            vault.get("https://gw.example").is_none(),
            "the sign-in's entry is untouched"
        );
    }

    /// A fake Gateway on a local port: each call's answer is the next one queued for its method and path, else 404.
    /// Records every call and its body.
    mod fake_gateway {
        use std::collections::{HashMap, VecDeque};
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use serde_json::Value;

        type Answers = HashMap<String, VecDeque<(u16, Value)>>;

        pub struct Server {
            pub url: String,
            answers: Arc<Mutex<Answers>>,
            pub calls: Arc<Mutex<Vec<(String, Value)>>>,
        }

        impl Server {
            pub fn start() -> Server {
                let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
                let url = format!("http://{}", listener.local_addr().expect("address"));
                let answers = Arc::new(Mutex::new(Answers::new()));
                let calls = Arc::new(Mutex::new(Vec::new()));
                let (queued, called) = (answers.clone(), calls.clone());
                std::thread::spawn(move || {
                    for connection in listener.incoming() {
                        let Ok(mut connection) = connection else { return };
                        let _ = connection.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut reader = BufReader::new(&mut connection);
                        let mut first = String::new();
                        if reader.read_line(&mut first).is_err() {
                            continue;
                        }
                        let mut length = 0;
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':')
                                && name.eq_ignore_ascii_case("content-length")
                            {
                                length = value.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut body = vec![0; length];
                        let _ = reader.read_exact(&mut body);
                        let mut parts = first.split(' ');
                        let call = format!("{} {}", parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                        called
                            .lock()
                            .expect("calls")
                            .push((call.clone(), serde_json::from_slice(&body).unwrap_or(Value::Null)));
                        let (status, answer) = queued
                            .lock()
                            .expect("answers")
                            .get_mut(&call)
                            .and_then(VecDeque::pop_front)
                            .unwrap_or((404, serde_json::json!({ "error": { "message": "not found" } })));
                        let answer = answer.to_string();
                        let _ = write!(
                            connection,
                            "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                            answer.len()
                        );
                    }
                });
                Server { url, answers, calls }
            }

            /// Queue `answer` for the next `call` (`POST /v1/...`).
            pub fn answer(&self, call: &str, status: u16, answer: Value) {
                self.answers
                    .lock()
                    .expect("answers")
                    .entry(call.to_string())
                    .or_default()
                    .push_back((status, answer));
            }

            pub fn calls(&self) -> Vec<String> {
                self.calls
                    .lock()
                    .expect("calls")
                    .iter()
                    .map(|(call, _)| call.clone())
                    .collect()
            }

            pub fn body(&self, index: usize) -> Value {
                self.calls.lock().expect("calls")[index].1.clone()
            }
        }
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A shell signed in to the fake Gateway, with a memory keychain.
    struct TestHost {
        gateway: String,
        client: Client,
        key: Mutex<Option<String>>,
        vault: Vault,
        showing: AtomicBool,
        shown: Mutex<Vec<Notice>>,
    }

    impl Host for TestHost {
        fn key(&self) -> Option<String> {
            self.key.lock().expect("key").clone()
        }
        fn session(&self) -> Option<(String, Client)> {
            Some((self.gateway.clone(), self.client.clone()))
        }
        fn vault(&self) -> Vault {
            self.vault.clone()
        }
        fn showing(&self) -> bool {
            self.showing.load(Ordering::SeqCst)
        }
        fn show(&self, notice: Notice) {
            self.shown.lock().expect("shown").push(notice);
        }
    }

    impl TestHost {
        fn shown(&self) -> Vec<Notice> {
            self.shown.lock().expect("shown").clone()
        }
    }

    fn host(server: &fake_gateway::Server, key: Option<&str>) -> Arc<TestHost> {
        let mut options = nexus_client::ClientOptions::new(&server.url);
        options.api_key = Some("nexus-test-token".into());
        Arc::new(TestHost {
            gateway: server.url.clone(),
            client: Client::new(options),
            key: Mutex::new(key.map(String::from)),
            vault: Vault::Memory(Default::default()),
            showing: AtomicBool::new(true),
            shown: Mutex::new(Vec::new()),
        })
    }

    fn run(shared: &Arc<PathNotices>, host: &Arc<TestHost>) -> tokio::task::JoinHandle<()> {
        let (shared, host) = (shared.clone(), host.clone());
        tokio::spawn(async move { shared.run(&*host).await })
    }

    /// Waits up to five seconds for `done`.
    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    const REGISTER: &str = "POST /v1/notifications/devices";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_loop_registers_shows_registers_again_after_a_404_and_sign_out_revokes() {
        let server = fake_gateway::Server::start();
        server.answer(REGISTER, 201, json!({ "id": "ndv_1" }));
        server.answer(
            "POST /v1/notifications/devices/ndv_1/collect",
            200,
            json!({ "items": [
                { "deliveryId": "ndl_1", "title": "Payment failed", "body": "Update the card.", "path": "/app?next=%2Fbilling" },
                { "deliveryId": "ndl_2", "body": "no title: not shown" },
            ] }),
        );
        // The second collect finds no answer (404: Gateway revoked the device), so the loop registers again.
        server.answer(REGISTER, 201, json!({ "id": "ndv_2" }));
        server.answer(
            "POST /v1/notifications/devices/ndv_2/collect",
            200,
            json!({ "items": [{ "title": "Access request", "path": "https://evil.example/app" }] }),
        );
        let host = host(&server, Some("gw|acme\ndesktop:1"));
        let shared = Arc::new(PathNotices::default());
        let running = run(&shared, &host);
        until(|| host.shown().len() == 1).await;
        assert_eq!(
            host.shown()[0],
            Notice {
                title: "Payment failed".into(),
                body: "Update the card.".into(),
                opens: Some("/app?next=%2Fbilling".into()),
            }
        );
        // The registration carries this computer's kept token.
        let token = host.vault.get(&format!("{}#notify-device", server.url)).expect("kept");
        assert_eq!(server.body(0), json!({ "platform": "desktop", "token": token }));

        // The nudges skip the waits: the 404, then the registration again.
        shared.nudge();
        until(|| server.calls().len() >= 3).await;
        shared.nudge();
        until(|| host.shown().len() == 2).await;
        assert_eq!(host.shown()[1].opens, None, "only the app's own pages open");
        assert_eq!(
            server.calls()[..5],
            [
                REGISTER,
                "POST /v1/notifications/devices/ndv_1/collect",
                "POST /v1/notifications/devices/ndv_1/collect",
                REGISTER,
                "POST /v1/notifications/devices/ndv_2/collect",
            ]
        );
        assert_eq!(
            server.body(3)["token"],
            json!(token),
            "the same token, so the same device"
        );

        // Sign-out revokes the device registered last, while the credential still works.
        server.answer(
            "DELETE /v1/notifications/devices/ndv_2?reason=signed_out",
            200,
            json!({}),
        );
        shared.sign_out(host.key().as_deref(), &host.client).await;
        assert_eq!(
            server.calls().last().map(String::as_str),
            Some("DELETE /v1/notifications/devices/ndv_2?reason=signed_out")
        );
        running.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn muted_still_collects_and_too_many_devices_stops_until_a_new_sign_in() {
        let server = fake_gateway::Server::start();
        server.answer(REGISTER, 201, json!({ "id": "ndv_1" }));
        server.answer(
            "POST /v1/notifications/devices/ndv_1/collect",
            200,
            json!({ "items": [{ "title": "Payment failed" }] }),
        );
        let host = host(&server, Some("gw|acme\ndesktop:1"));
        host.showing.store(false, Ordering::SeqCst);
        let shared = Arc::new(PathNotices::default());
        let running = run(&shared, &host);
        until(|| server.calls().len() == 2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(host.shown().is_empty(), "muted: collected, not shown");

        // Another organization, and too many devices: one registration, then none until the next sign-in.
        server.answer(
            REGISTER,
            409,
            json!({ "error": { "code": "device_limit", "message": "Too many devices." } }),
        );
        *host.key.lock().expect("key") = Some("gw|globex\ndesktop:2".into());
        shared.nudge();
        until(|| server.calls().len() == 3).await;
        shared.nudge();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(server.calls().len(), 3, "stopped for this sign-in");
        running.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_sign_in_nothing_is_called_or_revoked() {
        let server = fake_gateway::Server::start();
        let host = host(&server, None);
        let shared = Arc::new(PathNotices::default());
        let running = run(&shared, &host);
        shared.nudge();
        tokio::time::sleep(Duration::from_millis(100)).await;
        shared.sign_out(None, &host.client).await;
        shared.sign_out(Some("gw|acme\ndesktop:1"), &host.client).await;
        assert!(server.calls().is_empty(), "{:?}", server.calls());
        running.abort();
    }
}
