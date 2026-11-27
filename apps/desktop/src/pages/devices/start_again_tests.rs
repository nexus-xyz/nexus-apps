//! Start again on an expired pairing registers a new key. Gateway answers the expired key with its expired row.

#[path = "../budgets_test_support.rs"]
#[allow(dead_code)] // The shared fixture has fields this page does not read.
mod support;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{DevicesPage, Key, Load, Part, Verb, read_devices};
use crate::pages::CodeAccess;
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use nexus_client::e2e::{self, DeviceKey};
use serde_json::{Value, json};
use support::backend::{BackendFixture, Pending};

const NAME: &str = "Nexus for desktop on macOS";

fn row(id: &str, name: &str, key: &DeviceKey, state: &str) -> Value {
    json!({
        "id": id, "name": name,
        "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
        "state": state, "approvedBy": null, "approvalSignature": null,
        // Days ago: a pending row this old is an expired pairing.
        "createdAt": "2026-11-15T10:00:00Z", "approvedAt": null, "revokedAt": null
    })
}

struct Expired<'a> {
    page: Entity<DevicesPage>,
    cx: &'a mut VisualTestContext,
    gateway: Arc<BackendFixture>,
    /// How often the page told session sync that the device key changed.
    handed: Rc<Cell<u32>>,
    /// The expired pairing's encryption key.
    old: String,
}

/// The Devices page on a computer whose pairing expired, with a trusted phone.
fn expired_pairing(cx: &mut TestAppContext) -> Expired<'_> {
    support::init(cx);
    // The keychain work runs on its own thread (`off_thread`), outside the test scheduler.
    cx.executor().allow_parking();
    let gateway = BackendFixture::new();
    let key = DeviceKey::generate();
    let old = e2e::b64u(&key.encryption_key());
    let devices = read_devices(&json!({ "devices": [
        row("dev_root", "iPhone", &DeviceKey::generate(), "trusted"),
        row("dev_me", NAME, &key, "pending"),
    ] }));
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
        key: Load::Ready(Key::new(key)),
        devices: Load::Ready(devices),
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
        remote_computers: None,
        remote_preview: false,
        trust: Default::default(),
        offered: Default::default(),
        settling: false,
        gateway_waits: None,
        auto_registered: false,
        approver: Load::Loading,
        org_key: Load::Loading,
        _subscriptions: Vec::new(),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    Expired {
        page,
        cx,
        gateway,
        handed,
        old,
    }
}

/// Press Start again, and wait for the registration it sends.
fn start_again(cx: &mut VisualTestContext, gateway: &BackendFixture) -> Pending {
    let button = cx.debug_bounds("devices-again").expect("Start again shows");
    cx.simulate_click(button.center(), Modifiers::default());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let posted = gateway
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == "/v1/devices" && matches!(request.verb, Verb::Post));
        if posted {
            return gateway.take("/v1/devices");
        }
        assert!(Instant::now() < deadline, "Start again registered nothing");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn start_again_registers_a_new_key_under_the_same_name(cx: &mut TestAppContext) {
    let Expired {
        page,
        cx,
        gateway,
        handed,
        old,
    } = expired_pairing(cx);
    let registered = start_again(cx, &gateway);
    let body = registered.body.clone().expect("a registration body");
    assert_eq!(body["name"], NAME);
    assert_ne!(
        body["encryptionKey"], old,
        "the expired key gets its expired row back, so it must not be sent again"
    );
    assert_eq!(
        handed.get(),
        1,
        "session sync, and the background service with it, takes the new key"
    );

    // Gateway stores the new key as a new pending device, and the page reads the list again.
    let fresh = json!({ "id": "dev_new", "name": NAME, "encryptionKey": body["encryptionKey"],
        "signingKey": body["signingKey"], "state": "pending", "approvedBy": null, "approvalSignature": null,
        "createdAt": "2099-01-01T00:00:00Z", "approvedAt": null, "revokedAt": null });
    support::answer(registered, fresh.clone(), cx);
    support::answer(gateway.take("/v1/devices"), json!({ "devices": [fresh] }), cx);
    page.update(cx, |page, _| {
        assert!(!page.busy, "the new pairing shows");
        assert!(page.outcome.is_none(), "{:?}", page.outcome);
    });
    refuse_the_rest(&gateway, cx);
}

#[gpui_kit::test]
fn a_refused_start_again_says_so_under_this_device(cx: &mut TestAppContext) {
    let Expired { page, cx, gateway, .. } = expired_pairing(cx);
    let registered = start_again(cx, &gateway);
    registered
        .answer
        .send(Err(nexus_client::error::Transport::Other("refused".into()).into()))
        .unwrap();
    cx.run_until_parked();
    page.update(cx, |page, _| {
        assert!(!page.busy);
        let (part, note, error) = page.outcome.clone().expect("the refusal shows");
        assert_eq!(
            part,
            Part::ThisDevice,
            "under the button that was pressed, not below All devices"
        );
        assert!(error && note.starts_with("This device wasn't set up"), "{note}");
    });
    refuse_the_rest(&gateway, cx);
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
