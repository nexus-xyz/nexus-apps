//! A signed-in computer registers itself once in gateway mode, and does nothing in approvals mode.

#[path = "../budgets_test_support.rs"]
#[allow(dead_code)] // The shared fixture has fields these tests do not read.
mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{Registration, register};
use crate::gateway::Verb;
use crate::workspace::gateway_access::AccountGateway;
use gpui_kit::TestAppContext;
use nexus_client::e2e::{self, DeviceKey};
use serde_json::{Value, json};
use support::backend::{BackendFixture, Pending};

struct Run {
    gateway: Arc<BackendFixture>,
    /// How often session sync was told that the device key changed.
    handed: Rc<Cell<u32>>,
    /// Whether the keychain was read.
    keyed: Arc<AtomicBool>,
    outcome: Rc<RefCell<Option<Result<Registration, String>>>>,
}

fn init(cx: &mut TestAppContext) {
    support::init(cx);
    // The keychain work runs on its own thread (`off_thread`), outside the test scheduler.
    cx.executor().allow_parking();
}

/// Start the sign-in's registration with `key` as this computer's stored key (`made`: new now).
fn start(cx: &mut TestAppContext, key: DeviceKey, made: bool, local_cache: bool) -> Run {
    let gateway = BackendFixture::new();
    let handed = Rc::new(Cell::new(0));
    let keyed = Arc::new(AtomicBool::new(false));
    let outcome = Rc::new(RefCell::new(None));
    let (counted, read, done, fixture) = (handed.clone(), keyed.clone(), outcome.clone(), gateway.clone());
    cx.update(move |cx| {
        let task = register(
            AccountGateway::fixture(fixture),
            local_cache,
            move || {
                read.store(true, Ordering::SeqCst);
                Ok((key, made))
            },
            Some(Rc::new(move || counted.set(counted.get() + 1))),
            cx,
        );
        cx.spawn(async move |_| *done.borrow_mut() = Some(task.await)).detach();
    });
    cx.run_until_parked();
    Run {
        gateway,
        handed,
        keyed,
        outcome,
    }
}

fn listed(key: &DeviceKey, state: &str) -> Value {
    json!({
        "id": "dev_me", "name": "Studio Mac",
        "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
        "state": state, "approvedBy": null, "approvalSignature": null,
        "createdAt": "2026-11-20T10:00:00Z", "approvedAt": null, "revokedAt": null
    })
}

/// Wait for the registration the page or the hook sends, past the keychain's own thread.
fn posted(gateway: &BackendFixture, cx: &mut TestAppContext) -> Option<Pending> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let mut pending = gateway.pending.lock().unwrap();
        if let Some(index) = pending
            .iter()
            .position(|request| request.path == "/v1/devices" && matches!(request.verb, Verb::Post))
        {
            return Some(pending.remove(index));
        }
        drop(pending);
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait until the registration ends, past the keychain's own thread.
fn finished(run: &Run, cx: &mut TestAppContext) -> Result<Registration, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if let Some(outcome) = run.outcome.borrow().clone() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "the registration did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn gateway_mode_registers_a_new_computer_and_hands_its_key_to_session_sync(cx: &mut TestAppContext) {
    init(cx);
    let key = DeviceKey::generate();
    let encryption_key = e2e::b64u(&key.encryption_key());
    let run = start(cx, key, true, true);
    let directory = run.gateway.take("/v1/devices");
    assert!(matches!(directory.verb, Verb::Get));
    support::answer(directory, json!({ "trust": "gateway", "devices": [] }), cx);

    let registration = posted(&run.gateway, cx).expect("the computer registers with no click");
    let body = registration.body.clone().expect("a registration body");
    assert_eq!(body["encryptionKey"], encryption_key);
    assert!(body["signingKey"].as_str().is_some_and(|key| !key.is_empty()));
    assert!(body["name"].as_str().is_some_and(|name| !name.is_empty()));
    assert_eq!(run.handed.get(), 0, "session sync takes the key once Gateway answers");
    support::answer(registration, json!({ "id": "dev_me", "state": "trusted" }), cx);

    assert_eq!(finished(&run, cx), Ok(Registration::Registered));
    assert_eq!(run.handed.get(), 1, "session sync takes up the new key once");
    assert!(run.gateway.pending.lock().unwrap().is_empty(), "nothing else is sent");
}

#[gpui_kit::test]
fn a_computer_gateway_lists_already_registers_nothing(cx: &mut TestAppContext) {
    init(cx);
    let key = DeviceKey::generate();
    let row = listed(&key, "trusted");
    let run = start(cx, key, false, true);
    support::answer(
        run.gateway.take("/v1/devices"),
        json!({ "trust": "gateway", "devices": [row] }),
        cx,
    );
    assert_eq!(finished(&run, cx), Ok(Registration::Listed));
    assert!(run.gateway.pending.lock().unwrap().is_empty(), "no second registration");
    assert_eq!(run.handed.get(), 0);
}

#[gpui_kit::test]
fn a_revoked_computer_stays_revoked(cx: &mut TestAppContext) {
    init(cx);
    let key = DeviceKey::generate();
    let row = listed(&key, "revoked");
    let run = start(cx, key, false, true);
    support::answer(
        run.gateway.take("/v1/devices"),
        json!({ "trust": "gateway", "devices": [row] }),
        cx,
    );
    assert_eq!(finished(&run, cx), Ok(Registration::Listed));
    assert!(run.gateway.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn approvals_mode_changes_nothing_and_reads_no_key(cx: &mut TestAppContext) {
    init(cx);
    for directory in [json!({ "trust": "approvals", "devices": [] }), json!({ "devices": [] })] {
        let run = start(cx, DeviceKey::generate(), true, true);
        support::answer(run.gateway.take("/v1/devices"), directory, cx);
        assert_eq!(finished(&run, cx), Ok(Registration::Approvals));
        assert!(!run.keyed.load(Ordering::SeqCst), "the keychain is not read");
        assert!(run.gateway.pending.lock().unwrap().is_empty(), "nothing is registered");
        assert_eq!(run.handed.get(), 0);
    }
}

#[gpui_kit::test]
fn a_managed_computer_registers_nothing(cx: &mut TestAppContext) {
    init(cx);
    let run = start(cx, DeviceKey::generate(), true, false);
    support::answer(
        run.gateway.take("/v1/devices"),
        json!({ "trust": "gateway", "devices": [] }),
        cx,
    );
    assert_eq!(finished(&run, cx), Ok(Registration::Managed));
    assert!(!run.keyed.load(Ordering::SeqCst));
    assert!(run.gateway.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn a_refused_registration_still_hands_over_the_new_key(cx: &mut TestAppContext) {
    init(cx);
    let run = start(cx, DeviceKey::generate(), true, true);
    support::answer(
        run.gateway.take("/v1/devices"),
        json!({ "trust": "gateway", "devices": [] }),
        cx,
    );
    let registration = posted(&run.gateway, cx).expect("a registration");
    registration
        .answer
        .send(Err(nexus_client::error::Transport::Other("refused".into()).into()))
        .unwrap();
    assert!(finished(&run, cx).is_err());
    assert_eq!(run.handed.get(), 1, "the keychain holds the new key now");
}
