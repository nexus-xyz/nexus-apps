//! In gateway mode the open page registers this computer with no click, once; in approvals mode it waits for the
//! person, as before. A computer pinned to approvals follows Gateway's own trust only after the person opts in.

#[path = "../budgets_test_support.rs"]
#[allow(dead_code)] // The shared fixture has fields this page does not read.
mod support;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{DeviceTrust, DevicesPage, Key, Load, Verb};
use crate::pages::CodeAccess;
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use nexus_client::e2e::{self, DeviceKey};
use serde_json::{Value, json};
use support::backend::{BackendFixture, Pending};

struct Page<'a> {
    page: Entity<DevicesPage>,
    cx: &'a mut VisualTestContext,
    gateway: Arc<BackendFixture>,
    /// How often the page told session sync that the device key changed.
    handed: Rc<Cell<u32>>,
    key: (String, String),
}

/// The Devices page with this computer's key read and the directory not read yet.
fn open(cx: &mut TestAppContext) -> Page<'_> {
    support::init(cx);
    // The key store work runs on its own thread (`off_thread`), outside the test scheduler.
    cx.executor().allow_parking();
    let gateway = BackendFixture::new();
    let key = DeviceKey::generate();
    let keys = (e2e::b64u(&key.encryption_key()), e2e::b64u(&key.signing_key()));
    let fixture = gateway.clone();
    let handed = Rc::new(Cell::new(0));
    let counted = handed.clone();
    let (page, cx) = cx.add_window_view(move |window, cx| DevicesPage {
        ctx: {
            let mut ctx = support::ctx(window, fixture);
            ctx.device.code = Rc::new(move |_| CodeAccess {
                device_key_changed: Some({
                    let counted = counted.clone();
                    Rc::new(move || counted.set(counted.get() + 1))
                }),
                ..CodeAccess::default()
            });
            ctx
        },
        remote_computers: None,
        remote_preview: false,
        trust: DeviceTrust::default(),
        offered: DeviceTrust::default(),
        settling: false,
        gateway_waits: None,
        auto_registered: false,
        key: Load::Ready(Key::new(key)),
        devices: Load::Loading,
        name: cx.new(|cx| InputState::new(window, cx)),
        code: cx.new(|cx| InputState::new(window, cx)),
        found: None,
        busy: false,
        outcome: None,
        approve_outcome: None,
        confirm_revoke: None,
        copied_at: None,
        polling: false,
        phone_control: Load::Loading,
        control_busy: false,
        control_watched: true,
        link_invitation: None,
        link_candidate: None,
        link_busy: false,
        link_polling: false,
        link_outcome: None,
        approver: Load::Loading,
        org_key: Load::Loading,
        _subscriptions: Vec::new(),
    });
    page.update(cx, |page, cx| page.load(cx));
    cx.run_until_parked();
    Page {
        page,
        cx,
        gateway,
        handed,
        key: keys,
    }
}

fn take(gateway: &BackendFixture, post: bool) -> Option<Pending> {
    let mut pending = gateway.pending.lock().unwrap();
    let index = pending
        .iter()
        .position(|request| request.path == "/v1/devices" && matches!(request.verb, Verb::Post) == post)?;
    Some(pending.remove(index))
}

/// A person no other test signs in as, so each test has its own trust pin.
fn person() -> String {
    format!("user_{}", uuid::Uuid::new_v4().simple())
}

/// Answer the whoami the page reads the trust pin for, then wait past the key store's own thread until `done`.
fn answer_whoami(
    gateway: &BackendFixture,
    person: &str,
    page: &Entity<DevicesPage>,
    cx: &mut VisualTestContext,
    done: impl Fn(&DevicesPage) -> bool,
) {
    let whoami = {
        let mut pending = gateway.pending.lock().unwrap();
        let index = pending
            .iter()
            .position(|request| request.path == "/v1/auth/whoami")
            .expect("the page reads who the person is");
        pending.remove(index)
    };
    support::answer(whoami, json!({ "kind": "person", "userId": person }), cx);
    wait(page, cx, done);
}

fn wait(page: &Entity<DevicesPage>, cx: &mut VisualTestContext, done: impl Fn(&DevicesPage) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if page.read_with(cx, |page, _| done(page)) {
            return;
        }
        assert!(Instant::now() < deadline, "the key store did not answer");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn me(key: &(String, String), state: &str) -> Value {
    json!({ "id": "dev_me", "name": "Studio Mac", "encryptionKey": key.0, "signingKey": key.1, "state": state,
        "approvedBy": null, "approvalSignature": null, "createdAt": "2026-11-26T10:00:00Z",
        "approvedAt": "2026-11-26T10:00:00Z", "revokedAt": null })
}

fn phone() -> Value {
    let key = DeviceKey::generate();
    json!({ "id": "dev_phone", "name": "iPhone", "encryptionKey": e2e::b64u(&key.encryption_key()),
        "signingKey": e2e::b64u(&key.signing_key()), "state": "trusted", "approvedBy": null,
        "approvalSignature": null, "createdAt": "2026-11-20T10:00:00Z", "approvedAt": "2026-11-20T10:00:00Z",
        "revokedAt": null })
}

#[gpui_kit::test]
fn gateway_mode_registers_this_computer_with_no_click_once(cx: &mut TestAppContext) {
    let Page {
        page,
        cx,
        gateway,
        handed,
        key,
    } = open(cx);
    let directory = take(&gateway, false).expect("the page reads the directory");
    support::answer(directory, json!({ "trust": "gateway", "devices": [phone()] }), cx);
    // A new computer: no pin and no approvals remembered, so it follows Gateway's trust.
    answer_whoami(&gateway, &person(), &page, cx, |page| {
        page.trust == DeviceTrust::Gateway
    });

    let registration = take(&gateway, true).expect("the page registers this computer itself");
    let body = registration.body.clone().expect("a body");
    assert_eq!(
        (body["encryptionKey"].as_str(), body["signingKey"].as_str()),
        (Some(&*key.0), Some(&*key.1))
    );
    // The waiting state draws with no code, QR code or approval.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    support::answer(registration, me(&key, "trusted"), cx);
    assert_eq!(
        handed.get(),
        1,
        "session sync reads the list again with this computer in it"
    );

    let reread = take(&gateway, false).expect("the page reads the list again");
    support::answer(
        reread,
        json!({ "trust": "gateway", "devices": [phone(), me(&key, "trusted")] }),
        cx,
    );
    assert!(
        take(&gateway, true).is_none(),
        "a listed computer registers nothing more"
    );
    page.update(cx, |page, _| {
        assert!(page.me().is_some(), "this computer is listed");
        assert!(page.outcome.is_none(), "{:?}", page.outcome);
        let parts = page.parts();
        assert!(!parts.pairing && !parts.add_device && !parts.connection_code);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    refuse_the_rest(&gateway, cx);
}

#[gpui_kit::test]
fn a_refused_registration_is_not_sent_again_by_itself(cx: &mut TestAppContext) {
    let Page { page, cx, gateway, .. } = open(cx);
    let person = person();
    support::answer(
        take(&gateway, false).expect("the directory"),
        json!({ "trust": "gateway", "devices": [] }),
        cx,
    );
    answer_whoami(&gateway, &person, &page, cx, |page| page.trust == DeviceTrust::Gateway);
    let registration = take(&gateway, true).expect("a registration");
    registration
        .answer
        .send(Err(nexus_client::error::Transport::Other("refused".into()).into()))
        .unwrap();
    cx.run_until_parked();
    support::answer(
        take(&gateway, false).expect("the list again"),
        json!({ "trust": "gateway", "devices": [] }),
        cx,
    );
    answer_whoami(&gateway, &person, &page, cx, |page| !page.settling);
    assert!(take(&gateway, true).is_none(), "the person presses Try again");
    page.update(cx, |page, _| {
        assert!(page.outcome.as_ref().is_some_and(|(_, _, error)| *error))
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    refuse_the_rest(&gateway, cx);
}

/// In approvals mode, or with no mode named, nothing registers by itself and pairing shows as before.
fn waits_for_the_person(directory: Value, cx: &mut TestAppContext) {
    let Page {
        page,
        cx,
        gateway,
        handed,
        ..
    } = open(cx);
    support::answer(take(&gateway, false).expect("the directory"), directory, cx);
    page.update(cx, |page, _| {
        assert_eq!(page.trust, DeviceTrust::Approvals);
        let parts = page.parts();
        assert!(parts.pairing && parts.add_device && parts.connection_code);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(take(&gateway, true).is_none(), "Request approval waits for a click");
    assert_eq!(handed.get(), 0);
    refuse_the_rest(&gateway, cx);
}

#[gpui_kit::test]
fn with_no_mode_named_the_page_waits_for_the_person(cx: &mut TestAppContext) {
    waits_for_the_person(json!({ "devices": [phone()] }), cx);
}

#[gpui_kit::test]
fn approvals_mode_waits_for_the_person(cx: &mut TestAppContext) {
    waits_for_the_person(json!({ "trust": "approvals", "devices": [phone()] }), cx);
}

#[gpui_kit::test]
fn a_computer_pinned_to_approvals_follows_gateway_trust_only_after_the_person_opts_in(cx: &mut TestAppContext) {
    let Page {
        page,
        cx,
        gateway,
        handed,
        key,
    } = open(cx);
    let person = person();
    // This computer read approvals before: the pin says approvals.
    let owner = super::approver::owner(
        gateway_url(&page, cx).as_str(),
        &json!({ "kind": "person", "userId": person }),
    )
    .expect("owner");
    let store = super::approver::store().expect("store");
    store
        .settle_trust(&owner, nexus_workbench::session_sync::DeviceTrust::Approvals)
        .expect("pinned");

    support::answer(
        take(&gateway, false).expect("the directory"),
        json!({ "trust": "gateway", "devices": [phone()] }),
        cx,
    );
    answer_whoami(&gateway, &person, &page, cx, |page| page.gateway_waits.is_some());
    page.update(cx, |page, _| {
        assert_eq!(
            page.trust,
            DeviceTrust::Approvals,
            "Gateway's answer alone does not move it"
        );
        let parts = page.parts();
        assert!(parts.pairing && parts.add_device && parts.connection_code);
    });
    assert!(take(&gateway, true).is_none(), "nothing registers by itself");
    assert_eq!(handed.get(), 0);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The person opts in on this computer.
    page.update(cx, |page, cx| page.trust_gateway(cx));
    wait(&page, cx, |page| page.trust == DeviceTrust::Gateway);
    page.update(cx, |page, _| assert!(page.gateway_waits.is_none()));
    assert_eq!(
        handed.get(),
        1,
        "session sync reads the list again and follows Gateway's trust"
    );
    let registration = take(&gateway, true).expect("then the page registers this computer");
    assert_eq!(
        registration.body.as_ref().and_then(|body| body["signingKey"].as_str()),
        Some(&*key.1)
    );
    support::answer(registration, me(&key, "trusted"), cx);
    assert_eq!(
        store.settle_trust(&owner, nexus_workbench::session_sync::DeviceTrust::Gateway),
        Ok(nexus_workbench::session_sync::DeviceTrust::Gateway)
    );
    refuse_the_rest(&gateway, cx);
}

fn gateway_url(page: &Entity<DevicesPage>, cx: &mut VisualTestContext) -> String {
    page.read_with(cx, |page, _| page.ctx.gateway.url().to_string())
}

/// Every request still open gets a refusal, so no page task waits on a dropped answer when the test ends.
fn refuse_the_rest(gateway: &BackendFixture, cx: &mut VisualTestContext) {
    loop {
        let open: Vec<_> = gateway.pending.lock().unwrap().drain(..).collect();
        if open.is_empty() {
            return;
        }
        for request in open {
            request
                .answer
                .send(Err(nexus_client::error::Transport::Other("test ended".into()).into()))
                .ok();
        }
        cx.run_until_parked();
    }
}
