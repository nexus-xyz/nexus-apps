//! Approve keeps "Approving…" until the list shows the device.

#[path = "../budgets_test_support.rs"]
#[allow(dead_code)] // The shared fixture has fields this page does not read.
mod support;

use super::{DevicesPage, Key, Load, Verb, device_code, read_devices, read_pairing_input};
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use nexus_client::e2e::{self, DeviceKey};
use serde_json::{Value, json};

fn row(id: &str, key: &DeviceKey, state: &str) -> Value {
    json!({
        "id": id, "name": format!("Device {id}"),
        "encryptionKey": e2e::b64u(&key.encryption_key()), "signingKey": e2e::b64u(&key.signing_key()),
        "state": state, "approvedBy": null, "approvalSignature": null,
        "createdAt": "2026-11-15T10:00:00Z", "approvedAt": null, "revokedAt": null
    })
}

/// The Devices page on a trusted device, with a pending device found by its code.
fn page_with_a_found_device(
    cx: &mut TestAppContext,
) -> (
    Entity<DevicesPage>,
    &mut VisualTestContext,
    std::sync::Arc<support::backend::BackendFixture>,
) {
    support::init(cx);
    let gateway = support::backend::BackendFixture::new();
    let mine = DeviceKey::generate();
    let new = DeviceKey::generate();
    let devices =
        read_devices(&json!({ "devices": [row("dev_me", &mine, "trusted"), row("dev_new", &new, "pending")] }));
    let input = read_pairing_input(&device_code(&devices[1])).expect("the new device's code");
    let found = Some((devices[1].clone(), input));
    let fixture = gateway.clone();
    let (page, cx) = cx.add_window_view(move |window, cx| DevicesPage {
        ctx: support::ctx(window, fixture),
        key: Load::Ready(Key::new(mine)),
        devices: Load::Ready(devices),
        name: cx.new(|cx| InputState::new(window, cx)),
        code: cx.new(|cx| InputState::new(window, cx)),
        found,
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
    (page, cx, gateway)
}

#[gpui_kit::test]
fn approving_lasts_until_the_list_shows_the_device(cx: &mut TestAppContext) {
    let (page, cx, gateway) = page_with_a_found_device(cx);
    page.update_in(cx, |page, window, cx| page.approve(window, cx));
    cx.run_until_parked();
    // The handover reads who is signed in first. With no person, the approval goes without one.
    support::answer(gateway.take("/v1/auth/whoami"), json!({}), cx);

    let approve = gateway.take("/v1/devices/dev_new/approve");
    assert!(matches!(approve.verb, Verb::Post));
    assert!(
        approve.body.as_ref().is_some_and(|body| body.get("handover").is_none()),
        "no person, no key store: no handover"
    );
    support::answer(approve, json!({}), cx);
    page.update(cx, |page, _| {
        assert!(page.busy, "Approving… stays while the list reloads");
        assert!(page.found.is_some(), "the found device stays on screen with it");
        assert!(page.approve_outcome.is_none(), "the note waits for the list");
    });

    let list = gateway.take("/v1/devices");
    let mut approved = row("dev_new", &DeviceKey::generate(), "trusted");
    approved["approvedBy"] = "dev_me".into();
    support::answer(
        list,
        json!({ "devices": [row("dev_me", &DeviceKey::generate(), "trusted"), approved] }),
        cx,
    );
    page.update(cx, |page, _| {
        assert!(!page.busy, "the list arrived, so Approving… ends");
        assert!(page.found.is_none());
        let (note, error) = page.approve_outcome.clone().expect("the note shows");
        assert!(!error && note.contains("is trusted"), "{note}");
        let Load::Ready(devices) = &page.devices else {
            panic!("the list is read")
        };
        assert!(
            devices
                .iter()
                .any(|device| device.id == "dev_new" && device.state == "trusted"),
            "the device's row shows with the note"
        );
    });
}

#[gpui_kit::test]
fn a_refused_approval_ends_at_once(cx: &mut TestAppContext) {
    let (page, cx, gateway) = page_with_a_found_device(cx);
    page.update_in(cx, |page, window, cx| page.approve(window, cx));
    cx.run_until_parked();
    // The handover reads who is signed in first. With no person, the approval goes without one.
    support::answer(gateway.take("/v1/auth/whoami"), json!({}), cx);

    let approve = gateway.take("/v1/devices/dev_new/approve");
    let consumed = approve.consumed.clone();
    approve
        .answer
        .send(Err(nexus_client::error::Transport::Other("refused".into()).into()))
        .unwrap();
    cx.run_until_parked();
    assert!(consumed.load(std::sync::atomic::Ordering::SeqCst));
    page.update(cx, |page, _| {
        assert!(!page.busy);
        let (note, error) = page.approve_outcome.clone().expect("the refusal shows");
        assert!(error && note.starts_with("The device wasn't approved"), "{note}");
    });
}
